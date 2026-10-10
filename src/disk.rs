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

/// Information about a physical disk, gathered from `lsblk` and `smartctl`.
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
        let output = Command::new("smartctl")
            .args(["-A", &format!("/dev/{}", device_name)])
            .output()
            .await;

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
        if let Ok(health_output) = Command::new("smartctl")
            .args(["-H", &format!("/dev/{}", device_name)])
            .output()
            .await
        {
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
        let output = Command::new("smartctl")
            .args(["-A", &format!("/dev/{}", device_name)])
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
                let device = device.to_string();
                let base_device = device.trim_end_matches(|c: char| c.is_ascii_digit());
                return Ok(base_device.to_string());
            }
        }

        Err(DiskError::Parse("Could not detect system disk".to_string()))
    }

    /// Create a new RAID array
    pub async fn create_raid(
        name: &str,
        level: &str,
        devices: Vec<String>,
        filesystem: &str,
    ) -> Result<(), DiskError> {
        // Handle ZFS pools separately - ZFS has native RAID support
        if filesystem == "zfs" {
            return Self::create_zfs_pool_from_devices(name, level, devices).await;
        }

        // Validate RAID level
        let valid_levels = ["raid0", "raid1", "raid5", "raid6", "raid10"];
        if !valid_levels.contains(&level) {
            return Err(DiskError::Parse(format!("Invalid RAID level: {}", level)));
        }

        // Validate minimum devices for each level
        let min_devices = match level {
            "raid0" => 2,
            "raid1" => 2,
            "raid5" => 3,
            "raid6" => 4,
            "raid10" => 4,
            _ => 2,
        };

        if devices.len() < min_devices {
            return Err(DiskError::Parse(format!(
                "RAID {} requires at least {} devices",
                level, min_devices
            )));
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

        // Validate name
        if name.is_empty() || name.len() > 32 {
            return Err(DiskError::Parse(
                "RAID name must be 1-32 characters".to_string(),
            ));
        }
        if !name
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
        let raid_device = format!("/dev/md/{}", name);
        let level_str = level.replace("raid", "");
        let devices_count = devices.len().to_string();

        let mut args = vec![
            "--create",
            &raid_device,
            "--level",
            &level_str,
            "--raid-devices",
            &devices_count,
            // mdadm's v1.2 metadata (its own default) sits at the start of
            // the device, which triggers an interactive "may not be suitable
            // as a boot device — Continue creating array?" confirmation on
            // stdin. `Command::output()` gives the child a closed stdin, so
            // that prompt was reading EOF and aborting the create every
            // time — confirmed live, 100% failure rate. `--run` skips it.
            "--run",
        ];

        // Add devices
        for device in &devices {
            args.push(device);
        }

        let output = Command::new("sudo")
            .arg("mdadm")
            .args(&args)
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to create RAID: {}",
                stderr
            )));
        }

        // Wait for array to be ready
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Format with filesystem. If this (or the mount below) fails, tear
        // the array back down rather than propagating the error with `?` —
        // confirmed live: a too-small device made `mkfs.xfs` fail, and the
        // `?` short-circuit left the just-created array running, unformatted,
        // still holding the member devices — the very next create attempt on
        // those same devices then failed with "Device or resource busy",
        // and the orphaned array was invisible except by noticing it in the
        // arrays list and deleting it manually.
        if let Err(e) = Self::format_device(&raid_device, filesystem).await {
            Self::teardown_failed_array(&raid_device, &devices).await;
            return Err(e);
        }

        // Mount it and make it survive a reboot. Without this the array exists
        // but is unreachable (no mount point) and mdadm has no record to
        // reassemble it after the next boot.
        if let Err(e) = Self::mount_and_persist_raid(&raid_device, name, filesystem).await {
            Self::teardown_failed_array(&raid_device, &devices).await;
            return Err(e);
        }

        Ok(())
    }

    /// Stops and wipes a just-created array whose own setup (format/mount)
    /// failed partway through, so a failed `create_raid` doesn't leave a
    /// live array holding the member devices hostage. Best-effort — errors
    /// here are secondary to the original failure already being returned.
    async fn teardown_failed_array(raid_device: &str, devices: &[String]) {
        let _ = Command::new("sudo")
            .arg("mdadm")
            .args(["--stop", raid_device])
            .output()
            .await;
        for device in devices {
            let _ = Command::new("sudo")
                .arg("mdadm")
                .args(["--zero-superblock", device])
                .output()
                .await;
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
            return Err(DiskError::Parse(format!(
                "RAID array created but failed to mount: {}",
                stderr
            )));
        }

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
                let _ = Self::append_marked_entry("/etc/fstab", &marker, &fstab_line).await;
            }
        }

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

    /// Create a ZFS pool with native RAID support
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

        // Map RAID level to ZFS pool type
        // zfs: stripe (no redundancy)
        // raidz1: single parity (like raid5)
        // raidz2: double parity (like raid6)
        // raidz3: triple parity
        // mirror: mirror (like raid1)
        let pool_type = match raid_level {
            "raid0" => "", // stripe - no type needed
            "raid1" => "mirror",
            "raid5" => "raidz1",
            "raid6" => "raidz2",
            "raid10" | "raidz3" => "raidz3", // closest mapping
            _ => "",
        };

        // `zpool create [-f] <poolname> [vdev-type] <devices...>` — the pool
        // name comes BEFORE the vdev type. Passing the type first made zpool
        // read the pool name as a device ("cannot open '<name>': no such
        // device in /dev"), so every redundant level (mirror / raidzN) failed
        // outright — only raid0 (empty type) ever worked.
        let mut args: Vec<&str> = vec!["zpool", "create", "-f", name];

        if !pool_type.is_empty() {
            args.push(pool_type);
        }

        // Add devices - convert String to &str
        let device_refs: Vec<&str> = devices.iter().map(|s| s.as_str()).collect();
        args.extend(device_refs);

        let output = Command::new("sudo").args(&args).output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DiskError::Parse(format!(
                "Failed to create ZFS pool: {}",
                stderr
            )));
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

    /// Get available devices for RAID creation (excluding system disk and devices in use)
    pub async fn get_available_devices() -> Result<Vec<AvailableDevice>, DiskError> {
        let mut available = Vec::new();
        let system_disk = Self::get_system_disk().await.ok();
        let disks = Self::list_disks().await?;

        for disk in disks {
            // Skip system disk
            if let Some(ref sys_disk) = system_disk {
                if disk.device.starts_with(sys_disk) {
                    continue;
                }
            }

            // Check if device is in use (mounted or part of RAID)
            let in_use = disk.role != "Available";

            available.push(AvailableDevice {
                name: disk.device.clone(),
                size: disk.size_bytes,
                model: format!("{} {}", disk.vendor, disk.model),
                in_use,
            });
        }

        Ok(available)
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

            let output = Command::new("sudo")
                .arg("mdadm")
                .args(["--manage", &device, "--add", &replacement_path])
                .output()
                .await?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(DiskError::Parse(format!(
                    "Failed to add replacement device: {}",
                    stderr
                )));
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
            return Err(DiskError::Parse(format!(
                "Failed to check RAID: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Check SMART capabilities for a device via `smartctl -c`.
    pub async fn get_smart_capabilities(device_name: &str) -> SmartCapabilities {
        let device = format!("/dev/{}", device_name);
        let output = Command::new("sudo")
            .arg("smartctl")
            .args(["-c", &device])
            .output()
            .await;

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
        let output = Command::new("sudo")
            .arg("smartctl")
            .args(["-t", flag, &device])
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
        let output = Command::new("sudo")
            .arg("smartctl")
            .args(["-l", "selftest", &device])
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
        let output = Command::new("sudo")
            .arg("smartctl")
            .args(["-c", &device])
            .output()
            .await;

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
}

#[cfg(test)]
mod tests {
    use super::*;

    // `create_raid`'s validation runs level -> min-devices -> device-existence
    // -> name -> filesystem, all before it ever spawns `mdadm`/`sudo` — so a
    // test case that fails at (or before) a given step never touches the
    // system, no root/mdadm required. `/dev/null` stands in for "a device
    // that exists" (its own existence check just needs *a* real path).

    #[tokio::test]
    async fn test_create_raid_invalid_level() {
        let err = DiskService::create_raid(
            "hvtest",
            "raid99",
            vec!["/dev/null".to_string(), "/dev/null".to_string()],
            "ext4",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("Invalid RAID level")));
    }

    #[tokio::test]
    async fn test_create_raid_too_few_devices() {
        // raid5 needs at least 3 devices.
        let err = DiskService::create_raid(
            "hvtest",
            "raid5",
            vec!["/dev/null".to_string(), "/dev/null".to_string()],
            "ext4",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("at least 3 devices")));
    }

    #[tokio::test]
    async fn test_create_raid_device_not_found() {
        let err = DiskService::create_raid(
            "hvtest",
            "raid1",
            vec![
                "/dev/null".to_string(),
                "/dev/this-device-does-not-exist-hv-test".to_string(),
            ],
            "ext4",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiskError::DeviceNotFound(_)));
    }

    #[tokio::test]
    async fn test_create_raid_invalid_name() {
        let err = DiskService::create_raid(
            "bad name!",
            "raid1",
            vec!["/dev/null".to_string(), "/dev/null".to_string()],
            "ext4",
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, DiskError::Parse(ref m) if m.contains("1-32 characters") || m.contains("alphanumeric"))
        );
    }

    #[tokio::test]
    async fn test_create_raid_name_too_long() {
        let err = DiskService::create_raid(
            &"a".repeat(33),
            "raid1",
            vec!["/dev/null".to_string(), "/dev/null".to_string()],
            "ext4",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiskError::Parse(ref m) if m.contains("1-32 characters")));
    }

    #[tokio::test]
    async fn test_create_raid_invalid_filesystem() {
        let err = DiskService::create_raid(
            "hvtestvalidname",
            "raid1",
            vec!["/dev/null".to_string(), "/dev/null".to_string()],
            "fat32",
        )
        .await
        .unwrap_err();
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
}
