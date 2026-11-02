/* =============================================================================
 * File          : snapshot_locks.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Immutable snapshots: a retention lock is expressed twice — as a row in
 *   `snapshot_locks` (what the API consults) and as a filesystem-side hold
 *   (`zfs hold` / a read-only Btrfs subvolume), so a snapshot cannot be
 *   destroyed before its expiry even by a direct filesystem command. Expiry is
 *   enforced by the scheduler that releases lapsed locks; a break-glass
 *   release requires a typed confirmation and is journaled.
 *
 * Dependencies:
 *   - crate::snapshots (pool/backend resolution), sqlx (through the repository)
 *
 * Usage:
 *   handlers/snapshot_locks.rs, snapshot_scheduler.rs (prune) and the
 *   lock-release loop; see docs/guides/snapshots.md.
 * =============================================================================
 */

//! Immutable snapshots (see docs/guides/snapshots.md).
//!
//! The lock is deliberately **two** things:
//!
//! 1. a row in `snapshot_locks`, which is what HomeVault's own delete/prune
//!    paths consult before destroying anything;
//! 2. a filesystem-side hold — `zfs hold <tag>` or `btrfs property set ro
//!    true` — which is what makes `zfs destroy` itself fail. A UI-only flag
//!    would be a promise the appliance could not keep.
//!
//! `locked_until` is the retention horizon. A lock that has lapsed is *not*
//! silently considered released: the release loop (`release_expired`) runs
//! `zfs release` / `btrfs property … ro false` and only then marks the row
//! released. Until that runs, the snapshot stays protected — failing closed.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration as StdDuration;

use crate::snapshots::SnapshotKind;
use crate::utils::process::run_timed;

/// The ZFS hold tag HomeVault owns. One tag, so an admin can find every lock
/// with `zfs holds -r | grep homevault-lock` and nothing else claims it.
pub const LOCK_TAG: &str = "homevault-lock";

/// Deadline for one hold/release call. Cheap on a healthy pool; a pool whose
/// disks are spinning up still answers well inside it.
pub const LOCK_OP_TIMEOUT: StdDuration = StdDuration::from_secs(60);

/// Longest lock a single request may place (10 years). A longer horizon is a
/// typo, and an accidental "locked forever" snapshot on a full pool is a real
/// operational problem.
pub const MAX_LOCK_DAYS: i64 = 3650;

/// A row of `snapshot_locks`, as the API and the UI render it.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SnapshotLock {
    pub id: i64,
    pub pool: String,
    pub snapshot: String,
    pub tag: String,
    pub locked_until: String,
    pub reason: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub released_at: Option<String>,
    pub released_by: Option<String>,
    pub release_reason: Option<String>,
}

impl SnapshotLock {
    /// Whether this lock still protects its snapshot at `now` (unreleased,
    /// whatever the horizon — an expired hold is still a hold).
    pub fn is_active(&self) -> bool {
        self.released_at.is_none()
    }

    /// Whether the retention horizon has passed.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        parse_time(&self.locked_until).is_some_and(|until| until <= now)
    }

    /// The horizon as a timestamp (`None` when the stored value is corrupt —
    /// which the caller must treat as "still active", never as "expired").
    pub fn horizon(&self) -> Option<DateTime<Utc>> {
        parse_time(&self.locked_until)
    }

    /// The **bare** snapshot name (`pool@name` → `name`, a Btrfs name as-is).
    pub fn bare_snapshot(&self) -> &str {
        self.snapshot.rsplit('@').next().unwrap_or(&self.snapshot)
    }
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Whether a delete/prune must skip this snapshot.
///
/// A lock protects until its row is released; the expiry only decides when the
/// scheduler is allowed to release it. Corrupt horizons count as protected.
pub fn protects(locks: &[SnapshotLock], pool: &str, snapshot: &str, _now: DateTime<Utc>) -> bool {
    let bare = snapshot.rsplit('@').next().unwrap_or(snapshot);
    locks
        .iter()
        .any(|lock| lock.is_active() && lock.pool == pool && lock.bare_snapshot() == bare)
}

/// The bare names of every currently protected snapshot in one pool.
pub fn protected_names(locks: &[SnapshotLock], pool: &str) -> std::collections::HashSet<String> {
    locks
        .iter()
        .filter(|lock| lock.is_active() && lock.pool == pool)
        .map(|lock| lock.bare_snapshot().to_string())
        .collect()
}

/// Validation of a new lock request. Returns the horizon.
pub fn validate_lock_request(
    snapshot: &str,
    days: i64,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, String> {
    validate_snapshot_name(snapshot)?;
    if days <= 0 {
        return Err("choose a retention of at least one day".to_string());
    }
    if days > MAX_LOCK_DAYS {
        return Err(format!(
            "a lock cannot exceed {MAX_LOCK_DAYS} days ({MAX_LOCK_DAYS} days ≈ 10 years)"
        ));
    }
    Ok(now + Duration::days(days))
}

/// A snapshot name as HomeVault stores it: a bare name or `dataset@name`.
///
/// The value ends up in a `zfs`/`btrfs` argument and in a filesystem path
/// (`.snapshots/<name>`), so a `/`, `..`, a leading `-` or a control character
/// is refused.
pub fn validate_snapshot_name(snapshot: &str) -> Result<(), String> {
    let s = snapshot.trim();
    if s.is_empty() {
        return Err("a snapshot name is required".to_string());
    }
    if s.len() > 200 {
        return Err("the snapshot name is too long".to_string());
    }
    if s.starts_with('-') || s.starts_with('.') {
        return Err("a snapshot name cannot start with '-' or '.'".to_string());
    }
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("a snapshot name cannot contain spaces or control characters".to_string());
    }
    if s.split('/').any(|part| part == ".." || part == ".") {
        return Err("a snapshot name cannot contain '.' or '..' segments".to_string());
    }
    if s.contains('/') {
        // Btrfs snapshots live directly under `.snapshots/`; a ZFS name is
        // `dataset@snap`. A slash would escape the snapshots directory.
        return Err("a snapshot name cannot contain '/'".to_string());
    }
    Ok(())
}

/// The phrase an admin must type to release a lock before its horizon.
///
/// A typed phrase (not a checkbox) is the point: breaking a retention lock is
/// the one action whose whole purpose is to be hard to do by accident.
pub fn break_glass_phrase(snapshot: &str) -> String {
    format!(
        "release {}",
        snapshot.rsplit('@').next().unwrap_or(snapshot)
    )
}

/// Whether a release request is allowed.
///
/// * after the horizon — a normal release, no phrase needed;
/// * before it — refused unless `confirmation` equals
///   [`break_glass_phrase`], and the caller then journals it as break-glass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseDecision {
    /// The horizon has passed (or the row is already released).
    Normal,
    /// Before the horizon, with the typed phrase: allowed and audited.
    BreakGlass,
    /// Before the horizon, without the phrase.
    Refused,
}

pub fn decide_release(
    lock: &SnapshotLock,
    confirmation: Option<&str>,
    now: DateTime<Utc>,
) -> ReleaseDecision {
    if !lock.is_active() || lock.is_expired(now) {
        return ReleaseDecision::Normal;
    }
    match confirmation.map(str::trim) {
        Some(value) if value == break_glass_phrase(&lock.snapshot) => ReleaseDecision::BreakGlass,
        _ => ReleaseDecision::Refused,
    }
}

/// The `sudo zfs hold` argument vector placing the lock.
pub fn zfs_hold_args(dataset_snapshot: &str) -> Vec<String> {
    vec![
        "zfs".to_string(),
        "hold".to_string(),
        LOCK_TAG.to_string(),
        dataset_snapshot.to_string(),
    ]
}

/// The `sudo zfs release` argument vector removing it.
pub fn zfs_release_args(dataset_snapshot: &str) -> Vec<String> {
    vec![
        "zfs".to_string(),
        "release".to_string(),
        LOCK_TAG.to_string(),
        dataset_snapshot.to_string(),
    ]
}

/// The `sudo btrfs property set` arguments that make a snapshot read-only
/// (`read_only = true`) or writable again (`false`).
pub fn btrfs_read_only_args(path: &str, read_only: bool) -> Vec<String> {
    vec![
        "btrfs".to_string(),
        "property".to_string(),
        "set".to_string(),
        path.to_string(),
        "ro".to_string(),
        if read_only { "true" } else { "false" }.to_string(),
    ]
}

/// The filesystem path of a Btrfs snapshot (`<pool mount>/.snapshots/<name>`).
pub fn btrfs_snapshot_path(pool: &str, snapshot: &str) -> Result<String, String> {
    let mount = crate::pools::PoolService::btrfs_mount_point(pool)
        .ok_or_else(|| format!("pool '{pool}' is not mounted"))?;
    let bare = snapshot.rsplit('@').next().unwrap_or(snapshot);
    Ok(format!("{mount}/.snapshots/{bare}"))
}

fn run_sudo(args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.args(args);
    match run_timed(&mut cmd, LOCK_OP_TIMEOUT) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("{} failed: {}", args.join(" "), stderr.trim()))
        }
        Err(e) => Err(format!("{} failed: {e}", args.join(" "))),
    }
}

/// Place the filesystem-side hold for one snapshot.
pub fn apply_hold(kind: SnapshotKind, pool: &str, snapshot: &str) -> Result<(), String> {
    validate_snapshot_name(snapshot)?;
    match kind {
        SnapshotKind::Zfs => {
            let target = if snapshot.contains('@') {
                snapshot.to_string()
            } else {
                format!("{pool}@{snapshot}")
            };
            run_sudo(&zfs_hold_args(&target))
        }
        SnapshotKind::Btrfs => {
            let path = btrfs_snapshot_path(pool, snapshot)?;
            run_sudo(&btrfs_read_only_args(&path, true))
        }
    }
}

/// Remove the filesystem-side hold.
pub fn release_hold(kind: SnapshotKind, pool: &str, snapshot: &str) -> Result<(), String> {
    validate_snapshot_name(snapshot)?;
    match kind {
        SnapshotKind::Zfs => {
            let target = if snapshot.contains('@') {
                snapshot.to_string()
            } else {
                format!("{pool}@{snapshot}")
            };
            run_sudo(&zfs_release_args(&target))
        }
        SnapshotKind::Btrfs => {
            let path = btrfs_snapshot_path(pool, snapshot)?;
            run_sudo(&btrfs_read_only_args(&path, false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn lock(snapshot: &str, until: DateTime<Utc>) -> SnapshotLock {
        SnapshotLock {
            id: 1,
            pool: "tank".to_string(),
            snapshot: snapshot.to_string(),
            tag: LOCK_TAG.to_string(),
            locked_until: until.to_rfc3339(),
            reason: Some("ransomware protection".to_string()),
            created_by: "admin".to_string(),
            created_at: "2026-10-31T00:00:00+00:00".to_string(),
            released_at: None,
            released_by: None,
            release_reason: None,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap()
    }

    // -----------------------------------------------------------------
    // Protection state machine
    // -----------------------------------------------------------------

    #[test]
    fn test_an_active_lock_protects_its_snapshot_in_both_name_shapes() {
        let locks = vec![lock("snap-1", now() + Duration::days(30))];
        assert!(protects(&locks, "tank", "snap-1", now()));
        // A ZFS name (`tank@snap-1`) matches on the bare name.
        assert!(protects(&locks, "tank", "tank@snap-1", now()));
        // Another pool or another snapshot is untouched.
        assert!(!protects(&locks, "other", "snap-1", now()));
        assert!(!protects(&locks, "tank", "snap-2", now()));
    }

    #[test]
    fn test_a_released_lock_protects_nothing() {
        let mut released = lock("snap-1", now() + Duration::days(30));
        released.released_at = Some(now().to_rfc3339());
        assert!(!released.is_active());
        assert!(!protects(&[released], "tank", "snap-1", now()));
    }

    #[test]
    fn test_an_expired_but_unreleased_lock_still_protects() {
        // The horizon passed, but the scheduler has not released the hold yet:
        // failing closed is the only safe reading.
        let lapsed = lock("snap-1", now() - Duration::days(1));
        assert!(lapsed.is_expired(now()));
        assert!(protects(&[lapsed], "tank", "snap-1", now()));
    }

    #[test]
    fn test_a_corrupt_horizon_is_not_treated_as_expired() {
        let mut broken = lock("snap-1", now() + Duration::days(1));
        broken.locked_until = "not a timestamp".to_string();
        assert!(!broken.is_expired(now()));
        assert!(protects(&[broken], "tank", "snap-1", now()));
    }

    #[test]
    fn test_protected_names_is_per_pool() {
        let locks = vec![
            lock("a", now() + Duration::days(1)),
            lock("b", now() + Duration::days(1)),
            lock("c", now() + Duration::days(1)),
        ];
        let names = protected_names(&locks, "tank");
        assert_eq!(names.len(), 3);
        assert!(names.contains("a") && names.contains("b") && names.contains("c"));
        assert!(protected_names(&locks, "other").is_empty());
    }

    // -----------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------

    #[test]
    fn test_lock_request_horizon_is_bounded() {
        assert!(validate_lock_request("snap-1", 30, now()).is_ok());
        assert!(validate_lock_request("snap-1", 0, now()).is_err());
        assert!(validate_lock_request("snap-1", -5, now()).is_err());
        assert!(validate_lock_request("snap-1", MAX_LOCK_DAYS + 1, now()).is_err());
        // The horizon is `now + days`, to the day.
        let horizon = validate_lock_request("snap-1", 30, now()).unwrap();
        assert_eq!(horizon, now() + Duration::days(30));
    }

    #[test]
    fn test_snapshot_name_validation_refuses_escapes() {
        assert!(validate_snapshot_name("snap-20261031-1200").is_ok());
        assert!(validate_snapshot_name("tank@snap-1").is_ok());
        assert!(validate_snapshot_name("").is_err());
        assert!(validate_snapshot_name("../etc").is_err());
        assert!(validate_snapshot_name(".snapshots/x").is_err());
        assert!(validate_snapshot_name("-destroy").is_err());
        assert!(validate_snapshot_name("a b").is_err());
        assert!(validate_snapshot_name("a/b").is_err());
        assert!(validate_snapshot_name("a\nb").is_err());
        assert!(validate_snapshot_name(&"x".repeat(201)).is_err());
    }

    // -----------------------------------------------------------------
    // Release decision / break-glass
    // -----------------------------------------------------------------

    #[test]
    fn test_release_after_the_horizon_needs_no_confirmation() {
        let lapsed = lock("snap-1", now() - Duration::days(1));
        assert_eq!(
            decide_release(&lapsed, None, now()),
            ReleaseDecision::Normal
        );
        let already = SnapshotLock {
            released_at: Some(now().to_rfc3339()),
            ..lock("snap-2", now() + Duration::days(1))
        };
        assert_eq!(
            decide_release(&already, None, now()),
            ReleaseDecision::Normal
        );
    }

    #[test]
    fn test_release_before_the_horizon_requires_the_exact_phrase() {
        let active = lock("tank@snap-1", now() + Duration::days(30));
        assert_eq!(
            decide_release(&active, None, now()),
            ReleaseDecision::Refused
        );
        assert_eq!(
            decide_release(&active, Some("release snap-1"), now()),
            ReleaseDecision::BreakGlass
        );
        // The full `pool@name` form is not the phrase, and neither is a
        // near-miss: the point of the phrase is that it is exact.
        assert_eq!(
            decide_release(&active, Some("release tank@snap-1"), now()),
            ReleaseDecision::Refused
        );
        assert_eq!(
            decide_release(&active, Some("RELEASE SNAP-1"), now()),
            ReleaseDecision::Refused
        );
        assert_eq!(break_glass_phrase("tank@snap-1"), "release snap-1");
    }

    // -----------------------------------------------------------------
    // Command construction
    // -----------------------------------------------------------------

    #[test]
    fn test_hold_and_release_commands_name_the_tag_and_the_snapshot() {
        assert_eq!(
            zfs_hold_args("tank@snap-1"),
            vec!["zfs", "hold", LOCK_TAG, "tank@snap-1"]
        );
        assert_eq!(
            zfs_release_args("tank@snap-1"),
            vec!["zfs", "release", LOCK_TAG, "tank@snap-1"]
        );
        assert_eq!(
            btrfs_read_only_args("/mnt/pools/tank/.snapshots/snap-1", true),
            vec![
                "btrfs",
                "property",
                "set",
                "/mnt/pools/tank/.snapshots/snap-1",
                "ro",
                "true"
            ]
        );
        assert_eq!(
            btrfs_read_only_args("/mnt/pools/tank/.snapshots/snap-1", false)[5],
            "false"
        );
    }

    #[test]
    fn test_a_command_is_built_from_validated_values_only() {
        // An escape attempt never reaches the argv builder.
        assert!(apply_hold(SnapshotKind::Btrfs, "tank", "../../etc").is_err());
        assert!(release_hold(SnapshotKind::Zfs, "tank", "-destroy").is_err());
    }
}
