use crate::disk::DiskService;
use crate::handlers::{users_models::*, AppState};
use crate::quota::QuotaService;
use crate::storage::user_home_repo::UserHomeRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use chrono::Utc;
use std::sync::Arc;

async fn extract_session(state: &Arc<AppState>, headers: &HeaderMap) -> Option<String> {
    let token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if token.is_empty() {
        return None;
    }
    state.auth.get_username_from_token(token).await
}

// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
async fn require_admin(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Response> {
    let token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if token.is_empty() {
        return Err(ResponseHelper::json_error(
            "No session token provided".to_string(),
        ));
    }
    match state.auth.get_session(token).await {
        Ok(session) => {
            if !session.is_admin {
                return Err(ResponseHelper::json_error(
                    "Admin access required".to_string(),
                ));
            }
            Ok(session.username)
        }
        Err(_) => Err(ResponseHelper::json_unauthorized(
            "Invalid session".to_string(),
        )),
    }
}

/// Builds the absolute path for a user's home folder.
pub(crate) fn build_home_path(disk_mount_point: &str, username: &str) -> String {
    if disk_mount_point == "/home" || disk_mount_point.is_empty() {
        crate::config::paths::user_home_path(username)
            .to_string_lossy()
            .into_owned()
    } else {
        format!(
            "{}/users/{}",
            disk_mount_point.trim_end_matches('/'),
            username
        )
    }
}

/// Roots a home folder must never sit at or under (OWASP path-traversal / SSRF
/// hardening — an admin-supplied folder path is untrusted input).
const FORBIDDEN_HOME_ROOTS: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/proc", "/root",
    "/run", "/sbin", "/sys", "/usr", "/var",
];

/// Validates an admin-supplied absolute home-directory path and returns the
/// cleaned, symlink-resolved path. The leaf directory need not exist yet, but
/// its parent must be a real existing directory. Rejects relative paths,
/// `.`/`..` components, control characters, the filesystem root, every system
/// directory in `FORBIDDEN_HOME_ROOTS`, and anything inside the HomeVault data
/// directory.
pub(crate) fn validate_custom_home_path(raw: &str) -> Result<String, String> {
    let p = raw.trim();
    if p.is_empty() {
        return Err("Home directory path is empty".to_string());
    }
    if !p.starts_with('/') {
        return Err("Home directory path must be absolute".to_string());
    }
    if p.contains('\0') || p.contains('\n') || p.contains('\r') {
        return Err("Home directory path contains invalid characters".to_string());
    }

    let path = std::path::Path::new(p);
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::RootDir | Component::Normal(_) => {}
            _ => return Err("Home directory path must not contain '.' or '..'".to_string()),
        }
    }

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "Home directory path must name a directory".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Home directory path has no parent".to_string())?;

    // Canonicalize the parent so a symlink can't smuggle the real target
    // outside the checks below.
    let parent_real = std::fs::canonicalize(parent)
        .map_err(|_| format!("Parent directory '{}' does not exist", parent.display()))?;
    if !parent_real.is_dir() {
        return Err(format!("'{}' is not a directory", parent.display()));
    }

    let resolved = parent_real.join(file_name);
    let check = match std::fs::canonicalize(&resolved) {
        Ok(real) => {
            if !real.is_dir() {
                return Err(format!(
                    "'{}' exists and is not a directory",
                    resolved.display()
                ));
            }
            real
        }
        Err(_) => resolved,
    };
    let s = check.to_string_lossy().into_owned();

    if s == "/" {
        return Err("Home directory cannot be the filesystem root".to_string());
    }
    for root in FORBIDDEN_HOME_ROOTS {
        if s == *root || s.starts_with(&format!("{}/", root)) {
            return Err(format!(
                "Home directory cannot be placed under the system directory '{}'",
                root
            ));
        }
    }
    let data_dir = crate::config::paths::base_dir();
    let data_s = data_dir.to_string_lossy();
    let data_prefix = format!("{}/", data_s.trim_end_matches('/'));
    if s == data_s || s.starts_with(&data_prefix) {
        return Err(
            "Home directory cannot be placed inside the HomeVault data directory".to_string(),
        );
    }

    Ok(s)
}

/// The mounted partition with the longest mount-point that is a prefix of
/// `path` (so a home folder on a sub-mount is quota'd against that sub-mount,
/// not `/`).
fn containing_partition<'a>(
    path: &str,
    partitions: &'a [crate::disk::PartitionInfo],
) -> Option<&'a crate::disk::PartitionInfo> {
    partitions
        .iter()
        .filter(|p| {
            let mp = p.mount_point.trim_end_matches('/');
            mp.is_empty() // "/" trims to ""
                || path == p.mount_point
                || path.starts_with(&format!("{}/", mp))
        })
        .max_by_key(|p| p.mount_point.trim_end_matches('/').len())
}

/// Creates or removes a user's home folder on disk and updates the DB.
/// Called from both user_homes handlers and users handlers.
///
/// `size_bytes` is the hard disk-quota limit for the home directory; `None` or
/// `0` means no limit. On success returns `Ok(None)`, or `Ok(Some(warning))`
/// when the folder was saved but the size limit could not be enforced (the
/// target filesystem is not mounted with quota support).
pub(crate) async fn apply_home_folder(
    state: &Arc<AppState>,
    username: &str,
    enabled: bool,
    disk: Option<&str>,
    custom_path: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<Option<String>, String> {
    let repo = UserHomeRepository::new(state.db.pool());

    if enabled {
        // Verify the disk/partition exists
        let partitions = DiskService::list_partitions()
            .await
            .map_err(|e| format!("Failed to list partitions: {}", e))?;

        // Two ways to place the folder: an explicit picked directory (wins), or
        // a disk whose derived `<mount>/users/<name>` path is used. For a picked
        // directory `disk` is stored as NULL — that is the signal the frontend
        // uses to reopen it as a custom folder rather than a disk selection;
        // the quota still runs against the resolved containing `partition`.
        let picked = custom_path.map(str::trim).filter(|p| !p.is_empty());
        let (home_path, partition, disk_label): (
            String,
            crate::disk::PartitionInfo,
            Option<String>,
        ) = if let Some(raw) = picked {
            let home_path = validate_custom_home_path(raw)?;
            let partition = containing_partition(&home_path, &partitions)
                .cloned()
                .ok_or_else(|| {
                    format!("Could not determine which disk contains '{}'", home_path)
                })?;
            (home_path, partition, None)
        } else {
            let mount_point = disk.unwrap_or("/home");
            let partition = partitions
                .iter()
                .find(|p| p.mount_point == mount_point || p.device == mount_point)
                .cloned()
                .ok_or_else(|| {
                    format!("Disk/partition '{}' not found or not mounted", mount_point)
                })?;
            let home_path = build_home_path(&partition.mount_point, username);
            (home_path, partition, Some(mount_point.to_string()))
        };

        // Create the directory
        tokio::fs::create_dir_all(&home_path)
            .await
            .map_err(|e| format!("Failed to create home directory '{}': {}", home_path, e))?;

        // Get uid/gid of the user
        let uid_output = tokio::process::Command::new("id")
            .args(["-u", username])
            .output()
            .await
            .map_err(|e| format!("Failed to get UID for {}: {}", username, e))?;

        let gid_output = tokio::process::Command::new("id")
            .args(["-g", username])
            .output()
            .await
            .map_err(|e| format!("Failed to get GID for {}: {}", username, e))?;

        if !uid_output.status.success() || !gid_output.status.success() {
            return Err(format!(
                "User '{}' does not exist as a system user (cannot chown)",
                username
            ));
        }

        let uid = String::from_utf8_lossy(&uid_output.stdout)
            .trim()
            .to_string();
        let gid = String::from_utf8_lossy(&gid_output.stdout)
            .trim()
            .to_string();

        let chown_output = tokio::process::Command::new("chown")
            .args(["-R", &format!("{}:{}", uid, gid), &home_path])
            .output()
            .await
            .map_err(|e| format!("Failed to chown '{}': {}", home_path, e))?;

        if !chown_output.status.success() {
            let stderr = String::from_utf8_lossy(&chown_output.stderr);
            tracing::warn!("chown warning on '{}': {}", home_path, stderr);
        }

        // A positive limit is a real quota; None/0 means "unlimited".
        let limit = size_bytes.filter(|&b| b > 0);
        let mut warning: Option<String> = None;

        if let Some(limit) = limit {
            if (limit as u128) > partition.available_bytes as u128 {
                return Err(format!(
                    "Home directory size ({}) exceeds the free space available on '{}'",
                    QuotaService::format_size_kb(QuotaService::bytes_to_quota_kb(limit)),
                    partition.mount_point
                ));
            }
            if let Err(e) =
                QuotaService::set_user_home_quota(username, &partition.mount_point, limit)
            {
                tracing::warn!(
                    "Home-dir quota for user '{}' on '{}' could not be enforced: {}",
                    username,
                    partition.mount_point,
                    e
                );
                warning = Some(
                    "Home folder saved, but the size limit could not be enforced: the target \
                     filesystem is not mounted with disk-quota support."
                        .to_string(),
                );
            }
        } else {
            // Unlimited: best-effort clear of any quota left from a previous limit.
            let _ = QuotaService::set_user_home_quota(username, &partition.mount_point, 0);
        }

        let home = crate::storage::user_home_repo::UserHome {
            username: username.to_string(),
            enabled: true,
            disk: disk_label,
            path: Some(home_path.clone()),
            created_at: Utc::now(),
            size_limit_bytes: limit,
        };
        repo.upsert(&home)
            .await
            .map_err(|e| format!("Failed to save home config: {}", e))?;

        tracing::info!(
            "Home folder created for user '{}' at '{}'",
            username,
            home.path.as_ref().unwrap_or(&home_path)
        );

        return Ok(warning);
    } else {
        // Disable: remove from DB but keep the folder on disk
        if let Some(existing) = repo
            .find_by_username(username)
            .await
            .map_err(|e| e.to_string())?
        {
            if existing.enabled {
                let mut updated = existing;
                updated.enabled = false;
                updated.path = None;
                repo.upsert(&updated)
                    .await
                    .map_err(|e| format!("Failed to update home config: {}", e))?;
            }
        }
    }

    Ok(None)
}

/// Returns the home folder configuration for a user.
#[axum_macros::debug_handler]
pub async fn handle_user_home_get(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    if extract_session(&state, &headers).await.is_none() {
        return ResponseHelper::json_unauthorized("Invalid session".to_string());
    }

    let repo = UserHomeRepository::new(state.db.pool());
    match repo.find_by_username(&username).await {
        Ok(Some(home)) => ResponseHelper::json_success(serde_json::json!({
            "enabled": home.enabled,
            "disk": home.disk,
            "path": home.path,
            "created_at": home.created_at,
            "size_bytes": home.size_limit_bytes,
        })),
        Ok(None) => ResponseHelper::json_success(serde_json::json!({
            "enabled": false,
            "disk": (),
            "path": (),
            "created_at": (),
            "size_bytes": ()
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to get home config: {}", e)),
    }
}

/// Enables or disables a user's home folder, optionally on a specific disk.
#[axum_macros::debug_handler]
pub async fn handle_user_home_put(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<SetHomeFolderRequest>,
) -> Response {
    let _ = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };

    if let Some(sz) = req.size_bytes {
        if sz < 0 {
            return ResponseHelper::json_error(
                "Home directory size cannot be negative".to_string(),
            );
        }
    }

    if state.auth.user_exists(&username).await {
        tracing::debug!(
            "apply_home_folder: username={}, enabled={}, disk={:?}, path={:?}, size_bytes={:?}",
            username,
            req.enabled,
            req.disk,
            req.path,
            req.size_bytes
        );
        match apply_home_folder(
            &state,
            &username,
            req.enabled,
            req.disk.as_deref(),
            req.path.as_deref(),
            req.size_bytes,
        )
        .await
        {
            Ok(warning) => ResponseHelper::json_success(
                warning.unwrap_or_else(|| "Home folder updated".to_string()),
            ),
            Err(e) => ResponseHelper::json_error(e),
        }
    } else {
        ResponseHelper::json_error("User does not exist".to_string())
    }
}

/// Deletes a user's home folder from disk and removes the DB record.
#[axum_macros::debug_handler]
pub async fn handle_user_home_delete(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    let _ = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };

    let repo = UserHomeRepository::new(state.db.pool());

    // Get path from DB before deleting record
    let path = repo
        .find_by_username(&username)
        .await
        .ok()
        .flatten()
        .and_then(|h| h.path);

    // Delete the home directory from disk if it exists
    if let Some(ref home_path) = path {
        match tokio::fs::remove_dir_all(home_path).await {
            Ok(_) => {
                tracing::info!(
                    "Deleted home folder '{}' for user '{}'",
                    home_path,
                    username
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to delete home folder '{}' for user '{}': {}",
                    home_path,
                    username,
                    e
                );
                return ResponseHelper::json_error(format!(
                    "Failed to delete home folder '{}': {}",
                    home_path, e
                ));
            }
        }
    }

    match repo.delete(&username).await {
        Ok(_) => ResponseHelper::json_success("Home folder deleted".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete home config: {}", e)),
    }
}

/// A disk available for use as a user home folder.
#[derive(Debug, serde::Serialize)]
pub struct AvailableHomeDisksResponse {
    pub device: String,
    pub mount_point: String,
    pub filesystem: String,
    pub available_bytes: u64,
    pub total_bytes: u64,
}

/// Read-only filesystems that can be mounted on a partition but can never
/// hold a home folder. `squashfs` is the one that actually shows up on a
/// stock Ubuntu/Debian host: every snap package mounts a read-only squashfs
/// image on a loop device, so without this filter the picker offered
/// `/snap/core20/<rev>`-style mounts with 0 bytes free (confirmed live —
/// `lsblk` reported 16 of them on a dev host, and picking one left the size
/// slider with `min == max == 0`). `iso9660`/`erofs` are the same story for
/// mounted install media.
fn is_writable_home_filesystem(filesystem: &str) -> bool {
    !matches!(filesystem, "squashfs" | "iso9660" | "erofs")
}

/// Lists all mounted partitions that can be used as user home disks.
#[axum_macros::debug_handler]
pub async fn handle_available_home_disks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if extract_session(&state, &headers).await.is_none() {
        return ResponseHelper::json_unauthorized("Invalid session".to_string());
    }

    match DiskService::list_partitions().await {
        Ok(partitions) => {
            let disks: Vec<AvailableHomeDisksResponse> = partitions
                .into_iter()
                .filter(|p| is_writable_home_filesystem(&p.filesystem))
                .map(|p| AvailableHomeDisksResponse {
                    device: p.device,
                    mount_point: p.mount_point,
                    filesystem: p.filesystem,
                    available_bytes: p.available_bytes,
                    total_bytes: p.size_bytes,
                })
                .collect();
            ResponseHelper::json_success(disks)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list disks: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_writable_home_filesystem() {
        // Real writable filesystems stay offered.
        for fs in [
            "ext4", "xfs", "btrfs", "zfs", "f2fs", "vfat", "ntfs", "exfat", "unknown",
        ] {
            assert!(
                is_writable_home_filesystem(fs),
                "{fs} should be offered as a home disk"
            );
        }
        // Read-only mounts (snap squashfs images, install media) never can.
        for fs in ["squashfs", "iso9660", "erofs"] {
            assert!(
                !is_writable_home_filesystem(fs),
                "{fs} must never be offered as a home disk"
            );
        }
    }

    #[test]
    fn test_validate_custom_home_path_rejects_relative() {
        assert!(validate_custom_home_path("home/x").is_err());
        assert!(validate_custom_home_path("").is_err());
        assert!(validate_custom_home_path("   ").is_err());
    }

    #[test]
    fn test_validate_custom_home_path_rejects_traversal() {
        assert!(validate_custom_home_path("/home/../etc/passwd").is_err());
        assert!(validate_custom_home_path("/mnt/pool/../../etc").is_err());
        assert!(validate_custom_home_path("/..").is_err());
        // `std::path::Component` normalizes a bare `.` away, so `/home/./x`
        // is just `/home/x` — not a traversal and correctly accepted.
    }

    #[test]
    fn test_validate_custom_home_path_rejects_control_chars() {
        assert!(validate_custom_home_path("/home/x\ny").is_err());
        assert!(validate_custom_home_path("/home/x\0y").is_err());
    }

    #[test]
    fn test_validate_custom_home_path_rejects_root_and_system_dirs() {
        assert!(validate_custom_home_path("/").is_err());
        for root in FORBIDDEN_HOME_ROOTS {
            assert!(
                validate_custom_home_path(&format!("{}/homes/alice", root)).is_err(),
                "expected {} to be rejected",
                root
            );
        }
        // The system dir itself, not just a child.
        assert!(validate_custom_home_path("/etc").is_err());
    }

    #[test]
    fn test_validate_custom_home_path_rejects_missing_parent() {
        assert!(validate_custom_home_path("/nonexistent-xyz-123/deeper/home").is_err());
    }

    #[test]
    fn test_validate_custom_home_path_accepts_under_tmp() {
        // /tmp exists on every test host and is not a forbidden root.
        let dir = std::env::temp_dir();
        let p = dir.join("hv_home_test_leaf");
        let s = p.to_string_lossy().into_owned();
        let out = validate_custom_home_path(&s).expect("path under temp dir should validate");
        assert!(out.ends_with("hv_home_test_leaf"));
    }

    #[test]
    fn test_containing_partition_longest_prefix() {
        let parts = vec![
            crate::disk::PartitionInfo {
                device: "/dev/sda1".into(),
                mount_point: "/".into(),
                filesystem: "ext4".into(),
                size_bytes: 100,
                used_bytes: 10,
                available_bytes: 90,
                percent: 10,
                uuid: "a".into(),
            },
            crate::disk::PartitionInfo {
                device: "/dev/sdb1".into(),
                mount_point: "/mnt/pool".into(),
                filesystem: "xfs".into(),
                size_bytes: 200,
                used_bytes: 20,
                available_bytes: 180,
                percent: 10,
                uuid: "b".into(),
            },
        ];
        assert_eq!(
            containing_partition("/mnt/pool/users/x", &parts)
                .unwrap()
                .mount_point,
            "/mnt/pool"
        );
        assert_eq!(
            containing_partition("/home/x", &parts).unwrap().mount_point,
            "/"
        );
    }
}
