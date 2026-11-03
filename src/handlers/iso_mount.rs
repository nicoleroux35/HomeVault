/* =============================================================================
 * File          : iso_mount.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Read-only ISO/IMG mounting endpoints (phase B2): list what is mounted,
 *   mount a disc image under the appliance-managed `iso/` directory, and
 *   unmount one again.
 *
 *   Reads need a session; mounting and unmounting are **admin-only**, because
 *   both run root-owned `losetup`/`mount`/`umount` and because a mount point is
 *   a directory the whole appliance can then browse.
 *
 *   Per `docs/API.md`'s capability contract: a refusal that is about the
 *   *request* is `400`, a missing `losetup`/`mount` is `503`, and no route
 *   answers `400` because this host cannot do something.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::iso_mount, crate::handlers::AppState
 *
 * Usage:
 *   Routes under /api/explorer/iso in src/web/mod.rs; see
 *   docs/guides/iso-mount.md.
 * =============================================================================
 */

//! Read-only ISO/IMG mounting endpoints, see docs/guides/iso-mount.md.
//!
//! * `GET    /api/explorer/iso`          — mounted images + capability
//! * `POST   /api/explorer/iso`          — mount `{ path }` (admin)
//! * `DELETE /api/explorer/iso/{id}`     — unmount (admin)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::handlers::admin_guard::{require_admin, require_auth, require_session_token};
use crate::iso_mount::{self, ISO_DIR_NAME, LOSETUP_TIMEOUT, MOUNT_TIMEOUT, PROBE_TIMEOUT};
use crate::utils::ResponseHelper;

/// `GET /api/explorer/iso` — what is mounted, and whether this host can mount.
pub async fn handle_iso_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let available = tokio::task::spawn_blocking(iso_mount::is_available)
        .await
        .unwrap_or(false);

    let (iso_root, mounts) = match collect_mounts().await {
        Ok(pair) => pair,
        Err(message) => return ResponseHelper::json_error(message),
    };

    ResponseHelper::json_success(IsoListResponse {
        available,
        iso_root: iso_root.to_string_lossy().to_string(),
        mounts,
    })
}

/// `POST /api/explorer/iso` — mount an image read-only.
pub async fn handle_iso_mount(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    // The guard runs before the body is parsed, so an unauthenticated caller
    // can never probe the payload shape (P0.2).
    let req: MountRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    // `explorer_roots_for_session` takes the session, and the token is the only
    // way to resolve one — the username alone is not enough.
    let session_token = match require_session_token(&state, &headers).await {
        Ok(token) => token,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    // The roots the caller may read, from the one function the Explorer
    // sidebar uses — so a folder the sidebar would not show can never be
    // mounted, and an admin's mount is bounded the same way a search is.
    let roots = match super::explorer::explorer_roots_for_session(&state, &session, true).await {
        Ok(roots) => roots,
        Err(message) => return ResponseHelper::json_error(message),
    };
    let allowed: Vec<PathBuf> = roots.iter().map(|r| PathBuf::from(&r.path)).collect();

    let image = match resolve_image(&req.path, &allowed) {
        Ok(path) => path,
        Err(refusal) => return ResponseHelper::json_error(refusal.message()),
    };

    let (iso_root, existing) = match collect_mounts().await {
        Ok(pair) => pair,
        Err(message) => return ResponseHelper::json_error(message),
    };

    // Pre-flight before any syscall.
    if let Err(refusal) = iso_mount::check_mountable(&image, &allowed, &existing) {
        return ResponseHelper::json_error(refusal.message());
    }

    let mounted = match tokio::task::spawn_blocking(move || attach(&iso_root, &image)).await {
        Ok(Ok(mounted)) => mounted,
        Ok(Err(message)) => return ResponseHelper::json_unavailable(message),
        Err(e) => return ResponseHelper::json_internal_error(format!("Mount worker failed: {e}")),
    };

    crate::events::log_event(format!(
        "Admin '{}' mounted '{}' at {}",
        admin, mounted.label, mounted.mount_point
    ));

    ResponseHelper::json_success(mounted)
}

/// `DELETE /api/explorer/iso/{id}` — unmount one image.
pub async fn handle_iso_unmount(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let (iso_root, existing) = match collect_mounts().await {
        Ok(pair) => pair,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let Some(target) = existing.into_iter().find(|m| m.id == id) else {
        return ResponseHelper::json_error(format!("No ISO mount with id '{}'", id));
    };

    // The mount point comes from the HTTP path only as an id; the directory is
    // re-derived from it and re-checked, so a crafted id cannot name a path.
    let mount_point = PathBuf::from(&target.mount_point);
    if !iso_mount::is_inside_iso_root(&iso_root, &mount_point) {
        return ResponseHelper::json_error(
            "Refusing to unmount outside the ISO directory".to_string(),
        );
    }

    let loop_device = target.loop_device.clone();
    let result =
        tokio::task::spawn_blocking(move || detach(&mount_point, loop_device.as_deref())).await;

    match result {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{}' unmounted ISO '{}'",
                admin, target.label
            ));
            ResponseHelper::json_success(serde_json::json!({ "unmounted": id }))
        }
        Ok(Err(message)) => ResponseHelper::json_unavailable(message),
        Err(e) => ResponseHelper::json_internal_error(format!("Unmount worker failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MountRequest {
    /// Path of the `.iso`/`.img` file, as the Explorer shows it.
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct IsoListResponse {
    pub available: bool,
    pub iso_root: String,
    pub mounts: Vec<iso_mount::IsoMount>,
}

// ---------------------------------------------------------------------------
// Host work (blocking)
// ---------------------------------------------------------------------------

/// The directory ISO mounts live under.
fn iso_root() -> PathBuf {
    crate::config::paths::data_dir().join(ISO_DIR_NAME)
}

/// Resolve the request's path to an absolute image path, refusing anything
/// that escapes the caller's roots before it is even canonicalised.
fn resolve_image(path: &str, allowed: &[PathBuf]) -> Result<PathBuf, iso_mount::IsoRefusal> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(iso_mount::IsoRefusal::Missing);
    }
    let candidate = Path::new(trimmed);
    if !candidate.is_absolute() {
        return Err(iso_mount::IsoRefusal::OutsideRoots);
    }
    // A `..` in the request is refused outright: the roots check below would
    // pass on the lexical path while the kernel resolves elsewhere.
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(iso_mount::IsoRefusal::OutsideRoots);
    }
    let resolved = candidate
        .canonicalize()
        .map_err(|_| iso_mount::IsoRefusal::Missing)?;
    if !allowed.iter().any(|root| resolved.starts_with(root)) {
        return Err(iso_mount::IsoRefusal::OutsideRoots);
    }
    Ok(resolved)
}

/// Read `/proc/mounts` and the recorded metadata, and reconcile them.
async fn collect_mounts() -> Result<(PathBuf, Vec<iso_mount::IsoMount>), String> {
    let root = iso_root();
    let known = read_index(&root);
    let mounts = tokio::fs::read_to_string("/proc/mounts")
        .await
        .map_err(|e| format!("Cannot read /proc/mounts: {e}"))?;
    // The filesystem is the authority: a directory with no mount on it is not
    // a mount, and keeping it would show an empty folder as a mounted image.
    let live = iso_mount::mounts_under(&mounts, &root, &known)
        .into_iter()
        .cloned()
        .collect();
    Ok((root, live))
}

/// The metadata file listing what was mounted and where.
///
/// Only used to recover the *label*, the loop device and the mount point of a
/// mount that `/proc/mounts` already confirms; it is never trusted to claim a
/// mount exists. A corrupt file degrades to "no known labels", not to an error.
fn index_path(iso_root: &Path) -> PathBuf {
    iso_root.join("mounts.json")
}

fn read_index(iso_root: &Path) -> Vec<iso_mount::IsoMount> {
    std::fs::read_to_string(index_path(iso_root))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_index(iso_root: &Path, mounts: &[iso_mount::IsoMount]) -> Result<(), String> {
    std::fs::create_dir_all(iso_root)
        .map_err(|e| format!("Cannot create {}: {e}", iso_root.display()))?;
    let body = serde_json::to_string_pretty(mounts)
        .map_err(|e| format!("Cannot serialize the mount index: {e}"))?;
    std::fs::write(index_path(iso_root), body)
        .map_err(|e| format!("Cannot write the mount index: {e}"))
}

/// Attach `image` to a loop device and mount it read-only at `<iso_root>/<slug>`.
///
/// The mount point is **derived**, never taken from the request. On any failure
/// the loop device already attached is detached again, so a mount that fails
/// halfway does not leave a device leaked on the host.
fn attach(iso_root: &Path, image: &Path) -> Result<iso_mount::IsoMount, String> {
    if !iso_mount::is_available() {
        return Err(
            "Mounting requires 'losetup' and 'mount', which are not installed on this server"
                .to_string(),
        );
    }

    let id = iso_mount::slug_for(image);
    let mount_point = iso_root.join(&id);
    if !iso_mount::is_inside_iso_root(iso_root, &mount_point) {
        return Err("Refusing to mount outside the ISO directory".to_string());
    }

    std::fs::create_dir_all(&mount_point)
        .map_err(|e| format!("Cannot create {}: {e}", mount_point.display()))?;

    let device = iso_mount::run_sudo(&iso_mount::losetup_attach_args(image), LOSETUP_TIMEOUT)?;
    if device.is_empty() {
        return Err("losetup did not report a device".to_string());
    }

    if let Err(e) =
        iso_mount::run_sudo(&iso_mount::mount_args(&device, &mount_point), MOUNT_TIMEOUT)
    {
        // Do not leak the loop device when the mount itself is refused (an
        // image that is not a filesystem, a corrupt partition table, …).
        let _ = iso_mount::run_sudo(&iso_mount::losetup_detach_args(&device), PROBE_TIMEOUT);
        let _ = std::fs::remove_dir(&mount_point);
        return Err(e);
    }

    let size_bytes = std::fs::metadata(image).map(|m| m.len()).unwrap_or(0);
    let label = image
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| id.clone());

    let mounted = iso_mount::IsoMount {
        id: id.clone(),
        image_path: image.to_string_lossy().to_string(),
        label,
        mount_point: mount_point.to_string_lossy().to_string(),
        loop_device: Some(device),
        size_bytes,
    };

    let mut index = read_index(iso_root);
    index.retain(|m| m.mount_point != mounted.mount_point);
    index.push(mounted.clone());
    write_index(iso_root, &index)?;

    Ok(mounted)
}

/// Unmount `mount_point` and detach its loop device.
fn detach(mount_point: &Path, loop_device: Option<&str>) -> Result<(), String> {
    iso_mount::run_sudo(&iso_mount::umount_args(mount_point), MOUNT_TIMEOUT)?;

    if let Some(device) = loop_device {
        // Best effort: the mount is already gone, and a `losetup -d` that
        // fails leaves an unattached loop node the kernel recycles. Reporting
        // it as a failure would make a successful unmount look broken.
        let _ = iso_mount::run_sudo(&iso_mount::losetup_detach_args(device), PROBE_TIMEOUT);
    }

    // Drop the now-empty directory and its index row.
    let _ = std::fs::remove_dir(mount_point);
    if let Some(iso_root) = mount_point.parent() {
        let mut index = read_index(iso_root);
        index.retain(|m| Path::new(&m.mount_point) != mount_point);
        let _ = write_index(iso_root, &index);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relative path is refused: the Explorer always sends an absolute one,
    /// and a relative path would be resolved against the server's cwd.
    #[test]
    fn resolve_image_refuses_a_relative_path() {
        let roots = vec![PathBuf::from("/data")];
        assert_eq!(
            resolve_image("isos/a.iso", &roots),
            Err(iso_mount::IsoRefusal::OutsideRoots)
        );
        assert_eq!(
            resolve_image("", &roots),
            Err(iso_mount::IsoRefusal::Missing)
        );
    }

    /// A `..` is refused before `canonicalize`, so a lexical path that would
    /// pass the containment test cannot be used to reach outside a root.
    #[test]
    fn resolve_image_refuses_a_parent_traversal() {
        let roots = vec![PathBuf::from("/data")];
        assert_eq!(
            resolve_image("/data/../etc/shadow.iso", &roots),
            Err(iso_mount::IsoRefusal::OutsideRoots)
        );
    }

    #[test]
    fn resolve_image_refuses_a_missing_file() {
        let roots = vec![PathBuf::from("/data")];
        assert_eq!(
            resolve_image("/data/definitely-absent.iso", &roots),
            Err(iso_mount::IsoRefusal::Missing)
        );
    }

    #[test]
    fn resolve_image_accepts_a_real_file_inside_a_root() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("a.iso");
        std::fs::write(&image, b"x").unwrap();
        let roots = vec![dir.path().to_path_buf()];
        let resolved = resolve_image(&image.to_string_lossy(), &roots).unwrap();
        assert!(resolved.starts_with(dir.path()));
    }

    #[test]
    fn the_mount_point_is_derived_from_the_slug_not_the_request() {
        let iso_root = Path::new("/var/lib/homevault/iso");
        let image = Path::new("/data/My Disc.iso");
        let id = iso_mount::slug_for(image);
        let mount_point = iso_root.join(&id);
        // Deterministic, and contained — the two properties the handler relies
        // on when it re-checks before unmounting.
        assert!(iso_mount::is_inside_iso_root(iso_root, &mount_point));
        assert_eq!(mount_point, iso_root.join(iso_mount::slug_for(image)));
    }
}
