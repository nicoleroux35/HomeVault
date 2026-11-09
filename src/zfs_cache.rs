/* =============================================================================
 * File          : zfs_cache.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   ZFS read cache (L2ARC) and separate log (SLOG) devices: list what a pool
 *   currently has, validate a device before it is attached, and attach or
 *   detach it with `zpool add cache|log` / `zpool remove`.
 *
 *   This is not the "cache tier" of `src/cache.rs` (a whole pool used as a
 *   write buffer and drained by a mover). It is the ZFS-native, per-pool
 *   accelerator: opt-in, one device at a time, refused on a spinning disk
 *   (a cache vdev on an HDD makes the pool slower, not faster) and refused
 *   for a disk already in use.
 *
 *   The status parser is pure and unit-tested: only `probe`, `add` and
 *   `remove` touch the host, through bounded `sudo zpool` calls.
 *
 * Dependencies:
 *   - serde (>= 1.0), crate::utils::process, crate::disk::AvailableDevice
 *
 * Usage:
 *   handlers::zfs_cache exposes /api/zfs-cache; see docs/guides/cache.md.
 * =============================================================================
 */

//! ZFS L2ARC/SLOG attachment (see docs/guides/cache.md).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Deadline for a read-only `zpool status`.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(20);

/// Deadline for `zpool add`/`zpool remove`. Attaching a cache vdev starts a
/// resilver-free metadata write; a command that has not answered in two
/// minutes is wedged on the device.
pub const MODIFY_TIMEOUT: Duration = Duration::from_secs(120);

/// Which ZFS accelerator a device provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZfsCacheKind {
    /// Read cache (`zpool add <pool> cache <dev>`).
    L2Arc,
    /// Separate log (`zpool add <pool> log <dev>`).
    Slog,
}

impl ZfsCacheKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L2Arc => "l2arc",
            Self::Slog => "slog",
        }
    }

    /// The `zpool` vdev keyword.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::L2Arc => "cache",
            Self::Slog => "log",
        }
    }

    /// The section header `zpool status` prints.
    pub fn section(self) -> &'static str {
        match self {
            Self::L2Arc => "cache",
            Self::Slog => "logs",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "l2arc" | "cache" | "read" => Some(Self::L2Arc),
            "slog" | "log" | "logs" | "write" => Some(Self::Slog),
            _ => None,
        }
    }
}

/// One attached cache or log device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZfsCacheDevice {
    /// Device path as `zpool status -P` reports it (`/dev/sdc`).
    pub device: String,
    pub kind: ZfsCacheKind,
    /// `ONLINE`, `FAULTED`, `OFFLINE`, `REMOVED`, …
    pub state: String,
    pub read_errors: u64,
    pub write_errors: u64,
    pub checksum_errors: u64,
}

/// A pool and its accelerators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZfsPoolCache {
    pub pool: String,
    pub state: String,
    pub devices: Vec<ZfsCacheDevice>,
}

/// Parse `zpool status -P <pool>` into the pool name, its state and the
/// devices attached as `logs`/`cache`.
///
/// Deliberately line-based, like the rest of the codebase: the output is a
/// stable, documented table, and a pure parser is the only way to unit-test
/// the section bookkeeping (a `logs` line before `cache`, a vdev group such as
/// `mirror-0` that is *not* a leaf, a device with error counters).
pub fn parse_zpool_status(output: &str) -> Option<ZfsPoolCache> {
    let mut pool: Option<String> = None;
    let mut state = String::new();
    let mut devices: Vec<ZfsCacheDevice> = Vec::new();
    let mut section: Option<ZfsCacheKind> = None;
    let mut in_config = false;

    for raw in output.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Header fields above the config table.
        if let Some(rest) = trimmed.strip_prefix("pool:") {
            pool = Some(rest.trim().to_string());
            continue;
        }
        if !in_config {
            if trimmed == "config:" {
                in_config = true;
            } else if let Some(rest) = trimmed.strip_prefix("state:") {
                state = rest.trim().to_string();
            }
            continue;
        }
        if trimmed.starts_with("errors:") {
            in_config = false;
            section = None;
            continue;
        }

        let columns: Vec<&str> = trimmed.split_whitespace().collect();
        // Section headers: a single token, no state column.
        if columns.len() == 1 {
            section = match columns[0] {
                "cache" => Some(ZfsCacheKind::L2Arc),
                "logs" => Some(ZfsCacheKind::Slog),
                _ => None,
            };
            continue;
        }
        let Some(kind) = section else {
            continue;
        };
        // A device row under `logs`/`cache`: NAME STATE [READ WRITE CKSUM].
        // `zpool status -P` prints absolute paths, so anything else on that
        // row is a vdev container (`mirror-1`, `raidz1-0`, `spare-0`).
        let name = columns[0];
        if !name.starts_with('/') {
            continue;
        }
        let read_errors = columns.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
        let write_errors = columns.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
        let checksum_errors = columns.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
        devices.push(ZfsCacheDevice {
            device: name.to_string(),
            kind,
            state: columns[1].to_string(),
            read_errors,
            write_errors,
            checksum_errors,
        });
    }

    pool.map(|pool| ZfsPoolCache {
        pool,
        state,
        devices,
    })
}

/// A device may be attached as a cache/log vdev: an absolute `/dev/…` path
/// with no traversal, whitespace or control character (it becomes one argv
/// entry of `zpool add`).
pub fn validate_device(raw: &str) -> Result<String, String> {
    let device = raw.trim();
    if device.is_empty() {
        return Err("A device is required".to_string());
    }
    if !device.starts_with("/dev/") || device.len() < 6 {
        return Err("Choose a device below /dev".to_string());
    }
    if device
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == ':')
        || device.contains("..")
    {
        return Err("Invalid device path".to_string());
    }
    Ok(device.to_string())
}

/// Why a device cannot be attached, as a sentence the UI can show verbatim.
///
/// The rules are decisions, not guesses:
/// 1. the pool must already exist and be ZFS (the caller resolves that);
/// 2. the device must be a real block device that is not mounted, part of an
///    array or already a cache device of another pool;
/// 3. a pool already has at most one `logs` group and one `cache` group — a
///    second one is refused rather than silently stacked;
/// 4. **a spinning disk is refused**: an L2ARC on an HDD is slower than no
///    L2ARC (random reads on the cache device), and a SLOG on an HDD is
///    pointless. DSM lets the admin pick any disk; HomeVault states the rule.
pub fn check_device_for_pool(
    kind: ZfsCacheKind,
    device: &str,
    device_in_use: bool,
    device_reason: Option<&str>,
    device_exists: bool,
    rotational: bool,
    existing: &[ZfsCacheDevice],
) -> Result<(), String> {
    if !device_exists {
        return Err(format!("{device} does not exist"));
    }
    if device_in_use {
        let reason = device_reason.unwrap_or("it is already part of a pool or mounted");
        return Err(format!("{device} is in use: {reason}"));
    }
    if existing.iter().any(|d| d.device == device) {
        return Err(format!("{device} is already a cache device of this pool"));
    }
    if existing.iter().any(|d| d.kind == kind) {
        return Err(match kind {
            ZfsCacheKind::L2Arc => {
                "This pool already has a read cache. Remove it before adding another.".to_string()
            }
            ZfsCacheKind::Slog => {
                "This pool already has a separate log. Remove it before adding another.".to_string()
            }
        });
    }
    if rotational {
        return Err(match kind {
            ZfsCacheKind::L2Arc => {
                "A read cache (L2ARC) must be a flash device: on a spinning disk it makes the pool slower."
                    .to_string()
            }
            ZfsCacheKind::Slog => {
                "A separate log (SLOG) must be a flash device: on a spinning disk it gains nothing."
                    .to_string()
            }
        });
    }
    Ok(())
}

/// The `sudo` argv that attaches `device` to `pool`.
pub fn add_argv(pool: &str, kind: ZfsCacheKind, device: &str) -> Vec<String> {
    vec![
        "zpool".to_string(),
        "add".to_string(),
        "-f".to_string(),
        pool.to_string(),
        kind.keyword().to_string(),
        device.to_string(),
    ]
}

/// The `sudo` argv that detaches a cache/log device. `zpool remove` accepts a
/// cache or log vdev by name; a regular data vdev is refused by ZFS itself.
pub fn remove_argv(pool: &str, device: &str) -> Vec<String> {
    vec![
        "zpool".to_string(),
        "remove".to_string(),
        pool.to_string(),
        device.to_string(),
    ]
}

/// Run one `sudo` argv under a deadline; a non-zero exit becomes the tool's
/// stderr so the caller can act on the real reason.
fn run_sudo(args: &[String], deadline: Duration) -> Result<String, String> {
    let Some((program, rest)) = args.split_first() else {
        return Err("empty command".to_string());
    };
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(program).args(rest);
    let output = run_timed(&mut cmd, deadline).map_err(|e| {
        format!(
            "{program} did not answer within {}s: {e}",
            deadline.as_secs()
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{program} failed with {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether `zpool` is installed.
pub fn zfs_available() -> bool {
    run_timed(
        std::process::Command::new("which").arg("zpool"),
        Duration::from_secs(5),
    )
    .map(|out| out.status.success())
    .unwrap_or(false)
}

/// Read one pool's cache/log devices. Blocking (one process).
pub fn status(pool: &str) -> Result<ZfsPoolCache, String> {
    crate::pools::PoolService::validate_pool_name(pool).map_err(|e| e.to_string())?;
    let output = run_sudo(
        &[
            "zpool".to_string(),
            "status".to_string(),
            "-P".to_string(),
            pool.to_string(),
        ],
        STATUS_TIMEOUT,
    )?;
    parse_zpool_status(&output)
        .ok_or_else(|| format!("zpool status did not report a pool named '{pool}'"))
}

/// Every ZFS pool with its cache/log devices. Blocking (one process per pool).
pub fn status_all() -> Result<Vec<ZfsPoolCache>, String> {
    let pools = crate::pools::PoolService::scan_pools().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for pool in pools
        .into_iter()
        .filter(|pool| pool.pool_type == crate::pools::PoolType::Zfs)
    {
        out.push(status(&pool.name)?);
    }
    Ok(out)
}

/// Attach a device. The caller has already validated it with
/// [`check_device_for_pool`]; this only runs the command.
pub fn add(pool: &str, kind: ZfsCacheKind, device: &str) -> Result<(), String> {
    crate::pools::PoolService::validate_pool_name(pool).map_err(|e| e.to_string())?;
    let device = validate_device(device)?;
    run_sudo(&add_argv(pool, kind, &device), MODIFY_TIMEOUT).map(|_| ())
}

/// Detach a cache/log device.
pub fn remove(pool: &str, device: &str) -> Result<(), String> {
    crate::pools::PoolService::validate_pool_name(pool).map_err(|e| e.to_string())?;
    let device = validate_device(device)?;
    run_sudo(&remove_argv(pool, &device), MODIFY_TIMEOUT).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &str = "  pool: tank
 state: ONLINE
config:

        NAME        STATE     READ WRITE CKSUM
        tank        ONLINE       0     0     0
          mirror-0  ONLINE       0     0     0
            /dev/sda  ONLINE       0     0     0
            /dev/sdb  ONLINE       0     0     0

errors: No known data errors
";

    const WITH_DEVICES: &str = "  pool: tank
 state: ONLINE
config:

        NAME           STATE     READ WRITE CKSUM
        tank           ONLINE       0     0     0
          /dev/sda     ONLINE       0     0     0
        logs
          /dev/nvme1n1 ONLINE       0     0     0
        cache
          /dev/nvme0n1 ONLINE       1     2     3
          /dev/nvme2n1 DEGRADED      0     0     0

errors: No known data errors
";

    #[test]
    fn test_parse_reports_pool_state_and_no_devices() {
        let parsed = parse_zpool_status(PLAIN).expect("a pool");
        assert_eq!(parsed.pool, "tank");
        assert_eq!(parsed.state, "ONLINE");
        assert!(
            parsed.devices.is_empty(),
            "a plain pool has no cache or log device"
        );
    }

    #[test]
    fn test_parse_separates_logs_cache_and_ignores_data_vdevs() {
        let parsed = parse_zpool_status(WITH_DEVICES).expect("a pool");
        assert_eq!(parsed.devices.len(), 3);
        assert_eq!(parsed.devices[0].device, "/dev/nvme1n1");
        assert_eq!(parsed.devices[0].kind, ZfsCacheKind::Slog);
        assert_eq!(parsed.devices[1].device, "/dev/nvme0n1");
        assert_eq!(parsed.devices[1].kind, ZfsCacheKind::L2Arc);
        assert_eq!(parsed.devices[1].read_errors, 1);
        assert_eq!(parsed.devices[1].write_errors, 2);
        assert_eq!(parsed.devices[1].checksum_errors, 3);
        assert_eq!(parsed.devices[2].state, "DEGRADED");
        // The data vdev and the pool row are not cache devices.
        assert!(!parsed.devices.iter().any(|d| d.device == "/dev/sda"));
        assert!(!parsed.devices.iter().any(|d| d.device == "tank"));
    }

    #[test]
    fn test_parse_handles_a_mirrored_log_group() {
        // A mirrored SLOG prints the group then two leaves; only the leaves
        // are devices the admin can remove.
        let output = "  pool: tank
 state: ONLINE
config:

        NAME          STATE     READ WRITE CKSUM
        tank          ONLINE       0     0     0
          /dev/sda    ONLINE       0     0     0
        logs
          mirror-1    ONLINE       0     0     0
            /dev/nvme1n1 ONLINE     0     0     0
            /dev/nvme2n1 ONLINE     0     0     0

errors: No known data errors
";
        let parsed = parse_zpool_status(output).expect("a pool");
        assert_eq!(parsed.devices.len(), 2);
        assert!(parsed.devices.iter().all(|d| d.kind == ZfsCacheKind::Slog));
    }

    #[test]
    fn test_parse_returns_none_without_a_pool_header() {
        assert!(parse_zpool_status("").is_none());
        assert!(parse_zpool_status("cannot open 'tank': no such pool").is_none());
    }

    #[test]
    fn test_device_path_shape() {
        assert_eq!(validate_device(" /dev/sdc ").unwrap(), "/dev/sdc");
        assert_eq!(
            validate_device("/dev/disk/by-id/nvme-eui.0").unwrap(),
            "/dev/disk/by-id/nvme-eui.0"
        );
        for bad in [
            "",
            "sdc",
            "/dev/",
            "/etc/passwd",
            "/dev/../etc/passwd",
            "/dev/sd c",
        ] {
            assert!(validate_device(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn test_check_device_rules() {
        let existing = vec![ZfsCacheDevice {
            device: "/dev/nvme0n1".to_string(),
            kind: ZfsCacheKind::L2Arc,
            state: "ONLINE".to_string(),
            read_errors: 0,
            write_errors: 0,
            checksum_errors: 0,
        }];
        // A free flash device is accepted for either role.
        assert!(check_device_for_pool(
            ZfsCacheKind::Slog,
            "/dev/nvme1n1",
            false,
            None,
            true,
            false,
            &[]
        )
        .is_ok());

        // A spinning disk never is, for either role.
        for kind in [ZfsCacheKind::L2Arc, ZfsCacheKind::Slog] {
            let err =
                check_device_for_pool(kind, "/dev/sdc", false, None, true, true, &[]).unwrap_err();
            assert!(err.contains("flash"), "{err}");
        }

        // In use, missing, already attached, and a second group.
        assert!(check_device_for_pool(
            ZfsCacheKind::Slog,
            "/dev/sdc",
            true,
            Some("part of pool tank"),
            true,
            false,
            &[]
        )
        .unwrap_err()
        .contains("part of pool tank"));
        assert!(check_device_for_pool(
            ZfsCacheKind::Slog,
            "/dev/sdc",
            false,
            None,
            false,
            false,
            &[]
        )
        .is_err());
        assert!(check_device_for_pool(
            ZfsCacheKind::Slog,
            "/dev/nvme0n1",
            false,
            None,
            true,
            false,
            &existing
        )
        .is_err());
        assert!(check_device_for_pool(
            ZfsCacheKind::L2Arc,
            "/dev/nvme1n1",
            false,
            None,
            true,
            false,
            &existing
        )
        .unwrap_err()
        .contains("already has a read cache"));
    }

    #[test]
    fn test_argv_is_one_argument_per_parameter() {
        assert_eq!(
            add_argv("tank", ZfsCacheKind::L2Arc, "/dev/nvme0n1"),
            vec!["zpool", "add", "-f", "tank", "cache", "/dev/nvme0n1"]
        );
        assert_eq!(
            add_argv("tank", ZfsCacheKind::Slog, "/dev/nvme1n1"),
            vec!["zpool", "add", "-f", "tank", "log", "/dev/nvme1n1"]
        );
        assert_eq!(
            remove_argv("tank", "/dev/nvme0n1"),
            vec!["zpool", "remove", "tank", "/dev/nvme0n1"]
        );
    }

    #[test]
    fn test_kind_parsing_and_keywords() {
        assert_eq!(ZfsCacheKind::parse("l2arc"), Some(ZfsCacheKind::L2Arc));
        assert_eq!(ZfsCacheKind::parse("CACHE"), Some(ZfsCacheKind::L2Arc));
        assert_eq!(ZfsCacheKind::parse("slog"), Some(ZfsCacheKind::Slog));
        assert_eq!(ZfsCacheKind::parse("log"), Some(ZfsCacheKind::Slog));
        assert_eq!(ZfsCacheKind::parse("nope"), None);
        assert_eq!(ZfsCacheKind::L2Arc.keyword(), "cache");
        assert_eq!(ZfsCacheKind::Slog.keyword(), "log");
        assert_eq!(ZfsCacheKind::Slog.section(), "logs");
    }
}
