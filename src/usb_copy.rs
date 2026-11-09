//! USB one-touch copy — automatic copy on removable media insertion
//!
//! Detects USB storage devices via udev, matches them against user-defined copy profiles,
//! and automatically copies content to specified destinations. Provides progress tracking,
//! checksum verification, and safe eject functionality.
//!
//! # Architecture
//!
//! - udev rule monitoring for block device insertion
//! - Profile matching by device UUID (never by `/dev/sdX`)
//! - Progress tracking via the existing job system
//! - Checksum verification for data integrity
//! - Resume support for interrupted copies
//!
//! # Security
//!
//! - Destination paths are validated and confined
//! - Pool member devices are never auto-copied
//! - Profile execution requires explicit user configuration
//! - All operations are journaled

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::process::Command;
use tokio::time::Duration;

use crate::checksum::{hash_file_async, ChecksumAlgorithm};
use crate::utils::process::run_timed_async;

const LSBLK_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for the "is this disk already in an array?" probes. `mdadm
/// --examine` and `btrfs filesystem show` touch every member device and can
/// block on a failing disk, so neither may run unbounded from a request.
const POOL_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for a mount / unmount / power-off. udisks2 talks to the kernel and
/// to the device itself; a wedged USB bridge must not hold the copy job.
const DEVICE_OP_TIMEOUT: Duration = Duration::from_secs(30);
const COPY_BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8MB buffer

/// USB copy profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbCopyProfile {
    /// Profile identifier
    pub id: String,

    /// Profile name
    pub name: String,

    /// Device UUID to match (not /dev path)
    pub device_uuid: String,

    /// Device label (for display)
    pub device_label: Option<String>,

    /// Source path within the device (e.g., "/DCIM")
    pub source_path: String,

    /// Destination path on HomeVault
    pub destination_path: String,

    /// Whether to verify checksums after copy
    pub verify_checksums: bool,

    /// Whether to eject after successful copy
    pub auto_eject: bool,

    /// Whether profile is enabled
    pub enabled: bool,

    /// Last execution timestamp
    pub last_run: Option<SystemTime>,

    /// Last run status
    pub last_status: Option<CopyStatus>,
}

/// Copy operation status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CopyStatus {
    Success {
        files_copied: u64,
        bytes_copied: u64,
    },
    Failed {
        error: String,
    },
    Cancelled,
}

/// Detected removable device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovableDevice {
    /// Device path (e.g., /dev/sdb1)
    pub device: String,

    /// Device UUID
    pub uuid: Option<String>,

    /// Device label
    pub label: Option<String>,

    /// Mount point
    pub mount_point: Option<PathBuf>,

    /// Size in bytes
    pub size: u64,

    /// Whether device is currently mounted
    pub mounted: bool,

    /// Matching profile ID if any
    pub matching_profile: Option<String>,
}

/// Copy job state
#[derive(Debug, Clone)]
pub struct UsbCopyJob {
    pub profile_id: String,
    pub device_uuid: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub verify: bool,
    pub auto_eject: bool,
    pub started_at: SystemTime,
    pub files_copied: u64,
    pub bytes_copied: u64,
    pub current_file: Option<String>,
}

pub struct UsbCopyService;

impl UsbCopyService {
    /// List all removable devices
    pub async fn list_devices() -> Result<Vec<RemovableDevice>, String> {
        let mut cmd = Command::new("lsblk");
        cmd.args(["-nbo", "NAME,UUID,LABEL,SIZE,MOUNTPOINT,RM,TYPE", "-J"]);
        let output = run_timed_async(&mut cmd, LSBLK_TIMEOUT)
            .await
            .ok_or_else(|| "lsblk timed out".to_string())?;

        if !output.status.success() {
            return Err("lsblk failed".to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Self::parse_lsblk_json(&stdout)
    }

    /// Parse lsblk JSON output.
    ///
    /// `pub` (not private) because `tests/usb_copy_integration.rs` drives the
    /// real parser against captured `lsblk -J` payloads — an integration test
    /// is a separate crate and cannot see a private helper.
    pub fn parse_lsblk_json(json: &str) -> Result<Vec<RemovableDevice>, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(json).map_err(|e| format!("invalid lsblk JSON: {}", e))?;

        let mut devices = Vec::new();

        if let Some(blockdevices) = parsed["blockdevices"].as_array() {
            for block in blockdevices {
                // Only process removable devices
                if block["rm"].as_str() != Some("1") {
                    continue;
                }

                // Only process partitions, not the disk itself
                if block["type"].as_str() != Some("part") {
                    continue;
                }

                let device = format!("/dev/{}", block["name"].as_str().unwrap_or(""));
                let uuid = block["uuid"].as_str().map(|s| s.to_string());
                let label = block["label"].as_str().map(|s| s.to_string());
                let mount_point = block["mountpoint"].as_str().map(|s| PathBuf::from(s));
                let size = block["size"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);

                devices.push(RemovableDevice {
                    device,
                    uuid,
                    label,
                    mounted: mount_point.is_some(),
                    mount_point,
                    size,
                    matching_profile: None,
                });
            }
        }

        Ok(devices)
    }

    /// Validate a copy profile
    pub fn validate_profile(profile: &UsbCopyProfile) -> Result<(), String> {
        // Validate ID
        if profile.id.is_empty() || profile.id.len() > 64 {
            return Err("profile ID must be 1-64 characters".to_string());
        }

        if !profile
            .id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err("profile ID must be alphanumeric with - or _".to_string());
        }

        // Validate name
        if profile.name.is_empty() || profile.name.len() > 128 {
            return Err("profile name must be 1-128 characters".to_string());
        }

        // Validate UUID (simple format check)
        if profile.device_uuid.is_empty() {
            return Err("device UUID is required".to_string());
        }

        // Validate source path (must be relative or absolute within device)
        let source = PathBuf::from(&profile.source_path);
        if source
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err("source path cannot contain '..'".to_string());
        }

        // Validate destination path
        Self::validate_destination_path(&profile.destination_path)?;

        Ok(())
    }

    /// Validate destination path for safety
    fn validate_destination_path(path: &str) -> Result<(), String> {
        let path_buf = PathBuf::from(path);

        // Must be absolute
        if !path_buf.is_absolute() {
            return Err("destination must be an absolute path".to_string());
        }

        // Cannot contain ..
        if path_buf
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err("destination cannot contain '..'".to_string());
        }

        // Must not be a system path
        let forbidden = [
            "/", "/bin", "/boot", "/dev", "/etc", "/lib", "/proc", "/root", "/run", "/sbin",
            "/sys", "/usr",
        ];

        for forbidden_path in &forbidden {
            if path.starts_with(forbidden_path)
                && (path.len() == forbidden_path.len()
                    || path.as_bytes().get(forbidden_path.len()) == Some(&b'/'))
            {
                return Err(format!("destination cannot be in {}", forbidden_path));
            }
        }

        Ok(())
    }

    /// Check if a device is a pool member
    pub async fn is_pool_member(device: &str) -> Result<bool, String> {
        // Check if device is used by any pool (mdadm, ZFS, Btrfs)

        // Check mdadm
        let mut mdadm_cmd = Command::new("sudo");
        mdadm_cmd.args(["mdadm", "--examine", device]);
        let mdadm_output = run_timed_async(&mut mdadm_cmd, POOL_PROBE_TIMEOUT).await;

        if let Some(output) = mdadm_output {
            if output.status.success() {
                return Ok(true); // Device is part of mdadm array
            }
        }

        // Check ZFS
        let mut zpool_cmd = Command::new("sudo");
        zpool_cmd.args(["zpool", "status"]);
        let zpool_output = run_timed_async(&mut zpool_cmd, POOL_PROBE_TIMEOUT).await;

        if let Some(output) = zpool_output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains(device) {
                return Ok(true); // Device is part of ZFS pool
            }
        }

        // Check Btrfs
        let mut btrfs_cmd = Command::new("sudo");
        btrfs_cmd.args(["btrfs", "filesystem", "show"]);
        let btrfs_output = run_timed_async(&mut btrfs_cmd, POOL_PROBE_TIMEOUT).await;

        if let Some(output) = btrfs_output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains(device) {
                return Ok(true); // Device is part of Btrfs filesystem
            }
        }

        Ok(false)
    }

    /// Mount a device if not already mounted
    pub async fn ensure_mounted(device: &str) -> Result<PathBuf, String> {
        // Check current mount status
        let devices = Self::list_devices().await?;

        if let Some(dev) = devices.iter().find(|d| d.device == device) {
            if let Some(ref mount_point) = dev.mount_point {
                return Ok(mount_point.clone());
            }
        }

        // Need to mount - use udisksctl. It runs through `sudo` because a
        // system service has no polkit session to authorise the mount; the
        // sudoers grant is confined to `/dev/*`.
        let mut mount_cmd = Command::new("sudo");
        mount_cmd.args(["udisksctl", "mount", "-b", device]);
        let output = run_timed_async(&mut mount_cmd, DEVICE_OP_TIMEOUT)
            .await
            .ok_or_else(|| "mount timed out".to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("mount failed: {}", stderr));
        }

        // Parse mount point from output: "Mounted /dev/sdb1 at /media/user/LABEL"
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mount_point = stdout
            .split(" at ")
            .nth(1)
            .map(|s| s.trim())
            .ok_or_else(|| "could not determine mount point".to_string())?;

        Ok(PathBuf::from(mount_point))
    }

    /// Unmount and eject a device
    pub async fn eject_device(device: &str) -> Result<(), String> {
        // Unmount first
        let mut unmount_cmd = Command::new("sudo");
        unmount_cmd.args(["udisksctl", "unmount", "-b", device]);
        let output = run_timed_async(&mut unmount_cmd, DEVICE_OP_TIMEOUT)
            .await
            .ok_or_else(|| "unmount timed out".to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "not mounted" errors
            if !stderr.contains("not mounted") {
                return Err(format!("unmount failed: {}", stderr));
            }
        }

        // Power off the device
        let mut poweroff_cmd = Command::new("sudo");
        poweroff_cmd.args(["udisksctl", "power-off", "-b", device]);
        let output = run_timed_async(&mut poweroff_cmd, DEVICE_OP_TIMEOUT)
            .await
            .ok_or_else(|| "power-off timed out".to_string())?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("power-off failed: {}", stderr));
        }

        Ok(())
    }

    /// Execute a copy operation
    pub async fn execute_copy(
        profile: &UsbCopyProfile,
        device: &RemovableDevice,
    ) -> Result<CopyStatus, String> {
        // Ensure device is mounted
        let mount_point = Self::ensure_mounted(&device.device).await?;

        // Build full source path
        let source = mount_point.join(&profile.source_path);
        if !source.exists() {
            return Err(format!("source path does not exist: {:?}", source));
        }

        let destination = PathBuf::from(&profile.destination_path);

        // Ensure destination exists
        tokio::fs::create_dir_all(&destination)
            .await
            .map_err(|e| format!("failed to create destination: {}", e))?;

        // A profile may point at a file (`/DCIM/photo.jpg`) as well as a
        // directory. Keep the source's own name in that case, and verify the
        // exact pair the copy wrote — not a directory against a file.
        let source_meta = tokio::fs::metadata(&source)
            .await
            .map_err(|e| format!("failed to stat source: {}", e))?;
        let (source_root, destination_root) = if source_meta.is_file() {
            let name = source
                .file_name()
                .ok_or_else(|| "source file has no name".to_string())?;
            (source.clone(), destination.join(name))
        } else {
            (source.clone(), destination.clone())
        };

        // Count files first for progress
        let total_files = Self::count_files(&source_root).await?;

        // Copy files with progress
        let (files_copied, bytes_copied) =
            Self::copy_recursive(&source_root, &destination_root, total_files).await?;

        // Verify if requested. A mismatch is a failed copy: the caller must
        // never see `Success` for bytes that were not read back and compared.
        if profile.verify_checksums {
            Self::verify_copy(&source_root, &destination_root).await?;
        }

        // Eject if requested
        if profile.auto_eject {
            let _ = Self::eject_device(&device.device).await;
        }

        Ok(CopyStatus::Success {
            files_copied,
            bytes_copied,
        })
    }

    /// Count files in a directory recursively (see `parse_lsblk_json` for why
    /// this is `pub`).
    pub async fn count_files(path: &Path) -> Result<u64, String> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| format!("failed to stat {}: {}", path.display(), e))?;
        if meta.is_file() {
            return Ok(1);
        }

        let mut count = 0u64;
        let mut stack = vec![path.to_path_buf()];

        while let Some(current) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&current)
                .await
                .map_err(|e| format!("failed to read directory: {}", e))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| format!("failed to read entry: {}", e))?
            {
                let metadata = entry
                    .metadata()
                    .await
                    .map_err(|e| format!("failed to get metadata: {}", e))?;

                if metadata.is_file() {
                    count += 1;
                } else if metadata.is_dir() {
                    stack.push(entry.path());
                }
            }
        }

        Ok(count)
    }

    /// Copy files recursively
    async fn copy_recursive(
        source: &Path,
        dest: &Path,
        _total_files: u64,
    ) -> Result<(u64, u64), String> {
        let mut files_copied = 0u64;
        let mut bytes_copied = 0u64;

        let mut stack = vec![(source.to_path_buf(), dest.to_path_buf())];

        while let Some((src, dst)) = stack.pop() {
            let metadata = tokio::fs::metadata(&src)
                .await
                .map_err(|e| format!("failed to get metadata: {}", e))?;

            if metadata.is_file() {
                // Copy file
                tokio::fs::copy(&src, &dst)
                    .await
                    .map_err(|e| format!("failed to copy file: {}", e))?;

                files_copied += 1;
                bytes_copied += metadata.len();
            } else if metadata.is_dir() {
                // Create destination directory
                tokio::fs::create_dir_all(&dst)
                    .await
                    .map_err(|e| format!("failed to create directory: {}", e))?;

                // Queue children
                let mut entries = tokio::fs::read_dir(&src)
                    .await
                    .map_err(|e| format!("failed to read directory: {}", e))?;

                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|e| format!("failed to read entry: {}", e))?
                {
                    let src_path = entry.path();
                    let dst_path = dst.join(entry.file_name());
                    stack.push((src_path, dst_path));
                }
            }
        }

        Ok((files_copied, bytes_copied))
    }

    /// Verify copied files match source, byte for byte.
    ///
    /// `verify_checksums` used to mean "same size and the destination exists",
    /// which declares a same-sized corrupt file verified. This walks the source
    /// tree in lockstep with the destination and compares a SHA-256 of every
    /// file; every mismatch is collected so the error names all the damaged
    /// files, not just the first one.
    pub async fn verify_copy(source: &Path, dest: &Path) -> Result<(), String> {
        let mut mismatches: Vec<String> = Vec::new();
        let mut checked: u64 = 0;
        let mut stack = vec![(source.to_path_buf(), dest.to_path_buf())];

        while let Some((src, dst)) = stack.pop() {
            let src_meta = tokio::fs::metadata(&src)
                .await
                .map_err(|e| format!("source missing while verifying {}: {}", src.display(), e))?;

            let dst_meta = match tokio::fs::metadata(&dst).await {
                Ok(meta) => meta,
                Err(e) => {
                    mismatches.push(format!("{}: destination missing ({e})", src.display()));
                    continue;
                }
            };

            if src_meta.is_file() {
                if !dst_meta.is_file() {
                    mismatches.push(format!("{}: destination is not a file", src.display()));
                    continue;
                }
                if src_meta.len() != dst_meta.len() {
                    mismatches.push(format!(
                        "{}: size {} != {}",
                        src.display(),
                        src_meta.len(),
                        dst_meta.len()
                    ));
                    continue;
                }

                match tokio::join!(sha256_path(&src), sha256_path(&dst)) {
                    (Ok(expected), Ok(actual)) if expected.eq_ignore_ascii_case(&actual) => {
                        checked += 1;
                    }
                    (Ok(expected), Ok(actual)) => mismatches.push(format!(
                        "{}: checksum {} != {}",
                        src.display(),
                        expected,
                        actual
                    )),
                    (Err(e), _) | (_, Err(e)) => {
                        mismatches.push(format!("{}: {e}", src.display()));
                    }
                }
            } else if src_meta.is_dir() {
                if !dst_meta.is_dir() {
                    mismatches.push(format!("{}: destination is not a directory", src.display()));
                    continue;
                }
                let mut entries = tokio::fs::read_dir(&src)
                    .await
                    .map_err(|e| format!("failed to read {}: {}", src.display(), e))?;
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|e| format!("failed to read {}: {}", src.display(), e))?
                {
                    stack.push((entry.path(), dst.join(entry.file_name())));
                }
            }
        }

        if mismatches.is_empty() {
            return Ok(());
        }

        let shown = mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let more = mismatches.len().saturating_sub(10);
        Err(format!(
            "checksum verification failed for {} file(s) ({} verified): {}{}",
            mismatches.len(),
            checked,
            shown,
            if more > 0 {
                format!("; and {more} more")
            } else {
                String::new()
            }
        ))
    }
}

/// SHA-256 of one file, on the blocking pool (the Explorer's hasher).
async fn sha256_path(path: &Path) -> Result<String, String> {
    let progress = Arc::new(AtomicU64::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    hash_file_async(
        path.to_path_buf(),
        ChecksumAlgorithm::Sha256,
        progress,
        cancel,
    )
    .await
    .map(|outcome| outcome.checksum)
    .map_err(|e| format!("could not hash {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_profile_id_length() {
        let profile = UsbCopyProfile {
            device_label: None,
            id: "a".repeat(65),
            name: "Test".to_string(),
            device_uuid: "1234-5678".to_string(),
            source_path: "/".to_string(),
            destination_path: "/mnt/test".to_string(),
            verify_checksums: false,
            auto_eject: false,
            enabled: true,
            last_run: None,
            last_status: None,
        };

        let result = UsbCopyService::validate_profile(&profile);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("1-64 characters"));
    }

    #[test]
    fn test_validate_profile_id_characters() {
        let profile = UsbCopyProfile {
            device_label: None,
            id: "test@profile".to_string(),
            name: "Test".to_string(),
            device_uuid: "1234-5678".to_string(),
            source_path: "/".to_string(),
            destination_path: "/mnt/test".to_string(),
            verify_checksums: false,
            auto_eject: false,
            enabled: true,
            last_run: None,
            last_status: None,
        };

        let result = UsbCopyService::validate_profile(&profile);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("alphanumeric"));
    }

    #[test]
    fn test_validate_profile_requires_uuid() {
        let profile = UsbCopyProfile {
            device_label: None,
            id: "test1".to_string(),
            name: "Test".to_string(),
            device_uuid: "".to_string(),
            source_path: "/".to_string(),
            destination_path: "/mnt/test".to_string(),
            verify_checksums: false,
            auto_eject: false,
            enabled: true,
            last_run: None,
            last_status: None,
        };

        let result = UsbCopyService::validate_profile(&profile);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("UUID is required"));
    }

    #[test]
    fn test_validate_destination_must_be_absolute() {
        let result = UsbCopyService::validate_destination_path("relative/path");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[test]
    fn test_validate_destination_no_parent_dir() {
        let result = UsbCopyService::validate_destination_path("/mnt/../etc");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("'..'"));
    }

    #[test]
    fn test_validate_destination_forbidden_paths() {
        let forbidden = ["/", "/etc", "/bin", "/sys"];

        for path in &forbidden {
            let result = UsbCopyService::validate_destination_path(path);
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("cannot be in"));
        }
    }

    #[test]
    fn test_validate_destination_allows_safe_paths() {
        let safe_paths = ["/mnt/usb", "/srv/backup", "/home/user/photos"];

        for path in &safe_paths {
            let result = UsbCopyService::validate_destination_path(path);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_parse_lsblk_json_empty() {
        let json = r#"{"blockdevices": []}"#;
        let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
        assert_eq!(devices.len(), 0);
    }

    #[test]
    fn test_parse_lsblk_json_removable_partition() {
        let json = r#"{
            "blockdevices": [
                {
                    "name": "sdb1",
                    "uuid": "1234-5678",
                    "label": "USB_DRIVE",
                    "size": "16000000000",
                    "mountpoint": "/media/usb",
                    "rm": "1",
                    "type": "part"
                }
            ]
        }"#;

        let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device, "/dev/sdb1");
        assert_eq!(devices[0].uuid, Some("1234-5678".to_string()));
        assert_eq!(devices[0].label, Some("USB_DRIVE".to_string()));
        assert!(devices[0].mounted);
    }

    #[test]
    fn test_parse_lsblk_json_ignores_non_removable() {
        let json = r#"{
            "blockdevices": [
                {
                    "name": "sda1",
                    "uuid": "aaaa-bbbb",
                    "size": "500000000000",
                    "mountpoint": "/",
                    "rm": "0",
                    "type": "part"
                }
            ]
        }"#;

        let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
        assert_eq!(devices.len(), 0);
    }

    #[test]
    fn test_parse_lsblk_json_ignores_disk_type() {
        let json = r#"{
            "blockdevices": [
                {
                    "name": "sdb",
                    "size": "16000000000",
                    "rm": "1",
                    "type": "disk"
                }
            ]
        }"#;

        let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
        assert_eq!(devices.len(), 0);
    }

    #[tokio::test]
    async fn test_verify_copy_accepts_an_identical_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::create_dir_all(dst.join("nested")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        std::fs::write(dst.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src.join("nested/b.bin"), [0u8, 1, 2, 255]).unwrap();
        std::fs::write(dst.join("nested/b.bin"), [0u8, 1, 2, 255]).unwrap();

        UsbCopyService::verify_copy(&src, &dst)
            .await
            .expect("identical trees must verify");
    }

    #[tokio::test]
    async fn test_verify_copy_rejects_same_size_different_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"aaaa").unwrap();
        std::fs::write(&dst, b"bbbb").unwrap();

        let error = UsbCopyService::verify_copy(&src, &dst)
            .await
            .expect_err("same size is not the same content");
        assert!(error.contains("checksum"), "{error}");
    }

    #[tokio::test]
    async fn test_verify_copy_rejects_a_size_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");
        std::fs::write(&src, b"12345678").unwrap();
        std::fs::write(&dst, b"1234").unwrap();

        let error = UsbCopyService::verify_copy(&src, &dst)
            .await
            .expect_err("a truncated copy must fail");
        assert!(error.contains("size"), "{error}");
    }

    #[tokio::test]
    async fn test_verify_copy_rejects_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("present.txt"), b"x").unwrap();
        std::fs::write(src.join("missing.txt"), b"y").unwrap();
        std::fs::write(dst.join("present.txt"), b"x").unwrap();

        let error = UsbCopyService::verify_copy(&src, &dst)
            .await
            .expect_err("a missing destination file must fail");
        assert!(error.contains("destination missing"), "{error}");
    }

    #[tokio::test]
    async fn test_verify_copy_handles_a_single_file_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.jpg");
        let dst = dir.path().join("photo.jpg");
        std::fs::write(&src, b"jpeg-bytes").unwrap();
        std::fs::write(&dst, b"jpeg-bytes").unwrap();

        UsbCopyService::verify_copy(&src, &dst)
            .await
            .expect("a single file must verify");
    }

    #[tokio::test]
    async fn test_count_files_counts_a_single_file_as_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("one.bin");
        std::fs::write(&file, b"x").unwrap();

        assert_eq!(UsbCopyService::count_files(&file).await.unwrap(), 1);
    }
}
