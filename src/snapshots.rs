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
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use thiserror::Error;
use tracing::info;

use crate::browser::reflink::InstantCopyMode;
use crate::browser::FileBrowser;
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
/// The live Btrfs subvolume of a migrated pool.
pub const BTRFS_LIVE_SUBVOL: &str = "@";
/// The subvolume holding snapshots, mounted at `<pool mount>/.snapshots`.
pub const BTRFS_SNAPSHOT_SUBVOL: &str = "@snapshots";
/// Serializes Btrfs layout migration and in-place rollback: both unmount and
/// remount the same filesystem, so two concurrent calls would race each other.
static BTRFS_LAYOUT_LOCK: Mutex<()> = Mutex::new(());

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

/// A snapshot that covers a **share**, plus the read-only path under which the
/// share's files can be opened (Explorer) or copied back from (restore).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareSnapshot {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    /// Where the snapshot's copy of the share starts, e.g.
    /// `/tank/photos/.zfs/snapshot/hv-auto-…` (ZFS) or
    /// `/mnt/data/.snapshots/photos/hv-auto-…` (Btrfs).
    pub browse_path: String,
}

/// The backend target that contains a share path. ZFS snapshots the deepest
/// dataset whose mount point contains the share; Btrfs snapshots the share
/// path itself (which must be a subvolume) into a per-share directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareSnapshotTarget {
    Zfs {
        dataset: String,
        mount: String,
        relative: String,
    },
    Btrfs {
        mount: String,
        relative: String,
        slug: String,
    },
}

impl ShareSnapshotTarget {
    /// The snapshot root (without the share's relative path). `None` when the
    /// snapshot name is not a safe token, so a caller can never build a path
    /// out of an unchecked URL segment.
    fn snapshot_root(&self, name: &str) -> Option<String> {
        if !is_safe_snapshot_name(name) {
            return None;
        }
        match self {
            Self::Zfs { mount, .. } => Some(format!(
                "{}/.zfs/snapshot/{}",
                mount.trim_end_matches('/'),
                name
            )),
            Self::Btrfs { mount, slug, .. } => Some(format!(
                "{}/.snapshots/{}/{}",
                mount.trim_end_matches('/'),
                slug,
                name
            )),
        }
    }

    /// The snapshot's copy of the share itself (root + relative share path).
    pub fn browse_path(&self, name: &str) -> Option<String> {
        let root = self.snapshot_root(name)?;
        let relative = self.relative();
        if relative.is_empty() {
            Some(root)
        } else {
            Some(format!("{}/{}", root, relative))
        }
    }

    fn relative(&self) -> &str {
        match self {
            Self::Zfs { relative, .. } | Self::Btrfs { relative, .. } => relative,
        }
    }
}

/// The subvolume layout of a Btrfs pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BtrfsLayout {
    /// Historical layout: the pool mount point *is* the top-level subvolume.
    /// No in-place rollback is possible there.
    Legacy,
    /// `@` mounted at the pool mount point, `@snapshots` at
    /// `<mount>/.snapshots`. In-place rollback is available.
    At,
}

impl BtrfsLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::At => "at",
        }
    }
}

/// What the Storage UI/API reports for one Btrfs pool.
#[derive(Debug, Clone, Serialize)]
pub struct BtrfsLayoutStatus {
    pub pool: String,
    pub mount_point: String,
    pub layout: BtrfsLayout,
    pub layout_name: String,
    /// Whether `POST …/btrfs-rollback` can run on this pool.
    pub rollback_available: bool,
    /// Migration (legacy → `@`/`@snapshots`) is still possible.
    pub migration_required: bool,
}

/// The names and paths an in-place rollback will use. Kept as a pure value so
/// the destructive plan can be unit-tested without a real Btrfs filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BtrfsRollbackPlan {
    pub mount_point: String,
    pub snapshot: String,
    pub safety_snapshot: String,
    pub previous_subvolume: String,
    pub failed_subvolume: String,
}

/// Result of a completed in-place rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BtrfsRollbackOutcome {
    pub mount_point: String,
    pub snapshot: String,
    pub safety_snapshot: String,
    /// The pre-rollback live state, kept on the pool for manual recovery.
    pub previous_subvolume: String,
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

/// A backend scoped to one share: its containing ZFS dataset, or the share's
/// own Btrfs subvolume. Scheduled runs use this instead of [`PoolBackend`] when
/// the schedule names a share, so a per-share policy never lists/prunes the
/// pool's other snapshots.
pub struct ShareBackend {
    pool: String,
    share_path: String,
    /// Resolved once at construction: create/list/prune all reuse it instead
    /// of rescanning the host for every snapshot a prune deletes.
    target: ShareSnapshotTarget,
}

impl ShareBackend {
    pub fn new(pool: &str, share_path: &str) -> Result<Self, SnapError> {
        let target = SnapshotService::resolve_share_target(pool, share_path)?;
        Ok(Self {
            pool: pool.to_string(),
            share_path: share_path.to_string(),
            target,
        })
    }
}

impl SnapshotBackend for ShareBackend {
    fn create(&self, name: &str, recursive: bool) -> Result<(), SnapError> {
        SnapshotService::create_share_snapshot_at(&self.target, &self.share_path, name, recursive)
    }

    fn list(&self) -> Result<Vec<Snapshot>, SnapError> {
        Ok(
            SnapshotService::list_share_snapshots_at(&self.pool, &self.target)?
                .into_iter()
                .map(|s| s.snapshot)
                .collect(),
        )
    }

    fn delete(&self, name: &str) -> Result<(), SnapError> {
        SnapshotService::delete_share_snapshot_at(&self.target, name)
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

    // -- Per-share snapshots (tasklist P1-12) --------------------------------

    /// Resolves where a share lives: the deepest ZFS dataset containing it, or
    /// the Btrfs mount plus a deterministic per-share directory slug.
    pub fn resolve_share_target(
        pool: &str,
        share_path: &str,
    ) -> Result<ShareSnapshotTarget, SnapError> {
        match Self::kind_for(pool)? {
            SnapshotKind::Zfs => {
                let mounts = std::fs::read_to_string("/proc/mounts").map_err(|e| {
                    SnapError::CommandFailed(format!("Could not read /proc/mounts: {e}"))
                })?;
                zfs_target_from_mounts(pool, share_path, &mounts)
            }
            SnapshotKind::Btrfs => {
                let mount = Self::resolve_btrfs_mount(pool)?;
                btrfs_target(&mount, share_path)
            }
        }
    }

    // -- Btrfs @/@snapshots layout and in-place rollback (P1-13) ------------

    /// Reports whether `pool` can be rolled back in place. Legacy pools keep
    /// working for snapshots, but `btrfs-rollback` answers `503` until the
    /// explicit migration below has run.
    pub fn btrfs_layout_status(pool: &str) -> Result<BtrfsLayoutStatus, SnapError> {
        if Self::kind_for(pool)? != SnapshotKind::Btrfs {
            return Err(SnapError::Unsupported(format!(
                "Pool '{pool}' is not a Btrfs pool"
            )));
        }
        let mount_point = Self::resolve_btrfs_mount(pool)?;
        let layout = btrfs_layout(&mount_point)?;
        Ok(BtrfsLayoutStatus {
            pool: pool.to_string(),
            mount_point,
            layout,
            layout_name: layout.as_str().to_string(),
            rollback_available: layout == BtrfsLayout::At,
            migration_required: layout == BtrfsLayout::Legacy,
        })
    }

    /// Migrates a legacy top-level Btrfs pool to `@`/`@snapshots`.
    ///
    /// This is deliberately explicit and serialized. Existing snapshots are
    /// moved into `@snapshots`, every other top-level entry into `@`, the two
    /// fstab entries are rewritten, and the pool is remounted on `subvol=@`.
    /// Any failure before the remount moves the entries back and leaves the
    /// pool on its historical layout; after the remount the previous `@` is
    /// still the live data, so a manual recovery remains possible.
    pub fn migrate_btrfs_layout(pool: &str) -> Result<BtrfsLayoutStatus, SnapError> {
        let _guard = BTRFS_LAYOUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if Self::kind_for(pool)? != SnapshotKind::Btrfs {
            return Err(SnapError::Unsupported(format!(
                "Pool '{pool}' is not a Btrfs pool"
            )));
        }
        let mount_point = Self::resolve_btrfs_mount(pool)?;
        let mount_point = normalize_mount(&mount_point);
        if btrfs_layout(&mount_point)? == BtrfsLayout::At {
            return Err(SnapError::Unsupported(format!(
                "Pool '{pool}' already uses the @/@snapshots layout"
            )));
        }

        let mounts = std::fs::read_to_string("/proc/mounts")
            .map_err(|e| SnapError::CommandFailed(format!("Could not read /proc/mounts: {e}")))?;
        let device = mount_source_from_proc(&mounts, &mount_point).ok_or_else(|| {
            SnapError::CommandFailed(format!(
                "Could not find the device mounted at '{mount_point}'"
            ))
        })?;
        let uuid = blkid_uuid(&device)?;

        let live = format!("{mount_point}/{BTRFS_LIVE_SUBVOL}");
        let snapshots = format!("{mount_point}/{BTRFS_SNAPSHOT_SUBVOL}");
        let legacy_snapshots = format!("{mount_point}/.snapshots");

        // 1. Move the old snapshot directory under @snapshots (or create it).
        let moved_legacy_snapshots = Path::new(&legacy_snapshots).exists();
        if moved_legacy_snapshots {
            run_sudo_step(
                "mv",
                &[&legacy_snapshots, &snapshots],
                SNAPSHOT_WRITE_TIMEOUT,
                "Moving .snapshots to @snapshots",
            )?;
        } else {
            run_sudo_step(
                "btrfs",
                &["subvolume", "create", &snapshots],
                SNAPSHOT_WRITE_TIMEOUT,
                "Creating @snapshots",
            )?;
        }

        // 2. Create @ and its .snapshots mount point.
        if let Err(e) = run_sudo_step(
            "btrfs",
            &["subvolume", "create", &live],
            SNAPSHOT_WRITE_TIMEOUT,
            "Creating @",
        ) {
            rollback_btrfs_migration(pool, &mount_point, &uuid, &[], moved_legacy_snapshots);
            return Err(e);
        }
        if let Err(e) = run_sudo_step(
            "mkdir",
            &["-p", &format!("{live}/.snapshots")],
            SNAPSHOT_QUERY_TIMEOUT,
            "Creating the snapshot mount point",
        ) {
            rollback_btrfs_migration(pool, &mount_point, &uuid, &[], moved_legacy_snapshots);
            return Err(e);
        }

        // 3. Move every other top-level entry into @. A failure moves the
        //    already-moved entries back before reporting.
        let mut moved: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(&mount_point).map_err(|e| {
            SnapError::CommandFailed(format!("Could not read '{mount_point}': {e}"))
        })?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == BTRFS_LIVE_SUBVOL || name == BTRFS_SNAPSHOT_SUBVOL || name == ".snapshots" {
                continue;
            }
            let source = entry.path().to_string_lossy().into_owned();
            let destination = format!("{live}/");
            if let Err(e) = run_sudo_step(
                "mv",
                &[&source, &destination],
                SNAPSHOT_WRITE_TIMEOUT,
                &format!("Moving '{name}' into @"),
            ) {
                rollback_btrfs_migration(pool, &mount_point, &uuid, &moved, moved_legacy_snapshots);
                return Err(e);
            }
            moved.push(name);
        }

        // 4. Persist the new layout before remounting.
        if let Err(e) = write_btrfs_fstab(pool, &uuid, &mount_point, BtrfsLayout::At) {
            rollback_btrfs_migration(pool, &mount_point, &uuid, &moved, moved_legacy_snapshots);
            return Err(e);
        }

        // 5. Remount on @, then mount @snapshots under it. Any failure from
        //    here on first returns to the top-level, undoes the moves, and
        //    leaves the pool mounted on the legacy layout.
        if let Err(e) = run_sudo_step(
            "umount",
            &[&mount_point],
            SNAPSHOT_WRITE_TIMEOUT,
            "Unmounting the legacy top-level",
        ) {
            rollback_btrfs_migration(pool, &mount_point, &uuid, &moved, moved_legacy_snapshots);
            return Err(e);
        }
        let host_legacy = |error: SnapError| -> SnapError {
            // The pool is unmounted at this point. Recreate the legacy view so
            // a failed migration does not leave the data unreachable.
            let _ = run_sudo_step(
                "mount",
                &[&device, &mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: mounting the top-level",
            );
            rollback_btrfs_migration(pool, &mount_point, &uuid, &moved, moved_legacy_snapshots);
            let _ = run_sudo_step(
                "umount",
                &[&mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: unmounting the top-level",
            );
            let _ = run_sudo_step(
                "mount",
                &[&device, &mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: remounting the legacy top-level",
            );
            error
        };
        if let Err(e) = run_sudo_step(
            "mount",
            &["-o", "subvol=@", &device, &mount_point],
            SNAPSHOT_WRITE_TIMEOUT,
            "Mounting @",
        ) {
            return Err(host_legacy(e));
        }
        if let Err(e) = run_sudo_step(
            "mkdir",
            &["-p", &format!("{mount_point}/.snapshots")],
            SNAPSHOT_QUERY_TIMEOUT,
            "Creating the snapshot mount point",
        ) {
            let _ = run_sudo_step(
                "umount",
                &[&mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: unmounting @",
            );
            return Err(host_legacy(e));
        }
        if let Err(e) = run_sudo_step(
            "mount",
            &[
                "-o",
                "subvol=@snapshots",
                &device,
                &format!("{mount_point}/.snapshots"),
            ],
            SNAPSHOT_WRITE_TIMEOUT,
            "Mounting @snapshots",
        ) {
            let _ = run_sudo_step(
                "umount",
                &[&mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: unmounting @ after a failed snapshot mount",
            );
            return Err(host_legacy(e));
        }

        let layout = btrfs_layout(&mount_point)?;
        Ok(BtrfsLayoutStatus {
            pool: pool.to_string(),
            mount_point,
            layout,
            layout_name: layout.as_str().to_string(),
            rollback_available: layout == BtrfsLayout::At,
            migration_required: layout == BtrfsLayout::Legacy,
        })
    }

    /// Rolls a migrated Btrfs pool back to one of its snapshots, in place.
    ///
    /// A `pre-rollback-*` snapshot of the current `@` is taken first and the
    /// previous live subvolume is kept as `@before-rollback-*`: the swap is
    /// destructive and there is no automatic undo, but neither is data erased
    /// without a recovery copy. Legacy pools answer
    /// `SnapError::Unsupported` so the handler can return a named `503`.
    pub fn rollback_btrfs_in_place(
        pool: &str,
        snapshot: &str,
    ) -> Result<BtrfsRollbackOutcome, SnapError> {
        let _guard = BTRFS_LAYOUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if Self::kind_for(pool)? != SnapshotKind::Btrfs {
            return Err(SnapError::Unsupported(format!(
                "Pool '{pool}' is not a Btrfs pool"
            )));
        }
        let mount_point = normalize_mount(&Self::resolve_btrfs_mount(pool)?);
        match btrfs_layout(&mount_point)? {
            BtrfsLayout::At => {}
            BtrfsLayout::Legacy => {
                return Err(SnapError::Unsupported(format!(
                    "Pool '{pool}' uses the legacy top-level Btrfs layout — migrate it to @/@snapshots from Storage before rolling back"
                )))
            }
        }
        if !Self::list_btrfs_snapshots(pool)?
            .iter()
            .any(|s| s.name == snapshot)
        {
            return Err(SnapError::NotFound(snapshot.to_string()));
        }

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let plan = btrfs_rollback_plan(&mount_point, snapshot, stamp)?;

        let mounts = std::fs::read_to_string("/proc/mounts")
            .map_err(|e| SnapError::CommandFailed(format!("Could not read /proc/mounts: {e}")))?;
        let device = mount_source_from_proc(&mounts, &mount_point).ok_or_else(|| {
            SnapError::CommandFailed(format!(
                "Could not find the device mounted at '{mount_point}'"
            ))
        })?;

        // Safety copy of the state we are about to replace. It is a normal
        // Btrfs snapshot outside @, so the prune never deletes it.
        Self::create_btrfs_snapshot(pool, &plan.safety_snapshot, None)?;

        // Unmount the snapshot mount first (it lives under the live mount).
        let _ = run_sudo_step(
            "umount",
            &[&format!("{mount_point}/.snapshots")],
            SNAPSHOT_WRITE_TIMEOUT,
            "Unmounting @snapshots",
        );
        run_sudo_step(
            "umount",
            &[&mount_point],
            SNAPSHOT_WRITE_TIMEOUT,
            "Unmounting the live pool",
        )?;

        let live = format!("{mount_point}/{BTRFS_LIVE_SUBVOL}");
        let previous = format!("{mount_point}/{}", plan.previous_subvolume);
        let snapshot_source = format!("{mount_point}/{BTRFS_SNAPSHOT_SUBVOL}/{snapshot}");

        let swap = (|| -> Result<(), SnapError> {
            run_sudo_step(
                "mount",
                &[&device, &mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Mounting the top-level filesystem",
            )?;
            run_sudo_step(
                "mv",
                &[&live, &previous],
                SNAPSHOT_WRITE_TIMEOUT,
                "Preserving the previous @ subvolume",
            )?;
            run_sudo_step(
                "btrfs",
                &["subvolume", "snapshot", &snapshot_source, &live],
                SNAPSHOT_WRITE_TIMEOUT,
                "Creating @ from the snapshot",
            )?;
            run_sudo_step(
                "mkdir",
                &["-p", &format!("{live}/.snapshots")],
                SNAPSHOT_QUERY_TIMEOUT,
                "Creating the snapshot mount point",
            )?;
            run_sudo_step(
                "umount",
                &[&mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Unmounting the top-level filesystem",
            )?;
            run_sudo_step(
                "mount",
                &["-o", "subvol=@", &device, &mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Mounting the restored @",
            )?;
            run_sudo_step(
                "mkdir",
                &["-p", &format!("{mount_point}/.snapshots")],
                SNAPSHOT_QUERY_TIMEOUT,
                "Creating the snapshot mount point",
            )?;
            run_sudo_step(
                "mount",
                &[
                    "-o",
                    "subvol=@snapshots",
                    &device,
                    &format!("{mount_point}/.snapshots"),
                ],
                SNAPSHOT_WRITE_TIMEOUT,
                "Mounting @snapshots",
            )?;
            Ok(())
        })();

        if let Err(error) = swap {
            return Err(Self::recover_failed_btrfs_rollback(
                &mount_point,
                &device,
                &plan,
                error,
            ));
        }

        let layout = btrfs_layout(&mount_point)?;
        if layout != BtrfsLayout::At {
            return Err(SnapError::CommandFailed(format!(
                "Rollback finished but the pool is not mounted on {BTRFS_LIVE_SUBVOL} — the previous state is kept as '{}'",
                plan.previous_subvolume
            )));
        }
        Ok(BtrfsRollbackOutcome {
            mount_point,
            snapshot: snapshot.to_string(),
            safety_snapshot: plan.safety_snapshot,
            previous_subvolume: plan.previous_subvolume,
        })
    }

    /// Best-effort recovery after a failed swap: mount the top-level, move the
    /// failed `@` aside, put the previous `@` back, and remount the live pair.
    fn recover_failed_btrfs_rollback(
        mount_point: &str,
        device: &str,
        plan: &BtrfsRollbackPlan,
        original: SnapError,
    ) -> SnapError {
        let _ = run_sudo_step(
            "umount",
            &[mount_point],
            SNAPSHOT_WRITE_TIMEOUT,
            "Recovery: unmounting the pool",
        );
        let recovered = (|| -> Result<(), SnapError> {
            run_sudo_step(
                "mount",
                &[device, mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: mounting the top-level",
            )?;
            let previous = format!("{mount_point}/{}", plan.previous_subvolume);
            if Path::new(&previous).exists() {
                // Only undo the swap if it actually happened. A failure while
                // mounting the top-level leaves the original @ in place.
                let live = format!("{mount_point}/{BTRFS_LIVE_SUBVOL}");
                if Path::new(&live).exists() {
                    run_sudo_step(
                        "mv",
                        &[&live, &format!("{mount_point}/{}", plan.failed_subvolume)],
                        SNAPSHOT_WRITE_TIMEOUT,
                        "Recovery: moving the failed @ aside",
                    )?;
                }
                run_sudo_step(
                    "mv",
                    &[&previous, &live],
                    SNAPSHOT_WRITE_TIMEOUT,
                    "Recovery: restoring the previous @",
                )?;
            }
            run_sudo_step(
                "umount",
                &[mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: unmounting the top-level",
            )?;
            run_sudo_step(
                "mount",
                &["-o", "subvol=@", device, mount_point],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: remounting @",
            )?;
            run_sudo_step(
                "mkdir",
                &["-p", &format!("{mount_point}/.snapshots")],
                SNAPSHOT_QUERY_TIMEOUT,
                "Recovery: creating the snapshot mount point",
            )?;
            run_sudo_step(
                "mount",
                &[
                    "-o",
                    "subvol=@snapshots",
                    device,
                    &format!("{mount_point}/.snapshots"),
                ],
                SNAPSHOT_WRITE_TIMEOUT,
                "Recovery: remounting @snapshots",
            )?;
            Ok(())
        })();
        match recovered {
            Ok(()) => SnapError::CommandFailed(format!(
                "Rollback failed ({original}); the previous state was restored and the pool is available again"
            )),
            Err(recovery) => SnapError::CommandFailed(format!(
                "Rollback failed ({original}) and automatic recovery also failed ({recovery}). The previous live state is at '{}/{}'; mount the pool top-level and rename it back to '@' manually.",
                plan.mount_point, plan.previous_subvolume
            )),
        }
    }

    /// Every snapshot whose backend covers `share_path`, newest first, with
    /// the read-only browse path the Explorer/restore UI uses.
    pub fn list_share_snapshots(
        pool: &str,
        share_path: &str,
    ) -> Result<Vec<ShareSnapshot>, SnapError> {
        let target = Self::resolve_share_target(pool, share_path)?;
        Self::list_share_snapshots_at(pool, &target)
    }

    fn list_share_snapshots_at(
        pool: &str,
        target: &ShareSnapshotTarget,
    ) -> Result<Vec<ShareSnapshot>, SnapError> {
        let snapshots = match target {
            ShareSnapshotTarget::Zfs { dataset, .. } => {
                Self::list_zfs_dataset_snapshots(pool, dataset)?
            }
            ShareSnapshotTarget::Btrfs { mount, slug, .. } => {
                Self::list_btrfs_share_snapshots(pool, mount, slug)?
            }
        };
        Ok(snapshots
            .into_iter()
            .filter_map(|snapshot| {
                target
                    .browse_path(&snapshot.name)
                    .map(|browse_path| ShareSnapshot {
                        snapshot,
                        browse_path,
                    })
            })
            .collect())
    }

    /// Creates a snapshot that includes `share_path`. ZFS snapshots the
    /// containing dataset (recursively when asked); Btrfs snapshots the share
    /// path itself, which must be a subvolume.
    pub fn create_share_snapshot(
        pool: &str,
        share_path: &str,
        name: &str,
        recursive: bool,
    ) -> Result<(), SnapError> {
        if !is_safe_snapshot_name(name) {
            return Err(SnapError::ParseError(format!(
                "Invalid snapshot name '{name}'"
            )));
        }
        let target = Self::resolve_share_target(pool, share_path)?;
        Self::create_share_snapshot_at(&target, share_path, name, recursive)
    }

    fn create_share_snapshot_at(
        target: &ShareSnapshotTarget,
        share_path: &str,
        name: &str,
        recursive: bool,
    ) -> Result<(), SnapError> {
        match target {
            ShareSnapshotTarget::Zfs { dataset, .. } => {
                let snapshot_name = format!("{dataset}@{name}");
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
                info!("Created per-share ZFS snapshot: {}", snapshot_name);
                Ok(())
            }
            ShareSnapshotTarget::Btrfs { mount, slug, .. } => {
                let directory = format!("{}/.snapshots/{}", mount.trim_end_matches('/'), slug);
                let mkdir = run_timed(
                    Command::new("sudo").arg("mkdir").args(["-p", &directory]),
                    SNAPSHOT_WRITE_TIMEOUT,
                )
                .map_err(SnapError::CommandFailed)?;
                if !mkdir.status.success() {
                    return Err(SnapError::CommandFailed(
                        String::from_utf8_lossy(&mkdir.stderr).to_string(),
                    ));
                }
                let destination = format!("{directory}/{name}");
                let output = run_timed(
                    Command::new("sudo").arg("btrfs").args([
                        "subvolume",
                        "snapshot",
                        share_path,
                        &destination,
                    ]),
                    SNAPSHOT_WRITE_TIMEOUT,
                )
                .map_err(SnapError::CommandFailed)?;
                if !output.status.success() {
                    return Err(SnapError::Unsupported(format!(
                        "The share '{}' is not a Btrfs subvolume, so it cannot be snapshotted per share: {}",
                        share_path,
                        String::from_utf8_lossy(&output.stderr).trim()
                    )));
                }
                info!("Created per-share Btrfs snapshot: {}", destination);
                Ok(())
            }
        }
    }

    /// Deletes one snapshot of the share's backend.
    pub fn delete_share_snapshot(
        pool: &str,
        share_path: &str,
        name: &str,
    ) -> Result<(), SnapError> {
        if !is_safe_snapshot_name(name) {
            return Err(SnapError::ParseError(format!(
                "Invalid snapshot name '{name}'"
            )));
        }
        let target = Self::resolve_share_target(pool, share_path)?;
        Self::delete_share_snapshot_at(&target, name)
    }

    fn delete_share_snapshot_at(target: &ShareSnapshotTarget, name: &str) -> Result<(), SnapError> {
        let command = match target {
            ShareSnapshotTarget::Zfs { dataset, .. } => {
                format!("{dataset}@{name}")
            }
            ShareSnapshotTarget::Btrfs { mount, slug, .. } => {
                format!("{}/.snapshots/{}/{name}", mount.trim_end_matches('/'), slug)
            }
        };
        let output = match target {
            // `-r` removes the whole recursive snapshot set this schedule
            // created on child datasets; without it the parent would go and
            // the children would leak forever (GFS never lists them).
            ShareSnapshotTarget::Zfs { .. } => run_timed(
                Command::new("sudo")
                    .arg("zfs")
                    .args(["destroy", "-r", &command]),
                SNAPSHOT_WRITE_TIMEOUT,
            ),
            ShareSnapshotTarget::Btrfs { .. } => run_timed(
                Command::new("sudo")
                    .arg("btrfs")
                    .args(["subvolume", "delete", &command]),
                SNAPSHOT_WRITE_TIMEOUT,
            ),
        }
        .map_err(SnapError::CommandFailed)?;
        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        info!("Deleted per-share snapshot: {}", command);
        Ok(())
    }

    /// Restores one file or directory from a share snapshot back into the live
    /// share. `relative` is interpreted **inside** the share (the UI never
    /// supplies an absolute snapshot path), and both source and destination are
    /// canonicalised and required to stay under their roots.
    pub fn restore_share_snapshot(
        pool: &str,
        share_path: &str,
        name: &str,
        relative: &str,
    ) -> Result<PathBuf, SnapError> {
        let relative = sanitize_relative_path(relative)?;
        if relative.as_os_str().is_empty() {
            return Err(SnapError::ParseError(
                "Choose a file or folder to restore".to_string(),
            ));
        }
        let available = Self::list_share_snapshots(pool, share_path)?;
        if !available.iter().any(|s| s.snapshot.name == name) {
            return Err(SnapError::NotFound(name.to_string()));
        }
        let target = Self::resolve_share_target(pool, share_path)?;
        let snapshot_root = target
            .snapshot_root(name)
            .ok_or_else(|| SnapError::ParseError(format!("Invalid snapshot name '{name}'")))?;

        let shared_relative = target.relative();
        let source = if shared_relative.is_empty() {
            Path::new(&snapshot_root).join(&relative)
        } else {
            Path::new(&snapshot_root)
                .join(shared_relative)
                .join(&relative)
        };
        let share_root = Path::new(share_path);
        let destination = share_root.join(&relative);

        let canonical_snapshot_root = std::fs::canonicalize(&snapshot_root)
            .map_err(|e| SnapError::CommandFailed(format!("Snapshot is not readable: {e}")))?;
        let canonical_source = std::fs::canonicalize(&source).map_err(|_| {
            SnapError::NotFound(format!(
                "'{}' is not present in snapshot '{}'",
                relative.display(),
                name
            ))
        })?;
        if !canonical_source.starts_with(&canonical_snapshot_root) {
            return Err(SnapError::PermissionDenied);
        }

        let canonical_share = std::fs::canonicalize(share_root)
            .map_err(|e| SnapError::CommandFailed(format!("Share is not readable: {e}")))?;
        if !destination.starts_with(share_root) {
            return Err(SnapError::PermissionDenied);
        }
        if let Ok(existing) = std::fs::canonicalize(&destination) {
            if !existing.starts_with(&canonical_share) {
                return Err(SnapError::PermissionDenied);
            }
        } else if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                SnapError::CommandFailed(format!("Could not create the restore folder: {e}"))
            })?;
        }

        let metadata = std::fs::symlink_metadata(&canonical_source)
            .map_err(|e| SnapError::CommandFailed(e.to_string()))?;
        if metadata.file_type().is_symlink() {
            return Err(SnapError::Unsupported(
                "Restoring a symlink is not supported — restore the file it points to instead"
                    .to_string(),
            ));
        }
        if metadata.is_dir() {
            FileBrowser::copy_dir_all_with_mode(
                &canonical_source,
                &destination,
                InstantCopyMode::Auto,
                None,
            )
            .map_err(|e| SnapError::CommandFailed(format!("Restore failed: {e}")))?;
        } else {
            FileBrowser::copy_file_instant(&canonical_source, &destination, InstantCopyMode::Auto)
                .map_err(|e| SnapError::CommandFailed(format!("Restore failed: {e}")))?;
        }
        info!(
            "Restored {} from snapshot {} into {}",
            relative.display(),
            name,
            share_path
        );
        Ok(destination)
    }

    fn list_zfs_dataset_snapshots(pool: &str, dataset: &str) -> Result<Vec<Snapshot>, SnapError> {
        let output = run_timed(
            Command::new("sudo").arg("zfs").args([
                "list",
                "-H",
                "-p",
                "-t",
                "snapshot",
                "-o",
                "name,used,referenced,creation",
                "-d",
                "1",
                "-r",
                dataset,
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
        parse_zfs_dataset_snapshots(pool, dataset, &String::from_utf8_lossy(&output.stdout))
    }

    fn list_btrfs_share_snapshots(
        pool: &str,
        mount: &str,
        slug: &str,
    ) -> Result<Vec<Snapshot>, SnapError> {
        let output = run_timed(
            Command::new("sudo")
                .arg("btrfs")
                .args(["subvolume", "list", "-s", mount]),
            SNAPSHOT_QUERY_TIMEOUT,
        )
        .map_err(SnapError::CommandFailed)?;
        if !output.status.success() {
            return Err(SnapError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        parse_btrfs_share_snapshots(pool, slug, &String::from_utf8_lossy(&output.stdout))
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

/// A snapshot name is a URL path segment on restore/delete: keep it to the
/// characters HomeVault generates and rejects `/`, `@`, `..` and whitespace.
pub fn is_safe_snapshot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A user-supplied restore path must be relative to the share, with no `..`,
/// no absolute root and no NUL. The result is what the service joins onto the
/// snapshot root and the live share root.
pub fn sanitize_relative_path(input: &str) -> Result<PathBuf, SnapError> {
    if input.contains('\0') {
        return Err(SnapError::ParseError(
            "Restore path contains a NUL byte".to_string(),
        ));
    }
    if input.len() > 1024 {
        return Err(SnapError::ParseError(
            "Restore path is too long".to_string(),
        ));
    }
    let path = Path::new(input);
    if path.is_absolute() {
        return Err(SnapError::ParseError(
            "Restore path must be relative to the share".to_string(),
        ));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SnapError::ParseError(
                    "Restore path must stay inside the share".to_string(),
                ))
            }
        }
    }
    Ok(clean)
}

fn normalize_mount(mount: &str) -> String {
    if mount.len() > 1 {
        mount.trim_end_matches('/').to_string()
    } else {
        mount.to_string()
    }
}

/// `path` relative to `root`, or `None` when it is outside it. Both are
/// absolute, normalised mount paths (`/` is handled explicitly).
fn relative_to(root: &str, path: &str) -> Option<String> {
    let root = normalize_mount(root);
    if root == "/" {
        return path
            .starts_with('/')
            .then(|| path.trim_start_matches('/').to_string());
    }
    if path == root {
        return Some(String::new());
    }
    path.strip_prefix(&format!("{root}/")).map(str::to_string)
}

/// The deepest ZFS mount point that contains `share_path`, restricted to
/// datasets belonging to `pool`. Pure so the mount-table rules can be tested
/// without a live ZFS host.
fn zfs_target_from_mounts(
    pool: &str,
    share_path: &str,
    mounts: &str,
) -> Result<ShareSnapshotTarget, SnapError> {
    let mut best: Option<(usize, String, String, String)> = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let (Some(dataset), Some(mount), Some(fstype)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if fstype != "zfs" {
            continue;
        }
        if dataset != pool && !dataset.starts_with(&format!("{pool}/")) {
            continue;
        }
        let mount = normalize_mount(mount);
        let Some(relative) = relative_to(&mount, share_path) else {
            continue;
        };
        let deeper = best
            .as_ref()
            .map(|(len, ..)| mount.len() > *len)
            .unwrap_or(true);
        if deeper {
            best = Some((mount.len(), dataset.to_string(), mount, relative));
        }
    }
    match best {
        Some((_, dataset, mount, relative)) => Ok(ShareSnapshotTarget::Zfs {
            dataset,
            mount,
            relative,
        }),
        None => Err(SnapError::Unsupported(format!(
            "Share path '{}' is not under a mounted dataset of pool '{}'",
            share_path, pool
        ))),
    }
}

fn btrfs_target(mount: &str, share_path: &str) -> Result<ShareSnapshotTarget, SnapError> {
    let mount = normalize_mount(mount);
    let relative = relative_to(&mount, share_path).ok_or_else(|| {
        SnapError::Unsupported(format!(
            "Share path '{}' is not inside the Btrfs mount '{}'",
            share_path, mount
        ))
    })?;
    let slug = share_snapshot_slug(&relative);
    Ok(ShareSnapshotTarget::Btrfs {
        mount,
        relative,
        slug,
    })
}

/// Deterministic, filesystem-safe directory name for one share inside
/// `.snapshots`. A short hash keeps `data/photos` and `data-photos` (which
/// both slug to `data-photos`) in separate directories.
fn share_snapshot_slug(relative: &str) -> String {
    if relative.is_empty() {
        return "root".to_string();
    }
    let mut slug: String = relative
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        slug = "share".to_string();
    }
    let hash = relative.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    format!("{slug}-{hash:08x}")
}

fn parse_zfs_dataset_snapshots(
    pool: &str,
    dataset: &str,
    output: &str,
) -> Result<Vec<Snapshot>, SnapError> {
    let prefix = format!("{dataset}@");
    let mut snapshots = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let full_name = parts[0];
        let Some(name) = full_name.strip_prefix(&prefix) else {
            continue;
        };
        snapshots.push(Snapshot {
            name: name.to_string(),
            pool: pool.to_string(),
            created: parts[3].to_string(),
            used: parse_size(parts[1]).unwrap_or(0),
            referenced: parse_size(parts[2]).unwrap_or(0),
            properties: std::collections::HashMap::new(),
        });
    }
    // `zfs list` does not guarantee creation order (it sorts by name), so
    // sort on the creation timestamp the caller selected instead of trusting
    // the line order. Callers show newest first.
    snapshots
        .sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created.parse::<i64>().unwrap_or(0)));
    Ok(snapshots)
}

fn parse_btrfs_share_snapshots(
    pool: &str,
    slug: &str,
    output: &str,
) -> Result<Vec<Snapshot>, SnapError> {
    let prefix = format!(".snapshots/{slug}/");
    let mut snapshots = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(path_idx) = parts.iter().position(|p| *p == "path") else {
            continue;
        };
        let Some(path) = parts.get(path_idx + 1) else {
            continue;
        };
        let Some(relative) = path.strip_prefix(&prefix) else {
            continue;
        };
        // The snapshot directory is exactly one safe component: a path that
        // escapes it (`../…`) or nests another directory is not a snapshot of
        // this share and must never be listed.
        if relative.is_empty() || relative.contains('/') || !is_safe_snapshot_name(relative) {
            continue;
        }
        let name = relative.to_string();
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

/// Parses `findmnt -o OPTIONS` / `/proc/mounts` options. A Btrfs pool is on
/// the migrated layout only when its top-level mount uses `subvol=@`; the
/// legacy top-level mount has `subvolid=5`/`subvol=/` and no `@`.
pub fn parse_btrfs_layout_options(options: &str) -> BtrfsLayout {
    for option in options.split(',') {
        if let Some(value) = option.trim().strip_prefix("subvol=") {
            if value.trim_start_matches('/') == BTRFS_LIVE_SUBVOL {
                return BtrfsLayout::At;
            }
        }
    }
    BtrfsLayout::Legacy
}

/// The device mounted at `mount_point`, straight from `/proc/mounts` (pure so
/// the parser is testable).
fn mount_source_from_proc(mounts: &str, mount_point: &str) -> Option<String> {
    mounts.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let device = parts.next()?;
        let mount = parts.next()?;
        (normalize_mount(mount) == normalize_mount(mount_point)).then(|| device.to_string())
    })
}

/// The destructive plan for `btrfs-rollback`, without touching the host.
pub fn btrfs_rollback_plan(
    mount_point: &str,
    snapshot: &str,
    stamp: u64,
) -> Result<BtrfsRollbackPlan, SnapError> {
    if !is_safe_snapshot_name(snapshot) {
        return Err(SnapError::ParseError(format!(
            "Invalid snapshot name '{snapshot}'"
        )));
    }
    Ok(BtrfsRollbackPlan {
        mount_point: normalize_mount(mount_point),
        snapshot: snapshot.to_string(),
        safety_snapshot: format!("pre-rollback-{stamp}"),
        previous_subvolume: format!("@before-rollback-{stamp}"),
        failed_subvolume: format!("@failed-rollback-{stamp}"),
    })
}

/// Commands that put the previous `@` back after a failed swap. The caller
/// runs these while the top-level subvolume is mounted at the pool mount.
pub fn btrfs_rollback_recovery_commands(plan: &BtrfsRollbackPlan) -> Vec<Vec<String>> {
    let top = &plan.mount_point;
    vec![
        vec![
            "mv".to_string(),
            format!("{top}/{BTRFS_LIVE_SUBVOL}"),
            format!("{top}/{}", plan.failed_subvolume),
        ],
        vec![
            "mv".to_string(),
            format!("{top}/{}", plan.previous_subvolume),
            format!("{top}/{BTRFS_LIVE_SUBVOL}"),
        ],
    ]
}

/// Runs one privileged step via argv (never a shell) under a deadline.
fn run_sudo_step(
    program: &str,
    args: &[&str],
    timeout: Duration,
    label: &str,
) -> Result<(), SnapError> {
    let output = run_timed(Command::new("sudo").arg(program).args(args), timeout)
        .map_err(|e| SnapError::CommandFailed(format!("{label}: {e}")))?;
    if !output.status.success() {
        return Err(SnapError::CommandFailed(format!(
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn btrfs_layout(mount_point: &str) -> Result<BtrfsLayout, SnapError> {
    let output = run_timed(
        Command::new("findmnt").args(["-n", "-o", "OPTIONS", "--target", mount_point]),
        SNAPSHOT_QUERY_TIMEOUT,
    )
    .map_err(SnapError::CommandFailed)?;
    if !output.status.success() {
        return Err(SnapError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(parse_btrfs_layout_options(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn blkid_uuid(device: &str) -> Result<String, SnapError> {
    let output = run_timed(
        Command::new("sudo")
            .arg("blkid")
            .args(["-s", "UUID", "-o", "value", device]),
        SNAPSHOT_QUERY_TIMEOUT,
    )
    .map_err(SnapError::CommandFailed)?;
    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || uuid.is_empty() {
        return Err(SnapError::CommandFailed(format!(
            "Could not read the filesystem UUID of {device}"
        )));
    }
    Ok(uuid)
}

/// Rewrites the two pool fstab entries for the chosen layout. The snapshot
/// entry is tagged separately so deleting the pool removes both.
fn write_btrfs_fstab(
    pool: &str,
    uuid: &str,
    mount_point: &str,
    layout: BtrfsLayout,
) -> Result<(), SnapError> {
    let marker = format!("# homevault-pool:{pool}");
    let snapshots_marker = format!("# homevault-pool-snapshots:{pool}");
    crate::pools::PoolService::remove_pool_fstab_entry(&marker);
    crate::pools::PoolService::remove_pool_fstab_entry(&snapshots_marker);
    let (live, snapshots) = match layout {
        BtrfsLayout::Legacy => (
            format!("UUID={uuid} {mount_point} btrfs defaults,nofail 0 2"),
            None,
        ),
        BtrfsLayout::At => (
            format!(
                "UUID={uuid} {mount_point} btrfs defaults,nofail,subvol={BTRFS_LIVE_SUBVOL} 0 2"
            ),
            Some(format!(
                "UUID={uuid} {mount_point}/.snapshots btrfs defaults,nofail,subvol={BTRFS_SNAPSHOT_SUBVOL} 0 2"
            )),
        ),
    };
    crate::pools::PoolService::append_pool_fstab_entry(&marker, &live)
        .map_err(|e| SnapError::CommandFailed(format!("Could not write /etc/fstab: {e}")))?;
    if let Some(line) = snapshots {
        crate::pools::PoolService::append_pool_fstab_entry(&snapshots_marker, &line)
            .map_err(|e| SnapError::CommandFailed(format!("Could not write /etc/fstab: {e}")))?;
    }
    Ok(())
}

/// Restores a Btrfs pool to the legacy layout while its **top-level** is
/// mounted at `mount_point`. Used when migration fails before/while remounting.
fn rollback_btrfs_migration(
    pool: &str,
    mount_point: &str,
    uuid: &str,
    moved: &[String],
    moved_legacy_snapshots: bool,
) {
    let live = format!("{mount_point}/{BTRFS_LIVE_SUBVOL}");
    let snapshots = format!("{mount_point}/{BTRFS_SNAPSHOT_SUBVOL}");
    let legacy_snapshots = format!("{mount_point}/.snapshots");
    for name in moved.iter().rev() {
        let _ = run_sudo_step(
            "mv",
            &[&format!("{live}/{name}"), &format!("{mount_point}/")],
            SNAPSHOT_WRITE_TIMEOUT,
            "Rolling back a moved entry",
        );
    }
    if moved_legacy_snapshots {
        let _ = run_sudo_step(
            "mv",
            &[&snapshots, &legacy_snapshots],
            SNAPSHOT_WRITE_TIMEOUT,
            "Moving @snapshots back to .snapshots",
        );
    } else {
        let _ = run_sudo_step(
            "btrfs",
            &["subvolume", "delete", &snapshots],
            SNAPSHOT_WRITE_TIMEOUT,
            "Removing the empty @snapshots",
        );
    }
    // The `.snapshots` mount point inside @ blocks `subvolume delete`;
    // remove it first when it is empty.
    let _ = run_sudo_step(
        "rmdir",
        &[&format!("{live}/.snapshots")],
        SNAPSHOT_QUERY_TIMEOUT,
        "Removing the empty snapshot mount point",
    );
    let _ = run_sudo_step(
        "btrfs",
        &["subvolume", "delete", &live],
        SNAPSHOT_WRITE_TIMEOUT,
        "Removing the empty @",
    );
    let _ = write_btrfs_fstab(pool, uuid, mount_point, BtrfsLayout::Legacy);
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

// ---------------------------------------------------------------------------
// Per-file versions for the Explorer's "Rewind" drawer
// ---------------------------------------------------------------------------

/// The deepest root in `roots` that contains `target`, with segment
/// boundaries respected (`/srv/photos` must not match `/srv/photos2`).
pub fn deepest_containing_root(target: &str, roots: &[String]) -> Option<String> {
    let target = target.trim_end_matches('/');
    roots
        .iter()
        .map(|root| root.trim_end_matches('/'))
        .filter(|root| {
            !root.is_empty()
                && (target == *root
                    || target
                        .strip_prefix(root)
                        .is_some_and(|rest| rest.starts_with('/')))
        })
        .max_by_key(|root| root.len())
        .map(str::to_string)
}

/// The read-only path of `target` inside the snapshot whose copy of the share
/// starts at `snapshot_browse`.
///
/// `snapshot_browse` already points at the snapshot's copy of the **share**
/// (`…/.snapshots/<slug>/<name>`), so only the part of `target` below the
/// share has to be appended. `None` when `target` is not inside the share —
/// a caller can never build a version path out of a sibling folder.
pub fn version_path_in_snapshot(
    share_path: &str,
    snapshot_browse: &str,
    target: &str,
) -> Option<String> {
    let share = share_path.trim_end_matches('/');
    let target = target.trim_end_matches('/');
    let relative = if target == share {
        ""
    } else {
        let rest = target.strip_prefix(share)?;
        if !rest.starts_with('/') {
            return None;
        }
        rest.trim_start_matches('/')
    };
    let base = snapshot_browse.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    Some(if relative.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{relative}")
    })
}

/// Btrfs snapshot versions of `target`, each pointing at `target` itself.
///
/// `None` means "this path is not on a Btrfs pool / has no snapshot backend",
/// which is how the Explorer knows to keep whatever the ZFS listing produced
/// instead of showing a misleading empty list. An error is only returned for a
/// host that genuinely failed (pool scan, `btrfs subvolume list`).
pub fn btrfs_versions_for_path(target: &str) -> Option<Result<Vec<ShareSnapshot>, SnapError>> {
    let shares = crate::shares::SharesService::get_shares().ok()?;
    let paths: Vec<String> = shares.iter().map(|share| share.path.clone()).collect();
    let share_path = deepest_containing_root(target, &paths)?;

    let pools = crate::pools::PoolService::scan_pools().ok()?;
    let mounts: Vec<String> = pools.iter().filter_map(|p| p.mount_point.clone()).collect();
    let mount = deepest_containing_root(&share_path, &mounts)?;
    let pool = pools.iter().find(|p| {
        p.mount_point
            .as_deref()
            .map(|mp| mp.trim_end_matches('/') == mount)
            .unwrap_or(false)
    })?;
    if pool.pool_type != crate::pools::PoolType::Btrfs {
        return None;
    }

    Some(
        SnapshotService::list_share_snapshots(&pool.name, &share_path).map(|snapshots| {
            snapshots
                .into_iter()
                .filter_map(|mut snapshot| {
                    let browse =
                        version_path_in_snapshot(&share_path, &snapshot.browse_path, target)?;
                    snapshot.browse_path = browse;
                    Some(snapshot)
                })
                .collect()
        }),
    )
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

    // -- P1-12: per-share targets, names and browse paths --------------------

    const MOUNTS: &str = "\
tank /tank zfs rw,relatime 0 0
tank/photos /tank/photos zfs rw,relatime 0 0
other /other zfs rw,relatime 0 0
";

    #[test]
    fn zfs_share_target_picks_the_deepest_dataset_of_the_pool() {
        let target = zfs_target_from_mounts("tank", "/tank/photos/2024/a.jpg", MOUNTS).unwrap();
        assert_eq!(
            target,
            ShareSnapshotTarget::Zfs {
                dataset: "tank/photos".to_string(),
                mount: "/tank/photos".to_string(),
                relative: "2024/a.jpg".to_string(),
            }
        );

        // A path outside the nested dataset falls back to the pool root.
        let root = zfs_target_from_mounts("tank", "/tank/docs", MOUNTS).unwrap();
        assert_eq!(
            root,
            ShareSnapshotTarget::Zfs {
                dataset: "tank".to_string(),
                mount: "/tank".to_string(),
                relative: "docs".to_string(),
            }
        );

        // Another pool's dataset never matches, even with a containing mount.
        assert!(matches!(
            zfs_target_from_mounts("tank", "/other/data", MOUNTS),
            Err(SnapError::Unsupported(_))
        ));
    }

    #[test]
    fn btrfs_share_target_confines_the_path_and_hashes_the_slug() {
        let target = btrfs_target("/mnt/data", "/mnt/data/photos").unwrap();
        let ShareSnapshotTarget::Btrfs { relative, slug, .. } = &target else {
            panic!("expected btrfs target")
        };
        assert_eq!(relative, "photos");
        assert!(slug.starts_with("photos-"), "{slug}");

        // `data/photos` and `data-photos` must not share a snapshot directory.
        let nested = share_snapshot_slug("data/photos");
        let flat = share_snapshot_slug("data-photos");
        assert_ne!(nested, flat);
        assert_eq!(share_snapshot_slug("data/photos"), nested);

        assert!(matches!(
            btrfs_target("/mnt/data", "/mnt/other/photos"),
            Err(SnapError::Unsupported(_))
        ));
    }

    #[test]
    fn restore_paths_stay_relative_to_the_share() {
        assert_eq!(
            sanitize_relative_path("photos/2024/a.jpg").unwrap(),
            PathBuf::from("photos/2024/a.jpg")
        );
        // `.` components are harmless and collapse.
        assert_eq!(
            sanitize_relative_path("./photos/a.jpg").unwrap(),
            PathBuf::from("photos/a.jpg")
        );
        for dangerous in ["../etc/passwd", "/etc/passwd", "a/../../b", "a\0b"] {
            assert!(
                sanitize_relative_path(dangerous).is_err(),
                "{dangerous} must be refused"
            );
        }
    }

    #[test]
    fn snapshot_names_are_single_safe_path_segments() {
        assert!(is_safe_snapshot_name("hv-auto-20261105-120000"));
        assert!(is_safe_snapshot_name("manual.1_a"));
        for bad in ["", ".", "..", "a/b", "a@b", "a b", "../x"] {
            assert!(!is_safe_snapshot_name(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn browse_paths_point_at_the_share_inside_the_snapshot() {
        let zfs = ShareSnapshotTarget::Zfs {
            dataset: "tank/photos".to_string(),
            mount: "/tank/photos".to_string(),
            relative: "2024".to_string(),
        };
        assert_eq!(
            zfs.browse_path("hv-auto-20261105-120000").unwrap(),
            "/tank/photos/.zfs/snapshot/hv-auto-20261105-120000/2024"
        );
        assert!(zfs.browse_path("../escape").is_none());

        let btrfs = ShareSnapshotTarget::Btrfs {
            mount: "/mnt/data".to_string(),
            relative: "photos".to_string(),
            slug: "photos-deadbeef".to_string(),
        };
        assert_eq!(
            btrfs.browse_path("manual-20261105-120000").unwrap(),
            "/mnt/data/.snapshots/photos-deadbeef/manual-20261105-120000/photos"
        );
    }

    #[test]
    fn per_share_listing_parsers_ignore_other_datasets_and_shares() {
        let zfs = parse_zfs_dataset_snapshots(
            "tank",
            "tank/photos",
            "tank/photos@newest\t1M\t2M\t1760000000\n\
             tank/photos/child@child\t1M\t2M\t1760000000\n\
             tank/docs@other\t1M\t2M\t1760000000\n\
             tank/photos@oldest\t1M\t2M\t1750000000\n",
        )
        .unwrap();
        // Parent dataset only, and newest first.
        assert_eq!(
            zfs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["newest", "oldest"]
        );

        let btrfs = parse_btrfs_share_snapshots(
            "data",
            "photos-deadbeef",
            "ID 256 gen 1 cgen 1 top level 5 otime 2026-11-05 12:00:00 path .snapshots/photos-deadbeef/mine\n\
             ID 257 gen 1 cgen 1 top level 5 otime 2026-11-05 12:00:00 path .snapshots/other/mine\n\
             ID 258 gen 1 cgen 1 top level 5 otime 2026-11-05 12:00:00 path .snapshots/photos-deadbeef/../../bad\n",
        )
        .unwrap();
        assert_eq!(btrfs.len(), 1);
        assert_eq!(btrfs[0].name, "mine");
    }

    // -- P1-13: Btrfs @/@snapshots layout and in-place rollback --------------

    #[test]
    fn btrfs_layout_is_read_from_the_subvol_mount_option() {
        assert_eq!(
            parse_btrfs_layout_options("rw,relatime,subvolid=257,subvol=/@"),
            BtrfsLayout::At
        );
        assert_eq!(
            parse_btrfs_layout_options("rw,relatime,subvol=@"),
            BtrfsLayout::At
        );
        assert_eq!(
            parse_btrfs_layout_options("rw,relatime,subvolid=5,subvol=/"),
            BtrfsLayout::Legacy
        );
        assert_eq!(
            parse_btrfs_layout_options("rw,relatime,space_cache=v2"),
            BtrfsLayout::Legacy
        );
    }

    #[test]
    fn mount_source_is_read_without_guessing() {
        let mounts = "\
/dev/sda1 / btrfs rw 0 0
/dev/sdb1 /mnt/pools/data btrfs rw,subvol=/@ 0 0
";
        assert_eq!(
            mount_source_from_proc(mounts, "/mnt/pools/data").as_deref(),
            Some("/dev/sdb1")
        );
        assert_eq!(
            mount_source_from_proc(mounts, "/mnt/pools/data/").as_deref(),
            Some("/dev/sdb1")
        );
        assert!(mount_source_from_proc(mounts, "/mnt/pools/nope").is_none());
    }

    #[test]
    fn rollback_plan_uses_safe_distinct_names_and_a_recovery_path() {
        let plan = btrfs_rollback_plan("/mnt/pools/data", "hv-auto-20261105-120000", 1_760_000_000)
            .unwrap();
        assert_eq!(plan.mount_point, "/mnt/pools/data");
        assert_eq!(plan.safety_snapshot, "pre-rollback-1760000000");
        assert_eq!(plan.previous_subvolume, "@before-rollback-1760000000");
        assert_eq!(plan.failed_subvolume, "@failed-rollback-1760000000");
        assert_ne!(plan.safety_snapshot, plan.snapshot);

        let recovery = btrfs_rollback_recovery_commands(&plan);
        assert_eq!(
            recovery[0],
            vec![
                "mv".to_string(),
                "/mnt/pools/data/@".to_string(),
                "/mnt/pools/data/@failed-rollback-1760000000".to_string(),
            ]
        );
        assert_eq!(
            recovery[1],
            vec![
                "mv".to_string(),
                "/mnt/pools/data/@before-rollback-1760000000".to_string(),
                "/mnt/pools/data/@".to_string(),
            ]
        );

        assert!(btrfs_rollback_plan("/mnt/pools/data", "../escape", 1).is_err());
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

    // -- per-file versions (Btrfs, Explorer Rewind) --------------------------

    #[test]
    fn test_deepest_containing_root_respects_segment_boundaries() {
        let roots = vec![
            "/srv".to_string(),
            "/srv/photos".to_string(),
            "/srv/photos2".to_string(),
        ];
        assert_eq!(
            deepest_containing_root("/srv/photos/2024/a.jpg", &roots).as_deref(),
            Some("/srv/photos")
        );
        // `/srv/photos2` must not be treated as inside `/srv/photos`.
        assert_eq!(
            deepest_containing_root("/srv/photos2/a.jpg", &roots).as_deref(),
            Some("/srv/photos2")
        );
        assert_eq!(
            deepest_containing_root("/srv/photos", &roots).as_deref(),
            Some("/srv/photos")
        );
        assert_eq!(deepest_containing_root("/etc/passwd", &roots), None);
        assert_eq!(
            deepest_containing_root("/srv/photos/", &roots).as_deref(),
            Some("/srv/photos")
        );
    }

    #[test]
    fn test_version_path_appends_only_the_part_below_the_share() {
        let snapshot = "/mnt/data/.snapshots/photos/hv-auto-2026";
        assert_eq!(
            version_path_in_snapshot("/mnt/data/photos", snapshot, "/mnt/data/photos/2024/a.jpg")
                .as_deref(),
            Some("/mnt/data/.snapshots/photos/hv-auto-2026/2024/a.jpg")
        );
        // The share root itself.
        assert_eq!(
            version_path_in_snapshot("/mnt/data/photos", snapshot, "/mnt/data/photos").as_deref(),
            Some(snapshot)
        );
        // Outside the share, or a sibling whose name merely starts the same.
        assert_eq!(
            version_path_in_snapshot("/mnt/data/photos", snapshot, "/mnt/data/other/a.jpg"),
            None
        );
        assert_eq!(
            version_path_in_snapshot("/mnt/data/photos", snapshot, "/mnt/data/photos2/a.jpg"),
            None
        );
        assert_eq!(
            version_path_in_snapshot("/mnt/data/photos", "", "/mnt/data/photos/a.jpg"),
            None
        );
    }
}
