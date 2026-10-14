/* =============================================================================
 * File          : validator.rs
 * Author        : Nicolas Le Roux
 * Author note  : security boundary — every rescue input passes here
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SecurityValidator: validates every file path and block device before a
 *   rescue tool touches it — path-traversal rejection, device allow-listing
 *   and existence checks. The reason arbitrary recovery tools can be
 *   exposed through the API safely.
 *
 * Dependencies:
 *   - none (std::path only)
 *
 * Usage:
 *   Called by rescue/service.rs before every tool invocation.
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

    /// Allowed output directories for rescue operations
    fn allowed_output_dirs() -> Vec<String> {
        vec![
            "/tmp".to_string(),
            "/var/tmp".to_string(),
            crate::config::paths::rescue_dir()
                .to_string_lossy()
                .into_owned(),
        ]
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

    /// Validates that a path points to a legitimate block device
    ///
    /// # Security checks:
    /// - Must be in /dev
    /// - Must be a block device (not a regular file)
    /// - Must not be in the protected devices list
    /// - Path traversal attempts are blocked via canonicalization
    pub fn validate_block_device(path: &str) -> Result<PathBuf, RescueError> {
        // Reject empty or obviously malicious paths
        if path.is_empty() || path.contains('\0') {
            return Err(RescueError::Security(
                "Invalid device path: empty or contains null byte".to_string(),
            ));
        }

        let p = Path::new(path);

        // Canonicalize to resolve symlinks and get absolute path
        let canonical = std::fs::canonicalize(p).map_err(|e| {
            RescueError::Security(format!("Cannot access device '{}': {}", path, e))
        })?;

        // Must be in /dev
        if !canonical.starts_with("/dev") {
            return Err(RescueError::Security(format!(
                "Device must be in /dev, got: {}",
                canonical.display()
            )));
        }

        // Check if device is the real, currently-booted root disk — this
        // catches deployments where the boot device isn't literally named
        // sda/nvme0n1/vda, which the static PROTECTED_DEVICES guess below
        // would otherwise miss entirely.
        let canonical_str = canonical.to_string_lossy();
        if let Some(root_disk) = Self::root_disk() {
            if canonical_str.starts_with(&root_disk) {
                return Err(RescueError::Security(format!(
                    "Device '{}' is the active root disk and cannot be modified",
                    root_disk
                )));
            }
        }

        // Check if device is protected
        for protected in Self::PROTECTED_DEVICES {
            if canonical_str.starts_with(protected) {
                return Err(RescueError::Security(format!(
                    "Device '{}' is protected and cannot be modified",
                    protected
                )));
            }
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

    /// Validates an output path for rescue operations
    ///
    /// # Security checks:
    /// - Must be in an allowed directory
    /// - Path traversal (..) is blocked
    /// - Path is canonicalized to prevent symlink attacks
    pub fn validate_output_path(path: &str) -> Result<PathBuf, RescueError> {
        // Reject empty or obviously malicious paths
        if path.is_empty() || path.contains('\0') {
            return Err(RescueError::Security(
                "Invalid output path: empty or contains null byte".to_string(),
            ));
        }

        // Block path traversal attempts
        if path.contains("..") {
            return Err(RescueError::Security(
                "Path traversal (..) is not allowed".to_string(),
            ));
        }

        let p = Path::new(path);

        // For absolute paths, validate against allowed directories
        if p.is_absolute() {
            // Try to canonicalize the parent directory (the file might not exist yet)
            let parent = p.parent().ok_or_else(|| {
                RescueError::Security("Invalid path: no parent directory".to_string())
            })?;

            // Create parent directory if it doesn't exist (only in allowed dirs)
            let canonical_parent = if parent.exists() {
                std::fs::canonicalize(parent).map_err(|e| {
                    RescueError::Security(format!("Cannot access parent directory: {}", e))
                })?
            } else {
                // Check if parent starts with an allowed dir before creating
                let parent_str = parent.to_string_lossy();
                let allowed = Self::allowed_output_dirs();
                let is_allowed = allowed
                    .iter()
                    .any(|allowed| parent_str.starts_with(allowed));

                if !is_allowed {
                    return Err(RescueError::Security(format!(
                        "Output path must be in one of: {}",
                        allowed.join(", ")
                    )));
                }

                parent.to_path_buf()
            };

            // Verify the canonical path is in an allowed directory
            let canonical_str = canonical_parent.to_string_lossy();
            let allowed = Self::allowed_output_dirs();
            let is_allowed = allowed
                .iter()
                .any(|allowed| canonical_str.starts_with(allowed));

            if !is_allowed {
                return Err(RescueError::Security(format!(
                    "Output path must be in one of: {}",
                    allowed.join(", ")
                )));
            }

            Ok(p.to_path_buf())
        } else {
            // Relative paths are converted to absolute based on /tmp
            let absolute = PathBuf::from("/tmp").join(p);
            Self::validate_output_path(&absolute.to_string_lossy())
        }
    }

    /// Validates a log file path (special case of output path)
    pub fn validate_log_path(path: &str) -> Result<PathBuf, RescueError> {
        Self::validate_output_path(path)
    }

    /// Check if a device is in the protected list
    pub fn is_protected_device(path: &str) -> bool {
        Self::PROTECTED_DEVICES
            .iter()
            .any(|protected| path.starts_with(protected))
    }
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
}
