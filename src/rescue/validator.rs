/* =============================================================================
 * File          : validator.rs
 * Author        : Nicolas Le Roux
 * Author note  : security boundary — every rescue input passes here
 * Created       : 28/09/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 2.0.0
 *
 * Description:
 *   SecurityValidator: validates every block device, output directory and log
 *   file path before a rescue tool touches it — device allow-listing, path
 *   confinement to the rescue directories, symlink rejection, and the pure
 *   clone-safety rules (same device, size, in-use). This is the reason
 *   arbitrary recovery tools can be exposed through the API safely.
 *
 * Dependencies:
 *   - none (std::path / std::fs only)
 *
 * Usage:
 *   Called by rescue/service.rs before every tool invocation, and by
 *   handlers/rescue.rs before creating a progress job.
 * =============================================================================
 */

use super::models::RescueError;
use std::path::{Path, PathBuf};

/// SecurityValidator provides validation for file paths and block devices
/// to prevent path traversal attacks and unauthorized device access
pub struct SecurityValidator;

impl SecurityValidator {
    /// List of system devices that should never be modified through rescue operations
    const PROTECTED_DEVICES: &'static [&'static str] = &[
        "/dev/sda", // Common primary boot device
        "/dev/sda1",
        "/dev/sda2",
        "/dev/nvme0n1", // Common NVMe boot device
        "/dev/nvme0n1p1",
        "/dev/nvme0n1p2",
        "/dev/vda", // Common VM boot device
        "/dev/vda1",
    ];

    /// Allowed roots for rescue output: the two system temp directories and
    /// HomeVault's own root-owned rescue directory.
    pub fn allowed_output_dirs() -> Vec<String> {
        vec![
            "/tmp".to_string(),
            "/var/tmp".to_string(),
            crate::config::paths::rescue_dir()
                .to_string_lossy()
                .into_owned(),
        ]
    }

    /// `true` when `candidate` is the device `disk` itself or a partition of
    /// it (`sda1` of `sda`, `nvme0n1p2` of `nvme0n1`).
    ///
    /// A plain `str::starts_with` — what this used to do — also matched
    /// unrelated devices sharing a name prefix: `/dev/sdaa` and `/dev/sda1x`
    /// were "protected" (safe but wrong), while the real risk was the reverse
    /// direction: nothing checked that the prefix boundary was a partition
    /// separator, so a device named `/dev/sdb` was considered protected by a
    /// `/dev/sd` entry if one ever got added.
    fn is_same_or_partition_of(candidate: &str, disk: &str) -> bool {
        let Some(rest) = candidate.strip_prefix(disk) else {
            return false;
        };
        if rest.is_empty() {
            return true;
        }
        // sda1, vdb12 — partition numbers are appended directly.
        if rest.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
        // nvme0n1p1, mmcblk0p2 — separated by a literal 'p'.
        if let Some(digits) = rest.strip_prefix('p') {
            return !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
        }
        false
    }

    /// Resolves the parent disk of the currently-mounted root filesystem
    /// (e.g. `/dev/sdc3` -> `/dev/sdc`, `/dev/nvme1n1p2` -> `/dev/nvme1n1`),
    /// so the real boot disk is protected even when it isn't one of the
    /// common names in PROTECTED_DEVICES. Returns None if it can't be
    /// determined (e.g. `findmnt` unavailable) — callers must still fall
    /// back to the static list in that case.
    fn root_disk() -> Option<String> {
        let output = std::process::Command::new("findmnt")
            .args(["-n", "-o", "SOURCE", "/"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let source = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let name = source.strip_prefix("/dev/")?;
        if name.is_empty() {
            return None;
        }

        Some(format!("/dev/{}", Self::strip_partition_suffix(name)))
    }

    /// Strips a trailing partition number from a bare device name (no
    /// `/dev/` prefix). NVMe/mmcblk devices separate the partition with a
    /// 'p' (nvme0n1p2 -> nvme0n1, but a bare nvme0n1 has no such suffix and
    /// must be left alone); SATA/SCSI/virtio devices don't (sda1 -> sda).
    fn strip_partition_suffix(name: &str) -> String {
        if name.starts_with("nvme") || name.starts_with("mmcblk") {
            if let Some(pos) = name.rfind('p') {
                let after = &name[pos + 1..];
                if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
                    return name[..pos].to_string();
                }
            }
            name.to_string()
        } else {
            name.trim_end_matches(|c: char| c.is_ascii_digit())
                .to_string()
        }
    }

    /// Why `canonical` must not be touched, if it must not.
    ///
    /// Pure: takes the root disk as a parameter so the decision can be
    /// unit-tested without a real `findmnt`. Checks, in order, the live root
    /// disk (dynamic — catches boot devices that aren't named sda/nvme0n1/vda)
    /// and the static guess list.
    fn protected_reason(canonical: &str, root_disk: Option<&str>) -> Option<String> {
        if let Some(root) = root_disk {
            if Self::is_same_or_partition_of(canonical, root) {
                return Some(format!(
                    "Device '{}' is the active root disk and cannot be modified",
                    root
                ));
            }
        }
        for protected in Self::PROTECTED_DEVICES {
            if Self::is_same_or_partition_of(canonical, protected) {
                return Some(format!(
                    "Device '{}' is protected and cannot be modified",
                    protected
                ));
            }
        }
        None
    }

    /// Validates that a path points to a legitimate block device
    ///
    /// # Security checks:
    /// - Must be in /dev
    /// - Must be a block device (not a regular file)
    /// - Must not be the active root disk or in the protected devices list
    /// - Path traversal attempts are blocked via canonicalization
    pub fn validate_block_device(path: &str) -> Result<PathBuf, RescueError> {
        // Reject empty or obviously malicious paths — this also stops a value
        // that would be interpreted as an option (leading '-') by the tool.
        if path.is_empty() || path.contains('\0') {
            return Err(RescueError::Security(
                "Invalid device path: empty or contains null byte".to_string(),
            ));
        }
        if path.chars().any(|c| c.is_control()) || path.contains(char::is_whitespace) {
            return Err(RescueError::Security(
                "Invalid device path: contains whitespace or control characters".to_string(),
            ));
        }
        if !path.starts_with('/') {
            return Err(RescueError::Security(format!(
                "Invalid device path: '{}' is not absolute",
                path
            )));
        }

        let canonical = std::fs::canonicalize(path).map_err(|e| {
            RescueError::Security(format!("Cannot access device '{}': {}", path, e))
        })?;

        // Must be in /dev — component-wise, not `starts_with("/dev")` on the
        // string, which also accepted `/devfoo/...` and `/devil/x`.
        if !canonical.starts_with("/dev") || canonical == Path::new("/dev") {
            return Err(RescueError::Security(format!(
                "Device must be in /dev, got: {}",
                canonical.display()
            )));
        }

        let canonical_str = canonical.to_string_lossy().into_owned();
        if let Some(reason) = Self::protected_reason(&canonical_str, Self::root_disk().as_deref()) {
            return Err(RescueError::Security(reason));
        }

        // Verify it's actually a block device
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            let metadata = std::fs::metadata(&canonical).map_err(|e| {
                RescueError::Security(format!("Cannot access device metadata: {}", e))
            })?;

            if !metadata.file_type().is_block_device() {
                return Err(RescueError::Security(format!(
                    "'{}' is not a block device",
                    canonical.display()
                )));
            }
        }

        Ok(canonical)
    }

    /// `true` when `path` lies inside `root`, judged on path components —
    /// `/tmp-evil/x` is *not* inside `/tmp` (a plain `str::starts_with`
    /// accepted it, which is how a sibling directory named `/tmpfoo` became a
    /// valid rescue output target).
    fn path_is_within(path: &Path, root: &Path) -> bool {
        path.starts_with(root)
    }

    /// Canonicalise the deepest existing ancestor of `path` and rebuild the
    /// requested tail on top of it, so a symlinked directory in the middle of
    /// the path cannot redirect the write elsewhere (`/tmp/recovered` being a
    /// symlink to `/etc` used to be accepted as-is).
    fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf, String> {
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        let mut cursor = path.to_path_buf();

        loop {
            if cursor.exists() {
                let canonical = std::fs::canonicalize(&cursor)
                    .map_err(|e| format!("Cannot resolve '{}': {}", cursor.display(), e))?;
                let mut resolved = canonical;
                for component in tail.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            match (cursor.file_name(), cursor.parent()) {
                (Some(name), Some(parent)) => {
                    tail.push(name.to_os_string());
                    cursor = parent.to_path_buf();
                }
                _ => return Err(format!("Cannot resolve '{}'", path.display())),
            }
        }
    }

    /// Validates an output path for rescue operations and returns the **safe**
    /// path the caller must use.
    ///
    /// # Security checks:
    /// - Rejects empty paths, null bytes and control characters
    /// - Rejects `..` components
    /// - Confines the result to [`Self::allowed_output_dirs`] (component-wise)
    /// - Resolves symlinked parents, and refuses a final component that is
    ///   already a symlink or an existing non-directory/non-regular file
    pub fn validate_output_path(path: &str) -> Result<PathBuf, RescueError> {
        // Reject empty or obviously malicious paths
        if path.is_empty() || path.contains('\0') {
            return Err(RescueError::Security(
                "Invalid output path: empty or contains null byte".to_string(),
            ));
        }
        if path
            .chars()
            .any(|c| c.is_control() || c == '\n' || c == '\r')
        {
            return Err(RescueError::Security(
                "Invalid output path: contains control characters".to_string(),
            ));
        }

        // Block path traversal attempts (any component equal to ".." — a
        // substring test also rejected legitimate names like "a..b").
        if Path::new(path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(RescueError::Security(
                "Path traversal (..) is not allowed".to_string(),
            ));
        }

        let p = Path::new(path);
        // Relative paths are resolved under /tmp, as before.
        let absolute: PathBuf = if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new("/tmp").join(p)
        };

        let allowed = Self::allowed_output_dirs();

        // The path may be an allowed root itself (`/tmp`) — the generic
        // parent-based logic below can't express that, since its parent ("/")
        // is deliberately not allowed.
        for dir in &allowed {
            if absolute == Path::new(dir) {
                let canonical = std::fs::canonicalize(&absolute).map_err(|e| {
                    RescueError::Security(format!("Cannot resolve '{}': {}", dir, e))
                })?;
                return Ok(canonical);
            }
        }

        let (parent, file_name) = match (absolute.parent(), absolute.file_name()) {
            (Some(parent), Some(name)) => (parent.to_path_buf(), name.to_os_string()),
            _ => {
                return Err(RescueError::Security(
                    "Invalid path: no parent directory".to_string(),
                ))
            }
        };

        // Resolve the *parent* (following any symlink in it) but never the
        // final component: the whole point is to inspect the final component
        // with `symlink_metadata` and refuse to write through a link.
        let resolved_parent =
            Self::resolve_existing_ancestor(&parent).map_err(RescueError::Security)?;

        if !allowed
            .iter()
            .any(|dir| Self::path_is_within(&resolved_parent, Path::new(dir)))
        {
            return Err(RescueError::Security(format!(
                "Output path must be in one of: {}",
                allowed.join(", ")
            )));
        }

        let resolved = resolved_parent.join(&file_name);

        // The final component must not be a symlink (a planted
        // `/tmp/x -> /etc/shadow` would otherwise be written through), and an
        // existing target must be a directory or a regular file.
        if let Ok(meta) = std::fs::symlink_metadata(&resolved) {
            let ft = meta.file_type();
            if ft.is_symlink() {
                return Err(RescueError::Security(format!(
                    "'{}' is a symbolic link — refusing to write through it",
                    resolved.display()
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileTypeExt;
                if ft.is_block_device() || ft.is_char_device() || ft.is_fifo() || ft.is_socket() {
                    return Err(RescueError::Security(format!(
                        "'{}' is not a directory or regular file",
                        resolved.display()
                    )));
                }
            }
        }

        Ok(resolved)
    }

    /// Validates a log file path (special case of output path).
    pub fn validate_log_path(path: &str) -> Result<PathBuf, RescueError> {
        Self::validate_output_path(path)
    }

    /// Validates a log/mapfile path that is about to be **read** back (the
    /// ddrescue status endpoint).
    ///
    /// Stricter than [`Self::validate_log_path`] because nothing is created
    /// here: the path must already exist, be a regular file inside an allowed
    /// directory, be no symlink, and be no larger than `MAX_MAPFILE_BYTES`.
    /// Before this check the endpoint took the raw path segment from the URL
    /// and `read_to_string`'d it with the service's own privileges, i.e. an
    /// arbitrary-file read (and an existence oracle) for any admin session.
    pub fn validate_readable_log_path(path: &str) -> Result<PathBuf, RescueError> {
        const MAX_MAPFILE_BYTES: u64 = 16 * 1024 * 1024;

        if path.is_empty() || path.contains('\0') {
            return Err(RescueError::Security(
                "Invalid log path: empty or contains null byte".to_string(),
            ));
        }
        if path.chars().any(|c| c.is_control()) {
            return Err(RescueError::Security(
                "Invalid log path: contains control characters".to_string(),
            ));
        }
        if Path::new(path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(RescueError::Security(
                "Path traversal (..) is not allowed".to_string(),
            ));
        }

        let p = Path::new(path);
        if !p.is_absolute() {
            return Err(RescueError::Security(
                "Invalid log path: must be absolute".to_string(),
            ));
        }

        let (parent, file_name) = match (p.parent(), p.file_name()) {
            (Some(parent), Some(name)) => (parent.to_path_buf(), name.to_os_string()),
            _ => {
                return Err(RescueError::Security(
                    "Invalid log path: no parent directory".to_string(),
                ))
            }
        };

        let allowed = Self::allowed_output_dirs();
        let resolved_parent =
            Self::resolve_existing_ancestor(&parent).map_err(RescueError::Security)?;
        if !allowed
            .iter()
            .any(|dir| Self::path_is_within(&resolved_parent, Path::new(dir)))
        {
            return Err(RescueError::Security(format!(
                "Log file must be in one of: {}",
                allowed.join(", ")
            )));
        }

        // Inspect the file itself *without* following it: a mapfile that is a
        // symlink is not a mapfile, it is a request to read something else.
        let resolved = resolved_parent.join(&file_name);
        let meta = std::fs::symlink_metadata(&resolved)
            .map_err(|e| RescueError::JobNotFound(format!("{}: {e}", resolved.display())))?;
        if meta.file_type().is_symlink() {
            return Err(RescueError::Security(format!(
                "'{}' is a symbolic link — refusing to read through it",
                resolved.display()
            )));
        }
        if !meta.is_file() {
            return Err(RescueError::Security(format!(
                "'{}' is not a regular file",
                resolved.display()
            )));
        }
        if meta.len() > MAX_MAPFILE_BYTES {
            return Err(RescueError::Security(format!(
                "'{}' is too large to be a ddrescue mapfile ({} bytes)",
                resolved.display(),
                meta.len()
            )));
        }

        Ok(resolved)
    }

    /// Check if a device is in the protected list
    pub fn is_protected_device(path: &str) -> bool {
        Self::PROTECTED_DEVICES
            .iter()
            .any(|protected| Self::is_same_or_partition_of(path, protected))
    }
}

/// Everything [`check_clone_safety`] needs to know about a planned clone.
///
/// Deliberately free of I/O so the rules can be unit-tested: the caller
/// (`RescueService`) fills it in from `lsblk` and the validated paths.
#[derive(Debug, Clone)]
pub struct ClonePlan {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// Bytes, when `lsblk` could report them.
    pub source_size: Option<u64>,
    pub destination_size: Option<u64>,
    /// Non-empty when the destination is already mounted / in use (the reason).
    pub destination_in_use: Option<String>,
}

/// Rules that must hold before a **destructive** disk-to-disk clone starts.
///
/// Returns the warnings the caller should surface (checks that could not be
/// made, e.g. an unreadable size) — an empty vector means "everything was
/// verified".
pub fn check_clone_safety(plan: &ClonePlan) -> Result<Vec<String>, String> {
    if plan.source == plan.destination {
        return Err(format!(
            "Source and destination are the same device ({})",
            plan.source.display()
        ));
    }

    if let Some(reason) = &plan.destination_in_use {
        return Err(format!(
            "Destination {} is in use: {}. Unmount it before cloning onto it — a clone overwrites everything on the target.",
            plan.destination.display(),
            reason
        ));
    }

    let mut warnings = Vec::new();

    match (plan.source_size, plan.destination_size) {
        (Some(src), Some(dst)) if dst < src => {
            return Err(format!(
                "Destination {} ({} bytes) is smaller than the source {} ({} bytes) — the clone would be truncated",
                plan.destination.display(),
                dst,
                plan.source.display(),
                src
            ));
        }
        (Some(_), Some(_)) => {}
        _ => warnings.push(
            "Device sizes could not be read; the clone will fail if the destination is smaller than the source"
                .to_string(),
        ),
    }

    Ok(warnings)
}

#[cfg(test)]
mod partition_suffix_tests {
    use super::SecurityValidator;

    #[test]
    fn strips_sata_scsi_virtio_partition_numbers() {
        assert_eq!(SecurityValidator::strip_partition_suffix("sda1"), "sda");
        assert_eq!(SecurityValidator::strip_partition_suffix("sda"), "sda");
        assert_eq!(SecurityValidator::strip_partition_suffix("vda2"), "vda");
        assert_eq!(SecurityValidator::strip_partition_suffix("sdc10"), "sdc");
    }

    #[test]
    fn strips_nvme_and_mmcblk_partition_numbers_without_mangling_bare_disks() {
        assert_eq!(
            SecurityValidator::strip_partition_suffix("nvme0n1p2"),
            "nvme0n1"
        );
        // A bare namespace device (no partition) must be left untouched —
        // naively trimming trailing digits would wrongly turn this into
        // "nvme0n", which doesn't exist.
        assert_eq!(
            SecurityValidator::strip_partition_suffix("nvme0n1"),
            "nvme0n1"
        );
        assert_eq!(
            SecurityValidator::strip_partition_suffix("mmcblk0p1"),
            "mmcblk0"
        );
        assert_eq!(
            SecurityValidator::strip_partition_suffix("mmcblk0"),
            "mmcblk0"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protected_device_check() {
        assert!(SecurityValidator::is_protected_device("/dev/sda"));
        assert!(SecurityValidator::is_protected_device("/dev/sda1"));
        assert!(SecurityValidator::is_protected_device("/dev/nvme0n1"));
        assert!(!SecurityValidator::is_protected_device("/dev/sdb"));
        assert!(!SecurityValidator::is_protected_device("/dev/loop0"));
    }

    #[test]
    fn protected_matching_respects_the_device_name_boundary() {
        // A name that merely starts with a protected name is a different
        // device: `str::starts_with` used to conflate the two.
        assert!(!SecurityValidator::is_protected_device("/dev/sdaa"));
        assert!(!SecurityValidator::is_protected_device("/dev/sda1x"));
        assert!(!SecurityValidator::is_protected_device("/dev/vdab"));
        // …while real partitions of a protected disk still match.
        assert!(SecurityValidator::is_protected_device("/dev/sda12"));
        assert!(SecurityValidator::is_protected_device("/dev/nvme0n1p2"));
    }

    #[test]
    fn same_or_partition_of_handles_nvme_and_plain_names() {
        assert!(SecurityValidator::is_same_or_partition_of(
            "/dev/sdc", "/dev/sdc"
        ));
        assert!(SecurityValidator::is_same_or_partition_of(
            "/dev/sdc3",
            "/dev/sdc"
        ));
        assert!(!SecurityValidator::is_same_or_partition_of(
            "/dev/sdcd",
            "/dev/sdc"
        ));
        assert!(SecurityValidator::is_same_or_partition_of(
            "/dev/nvme1n1p3",
            "/dev/nvme1n1"
        ));
        assert!(!SecurityValidator::is_same_or_partition_of(
            "/dev/nvme1n1q3",
            "/dev/nvme1n1"
        ));
        assert!(!SecurityValidator::is_same_or_partition_of(
            "/dev/sdb", "/dev/sdc"
        ));
    }

    #[test]
    fn root_disk_protection_is_judged_on_the_whole_device() {
        // The live root disk is resolved dynamically; both the disk and its
        // partitions must be refused, an unrelated disk must not be.
        let reason = SecurityValidator::protected_reason("/dev/sdc3", Some("/dev/sdc"));
        assert!(reason.unwrap_or_default().contains("active root disk"));
        assert!(SecurityValidator::protected_reason("/dev/sdc", Some("/dev/sdc")).is_some());
        assert!(SecurityValidator::protected_reason("/dev/sdd", Some("/dev/sdc")).is_none());
        // An unknown root disk must not swallow everything.
        assert!(SecurityValidator::protected_reason("/dev/sdz", None).is_none());
    }

    #[test]
    fn test_validate_output_path_traversal() {
        let result = SecurityValidator::validate_output_path("/tmp/../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }

    #[test]
    fn test_validate_output_path_null_byte() {
        let result = SecurityValidator::validate_output_path("/tmp/test\0file");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_output_path_rejects_newline() {
        // A newline here would end up in an `/etc/fstab`-like consumer or a
        // shell argument; nothing in this app needs one.
        assert!(SecurityValidator::validate_output_path("/tmp/a\nb").is_err());
    }

    #[test]
    fn test_validate_output_path_empty() {
        let result = SecurityValidator::validate_output_path("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_output_path_valid_tmp() {
        let result = SecurityValidator::validate_output_path("/tmp/rescue_output.log");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_output_path_unauthorized() {
        let result = SecurityValidator::validate_output_path("/root/sensitive.log");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be in one of"));
    }

    #[test]
    fn output_confisment_is_component_wise_not_a_string_prefix() {
        // `/tmpfoo` is a sibling of `/tmp`, not a child — the old string
        // `starts_with("/tmp")` accepted it as an allowed output root.
        assert!(SecurityValidator::path_is_within(
            Path::new("/tmp/x"),
            Path::new("/tmp")
        ));
        assert!(!SecurityValidator::path_is_within(
            Path::new("/tmpfoo/x"),
            Path::new("/tmp")
        ));
        assert!(
            SecurityValidator::validate_output_path("/tmpfoo/hv_rescue_out.log").is_err(),
            "a sibling directory sharing the prefix must be refused"
        );
    }

    #[test]
    fn an_allowed_root_itself_is_a_valid_output_directory() {
        // PhotoRec may legitimately be pointed at /tmp itself.
        let resolved = SecurityValidator::validate_output_path("/tmp").unwrap();
        assert_eq!(resolved, Path::new("/tmp"));
    }

    #[test]
    fn relative_paths_are_resolved_under_tmp() {
        let resolved =
            SecurityValidator::validate_output_path("hv_rescue_relative_out.log").unwrap();
        assert!(resolved.starts_with("/tmp"));
        assert_eq!(
            resolved.file_name().unwrap().to_str().unwrap(),
            "hv_rescue_relative_out.log"
        );
    }

    #[test]
    fn output_path_refuses_to_write_through_a_symlink() {
        // A symlink planted in /tmp pointing at a root-owned file used to be
        // accepted as-is, so PhotoRec's `create_dir_all`/ddrescue's mapfile
        // write would follow it.
        let dir = std::env::temp_dir().join(format!("hv_rescue_symlink_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("link.log");
        std::os::unix::fs::symlink("/etc/shadow", &link).unwrap();

        let result = SecurityValidator::validate_output_path(&link.to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();

        let err = result.expect_err("writing through a symlink must be refused");
        assert!(err.to_string().contains("symbolic link"), "{err}");
    }

    #[test]
    fn output_path_resolves_a_symlinked_parent() {
        // `/tmp/x -> /etc` must not let a nested path escape to /etc.
        let dir = std::env::temp_dir().join(format!("hv_rescue_symdir_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("escape");
        std::os::unix::fs::symlink("/etc", &link).unwrap();

        let candidate = link.join("evil.log");
        let result = SecurityValidator::validate_output_path(&candidate.to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            result.is_err(),
            "a symlinked parent resolving outside the allowed roots must be refused"
        );
    }

    #[test]
    fn readable_log_path_requires_an_existing_regular_file_under_an_allowed_root() {
        let inside = std::env::temp_dir().join(format!("hv_rescue_map_{}.log", std::process::id()));
        std::fs::write(&inside, "# mapfile\n").unwrap();

        let ok = SecurityValidator::validate_readable_log_path(&inside.to_string_lossy());
        assert!(ok.is_ok(), "{ok:?}");

        // Outside the allowed roots.
        assert!(SecurityValidator::validate_readable_log_path("/etc/shadow").is_err());
        // Traversal, even if it would resolve inside a root.
        assert!(SecurityValidator::validate_readable_log_path("/tmp/../etc/shadow").is_err());
        // Relative.
        assert!(SecurityValidator::validate_readable_log_path("ddrescue.log").is_err());
        // A directory is not a mapfile.
        let dir = inside.parent().unwrap().to_path_buf();
        assert!(SecurityValidator::validate_readable_log_path(&dir.to_string_lossy()).is_err());

        std::fs::remove_file(&inside).ok();
    }

    #[test]
    fn readable_log_path_refuses_symlinks() {
        let dir = std::env::temp_dir().join(format!("hv_rescue_rlink_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("map.log");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();

        let result = SecurityValidator::validate_readable_log_path(&link.to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            result.is_err(),
            "reading /etc/passwd through a symlink must be refused"
        );
    }

    fn plan(src: &str, dst: &str) -> ClonePlan {
        ClonePlan {
            source: PathBuf::from(src),
            destination: PathBuf::from(dst),
            source_size: Some(1_000),
            destination_size: Some(2_000),
            destination_in_use: None,
        }
    }

    #[test]
    fn clone_safety_accepts_a_large_enough_unused_destination() {
        let warnings = check_clone_safety(&plan("/dev/sdb", "/dev/sdc")).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn clone_safety_rejects_the_same_device() {
        let err = check_clone_safety(&plan("/dev/sdb", "/dev/sdb")).unwrap_err();
        assert!(err.contains("same device"), "{err}");
    }

    #[test]
    fn clone_safety_rejects_a_smaller_destination() {
        let mut p = plan("/dev/sdb", "/dev/sdc");
        p.destination_size = Some(999);
        let err = check_clone_safety(&p).unwrap_err();
        assert!(err.contains("smaller than the source"), "{err}");
    }

    #[test]
    fn clone_safety_rejects_an_in_use_destination() {
        let mut p = plan("/dev/sdb", "/dev/sdc");
        p.destination_in_use = Some("/home (ext4)".to_string());
        let err = check_clone_safety(&p).unwrap_err();
        assert!(err.contains("in use"), "{err}");
        assert!(
            err.contains("/home (ext4)"),
            "the reason must be surfaced: {err}"
        );
    }

    #[test]
    fn clone_safety_warns_when_sizes_are_unknown() {
        let mut p = plan("/dev/sdb", "/dev/sdc");
        p.source_size = None;
        p.destination_size = None;
        let warnings = check_clone_safety(&p).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("could not be read"));
    }

    #[test]
    fn equal_sized_destination_is_allowed() {
        let mut p = plan("/dev/sdb", "/dev/sdc");
        p.destination_size = Some(1_000);
        assert!(check_clone_safety(&p).is_ok());
    }
}
