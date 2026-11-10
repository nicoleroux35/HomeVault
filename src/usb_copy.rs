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
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::process::Command;
use tokio::time::Duration;

use chrono::{Datelike, Utc};

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

/// How many `.versions/<stamp>/` folders a versioned profile keeps by default
/// (FIAB-26). Older snapshots are removed after a successful run.
pub const DEFAULT_VERSIONS_TO_KEEP: u32 = 10;
/// Upper bound accepted from the API, so a typo cannot turn the retention pass
/// into a full-tree scan that never ends.
pub const MAX_VERSIONS_TO_KEEP: u32 = 1_000;
/// Directories visited by one retention pass. A normal destination has a few
/// dozen; the bound keeps a hostile or accidentally huge tree from stalling
/// the copy job.
const MAX_PURGE_DIRECTORIES: usize = 10_000;

/// Shape of a version folder: `YYYYMMDD-HHMMSS`, the stamp written by
/// `version_existing`. Anything else under `.versions/` is left alone.
fn is_version_stamp(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 15
        && bytes[8] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || byte.is_ascii_digit())
}

/// Default for `versions_to_keep` (FIAB-26).
const fn default_versions_to_keep() -> u32 {
    DEFAULT_VERSIONS_TO_KEEP
}

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

    /// How an existing destination file is handled (FIAB-13).
    #[serde(default)]
    pub strategy: CopyStrategy,

    /// What to do when a destination file already exists (FIAB-13).
    #[serde(default)]
    pub conflict: ConflictPolicy,

    /// How many versioned snapshots to keep per folder (FIAB-26).
    ///
    /// `0` keeps every snapshot — the historical behaviour — so a profile can
    /// opt out of the cleanup. Older profiles have no field and load with the
    /// default.
    #[serde(default = "default_versions_to_keep")]
    pub versions_to_keep: u32,

    /// Whether to eject after successful copy
    pub auto_eject: bool,

    /// Whether profile is enabled
    pub enabled: bool,

    /// Last execution timestamp
    pub last_run: Option<SystemTime>,

    /// Last run status
    pub last_status: Option<CopyStatus>,
}

/// The copy strategy (FIAB-13), mirroring DSM's USB Copy modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CopyStrategy {
    /// Copy only files that are missing or newer; keep the rest.
    #[default]
    Incremental,
    /// Incremental, then remove destination files the source no longer has.
    Mirror,
    /// Move an existing destination file into `.versions/<timestamp>/` first.
    Versioned,
    /// Sort into `YYYY/MM/` by the file's modification date.
    PhotoImport,
}

impl CopyStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Mirror => "mirror",
            Self::Versioned => "versioned",
            Self::PhotoImport => "photo_import",
        }
    }
}

/// What happens when a destination file already exists (FIAB-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Replace the destination (the historical behaviour).
    #[default]
    Overwrite,
    /// Leave the destination untouched.
    Skip,
    /// Keep both, e.g. `photo (1).jpg`.
    Rename,
}

impl ConflictPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overwrite => "overwrite",
            Self::Skip => "skip",
            Self::Rename => "rename",
        }
    }
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

        if profile.versions_to_keep > MAX_VERSIONS_TO_KEEP {
            return Err(format!(
                "versions to keep must be 0-{MAX_VERSIONS_TO_KEEP} (0 keeps every version)"
            ));
        }

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
        // directory. The copy engine keeps the file's own name inside the
        // destination folder; verification compares the exact pair it wrote
        // — not a directory against a file.
        let source_meta = tokio::fs::metadata(&source)
            .await
            .map_err(|e| format!("failed to stat source: {}", e))?;
        let verify_root = if source_meta.is_file() {
            let name = source
                .file_name()
                .ok_or_else(|| "source file has no name".to_string())?;
            destination.join(name)
        } else {
            destination.clone()
        };

        // FIAB-13: the strategy and conflict policy are applied per file.
        let strategy = profile.strategy;
        let conflict = profile.conflict;
        let (files_copied, bytes_copied) = Self::copy_recursive(
            &source,
            &destination,
            strategy,
            conflict,
            matches!(strategy, CopyStrategy::PhotoImport),
            profile.verify_checksums,
        )
        .await?;

        // Verify if requested. Photo import rewrites the destination layout, so
        // its per-file verification happened inside the copy loop.
        if profile.verify_checksums && !matches!(strategy, CopyStrategy::PhotoImport) {
            Self::verify_copy(&source, &verify_root).await?;
        }

        // FIAB-26: bound the `.versions/` history once the copy is complete.
        // Best-effort by design — a failed cleanup must never turn a verified
        // copy into a failed run.
        if matches!(strategy, CopyStrategy::Versioned) && profile.versions_to_keep > 0 {
            Self::purge_versions(&destination, profile.versions_to_keep).await;
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

    /// Copy files recursively under a strategy (FIAB-13).
    ///
    /// `photo_dates` maps every file into `YYYY/MM/` by its modification date;
    /// `verify` hashes the pair as soon as each file is written (the global
    /// `verify_copy` cannot compare two different layouts).
    #[allow(clippy::too_many_arguments)]
    async fn copy_recursive(
        source: &Path,
        dest: &Path,
        strategy: CopyStrategy,
        conflict: ConflictPolicy,
        photo_dates: bool,
        verify: bool,
    ) -> Result<(u64, u64), String> {
        let mut files_copied = 0u64;
        let mut bytes_copied = 0u64;
        let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let mut expected: HashSet<PathBuf> = HashSet::new();

        // (source path, path relative to the source root)
        let mut stack = vec![(source.to_path_buf(), PathBuf::new())];

        while let Some((src, relative)) = stack.pop() {
            let metadata = tokio::fs::metadata(&src)
                .await
                .map_err(|e| format!("failed to get metadata: {e}"))?;

            if metadata.is_file() {
                // A single-file source (`relative` empty) keeps its own name;
                // photo import prefixes the year/month folder.
                let own_name = src
                    .file_name()
                    .map(PathBuf::from)
                    .ok_or("source file has no name")?;
                let relative = if photo_dates {
                    let modified = metadata.modified().unwrap_or(SystemTime::now());
                    let when: chrono::DateTime<Utc> = modified.into();
                    let folder = PathBuf::from(format!("{:04}/{:02}", when.year(), when.month()));
                    if relative.as_os_str().is_empty() {
                        folder.join(own_name)
                    } else {
                        folder.join(&relative)
                    }
                } else if relative.as_os_str().is_empty() {
                    own_name
                } else {
                    relative
                };
                let dst = dest.join(&relative);
                expected.insert(relative);

                if let Some(parent) = dst.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| format!("failed to create directory: {e}"))?;
                }

                let mut should_copy = true;
                if tokio::fs::try_exists(&dst).await.unwrap_or(false) {
                    if matches!(strategy, CopyStrategy::Incremental | CopyStrategy::Mirror)
                        && Self::is_up_to_date(&dst, &metadata).await
                    {
                        should_copy = false;
                    }
                    if should_copy {
                        // The conflict policy decides whether the existing file
                        // survives, is moved aside, or is replaced. Versioning
                        // only applies to a real replacement, so `skip` and
                        // `rename` never hide a file in `.versions`.
                        match conflict {
                            ConflictPolicy::Skip => should_copy = false,
                            ConflictPolicy::Rename => {
                                let renamed = Self::unique_destination(&dst).await;
                                let _ = tokio::fs::rename(&dst, &renamed).await;
                            }
                            ConflictPolicy::Overwrite => {
                                if matches!(strategy, CopyStrategy::Versioned) {
                                    Self::version_existing(&dst, &stamp).await?;
                                }
                            }
                        }
                    }
                }
                if !should_copy {
                    continue;
                }

                tokio::fs::copy(&src, &dst)
                    .await
                    .map_err(|e| format!("failed to copy file: {e}"))?;
                if verify && !Self::same_sha256(&src, &dst).await? {
                    return Err(format!(
                        "checksum mismatch after copying {} to {}",
                        src.display(),
                        dst.display()
                    ));
                }

                files_copied += 1;
                bytes_copied += metadata.len();
            } else if metadata.is_dir() {
                if !photo_dates {
                    tokio::fs::create_dir_all(dest.join(&relative))
                        .await
                        .map_err(|e| format!("failed to create directory: {e}"))?;
                }

                let mut entries = tokio::fs::read_dir(&src)
                    .await
                    .map_err(|e| format!("failed to read directory: {e}"))?;
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|e| format!("failed to read entry: {e}"))?
                {
                    stack.push((entry.path(), relative.join(entry.file_name())));
                }
            }
        }

        // Mirror: a file the source no longer has must disappear from the
        // destination. Stay inside `dest` and never touch `.versions`.
        if matches!(strategy, CopyStrategy::Mirror) {
            Self::remove_unexpected(dest, Path::new(""), &expected).await?;
        }

        Ok((files_copied, bytes_copied))
    }

    /// A destination is up to date when it exists with the same size and is
    /// not older than the source.
    async fn is_up_to_date(dst: &Path, src_meta: &std::fs::Metadata) -> bool {
        let Ok(dst_meta) = tokio::fs::metadata(dst).await else {
            return false;
        };
        if !dst_meta.is_file() || dst_meta.len() != src_meta.len() {
            return false;
        }
        let src_time = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let dst_time = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        dst_time >= src_time
    }

    /// Moves an existing destination into `.versions/<stamp>/` next to it.
    async fn version_existing(dst: &Path, stamp: &str) -> Result<(), String> {
        let Some(parent) = dst.parent() else {
            return Ok(());
        };
        let name = dst.file_name().ok_or("destination has no name")?;
        let versions = parent.join(".versions").join(stamp);
        tokio::fs::create_dir_all(&versions)
            .await
            .map_err(|e| format!("failed to create the versions folder: {e}"))?;
        tokio::fs::rename(dst, versions.join(name))
            .await
            .map_err(|e| format!("failed to move the previous version: {e}"))
    }

    /// Removes the oldest `.versions/<stamp>/` folders beyond `keep`.
    ///
    /// Only directories named like a timestamp are candidates, only
    /// `.versions` folders under `root` are visited, and symlinks are never
    /// followed — so a retention pass can never reach outside the destination
    /// or delete a user's own folder. The directory walk is bounded so a
    /// pathological tree cannot stall the copy job.
    async fn purge_versions(root: &Path, keep: u32) {
        if keep == 0 {
            return;
        }
        let mut stack = vec![root.to_path_buf()];
        let mut visited = 0usize;

        while let Some(dir) = stack.pop() {
            visited += 1;
            if visited > MAX_PURGE_DIRECTORIES {
                return;
            }
            let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let Ok(file_type) = entry.file_type().await else {
                    continue;
                };
                // `file_type()` does not follow symlinks: a link to another
                // tree is skipped instead of walked.
                if !file_type.is_dir() {
                    continue;
                }
                if entry.file_name() == ".versions" {
                    Self::purge_one_versions_dir(&entry.path(), keep).await;
                } else {
                    stack.push(entry.path());
                }
            }
        }
    }

    /// Applies the retention count to one `.versions/` folder.
    async fn purge_one_versions_dir(dir: &Path, keep: u32) {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            return;
        };
        let mut stamps: Vec<(String, PathBuf)> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if is_version_stamp(&name) {
                stamps.push((name, entry.path()));
            }
        }
        if stamps.len() <= keep as usize {
            return;
        }
        // The stamp sorts chronologically as text (`YYYYMMDD-HHMMSS`).
        stamps.sort_by(|a, b| a.0.cmp(&b.0));
        let excess = stamps.len() - keep as usize;
        for (_, path) in stamps.into_iter().take(excess) {
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
    }

    /// `name (1).ext`, `name (2).ext`… bounded so a full folder cannot loop.
    async fn unique_destination(dst: &Path) -> PathBuf {
        let parent = dst.parent().unwrap_or(Path::new(""));
        let stem = dst
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let ext = dst
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        for n in 1..=1000 {
            let candidate = parent.join(format!("{stem} ({n}){ext}"));
            if !tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                return candidate;
            }
        }
        dst.to_path_buf()
    }

    /// Removes files under `dir` whose relative path is not in `expected`.
    fn remove_unexpected<'a>(
        dir: &'a Path,
        relative: &'a Path,
        expected: &'a HashSet<PathBuf>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
                return Ok(());
            };
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| format!("failed to read {}: {e}", dir.display()))?
            {
                let name = entry.file_name();
                if name == ".versions" {
                    continue;
                }
                let path = relative.join(&name);
                let metadata = entry
                    .metadata()
                    .await
                    .map_err(|e| format!("failed to stat {}: {e}", entry.path().display()))?;
                if metadata.is_dir() {
                    Self::remove_unexpected(&entry.path(), &path, expected).await?;
                    // Remove the directory only when it became empty.
                    let _ = tokio::fs::remove_dir(entry.path()).await;
                } else if !expected.contains(&path) {
                    tokio::fs::remove_file(entry.path())
                        .await
                        .map_err(|e| format!("failed to remove {}: {e}", entry.path().display()))?;
                }
            }
            Ok(())
        })
    }

    async fn same_sha256(src: &Path, dst: &Path) -> Result<bool, String> {
        let (a, b) = tokio::join!(sha256_path(src), sha256_path(dst));
        match (a, b) {
            (Ok(a), Ok(b)) => Ok(a.eq_ignore_ascii_case(&b)),
            (Err(e), _) | (_, Err(e)) => Err(e),
        }
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
            strategy: CopyStrategy::Incremental,
            conflict: ConflictPolicy::Overwrite,
            versions_to_keep: DEFAULT_VERSIONS_TO_KEEP,
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
            strategy: CopyStrategy::Incremental,
            conflict: ConflictPolicy::Overwrite,
            versions_to_keep: DEFAULT_VERSIONS_TO_KEEP,
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
            strategy: CopyStrategy::Incremental,
            conflict: ConflictPolicy::Overwrite,
            versions_to_keep: DEFAULT_VERSIONS_TO_KEEP,
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

    #[tokio::test]
    async fn incremental_skips_an_up_to_date_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"same").unwrap();

        let first = UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Incremental,
            ConflictPolicy::Overwrite,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(first.0, 1);
        let second = UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Incremental,
            ConflictPolicy::Overwrite,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(second.0, 0, "an up-to-date file is not copied again");
    }

    #[tokio::test]
    async fn mirror_removes_a_file_the_source_no_longer_has() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("keep.txt"), b"keep").unwrap();
        std::fs::write(dst.join("keep.txt"), b"keep").unwrap();
        std::fs::write(dst.join("stale.txt"), b"stale").unwrap();

        UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Mirror,
            ConflictPolicy::Overwrite,
            false,
            false,
        )
        .await
        .unwrap();
        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join("stale.txt").exists(), "mirror removes extras");
    }

    #[tokio::test]
    async fn versioned_keeps_the_previous_file_under_versions() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(dst.join("a.txt"), b"old").unwrap();

        UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Versioned,
            ConflictPolicy::Overwrite,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"new");
        let versions = dst.join(".versions");
        let stamp = std::fs::read_dir(&versions)
            .unwrap()
            .next()
            .expect("one version folder")
            .unwrap();
        assert_eq!(std::fs::read(stamp.path().join("a.txt")).unwrap(), b"old");
    }

    #[tokio::test]
    async fn retention_keeps_only_the_newest_versions() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let versions = dst.join(".versions");
        std::fs::create_dir_all(&versions).unwrap();
        for stamp in [
            "20260101-000000",
            "20260102-000000",
            "20260103-000000",
            "20260104-000000",
            "20260105-000000",
        ] {
            std::fs::create_dir_all(versions.join(stamp)).unwrap();
            std::fs::write(versions.join(stamp).join("a.txt"), stamp).unwrap();
        }
        // Neither a loose file nor a folder that is not a stamp is a version.
        std::fs::write(versions.join("notes.txt"), b"keep me").unwrap();
        std::fs::create_dir_all(versions.join("manual")).unwrap();

        UsbCopyService::purge_versions(&dst, 2).await;

        let mut left: Vec<String> = std::fs::read_dir(&versions)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "20260104-000000".to_string(),
                "20260105-000000".to_string(),
                "manual".to_string(),
                "notes.txt".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn retention_zero_keeps_every_version() {
        let dir = tempfile::tempdir().unwrap();
        let versions = dir.path().join(".versions");
        for stamp in ["20260101-000000", "20260102-000000"] {
            std::fs::create_dir_all(versions.join(stamp)).unwrap();
        }

        UsbCopyService::purge_versions(dir.path(), 0).await;

        assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn retention_ignores_lookalike_folders_outside_versions() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        // A user's own timestamp-shaped folder must survive: only `.versions`
        // folders are retention targets.
        let user_folder = dst.join("20260101-000000");
        std::fs::create_dir_all(&user_folder).unwrap();
        std::fs::write(user_folder.join("keep.txt"), b"user data").unwrap();

        let versions = dst.join(".versions");
        for stamp in ["20260101-000000", "20260102-000000"] {
            std::fs::create_dir_all(versions.join(stamp)).unwrap();
        }

        UsbCopyService::purge_versions(&dst, 1).await;

        assert!(user_folder.join("keep.txt").exists());
        assert_eq!(std::fs::read_dir(&versions).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn retention_walks_nested_versions_folders() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let nested = dst.join("DCIM").join(".versions");
        for stamp in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            std::fs::create_dir_all(nested.join(stamp)).unwrap();
        }

        UsbCopyService::purge_versions(&dst, 1).await;

        let left: Vec<String> = std::fs::read_dir(&nested)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left, vec!["20260103-000000".to_string()]);
    }

    #[test]
    fn version_stamp_shape_is_enforced() {
        assert!(is_version_stamp("20260101-000000"));
        assert!(!is_version_stamp("20260101-00000"));
        assert!(!is_version_stamp("20260101_000000"));
        assert!(!is_version_stamp("manual"));
        assert!(!is_version_stamp("abcdefgh-000000"));
    }

    #[test]
    fn validate_profile_bounds_the_version_retention() {
        let mut profile = UsbCopyProfile {
            device_label: None,
            id: "test".to_string(),
            name: "Test".to_string(),
            device_uuid: "1234-5678".to_string(),
            source_path: "/".to_string(),
            destination_path: "/mnt/test".to_string(),
            verify_checksums: false,
            strategy: CopyStrategy::Versioned,
            conflict: ConflictPolicy::Overwrite,
            versions_to_keep: MAX_VERSIONS_TO_KEEP + 1,
            auto_eject: false,
            enabled: true,
            last_run: None,
            last_status: None,
        };
        let error = UsbCopyService::validate_profile(&profile).expect_err("too many versions");
        assert!(error.contains("versions to keep"), "{error}");

        profile.versions_to_keep = 0;
        UsbCopyService::validate_profile(&profile).expect("zero means keep every version");
    }

    #[tokio::test]
    async fn skip_keeps_the_destination_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new-bytes").unwrap();
        std::fs::write(dst.join("a.txt"), b"old!").unwrap();

        let (copied, _) = UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Versioned,
            ConflictPolicy::Skip,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(copied, 0, "skip writes nothing");
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"old!");
    }

    #[tokio::test]
    async fn rename_keeps_both_copies() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(dst.join("a.txt"), b"old").unwrap();

        let (copied, _) = UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Versioned,
            ConflictPolicy::Rename,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(copied, 1);
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(dst.join("a (1).txt")).unwrap(), b"old");
    }

    #[tokio::test]
    async fn a_single_file_source_lands_under_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.jpg");
        let dst = dir.path().join("dst");
        std::fs::write(&src, b"jpeg-bytes").unwrap();

        let (copied, bytes) = UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::Incremental,
            ConflictPolicy::Overwrite,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(copied, 1);
        assert_eq!(bytes, 10);
        assert_eq!(std::fs::read(dst.join("photo.jpg")).unwrap(), b"jpeg-bytes");
    }

    #[tokio::test]
    async fn a_single_file_photo_import_keeps_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("IMG_0002.jpg");
        let dst = dir.path().join("dst");
        std::fs::write(&src, b"photo").unwrap();

        UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::PhotoImport,
            ConflictPolicy::Overwrite,
            true,
            false,
        )
        .await
        .unwrap();

        let when: chrono::DateTime<Utc> =
            std::fs::metadata(&src).unwrap().modified().unwrap().into();
        let expected = dst
            .join(format!("{:04}/{:02}", when.year(), when.month()))
            .join("IMG_0002.jpg");
        assert!(expected.exists(), "missing {}", expected.display());
    }

    #[tokio::test]
    async fn photo_import_sorts_files_by_year_and_month() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("IMG_0001.jpg"), b"photo").unwrap();

        UsbCopyService::copy_recursive(
            &src,
            &dst,
            CopyStrategy::PhotoImport,
            ConflictPolicy::Overwrite,
            true,
            false,
        )
        .await
        .unwrap();
        let when: chrono::DateTime<Utc> = std::fs::metadata(src.join("IMG_0001.jpg"))
            .unwrap()
            .modified()
            .unwrap()
            .into();
        let expected = dst
            .join(format!("{:04}/{:02}", when.year(), when.month()))
            .join("IMG_0001.jpg");
        assert!(expected.exists(), "missing {}", expected.display());
    }
}
