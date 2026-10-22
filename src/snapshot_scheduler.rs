/* =============================================================================
 * File          : snapshot_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled pool snapshots with GFS (grandfather-father-son) retention:
 *   one background loop creates `{prefix}-{YYYYMMDD-HHMMSS}` snapshots on
 *   each schedule's cron, then prunes expired generations — deleting only
 *   snapshots carrying the schedule's own prefix. The bucketing/selection
 *   logic is a pure, unit-tested function; pool calls go through the
 *   `SnapshotBackend` trait (spawn_blocking), resolved once per run.
 *   Runs are single-flight per schedule and per pool, and a failed prune is
 *   recorded as a failure (it used to be a green run).
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; CRUD via
 *   handlers/snapshot_schedules.rs.
 * =============================================================================
 */

//! Snapshot schedules: create + GFS-prune loop (see docs/guides/snapshots.md).

use crate::handlers::AppState;
use crate::restic_scheduler::compute_next_run;
use crate::snapshots::{PoolBackend, SnapshotBackend};
use crate::storage::snapshot_schedule_repo::{SnapshotSchedule, SnapshotScheduleRepository};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 30;
/// Snapshot name format: `{prefix}-YYYYMMDD-HHMMSS` — the timestamp suffix
/// is what GFS ordering parses back out.
pub const SNAPSHOT_TIME_FORMAT: &str = "%Y%m%d-%H%M%S";

/// GFS retention counts (one row of the schedule, hoisted for the pure fn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GfsPolicy {
    pub hourly: u32,
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
    pub yearly: u32,
}

/// The GFS tier a snapshot falls into at a given instant: the *youngest*
/// tier whose window still contains it, so each snapshot is counted exactly
/// once (a 2-hour-old snapshot is an hourly generation, not also a daily
/// one — the newest of each *day* is picked from among snapshots that have
/// aged out of the hourly window).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GfsTier {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl GfsTier {
    /// How much time one kept generation of this tier covers — the window
    /// is `keep_count × period`, so "keep 48 hourly" really keeps 48 hours.
    /// `Yearly` has no span of its own: it takes everything older than the
    /// monthly window.
    fn span_secs(&self, policy: &GfsPolicy) -> i64 {
        match self {
            GfsTier::Hourly => policy.hourly as i64 * 3600,
            GfsTier::Daily => policy.daily as i64 * 24 * 3600,
            GfsTier::Weekly => policy.weekly as i64 * 7 * 24 * 3600,
            GfsTier::Monthly => policy.monthly as i64 * 30 * 24 * 3600,
            GfsTier::Yearly => i64::MAX,
        }
    }

    /// The bucket key snapshots of this tier compete within: hour / day /
    /// ISO week / month / year of their creation time.
    fn bucket_key(&self, ts: DateTime<Utc>) -> String {
        match self {
            GfsTier::Hourly => ts.format("%Y%m%d%H").to_string(),
            GfsTier::Daily => ts.format("%Y%m%d").to_string(),
            GfsTier::Weekly => ts.format("%G-W%V").to_string(),
            GfsTier::Monthly => ts.format("%Y%m").to_string(),
            GfsTier::Yearly => ts.format("%Y").to_string(),
        }
    }

    fn keep_count(&self, policy: &GfsPolicy) -> u32 {
        match self {
            GfsTier::Hourly => policy.hourly,
            GfsTier::Daily => policy.daily,
            GfsTier::Weekly => policy.weekly,
            GfsTier::Monthly => policy.monthly,
            GfsTier::Yearly => policy.yearly,
        }
    }
}

/// The bounded tiers, youngest first. Order matters: a snapshot belongs to
/// the first tier whose window still contains it.
const BOUNDED_TIERS: [GfsTier; 4] = [
    GfsTier::Hourly,
    GfsTier::Daily,
    GfsTier::Weekly,
    GfsTier::Monthly,
];

/// Window end (exclusive, seconds of age) of each bounded tier.
///
/// Derived from the **configured** counts, not from constants: the fixed
/// 24 h / 7 d / 30 d / 365 d windows meant a schedule asking for "keep 48
/// hourly" could never see more than 24 hourly buckets, and "keep 8 weekly"
/// never more than 4 — the operator's choice was silently ignored. Windows
/// are clamped to be non-decreasing so a tier can never become unreachable
/// (200 hourly buckets span further than a 7-day daily window).
fn tier_windows(policy: &GfsPolicy) -> [i64; 4] {
    let mut windows = [0i64; 4];
    let mut previous_end = 0i64;
    for (i, tier) in BOUNDED_TIERS.iter().enumerate() {
        previous_end = previous_end.max(tier.span_secs(policy));
        windows[i] = previous_end;
    }
    windows
}

fn tier_for_age(age_secs: i64, policy: &GfsPolicy) -> GfsTier {
    let windows = tier_windows(policy);
    for (i, tier) in BOUNDED_TIERS.iter().enumerate() {
        if age_secs < windows[i] {
            return *tier;
        }
    }
    GfsTier::Yearly
}

/// Parses the timestamp out of an auto-snapshot name. Returns `None` for
/// names that don't carry a `{prefix}-YYYYMMDD-HHMMSS` suffix — such
/// snapshots are never selected for deletion (belt and braces: only
/// machine-generated names are prunable).
pub fn parse_auto_snapshot_time(name: &str, prefix: &str) -> Option<DateTime<Utc>> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('-')?;
    if rest.len() != 15 || rest.as_bytes()[8] != b'-' {
        return None;
    }
    // Naive (no offset) — snapshot names are always written in UTC by
    // `run_schedule`, and `DateTime::parse_from_str` would demand an offset
    // the name doesn't carry.
    chrono::NaiveDateTime::parse_from_str(rest, SNAPSHOT_TIME_FORMAT)
        .ok()
        .map(|d| d.and_utc())
}

/// One snapshot as the GFS selector sees it: bare name + creation time.
pub type NamedSnapshot = (String, DateTime<Utc>);

/// Snapshots grouped per GFS tier, then per generation bucket.
type TierBuckets =
    std::collections::HashMap<GfsTier, std::collections::HashMap<String, Vec<NamedSnapshot>>>;

/// Selects which auto snapshots to delete under GFS. Only receives
/// snapshots already filtered to the schedule's prefix — the caller (and
/// the tests) guarantee manual/restic snapshots can't reach this.
pub fn select_gfs_deletions(
    snaps: &[NamedSnapshot],
    policy: &GfsPolicy,
    now: DateTime<Utc>,
) -> Vec<String> {
    use std::collections::HashMap;

    // Classify each snapshot into its single youngest applicable tier.
    let mut tiers: TierBuckets = HashMap::new();
    for (name, ts) in snaps {
        let age = (now - *ts).num_seconds().max(0);
        let tier = tier_for_age(age, policy);
        tiers
            .entry(tier)
            .or_default()
            .entry(tier.bucket_key(*ts))
            .or_default()
            .push((name.clone(), *ts));
    }

    let mut deletions = Vec::new();
    for tier in [
        GfsTier::Hourly,
        GfsTier::Daily,
        GfsTier::Weekly,
        GfsTier::Monthly,
        GfsTier::Yearly,
    ] {
        let keep = tier.keep_count(policy);
        let Some(buckets) = tiers.get_mut(&tier) else {
            continue;
        };
        // Within the tier: keep the newest `keep` BUCKETS (generations), and
        // within each kept bucket keep only its newest snapshot — that is
        // what makes N=1 yearly "one snapshot per year", not one per year
        // plus its stragglers.
        let mut bucket_keys: Vec<String> = buckets.keys().cloned().collect();
        bucket_keys.sort();
        let keep_usize = keep as usize;
        // Ascending bucket keys = oldest first; the newest `keep` buckets
        // (the last `keep` indices) are the generations that survive.
        let keep_from = bucket_keys.len().saturating_sub(keep_usize);
        for (i, key) in bucket_keys.iter().enumerate() {
            let Some(bucket) = buckets.get_mut(key) else {
                continue;
            };
            bucket.sort_by_key(|a| std::cmp::Reverse(a.1));
            let bucket_kept = if i >= keep_from {
                1 // newest snapshot of a kept generation
            } else {
                0 // whole generation expired
            };
            for (j, (name, _)) in bucket.iter().enumerate() {
                if j >= bucket_kept {
                    deletions.push(name.clone());
                }
            }
        }
    }
    deletions
}

/// Runs forever. Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = tick_due(&state).await {
            tracing::warn!("Snapshot scheduler tick failed: {}", e);
        }
    }
}

async fn tick_due(state: &Arc<AppState>) -> Result<(), String> {
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    let schedules = repo.list_enabled().await.map_err(|e| e.to_string())?;
    let now = Utc::now();

    for schedule in schedules {
        let after = schedule
            .last_run_at
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| now - chrono::Duration::days(8));
        let next = match compute_next_run(&schedule.cron, after) {
            Ok(n) => n,
            Err(reason) => {
                let _ = repo
                    .mark_finished(&schedule.id, false, &format!("Invalid cron: {reason}"))
                    .await;
                continue;
            }
        };
        let Some(next) = next else { continue };
        let _ = repo
            .set_next_run(&schedule.id, next.to_rfc3339().as_str())
            .await;

        if next <= now {
            let snapshot_state = state.clone();
            let schedule_clone = schedule.clone();
            // One schedule at a time (sequential in this tick): snapshots
            // contend for the pool anyway, and it keeps ZFS calls serial.
            match run_schedule(&snapshot_state, &schedule_clone).await {
                Ok(_) => {}
                // A "Run now" already owns this schedule or pool: skip this
                // tick rather than run two prunes over the same set.
                Err(RunScheduleError::Busy(msg)) => tracing::info!(
                    "Snapshot schedule '{}' (pool {}) skipped: {}",
                    schedule_clone.prefix,
                    schedule_clone.pool,
                    msg
                ),
                Err(RunScheduleError::Failed(msg)) => tracing::warn!(
                    "Snapshot schedule '{}' (pool {}) failed: {}",
                    schedule_clone.prefix,
                    schedule_clone.pool,
                    msg
                ),
            }
        }
    }
    Ok(())
}

/// Why a scheduled run produced no summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunScheduleError {
    /// Another run already holds this schedule or its pool. Nothing was
    /// attempted, so the schedule row keeps its previous status.
    Busy(String),
    /// The run was attempted and failed; already recorded on the schedule
    /// row and, on the first failure, in the notification center.
    Failed(String),
}

impl std::fmt::Display for RunScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunScheduleError::Busy(m) | RunScheduleError::Failed(m) => f.write_str(m),
        }
    }
}

/// Runs in progress, by schedule id and by pool.
#[derive(Default)]
struct RunRegistry {
    schedules: HashSet<String>,
    pools: HashSet<String>,
}

/// Process-wide: one process owns both the scheduler loop and the HTTP
/// handlers, so an in-memory set is enough (same reasoning as the cache
/// mover's `MOVER_RUNNING`).
static RUNNING: LazyLock<Mutex<RunRegistry>> = LazyLock::new(|| Mutex::new(RunRegistry::default()));

/// Claims a schedule **and** its pool for one run. Dropping it releases
/// both, so an early return or a panic can never wedge a schedule forever.
/// Only the insert/remove touches the mutex — never across an await.
#[derive(Debug)]
struct RunGuard {
    schedule_id: String,
    pool: String,
}

impl RunGuard {
    fn acquire(schedule: &SnapshotSchedule) -> Result<Self, RunScheduleError> {
        let mut registry = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !registry.schedules.insert(schedule.id.clone()) {
            return Err(RunScheduleError::Busy(format!(
                "Schedule '{}' is already running",
                schedule.prefix
            )));
        }
        if !registry.pools.insert(schedule.pool.clone()) {
            registry.schedules.remove(&schedule.id);
            return Err(RunScheduleError::Busy(format!(
                "Another snapshot run is already in progress on pool '{}'",
                schedule.pool
            )));
        }
        Ok(Self {
            schedule_id: schedule.id.clone(),
            pool: schedule.pool.clone(),
        })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut registry = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        registry.schedules.remove(&self.schedule_id);
        registry.pools.remove(&self.pool);
    }
}

/// A run that keeps failing (a renamed pool, a full disk) must notify once,
/// not on every tick: only the transition into failure does.
fn is_new_failure(previous_status: Option<&str>) -> bool {
    previous_status != Some("failed")
}

/// Records a failed run and, on the transition into failure, raises a
/// notification the admin can mute (`snapshot.schedules`).
async fn record_failure(state: &Arc<AppState>, schedule: &SnapshotSchedule, message: &str) {
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    let notify = is_new_failure(schedule.last_status.as_deref());
    repo.mark_finished(&schedule.id, false, message).await.ok();
    if !notify {
        return;
    }
    crate::notification_engine::notify(
        state,
        "snapshot.schedules",
        &format!("Snapshot schedule on '{}' failed", schedule.pool),
        message,
        "error",
        "storage",
        "snapshot_scheduler",
    )
    .await;
}

/// Creates the snapshot, then prunes expired generations. `Ok(summary)`
/// reports what happened; a failure is recorded on the schedule row.
///
/// The pool's backend is resolved once here (`PoolBackend`), so a run costs
/// a single pool scan instead of one per snapshot.
pub async fn run_schedule(
    state: &Arc<AppState>,
    schedule: &SnapshotSchedule,
) -> Result<String, RunScheduleError> {
    run_schedule_inner(state, schedule, None).await
}

/// Runs a schedule against an injected backend. Used by tests to force a
/// prune failure without a real pool; production code goes through
/// [`run_schedule`].
pub async fn run_schedule_with(
    state: &Arc<AppState>,
    schedule: &SnapshotSchedule,
    backend: Arc<dyn SnapshotBackend>,
) -> Result<String, RunScheduleError> {
    run_schedule_inner(state, schedule, Some(backend)).await
}

async fn run_schedule_inner(
    state: &Arc<AppState>,
    schedule: &SnapshotSchedule,
    backend: Option<Arc<dyn SnapshotBackend>>,
) -> Result<String, RunScheduleError> {
    // Single-flight first: a refused run must not touch the row (5.2).
    let _guard = RunGuard::acquire(schedule)?;

    let backend = match backend {
        Some(backend) => backend,
        None => {
            let pool = schedule.pool.clone();
            let resolved = tokio::task::spawn_blocking(move || PoolBackend::new(&pool))
                .await
                .map_err(|e| RunScheduleError::Failed(format!("Snapshot task panicked: {e}")))?;
            match resolved {
                Ok(backend) => Arc::new(backend) as Arc<dyn SnapshotBackend>,
                Err(e) => {
                    let message = e.to_string();
                    record_failure(state, schedule, &message).await;
                    return Err(RunScheduleError::Failed(message));
                }
            }
        }
    };

    let now = Utc::now();
    let name = format!("{}-{}", schedule.prefix, now.format(SNAPSHOT_TIME_FORMAT));

    let create_backend = backend.clone();
    let create_name = name.clone();
    let recursive = schedule.recursive;
    let created =
        tokio::task::spawn_blocking(move || create_backend.create(&create_name, recursive))
            .await
            .map_err(|e| RunScheduleError::Failed(format!("Snapshot task panicked: {e}")))?;
    if let Err(e) = created {
        let message = e.to_string();
        record_failure(state, schedule, &message).await;
        return Err(RunScheduleError::Failed(message));
    }

    // Prune: the backend lists, we keep only THIS schedule's prefix (manual
    // snapshots and other schedules' prefixes are untouchable), then destroy
    // what GFS selects.
    let policy = GfsPolicy {
        hourly: schedule.gfs_hourly,
        daily: schedule.gfs_daily,
        weekly: schedule.gfs_weekly,
        monthly: schedule.gfs_monthly,
        yearly: schedule.gfs_yearly,
    };
    let prefix = schedule.prefix.clone();
    let prune_backend = backend.clone();
    let pruned =
        tokio::task::spawn_blocking(move || prune_pool(prune_backend.as_ref(), &prefix, &policy))
            .await
            .map_err(|e| RunScheduleError::Failed(format!("Prune task panicked: {e}")))?;

    match pruned {
        Ok(deleted) => {
            let summary = format!("Created {name}, pruned {} snapshot(s)", deleted.len());
            let repo = SnapshotScheduleRepository::new(state.db.pool());
            repo.mark_finished(&schedule.id, true, &summary).await.ok();
            Ok(summary)
        }
        Err(e) => {
            // The snapshot was taken but retention did not run: the pool
            // keeps filling up, so this is a failed run, not a green one
            // (it used to be recorded as `success` in both arms).
            let message = format!("Created {name}; prune failed: {e}");
            record_failure(state, schedule, &message).await;
            Err(RunScheduleError::Failed(message))
        }
    }
}

/// Lists the pool's snapshots, keeps only `{prefix}-…` auto ones, and
/// destroys the ones GFS selects. Returns the deleted names.
fn prune_pool(
    backend: &dyn SnapshotBackend,
    prefix: &str,
    policy: &GfsPolicy,
) -> Result<Vec<String>, String> {
    let listed = backend.list().map_err(|e| e.to_string())?;
    let mut autos: Vec<NamedSnapshot> = Vec::new();
    for snap in listed {
        // Match on the snapshot's bare name (ZFS lists `pool@name`).
        let bare = snap
            .name
            .rsplit('@')
            .next()
            .unwrap_or(&snap.name)
            .to_string();
        if let Some(ts) = parse_auto_snapshot_time(&bare, prefix) {
            autos.push((bare, ts));
        }
    }
    let to_delete = select_gfs_deletions(&autos, policy, Utc::now());
    for name in &to_delete {
        backend.delete(name).map_err(|e| e.to_string())?;
    }
    Ok(to_delete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs_ago: i64, now: DateTime<Utc>) -> DateTime<Utc> {
        now - chrono::Duration::seconds(secs_ago)
    }

    fn names(v: Vec<String>) -> Vec<String> {
        v
    }

    #[allow(dead_code)]
    fn names_len(v: Vec<String>) -> usize {
        v.len()
    }

    #[test]
    fn auto_snapshot_names_parse_and_reject_others() {
        let prefix = "hv-auto";
        let ts = parse_auto_snapshot_time("hv-auto-20261013-120000", prefix).expect("valid");
        assert_eq!(
            ts.format(SNAPSHOT_TIME_FORMAT).to_string(),
            "20261013-120000"
        );

        // Manual snapshots and other prefixes are unparseable → never pruned.
        assert!(parse_auto_snapshot_time("manual-snap", prefix).is_none());
        assert!(parse_auto_snapshot_time("other-20261013-120000", prefix).is_none());
        assert!(parse_auto_snapshot_time("hv-auto-notatime", prefix).is_none());
        assert!(parse_auto_snapshot_time("hv-auto-20261013-1200", prefix).is_none());
    }

    #[test]
    fn tier_assignment_uses_the_youngest_matching_window() {
        let now = Utc::now();
        // The default retention (24 hourly / 7 daily / 4 weekly / 12
        // monthly / 1 yearly) yields 24 h, 7 d, 28 d and 360 d windows.
        let policy = GfsPolicy {
            hourly: 24,
            daily: 7,
            weekly: 4,
            monthly: 12,
            yearly: 1,
        };
        assert_eq!(
            tier_for_age((now - at(3600, now)).num_seconds(), &policy),
            GfsTier::Hourly
        );
        assert_eq!(tier_for_age(2 * 24 * 3600, &policy), GfsTier::Daily);
        assert_eq!(tier_for_age(10 * 24 * 3600, &policy), GfsTier::Weekly);
        assert_eq!(tier_for_age(100 * 24 * 3600, &policy), GfsTier::Monthly);
        assert_eq!(tier_for_age(400 * 24 * 3600, &policy), GfsTier::Yearly);
    }

    /// The windows are derived from the configured counts, not from
    /// constants, and stay ordered so no tier can become unreachable.
    #[test]
    fn tier_windows_follow_the_counts() {
        let windows = tier_windows(&GfsPolicy {
            hourly: 48,
            daily: 7,
            weekly: 8,
            monthly: 12,
            yearly: 1,
        });
        assert_eq!(windows[0], 48 * 3600);
        assert_eq!(windows[1], 7 * 86_400);
        assert_eq!(windows[2], 8 * 7 * 86_400);
        assert_eq!(windows[3], 12 * 30 * 86_400);

        // 500 hourly buckets span further than a 7-day daily window: the
        // daily window is clamped, never narrowed.
        let clamped = tier_windows(&GfsPolicy {
            hourly: 500,
            daily: 7,
            ..Default::default()
        });
        assert_eq!(clamped[0], 500 * 3600);
        assert_eq!(clamped[1], 500 * 3600);
    }

    /// "hourly, keep 48" really keeps 48 hours. With the old fixed 24 h
    /// window the second day of hourly generations fell into the daily tier
    /// and was thinned to one per day.
    #[test]
    fn hourly_retention_can_exceed_a_day() {
        let now = Utc::now();
        let hour = 3600;
        let snaps: Vec<NamedSnapshot> = (1..=30)
            .map(|i| (format!("h{i}"), at(i * hour + 60, now)))
            .collect();
        let del = select_gfs_deletions(
            &snaps,
            &GfsPolicy {
                hourly: 48,
                daily: 7,
                ..Default::default()
            },
            now,
        );
        assert!(
            del.is_empty(),
            "all 30 hourly generations fit in the 48 h window: {del:?}"
        );
    }

    #[test]
    fn gfs_keeps_the_newest_of_each_generation() {
        // Fixed `now` (12:00 UTC): the three snapshots land at 11:10, 11:26
        // and 11:43 — provably the same wall-clock hour bucket regardless of
        // when the test runs (Utc::now() made this flaky).
        let now = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 10, 13, 12, 0, 0).unwrap();
        // 3 snapshots in the same hour: hourly=1 keeps only the newest.
        let snaps = vec![
            ("a".to_string(), at(3000, now)),
            ("b".to_string(), at(2000, now)),
            ("c".to_string(), at(1000, now)),
        ];
        let del = select_gfs_deletions(
            &snaps,
            &GfsPolicy {
                hourly: 1,
                ..Default::default()
            },
            now,
        );
        // One bucket (hour 11): keep the newest (c), delete the rest newest-
        // first — that's the bucket's internal ordering, and it's stable.
        assert_eq!(names(del), vec!["b".to_string(), "a".to_string()]);
    }

    /// Within a tier, only the newest N generation buckets survive. Yearly
    /// has no window of its own (it takes everything older than monthly), so
    /// the count alone decides: 5 yearly generations, keep 2.
    #[test]
    fn gfs_keeps_n_generations_per_tier() {
        let now = Utc::now();
        let year = 365 * 24 * 3600;
        let snaps: Vec<NamedSnapshot> = (1..=5)
            .map(|i| (format!("y{i}"), at(i * year + 120, now)))
            .collect();
        let del = select_gfs_deletions(
            &snaps,
            &GfsPolicy {
                yearly: 2,
                ..Default::default()
            },
            now,
        );
        // y1/y2 are the two newest years; y3..y5 expire (oldest first).
        assert_eq!(
            names(del),
            vec!["y5".to_string(), "y4".to_string(), "y3".to_string()]
        );
    }

    #[test]
    fn gfs_zero_policy_deletes_everything_in_that_tier() {
        let now = Utc::now();
        let snaps = vec![
            ("a".to_string(), at(60, now)),
            ("b".to_string(), at(120, now)),
        ];
        let del = select_gfs_deletions(
            &snaps,
            &GfsPolicy {
                hourly: 0,
                daily: 0,
                weekly: 0,
                monthly: 0,
                yearly: 0,
            },
            now,
        );
        assert_eq!(names(del).len(), 2, "policy 0 = keep nothing");
    }

    #[test]
    fn gfs_aged_snapshots_move_to_older_tiers_not_deleted_early() {
        let now = Utc::now();
        let day = 24 * 3600;
        // One snapshot per day for 10 days (ages 1d+2m .. 10d+2m), daily=7,
        // everything else 0. Snapshots older than the 7-day daily window
        // fall into the weekly tier, where weekly=0 expires them: d7..d10
        // (ages 7d+2m and beyond) go; d1..d6 stay via the daily tier.
        let snaps: Vec<NamedSnapshot> = (1..=10)
            .map(|i| (format!("d{i}"), at(i * day + 120, now)))
            .collect();
        let del = select_gfs_deletions(
            &snaps,
            &GfsPolicy {
                hourly: 0,
                daily: 7,
                weekly: 0,
                monthly: 0,
                yearly: 0,
            },
            now,
        );
        assert_eq!(del.len(), 4);
        for gone in ["d7", "d8", "d9", "d10"] {
            assert!(del.iter().any(|d| d == gone), "{gone} must be pruned");
        }
        for kept in ["d1", "d2", "d6"] {
            assert!(!del.iter().any(|d| d == kept), "{kept} must be kept");
        }
    }

    #[test]
    fn gfs_never_deletes_a_snapshot_that_has_no_timestamp() {
        // Only names that parse are passed in by prune_pool; assert the
        // pure fn is total on its input contract (no panic, no phantom
        // deletions on empty input).
        let now = Utc::now();
        assert!(select_gfs_deletions(&[], &GfsPolicy::default(), now).is_empty());
    }

    #[test]
    fn weekly_bucket_key_is_iso_week() {
        let ts = Utc.with_ymd_and_hms(2026, 1, 6, 12, 0, 0).unwrap(); // Tuesday
        let key = GfsTier::Weekly.bucket_key(ts);
        assert!(key.starts_with("2026-W"), "got {key}");
    }

    fn schedule(id: &str, pool: &str) -> SnapshotSchedule {
        SnapshotSchedule {
            id: id.to_string(),
            pool: pool.to_string(),
            enabled: true,
            cron: "0 */4 * * *".to_string(),
            prefix: "hv-auto".to_string(),
            recursive: true,
            gfs_hourly: 24,
            gfs_daily: 7,
            gfs_weekly: 4,
            gfs_monthly: 12,
            gfs_yearly: 1,
            last_run_at: None,
            last_status: None,
            last_error: None,
            next_run_at: None,
            created_at: "2026-10-22T00:00:00Z".to_string(),
            updated_at: "2026-10-22T00:00:00Z".to_string(),
        }
    }

    /// 5.2: a second start is refused while one run holds the schedule, and
    /// the claim is released when the guard drops.
    #[test]
    fn one_run_per_schedule_at_a_time() {
        let s = schedule("id-single-flight-a", "pool-single-flight-a");
        let guard = RunGuard::acquire(&s).expect("the first run takes the lock");

        let refused = RunGuard::acquire(&s);
        assert!(
            matches!(refused, Err(RunScheduleError::Busy(_))),
            "a concurrent run of the same schedule must be refused"
        );

        drop(guard);
        assert!(
            RunGuard::acquire(&s).is_ok(),
            "the claim must be released when the run ends"
        );
    }

    /// 5.2: two schedules on the same pool cannot prune concurrently even
    /// though their ids differ — one would delete what the other just kept.
    #[test]
    fn one_run_per_pool_at_a_time() {
        let first = schedule("id-single-flight-b1", "pool-single-flight-b");
        let second = schedule("id-single-flight-b2", "pool-single-flight-b");
        let guard = RunGuard::acquire(&first).expect("the first run takes the pool");

        match RunGuard::acquire(&second) {
            Err(RunScheduleError::Busy(msg)) => {
                assert!(msg.contains("pool-single-flight-b"), "got {msg}")
            }
            other => panic!("expected Busy, got {other:?}"),
        }

        drop(guard);
        // The refused schedule did not keep a claim of its own.
        assert!(RunGuard::acquire(&second).is_ok());
    }

    /// A schedule that fails every tick (a renamed pool, a full disk) must
    /// notify once, on the transition — not on every poll.
    #[test]
    fn only_the_transition_into_failure_notifies() {
        assert!(is_new_failure(None));
        assert!(is_new_failure(Some("success")));
        assert!(!is_new_failure(Some("failed")));
    }
}
