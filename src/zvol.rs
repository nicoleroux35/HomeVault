/* =============================================================================
 * File          : zvol.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Thin ZFS volumes for iSCSI LUNs: create a zvol (sparse, snapshottable),
 *   list the pool's volumes, snapshot one, clone a snapshot into a new LUN,
 *   and destroy a volume.
 *
 *   A zvol is the honest way to export a LUN on a NAS that already runs ZFS:
 *   the blocks are thin (allocated on write), the LUN can be snapshotted at
 *   any instant, and a clone gives a writable copy without duplicating data.
 *   The file-backed backstore (`fileio`) stays for pools that are not ZFS.
 *
 *   Every rule that decides *whether* something may run is pure and
 *   unit-tested (names, sizes, dataset/device paths, argv, list parsing); only
 *   `create`/`snapshot`/`clone`/`destroy`/`list` touch the host, through
 *   bounded `sudo zfs` calls.
 *
 * Dependencies:
 *   - serde (>= 1.0), crate::utils::process
 *
 * Usage:
 *   handlers::iscsi creates and snapshots zvols; see docs/guides/iscsi.md.
 * =============================================================================
 */

//! Thin ZFS volumes for iSCSI LUNs (see docs/guides/iscsi.md).

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Parent dataset every HomeVault zvol lives under, inside its pool. One
/// folder keeps "what HomeVault owns" obvious in `zfs list` and makes the
/// listing bounded to our own volumes.
pub const ZVOL_PARENT: &str = "homevault-iscsi";

/// Where udev exposes a zvol.
pub const ZVOL_DEVICE_PREFIX: &str = "/dev/zvol/";

/// Smallest accepted volume. Below this a LUN is a toy; the UI starts at GiB.
pub const MIN_SIZE_BYTES: u64 = 1024 * 1024 * 1024;

/// Largest accepted volume (64 TiB), matching the file-backed backstore cap.
pub const MAX_SIZE_BYTES: u64 = 64 * 1024 * 1024 * 1024 * 1024;

/// Longest accepted zvol name.
pub const MAX_NAME_LEN: usize = 48;

/// Deadline for `zfs create`/`clone`/`snapshot`: metadata plus a `volsize`
/// reservation; a command that has not answered in two minutes is wedged.
pub const MODIFY_TIMEOUT: Duration = Duration::from_secs(120);

/// Deadline for `zfs destroy`, which may free many blocks.
pub const DESTROY_TIMEOUT: Duration = Duration::from_secs(300);

/// Deadline for a read-only `zfs list`.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// How many volumes one listing returns.
pub const MAX_LISTED: usize = 500;

/// One thin volume, as `zfs list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZvolInfo {
    /// Full dataset (`tank/homevault-iscsi/disk1`).
    pub dataset: String,
    /// Declared size (`volsize`).
    pub volsize_bytes: u64,
    /// Bytes actually allocated: less than `volsize` means it is thin.
    pub used_bytes: u64,
    pub referenced_bytes: u64,
}

impl ZvolInfo {
    /// The device a LUN exports.
    pub fn device(&self) -> String {
        device_path(&self.dataset)
    }

    /// `volsize - used > 0`: the volume is thin (sparse), not fully allocated.
    pub fn thin(&self) -> bool {
        self.used_bytes < self.volsize_bytes
    }
}

/// One snapshot of a volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZvolSnapshot {
    /// Full name (`tank/homevault-iscsi/disk1@before-upgrade`).
    pub name: String,
    /// The part after `@`.
    pub short_name: String,
    pub created: String,
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

/// A zvol name is a dataset component: lowercase letters, digits and dashes.
/// Reusing the iSCSI backstore name keeps the two views aligned (`disk1`).
pub fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("A volume name is required".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!("The name is longer than {MAX_NAME_LEN} characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(
            "Use lowercase letters, digits and dashes only (for example 'disk1')".to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("The name must not start or end with a dash".to_string());
    }
    Ok(name.to_string())
}

/// A snapshot name is a `@`-suffixed dataset component (ZFS's own rules).
pub fn validate_snapshot_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("A snapshot name is required".to_string());
    }
    if name.len() > 128 {
        return Err("The snapshot name is longer than 128 characters".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || name == "."
        || name == ".."
    {
        return Err("Use letters, digits, '.', '_' or '-' in the snapshot name".to_string());
    }
    Ok(name.to_string())
}

/// A volume must be at least 1 GiB and at most 64 TiB.
pub fn validate_size(bytes: u64) -> Result<u64, String> {
    if !(MIN_SIZE_BYTES..=MAX_SIZE_BYTES).contains(&bytes) {
        return Err(format!(
            "A volume must be between {} GiB and {} TiB",
            MIN_SIZE_BYTES / (1024 * 1024 * 1024),
            MAX_SIZE_BYTES / (1024 * 1024 * 1024 * 1024)
        ));
    }
    Ok(bytes)
}

/// The dataset of one HomeVault zvol: `<pool>/homevault-iscsi/<name>`.
///
/// An empty name yields the parent dataset itself (the listing root), which is
/// why the name is optional here — callers that create always pass a name.
pub fn dataset(pool: &str, name: &str) -> Result<String, String> {
    crate::pools::PoolService::validate_pool_name(pool).map_err(|e| e.to_string())?;
    if name.trim().is_empty() {
        return Ok(format!("{}/{ZVOL_PARENT}", pool.trim()));
    }
    let name = validate_name(name)?;
    Ok(format!("{}/{ZVOL_PARENT}/{name}", pool.trim()))
}

/// The pool a dataset belongs to (its first component).
pub fn pool_of_dataset(value: &str) -> Option<String> {
    let pool = value.trim().split('/').next()?;
    if pool.is_empty() {
        None
    } else {
        Some(pool.to_string())
    }
}

/// The device path of a dataset (`/dev/zvol/tank/homevault-iscsi/disk1`).
pub fn device_path(value: &str) -> String {
    format!(
        "{ZVOL_DEVICE_PREFIX}{}",
        value.trim().trim_start_matches('/')
    )
}

/// The dataset behind a device path, when it is one of ours.
///
/// Strict on purpose: this is how the iSCSI code recognises "this backstore is
/// a zvol HomeVault created", so a path that merely looks similar must not
/// match.
pub fn dataset_from_device(path: &str) -> Option<String> {
    let rest = path.trim().strip_prefix(ZVOL_DEVICE_PREFIX)?;
    let dataset = rest.trim_matches('/');
    if dataset.is_empty() || dataset.split('/').count() < 2 {
        return None;
    }
    if dataset.contains("..") || dataset.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    Some(dataset.to_string())
}

/// Whether `dataset` is a HomeVault-managed iSCSI volume (not an arbitrary
/// dataset an admin happens to point a block backstore at).
pub fn is_managed_dataset(dataset: &str) -> bool {
    let parts: Vec<&str> = dataset.trim().split('/').collect();
    parts.len() == 3 && parts[1] == ZVOL_PARENT
}

/// `zfs create -p [-s] -V <bytes> <dataset>`.
///
/// `-s` is ZFS's sparse flag: the volume is thin from the first byte. Without
/// it a zvol reserves its whole `volsize`, which is exactly what a home NAS
/// cannot afford.
pub fn create_argv(dataset: &str, size_bytes: u64, sparse: bool) -> Vec<String> {
    let mut argv = vec!["zfs".to_string(), "create".to_string(), "-p".to_string()];
    if sparse {
        argv.push("-s".to_string());
    }
    argv.push("-V".to_string());
    argv.push(size_bytes.to_string());
    argv.push(dataset.to_string());
    argv
}

/// `zfs list -H -p -o name,volsize,used,referenced -t volume -r <root>`.
pub fn list_argv(root: &str) -> Vec<String> {
    vec![
        "zfs".to_string(),
        "list".to_string(),
        "-H".to_string(),
        "-p".to_string(),
        "-o".to_string(),
        "name,volsize,used,referenced".to_string(),
        "-t".to_string(),
        "volume".to_string(),
        "-r".to_string(),
        root.to_string(),
    ]
}

/// `zfs list -H -p -o name,creation -t snapshot -r <dataset>` (-d 1 would
/// also list a clone's children, which is not this volume's business).
pub fn snapshot_list_argv(dataset: &str) -> Vec<String> {
    vec![
        "zfs".to_string(),
        "list".to_string(),
        "-H".to_string(),
        "-p".to_string(),
        "-o".to_string(),
        "name,creation".to_string(),
        "-t".to_string(),
        "snapshot".to_string(),
        "-d".to_string(),
        "1".to_string(),
        "-r".to_string(),
        dataset.to_string(),
    ]
}

/// `zfs snapshot <dataset>@<snap>`.
pub fn snapshot_argv(dataset: &str, snapshot: &str) -> Result<Vec<String>, String> {
    let snapshot = validate_snapshot_name(snapshot)?;
    Ok(vec![
        "zfs".to_string(),
        "snapshot".to_string(),
        format!("{}@{snapshot}", dataset.trim()),
    ])
}

/// `zfs clone <dataset>@<snap> <target>`.
pub fn clone_argv(dataset: &str, snapshot: &str, target: &str) -> Result<Vec<String>, String> {
    let snapshot = validate_snapshot_name(snapshot)?;
    Ok(vec![
        "zfs".to_string(),
        "clone".to_string(),
        format!("{}@{snapshot}", dataset.trim()),
        target.trim().to_string(),
    ])
}

/// `zfs destroy [-r] <dataset>`.
///
/// `-r` is required when the volume has snapshots; the caller asks for it
/// explicitly (the delete endpoint requires the name to be typed).
pub fn destroy_argv(dataset: &str, recursive: bool) -> Vec<String> {
    let mut argv = vec!["zfs".to_string(), "destroy".to_string()];
    if recursive {
        argv.push("-r".to_string());
    }
    argv.push(dataset.trim().to_string());
    argv
}

/// `zfs destroy <dataset>@<snap>`.
pub fn destroy_snapshot_argv(dataset: &str, snapshot: &str) -> Result<Vec<String>, String> {
    let snapshot = validate_snapshot_name(snapshot)?;
    Ok(vec![
        "zfs".to_string(),
        "destroy".to_string(),
        format!("{}@{snapshot}", dataset.trim()),
    ])
}

// ---------------------------------------------------------------------------
// Output parsing (pure)
// ---------------------------------------------------------------------------

/// Parse `zfs list -H -p -o name,volsize,used,referenced` output.
pub fn parse_volume_list(output: &str) -> Vec<ZvolInfo> {
    let mut out = Vec::new();
    for line in output.lines() {
        let columns: Vec<&str> = line.split('\t').collect();
        if columns.len() < 4 {
            continue;
        }
        let Ok(volsize) = columns[1].trim().parse::<u64>() else {
            continue;
        };
        let used = columns[2].trim().parse::<u64>().unwrap_or(0);
        let referenced = columns[3].trim().parse::<u64>().unwrap_or(0);
        out.push(ZvolInfo {
            dataset: columns[0].trim().to_string(),
            volsize_bytes: volsize,
            used_bytes: used,
            referenced_bytes: referenced,
        });
        if out.len() >= MAX_LISTED {
            break;
        }
    }
    out
}

/// Parse `zfs list -H -p -o name,creation` output.
pub fn parse_snapshot_list(output: &str) -> Vec<ZvolSnapshot> {
    let mut out = Vec::new();
    for line in output.lines() {
        let columns: Vec<&str> = line.split('\t').collect();
        if columns.len() < 2 {
            continue;
        }
        let name = columns[0].trim();
        let Some((_, short)) = name.rsplit_once('@') else {
            continue;
        };
        out.push(ZvolSnapshot {
            name: name.to_string(),
            short_name: short.to_string(),
            created: columns[1].trim().to_string(),
        });
        if out.len() >= MAX_LISTED {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Host operations (blocking)
// ---------------------------------------------------------------------------

/// Whether `zfs` is installed.
pub fn available() -> bool {
    run_timed(
        std::process::Command::new("which").arg("zfs"),
        Duration::from_secs(5),
    )
    .map(|out| out.status.success())
    .unwrap_or(false)
}

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
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Create a thin volume. The device appears under `/dev/zvol/<dataset>`.
pub fn create(pool: &str, name: &str, size_bytes: u64, sparse: bool) -> Result<String, String> {
    let dataset = dataset(pool, name)?;
    validate_size(size_bytes)?;
    run_sudo(&create_argv(&dataset, size_bytes, sparse), MODIFY_TIMEOUT)?;
    Ok(dataset)
}

/// Every HomeVault iSCSI volume of a pool.
pub fn list(pool: &str) -> Result<Vec<ZvolInfo>, String> {
    let root = dataset(pool, "")?;
    let output = run_sudo(&list_argv(&root), QUERY_TIMEOUT)?;
    Ok(parse_volume_list(&output).into_iter().collect())
}

/// Snapshot one volume.
pub fn snapshot(pool: &str, name: &str, snapshot_name: &str) -> Result<String, String> {
    let dataset = dataset(pool, name)?;
    let argv = snapshot_argv(&dataset, snapshot_name)?;
    run_sudo(&argv, MODIFY_TIMEOUT)?;
    Ok(format!("{}@{snapshot_name}", dataset))
}

/// Snapshots of one volume, newest first as `zfs list` prints them.
pub fn snapshots(pool: &str, name: &str) -> Result<Vec<ZvolSnapshot>, String> {
    let dataset = dataset(pool, name)?;
    let output = run_sudo(&snapshot_list_argv(&dataset), QUERY_TIMEOUT)?;
    Ok(parse_snapshot_list(&output))
}

/// Clone a snapshot into a new volume in the same pool.
pub fn clone(
    pool: &str,
    source: &str,
    snapshot_name: &str,
    target_name: &str,
    sparse: bool,
) -> Result<String, String> {
    let source_dataset = dataset(pool, source)?;
    let target_dataset = dataset(pool, target_name)?;
    let argv = clone_argv(&source_dataset, snapshot_name, &target_dataset)?;
    run_sudo(&argv, MODIFY_TIMEOUT)?;
    // A clone inherits the snapshot's blocks; nothing to reserve. `-s` is not
    // a valid flag for `clone`, so the sparse choice only affects `create`.
    let _ = sparse;
    Ok(target_dataset)
}

/// Destroy a volume, recursively when it has snapshots.
pub fn destroy(pool: &str, name: &str, recursive: bool) -> Result<(), String> {
    let dataset = dataset(pool, name)?;
    run_sudo(&destroy_argv(&dataset, recursive), DESTROY_TIMEOUT).map(|_| ())
}

/// Destroy one snapshot of a volume.
pub fn destroy_snapshot(pool: &str, name: &str, snapshot_name: &str) -> Result<(), String> {
    let dataset = dataset(pool, name)?;
    let argv = destroy_snapshot_argv(&dataset, snapshot_name)?;
    run_sudo(&argv, DESTROY_TIMEOUT).map(|_| ())
}

/// Whether the zvol device node exists (the iSCSI export needs it).
pub fn device_exists(dataset: &str) -> bool {
    Path::new(&device_path(dataset)).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_names_are_dataset_components() {
        assert_eq!(validate_name(" disk1 ").unwrap(), "disk1");
        for bad in ["", "Disk1", "a/b", "a b", "-x", "x-", "@snap", "é"] {
            assert!(validate_name(bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(
            validate_snapshot_name("before-upgrade_1.0").unwrap(),
            "before-upgrade_1.0"
        );
        for bad in ["", ".", "..", "a/b", "a b", "a@b"] {
            assert!(
                validate_snapshot_name(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn test_dataset_and_device_round_trip() {
        assert_eq!(
            dataset("tank", "disk1").unwrap(),
            "tank/homevault-iscsi/disk1"
        );
        assert_eq!(dataset("tank", "").unwrap(), "tank/homevault-iscsi");
        assert!(dataset("bad pool", "disk1").is_err());
        assert_eq!(
            device_path("tank/homevault-iscsi/disk1"),
            "/dev/zvol/tank/homevault-iscsi/disk1"
        );
        assert_eq!(
            dataset_from_device("/dev/zvol/tank/homevault-iscsi/disk1").as_deref(),
            Some("tank/homevault-iscsi/disk1")
        );
        // Not our prefix, too short, or a traversal attempt.
        for bad in [
            "/dev/sdb",
            "/dev/zvol/tank",
            "/dev/zvol/../etc/passwd",
            "/dev/zvol/tank/../etc",
        ] {
            assert_eq!(dataset_from_device(bad), None, "{bad:?}");
        }
        assert_eq!(
            pool_of_dataset("tank/homevault-iscsi/disk1").as_deref(),
            Some("tank")
        );
        assert_eq!(pool_of_dataset(""), None);
        assert!(is_managed_dataset("tank/homevault-iscsi/disk1"));
        assert!(!is_managed_dataset("tank/data/disk1"));
        assert!(!is_managed_dataset("tank/disk1"));
    }

    #[test]
    fn test_size_bounds() {
        assert_eq!(
            validate_size(10 * 1024 * 1024 * 1024).unwrap(),
            10 * 1024 * 1024 * 1024
        );
        assert!(validate_size(0).is_err());
        assert!(validate_size(512 * 1024 * 1024).is_err());
        assert!(validate_size(MAX_SIZE_BYTES + 1).is_err());
    }

    #[test]
    fn test_argv_shapes() {
        assert_eq!(
            create_argv("tank/homevault-iscsi/disk1", 10 * 1024 * 1024 * 1024, true),
            vec![
                "zfs",
                "create",
                "-p",
                "-s",
                "-V",
                "10737418240",
                "tank/homevault-iscsi/disk1"
            ]
        );
        // Thick: no `-s`, so the whole volsize is reserved.
        assert_eq!(
            create_argv("tank/homevault-iscsi/disk1", 1024, false),
            vec![
                "zfs",
                "create",
                "-p",
                "-V",
                "1024",
                "tank/homevault-iscsi/disk1"
            ]
        );
        assert_eq!(
            snapshot_argv("tank/homevault-iscsi/disk1", "before").unwrap(),
            vec!["zfs", "snapshot", "tank/homevault-iscsi/disk1@before"]
        );
        assert_eq!(
            clone_argv(
                "tank/homevault-iscsi/disk1",
                "before",
                "tank/homevault-iscsi/copy"
            )
            .unwrap(),
            vec![
                "zfs",
                "clone",
                "tank/homevault-iscsi/disk1@before",
                "tank/homevault-iscsi/copy"
            ]
        );
        assert_eq!(
            destroy_argv("tank/homevault-iscsi/disk1", true),
            vec!["zfs", "destroy", "-r", "tank/homevault-iscsi/disk1"]
        );
        assert_eq!(
            destroy_snapshot_argv("tank/homevault-iscsi/disk1", "before").unwrap(),
            vec!["zfs", "destroy", "tank/homevault-iscsi/disk1@before"]
        );
        // A snapshot name is validated even when it comes from a URL.
        assert!(snapshot_argv("tank/homevault-iscsi/disk1", "../x").is_err());
    }

    #[test]
    fn test_parse_volume_list() {
        let output = "tank/homevault-iscsi/disk1\t10737418240\t1048576\t1048576\n\
                      tank/homevault-iscsi/disk2\t21474836480\t21474836480\t21474836480\n";
        let volumes = parse_volume_list(output);
        assert_eq!(volumes.len(), 2);
        assert_eq!(volumes[0].dataset, "tank/homevault-iscsi/disk1");
        assert!(volumes[0].thin(), "1 MiB used of 10 GiB is thin");
        assert!(!volumes[1].thin(), "fully allocated is not thin");
        assert_eq!(volumes[0].device(), "/dev/zvol/tank/homevault-iscsi/disk1");
        // A malformed row is skipped, not fatal.
        assert!(parse_volume_list("garbage\n").is_empty());
    }

    #[test]
    fn test_parse_snapshot_list() {
        let output = "tank/homevault-iscsi/disk1@before\t1762700000\n\
                      tank/homevault-iscsi/disk1@after\t1762800000\n";
        let snapshots = parse_snapshot_list(output);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].short_name, "before");
        assert_eq!(snapshots[1].name, "tank/homevault-iscsi/disk1@after");
        assert!(parse_snapshot_list("no-at-sign\t1\n").is_empty());
    }
}
