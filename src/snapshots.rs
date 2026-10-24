/* =============================================================================
 * File          : snapshots.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   ZFS/Btrfs snapshot management for storage pools and datasets: create,
 *   list, rollback and destroy snapshots through the zfs/btrfs CLI
 *   (sudo-scoped), feeding the Storage and Snapshots apps. The backend is
 *   chosen per **pool** (`SnapshotService::kind_for`), never from which
 *   binary happens to be installed.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2)
 *
 * Usage:
 *   handlers/storage.rs / handlers/explorer.rs snapshot endpoints and the
 *   scheduled-snapshot engine (see docs/guides/snapshots.md).
 * =============================================================================
 */

//! ZFS/Btrfs snapshot management (create/list/rollback/destroy) for pools.

use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;
use thiserror::Error;
use tracing::info;

use crate::utils::process::run_timed;

/// Deadline for a metadata read (`zfs list`, `btrfs subvolume list`, `zfs get`).
/// Cheap on a healthy pool, and a pool whose disks have spun down still answers
/// well inside it.
pub const SNAPSHOT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Deadline for a mutating snapshot call (`zfs snapshot|destroy|rollback -R`,
/// `btrfs subvolume snapshot|delete`, `sudo mkdir -p`). A recursive rollback
/// on a large dataset is the slowest honest case; `kill_on_drop` bounds the
/// rest. ROADMAP X.2: none of these had a deadline, so a wedged `zfs` held the
/// calling `spawn_blocking` thread — and the request or scheduler tick behind
/// it — for the life of the process.
pub const SNAPSHOT_WRITE_TIMEOUT: Duration = Duration::from_secs(600);

/// Errors that can occur during snapshot operations.
#[derive(Error, Debug)]
pub enum SnapError {
    #[error("Snapshot command failed: {0}")]
    CommandFailed(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Pool not found: {0}")]
    PoolNotFound(String),
    #[error("Snapshot not found: {0}")]
    NotFound(String),
    #[error("Permission denied")]
    PermissionDenied,
    #[error("{0}")]
    Unsupported(String),
}

/// The on-disk snapshot technology of a **pool** (never of the host).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    Zfs,
    Btrfs,
}

/// A single filesystem snapshot (ZFS or Btrfs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub pool: String,
    pub created: String,
    pub used: u64,
    pub referenced: u64,
    pub properties: std::collections::HashMap<String, String>,
}

/// A dataset and its associated snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDataset {
    pub dataset: String,
    pub snapshots: Vec<Snapshot>,
}

/// Parameters for creating a new snapshot. The pool name comes from the URL
/// path (`handle_snapshot_create`'s `Path(pool)`), not this body — a `pool`
/// field here used to be required by Serde but never actually read, so the
/// real frontend (which correctly never sends a redundant `pool` in the
/// body) always failed with "missing field `pool`" before this was removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSnapshotRequest {
    pub name: String,
    pub dataset: Option<String>,
    pub recursive: bool,
}

/// The pool snapshot operations one scheduled run needs: create, list and
/// delete. Production code goes through [`PoolBackend`]; tests inject a fake
/// so a failing prune is reachable without a real pool.
pub trait SnapshotBackend: Send + Sync {
    fn create(&self, name: &str, recursive: bool) -> Result<(), SnapError>;
    fn list(&self) -> Result<Vec<Snapshot>, SnapError>;
    fn delete(&self, name: &str) -> Result<(), SnapError>;
}

/// The real backend: one pool, resolved to its own technology **once** (a
/// single `PoolService::scan_pools`), then reused by every call — a prune
/// deletes several snapshots and must not rescan the host each time.
pub struct PoolBackend {
    pool: String,
    kind: SnapshotKind,
}

impl PoolBackend {
    pub fn new(pool: &str) -> Result<Self, SnapError> {
        Ok(Self {
            pool: pool.to_string(),
            kind: SnapshotService::kind_for(pool)?,
        })
    }
}

impl SnapshotBackend for PoolBackend {
    fn create(&self, name: &str, recursive: bool) -> Result<(), SnapError> {
        SnapshotService::create_snapshot_of(self.kind, &self.pool, name, None, recursive)
    }

    fn list(&self) -> Result<Vec<Snapshot>, SnapError> {
        SnapshotService::list_snapshots_of(self.kind, &self.pool)
    }

    fn delete(&self, name: &str) -> Result<(), SnapError> {
        SnapshotService::delete_snapshot_of(self.kind, &self.pool, name)
    }
}

/// Service for creating, listing, deleting, and rolling back ZFS/Btrfs snapshots.
pub struct SnapshotService;

impl SnapshotService {
    /// The backend this **pool** needs, from its own technology. Never from
    /// which binary is installed: `zfsutils-linux` and `btrfs-progs` are
    /// usually both present (`installer/install.sh`), so "the `zfs` binary
    /// exists" says nothing about this pool — that test sent every Btrfs
    /// pool down the ZFS branch and made create/delete/rollback fail.
    pub fn kind_for(pool: &str) -> Result<SnapshotKind, SnapError> {
        let pools = crate::pools::PoolService::scan_pools()
            .map_err(|e| SnapError::CommandFailed(format!("Failed to scan pools: {e}")))?;
        // A ZFS dataset path (`tank/data`) names its pool by the first
        // component, so walk up until a known pool matches.
        let mut candidate = pool;
        loop {
            if let Some(info) = pools.iter().find(|p| p.name == candidate) {
                return Self::kind_of_pool(info);
            }
            match candidate.rsplit_once('/') {
                Some((parent, _)) if !parent.is_empty() => candidate = parent,
                _ => return Err(SnapError::PoolNotFound(pool.to_string())),
            }
        }
    }

    fn kind_of_pool(info: &crate::pools::PoolInfo) -> Result<SnapshotKind, SnapError> {
        match info.pool_type {
            crate::pools::PoolType::Zfs => Ok(SnapshotKind::Zfs),
            crate::pools::PoolType::Btrfs => Ok(SnapshotKind::Btrfs),
            // An mdadm array (or an unidentified pool) says nothing about
            // the filesystem on top of it: only a Btrfs-formatted array can
            // be snapshotted, and a ZFS one is reported by `zpool` anyway.
            crate::pools::PoolType::Mdadm | crate::pools::PoolType::Unknown => {
                match info.mount_point.as_deref().and_then(mount_fstype).as_deref() {
                    Some("btrfs") => Ok(SnapshotKind::Btrfs),
                    Some("zfs") => Ok(SnapshotKind::Zfs),
                    _ => Err(SnapError::Unsupported(format!(
                        "Pool '{}' is not formatted with ZFS or Btrfs — it has no snapshots to manage",
                        info.name
                    ))),
                }
            }
        }
    }

    pub fn list_snapshots(pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        Self::list_snapshots_of(Self::kind_for(pool)?, pool)
    }

    /// Lists snapshots with an already-resolved backend (see [`PoolBackend`]).
    pub fn list_snapshots_of(kind: SnapshotKind, pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        match kind {
            SnapshotKind::Zfs => Self::list_zfs_snapshots(pool),
            SnapshotKind::Btrfs => Self::list_btrfs_snapshots(pool),
        }
    }

    fn list_zfs_snapshots(pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        let output = run_timed(
            Command::new("sudo").arg("zfs").args([
                "list",
                "-t",
                "snapshot",
                "-o",
                "name,used,referenced,creation",
                "-r",
                pool,
            ]),
            SNAPSHOT_QUERY_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("dataset does not exist") {
                return Err(SnapError::PoolNotFound(pool.to_string()));
            }
            return Err(SnapError::CommandFailed(stderr.to_string()));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_zfs_snapshots(pool, &output)
    }

    /// Btrfs has no "pool name" of its own — a pool is just a mounted
    /// filesystem, and every `btrfs` subcommand needs a real path, not the
    /// label. Resolves the pool name (as reported by `PoolService::scan_pools`)
    /// to its actual mount point.
    fn resolve_btrfs_mount(pool: &str) -> Result<String, SnapError> {
        crate::pools::PoolService::btrfs_mount_point(pool)
            .ok_or_else(|| SnapError::PoolNotFound(format!("{} is not mounted", pool)))
    }

    fn list_btrfs_snapshots(pool: &str) -> Result<Vec<Snapshot>, SnapError> {
        let mount_point = Self::resolve_btrfs_mount(pool)?;

        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["subvolume", "list", "-s", &mount_point]),
            SNAPSHOT_QUERY_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_btrfs_snapshots(pool, &output)
    }

    pub fn create_snapshot(
        pool: &str,
        name: &str,
        dataset: Option<&str>,
        recursive: bool,
    ) -> Result<(), SnapError> {
        Self::create_snapshot_of(Self::kind_for(pool)?, pool, name, dataset, recursive)
    }

    /// Creates a snapshot with an already-resolved backend.
    pub fn create_snapshot_of(
        kind: SnapshotKind,
        pool: &str,
        name: &str,
        dataset: Option<&str>,
        recursive: bool,
    ) -> Result<(), SnapError> {
        match kind {
            SnapshotKind::Zfs => Self::create_zfs_snapshot(pool, name, dataset, recursive),
            SnapshotKind::Btrfs => Self::create_btrfs_snapshot(pool, name, dataset),
        }
    }

    fn create_zfs_snapshot(
        pool: &str,
        name: &str,
        dataset: Option<&str>,
        recursive: bool,
    ) -> Result<(), SnapError> {
        let dataset_path = dataset.unwrap_or(pool);
        let snapshot_name = format!("{}@{}", dataset_path, name);

        let mut args = vec!["snapshot".to_string()];
        if recursive {
            args.push("-r".to_string());
        }
        args.push(snapshot_name.clone());

        let output = run_timed(
            Command::new("sudo").arg("zfs").args(&args),
            SNAPSHOT_WRITE_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Created ZFS snapshot: {}", snapshot_name);
        Ok(())
    }

    fn create_btrfs_snapshot(
        pool: &str,
        name: &str,
        dataset: Option<&str>,
    ) -> Result<(), SnapError> {
        let mount_point = Self::resolve_btrfs_mount(pool)?;
        let source = dataset.unwrap_or(&mount_point);
        let snapshots_dir = format!("{}/.snapshots", mount_point);
        // sudo mkdir, not std::fs::create_dir_all — the pool's mount point
        // root is owned by root (it was formatted and mounted as root, via
        // sudo, in create_btrfs_advanced_pool), so the unprivileged
        // homevault process can't create a directory under it directly.
        let mkdir_output = run_timed(
            Command::new("sudo")
                .arg("mkdir")
                .args(["-p", &snapshots_dir]),
            SNAPSHOT_WRITE_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;
        if !mkdir_output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&mkdir_output.stderr).to_string(),
            ));
        }
        let snapshot_path = format!("{}/{}", snapshots_dir, name);

        let mut args = vec!["subvolume".to_string(), "snapshot".to_string()];
        args.push(source.to_string());
        args.push(snapshot_path.clone());

        let output = run_timed(
            Command::new("sudo").arg("btrfs").args(&args),
            SNAPSHOT_WRITE_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Created Btrfs snapshot: {}", snapshot_path);
        Ok(())
    }

    pub fn delete_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        Self::delete_snapshot_of(Self::kind_for(pool)?, pool, name)
    }

    /// Deletes a snapshot with an already-resolved backend.
    pub fn delete_snapshot_of(kind: SnapshotKind, pool: &str, name: &str) -> Result<(), SnapError> {
        match kind {
            SnapshotKind::Zfs => Self::delete_zfs_snapshot(pool, name),
            SnapshotKind::Btrfs => Self::delete_btrfs_snapshot(pool, name),
        }
    }

    fn delete_zfs_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        let snapshot_name = if name.contains('@') {
            name.to_string()
        } else {
            format!("{}@{}", pool, name)
        };

        let output = run_timed(
            Command::new("sudo")
                .arg("zfs")
                .args(["destroy", &snapshot_name]),
            SNAPSHOT_WRITE_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Deleted ZFS snapshot: {}", snapshot_name);
        Ok(())
    }

    fn delete_btrfs_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        let mount_point = Self::resolve_btrfs_mount(pool)?;
        let path = format!("{}/.snapshots/{}", mount_point, name);

        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["subvolume", "delete", &path]),
            SNAPSHOT_WRITE_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Deleted Btrfs snapshot: {}", path);
        Ok(())
    }

    pub fn rollback_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        match Self::kind_for(pool)? {
            SnapshotKind::Zfs => Self::rollback_zfs_snapshot(pool, name),
            SnapshotKind::Btrfs => Self::rollback_btrfs_snapshot(pool, name),
        }
    }

    fn rollback_zfs_snapshot(pool: &str, name: &str) -> Result<(), SnapError> {
        let snapshot_name = if name.contains('@') {
            name.to_string()
        } else {
            format!("{}@{}", pool, name)
        };

        let output = run_timed(
            Command::new("sudo")
                .arg("zfs")
                .args(["rollback", "-R", &snapshot_name]),
            SNAPSHOT_WRITE_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        info!("Rolled back ZFS snapshot: {}", snapshot_name);
        Ok(())
    }

    fn rollback_btrfs_snapshot(pool: &str, _name: &str) -> Result<(), SnapError> {
        // Unlike `zfs rollback`, Btrfs has no single command that reverts a
        // *mounted, top-level* subvolume (subvolid=5, which is exactly how
        // `PoolService::create_pool` mounts every Btrfs pool) back to an
        // earlier snapshot in place — that requires either unmounting and
        // juggling default subvolumes, or a non-default `@`-subvolume mount
        // layout designed for it (as e.g. Snapper/openSUSE use). Doing this
        // naively (as an earlier version of this function attempted, via
        // `btrfs subvolume snapshot <snap> <pool>`) always fails: the
        // destination path is already an existing, mounted subvolume, and
        // `btrfs subvolume snapshot` refuses to write over one. Rather than
        // attempt a partial/unsafe rollback, fail clearly so the caller
        // knows to restore files from `<mount>/.snapshots/<name>` manually.
        let _ = Self::resolve_btrfs_mount(pool)?;
        Err(SnapError::CommandFailed(
            "In-place rollback isn't supported for Btrfs pools yet — browse and copy files back from the snapshot under <mount>/.snapshots/<name> instead".to_string(),
        ))
    }

    pub fn get_snapshot_info(pool: &str, name: &str) -> Result<Snapshot, SnapError> {
        match Self::kind_for(pool)? {
            SnapshotKind::Zfs => Self::get_zfs_snapshot_info(pool, name),
            SnapshotKind::Btrfs => Err(SnapError::Unsupported(
                "Only ZFS supported for detailed info".to_string(),
            )),
        }
    }

    fn get_zfs_snapshot_info(pool: &str, name: &str) -> Result<Snapshot, SnapError> {
        let snapshot_name = if name.contains('@') {
            name.to_string()
        } else {
            format!("{}@{}", pool, name)
        };

        let output = run_timed(
            Command::new("sudo").arg("zfs").args([
                "get",
                "-H",
                "-o",
                "property,value",
                "-p",
                "all",
                &snapshot_name,
            ]),
            SNAPSHOT_QUERY_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;

        if !output.status.success() {
            return Err(SnapError::NotFound(name.to_string()));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_zfs_snapshot_properties(pool, name, &output)
    }
}

/// Filesystem type mounted at `mount_point`, straight from `/proc/mounts`
/// — no privileges, no subprocess.
fn mount_fstype(mount_point: &str) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    mounts.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let _device = parts.next()?;
        let mount = parts.next()?;
        let fstype = parts.next()?;
        (mount == mount_point).then(|| fstype.to_string())
    })
}

fn parse_zfs_snapshots(pool: &str, output: &str) -> Result<Vec<Snapshot>, SnapError> {
    let mut snapshots = Vec::new();
    let lines = output.lines().skip(1);

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let full_name = parts[0];
            let used = parse_size(parts[1]).unwrap_or(0);
            let referenced = parse_size(parts[2]).unwrap_or(0);
            let created = parts[3].to_string();

            let snapshot_name = full_name.split('@').nth(1).unwrap_or(full_name);

            snapshots.push(Snapshot {
                name: snapshot_name.to_string(),
                pool: pool.to_string(),
                created,
                used,
                referenced,
                properties: std::collections::HashMap::new(),
            });
        }
    }

    Ok(snapshots)
}

fn parse_btrfs_snapshots(pool: &str, output: &str) -> Result<Vec<Snapshot>, SnapError> {
    let mut snapshots = Vec::new();

    // Real `btrfs subvolume list -s` output, e.g.:
    // "ID 256 gen 10 cgen 10 top level 5 otime 2026-07-14 19:57:51 path .snapshots/snap1"
    // Field positions aren't fixed-width, so look up each value by the label
    // token right before it rather than a hardcoded index.
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(path_idx) = parts.iter().position(|p| *p == "path") else {
            continue;
        };
        let Some(path) = parts.get(path_idx + 1) else {
            continue;
        };

        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_string();

        let created = match parts.iter().position(|p| *p == "otime") {
            Some(i) => [parts.get(i + 1), parts.get(i + 2)]
                .into_iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            None => String::new(),
        };

        snapshots.push(Snapshot {
            name,
            pool: pool.to_string(),
            created,
            used: 0,
            referenced: 0,
            properties: std::collections::HashMap::new(),
        });
    }

    Ok(snapshots)
}

fn parse_zfs_snapshot_properties(
    pool: &str,
    name: &str,
    output: &str,
) -> Result<Snapshot, SnapError> {
    let mut properties = std::collections::HashMap::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            properties.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    Ok(Snapshot {
        name: name.to_string(),
        pool: pool.to_string(),
        created: properties.get("creation").cloned().unwrap_or_default(),
        used: properties
            .get("used")
            .and_then(|v| parse_size(v).ok())
            .unwrap_or(0),
        referenced: properties
            .get("referenced")
            .and_then(|v| parse_size(v).ok())
            .unwrap_or(0),
        properties,
    })
}

fn parse_size(s: &str) -> Result<u64, SnapError> {
    let s = s.trim();
    let multiplier: u64;
    let numeric: String;

    if s.ends_with('K') || s.ends_with("KB") {
        multiplier = 1024;
        numeric = s.trim_end_matches(['K', 'B']).to_string();
    } else if s.ends_with('M') || s.ends_with("MB") {
        multiplier = 1024 * 1024;
        numeric = s.trim_end_matches(['M', 'B']).to_string();
    } else if s.ends_with('G') || s.ends_with("GB") {
        multiplier = 1024 * 1024 * 1024;
        numeric = s.trim_end_matches(['G', 'B']).to_string();
    } else if s.ends_with('T') || s.ends_with("TB") {
        multiplier = 1024 * 1024 * 1024 * 1024;
        numeric = s.trim_end_matches(['T', 'B']).to_string();
    } else if s.ends_with('P') || s.ends_with("PB") {
        multiplier = 1024 * 1024 * 1024 * 1024 * 1024;
        numeric = s.trim_end_matches(['P', 'B']).to_string();
    } else {
        return s.parse().map_err(|_| SnapError::ParseError(s.to_string()));
    }

    let value: f64 = numeric
        .parse()
        .map_err(|_| SnapError::ParseError(s.to_string()))?;

    Ok((value * multiplier as f64) as u64)
}

impl Default for SnapshotService {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pools::{PoolInfo, PoolType};

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_size("invalid").is_err());
    }

    fn pool(name: &str, pool_type: PoolType, mount_point: Option<&str>) -> PoolInfo {
        PoolInfo {
            name: name.to_string(),
            pool_type,
            size_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
            health: "online".to_string(),
            device_count: 1,
            mount_point: mount_point.map(str::to_string),
            raid_profile: None,
        }
    }

    /// 5.3: the backend follows the *pool's* technology. A host with both
    /// `zfs` and `btrfs` installed (what `installer/install.sh` produces)
    /// used to send every Btrfs pool down the ZFS branch.
    #[test]
    fn backend_follows_the_pool_type_not_the_installed_binaries() {
        assert_eq!(
            SnapshotService::kind_of_pool(&pool("tank", PoolType::Zfs, Some("/tank"))).unwrap(),
            SnapshotKind::Zfs
        );
        assert_eq!(
            SnapshotService::kind_of_pool(&pool("data", PoolType::Btrfs, Some("/mnt/data")))
                .unwrap(),
            SnapshotKind::Btrfs
        );
    }

    /// An mdadm array is not a filesystem: only a Btrfs-formatted one can be
    /// snapshotted, and anything else gets a clear error instead of a doomed
    /// `zfs snapshot`.
    #[test]
    fn mdadm_pools_are_resolved_by_the_filesystem_on_top() {
        let unmounted = pool("/dev/md0", PoolType::Mdadm, None);
        assert!(matches!(
            SnapshotService::kind_of_pool(&unmounted),
            Err(SnapError::Unsupported(_))
        ));

        // A mount point that isn't in /proc/mounts can't be classified
        // either — no silent ZFS attempt.
        let unknown_fs = pool("/dev/md1", PoolType::Mdadm, Some("/nonexistent-hv-mount"));
        assert!(matches!(
            SnapshotService::kind_of_pool(&unknown_fs),
            Err(SnapError::Unsupported(_))
        ));
    }

    #[test]
    fn mount_fstype_reads_proc_mounts() {
        // `/` is always mounted; an invented path never is.
        assert!(mount_fstype("/").is_some());
        assert!(mount_fstype("/nonexistent-hv-mount").is_none());
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
        let src = include_str!("snapshots.rs");
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
        assert!(SNAPSHOT_QUERY_TIMEOUT >= std::time::Duration::from_secs(5));
        assert!(SNAPSHOT_WRITE_TIMEOUT >= SNAPSHOT_QUERY_TIMEOUT);
        assert!(SNAPSHOT_WRITE_TIMEOUT <= std::time::Duration::from_secs(60 * 60));
    }
}
