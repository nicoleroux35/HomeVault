/* =============================================================================
 * File          : pools.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage-pool service for ZFS and multi-device Btrfs: scan (typed
 *   PoolInfo for every technology), create from devices with a chosen
 *   data profile, destroy, scrub start/status and import of foreign
 *   pools — the second half of the virtual-disk feature set (mdadm lives
 *   in disk.rs).
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tracing (>= 0.1)
 *
 * Usage:
 *   handlers/storage.rs virtual-disk endpoints; RAID creation dispatches
 *   Btrfs here (see docs/guides/storage.md).
 * =============================================================================
 */

//! Storage-pool service for ZFS and multi-device Btrfs (scan, create,
//! destroy, scrub, import).

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;
use thiserror::Error;
use tracing::warn;

use crate::utils::process::{run_timed, run_timed_with_stdin};

/// Deadline for a metadata read (`zpool list/status/get`, `btrfs filesystem
/// show/df`, `df`, `blkid`, `findmnt`). A pool whose disks have spun down still
/// answers well inside it.
pub const POOL_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline for a mutating pool call (`zpool create|destroy|import`,
/// `mkfs.btrfs`, `mount`, `umount`, `wipefs`, `rmdir`). `zpool create` on a set
/// of large disks is the slowest honest case; `zpool import` scans every label
/// on the box. ROADMAP X.2: none of these had a deadline, so a wedged `zpool`
/// held the calling `spawn_blocking` thread — and the create wizard's request —
/// for the life of the process.
pub const POOL_WRITE_TIMEOUT: Duration = Duration::from_secs(900);

/// Errors that can occur during storage pool operations.
#[derive(Error, Debug)]
pub enum PoolError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Pool not found: {0}")]
    PoolNotFound(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

impl From<String> for PoolError {
    /// A `run_timed` failure (deadline expired, or the tool could not be
    /// started) arrives as its own sentence — "zpool did not finish within
    /// 900 seconds" — and is reported as a command failure, not as a parse
    /// error, so the wizard can show it verbatim.
    fn from(message: String) -> Self {
        Self::CommandFailed(message)
    }
}

/// Information about a storage pool (ZFS or MD RAID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolInfo {
    pub name: String,
    #[serde(rename = "pool_type")]
    pub pool_type: PoolType,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub health: String,
    pub device_count: usize,
    #[serde(rename = "mount_point")]
    pub mount_point: Option<String>,
    /// The redundancy profile in use (e.g. "mirror", "raidz1", "raid1", "stripe").
    /// `None` when it couldn't be determined.
    #[serde(default)]
    pub raid_profile: Option<String>,
}

/// Supported pool technologies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PoolType {
    Zfs,
    Mdadm,
    Btrfs,
    Unknown,
}

/// A device (disk) that is part of a storage pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolDevice {
    pub name: String,
    #[serde(rename = "device_path")]
    pub device: String,
    pub state: String,
    #[serde(rename = "size")]
    pub size_bytes: u64,
}

/// A pool discovered on attached disks that is **not** currently imported /
/// mounted — the "foreign pool" case. The user can import it (ZFS) or mount
/// it (Btrfs) from the Storage page without recreating it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportablePool {
    /// The pool name recorded on disk (ZFS label / Btrfs label).
    pub name: String,
    #[serde(rename = "pool_type")]
    pub pool_type: PoolType,
    /// ZFS numeric pool id, when known.
    pub id: Option<String>,
    /// Device paths that carry the pool, when known.
    pub devices: Vec<String>,
    /// Reported state, e.g. `ONLINE`, `DEGRADED`, `unknown`.
    pub state: String,
    /// Free-form status/action text from `zpool import`.
    pub status: Option<String>,
}

/// Service for scanning and managing ZFS pools and MD RAID arrays.
pub struct PoolService;

impl PoolService {
    pub fn scan_pools() -> Result<Vec<PoolInfo>, PoolError> {
        let mut pools = Vec::new();

        pools.extend(Self::scan_zfs_pools()?);
        pools.extend(Self::scan_mdadm_pools()?);
        pools.extend(Self::scan_btrfs_pools()?);

        Ok(pools)
    }

    /// Lists pools present on attached disks but not currently imported.
    ///
    /// ZFS is the canonical case: `zpool import` with no arguments prints a
    /// block per importable pool. Btrfs is best-effort — `btrfs filesystem
    /// show` lists every filesystem including mounted ones, so the mounted
    /// UUIDs are subtracted first.
    pub fn list_importable_pools() -> Result<Vec<ImportablePool>, PoolError> {
        let mut pools = Vec::new();
        pools.extend(Self::list_importable_zfs_pools());
        pools.extend(Self::list_importable_btrfs_pools());
        Ok(pools)
    }

    fn list_importable_zfs_pools() -> Vec<ImportablePool> {
        // `zpool import` needs root to read the on-disk labels; a system
        // without ZFS installed simply has none.
        let output = match run_timed(
            Command::new("sudo").arg("zpool").arg("import"),
            POOL_WRITE_TIMEOUT,
        ) {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };
        if !output.status.success() {
            return Vec::new();
        }
        Self::parse_zpool_import(&String::from_utf8_lossy(&output.stdout))
    }

    fn list_importable_btrfs_pools() -> Vec<ImportablePool> {
        let output = match run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["filesystem", "show"]),
            POOL_QUERY_TIMEOUT,
        ) {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };
        if !output.status.success() {
            return Vec::new();
        }
        // Subtract filesystems that are already mounted (those aren't
        // "foreign" — `scan_btrfs_pools` already lists them).
        let mounted = Self::mounted_btrfs_uuids();
        Self::parse_btrfs_show(&String::from_utf8_lossy(&output.stdout), &mounted)
    }

    /// UUIDs of Btrfs filesystems currently mounted, from `/proc/mounts`.
    fn mounted_btrfs_uuids() -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
            return set;
        };
        for line in mounts.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 3 && fields[2] == "btrfs" {
                if let Some(uuid) = fields[0].strip_prefix("UUID=") {
                    set.insert(uuid.to_string());
                }
            }
        }
        set
    }

    /// Parses `zpool import` output into one [`ImportablePool`] per block.
    ///
    /// A block looks like:
    /// ```text
    ///    pool: tank
    ///      id: 1234567890
    ///   state: ONLINE
    ///  action: The pool can be imported using its name or numeric identifier.
    ///  config:
    ///
    ///     tank        ONLINE
    ///       sdb       ONLINE
    /// ```
    /// Multiple pools are separated by a blank line. Pure function so it can
    /// be unit-tested without `zpool` installed.
    fn parse_zpool_import(output: &str) -> Vec<ImportablePool> {
        let mut pools = Vec::new();
        let mut current: Option<ImportablePool> = None;
        let mut in_config = false;

        fn flush(pools: &mut Vec<ImportablePool>, cur: &mut Option<ImportablePool>) {
            if let Some(p) = cur.take() {
                if !p.name.is_empty() {
                    pools.push(p);
                }
            }
        }

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                // Real `zpool import` output has a blank line *between* the
                // `config:` header and the device rows, so a blank line must
                // not end the block while we're still reading the config.
                if !in_config {
                    flush(&mut pools, &mut current);
                }
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("pool:") {
                flush(&mut pools, &mut current);
                in_config = false;
                current = Some(ImportablePool {
                    name: rest.trim().to_string(),
                    pool_type: PoolType::Zfs,
                    id: None,
                    devices: Vec::new(),
                    state: "unknown".to_string(),
                    status: None,
                });
                continue;
            }
            let Some(pool) = current.as_mut() else {
                continue;
            };
            if trimmed == "config:" {
                in_config = true;
                continue;
            }
            if in_config {
                // Config rows are indented; the first token is the vdev or
                // device name, the second the state. Only absolute device
                // paths are real leaf devices (mirrors/raidz containers and
                // the pool-name row are not).
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 && parts[0].starts_with("/dev/") {
                    pool.devices.push(parts[0].to_string());
                }
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix("id:") {
                pool.id = Some(rest.trim().to_string());
            } else if let Some(rest) = trimmed.strip_prefix("state:") {
                pool.state = rest.trim().to_string();
            } else if let Some(rest) = trimmed.strip_prefix("status:") {
                pool.status = Some(rest.trim().to_string());
            }
        }
        flush(&mut pools, &mut current);
        pools
    }

    /// Parses `btrfs filesystem show` output, skipping any filesystem whose
    /// UUID is in `mounted`. Block shape:
    /// ```text
    /// Label: 'mypool'  uuid: 1111-2222
    ///     Total devices 2 FS bytes used 1.00GiB
    ///     devid    1 size 10.00GiB used 1.00GiB path /dev/sdb
    /// ```
    fn parse_btrfs_show(
        output: &str,
        mounted_uuids: &std::collections::HashSet<String>,
    ) -> Vec<ImportablePool> {
        let mut pools = Vec::new();
        let mut current: Option<ImportablePool> = None;
        let mut current_uuid: Option<String> = None;

        fn flush(
            pools: &mut Vec<ImportablePool>,
            cur: &mut Option<ImportablePool>,
            uuid: &mut Option<String>,
            mounted_uuids: &std::collections::HashSet<String>,
        ) {
            if let Some(p) = cur.take() {
                let skip = uuid
                    .take()
                    .map(|u| mounted_uuids.contains(&u))
                    .unwrap_or(false);
                if !p.name.is_empty() && !skip {
                    pools.push(p);
                }
            } else {
                uuid.take();
            }
        }

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Label:") {
                flush(&mut pools, &mut current, &mut current_uuid, mounted_uuids);
                let label = trimmed
                    .split_whitespace()
                    .nth(1)
                    .map(|s| s.trim_matches('\'').to_string())
                    .unwrap_or_default();
                let uuid = trimmed.split("uuid:").nth(1).map(|s| s.trim().to_string());
                current_uuid = uuid;
                current = Some(ImportablePool {
                    name: label,
                    pool_type: PoolType::Btrfs,
                    id: None,
                    devices: Vec::new(),
                    state: "unknown".to_string(),
                    status: None,
                });
                continue;
            }
            if let Some(pool) = current.as_mut() {
                if let Some(path) = trimmed.split("path ").nth(1) {
                    let dev = path.trim().to_string();
                    if dev.starts_with("/dev/") {
                        pool.devices.push(dev);
                    }
                }
            }
        }
        flush(&mut pools, &mut current, &mut current_uuid, mounted_uuids);
        pools
    }

    /// Imports a foreign pool. ZFS: `zpool import [-N] <name>` (optionally
    /// renamed). Btrfs: mounts the first member device at the standard pool
    /// mount point and persists it in `/etc/fstab`.
    pub fn import_pool(
        name: &str,
        pool_type: PoolType,
        new_name: Option<&str>,
    ) -> Result<(), PoolError> {
        Self::validate_pool_name(name)?;
        if let Some(rename) = new_name {
            Self::validate_pool_name(rename)?;
        }

        match pool_type {
            PoolType::Zfs => {
                let mut args: Vec<&str> = vec!["import"];
                if let Some(rename) = new_name {
                    args.push(rename);
                }
                args.push(name);
                let output = run_timed(
                    Command::new("sudo").arg("zpool").args(&args),
                    POOL_QUERY_TIMEOUT,
                )?;
                if !output.status.success() {
                    return Err(PoolError::CommandFailed(
                        String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    ));
                }
                Ok(())
            }
            PoolType::Btrfs => {
                // Locate the device carrying this label from `btrfs filesystem
                // show`, then mount it like `create_btrfs_advanced_pool` does.
                let importable = Self::list_importable_btrfs_pools();
                let pool = importable
                    .into_iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| PoolError::PoolNotFound(name.to_string()))?;
                let device = pool
                    .devices
                    .first()
                    .ok_or_else(|| PoolError::Parse(format!("no device found for '{}'", name)))?;
                let mount_point = format!("{}/{}", crate::config::paths::POOLS_MOUNT_BASE, name);

                let mkdir = run_timed(
                    Command::new("sudo").arg("mkdir").args(["-p", &mount_point]),
                    POOL_QUERY_TIMEOUT,
                )?;
                if !mkdir.status.success() {
                    return Err(PoolError::CommandFailed(format!(
                        "Failed to create mount point: {}",
                        String::from_utf8_lossy(&mkdir.stderr)
                    )));
                }
                let mount = run_timed(
                    Command::new("sudo")
                        .arg("mount")
                        .args([device.as_str(), &mount_point]),
                    POOL_WRITE_TIMEOUT,
                )?;
                if !mount.status.success() {
                    return Err(PoolError::CommandFailed(format!(
                        "Failed to mount pool: {}",
                        String::from_utf8_lossy(&mount.stderr)
                    )));
                }
                if let Ok(uuid_output) = run_timed(
                    Command::new("sudo")
                        .arg("blkid")
                        .args(["-s", "UUID", "-o", "value", device]),
                    POOL_QUERY_TIMEOUT,
                ) {
                    let uuid = String::from_utf8_lossy(&uuid_output.stdout)
                        .trim()
                        .to_string();
                    if !uuid.is_empty() {
                        let marker = format!("# homevault-pool:{}", name);
                        let line =
                            format!("UUID={} {} btrfs defaults,nofail 0 2", uuid, mount_point);
                        let _ = Self::append_pool_fstab_entry(&marker, &line);
                    }
                }
                Ok(())
            }
            PoolType::Mdadm => Err(PoolError::CommandFailed(
                "RAID arrays are managed by the RAID app, not Storage".to_string(),
            )),
            PoolType::Unknown => Err(PoolError::CommandFailed(
                "Unsupported pool type".to_string(),
            )),
        }
    }

    pub fn get_pool(&self, name: &str) -> Result<PoolInfo, PoolError> {
        let pools = Self::scan_pools()?;
        pools
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| PoolError::PoolNotFound(name.to_string()))
    }

    pub fn get_pool_devices(name: &str) -> Result<Vec<PoolDevice>, PoolError> {
        let pools = Self::scan_pools()?;

        if let Some(pool) = pools.iter().find(|p| p.name == name) {
            match pool.pool_type {
                PoolType::Zfs => Self::get_zfs_devices(name),
                PoolType::Mdadm => Self::get_mdadm_devices(name),
                PoolType::Btrfs => Self::get_btrfs_devices(name),
                _ => Ok(Vec::new()),
            }
        } else {
            Err(PoolError::PoolNotFound(name.to_string()))
        }
    }

    pub fn create_zfs_pool(name: &str, devices: &[&str]) -> Result<(), PoolError> {
        let output = run_timed(
            Command::new("sudo")
                .arg("zpool")
                .args(["create", name])
                .args(devices),
            POOL_WRITE_TIMEOUT,
        )?;

        if !output.status.success() {
            return Err(PoolError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    pub fn delete_pool(name: &str, pool_type: PoolType) -> Result<(), PoolError> {
        match pool_type {
            PoolType::Zfs => {
                let output = run_timed(
                    Command::new("sudo")
                        .arg("zpool")
                        .args(["destroy", "-f", name]),
                    POOL_WRITE_TIMEOUT,
                )?;

                if !output.status.success() {
                    return Err(PoolError::CommandFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
            }
            // Deliberately unsupported: dismantling a RAID array is the RAID
            // page's job (disk.rs::delete_raid), which also cleans up
            // /etc/mdadm/mdadm.conf and /etc/fstab entries that a bare
            // `mdadm --stop`/`--remove` here would leave behind, causing a
            // stale reassembly attempt on the next boot. This used to have a
            // real Mdadm branch reachable via the advanced-pool-delete API
            // even though the Storage UI never exposed a button for it.
            PoolType::Mdadm => {
                return Err(PoolError::CommandFailed(
                    "RAID arrays are managed by the RAID app, not Storage".to_string(),
                ))
            }
            PoolType::Btrfs => {
                let mount_point = Self::btrfs_mount_point(name);

                if let Some(ref mp) = mount_point {
                    let _ = run_timed(
                        Command::new("sudo").arg("umount").arg(mp),
                        POOL_WRITE_TIMEOUT,
                    );
                }

                let devices = Self::get_btrfs_devices(name)?;
                Self::remove_pool_fstab_entry(&format!("# homevault-pool:{}", name));

                for dev in &devices {
                    let _ = run_timed(
                        Command::new("sudo").arg("wipefs").args(["-a", &dev.device]),
                        POOL_WRITE_TIMEOUT,
                    );
                }

                if let Some(mp) = mount_point {
                    // sudo, not std::fs::remove_dir — same reasoning as the
                    // sudo mkdir in create_btrfs_advanced_pool above.
                    let _ = run_timed(
                        Command::new("sudo").arg("rmdir").arg(&mp),
                        POOL_WRITE_TIMEOUT,
                    );
                }
            }
            _ => {
                return Err(PoolError::CommandFailed(
                    "Unsupported pool type".to_string(),
                ))
            }
        }
        Ok(())
    }

    pub fn start_scrub(name: &str, pool_type: PoolType) -> Result<(), PoolError> {
        match pool_type {
            PoolType::Zfs => {
                let output = run_timed(
                    Command::new("sudo").arg("zpool").args(["scrub", name]),
                    POOL_QUERY_TIMEOUT,
                )?;

                if !output.status.success() {
                    return Err(PoolError::CommandFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
            }
            // RAID array maintenance (scrub/check) is the RAID app's job, not
            // Storage's — see delete_pool above for the same reasoning. This
            // previously wrote directly to /sys/block/*/md/sync_action, which
            // requires root and always failed with EACCES for the real
            // unprivileged homevault service account (confirmed live) — so
            // it never actually worked even before being removed here.
            PoolType::Mdadm => {
                return Err(PoolError::CommandFailed(
                    "RAID arrays are managed by the RAID app, not Storage".to_string(),
                ))
            }
            PoolType::Btrfs => {
                let mount_point = Self::btrfs_mount_point(name)
                    .ok_or_else(|| PoolError::PoolNotFound(format!("{} is not mounted", name)))?;

                let output = run_timed(
                    Command::new("sudo")
                        .arg("btrfs")
                        .args(["scrub", "start", &mount_point]),
                    POOL_QUERY_TIMEOUT,
                )?;

                if !output.status.success() {
                    return Err(PoolError::CommandFailed(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ));
                }
            }
            _ => {
                return Err(PoolError::CommandFailed(
                    "Unsupported pool type".to_string(),
                ))
            }
        }
        Ok(())
    }

    pub fn get_scrub_status(name: &str, pool_type: PoolType) -> Result<String, PoolError> {
        match pool_type {
            PoolType::Zfs => {
                let output = run_timed(
                    Command::new("sudo").arg("zpool").args(["status", name]),
                    POOL_QUERY_TIMEOUT,
                )?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("scan:") {
                        return Ok(line.trim().to_string());
                    }
                }
                Ok("No scrub in progress".to_string())
            }
            PoolType::Mdadm => {
                let sync_action_path = format!("/sys/block/{}/md/sync_action", name);
                if !std::path::Path::new(&sync_action_path).exists() {
                    warn!("MD sync_action file not found: {}", sync_action_path);
                    return Ok("Not available".to_string());
                }
                let sync_action = std::fs::read_to_string(sync_action_path)?;
                Ok(sync_action.trim().to_string())
            }
            PoolType::Btrfs => {
                let Some(mount_point) = Self::btrfs_mount_point(name) else {
                    return Ok("Not available".to_string());
                };

                let output = run_timed(
                    Command::new("sudo")
                        .arg("btrfs")
                        .args(["scrub", "status", &mount_point]),
                    POOL_QUERY_TIMEOUT,
                )?;

                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.trim_start().starts_with("Status:") {
                        return Ok(line.trim().to_string());
                    }
                }
                Ok(stdout.trim().to_string())
            }
            _ => Ok("Unsupported".to_string()),
        }
    }

    pub fn validate_disks_for_pool(disks: &str, pool_type: &str) -> Result<String, PoolError> {
        let disk_list: Vec<&str> = disks.split(',').collect();

        if disk_list.is_empty() {
            return Err(PoolError::Parse("No disks specified".to_string()));
        }

        let mut errors = Vec::new();
        let mut valid_disks = Vec::new();

        for disk in &disk_list {
            let disk_path = if disk.starts_with("/dev/") {
                disk.to_string()
            } else {
                format!("/dev/{}", disk)
            };

            if std::path::Path::new(&disk_path).exists() {
                valid_disks.push((*disk).to_string());
            } else {
                errors.push(format!("Disk {} not found", disk));
            }
        }

        if !errors.is_empty() {
            return Err(PoolError::Parse(errors.join(", ")));
        }

        match pool_type {
            "zfs" => {
                if valid_disks.is_empty() {
                    return Err(PoolError::Parse("ZFS requires at least 1 disk".to_string()));
                }
            }
            "btrfs" => {
                if valid_disks.is_empty() {
                    return Err(PoolError::Parse(
                        "Btrfs requires at least 1 disk".to_string(),
                    ));
                }
            }
            "mdadm" if valid_disks.len() < 2 => {
                return Err(PoolError::Parse(
                    "MDADM requires at least 2 disks".to_string(),
                ));
            }
            _ => {}
        }

        Ok("Valid".to_string())
    }

    fn scan_zfs_pools() -> Result<Vec<PoolInfo>, PoolError> {
        let mut pools = Vec::new();

        // `zpool` is an optional package (see installer/install.sh) — a
        // system without ZFS installed simply has no ZFS pools, it's not
        // an error condition for the rest of the pool list.
        let output = match run_timed(
            Command::new("sudo").arg("zpool").args([
                "list",
                "-H",
                "-o",
                "name,size,allocated,free,cap,health,altroot",
            ]),
            POOL_QUERY_TIMEOUT,
        ) {
            Ok(o) => o,
            Err(_) => return Ok(Vec::new()),
        };

        if !output.status.success() {
            return Ok(Vec::new());
        }

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 6 {
                pools.push(PoolInfo {
                    name: parts[0].to_string(),
                    pool_type: PoolType::Zfs,
                    size_bytes: Self::parse_size(parts[1]),
                    used_bytes: Self::parse_size(parts[2]),
                    available_bytes: Self::parse_size(parts[3]),
                    health: parts.get(5).unwrap_or(&"Unknown").to_string(),
                    device_count: Self::get_zfs_device_count(parts[0]),
                    mount_point: Self::get_zfs_mount_point(parts[0]),
                    raid_profile: Self::get_zfs_raid_profile(parts[0]),
                });
            }
        }

        Ok(pools)
    }

    fn scan_mdadm_pools() -> Result<Vec<PoolInfo>, PoolError> {
        let mut pools = Vec::new();

        // Same reasoning as `scan_zfs_pools`: `mdadm` missing just means no
        // mdadm arrays exist, not a failure of the whole pool list. `sudo` is
        // required here (and on every other mdadm call in this file) because
        // `mdadm --detail`/`--scan` need root even to read — confirmed live:
        // "must be super-user to perform this action" for the real
        // unprivileged homevault service account. Without it, this silently
        // returned an empty list (the command runs and exits non-zero, but
        // that's not a Rust-level `Err`, so nothing here ever surfaced the
        // failure) — meaning an existing RAID array could never be offered
        // as a cache-tier disk candidate even though that's still meant to
        // work (see the `handle_pools_list` doc comment in handlers/storage.rs
        // for why cache-tier is intentionally exempt from the "no RAID in
        // Storage" rule).
        let output = match run_timed(
            Command::new("sudo")
                .arg("mdadm")
                .args(["--detail", "--scan"]),
            POOL_QUERY_TIMEOUT,
        ) {
            Ok(o) => o,
            Err(_) => return Ok(Vec::new()),
        };

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if line.contains("ARRAY") {
                let mut name = String::new();
                let mut state = String::new();

                for part in line.split_whitespace() {
                    if part.starts_with("/dev/md") {
                        name = part.replace("UUID=", "").replace("]", "");
                    } else if part.starts_with("state=") {
                        state = part.replace("state=", "").replace(",", "");
                    }
                }

                if !name.is_empty() {
                    let devices = Self::get_mdadm_devices(&name)?;
                    let size = Self::get_mdadm_size(&name)?;
                    let mount_point = Self::get_mdadm_mount_point(&name);

                    // Real usage from `df` when the array is mounted; only
                    // fall back to a neutral 0/size split otherwise (the old
                    // hard-coded `size/2` reported a brand-new empty array as
                    // half-full).
                    let (used_bytes, available_bytes) = mount_point
                        .as_deref()
                        .and_then(Self::df_usage)
                        .unwrap_or((0, size));

                    pools.push(PoolInfo {
                        name: name.clone(),
                        pool_type: PoolType::Mdadm,
                        size_bytes: size,
                        used_bytes,
                        available_bytes,
                        health: state,
                        device_count: devices.len(),
                        mount_point,
                        raid_profile: Self::get_mdadm_raid_profile(&name),
                    });
                }
            }
        }

        Ok(pools)
    }

    fn get_zfs_devices(name: &str) -> Result<Vec<PoolDevice>, PoolError> {
        let output = run_timed(
            Command::new("sudo")
                .arg("zpool")
                .args(["status", name, "-P", "-L"]),
            POOL_QUERY_TIMEOUT,
        )?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();
        let mut in_devices = false;

        for line in stdout.lines() {
            if line.contains("config:") {
                in_devices = true;
                continue;
            }
            if in_devices && !line.trim().is_empty() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                // With `-P` every real leaf device is an absolute path; the
                // pool-name row, the `mirror-0`/`raidz1-0` vdev-container
                // rows and the trailing `errors:` line are not — they were
                // all being counted as devices (a 2-disk mirror reported 5).
                if parts.len() >= 2 && parts[0].starts_with("/dev/") {
                    devices.push(PoolDevice {
                        name: parts[0].to_string(),
                        device: parts[0].to_string(),
                        state: parts[1].to_string(),
                        size_bytes: parts.get(2).map(|s| Self::parse_size(s)).unwrap_or(0),
                    });
                }
            }
        }

        Ok(devices)
    }

    // `name` here (and in every other get_mdadm_* helper below) is always
    // the *full* device path already, e.g. "/dev/md/storagetest" — it comes
    // straight from `scan_mdadm_pools`'s `part.starts_with("/dev/md")`
    // extraction. These used to re-prepend "/dev/" on top of that
    // ("/dev//dev/md/storagetest"), which mdadm rejects outright
    // ("cannot open ...: No such file or directory") — confirmed live this
    // silently zeroed out every mdadm pool's reported size/devices/profile.
    fn get_mdadm_devices(name: &str) -> Result<Vec<PoolDevice>, PoolError> {
        let output = run_timed(
            Command::new("sudo").arg("mdadm").args(["--detail", name]),
            POOL_QUERY_TIMEOUT,
        )?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        // The member devices live in `mdadm --detail`'s trailing table:
        //   Number   Major   Minor   RaidDevice State
        //      0       7        0        0      active sync   /dev/loop0
        // — the device path is the LAST token, not the first (the first is
        // the "Number" column). The previous `parts.first()` check matched
        // nothing there and instead counted the `/dev/md/<name>:` header
        // line as a single phantom device, so every mdadm array reported
        // exactly one member.
        let mut in_table = false;
        for line in stdout.lines() {
            if line.contains("RaidDevice") && line.contains("State") {
                in_table = true;
                continue;
            }
            if !in_table {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.last() {
                Some(dev) if dev.starts_with("/dev/") => {
                    let state = parts
                        .iter()
                        .find(|t| {
                            ["active", "spare", "faulty", "rebuilding", "sync"]
                                .contains(&t.to_lowercase().as_str())
                        })
                        .copied()
                        .unwrap_or("active");
                    devices.push(PoolDevice {
                        name: dev.replace("/dev/", ""),
                        device: dev.to_string(),
                        state: state.to_string(),
                        size_bytes: 0,
                    });
                }
                _ => {}
            }
        }

        Ok(devices)
    }

    fn get_zfs_device_count(name: &str) -> usize {
        Self::get_zfs_devices(name).map(|d| d.len()).unwrap_or(0)
    }

    fn get_zfs_mount_point(name: &str) -> Option<String> {
        // `mountpoint` is a ZFS *dataset* property (`zfs get`), not a pool
        // property — `zpool get mountpoint` errors out with "invalid
        // property", which left every ZFS pool showing as unmounted.
        let output = run_timed(
            Command::new("sudo")
                .arg("zfs")
                .args(["get", "-H", "-o", "value", "mountpoint", name]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;

        let mount = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if mount == "none" || mount == "-" || mount == "legacy" || mount.is_empty() {
            None
        } else {
            Some(mount)
        }
    }

    fn get_mdadm_mount_point(name: &str) -> Option<String> {
        // `name` is the `/dev/md/<name>` symlink, but `/proc/mounts` records
        // the kernel device it points at (`/dev/md127`), so match on the
        // canonical path too — otherwise a mounted array always read as
        // unmounted.
        let canonical = std::fs::canonicalize(name)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let Some(dev) = parts.first() else { continue };
            let hit = *dev == name || canonical.as_deref() == Some(*dev) || dev.contains(name);
            if hit {
                return Some(parts.get(1).unwrap_or(&"").to_string());
            }
        }
        None
    }

    /// `(used_bytes, available_bytes)` for a mounted filesystem, from `df`.
    fn df_usage(mount_point: &str) -> Option<(u64, u64)> {
        let output = run_timed(
            Command::new("df").args(["-B1", "--output=used,avail", mount_point]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let last = text.lines().nth(1)?;
        let mut nums = last.split_whitespace();
        let used = nums.next()?.parse::<u64>().ok()?;
        let avail = nums.next()?.parse::<u64>().ok()?;
        Some((used, avail))
    }

    fn get_mdadm_size(name: &str) -> Result<u64, PoolError> {
        let output = run_timed(
            Command::new("sudo").arg("mdadm").args(["--detail", name]),
            POOL_QUERY_TIMEOUT,
        )?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("Array Size") {
                if let Some(s) = line.split(':').nth(1) {
                    // `mdadm --detail` prints "Array Size : 408576 (399.00
                    // MiB 417.87 MB)" — the leading number is in 1K blocks,
                    // not bytes, so it was under-reporting the array size by
                    // 1024× (a 400 GiB array showed as ~400 MiB).
                    let blocks = s
                        .split_whitespace()
                        .next()
                        .and_then(|t| t.parse::<u64>().ok())
                        .unwrap_or(0);
                    return Ok(blocks * 1024);
                }
            }
        }
        Ok(0)
    }

    /// Extracts the mdadm RAID level (e.g. "raid1") from `mdadm --detail`.
    fn get_mdadm_raid_profile(name: &str) -> Option<String> {
        let output = run_timed(
            Command::new("sudo").arg("mdadm").args(["--detail", name]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.contains("Raid Level") {
                if let Some(level) = line.split(':').nth(1) {
                    return Some(level.trim().to_string());
                }
            }
        }
        None
    }

    /// Extracts the ZFS pool's redundancy profile (mirror/raidzN/stripe)
    /// from `zpool status`'s `config:` section.
    fn get_zfs_raid_profile(name: &str) -> Option<String> {
        let output = run_timed(
            Command::new("sudo").arg("zpool").args(["status", name]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut in_config = false;
        let mut saw_plain_disk = false;
        for line in stdout.lines() {
            if line.contains("config:") {
                in_config = true;
                continue;
            }
            if !in_config {
                continue;
            }
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with("NAME") {
                continue;
            }
            let token = trimmed.split_whitespace().next().unwrap_or("");
            for prefix in ["mirror-", "raidz1-", "raidz2-", "raidz3-"] {
                if token.starts_with(prefix) {
                    return Some(prefix.trim_end_matches('-').to_string());
                }
            }
            if token == name {
                continue;
            }
            if !token.is_empty() {
                saw_plain_disk = true;
            }
        }

        if saw_plain_disk {
            Some("stripe".to_string())
        } else {
            None
        }
    }

    /// Scans Btrfs filesystems (each one is its own "pool" — Btrfs has no
    /// separate pool/filesystem distinction like ZFS).
    fn scan_btrfs_pools() -> Result<Vec<PoolInfo>, PoolError> {
        let output = match run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["filesystem", "show"]),
            POOL_QUERY_TIMEOUT,
        ) {
            Ok(o) => o,
            Err(_) => return Ok(Vec::new()),
        };

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut pools = Vec::new();
        let mut current_label: Option<String> = None;
        let mut current_devices: Vec<(String, u64)> = Vec::new();
        let mut missing = false;

        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("Label:") {
                if let Some(label) = current_label.take() {
                    pools.push(Self::build_btrfs_pool_info(
                        label,
                        &current_devices,
                        missing,
                    ));
                }
                current_devices.clear();
                missing = false;

                // `Label: 'name' uuid: ...` or `Label: none  uuid: ...`
                let after = trimmed.trim_start_matches("Label:").trim();
                let label = if after.starts_with('\'') {
                    after
                        .trim_start_matches('\'')
                        .split('\'')
                        .next()
                        .unwrap_or("")
                        .to_string()
                } else {
                    after
                        .split_whitespace()
                        .next()
                        .unwrap_or("unknown")
                        .to_string()
                };
                current_label = Some(label);
            } else if trimmed.starts_with("devid") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                // devid N size S used U path /dev/X
                if let Some(path_idx) = parts.iter().position(|p| *p == "path") {
                    if let Some(path) = parts.get(path_idx + 1) {
                        let size = parts
                            .iter()
                            .position(|p| *p == "size")
                            .and_then(|i| parts.get(i + 1))
                            .map(|s| Self::parse_size(s))
                            .unwrap_or(0);
                        current_devices.push((path.to_string(), size));
                    } else {
                        missing = true;
                    }
                } else {
                    missing = true;
                }
            }
        }

        if let Some(label) = current_label.take() {
            pools.push(Self::build_btrfs_pool_info(
                label,
                &current_devices,
                missing,
            ));
        }

        Ok(pools)
    }

    fn build_btrfs_pool_info(name: String, devices: &[(String, u64)], missing: bool) -> PoolInfo {
        let mount_point = Self::btrfs_mount_point(&name);
        let size_bytes: u64 = devices.iter().map(|(_, s)| s).sum();

        let (used_bytes, available_bytes) = if let Some(mp) = &mount_point {
            Self::get_btrfs_usage(mp).unwrap_or((0, size_bytes))
        } else {
            (0, size_bytes)
        };

        PoolInfo {
            name: name.clone(),
            pool_type: PoolType::Btrfs,
            size_bytes,
            used_bytes,
            available_bytes,
            health: if missing {
                "degraded".to_string()
            } else {
                "clean".to_string()
            },
            device_count: devices.len(),
            raid_profile: mount_point
                .as_deref()
                .and_then(Self::get_btrfs_raid_profile),
            mount_point,
        }
    }

    /// Reads the Btrfs "Data" redundancy profile (e.g. "RAID1", "single")
    /// for a mounted filesystem via `btrfs filesystem df`.
    fn get_btrfs_raid_profile(mount_point: &str) -> Option<String> {
        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["filesystem", "df", mount_point]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("Data, ") {
                let profile = rest.split(':').next()?.trim();
                return Some(profile.to_lowercase());
            }
        }
        None
    }

    /// Finds a Btrfs filesystem's mount point by matching any of its member
    /// devices against `/proc/mounts` (same idiom as `get_mdadm_mount_point`).
    pub(crate) fn btrfs_mount_point(name: &str) -> Option<String> {
        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["filesystem", "show", name]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut member_devices = Vec::new();
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("devid") {
                if let Some(path) = trimmed.split_whitespace().last() {
                    member_devices.push(path.to_string());
                }
            }
        }

        let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let Some(device) = parts.first() else {
                continue;
            };
            let Some(fstype) = parts.get(2) else { continue };
            if *fstype == "btrfs" && member_devices.iter().any(|d| d == device) {
                return parts.get(1).map(|s| s.to_string());
            }
        }
        None
    }

    /// Reads used/free bytes for a mounted Btrfs filesystem.
    fn get_btrfs_usage(mount_point: &str) -> Option<(u64, u64)> {
        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["filesystem", "usage", "-b", mount_point]),
            POOL_QUERY_TIMEOUT,
        )
        .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let mut used = None;
        let mut free = None;
        for line in stdout.lines() {
            let trimmed = line.trim();
            if let Some(v) = trimmed.strip_prefix("Used:") {
                used = v.split_whitespace().next().and_then(|s| s.parse().ok());
            } else if trimmed.starts_with("Free (estimated):") {
                free = trimmed
                    .trim_start_matches("Free (estimated):")
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok());
            }
        }

        match (used, free) {
            (Some(u), Some(f)) => Some((u, f)),
            _ => None,
        }
    }

    /// Lists `devid`/path/size entries for a named Btrfs filesystem.
    fn get_btrfs_devices(name: &str) -> Result<Vec<PoolDevice>, PoolError> {
        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["filesystem", "show", name]),
            POOL_QUERY_TIMEOUT,
        )?;

        if !output.status.success() {
            return Err(PoolError::PoolNotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut devices = Vec::new();

        for line in stdout.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("devid") {
                continue;
            }
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let has_path = parts.contains(&"path");
            let path = if has_path {
                parts.last().map(|s| s.to_string())
            } else {
                None
            };
            let size = parts
                .iter()
                .position(|p| *p == "size")
                .and_then(|i| parts.get(i + 1))
                .map(|s| Self::parse_size(s))
                .unwrap_or(0);
            let devid = parts
                .iter()
                .position(|p| *p == "devid")
                .and_then(|i| parts.get(i + 1))
                .unwrap_or(&"?");

            devices.push(PoolDevice {
                name: path.clone().unwrap_or_else(|| format!("devid-{}", devid)),
                device: path.unwrap_or_else(|| "missing".to_string()),
                state: if has_path {
                    "active".to_string()
                } else {
                    "missing".to_string()
                },
                size_bytes: size,
            });
        }

        Ok(devices)
    }

    /// Idempotently appends `entry` to `/etc/fstab`, tagged with `marker`,
    /// unless that exact marker is already present. Deliberately independent
    /// from `disk.rs`'s equivalent RAID helper (which is frozen and must not
    /// be edited or made callable from here) — this is the advanced-storage
    /// pools' own small copy of the same idempotent-marker idiom.
    /// Writes `content` to `path` via `sudo tee`, not `std::fs::write` —
    /// `/etc/fstab` is `644 root:root`, so a direct write from the
    /// unprivileged homevault process fails with EACCES. This was
    /// previously swallowed by a bare `let _ =` at both call sites below,
    /// so a pool/encrypted-volume mount was silently never persisted to
    /// fstab — confirmed live: it always worked in the moment (mounted
    /// immediately after creation) but would not have survived a reboot.
    fn write_file_as_root(path: &str, content: &str) -> std::io::Result<()> {
        // `run_timed_with_stdin`, not an unbounded wait on the child: `sudo tee`
        // can block on the sudo PAM stack or on a busy filesystem, and this is
        // the call that persists a pool's mount point — an unbounded wait here
        // is the one that silently loses the fstab entry on the next reboot.
        let output = run_timed_with_stdin(
            Command::new("sudo").args(["tee", path]),
            POOL_WRITE_TIMEOUT,
            content.as_bytes(),
        )
        .map_err(std::io::Error::other)?;
        if !output.status.success() {
            return Err(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    fn append_pool_fstab_entry(marker: &str, entry: &str) -> Result<(), PoolError> {
        let existing = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
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

        Self::write_file_as_root("/etc/fstab", &updated)?;
        Ok(())
    }

    /// Removes the `marker` line and the entry right after it from `/etc/fstab`.
    fn remove_pool_fstab_entry(marker: &str) {
        let Ok(existing) = std::fs::read_to_string("/etc/fstab") else {
            return;
        };

        let lines: Vec<&str> = existing.lines().collect();
        let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
        let mut i = 0;
        let mut changed = false;
        while i < lines.len() {
            if lines[i] == marker {
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
            let _ = Self::write_file_as_root("/etc/fstab", &updated);
        }
    }

    /// Validates a new pool `name` the same way `disk.rs`'s RAID creation
    /// does (kept as an independent copy since that code is frozen).
    fn validate_pool_name(name: &str) -> Result<(), PoolError> {
        if name.is_empty() || name.len() > 32 {
            return Err(PoolError::Parse(
                "Pool name must be 1-32 characters".to_string(),
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(PoolError::Parse(
                "Pool name can only contain alphanumeric, dash and underscore characters"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Minimum device count required for a given filesystem + redundancy profile.
    fn min_devices_for_profile(pool_type: &PoolType, profile: &str) -> usize {
        match (pool_type, profile) {
            (PoolType::Btrfs, "single") => 1,
            (PoolType::Btrfs, "raid0") => 2,
            (PoolType::Btrfs, "raid1") => 2,
            (PoolType::Btrfs, "raid10") => 4,
            (PoolType::Btrfs, "raid5") => 3,
            (PoolType::Btrfs, "raid6") => 4,
            (PoolType::Zfs, "stripe") => 1,
            (PoolType::Zfs, "mirror") => 2,
            (PoolType::Zfs, "raidz1") => 3,
            (PoolType::Zfs, "raidz2") => 4,
            (PoolType::Zfs, "raidz3") => 5,
            _ => usize::MAX, // unknown profile: reject
        }
    }

    /// Creates a new advanced-storage pool (ZFS or Btrfs) with the given
    /// redundancy profile. Deliberately shares its ZFS dataset policy with
    /// `disk.rs::create_zfs_pool_from_devices` (the RAID app's own ZFS path)
    /// so every ZFS volume the appliance creates behaves identically.
    pub fn create_pool(
        name: &str,
        pool_type: PoolType,
        raid_profile: &str,
        devices: &[String],
        mount_point: Option<&str>,
    ) -> Result<(), PoolError> {
        Self::validate_pool_name(name)?;

        if !matches!(pool_type, PoolType::Zfs | PoolType::Btrfs) {
            return Err(PoolError::CommandFailed(
                "Advanced pools only support ZFS or Btrfs".to_string(),
            ));
        }

        let min_devices = Self::min_devices_for_profile(&pool_type, raid_profile);
        if min_devices == usize::MAX {
            return Err(PoolError::Parse(format!(
                "Unknown redundancy profile '{}' for this filesystem",
                raid_profile
            )));
        }
        if devices.len() < min_devices {
            return Err(PoolError::Parse(format!(
                "Profile '{}' requires at least {} device(s)",
                raid_profile, min_devices
            )));
        }

        let pool_type_str = match pool_type {
            PoolType::Zfs => "zfs",
            PoolType::Btrfs => "btrfs",
            _ => unreachable!(),
        };
        Self::validate_disks_for_pool(&devices.join(","), pool_type_str)?;

        let device_paths: Vec<String> = devices
            .iter()
            .map(|d| {
                if d.starts_with("/dev/") {
                    d.clone()
                } else {
                    format!("/dev/{}", d)
                }
            })
            .collect();

        let target_mount = mount_point
            .map(|m| m.to_string())
            .unwrap_or_else(|| format!("{}/{}", crate::config::paths::POOLS_MOUNT_BASE, name));

        match pool_type {
            PoolType::Zfs => {
                Self::create_zfs_advanced_pool(name, raid_profile, &device_paths, &target_mount)
            }
            PoolType::Btrfs => {
                Self::create_btrfs_advanced_pool(name, raid_profile, &device_paths, &target_mount)
            }
            _ => unreachable!(),
        }
    }

    fn create_zfs_advanced_pool(
        name: &str,
        raid_profile: &str,
        devices: &[String],
        mount_point: &str,
    ) -> Result<(), PoolError> {
        // `zpool create [opts] <poolname> [vdev-type] <devices...>` — the
        // pool name must precede the vdev type, or zpool treats the name as
        // a device and fails ("cannot open '<name>': no such device").
        let mut args: Vec<&str> = vec!["create", "-f", "-m", mount_point];
        // One shared root-dataset policy, mirrored from
        // `disk.rs::create_zfs_pool_from_devices`: lz4 compression, no atime,
        // POSIX ACLs (the shares' setfacl flow relies on them) and xattr in
        // the dnode, on every ZFS volume the appliance creates.
        for kv in [
            "-O",
            "compression=lz4",
            "-O",
            "atime=off",
            "-O",
            "acltype=posixacl",
            "-O",
            "xattr=sa",
        ] {
            args.push(kv);
        }
        args.push(name);
        if raid_profile != "stripe" {
            args.push(raid_profile);
        }
        let device_refs: Vec<&str> = devices.iter().map(|s| s.as_str()).collect();
        args.extend(device_refs);

        let output = run_timed(
            Command::new("sudo").arg("zpool").args(&args),
            POOL_QUERY_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(PoolError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(())
    }

    fn create_btrfs_advanced_pool(
        name: &str,
        raid_profile: &str,
        devices: &[String],
        mount_point: &str,
    ) -> Result<(), PoolError> {
        // Btrfs raid5/raid6 have a known upstream write-hole bug for
        // *metadata* specifically; deliberately keep metadata on raid1
        // even when data uses raid5/raid6, rather than the (more common
        // but riskier) practice of mirroring the data profile for metadata too.
        let metadata_profile = match raid_profile {
            "raid5" | "raid6" => "raid1",
            other => other,
        };

        let mut args: Vec<&str> =
            vec!["-f", "-d", raid_profile, "-m", metadata_profile, "-L", name];
        let device_refs: Vec<&str> = devices.iter().map(|s| s.as_str()).collect();
        args.extend(device_refs);

        let output = run_timed(
            Command::new("sudo").arg("mkfs.btrfs").args(&args),
            POOL_WRITE_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(PoolError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        // `sudo mkdir`, not `std::fs::create_dir_all` — the latter runs as
        // the unprivileged homevault process directly and fails outright
        // under /mnt (root-owned, confirmed live), since sudo only wraps
        // external Command invocations, not Rust's own syscalls.
        let mkdir_output = run_timed(
            Command::new("sudo").arg("mkdir").args(["-p", mount_point]),
            POOL_QUERY_TIMEOUT,
        )?;
        if !mkdir_output.status.success() {
            return Err(PoolError::CommandFailed(format!(
                "Failed to create mount point: {}",
                String::from_utf8_lossy(&mkdir_output.stderr)
            )));
        }
        let mount_output = run_timed(
            Command::new("sudo")
                .arg("mount")
                .args([&devices[0], mount_point]),
            POOL_WRITE_TIMEOUT,
        )?;
        if !mount_output.status.success() {
            return Err(PoolError::CommandFailed(format!(
                "Pool created but failed to mount: {}",
                String::from_utf8_lossy(&mount_output.stderr)
            )));
        }

        let marker = format!("# homevault-pool:{}", name);
        if let Ok(uuid_output) = run_timed(
            Command::new("sudo")
                .arg("blkid")
                .args(["-s", "UUID", "-o", "value", &devices[0]]),
            POOL_QUERY_TIMEOUT,
        ) {
            let uuid = String::from_utf8_lossy(&uuid_output.stdout)
                .trim()
                .to_string();
            if !uuid.is_empty() {
                let fstab_line = format!("UUID={} {} btrfs defaults,nofail 0 2", uuid, mount_point);
                let _ = Self::append_pool_fstab_entry(&marker, &fstab_line);
            }
        }

        Ok(())
    }

    /// Parses a human-readable size like `"279G"` (`zpool list`/`mdadm`
    /// single-letter suffix) or `"300.00MiB"` (`btrfs filesystem show`'s
    /// binary-unit suffix) into raw bytes. The two-letter "iB" suffixes are
    /// checked first since e.g. `"MiB"` doesn't end with the bare `'M'` used
    /// by the other tools.
    fn parse_size(s: &str) -> u64 {
        let s = s.trim();
        let multiplier: u64 = if s.ends_with("TiB") || s.ends_with('T') {
            1024 * 1024 * 1024 * 1024
        } else if s.ends_with("GiB") || s.ends_with('G') {
            1024 * 1024 * 1024
        } else if s.ends_with("MiB") || s.ends_with('M') {
            1024 * 1024
        } else if s.ends_with("KiB") || s.ends_with('K') {
            1024
        } else {
            1
        };

        // `zpool list -H` (and `lsblk`, `mdadm --detail`) format the numeric
        // part following the process locale: "1.81T" in C, "1,81T" under a
        // locale that uses ',' as the decimal separator. Keep digits, '.'
        // and ',', then normalise ',' -> '.' before parsing — same fix as
        // `disk.rs::parse_size` (which had the identical two bugs).
        let num_str: String = s
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
            .collect::<String>()
            .replace(',', ".");
        // Multiply in f64 first, cast to u64 last. Casting the parsed value
        // to u64 *before* multiplying (as this did previously) dropped the
        // fractional part up front, so a 1.81 TiB pool was reported as
        // exactly 1 TiB (undercount of ~0.81 TiB) — same bug `disk.rs` was
        // fixed for.
        (num_str.parse::<f64>().unwrap_or(0.0) * multiplier as f64) as u64
    }
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for `PoolService`.
    //!
    //! Scope: the deterministic, side-effect-free helpers — pool-name and
    //! device-count validation, the redundancy-profile → min-device table,
    //! `zpool list` size parsing, and the **RAID/Storage boundary** (the
    //! frozen guarantee that `PoolType::Mdadm` is rejected by `delete_pool` /
    //! `start_scrub` / `create_pool` *before any command runs*, so Storage
    //! can never dismantle or scrub an mdadm array — that's the RAID app's
    //! job). Nothing here shells out to `zpool`/`mdadm`/`btrfs`; the
    //! invalid-input paths of `create_pool` are verified to return `Err`
    //! without reaching a `Command`.
    //!
    //! Run: `cargo test pools::tests`

    use super::*;

    // -- parse_size (zpool list -H -o size,allocated,free) -----------------

    #[test]
    fn test_parse_size_bare_integer() {
        assert_eq!(PoolService::parse_size("512"), 512);
    }

    #[test]
    fn test_parse_size_integer_units() {
        assert_eq!(PoolService::parse_size("1K"), 1024);
        assert_eq!(PoolService::parse_size("1M"), 1024 * 1024);
        assert_eq!(PoolService::parse_size("2G"), 2 * 1024 * 1024 * 1024);
        assert_eq!(PoolService::parse_size("1T"), 1024u64.pow(4));
    }

    #[test]
    fn test_parse_size_iec_suffixes() {
        assert_eq!(PoolService::parse_size("1KiB"), 1024);
        assert_eq!(PoolService::parse_size("1MiB"), 1024 * 1024);
        assert_eq!(PoolService::parse_size("1GiB"), 1024 * 1024 * 1024);
        assert_eq!(PoolService::parse_size("1TiB"), 1024u64.pow(4));
    }

    #[test]
    fn test_parse_size_empty_and_garbage_are_zero() {
        assert_eq!(PoolService::parse_size(""), 0);
        assert_eq!(PoolService::parse_size("   "), 0);
        assert_eq!(PoolService::parse_size("-"), 0);
        assert_eq!(PoolService::parse_size("N/A"), 0);
    }

    #[test]
    fn test_parse_size_keeps_the_fraction() {
        // `zpool list -H` emits fractional sizes ("1.81T", "9.06G"). The
        // fraction must survive: multiply in f64, cast to u64 last.
        // Regression test for the old `x as u64 * mult` form that reported a
        // 1.81 TiB pool as exactly 1 TiB (fixed 2026-09-06, mirroring the
        // identical earlier fix in `disk.rs::parse_size`).
        assert_eq!(
            PoolService::parse_size("1.81T"),
            (1.81_f64 * 1024u64.pow(4) as f64) as u64
        );
        assert_eq!(
            PoolService::parse_size("9.06G"),
            (9.06_f64 * 1024u64.pow(3) as f64) as u64
        );
        // sanity: that's ~1.81 TiB, comfortably above 1 TiB
        assert!(PoolService::parse_size("1.81T") > 1024u64.pow(4) + 800 * 1024u64.pow(3));
    }

    #[test]
    fn test_parse_size_accepts_a_locale_decimal_comma() {
        // Under a locale using ',' as the decimal separator, `zpool`/`lsblk`
        // print "1,81T" — it must parse the same as "1.81T", not as 181 TiB.
        assert_eq!(
            PoolService::parse_size("1,81T"),
            PoolService::parse_size("1.81T")
        );
        assert_eq!(
            PoolService::parse_size("465,8G"),
            (465.8_f64 * 1024u64.pow(3) as f64) as u64
        );
    }

    // -- PoolType serde ---------------------------------------------------

    #[test]
    fn test_pool_type_serialization() {
        assert_eq!(serde_json::to_string(&PoolType::Zfs).unwrap(), "\"Zfs\"");
        assert_eq!(
            serde_json::to_string(&PoolType::Mdadm).unwrap(),
            "\"Mdadm\""
        );
        assert_eq!(
            serde_json::to_string(&PoolType::Btrfs).unwrap(),
            "\"Btrfs\""
        );
    }

    #[test]
    fn test_pool_info_uses_renamed_keys() {
        let info = PoolInfo {
            name: "tank".into(),
            pool_type: PoolType::Zfs,
            size_bytes: 100,
            used_bytes: 40,
            available_bytes: 60,
            health: "ONLINE".into(),
            device_count: 2,
            mount_point: Some("/mnt/tank".into()),
            raid_profile: Some("mirror".into()),
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["pool_type"], "Zfs");
        assert_eq!(v["mount_point"], "/mnt/tank");
        assert_eq!(v["raid_profile"], "mirror");
    }

    // -- validate_pool_name --------------------------------------------

    #[test]
    fn test_validate_pool_name_accepts_reasonable_names() {
        assert!(PoolService::validate_pool_name("tank").is_ok());
        assert!(PoolService::validate_pool_name("data-01").is_ok());
        assert!(PoolService::validate_pool_name("my_pool_2").is_ok());
    }

    #[test]
    fn test_validate_pool_name_rejects_empty_and_overlong() {
        assert!(PoolService::validate_pool_name("").is_err());
        assert!(PoolService::validate_pool_name(&"x".repeat(33)).is_err());
        assert!(PoolService::validate_pool_name(&"x".repeat(32)).is_ok());
    }

    #[test]
    fn test_validate_pool_name_rejects_shell_and_path_metacharacters() {
        for bad in [
            "../etc",
            "tank; rm -rf /",
            "a b",
            "pool$(id)",
            "p/../q",
            "tank\n",
        ] {
            assert!(
                PoolService::validate_pool_name(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    // -- min_devices_for_profile -------------------------------------

    #[test]
    fn test_min_devices_for_profile_known_profiles() {
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Btrfs, "single"),
            1
        );
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Btrfs, "raid1"),
            2
        );
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Btrfs, "raid10"),
            4
        );
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Zfs, "stripe"),
            1
        );
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Zfs, "mirror"),
            2
        );
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Zfs, "raidz1"),
            3
        );
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Zfs, "raidz3"),
            5
        );
    }

    #[test]
    fn test_min_devices_for_profile_unknown_is_rejected() {
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Zfs, "raidz9"),
            usize::MAX
        );
        // profile valid for the *other* filesystem is still unknown here
        assert_eq!(
            PoolService::min_devices_for_profile(&PoolType::Zfs, "raid10"),
            usize::MAX
        );
    }

    // -- validate_disks_for_pool -----------------------------------

    #[test]
    fn test_validate_disks_for_pool_rejects_nonexistent_disk() {
        let err = PoolService::validate_disks_for_pool("hv-does-not-exist-x1", "zfs").unwrap_err();
        assert!(matches!(err, PoolError::Parse(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_validate_disks_for_pool_mdadm_requires_two_even_if_missing() {
        // Both names resolve to nonexistent devices, so the "not found" check
        // fires first — the point is it never touches a real disk.
        let err = PoolService::validate_disks_for_pool("hv-fake-a,hv-fake-b", "mdadm").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // -- RAID/Storage boundary (frozen guarantee) -----------------

    #[test]
    fn test_delete_pool_refuses_mdadm_without_running_a_command() {
        let err = PoolService::delete_pool("md0", PoolType::Mdadm).unwrap_err();
        assert!(
            err.to_string().contains("managed by the RAID app"),
            "got: {err}"
        );
    }

    #[test]
    fn test_delete_pool_refuses_unknown_type() {
        let err = PoolService::delete_pool("x", PoolType::Unknown).unwrap_err();
        assert!(err.to_string().contains("Unsupported pool type"));
    }

    #[test]
    fn test_start_scrub_refuses_mdadm_without_running_a_command() {
        let err = PoolService::start_scrub("md0", PoolType::Mdadm).unwrap_err();
        assert!(
            err.to_string().contains("managed by the RAID app"),
            "got: {err}"
        );
    }

    #[test]
    fn test_start_scrub_refuses_unknown_type() {
        let err = PoolService::start_scrub("x", PoolType::Unknown).unwrap_err();
        assert!(err.to_string().contains("Unsupported"));
    }

    // -- create_pool validation gates (no Command reached) --------

    #[test]
    fn test_create_pool_rejects_bad_name() {
        let err = PoolService::create_pool(
            "bad name!",
            PoolType::Zfs,
            "mirror",
            &["hv-a".into(), "hv-b".into()],
            None,
        )
        .unwrap_err();
        assert!(matches!(err, PoolError::Parse(_)));
    }

    #[test]
    fn test_create_pool_rejects_mdadm_and_unknown_types() {
        for ty in [PoolType::Mdadm, PoolType::Unknown] {
            let err =
                PoolService::create_pool("tank", ty, "mirror", &["hv-a".into()], None).unwrap_err();
            assert!(
                err.to_string().contains("only support ZFS or Btrfs"),
                "got: {err}"
            );
        }
    }

    #[test]
    fn test_create_pool_rejects_unknown_profile() {
        let err = PoolService::create_pool(
            "tank",
            PoolType::Zfs,
            "raidzX",
            &["hv-a".into(), "hv-b".into(), "hv-c".into()],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("profile"));
    }

    #[test]
    fn test_create_pool_rejects_too_few_devices() {
        let err = PoolService::create_pool("tank", PoolType::Zfs, "mirror", &["hv-a".into()], None)
            .unwrap_err();
        assert!(err.to_string().contains("at least 2"));
    }

    #[test]
    fn test_create_pool_with_enough_devices_but_bogus_paths_fails_before_any_command() {
        // name ok, type ok, profile ok, count ok → validate_disks_for_pool
        // rejects the nonexistent paths *before* create_zfs_advanced_pool.
        let err = PoolService::create_pool(
            "tank",
            PoolType::Zfs,
            "mirror",
            &["hv-fake-x1".into(), "hv-fake-x2".into()],
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    // -- parse_zpool_import (foreign-pool discovery) -----------------------

    #[test]
    fn test_parse_zpool_import_single_pool() {
        let out = "   pool: tank\n     id: 1234567890\n  state: ONLINE\n action: The pool can be imported using its name or numeric identifier.\n config:\n\n\ttank        ONLINE\n\t  /dev/sdb    ONLINE\n";
        let pools = PoolService::parse_zpool_import(out);
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].name, "tank");
        assert_eq!(pools[0].pool_type, PoolType::Zfs);
        assert_eq!(pools[0].id.as_deref(), Some("1234567890"));
        assert_eq!(pools[0].state, "ONLINE");
        // Only the leaf device, not the pool-name row.
        assert_eq!(pools[0].devices, vec!["/dev/sdb"]);
    }

    #[test]
    fn test_parse_zpool_import_multiple_pools() {
        let out = "   pool: tank\n     id: 1\n  state: ONLINE\n config:\n\n\ttank  ONLINE\n\t  /dev/sdb  ONLINE\n\n   pool: backup\n     id: 2\n  state: DEGRADED\n config:\n\n\tbackup  DEGRADED\n\t  /dev/sdc  UNAVAIL\n";
        let pools = PoolService::parse_zpool_import(out);
        assert_eq!(pools.len(), 2);
        assert_eq!(pools[0].name, "tank");
        assert_eq!(pools[1].name, "backup");
        assert_eq!(pools[1].state, "DEGRADED");
        assert_eq!(pools[1].devices, vec!["/dev/sdc"]);
    }

    #[test]
    fn test_parse_zpool_import_empty_output() {
        assert!(PoolService::parse_zpool_import("").is_empty());
    }

    #[test]
    fn test_parse_zpool_import_ignores_vdev_container_rows() {
        // A mirror reports `mirror-0` and the pool name in the config block;
        // neither is a leaf device.
        let out = "   pool: tank\n     id: 9\n  state: ONLINE\n config:\n\n\ttank        ONLINE\n\t  mirror-0  ONLINE\n\t    /dev/sdb  ONLINE\n\t    /dev/sdc  ONLINE\n";
        let pools = PoolService::parse_zpool_import(out);
        assert_eq!(pools[0].devices, vec!["/dev/sdb", "/dev/sdc"]);
    }

    // -- parse_btrfs_show (foreign-pool discovery) -------------------------

    #[test]
    fn test_parse_btrfs_show_returns_unmounted_only() {
        let out = "Label: 'mypool'  uuid: 1111-2222\n\tTotal devices 2 FS bytes used 1.00GiB\n\tdevid    1 size 10.00GiB used 1.00GiB path /dev/sdb\n\tdevid    2 size 10.00GiB used 1.00GiB path /dev/sdc\n";
        let mounted = std::collections::HashSet::new();
        let pools = PoolService::parse_btrfs_show(out, &mounted);
        assert_eq!(pools.len(), 1);
        assert_eq!(pools[0].name, "mypool");
        assert_eq!(pools[0].pool_type, PoolType::Btrfs);
        assert_eq!(pools[0].devices, vec!["/dev/sdb", "/dev/sdc"]);
    }

    #[test]
    fn test_parse_btrfs_show_skips_mounted_uuid() {
        let out = "Label: 'mypool'  uuid: 1111-2222\n\tdevid    1 size 10.00GiB used 1.00GiB path /dev/sdb\n";
        let mut mounted = std::collections::HashSet::new();
        mounted.insert("1111-2222".to_string());
        assert!(PoolService::parse_btrfs_show(out, &mounted).is_empty());
    }

    #[test]
    fn test_parse_btrfs_show_multiple_labels() {
        let out = "Label: 'one'  uuid: aaaa\n\tdevid    1 size 1.00GiB used 0 path /dev/sdb\nLabel: 'two'  uuid: bbbb\n\tdevid    1 size 1.00GiB used 0 path /dev/sdc\n";
        let mounted = std::collections::HashSet::new();
        let pools = PoolService::parse_btrfs_show(out, &mounted);
        assert_eq!(pools.len(), 2);
        assert_eq!(pools[0].name, "one");
        assert_eq!(pools[1].name, "two");
    }

    #[test]
    fn test_import_pool_rejects_mdadm() {
        // Storage must never import a RAID array — that's the RAID app's job.
        let err = PoolService::import_pool("md0", PoolType::Mdadm, None).unwrap_err();
        assert!(err.to_string().contains("RAID"));
    }

    #[test]
    fn test_import_pool_rejects_invalid_name_before_any_command() {
        let err = PoolService::import_pool("bad name!", PoolType::Zfs, None).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("name"));
    }

    // -- ROADMAP X.2: no subprocess of this module runs unbounded ------------

    /// The defect class this guards against is invisible to a behavioural
    /// test: a bare `.output()` still answers correctly on an idle box and
    /// only pins the calling `spawn_blocking` thread when a pool or a disk
    /// stops answering. So the invariant is asserted at the source.
    ///
    /// `concat!` keeps each pattern out of this file's own source, so the
    /// assertion cannot count itself.
    #[test]
    fn test_no_subprocess_of_this_module_runs_without_a_deadline() {
        let src = include_str!("pools.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for unbounded in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!(".", "status()"),
            concat!("wait_with_", "output()"),
        ] {
            assert!(
                !production.contains(unbounded),
                "`{unbounded}` waits without a deadline — use `run_timed` (ROADMAP X.2)"
            );
        }
    }

    #[test]
    fn test_deadlines_are_bounded_and_ordered() {
        assert!(POOL_QUERY_TIMEOUT >= std::time::Duration::from_secs(5));
        assert!(POOL_WRITE_TIMEOUT >= POOL_QUERY_TIMEOUT);
        assert!(POOL_WRITE_TIMEOUT <= std::time::Duration::from_secs(60 * 60));
    }
}
