/* =============================================================================
 * File          : disk.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Physical disk service: inventory of disks/partitions/removable drives,
 *   SMART capabilities and self-tests, system-disk detection, formatting
 *   and mounting, and the mdadm RAID create/teardown/rebuild operations
 *   underlying the virtual-disk features.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tokio (>= 1.52)
 *
 * Usage:
 *   handlers/storage.rs, handlers/explorer.rs (eject), RAID creation.
 * =============================================================================
 */

//! Physical disk service (inventory, SMART, format/mount, mdadm RAID).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;

/// Errors that can occur during disk operations.
#[derive(Error, Debug)]
pub enum DiskError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Command '{command}' failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
    #[error("Device not found: {0}")]
    DeviceNotFound(String),
    #[error("SMART not available: {0}")]
    SmartNotAvailable(String),
    #[error("Test already in progress")]
    SmartTestInProgress,
}

/// SMART self-test capabilities for a disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartCapabilities {
    pub available: bool,
    pub supports_short_test: bool,
    pub supports_long_test: bool,
    pub supports_conveyance_test: bool,
}

/// Status of an in-progress or last completed SMART self-test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartTestStatus {
    pub in_progress: bool,
    pub progress_pct: u8,
    pub remaining_minutes: u32,
    pub last_result: Option<String>,
}

/// Progress reporter handed to the virtual-disk creation flow so it can feed
/// percent + human status into the tracked progress job created by the API
/// handler. The job system (`crate::sse::progress`) is the only consumer.
pub trait CreateProgress: Send + Sync {
    /// Report a milestone: `percent` 0-100 plus a short human message.
    fn report(&self, percent: u8, message: &str);
}

/// Options for creating an mdadm array (the ext4/xfs arm of the virtual-disk
/// dispatcher). `members` are the active devices; `spares` the extra hot
/// spares (redundant levels only); `allow_missing` creates the array
/// deliberately degraded when fewer members than the level minimum arrived —
/// mdadm fills the missing slots with the literal `missing` placeholder.
#[derive(Debug, Clone)]
pub struct MdadmRaidRequest {
    pub name: String,
    pub level: String,
    pub members: Vec<String>,
    pub spares: Vec<String>,
    pub allow_missing: bool,
    pub filesystem: String,
}

/// A full virtual-disk creation request: one of the five supported RAID
/// levels on top of one of the four technologies (`ext4`/`xfs` → mdadm,
/// `btrfs` → a native multi-device Btrfs filesystem, `zfs` → a native ZFS
/// pool). Pure input, validated by `validate_virtual_disk_request` before
/// any tool runs.
#[derive(Debug, Clone)]
pub struct VirtualDiskRequest {
    pub name: String,
    pub level: String,
    pub members: Vec<String>,
    pub spares: Vec<String>,
    pub allow_missing: bool,
    pub filesystem: String,
}

/// Serializes destructive virtual-disk creation. Two concurrent creates that
/// settle on the same free disk would otherwise both pass the free-device
/// gate and let mdadm/zpool clobber each other; holding this lock across the
/// whole creation makes one of them wait instead.
pub static CREATE_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// Serializes `/etc/fstab` + `/etc/mdadm/mdadm.conf` edits inside this module
/// so two scans (create + delete) cannot interleave read-modify-write cycles
/// and resurrect each other's entries.
pub static PERSIST_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// Minimum device count per RAID level (mirror of the frontend's minDisks).
fn min_devices_for_level(level: &str) -> usize {
    match level {
        "raid0" | "raid1" => 2,
        "raid5" => 3,
        "raid6" | "raid10" => 4,
        _ => 2,
    }
}

/// How many member slots a level may start deliberately degraded (the
/// `missing` placeholder count mdadm accepts at `--create` time). raid0 has
/// no redundancy to degrade; raid10's layout makes missing-slot creation
/// fragile, so both are refused.
fn max_missing_for_level(level: &str) -> usize {
    match level {
        "raid1" | "raid5" => 1,
        "raid6" => 2,
        _ => 0,
    }
}

/// How many mdadm "member slots" a request implies: the real members plus the
/// missing placeholders when `allow_missing` is set (i.e. the array starts
/// degraded and grows to full redundancy as disks are added later).
fn md_member_slots(req: &VirtualDiskRequest) -> usize {
    if !req.allow_missing {
        return req.members.len();
    }
    let min = min_devices_for_level(&req.level);
    usize::max(req.members.len(), min)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    pub name: String,
    pub device: String,
    /// Total size in bytes.
    #[serde(rename = "size")]
    pub size_bytes: u64,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    /// Overall SMART health status (`"Healthy"`, `"Failed"`, or `"Unknown"`).
    #[serde(rename = "smart_status")]
    pub smart_status: String,
    pub temperature: Option<f64>,
    #[serde(rename = "partition_count")]
    pub partition_count: usize,
    /// Connection transport (e.g. `"sata"`, `"usb"`, `"nvme"`).
    #[serde(rename = "connection_type")]
    pub connection_type: String,
    #[serde(rename = "errors")]
    pub smart_read_errors: u64,
    pub smart_write_errors: u64,
    pub smart_bad_sectors: u64,
    /// Role string (`"System"`, `"Data"`, `"RAID"`, `"ZFS Pool"`, `"Btrfs"`, `"Unassigned"`).
    pub role: String,
}

/// Information about a single partition and its mount status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionInfo {
    pub device: String,
    pub mount_point: String,
    pub filesystem: String,
    #[serde(rename = "size")]
    pub size_bytes: u64,
    #[serde(rename = "used")]
    pub used_bytes: u64,
    #[serde(rename = "available")]
    pub available_bytes: u64,
    /// Usage percentage (0-100).
    pub percent: u32,
    pub uuid: String,
}

/// A Linux MD RAID array discovered via `mdadm`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaidArray {
    pub name: String,
    /// RAID level (e.g. `"raid0"`, `"raid1"`, `"raid5"`).
    pub level: String,
    pub state: String,
    pub devices: Vec<String>,
    #[serde(rename = "size")]
    pub size_bytes: u64,
    #[serde(rename = "used")]
    pub used_bytes: u64,
    #[serde(rename = "chunk_size")]
    pub chunk_size: u64,
}

/// A removable (USB / external) disk detected by transport type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemovableDisk {
    pub device: String,
    pub name: String,
    #[serde(rename = "size")]
    pub size_bytes: u64,
    pub filesystem: String,
    #[serde(rename = "mount_point")]
    pub mount_point: Option<String>,
    pub label: Option<String>,
    pub uuid: Option<String>,
    #[serde(rename = "mounted")]
    pub is_mounted: bool,
}

/// Service for enumerating, mounting, and managing physical disks and RAID arrays.
pub struct DiskService;

impl DiskService {
    /// List all physical disks via `lsblk`, enriched with SMART data and role detection.
    pub async fn list_disks() -> Result<Vec<DiskInfo>, DiskError> {
        let mut disks = Vec::new();

        // `-P` (pairs, `KEY="value"`) output, not positional `-l` — a VENDOR
        // or MODEL containing a space (e.g. "TOSHIBA MQ01ABF050") silently
        // shifted every column after it when this used plain
        // `split_whitespace()` (confirmed live: MODEL/SERIAL/TRAN all came
        // out wrong for a real disk). Same fix `list_partitions` already
        // uses, via the same `parse_lsblk_pairs` helper.
        let lsblk_output = Command::new("lsblk")
            .args([
                "-d",
                "-n",
                "-P",
                "-o",
                "NAME,SIZE,TYPE,VENDOR,MODEL,SERIAL,TRAN",
            ])
            .output()
            .await?;

        for line in String::from_utf8_lossy(&lsblk_output.stdout).lines() {
            let fields = Self::parse_lsblk_pairs(line);
            if fields.get("TYPE").map(String::as_str) != Some("disk") {
                continue;
            }

            let name = fields.get("NAME").cloned().unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let size = Self::parse_size(fields.get("SIZE").map(String::as_str).unwrap_or("0"));
            let unknown = || "Unknown".to_string();
            let vendor = fields
                .get("VENDOR")
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(unknown);
            let model = fields
                .get("MODEL")
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(unknown);
            let serial = fields
                .get("SERIAL")
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(unknown);
            let connection_type = fields
                .get("TRAN")
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(unknown);

            let (smart_status, read_errors, write_errors, bad_sectors) =
                Self::get_smart_details(&name).await;
            let temperature = Self::get_disk_temperature(&name).await.ok();
            let role = Self::detect_disk_role(&name).await;

            disks.push(DiskInfo {
                name: name.clone(),
                device: format!("/dev/{}", name),
                size_bytes: size,
                vendor,
                model,
                serial,
                smart_status,
                temperature,
                partition_count: Self::count_partitions(&name).await,
                connection_type,
                smart_read_errors: read_errors,
                smart_write_errors: write_errors,
                smart_bad_sectors: bad_sectors,
                role,
            });
        }

        Ok(disks)
    }

    /// List all partitions with filesystem, mount point, and usage information.
    pub async fn list_partitions() -> Result<Vec<PartitionInfo>, DiskError> {
        let mut partitions = Vec::new();

        // `-P` (pairs, `KEY="value"`) output is used instead of the plain
        // `-l` list format: whitespace-splitting a `-l` line silently shifts
        // every column after an empty one to the left — confirmed live that
        // an unmounted partition's empty MOUNTPOINT column made its UUID get
        // misread as a real mount point, which then showed up in the
        // home-folder disk picker as a selectable "mounted" disk. `-P`
        // always emits every requested key, even with an empty value, so a
        // missing column can no longer shift the rest.
        let lsblk_output = Command::new("lsblk")
            .args(["-n", "-P", "-o", "NAME,SIZE,FSTYPE,MOUNTPOINT,UUID"])
            .output()
            .await?;

        for line in String::from_utf8_lossy(&lsblk_output.stdout).lines() {
            let fields = Self::parse_lsblk_pairs(line);
            let mount = fields.get("MOUNTPOINT").cloned().unwrap_or_default();
            if mount.is_empty() {
                continue;
            }

            let name = fields.get("NAME").cloned().unwrap_or_default();
            let size = Self::parse_size(fields.get("SIZE").map(String::as_str).unwrap_or("0"));
            let fs = fields
                .get("FSTYPE")
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let uuid = fields.get("UUID").cloned().unwrap_or_default();

            let (used, available) = Self::get_partition_usage(&format!("/dev/{}", name)).await;
            let percent = if size > 0 {
                ((used as f64 / size as f64) * 100.0) as u32
            } else {
                0
            };

            partitions.push(PartitionInfo {
                device: format!("/dev/{}", name),
                mount_point: mount,
                filesystem: fs,
                size_bytes: size,
                used_bytes: used,
                available_bytes: available,
                percent,
                uuid,
            });
        }

        Ok(partitions)
    }

    /// Parse one line of `lsblk -P` output (`KEY="value" KEY2="value2" ...`)
    /// into a key/value map. `lsblk` escapes any literal `"` inside a value
    /// as `\x22`, so a plain search for the next unescaped `"` is safe.
    fn parse_lsblk_pairs(line: &str) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        let mut rest = line.trim();
        while let Some(eq_pos) = rest.find('=') {
            let key = rest[..eq_pos].trim().to_string();
            rest = &rest[eq_pos + 1..];
            if !rest.starts_with('"') {
                break;
            }
            rest = &rest[1..];
            let end = match rest.find('"') {
                Some(pos) => pos,
                None => break,
            };
            map.insert(key, rest[..end].to_string());
            rest = rest[end + 1..].trim_start();
        }
        map
    }

    /// List all active MD RAID arrays with detail information.
    ///
    /// Every `mdadm` call in this file goes through `sudo` — mdadm requires
    /// root even for a read-only `--detail` ("must be super-user to perform
    /// this action" otherwise, confirmed live), and the `homevault` service
    /// account is unprivileged. Delegated to root via the sudoers NOPASSWD
    /// rule in `setup.rs::configure_sudoers`, same as ufw/wg/openvpn.
    pub async fn list_raid_arrays() -> Result<Vec<RaidArray>, DiskError> {
        let mut arrays = Vec::new();

        let mdadm_output = Command::new("sudo")
            .arg("mdadm")
            .args(["--detail", "--scan"])
            .output()
            .await?;

        for line in String::from_utf8_lossy(&mdadm_output.stdout).lines() {
            if line.contains("ARRAY") {
                let mut array = RaidArray {
                    name: String::new(),
                    level: String::new(),
                    state: String::new(),
                    devices: Vec::new(),
                    size_bytes: 0,
                    used_bytes: 0,
                    chunk_size: 0,
                };

                for part in line.split_whitespace() {
                    if part.starts_with("/dev/md") {
                        array.name = part.replace("UUID=", "").replace(']', "");
                    }
                }

                if !array.name.is_empty() {
                    // `mdadm --detail --scan`'s ARRAY line only carries
                    // `metadata=`/`UUID=` on this mdadm version — no
                    // `level=`/`state=` tokens, confirmed live (both always
                    // came out empty). Get them from `--detail <device>`
                    // instead, same call `get_raid_detail` already makes
                    // for size/devices.
                    if let Ok(detail) = Self::get_raid_detail(&array.name).await {
                        array.size_bytes = detail.0;
                        array.used_bytes = detail.1;
                        array.devices = detail.2;
                        array.level = detail.3;
                        array.state = detail.4;
                    }
                    arrays.push(array);
                }
            }
        }

        Ok(arrays)
    }

    /// List USB and other removable disks by filtering on transport type.
    pub async fn list_removable_disks() -> Result<Vec<RemovableDisk>, DiskError> {
        let mut removable_disks = Vec::new();

        let output = Command::new("lsblk")
            .args(["-n", "-o", "NAME,SIZE,FSTYPE,MOUNTPOINT,LABEL,UUID,TRAN"])
            .output()
            .await?;

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 6 {
                continue;
            }

            let transport = parts.get(6).unwrap_or(&"");
            if !Self::is_removable_transport(transport) {
                continue;
            }

            let name = parts[0];
            let size = Self::parse_size(parts.get(1).unwrap_or(&"0"));
            let filesystem = parts.get(2).unwrap_or(&"").to_string();
            let mount_point = parts.get(3).map(|s| s.to_string());
            let label = parts
                .get(4)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let uuid = parts
                .get(5)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            let is_mounted = mount_point.as_ref().map(|s| !s.is_empty()).unwrap_or(false);

            removable_disks.push(RemovableDisk {
                device: format!("/dev/{}", name),
                name: label
                    .clone()
                    .unwrap_or_else(|| format!("USB Device {}", name)),
                size_bytes: size,
                filesystem: if filesystem.is_empty() {
                    "Unknown".to_string()
                } else {
                    filesystem
                },
                mount_point,
                label,
                uuid,
                is_mounted,
            });
        }

        Ok(removable_disks)
    }

    /// Returns `true` if the transport type is considered removable.
    fn is_removable_transport(transport: &str) -> bool {
        matches!(
            transport,
            "usb" | "ata" | "sata" | "scsi" | "nvme" | "firewire" | "sd" | "mmc"
        )
    }

    /// Mount a removable disk with user-level ownership (uid=1000,gid=1000).
    /// Creates the mount point directory if it does not exist.
    pub async fn mount_removable_disk(
        &self,
        device: &str,
        mount_point: &str,
    ) -> Result<(), DiskError> {
        // `sudo mkdir`, not `std::fs::create_dir_all` — the mount point may be
        // under a root-owned directory (e.g. /mnt/) and the unprivileged
        // homevault process cannot create subdirectories there.
        let mkdir_output = Command::new("sudo")
            .arg("mkdir")
            .args(["-p", mount_point])
            .output()
            .await?;
        if !mkdir_output.status.success() {
            let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to create mount point '{}': {}",
                mount_point, stderr
            )));
        }
        let output = Command::new("mount")
            .args(["-o", "uid=1000,gid=1000", device, mount_point])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!("Mount failed: {}", stderr)));
        }
        Ok(())
    }

    /// Unmount a removable disk by its mount point.
    pub async fn unmount_removable_disk(&self, mount_point: &str) -> Result<(), DiskError> {
        let output = Command::new("umount").arg(mount_point).output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!("Unmount failed: {}", stderr)));
        }
        Ok(())
    }

    /// Mount a disk (including non-removable) at the given mount point.
    pub async fn mount_disk(&self, device: &str, mount_point: &str) -> Result<(), DiskError> {
        // `sudo mkdir`, not `std::fs::create_dir_all` — same reason as mount_removable_disk.
        let mkdir_output = Command::new("sudo")
            .arg("mkdir")
            .args(["-p", mount_point])
            .output()
            .await?;
        if !mkdir_output.status.success() {
            let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to create mount point '{}': {}",
                mount_point, stderr
            )));
        }
        Command::new("mount")
            .args([device, mount_point])
            .output()
            .await?;
        Ok(())
    }

    /// Unmount a disk by its mount point.
    pub async fn unmount_disk(&self, mount_point: &str) -> Result<(), DiskError> {
        Command::new("umount").arg(mount_point).output().await?;
        Ok(())
    }

    /// Retrieve SMART attribute values and overall health via `smartctl`.
    /// Returns `(status, read_errors, write_errors, bad_sectors)`.
    async fn get_smart_details(device_name: &str) -> (String, u64, u64, u64) {
        let device = format!("/dev/{}", device_name);
        let output = crate::smart::tokio_command(&device, &["-A"]).output().await;

        if output.is_err() {
            return ("Unknown".to_string(), 0, 0, 0);
        }

        let stdout_val = output.unwrap();
        let stdout = String::from_utf8_lossy(&stdout_val.stdout);
        let mut status = "Unknown".to_string();
        let mut read_errors = 0u64;
        let mut write_errors = 0u64;
        let mut bad_sectors = 0u64;

        for line in stdout.lines() {
            if line.contains("ID") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 10 {
                    let attr_name = parts.get(1).unwrap_or(&"");
                    let raw_value = parts.get(9).unwrap_or(&"0");

                    match *attr_name {
                        "Read_Error_Rate" | "Hardware_ECC_Corrected" => {
                            read_errors = raw_value.parse().unwrap_or(0);
                        }
                        "Write_Error_Rate" => {
                            write_errors = raw_value.parse().unwrap_or(0);
                        }
                        "Reallocated_Sector_Ct" | "Current_Pending_Sector" => {
                            bad_sectors += raw_value.parse().unwrap_or(0);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Get overall health status
        if let Ok(health_output) = crate::smart::tokio_command(&device, &["-H"]).output().await {
            let health_stdout = String::from_utf8_lossy(&health_output.stdout);
            if health_stdout.contains("PASSED") || health_stdout.contains("OK") {
                status = "Healthy".to_string();
            } else if health_stdout.contains("FAILED") {
                status = "Failed".to_string();
            }
        }

        (status, read_errors, write_errors, bad_sectors)
    }

    /// Read the disk temperature from SMART attribute `Temperature_Celsius` or `Airflow_Temperature_Cel`.
    async fn get_disk_temperature(device_name: &str) -> Result<f64, DiskError> {
        let device = format!("/dev/{}", device_name);
        let output = crate::smart::tokio_command(&device, &["-A"])
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("Temperature_Celsius") || line.contains("Airflow_Temperature_Cel") {
                let parts: Vec<&str> = line.split_whitespace().filter(|s| !s.is_empty()).collect();
                if let Some(temp) = parts.last() {
                    if let Ok(t) = temp.parse::<f64>() {
                        return Ok(t);
                    }
                }
            }
        }
        Err(DiskError::Parse("Temperature not found".to_string()))
    }

    /// Detect the role of a disk by checking RAID membership, ZFS, Btrfs, and mount points.
    async fn detect_disk_role(device_name: &str) -> String {
        // Check if disk is part of a RAID array
        if let Ok(mdadm_output) = Command::new("sudo")
            .arg("mdadm")
            .args(["--examine", &format!("/dev/{}", device_name)])
            .output()
            .await
        {
            let stdout = String::from_utf8_lossy(&mdadm_output.stdout);
            if stdout.contains("raid") {
                return "RAID".to_string();
            }
        }

        // Check if disk is part of ZFS pool
        if let Ok(zpool_output) = Command::new("zpool")
            .args(["status", device_name])
            .output()
            .await
        {
            if zpool_output.status.success() {
                return "ZFS Pool".to_string();
            }
        }

        // Check if disk is part of Btrfs filesystem
        if let Ok(btrfs_output) = Command::new("btrfs")
            .args(["filesystem", "show", &format!("/dev/{}", device_name)])
            .output()
            .await
        {
            if btrfs_output.status.success() {
                return "Btrfs".to_string();
            }
        }

        // Check mount points to determine role
        if let Ok(lsblk_output) = Command::new("lsblk")
            .args(["-n", "-o", "MOUNTPOINT", &format!("/dev/{}", device_name)])
            .output()
            .await
        {
            let stdout = String::from_utf8_lossy(&lsblk_output.stdout);
            let mount_point = stdout.trim();
            if mount_point == "/" {
                return "System".to_string();
            } else if !mount_point.is_empty() {
                return "Data".to_string();
            }
        }

        "Unassigned".to_string()
    }

    /// Count partitions on a disk by subtracting the device line from `lsblk` output.
    async fn count_partitions(device_name: &str) -> usize {
        if let Ok(output) = Command::new("lsblk")
            .args(["-n", "-o", "NAME", &format!("/dev/{}", device_name)])
            .output()
            .await
        {
            let lines = String::from_utf8_lossy(&output.stdout).lines().count();
            lines.saturating_sub(1)
        } else {
            0
        }
    }

    /// Query `df -B1` for used and available bytes of a partition.
    async fn get_partition_usage(path: &str) -> (u64, u64) {
        if let Ok(output) = Command::new("df").args(["-B1", path]).output().await {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();
            if lines.len() >= 2 {
                let parts: Vec<&str> = lines[1].split_whitespace().collect();
                if parts.len() >= 4 {
                    let used = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let available = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                    return (used, available);
                }
            }
        }
        (0, 0)
    }

    /// Query `mdadm --detail` for array size and member device paths, and `df`
    /// (via the array's mount point, if any) for the actual used bytes.
    async fn get_raid_detail(
        name: &str,
    ) -> Result<(u64, u64, Vec<String>, String, String), DiskError> {
        let output = Command::new("sudo")
            .arg("mdadm")
            .args(["--detail", name])
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut size = 0u64;
        let mut devices = Vec::new();
        let mut level = String::new();
        let mut state = String::new();

        for line in stdout.lines() {
            if line.contains("Array Size") {
                if let Some(s) = line.split(':').nth(1) {
                    size = Self::parse_size(s.split_whitespace().next().unwrap_or("0"));
                }
            } else if line.contains("Raid Level") {
                if let Some(s) = line.split(':').nth(1) {
                    level = s.trim().to_string();
                }
            } else if line.trim_start().starts_with("State ") {
                // Exact field line ("           State : clean"), not the
                // "Number Major Minor RaidDevice State" member-table header
                // (which starts with "Number", not "State").
                if let Some(s) = line.split(':').nth(1) {
                    state = s.trim().to_string();
                }
            } else if line.contains("/dev/") {
                // Member-device lines list the path as the LAST field
                // ("   0   8   32   0   active sync   /dev/loop0"), not the
                // first — the first line of the whole output is the device
                // header itself ("/dev/md/hvtest1:"), which is a single
                // token equal to both the first *and* last, so it's told
                // apart by its trailing ':'. Confirmed live: taking the
                // first token instead always captured the header and never
                // a real member device.
                if let Some(last) = line.split_whitespace().last() {
                    if last.starts_with("/dev/") && !last.ends_with(':') {
                        devices.push(last.to_string());
                    }
                }
            }
        }

        // `df` resolves the array device to its mount point and reports real
        // usage; it returns (0, 0) if the array isn't mounted.
        let (used, _available) = Self::get_partition_usage(name).await;

        Ok((size, used, devices, level, state))
    }

    /// Parse a human-readable size string (e.g. `"1T"`, `"512G"`, `"256M"`) into bytes.
    fn parse_size(s: &str) -> u64 {
        let s = s.trim();
        let multiplier: u64 = if s.ends_with('G') {
            1024 * 1024 * 1024
        } else if s.ends_with('M') {
            1024 * 1024
        } else if s.ends_with('K') || s.ends_with("KB") {
            1024
        } else if s.ends_with('T') {
            1024 * 1024 * 1024 * 1024
        } else {
            1
        };

        // `lsblk`'s SIZE column follows the process locale for the decimal
        // separator (e.g. "465,8G" under a French locale) — `str::parse`
        // only accepts '.', so every size silently parsed to 0 on such a
        // system. Confirmed live. Normalize ',' to '.' before parsing.
        let num_str = s
            .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
            .replace(',', ".");
        // Multiply in f64 first, cast to u64 last — casting the parsed
        // value to u64 *before* multiplying (as this did previously) drops
        // the fractional part up front, so "465.8G" silently reported as
        // if it were "465G" (undercounting by ~0.8 GiB here, and by up to
        // ~1 TiB for a T-scale size). Caught by a unit test whose expected
        // value was computed the mathematically correct way and didn't
        // match this function's own (wrong) output.
        (num_str.parse::<f64>().unwrap_or(0.0) * multiplier as f64) as u64
    }

    /// Returns the system disk device (where root / is mounted).
    /// This is used to exclude the OS disk from RAID selection.
    pub async fn get_system_disk() -> Result<String, DiskError> {
        let output = Command::new("df").args(["-h", "/"]).output().await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();

        if lines.len() >= 2 {
            let parts: Vec<&str> = lines[1].split_whitespace().collect();
            if let Some(device) = parts.first() {
                // Returned RAW ("/dev/sda1", "/dev/nvme0n1p2", "/dev/mapper/…")
                // on purpose: callers match it against candidate names via
                // `same_device` — a plain trailing-digit trim cannot tell
                // "/dev/nvme0n1p2" (a partition of "nvme0n1") apart from a
                // whole disk named "…1".
                return Ok(device.to_string());
            }
        }

        Err(DiskError::Parse("Could not detect system disk".to_string()))
    }

    /// `true` when `candidate` names `disk_name` itself or one of its
    /// partitions: the sd/virtio style (`sda1` = disk + bare digits) and the
    /// nvme/mmc/loop style (`nvme0n1p2` = disk + `p` + digits). Rejects
    /// lookalikes: `"sdaa"` is not `"sda"`, `"nvme1n1"` is not `"nvme0n1"`.
    fn is_same_disk(candidate: &str, disk_name: &str) -> bool {
        let disk_name = disk_name.trim_start_matches("/dev/");
        if candidate.trim_start_matches("/dev/") == disk_name {
            return true;
        }
        let rest = match candidate
            .trim_start_matches("/dev/")
            .strip_prefix(disk_name)
        {
            Some(rest) => rest,
            None => return false,
        };
        let rest = rest.strip_prefix('p').unwrap_or(rest);
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
    }

    /// Symmetric whole-disk equality: two device paths name the same disk
    /// when either side is the other's base (the system disk detector works
    /// from `df /`, which often reports the *partition* while the candidate
    /// is the whole disk — and vice versa).
    fn same_device(a: &str, b: &str) -> bool {
        Self::is_same_disk(a, b) || Self::is_same_disk(b, a)
    }

    /// Create a new mdadm RAID array from the request's members/spares and
    /// format + mount + persist it. Called only for the `ext4`/`xfs`
    /// technologies — `create_virtual_disk` owns the technology dispatch
    /// (ZFS pools and native Btrfs filesystems never reach this function).
    ///
    /// Validates level/counts/name/filesystem itself (defense in depth — the
    /// API gate already ran `validate_virtual_disk_request`), then runs
    /// `mdadm --create --run`, waits for the array to actually report ready
    /// (polling `/sys/block/mdX/md/array_state` instead of sleeping a fixed
    /// 2 s), and reports the initial resync percent through `progress`.
    pub async fn create_raid(
        req: &MdadmRaidRequest,
        progress: Option<&dyn CreateProgress>,
    ) -> Result<(), DiskError> {
        let filesystem = req.filesystem.as_str();

        // Validate RAID level
        let valid_levels = ["raid0", "raid1", "raid5", "raid6", "raid10"];
        if !valid_levels.contains(&req.level.as_str()) {
            return Err(DiskError::Parse(format!(
                "Invalid RAID level: {}",
                req.level
            )));
        }

        // Validate minimum devices for each level
        let min_devices = min_devices_for_level(&req.level);
        let missing_slots = if req.allow_missing {
            let min = min_devices;
            if req.members.len() < min {
                usize::max(
                    max_missing_for_level(&req.level),
                    min.saturating_sub(req.members.len()),
                )
            } else {
                0
            }
        } else {
            0
        };
        let member_count = req.members.len() + missing_slots;

        if member_count < min_devices || (missing_slots > 0 && member_count > min_devices) {
            let hint = if missing_slots > 0 {
                format!(
                    " a degraded create needs exactly the level minimum (missing slots up to {})",
                    max_missing_for_level(&req.level)
                )
            } else {
                String::new()
            };
            return Err(DiskError::Parse(format!(
                "RAID {} requires at least {} devices{}",
                req.level, min_devices, hint
            )));
        }

        if missing_slots > max_missing_for_level(&req.level) {
            return Err(DiskError::Parse(format!(
                "RAID {} cannot start degraded by more than {} device(s)",
                req.level,
                max_missing_for_level(&req.level)
            )));
        }

        // RAID 10 is a stripe of mirror pairs — a clean pair split is required.
        if req.level == "raid10" && member_count % 2 != 0 {
            return Err(DiskError::Parse(
                "RAID 10 requires an even number of member slots".to_string(),
            ));
        }

        // Spares only make sense on levels with redundancy, and mdadm wants
        // the resolved device paths.
        if !req.spares.is_empty() && req.level == "raid0" {
            return Err(DiskError::Parse(
                "A RAID 0 array has no redundancy — hot spares are not supported".to_string(),
            ));
        }
        let members: Vec<String> = req
            .members
            .iter()
            .chain(req.spares.iter())
            .map(|d| {
                if d.starts_with("/dev/") {
                    d.clone()
                } else {
                    format!("/dev/{}", d)
                }
            })
            .collect();

        // Validate that all devices exist
        for device in &members {
            if !std::path::Path::new(device).exists() {
                return Err(DiskError::DeviceNotFound(format!(
                    "Device {} does not exist",
                    device
                )));
            }
        }

        // Validate name
        if req.name.is_empty() || req.name.len() > 32 {
            return Err(DiskError::Parse(
                "RAID name must be 1-32 characters".to_string(),
            ));
        }
        if !req
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(DiskError::Parse(
                "RAID name can only contain alphanumeric, dash and underscore characters"
                    .to_string(),
            ));
        }

        // Validate filesystem
        let valid_filesystems = ["ext4", "xfs", "btrfs"];
        if !valid_filesystems.contains(&filesystem) {
            return Err(DiskError::Parse(format!(
                "Invalid filesystem: {}. Supported for mdadm: ext4, xfs, btrfs",
                filesystem
            )));
        }

        // Create RAID array with mdadm
        let raid_device = format!("/dev/md/{}", req.name);
        let level_str = req.level.replace("raid", "");
        let raid_count = member_count.to_string();
        let spare_count = req.spares.len().to_string();

        let mut args = vec![
            "--create",
            &raid_device,
            "--level",
            &level_str,
            "--raid-devices",
            &raid_count,
        ];
        if !req.spares.is_empty() {
            args.push("--spare-devices");
            args.push(&spare_count);
        }
        // A write-intent bitmap makes later resyncs take seconds instead of
        // rewriting the whole array; redundant levels only (raid0 has no
        // bitmap support).
        if req.level != "raid0" {
            args.push("--bitmap=internal");
        }
        // Pin the metadata name so the array keeps a stable identity across
        // machines (mdadm otherwise prefixes the local hostname).
        args.push("--name");
        args.push(&req.name);
        // mdadm's v1.2 metadata (its own default) sits at the start of
        // the device, which triggers an interactive "may not be suitable
        // as a boot device — Continue creating array?" confirmation on
        // stdin. `Command::output()` gives the child a closed stdin, so
        // that prompt was reading EOF and aborting the create every
        // time — confirmed live, 100% failure rate. `--run` skips it.
        args.push("--run");

        // Member devices first; a degraded create gets literal `missing`
        // placeholders so mdadm reserves the slot; spares come last.
        for device in members.iter().take(req.members.len()) {
            args.push(device);
        }
        for _ in 0..missing_slots {
            args.push("missing");
        }
        for device in members.iter().skip(req.members.len()) {
            args.push(device);
        }

        let output = Command::new("sudo")
            .arg("mdadm")
            .args(&args)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("mdadm --create {}", raid_device),
                stderr: friendly_tool_error("Array creation", &stderr),
            });
        }

        // Wait for the array to become active, streaming the initial resync
        // percent when one starts. Replaces the blind 2 s sleep: the array is
        // ready as soon as the kernel says so, whatever the hardware.
        Self::wait_for_array_ready(&raid_device, progress).await;

        // Format with filesystem. If this (or the mount below) fails, tear
        // the array back down rather than propagating the error with `?` —
        // confirmed live: a too-small device made `mkfs.xfs` fail, and the
        // `?` short-circuit left the just-created array running, unformatted,
        // still holding the member devices — the very next create attempt on
        // those same devices then failed with "Device or resource busy",
        // and the orphaned array was invisible except by noticing it in the
        // arrays list and deleting it manually.
        if let Some(p) = progress {
            p.report(55, &format!("Writing the {} filesystem", filesystem));
        }
        if let Err(e) = Self::format_device(&raid_device, filesystem).await {
            Self::teardown_failed_array(&raid_device, &members, &e.to_string()).await;
            return Err(e);
        }

        // Mount it and make it survive a reboot. Without this the array exists
        // but is unreachable (no mount point) and mdadm has no record to
        // reassemble it after the next boot.
        if let Some(p) = progress {
            p.report(75, "Mounting and persisting to boot config");
        }
        if let Err(e) = Self::mount_and_persist_raid(&raid_device, &req.name, filesystem).await {
            Self::teardown_failed_array(&raid_device, &members, &e.to_string()).await;
            return Err(e);
        }

        Ok(())
    }

    /// Blocks (bounded) until a freshly created md array reports a usable
    /// state in `/sys/block/mdX/md/array_state`, streaming the initial
    /// resync percent through `progress` while it runs. Tolerates a missing
    /// sysfs path (very old md): it simply returns after the timeout.
    async fn wait_for_array_ready(raid_device: &str, progress: Option<&dyn CreateProgress>) {
        let md = std::fs::canonicalize(raid_device)
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()));
        let Some(md) = md else { return };
        let state_path = format!("/sys/block/{}/md/array_state", md);
        let sync_path = format!("/sys/block/{}/md/sync_action", md);
        let completed_path = format!("/sys/block/{}/md/sync_completed", md);

        // 40 polls × 500 ms = a hard 20 s ceiling; a healthy create reports
        // "clean"/"active" in well under a second.
        for poll in 0..40 {
            match tokio::fs::read_to_string(&state_path).await {
                Ok(state) => {
                    let state = state.trim();
                    if matches!(state, "clean" | "active" | "active-idle" | "readonly") {
                        // Resync (initial parity build) may already be racing in
                        // the background: surface its percent while we're here.
                        if let Some(p) = progress {
                            if let Ok(action) = tokio::fs::read_to_string(&sync_path).await {
                                let action = action.trim();
                                if matches!(action, "resync" | "rebuild" | "check" | "repair") {
                                    if let Ok(done) =
                                        tokio::fs::read_to_string(&completed_path).await
                                    {
                                        let done = done.trim();
                                        let pos = done.split_whitespace().next().unwrap_or("0");
                                        let total = done.split_whitespace().nth(1).unwrap_or("1");
                                        if let (Ok(pos), Ok(total)) =
                                            (pos.parse::<f64>(), total.parse::<f64>())
                                        {
                                            if total > 0.0 {
                                                let pct = ((pos / total) * 100.0).clamp(0.0, 100.0);
                                                p.report(
                                                    10 + (pct * 0.3) as u8,
                                                    &format!("Initial resync running: {:.0}%", pct),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        return;
                    }
                }
                Err(_) => {}
            }
            let _ = poll;
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        }
        // Not ready within 20 s: mdadm create already succeeded, so continue
        // anyway (format/mount failures are handled separately) — but the
        // array may still be syncing in the background.
        if let Some(p) = progress {
            p.report(40, "Array is assembling; continuing setup");
        }
    }

    /// Stops and wipes a just-created array whose own setup (format/mount)
    /// failed partway through, so a failed `create_raid` doesn't leave a
    /// live array holding the member devices hostage. Best-effort — errors
    /// here are secondary to the original failure already being returned;
    /// they are logged so a stuck teardown is diagnosable from the journal.
    async fn teardown_failed_array(raid_device: &str, devices: &[String], cause: &str) {
        tracing::warn!(
            "Virtual disk setup failed ({}): tearing down {} to release {} device(s)",
            cause,
            raid_device,
            devices.len()
        );
        if let Err(e) = Command::new("sudo")
            .arg("mdadm")
            .args(["--stop", raid_device])
            .output()
            .await
        {
            tracing::warn!("Failed to stop broken array {}: {}", raid_device, e);
        }
        for device in devices {
            if let Err(e) = Command::new("sudo")
                .arg("mdadm")
                .args(["--zero-superblock", device])
                .output()
                .await
            {
                tracing::warn!("Failed to zero superblock on {}: {}", device, e);
            }
        }
    }

    /// Mount a freshly created RAID array at `RAID_MOUNT_BASE/{name}` and persist it
    /// across reboots: an `/etc/mdadm/mdadm.conf` entry so mdadm reassembles
    /// the array on boot, and an `/etc/fstab` entry (keyed by filesystem UUID,
    /// not the `/dev/mdX` device name, since that numbering isn't guaranteed
    /// stable across reboots) so it gets mounted automatically. Both entries
    /// are tagged with a `# homevault-raid:{name}` marker so `delete_raid` can
    /// cleanly remove them later.
    async fn mount_and_persist_raid(
        raid_device: &str,
        name: &str,
        filesystem: &str,
    ) -> Result<(), DiskError> {
        let mount_point = format!("{}/{}", crate::config::paths::RAID_MOUNT_BASE, name);

        // `sudo mkdir`, not `tokio::fs::create_dir_all` — `/mnt` is root-owned
        // and the unprivileged homevault process cannot create subdirectories there.
        let mkdir_output = Command::new("sudo")
            .arg("mkdir")
            .args(["-p", &mount_point])
            .output()
            .await?;
        if !mkdir_output.status.success() {
            let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to create RAID mount point '{}': {}",
                mount_point, stderr
            )));
        }

        // `sudo mount`, not a bare `mount` — same reason as the `sudo mkdir`
        // above: only root can mount, and this must keep working if the
        // service is ever run unprivileged (it currently runs as root, but
        // every other privileged call in this module is `sudo`-wrapped).
        let mount_output = Command::new("sudo")
            .arg("mount")
            .args([raid_device, &mount_point])
            .output()
            .await?;
        if !mount_output.status.success() {
            let stderr = String::from_utf8_lossy(&mount_output.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("mount {} {}", raid_device, mount_point),
                stderr: friendly_tool_error("Mount", &stderr),
            });
        }

        // fstab/mdadm.conf edits are serialized (PERSIST_LOCK) so a create
        // and a teardown can never interleave read-modify-write cycles.
        let _guard = PERSIST_LOCK.lock().await;

        let marker = format!("# homevault-raid:{}", name);

        // Record the array so mdadm reassembles it automatically on boot.
        if let Ok(scan_output) = Command::new("sudo")
            .arg("mdadm")
            .args(["--detail", "--scan", raid_device])
            .output()
            .await
        {
            if scan_output.status.success() {
                let array_line = String::from_utf8_lossy(&scan_output.stdout)
                    .trim()
                    .to_string();
                if !array_line.is_empty() {
                    let _ =
                        Self::append_marked_entry("/etc/mdadm/mdadm.conf", &marker, &array_line)
                            .await;
                }
            }
        }

        // Add an fstab entry keyed by UUID so the array mounts automatically on boot.
        // `nofail` keeps boot from hanging if the array can't be assembled for some reason.
        if let Ok(uuid_output) = Command::new("sudo")
            .arg("blkid")
            .args(["-s", "UUID", "-o", "value", raid_device])
            .output()
            .await
        {
            let uuid = String::from_utf8_lossy(&uuid_output.stdout)
                .trim()
                .to_string();
            if !uuid.is_empty() {
                let fstab_line = format!(
                    "UUID={} {} {} defaults,nofail 0 2",
                    uuid, mount_point, filesystem
                );
                if let Err(e) = Self::append_marked_entry("/etc/fstab", &marker, &fstab_line).await
                {
                    return Err(e);
                }
                // A malformed fstab line breaks the next boot — worth a
                // verify + rollback rather than fire-and-forget. `findmnt
                // --verify` parses /etc/fstab exactly like mount does;
                // if verification fails, undo the append so the host boots
                // normally, and fail the creation.
                let verify = Command::new("sudo")
                    .arg("findmnt")
                    .args(["--verify", "--fstab"])
                    .output()
                    .await;
                match verify {
                    Ok(v) if v.status.success() => {}
                    Ok(v) => {
                        let stderr = String::from_utf8_lossy(&v.stderr);
                        Self::remove_marked_entry("/etc/fstab", &marker).await;
                        return Err(DiskError::CommandFailed {
                            command: "findmnt --verify --fstab".to_string(),
                            stderr: friendly_tool_error(
                                "The boot fstab entry failed validation (rolled back)",
                                &stderr,
                            ),
                        });
                    }
                    // `findmnt` simply not installed: keep the historical
                    // behavior (entry written, unverified).
                    Err(_) => {}
                }
            }
        }

        // mdadm arrays are assembled early in boot from the initramfs, so a
        // new `/etc/mdadm/mdadm.conf` entry only takes effect once the boot
        // image is regenerated. Best-effort across the two common packagers
        // (Debian's update-initramfs, dracut elsewhere); a missing tool or a
        // failed regeneration must not fail the creation — the fstab entry
        // still mounts the array once mdadm.conf is picked up by any later
        // initramfs rebuild.
        let _ = Command::new("sudo")
            .arg("update-initramfs")
            .arg("-u")
            .output()
            .await;
        let _ = Command::new("sudo").arg("dracut").arg("-f").output().await;

        Ok(())
    }

    /// Append a `marker` comment line followed by `entry` to `path`, unless a
    /// line with that exact marker is already present (keeps this idempotent).
    /// Uses `sudo tee` because system files like `/etc/fstab` are root-owned.
    async fn append_marked_entry(path: &str, marker: &str, entry: &str) -> Result<(), DiskError> {
        let existing = tokio::fs::read_to_string(path).await.unwrap_or_default();
        if existing.lines().any(|l| l == marker) {
            return Ok(());
        }

        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(marker);
        updated.push('\n');
        updated.push_str(entry);
        updated.push('\n');

        Self::write_file_as_root(path, &updated).await?;
        Ok(())
    }

    /// Remove the `# homevault-raid:{name}` marker and the entry line right
    /// after it from `path`, if present. Used by `delete_raid` to clean up
    /// `/etc/fstab` and `/etc/mdadm/mdadm.conf` so they don't keep a stale
    /// reference to an array that no longer exists.
    async fn remove_marked_entry(path: &str, marker: &str) {
        let Ok(existing) = tokio::fs::read_to_string(path).await else {
            return;
        };

        let lines: Vec<&str> = existing.lines().collect();
        let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
        let mut i = 0;
        let mut changed = false;
        while i < lines.len() {
            if lines[i] == marker {
                // Skip the marker and, if present, the entry line right after it.
                changed = true;
                i += 2;
                continue;
            }
            kept.push(lines[i]);
            i += 1;
        }

        if changed {
            let mut updated = kept.join("\n");
            if !updated.is_empty() {
                updated.push('\n');
            }
            let _ = Self::write_file_as_root(path, &updated).await;
        }
    }

    /// Write content to a root-owned file via `sudo tee`, since the
    /// unprivileged homevault process cannot write to system files directly.
    async fn write_file_as_root(path: &str, content: &str) -> Result<(), DiskError> {
        use tokio::io::AsyncWriteExt;
        let mut child = Command::new("sudo")
            .args(["tee", path])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| DiskError::Parse(format!("Failed to spawn sudo tee: {}", e)))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(content.as_bytes()).await.map_err(|e| {
                DiskError::Parse(format!("Failed to write to sudo tee stdin: {}", e))
            })?;
        }
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| DiskError::Parse(format!("Failed to wait for sudo tee: {}", e)))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!("sudo tee failed: {}", stderr)));
        }
        Ok(())
    }

    /// Create a native ZFS pool from whole disks, mounting the root dataset
    /// at `RAID_MOUNT_BASE/{name}` and giving it appliance-standard dataset
    /// properties (lz4 compression, no atime, POSIX ACLs matching the shares'
    /// setfacl flow, xattr in the dnode).
    ///
    /// Level mapping: raid0 → stripe, raid1 → mirror, raid5 → raidz1,
    /// raid6 → raidz2, and raid10 → **striped mirrors** (one `mirror` vdev
    /// per device pair). This used to map raid10 → a single raidz3 — the
    /// closest-capacity label on paper, but a completely different layout
    /// (triple parity on all N disks instead of independent 2-disk mirrors).
    async fn create_zfs_pool_from_devices(
        name: &str,
        raid_level: &str,
        devices: Vec<String>,
    ) -> Result<(), DiskError> {
        if name.is_empty() || name.len() > 32 {
            return Err(DiskError::Parse(
                "Pool name must be 1-32 characters".to_string(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(DiskError::Parse(
                "Pool name can only contain alphanumeric, dash and underscore characters"
                    .to_string(),
            ));
        }

        // Validate ZFS pool exists
        if devices.is_empty() {
            return Err(DiskError::Parse("ZFS requires at least 1 disk".to_string()));
        }

        // Validate that all devices exist
        for device in &devices {
            let device_path = if device.starts_with("/dev/") {
                device.clone()
            } else {
                format!("/dev/{}", device)
            };
            if !std::path::Path::new(&device_path).exists() {
                return Err(DiskError::DeviceNotFound(format!(
                    "Device {} does not exist",
                    device
                )));
            }
        }

        // Map RAID level to ZFS layout.
        // raidz1: single parity (like raid5)
        // raidz2: double parity (like raid6)
        // mirror: 2-disk mirror (like raid1) / striped mirrors (like raid10)
        let pool_type = match raid_level {
            "raid0" => "",
            "raid1" => "mirror",
            "raid5" => "raidz1",
            "raid6" => "raidz2",
            "raid10" => "mirror",
            _ => "",
        };

        if raid_level == "raid10" && devices.len() % 2 != 0 {
            return Err(DiskError::Parse(
                "A ZFS raid10 pool needs pairs of devices (even count)".to_string(),
            ));
        }

        let mount_point = format!("{}/{}", crate::config::paths::RAID_MOUNT_BASE, name);
        // `sudo mkdir` — /mnt is root-owned (same reasoning as
        // mount_and_persist_raid below); zfs mounts over it happily.
        let mkdir_output = Command::new("sudo")
            .arg("mkdir")
            .args(["-p", &mount_point])
            .output()
            .await?;
        if !mkdir_output.status.success() {
            let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("mkdir -p {}", mount_point),
                stderr: friendly_tool_error("Mount point creation", &stderr),
            });
        }

        // `zpool create [-f] [-m mountpoint] [-O props] <poolname> <layout>`
        // — the pool name comes BEFORE the vdev type. Passing the type first
        // made zpool read the pool name as a device ("cannot open '<name>':
        // no such device in /dev"), so every redundant level (mirror /
        // raidzN) failed outright — only raid0 (empty type) ever worked.
        let mut args: Vec<String> = vec!["zpool".into(), "create".into(), "-f".into()];
        args.push("-m".into());
        args.push(mount_point.clone());
        // One shared root-dataset policy; kept mirrored with
        // `pools.rs::create_zfs_advanced_pool` so every ZFS volume the
        // appliance creates behaves the same for the shares' ACL tooling.
        for kv in [
            "compression=lz4",
            "atime=off",
            "acltype=posixacl",
            "xattr=sa",
        ] {
            args.push("-O".into());
            args.push(kv.into());
        }
        args.push(name.to_string());

        if !pool_type.is_empty() && raid_level != "raid10" {
            args.push(pool_type.to_string());
        }

        // raid10: one `mirror` keyword group per device pair:
        // `zpool create -f tank mirror d1 d2 mirror d3 d4 …`
        if raid_level == "raid10" {
            for pair in devices.chunks(2) {
                args.push("mirror".to_string());
                for device in pair {
                    args.push(if device.starts_with("/dev/") {
                        device.clone()
                    } else {
                        format!("/dev/{}", device)
                    });
                }
            }
        } else {
            let device_refs: Vec<String> = devices
                .iter()
                .map(|s| {
                    if s.starts_with("/dev/") {
                        s.clone()
                    } else {
                        format!("/dev/{}", s)
                    }
                })
                .collect();
            args.extend(device_refs);
        }

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = Command::new("sudo").args(&arg_refs).output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("zpool create {}", name),
                stderr: friendly_tool_error("ZFS pool creation", &stderr),
            });
        }

        Ok(())
    }

    /// Format a device with specified filesystem
    pub async fn format_device(device: &str, filesystem: &str) -> Result<(), DiskError> {
        let command = match filesystem {
            "ext4" => "mkfs.ext4",
            "xfs" => "mkfs.xfs",
            "btrfs" => "mkfs.btrfs",
            "ntfs" => "mkfs.ntfs",
            "zfs" => {
                // ZFS is handled differently - it's a pool-based filesystem
                return Self::create_zfs_pool(device).await;
            }
            _ => {
                return Err(DiskError::Parse(format!(
                    "Unsupported filesystem: {}",
                    filesystem
                )))
            }
        };

        let mut args = vec!["-f"];
        if filesystem == "ext4" {
            args = vec!["-F"];
        }
        args.push(device);

        // mkfs.* on a real block device needs root regardless of the caller's
        // own group memberships (same reasoning as pools.rs/encryption.rs —
        // confirmed live against a device with realistic 660 root:disk
        // permissions). This is reached from the encryption feature to
        // format a freshly-opened /dev/mapper/* LUKS device.
        let output = Command::new("sudo")
            .arg(command)
            .args(&args)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to format device: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Create a ZFS pool on a RAID device
    async fn create_zfs_pool(device: &str) -> Result<(), DiskError> {
        // Extract pool name from device path
        let pool_name = device
            .trim_start_matches("/dev/md/")
            .trim_start_matches("/dev/");

        // Create ZFS pool
        let output = Command::new("zpool")
            .args(["create", "-f", pool_name, device])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to create ZFS pool: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Delete a RAID array
    pub async fn delete_raid(name: &str) -> Result<(), DiskError> {
        let device = if name.starts_with("/dev/") {
            name.to_string()
        } else {
            format!("/dev/md/{}", name)
        };

        // Check if the RAID exists before trying to delete
        let raid_exists = std::path::Path::new(&device).exists()
            || Command::new("sudo")
                .arg("mdadm")
                .args(["--detail", "--scan"])
                .output()
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(&device))
                .unwrap_or(false);

        if !raid_exists {
            return Err(DiskError::DeviceNotFound(format!(
                "RAID array {} does not exist",
                name
            )));
        }

        // Unmount first — `mdadm --stop` fails with "device busy" while the
        // array is still mounted (it now always is, since create_raid mounts it).
        // `sudo umount`, not a bare `umount`: a bare one silently fails as a
        // non-root process, leaving the array mounted so `--stop` then errors
        // out and the whole delete aborts.
        let _ = Command::new("sudo")
            .arg("umount")
            .arg(&device)
            .output()
            .await;

        // Stop the array
        let stop_output = Command::new("sudo")
            .arg("mdadm")
            .args(["--stop", &device])
            .output()
            .await?;

        if !stop_output.status.success() {
            let stderr = String::from_utf8_lossy(&stop_output.stderr);
            // Continue even if stop fails - array might already be stopped
            if !stderr.contains("No such file")
                && !stderr.contains("not found")
                && !stderr.contains("inactive")
            {
                return Err(DiskError::Parse(format!("Failed to stop RAID: {}", stderr)));
            }
        }

        // Remove the mdadm.conf/fstab entries added by create_raid so they
        // don't keep referencing an array that no longer exists.
        let marker_name = device
            .trim_start_matches("/dev/md/")
            .trim_start_matches("/dev/");
        let marker = format!("# homevault-raid:{}", marker_name);
        Self::remove_marked_entry("/etc/mdadm/mdadm.conf", &marker).await;
        Self::remove_marked_entry("/etc/fstab", &marker).await;

        // Remove the mount point directory (now empty after unmount).
        let mount_point = format!("{}/{}", crate::config::paths::RAID_MOUNT_BASE, marker_name);
        let _ = Command::new("sudo")
            .arg("rmdir")
            .arg(&mount_point)
            .output()
            .await;

        // Get device list before zeroing (in case we need cleanup)
        let devices = Self::get_raid_devices_for_wipe(name).await;

        // Zero superblock on all devices
        for device_path in devices {
            let _ = Command::new("sudo")
                .arg("mdadm")
                .args(["--zero-superblock", &device_path])
                .output()
                .await;
        }

        Ok(())
    }

    /// Get device paths from RAID for cleanup
    async fn get_raid_devices_for_wipe(name: &str) -> Vec<String> {
        if let Ok(detail) = Self::get_raid_detail(name).await {
            detail.2
        } else {
            Vec::new()
        }
    }

    /// Get available devices for RAID creation. Every candidate (non-system)
    /// disk is listed; free ones come with `inUse: false`, and in-use ones
    /// are still shown, dimmed, with `reason` explaining what holds them —
    /// judged from real facts (see `gather_device_facts` / the single
    /// full-tree `lsblk` pass below).
    ///
    /// The busy/in-free verdict comes from a **single** `lsblk` tree scan of
    /// every block device at once (parented via `PKNAME`): the per-disk
    /// variant used to spawn one lsblk per candidate disk — an N+1 shell
    /// storm on many-bay machines. Only the three membership probes
    /// (`mdadm --examine`, `zpool status`, `btrfs show`) remain per disk,
    /// and only when the tree itself doesn't already disqualify it.
    pub async fn get_available_devices() -> Result<Vec<AvailableDevice>, DiskError> {
        let mut available = Vec::new();
        let system_disk = Self::get_system_disk().await.ok();

        // One inventory pass over the whole tree: disks AND their children
        // (partitions, LUKS, LVM…), with `PKNAME` linking each child to its
        // parent. No SMART probes here — the free/busy verdict needs none.
        let lsblk_output = Command::new("lsblk")
            .args([
                "-n",
                "-P",
                "-o",
                "NAME,TYPE,SIZE,VENDOR,MODEL,PKNAME,MOUNTPOINT,FSTYPE",
            ])
            .output()
            .await?;

        let mut rows: Vec<LsblkRow> = Vec::new();
        for line in String::from_utf8_lossy(&lsblk_output.stdout).lines() {
            let fields = Self::parse_lsblk_pairs(line);
            let name = fields.get("NAME").cloned().unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            rows.push(LsblkRow {
                name,
                r#type: fields.get("TYPE").cloned().unwrap_or_default(),
                parent: fields.get("PKNAME").filter(|p| !p.is_empty()).cloned(),
                mount_point: fields.get("MOUNTPOINT").filter(|m| !m.is_empty()).cloned(),
                fstype: fields.get("FSTYPE").filter(|f| !f.is_empty()).cloned(),
                size: fields.get("SIZE").cloned().unwrap_or_else(|| "0".into()),
                vendor: fields.get("VENDOR").cloned().unwrap_or_default(),
                model: fields.get("MODEL").cloned().unwrap_or_default(),
            });
        }

        // name → direct children index.
        let mut children_by_parent: std::collections::HashMap<&str, Vec<usize>> =
            std::collections::HashMap::new();
        for (idx, row) in rows.iter().enumerate() {
            if let Some(parent) = row.parent.as_deref() {
                children_by_parent.entry(parent).or_default().push(idx);
            }
        }

        for (idx, row) in rows.iter().enumerate() {
            if row.r#type != "disk" {
                continue;
            }

            // Skip the system disk entirely (its own role is "System" by
            // definition; it must never be offered anywhere).
            if let Some(ref sys_disk) = system_disk {
                if Self::same_device(&row.name, sys_disk) {
                    continue;
                }
            }

            // Fold the disk's own row + every descendant into facts, exactly
            // like `gather_device_facts`' per-device tree walk.
            let mut facts = DeviceFacts::default();
            let mut stack: Vec<usize> = vec![idx];
            while let Some(i) = stack.pop() {
                let r = &rows[i];
                if i != idx {
                    facts.child_count += 1;
                }
                if let Some(mount) = r.mount_point.as_deref() {
                    facts.mount_points.push(mount.to_string());
                    if mount == "/" {
                        facts.is_system_disk = true;
                    }
                }
                if let Some(fs) = r.fstype.as_deref() {
                    facts.filesystems.push(fs.to_string());
                }
                if let Some(kids) = children_by_parent.get(r.name.as_str()) {
                    stack.extend(kids.iter().copied());
                }
            }
            if let Some(sys) = system_disk.as_deref() {
                if Self::same_device(&row.name, sys) {
                    facts.is_system_disk = true;
                }
            }

            // The three membership probes only matter when the tree already
            // looks clean — a device with partitions is disqualified anyway.
            if !Self::facts_indicate_in_use(&facts) {
                let device_path = format!("/dev/{}", row.name);
                // mdadm requires root even for a read (`list_raid_arrays`'s
                // comment), while `zpool status` and `btrfs show` reject
                // non-members with a non-zero exit, so success means "member".
                if let Ok(out) = Command::new("sudo")
                    .arg("mdadm")
                    .args(["--examine", &device_path])
                    .output()
                    .await
                {
                    facts.md_member = String::from_utf8_lossy(&out.stdout).contains("raid");
                }
                if let Ok(out) = Command::new("zpool")
                    .args(["status", &device_path])
                    .output()
                    .await
                {
                    facts.zfs_member = out.status.success();
                }
                if let Ok(out) = Command::new("btrfs")
                    .args(["filesystem", "show", &device_path])
                    .output()
                    .await
                {
                    facts.btrfs_member = out.status.success();
                }
            }

            let in_use = Self::facts_indicate_in_use(&facts);
            let model = format!("{} {}", row.vendor, row.model).trim().to_string();

            available.push(AvailableDevice {
                name: format!("/dev/{}", row.name),
                size: Self::parse_size(&row.size),
                model: if model.is_empty() {
                    "Unknown disk".to_string()
                } else {
                    model
                },
                in_use,
                reason: if in_use {
                    Some(Self::in_use_reason(&facts))
                } else {
                    None
                },
                smart_status: Some(Self::get_smart_status(&row.name).await),
            });
        }

        Ok(available)
    }

    /// Quick SMART overall-health read (`smartctl -H`, one probe): returns
    /// `"Healthy"`, `"Failed"`, or `"Unknown"` when smartctl is missing or
    /// the device reports nothing intelligible. Used by the wizard's disk
    /// step so a failing disk surfaces before it enters a new array.
    async fn get_smart_status(device_name: &str) -> String {
        match crate::smart::tokio_command(&format!("/dev/{}", device_name), &["-H"])
            .output()
            .await
        {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let stderr = String::from_utf8_lossy(&o.stderr);
                if stdout.to_ascii_uppercase().contains("FAILED")
                    || stderr.to_ascii_uppercase().contains("FAILED")
                {
                    "Failed".to_string()
                } else if stdout.contains("PASSED") || stdout.contains("OK") {
                    "Healthy".to_string()
                } else {
                    "Unknown".to_string()
                }
            }
            Err(_) => "Unknown".to_string(),
        }
    }

    /// Server-side safety gate run **before any** `mdadm`/`zpool`/`mkfs`
    /// call on a virtual-disk creation request (all technologies): every
    /// candidate device must exist, must not host the system disk, and must
    /// be entirely free (`facts_indicate_in_use`).
    ///
    /// This used to trust only the wizard's own device list — the endpoint
    /// barely checked device *existence* — so a hand-built (or buggy)
    /// admin request could `mdadm --create --run` straight over a mounted,
    /// partitioned, or filesystem-carrying disk (or the system disk) with no
    /// confirmation at all. mdadm's own "may not be suitable…" prompt is
    /// skipped by `--run`, so the overwrite would be silent.
    pub async fn validate_pool_devices(devices: &[String]) -> Result<(), DiskError> {
        let system_disk = Self::get_system_disk().await.ok();

        for device in devices {
            let device_path = if device.starts_with("/dev/") {
                device.clone()
            } else {
                format!("/dev/{}", device)
            };
            if !std::path::Path::new(&device_path).exists() {
                return Err(DiskError::DeviceNotFound(format!(
                    "Device {} does not exist",
                    device
                )));
            }

            let facts = Self::gather_device_facts(&device_path, system_disk.as_deref()).await;
            if Self::facts_indicate_in_use(&facts) {
                return Err(DiskError::Parse(format!(
                    "Device {} is not available — {}",
                    device_path,
                    Self::in_use_reason(&facts)
                )));
            }
        }

        Ok(())
    }

    /// Collects the observable facts about one block device for the
    /// free/in-use verdict: its lsblk tree (partitions, mounts, filesystem
    /// signatures) plus mdadm (`--examine`), ZFS (`zpool status`) and Btrfs
    /// (`filesystem show`) membership. Every probe is tolerant: an absent
    /// tool simply doesn't count as membership. `system_disk` (the base-disk
    /// path from `get_system_disk`, if detected) flags the rootfs disk; a
    /// descendant mounted at `/` is flagged directly by the tree scan.
    async fn gather_device_facts(device_path: &str, system_disk: Option<&str>) -> DeviceFacts {
        let name = device_path
            .rsplit('/')
            .next()
            .unwrap_or(device_path)
            .to_string();
        let mut facts = DeviceFacts::default();

        if let Some(sys) = system_disk {
            if Self::same_device(&name, sys) {
                facts.is_system_disk = true;
            }
        }

        // One tree-level pass: the device's own row first, then every
        // descendant (partitions, LUKS, LVM…). Children automatically
        // disqualify — a pool member must be a bare leaf block device.
        if let Ok(output) = Command::new("lsblk")
            .args(["-n", "-P", "-o", "NAME,TYPE,MOUNTPOINT,FSTYPE", device_path])
            .output()
            .await
        {
            let mut first_row = true;
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let fields = Self::parse_lsblk_pairs(line);
                let row_name = fields.get("NAME").cloned().unwrap_or_default();
                if row_name.is_empty() {
                    continue;
                }
                if first_row {
                    first_row = false;
                } else {
                    facts.child_count += 1;
                }
                if let Some(mount) = fields.get("MOUNTPOINT").filter(|m| !m.is_empty()) {
                    facts.mount_points.push(mount.clone());
                    if mount == "/" {
                        facts.is_system_disk = true;
                    }
                }
                if let Some(fs) = fields.get("FSTYPE").filter(|f| !f.is_empty()) {
                    facts.filesystems.push(fs.clone());
                }
            }
        }

        // Same membership probes `detect_disk_role` predicated on — mdadm
        // requires root even for a read (`list_raid_arrays`'s comment),
        // while `zpool status` and `btrfs show` reject non-members with a
        // non-zero exit, so success means "member".
        if let Ok(out) = Command::new("sudo")
            .arg("mdadm")
            .args(["--examine", device_path])
            .output()
            .await
        {
            facts.md_member = String::from_utf8_lossy(&out.stdout).contains("raid");
        }
        if let Ok(out) = Command::new("zpool")
            .args(["status", device_path])
            .output()
            .await
        {
            facts.zfs_member = out.status.success();
        }
        if let Ok(out) = Command::new("btrfs")
            .args(["filesystem", "show", device_path])
            .output()
            .await
        {
            facts.btrfs_member = out.status.success();
        }

        facts
    }

    /// Pure judgment over the collected facts: `true` when the device is
    /// **not** free to consume. Unit-tested; `gather_device_facts` does the
    /// syscalls, this decides.
    fn facts_indicate_in_use(facts: &DeviceFacts) -> bool {
        facts.is_system_disk
            || facts.child_count > 0
            || !facts.mount_points.is_empty()
            || !facts.filesystems.is_empty()
            || facts.md_member
            || facts.zfs_member
            || facts.btrfs_member
    }

    /// The first reason the facts disqualify the device — used in the
    /// validation error so the API answer explains itself.
    fn in_use_reason(facts: &DeviceFacts) -> String {
        if facts.is_system_disk {
            return "it hosts the system (root filesystem)".to_string();
        }
        if facts.child_count > 0 {
            return format!(
                "it has {} child device{} (partitions, LUKS, LVM…)",
                facts.child_count,
                if facts.child_count == 1 { "" } else { "s" }
            );
        }
        if facts.md_member {
            return "it carries an mdadm RAID superblock".to_string();
        }
        if facts.zfs_member {
            return "it is a member of a ZFS pool".to_string();
        }
        if facts.btrfs_member {
            return "it is a member of a Btrfs filesystem".to_string();
        }
        if let Some(mount) = facts.mount_points.first() {
            return format!("it is mounted at '{}'", mount);
        }
        if let Some(fs) = facts.filesystems.first() {
            return format!("it carries a '{}' filesystem signature", fs);
        }
        "it is not reported as free".to_string()
    }

    /// Rebuild a RAID array. If `replacement_device` is given, it is added as a
    /// new member — mdadm automatically starts recovery onto it if the array is
    /// missing a device. Without a replacement, a degraded array can't actually
    /// be repaired (there's no failed member to resync onto), so this only runs
    /// a data-integrity scrub on healthy arrays.
    pub async fn rebuild_raid(
        name: &str,
        replacement_device: Option<&str>,
    ) -> Result<(), DiskError> {
        let device = if name.starts_with("/dev/") {
            name.to_string()
        } else {
            format!("/dev/md/{}", name)
        };

        // Check if the RAID exists
        let exists = std::path::Path::new(&device).exists();
        if !exists {
            return Err(DiskError::DeviceNotFound(format!(
                "RAID array {} does not exist",
                name
            )));
        }

        // Get array state to determine whether it's actually missing a device.
        let state_output = Command::new("sudo")
            .arg("mdadm")
            .args(["--detail", &device])
            .output()
            .await?;

        let state = String::from_utf8_lossy(&state_output.stdout);
        let is_degraded = state.contains("degraded") || state.contains("recovery");

        if let Some(replacement) = replacement_device {
            let replacement_path = if replacement.starts_with("/dev/") {
                replacement.to_string()
            } else {
                format!("/dev/{}", replacement)
            };
            if !std::path::Path::new(&replacement_path).exists() {
                return Err(DiskError::DeviceNotFound(format!(
                    "Replacement device {} does not exist",
                    replacement
                )));
            }

            // Same gate as creation: the replacement must be a whole, free
            // disk — not the system disk, not partitioned, not carrying
            // another array's superblock. `mdadm --add` used to accept any
            // device silently here.
            if let Err(e) = Self::validate_pool_devices(&[replacement_path.clone()]).await {
                return Err(e);
            }

            let output = Command::new("sudo")
                .arg("mdadm")
                .args(["--manage", &device, "--add", &replacement_path])
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(DiskError::CommandFailed {
                    command: format!("mdadm --manage {} --add {}", device, replacement_path),
                    stderr: friendly_tool_error("Adding the replacement disk", &stderr),
                });
            }

            return Ok(());
        }

        if is_degraded {
            return Err(DiskError::Parse(
                "This array is degraded and missing a device. Select a replacement disk to rebuild it.".to_string(),
            ));
        }

        // Healthy array: run an integrity scrub.
        let output = Command::new("sudo")
            .arg("mdadm")
            .args(["--action", "check", &device])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("mdadm --action check {}", device),
                stderr: friendly_tool_error("Consistency check", &stderr),
            });
        }

        Ok(())
    }

    /// Grows an existing mdadm array onto an additional device: `mdadm
    /// --add` plugs the disk in (as a spare for degraded arrays), then
    /// `--grow --raid-devices N+1` promotes it to an active member. The
    /// reshape runs in the background for hours on real parity arrays — this
    /// only kicks it off, it does not wait.
    ///
    /// raid0 has no redundancy to grow into and raid10's reshape is poorly
    /// supported upstream, so both are refused; the caller picks a spare or
    /// the Storage UI restricts the surface.
    pub async fn grow_raid(name: &str, new_device: &str) -> Result<(), DiskError> {
        let device = if name.starts_with("/dev/") {
            name.to_string()
        } else {
            format!("/dev/md/{}", name)
        };

        if !std::path::Path::new(&device).exists() {
            return Err(DiskError::DeviceNotFound(format!(
                "RAID array {} does not exist",
                name
            )));
        }

        // Current level + member count, from `mdadm --detail`.
        let detail = Command::new("sudo")
            .arg("mdadm")
            .args(["--detail", &device])
            .output()
            .await?;
        let detail_text = String::from_utf8_lossy(&detail.stdout).to_string();
        let mut raid_devices: Option<usize> = None;
        let mut level: Option<String> = None;
        for line in detail_text.lines() {
            if let Some(rest) = line.strip_prefix("Raid Devices :") {
                raid_devices = rest.trim().parse().ok();
            }
            if let Some(rest) = line.strip_prefix("Raid Level :") {
                level = Some(rest.trim().to_string());
            }
        }
        let (Some(raid_devices), Some(level)) = (raid_devices, level) else {
            return Err(DiskError::Parse(format!(
                "Could not read the layout of array '{}'",
                name
            )));
        };

        if !matches!(level.as_str(), "raid1" | "raid5" | "raid6") {
            return Err(DiskError::Parse(format!(
                "{} arrays cannot be grown by adding a disk (only raid1/5/6 are supported)",
                level
            )));
        }

        // Resolve + gate the new device exactly like creation: whole, free,
        // not the system disk, no leftover superblock.
        let new_path = if new_device.starts_with("/dev/") {
            new_device.to_string()
        } else {
            format!("/dev/{}", new_device)
        };
        if !std::path::Path::new(&new_path).exists() {
            return Err(DiskError::DeviceNotFound(format!(
                "Device {} does not exist",
                new_device
            )));
        }
        Self::validate_pool_devices(&[new_path.clone()]).await?;

        // Plug the disk in, then promote it to an active member. For a
        // degraded array --add starts recovery onto it immediately; --grow
        // is what raises the member count (an no-op change for raid1 sizes,
        // but it is how raid5/6 restripes).
        let add = Command::new("sudo")
            .arg("mdadm")
            .args(["--manage", &device, "--add", &new_path])
            .output()
            .await?;
        if !add.status.success() {
            let stderr = String::from_utf8_lossy(&add.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("mdadm --manage {} --add {}", device, new_path),
                stderr: friendly_tool_error("Adding the disk", &stderr),
            });
        }

        let count = (raid_devices + 1).to_string();
        let grow = Command::new("sudo")
            .arg("mdadm")
            .args(["--grow", &device, "--raid-devices", &count])
            .output()
            .await?;
        if !grow.status.success() {
            let stderr = String::from_utf8_lossy(&grow.stderr);
            return Err(DiskError::CommandFailed {
                command: format!("mdadm --grow {} --raid-devices {}", device, count),
                stderr: friendly_tool_error("Growing the array", &stderr),
            });
        }

        Ok(())
    }

    /// Check SMART capabilities for a device via `smartctl -c`.
    pub async fn get_smart_capabilities(device_name: &str) -> SmartCapabilities {
        let device = format!("/dev/{}", device_name);
        let output = crate::smart::tokio_command(&device, &["-c"]).output().await;

        let stdout = match output {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout).to_string();
                if s.contains("SMART support is: Unavailable") {
                    return SmartCapabilities {
                        available: false,
                        supports_short_test: false,
                        supports_long_test: false,
                        supports_conveyance_test: false,
                    };
                }
                s
            }
            Err(_) => {
                return SmartCapabilities {
                    available: false,
                    supports_short_test: false,
                    supports_long_test: false,
                    supports_conveyance_test: false,
                }
            }
        };

        SmartCapabilities {
            available: true,
            supports_short_test: stdout.contains("Short self-test routine"),
            supports_long_test: stdout.contains("Extended self-test routine"),
            supports_conveyance_test: stdout.contains("Conveyance self-test routine"),
        }
    }

    /// Start a SMART self-test. `test_type` is one of `"short"`, `"long"`, `"conveyance"`.
    pub async fn start_smart_test(device_name: &str, test_type: &str) -> Result<(), DiskError> {
        let caps = Self::get_smart_capabilities(device_name).await;
        if !caps.available {
            return Err(DiskError::SmartNotAvailable(
                "SMART is not available on this device".to_string(),
            ));
        }

        let flag = match test_type {
            "short" => "short",
            "long" => "long",
            "conveyance" => "conveyance",
            _ => {
                return Err(DiskError::Parse(format!(
                    "Unknown test type: {}",
                    test_type
                )))
            }
        };

        let device = format!("/dev/{}", device_name);
        let output = crate::smart::tokio_command(&device, &["-t", flag])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("a test is already running") {
                return Err(DiskError::SmartTestInProgress);
            }
            return Err(DiskError::Parse(format!(
                "Failed to start SMART test: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Get the current (or last completed) SMART self-test status via `smartctl -l selftest`.
    pub async fn get_smart_test_status(device_name: &str) -> SmartTestStatus {
        let device = format!("/dev/{}", device_name);
        let output = crate::smart::tokio_command(&device, &["-l", "selftest"])
            .output()
            .await;

        let stdout = match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(_) => {
                return SmartTestStatus {
                    in_progress: false,
                    progress_pct: 0,
                    remaining_minutes: 0,
                    last_result: None,
                }
            }
        };

        // Parse the self-test log — most recent entry first.
        let mut in_progress = false;
        let mut progress_pct: u8 = 0;
        let mut remaining_minutes: u32 = 0;
        let mut last_result: Option<String> = None;

        for line in stdout.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('#') {
                continue;
            }
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            if tokens.len() < 7 {
                continue;
            }

            let status_field = tokens[3..].join(" ");
            let remaining_str = tokens
                .iter()
                .find(|t| t.ends_with('%'))
                .copied()
                .unwrap_or("00%");

            // Extract Remaining %: "90%" → 90
            if let Some(pct_str) = remaining_str.strip_suffix('%') {
                progress_pct = pct_str.parse().unwrap_or(0);
            }

            if status_field.contains("in progress") {
                in_progress = true;
                // Estimate remaining minutes based on test type + progress
                // We can't determine exact type from log, use a reasonable default
                remaining_minutes = ((progress_pct as u32) * 2) / 10; // rough estimate
            } else {
                in_progress = false;
                remaining_minutes = 0;
                last_result = Some(status_field);
            }

            // In smartctl -l selftest, the first (#1) entry is the most recent.
            // Break after first entry to only get the latest test result.
            break;
        }

        SmartTestStatus {
            in_progress,
            progress_pct,
            remaining_minutes,
            last_result,
        }
    }

    /// Resolve remaining SMART test minutes from `smartctl -c` output for a known test type.
    pub async fn get_smart_test_remaining(device_name: &str) -> u32 {
        let device = format!("/dev/{}", device_name);
        let output = crate::smart::tokio_command(&device, &["-c"]).output().await;

        let stdout = match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(_) => return 0,
        };

        // Look for the polling time lines indicating remaining time
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.contains("minutes") && trimmed.contains("(") && trimmed.contains(")") {
                if let Some(paren) = trimmed.split('(').nth(1) {
                    if let Some(min) = paren.split(')').next() {
                        if let Ok(minutes) = min.trim().parse::<u32>() {
                            return minutes;
                        }
                    }
                }
            }
        }
        0
    }

    // -- Virtual disk creation (the unified dispatcher) ----------------------

    /// Pure (no-I/O) validation of a virtual-disk creation request. Runs in
    /// the API handler for immediate 400s, then again inside the background
    /// job as defense in depth (request validation also ran once before the
    /// device list could change underneath it).
    ///
    /// Rules:
    ///   - `name`: 1-32 chars, alphanum/dash/underscore
    ///   - `level`: raid0|raid1|raid5|raid6|raid10
    ///   - `filesystem`: ext4|xfs|btrfs|zfs (the technology dispatch)
    ///   - `members`: at least the level minimum — spares don't count
    ///   - `allow_missing`: only redundant mdadm levels, up to the level's
    ///     missing-slot budget, and the placeholder fill brings the total
    ///     exactly to the minimum (no exotic layouts)
    ///   - `spares`: mdadm (ext4/xfs) only, never on raid0
    ///   - raid10 on mdadm/ZFS requires an even member count
    pub fn validate_virtual_disk_request(req: &VirtualDiskRequest) -> Result<(), DiskError> {
        if req.name.is_empty() || req.name.len() > 32 {
            return Err(DiskError::Parse(
                "Virtual disk name must be 1-32 characters".to_string(),
            ));
        }
        if !req
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(DiskError::Parse(
                "Virtual disk name can only contain alphanumeric, dash and underscore characters"
                    .to_string(),
            ));
        }

        let valid_levels = ["raid0", "raid1", "raid5", "raid6", "raid10"];
        if !valid_levels.contains(&req.level.as_str()) {
            return Err(DiskError::Parse(format!(
                "Invalid RAID level: {}",
                req.level
            )));
        }

        match req.filesystem.as_str() {
            "ext4" | "xfs" | "btrfs" | "zfs" => {}
            other => {
                return Err(DiskError::Parse(format!(
                    "Invalid filesystem: {}. Supported: ext4, xfs, btrfs, zfs",
                    other
                )))
            }
        }

        let min = min_devices_for_level(&req.level);
        if req.members.is_empty() {
            return Err(DiskError::Parse(
                "At least one member device is required".to_string(),
            ));
        }

        // Slot accounting ONCE, before every rule below: with `allow_missing`
        // the request's mdadm member-slot count includes the `missing`
        // placeholders, so a degraded create must never be measured against
        // the raw member count (a below-minimum member list is exactly the
        // point of a degraded create).
        let slots_total = md_member_slots(req);

        // Technological rules the dispatcher will enforce; spelled out here
        // so the API rejects before the job ever starts. They run BEFORE the
        // device-count floor on purpose: they are the sharper diagnostics, and
        // a 3-member raid10 must say "pairs", not "needs 4 devices".
        if req.filesystem == "zfs" {
            if req.level == "raid10" && slots_total % 2 != 0 {
                return Err(DiskError::Parse(
                    "A ZFS raid10 pool needs pairs of devices (even count)".to_string(),
                ));
            }
        }
        if req.filesystem == "ext4" || req.filesystem == "xfs" {
            if req.level == "raid10" && slots_total % 2 != 0 {
                return Err(DiskError::Parse(
                    "RAID 10 requires an even number of members".to_string(),
                ));
            }
        }

        if req.filesystem == "zfs" || req.filesystem == "btrfs" {
            if !req.spares.is_empty() || req.allow_missing {
                return Err(DiskError::Parse(
                    "Native ZFS/Btrfs storage doesn't support hot spares or missing-device creates"
                        .to_string(),
                ));
            }
        } else if !req.spares.is_empty() && req.level == "raid0" {
            return Err(DiskError::Parse(
                "A RAID 0 array has no redundancy — hot spares are not supported".to_string(),
            ));
        } else if req.allow_missing {
            let slots = slots_total - req.members.len();
            if slots == 0 {
                return Err(DiskError::Parse(
                    "The array already meets its minimum — nothing to create degraded".to_string(),
                ));
            }
            if slots > max_missing_for_level(&req.level) {
                return Err(DiskError::Parse(format!(
                    "RAID {} cannot start degraded by more than {} device(s)",
                    req.level,
                    max_missing_for_level(&req.level)
                )));
            }
            if slots_total > min {
                return Err(DiskError::Parse(
                    "A degraded create needs exactly the level minimum".to_string(),
                ));
            }
        } else if req.members.len() < min {
            return Err(DiskError::Parse(format!(
                "RAID {} requires at least {} devices (spares don't count)",
                req.level, min
            )));
        }

        Ok(())
    }

    /// `true` when `name` is already claimed by an existing mdadm array:
    /// the `/dev/md/<name>` node exists, or `mdadm --detail --scan` lists an
    /// array whose metadata name resolves to the same label. Checked before
    /// the job starts so a duplicate name is an immediate 400, not a failure
    /// after minutes of mdadm work.
    pub async fn array_name_taken(name: &str) -> bool {
        let device = format!("/dev/md/{}", name);
        if std::path::Path::new(&device).exists() {
            return true;
        }
        // Metadata names look like `hostname:label` — compare the bare label.
        if let Ok(out) = Command::new("sudo")
            .arg("mdadm")
            .args(["--detail", "--scan"])
            .output()
            .await
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                for token in line.split_whitespace() {
                    if let Some(named) = token.strip_prefix("name=") {
                        let bare = named.rsplit(':').next().unwrap_or(named);
                        if bare.eq_ignore_ascii_case(name) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// The one dispatch point for creating a virtual disk — everything the
    /// POST /storage/raid path used to inline (or hide in different helpers
    /// per technology): validation, name-collision check, the free-device
    /// gate (re-run inside the job so a device taken between the API check
    /// and the actual job fails cleanly), the technology dispatch, and the
    /// progress milestones for the tracking job.
    ///
    /// Returns the mount point (`RAID_MOUNT_BASE/<name>`); the caller (the
    /// handler's background job) turns it into the completion message.
    pub async fn create_virtual_disk(
        req: &VirtualDiskRequest,
        progress: &dyn CreateProgress,
    ) -> Result<String, DiskError> {
        // Defense in depth: the handler validated, the device list may have
        // changed since (or the payload could come from another caller).
        Self::validate_virtual_disk_request(req)?;

        if Self::array_name_taken(&req.name).await {
            return Err(DiskError::Parse(format!(
                "An array or pool named '{}' already exists",
                req.name
            )));
        }

        let mut devices: Vec<String> = req.members.clone();
        devices.extend(req.spares.iter().cloned());
        // Re-gate every device inside the job (close the validate-then-use
        // window: the handler checked long before mdadm/zpool actually run).
        Self::validate_pool_devices(&devices).await?;
        progress.report(8, "Validating target devices");

        let mount_point = format!("{}/{}", crate::config::paths::RAID_MOUNT_BASE, &req.name);

        match req.filesystem.as_str() {
            "zfs" => {
                progress.report(15, &format!("Creating the ZFS pool '{}'", req.name));
                Self::create_zfs_pool_from_devices(&req.name, &req.level, req.members.clone())
                    .await?;
            }
            "btrfs" => {
                progress.report(
                    15,
                    &format!("Creating the native Btrfs filesystem '{}'", req.name),
                );
                // `mkfs.btrfs` on real devices blocks a while — keep the job
                // executor responsive by staying out of the async runtime.
                let name = req.name.clone();
                let level = req.level.clone();
                let members = req.members.clone();
                let mount = mount_point.clone();
                tokio::task::spawn_blocking(move || {
                    crate::pools::PoolService::create_pool(
                        &name,
                        crate::pools::PoolType::Btrfs,
                        &level,
                        &members,
                        Some(&mount),
                    )
                    .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| DiskError::Parse(format!("Internal error: {}", e)))?
                .map_err(|e| DiskError::Parse(e))?;
            }
            // ext4 / xfs — mdadm underneath.
            "ext4" | "xfs" => {
                progress.report(
                    12,
                    &format!("Creating the mdadm array /dev/md/{}", req.name),
                );
                Self::create_raid(
                    &MdadmRaidRequest {
                        name: req.name.clone(),
                        level: req.level.clone(),
                        members: req.members.clone(),
                        spares: req.spares.clone(),
                        allow_missing: req.allow_missing,
                        filesystem: req.filesystem.clone(),
                    },
                    // Map create_raid's own resync percentage window (0-100)
                    // into the 15-55 band of the overall job.
                    Some(&SubProgress {
                        inner: progress,
                        from: 15,
                        to: 55,
                    }),
                )
                .await?;
            }
            other => {
                return Err(DiskError::Parse(format!(
                    "Invalid filesystem: {}. Supported: ext4, xfs, btrfs, zfs",
                    other
                )))
            }
        }

        progress.report(100, "Virtual disk ready");
        Ok(mount_point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `create_raid`'s validation runs level -> min-devices -> device-existence
    // -> name -> filesystem, all before it ever spawns `mdadm`/`sudo` — so a
    // test case that fails at (or before) a given step never touches the
    // system, no root/mdadm required. `/dev/null` stands in for "a device
    // that exists" (its own existence check just needs *a* real path).

    /// Build a `MdadmRaidRequest` with the members the tests occupy.
    fn mdreq(name: &str, level: &str, members: Vec<&str>, filesystem: &str) -> MdadmRaidRequest {
        MdadmRaidRequest {
            name: name.to_string(),
            level: level.to_string(),
            members: members.into_iter().map(String::from).collect(),
            spares: Vec::new(),
            allow_missing: false,
            filesystem: filesystem.to_string(),
        }
    }

    #[tokio::test]
    async fn test_create_raid_invalid_level() {
        let req = mdreq("hvtest", "raid99", vec!["/dev/null", "/dev/null"], "ext4");
        let err = DiskService::create_raid(&req, None).await.unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("Invalid RAID level")));
    }

    #[tokio::test]
    async fn test_create_raid_too_few_devices() {
        // raid5 needs at least 3 devices.
        let req = mdreq("hvtest", "raid5", vec!["/dev/null", "/dev/null"], "ext4");
        let err = DiskService::create_raid(&req, None).await.unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("at least 3 devices")));
    }

    #[tokio::test]
    async fn test_create_raid_device_not_found() {
        let req = mdreq(
            "hvtest",
            "raid1",
            vec!["/dev/null", "/dev/this-device-does-not-exist-hv-test"],
            "ext4",
        );
        let err = DiskService::create_raid(&req, None).await.unwrap_err();
        assert!(matches!(err, DiskError::DeviceNotFound(_)));
    }

    #[tokio::test]
    async fn test_create_raid_invalid_name() {
        let req = mdreq("bad name!", "raid1", vec!["/dev/null", "/dev/null"], "ext4");
        let err = DiskService::create_raid(&req, None).await.unwrap_err();
        assert!(
            matches!(err, DiskError::Parse(ref m) if m.contains("1-32 characters") || m.contains("alphanumeric"))
        );
    }

    #[tokio::test]
    async fn test_create_raid_name_too_long() {
        let req = mdreq(
            &"a".repeat(33),
            "raid1",
            vec!["/dev/null", "/dev/null"],
            "ext4",
        );
        let err = DiskService::create_raid(&req, None).await.unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("1-32 characters")));
    }

    #[tokio::test]
    async fn test_create_raid_invalid_filesystem() {
        let req = mdreq(
            "hvtestvalidname",
            "raid1",
            vec!["/dev/null", "/dev/null"],
            "fat32",
        );
        let err = DiskService::create_raid(&req, None).await.unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("Invalid filesystem")));
    }

    #[test]
    fn test_parse_size_gigabytes_dot() {
        assert_eq!(
            DiskService::parse_size("465.8G"),
            (465.8 * 1024.0 * 1024.0 * 1024.0) as u64
        );
    }

    #[test]
    fn test_parse_size_gigabytes_comma_locale() {
        // Regression test: `lsblk` under a comma-decimal locale (e.g. French)
        // reports "465,8G" — confirmed live this silently parsed to 0 before
        // the fix, making every disk size in the RAID/Storage UI show 0.
        assert_eq!(
            DiskService::parse_size("465,8G"),
            DiskService::parse_size("465.8G")
        );
        assert!(DiskService::parse_size("465,8G") > 0);
    }

    #[test]
    fn test_parse_size_megabytes() {
        assert_eq!(DiskService::parse_size("512M"), 512 * 1024 * 1024);
    }

    #[test]
    fn test_parse_size_terabytes() {
        assert_eq!(DiskService::parse_size("1T"), 1024 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_size_kilobytes() {
        assert_eq!(DiskService::parse_size("256K"), 256 * 1024);
    }

    #[test]
    fn test_parse_size_invalid_input() {
        assert_eq!(DiskService::parse_size(""), 0);
        assert_eq!(DiskService::parse_size("abc"), 0);
    }

    // -- same_device (system-disk matching) ---------------------------------

    #[test]
    fn test_same_device_matches_a_disk_and_its_partitions() {
        // The system detector reports the PARTITION; candidates are usually
        // the whole disk (and sometimes the partition itself).
        let sys = "/dev/nvme0n1p2";
        assert!(DiskService::same_device("/dev/nvme0n1", sys));
        assert!(DiskService::same_device(sys, sys));
        // Two different partitions of the same disk: different devices, and
        // each carries its own filesystem anyway (see facts child/fs checks).
        assert!(!DiskService::same_device("/dev/nvme0n1p1", sys));
        let sys = "/dev/sda1";
        assert!(DiskService::same_device("/dev/sda", sys));
        assert!(DiskService::same_device("/dev/sda12", sys));
        // Bare names too (the lsblk inventory reports them without /dev/).
        assert!(DiskService::same_device("sda", "/dev/sda2"));
    }

    #[test]
    fn test_same_device_rejects_lookalikes_and_other_disks() {
        let sys = "/dev/sda1";
        assert!(!DiskService::same_device("/dev/sdaa", sys));
        assert!(!DiskService::same_device("/dev/sdb", sys));
        assert!(!DiskService::same_device("/dev/nvme0n1", sys));
        assert!(!DiskService::same_device("/dev/nvme1n1", "/dev/nvme0n1p2"));
        // Substring without the digit-run shape: "nvme0" is not "nvme0n1".
        assert!(!DiskService::same_device("/dev/nvme0", "/dev/nvme0n1p2"));
    }

    // -- facts_indicate_in_use ------------------------------------------------

    #[test]
    fn test_all_clear_facts_mean_the_device_is_free() {
        assert!(!DiskService::facts_indicate_in_use(&DeviceFacts::default()));
    }

    #[test]
    fn test_every_in_use_signal_disqualifies_the_device() {
        let cases: Vec<DeviceFacts> = vec![
            DeviceFacts {
                is_system_disk: true,
                ..DeviceFacts::default()
            },
            DeviceFacts {
                child_count: 1,
                ..DeviceFacts::default()
            },
            DeviceFacts {
                mount_points: vec!["/mnt/data".to_string()],
                ..DeviceFacts::default()
            },
            DeviceFacts {
                filesystems: vec!["ext4".to_string()],
                ..DeviceFacts::default()
            },
            DeviceFacts {
                md_member: true,
                ..DeviceFacts::default()
            },
            DeviceFacts {
                zfs_member: true,
                ..DeviceFacts::default()
            },
            DeviceFacts {
                btrfs_member: true,
                ..DeviceFacts::default()
            },
        ];
        for facts in &cases {
            assert!(
                DiskService::facts_indicate_in_use(facts),
                "every collected signal must mark the device as in use: {:?}",
                facts
            );
        }
    }

    #[test]
    fn test_in_use_reason_is_non_empty_and_explains_itself() {
        let facts = DeviceFacts {
            md_member: true,
            ..DeviceFacts::default()
        };
        let reason = DiskService::in_use_reason(&facts);
        assert!(reason.contains("mdadm"));
    }

    // -- validate_virtual_disk_request --------------------------------------

    fn vreq(name: &str, level: &str, members: &[&str], filesystem: &str) -> VirtualDiskRequest {
        VirtualDiskRequest {
            name: name.to_string(),
            level: level.to_string(),
            members: members.iter().map(|s| s.to_string()).collect(),
            spares: Vec::new(),
            allow_missing: false,
            filesystem: filesystem.to_string(),
        }
    }

    #[test]
    fn test_validate_accepts_a_baseline_raid5_mdadm_request() {
        let req = vreq(
            "storage",
            "raid5",
            &["/dev/sdb", "/dev/sdc", "/dev/sdd"],
            "ext4",
        );
        DiskService::validate_virtual_disk_request(&req).unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn test_validate_rejects_an_unknown_filesystem() {
        let req = vreq("storage", "raid5", &["/dev/sdb", "/dev/sdc"], "ntfs");
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("Invalid filesystem")));
    }

    #[test]
    fn test_validate_counts_spares_out_of_the_level_minimum() {
        // raid5 with 2 members + 1 spare is not 3 members.
        let mut req = vreq("storage", "raid5", &["/dev/sdb", "/dev/sdc"], "ext4");
        req.spares = vec!["/dev/sdd".to_string()];
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("spares don't count")));
    }

    #[test]
    fn test_validate_rejects_spares_and_missing_on_native_pools() {
        let mut req = vreq(
            "tank",
            "raid6",
            &["/dev/sdb", "/dev/sdc", "/dev/sdd", "/dev/sde"],
            "zfs",
        );
        req.spares = vec!["/dev/sdf".to_string()];
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("hot spares")));

        let mut req = vreq(
            "tank",
            "raid6",
            &["/dev/sdb", "/dev/sdc", "/dev/sdd", "/dev/sde"],
            "btrfs",
        );
        req.allow_missing = true;
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("hot spares")));
    }

    #[test]
    fn test_validate_bounds_the_degraded_create() {
        // raid5, 2 members, deliberately degraded → ok (1 missing slot).
        let mut req = vreq("hv", "raid5", &["/dev/sdb", "/dev/sdc"], "ext4");
        req.allow_missing = true;
        DiskService::validate_virtual_disk_request(&req).unwrap_or_else(|e| panic!("{e}"));

        // Two missing slots on raid5 exceed the level's budget of 1.
        let mut req = vreq("hv", "raid5", &["/dev/sdb"], "ext4");
        req.allow_missing = true;
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("cannot start degraded")));
    }

    #[test]
    fn test_validate_rejects_uneven_raid10_layouts() {
        let req = vreq("hv", "raid10", &["/dev/sdb", "/dev/sdc", "/dev/sdd"], "zfs");
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("pairs of devices")));

        let req = vreq(
            "hv",
            "raid10",
            &["/dev/sdb", "/dev/sdc", "/dev/sdd"],
            "ext4",
        );
        let err = DiskService::validate_virtual_disk_request(&req).unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("even number")));
    }

    // -- friendly_tool_error + SubProgress -----------------------------------

    #[test]
    fn test_friendly_tool_error_maps_known_patterns_and_keeps_the_tail() {
        let msg = friendly_tool_error("Array creation", "mdadm: Device or resource busy");
        assert!(msg.contains("a device is busy"), "{msg}");
        assert!(msg.contains("Device or resource busy"), "{msg}");

        let msg = friendly_tool_error("Formatting", "");
        assert!(msg.contains("no details reported"), "{msg}");

        let msg = friendly_tool_error("Formatting", "line one\nline two: real reason");
        assert!(msg.ends_with("real reason"), "{msg}");
    }

    #[test]
    fn test_sub_progress_maps_a_window_onto_the_parent_scale() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct Capture(Arc<AtomicUsize>);
        impl CreateProgress for Capture {
            fn report(&self, percent: u8, _message: &str) {
                self.0.store(percent as usize, Ordering::SeqCst);
            }
        }

        let last = Arc::new(AtomicUsize::new(0));
        let capture = Capture(last.clone());
        let sub = SubProgress {
            inner: &capture,
            from: 15,
            to: 55,
        };
        sub.report(0, "start");
        assert_eq!(last.load(Ordering::SeqCst), 15);
        sub.report(100, "done");
        assert_eq!(last.load(Ordering::SeqCst), 55);
        sub.report(50, "half");
        assert_eq!(last.load(Ordering::SeqCst), 35); // 15 + 40/2
    }
}

/// Maps a sub-window (e.g. `from=15, to=55`) of an overall progress job onto
/// the 0-100 scale an inner step reports. Used by `create_virtual_disk` to
/// fold the mdadm arm's internal resync percent into the tracking job.
struct SubProgress<'a> {
    inner: &'a (dyn CreateProgress + 'a),
    from: u8,
    to: u8,
}

impl CreateProgress for SubProgress<'_> {
    fn report(&self, percent: u8, message: &str) {
        let span = i16::from(self.to) - i16::from(self.from);
        let mapped = i16::from(self.from) + (i16::from(percent) * span.max(0)) / 100;
        let mapped = mapped.clamp(i16::from(self.from), i16::from(self.to));
        self.inner.report(mapped as u8, message);
    }
}

/// One parsed `lsblk -P` row from the single full-tree inventory pass used by
/// `get_available_devices` (`PKNAME` links a child to its parent disk, so a
/// device's facts can be folded without re-spawning lsblk per disk).
#[derive(Debug, Clone, Default)]
struct LsblkRow {
    name: String,
    r#type: String,
    parent: Option<String>,
    mount_point: Option<String>,
    fstype: Option<String>,
    size: String,
    vendor: String,
    model: String,
}

/// Translates the raw stderr of a failed creation/management step into a
/// short, sentence-shaped reason for the API answer (the wizard shows this
/// text as-is). Known tool patterns get a cleaner sentence; anything else
/// keeps the tool's own first error line.
pub(crate) fn friendly_tool_error(step_label: &str, stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        return format!("{} failed (no details reported)", step_label);
    }
    let lower = trimmed.to_ascii_lowercase();
    let known = if lower.contains("device or resource busy") || lower.contains("is busy") {
        Some("a device is busy (still mounted or held by another process)")
    } else if lower.contains("too small") {
        Some("a device is too small for this layout")
    } else if lower.contains("not enough") {
        Some("not enough devices were given for this layout")
    } else if lower.contains("already used") || lower.contains("in use by") {
        Some("a device is already part of another array or pool")
    } else if lower.contains("no such device") || lower.contains("does not exist") {
        Some("a device path does not exist")
    } else if lower.contains("permission") {
        Some("a device could not be accessed (permission problem)")
    } else if lower.contains("exclusive") {
        Some("a device cannot be opened exclusively right now")
    } else {
        None
    };

    // The last non-empty stderr line usually carries the actual reason —
    // multi-line tool chatter otherwise floods dialogs and toasts.
    let tail = trimmed
        .lines()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or(trimmed)
        .to_string();

    match known {
        Some(reason) => format!("{} — {}: {}", step_label, reason, tail),
        None => format!("{} — {}", step_label, tail),
    }
}

/// Observed facts about a block device considered for a new pool / RAID set.
///
/// Gathered by `DiskService::gather_device_facts` (system calls) and judged
/// by `DiskService::facts_indicate_in_use` (pure, unit-tested): a device
/// offered to the RAID wizard — or accepted by `POST /storage/raid` — must
/// be entirely free, because `mdadm --create --run`, `mkfs` and `zpool
/// create` overwrite whatever is on the disk without any confirmation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceFacts {
    /// It hosts the root filesystem (or *is* the detected system disk).
    pub is_system_disk: bool,
    /// Number of descendants (partitions, LUKS, LVM…) in its lsblk tree.
    pub child_count: usize,
    /// Every non-empty mount point in its lsblk tree (self + descendants).
    pub mount_points: Vec<String>,
    /// Every non-empty filesystem signature in its lsblk tree.
    pub filesystems: Vec<String>,
    /// Carries an mdadm superblock (`mdadm --examine`).
    pub md_member: bool,
    /// Is a member of an imported ZFS pool (`zpool status`).
    pub zfs_member: bool,
    /// Is a member of a Btrfs filesystem (`btrfs filesystem show`).
    pub btrfs_member: bool,
}

/// A device that is eligible for use in a new RAID array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableDevice {
    pub name: String,
    pub size: u64,
    pub model: String,
    /// Whether the device is already mounted or part of an existing array.
    #[serde(rename = "inUse")]
    pub in_use: bool,
    /// Why the device is in use (a short English sentence, shown verbatim in
    /// the wizard's locked rows). `None` for free devices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Overall SMART health (`"Healthy"`, `"Failed"` or `"Unknown"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_status: Option<String>,
}
