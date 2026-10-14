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
 *   logic is a pure, unit-tested function; ZFS/Btrfs calls go through
 *   SnapshotService (spawn_blocking).
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
use crate::snapshots::SnapshotService;
use crate::storage::snapshot_schedule_repo::{SnapshotSchedule, SnapshotScheduleRepository};
use chrono::{DateTime, Utc};
use std::sync::Arc;
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
    /// Window end (exclusive) in seconds of age. 365 days approximates a
    /// year — GFS doesn't need calendar precision, only stable ordering.
    fn window_secs(&self) -> i64 {
        match self {
            GfsTier::Hourly => 24 * 3600,
            GfsTier::Daily => 7 * 24 * 3600,
            GfsTier::Weekly => 30 * 24 * 3600,
            GfsTier::Monthly => 365 * 24 * 3600,
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

fn tier_for_age(age_secs: i64) -> GfsTier {
    for tier in [
        GfsTier::Hourly,
        GfsTier::Daily,
        GfsTier::Weekly,
        GfsTier::Monthly,
    ] {
        if age_secs < tier.window_secs() {
            return tier;
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
        let tier = tier_for_age(age);
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
            if let Err(e) = run_schedule(&snapshot_state, &schedule_clone).await {
                tracing::warn!(
                    "Snapshot schedule '{}' (pool {}) failed: {}",
                    schedule_clone.prefix,
                    schedule_clone.pool,
                    e
                );
            }
        }
    }
    Ok(())
}

/// Creates the snapshot, then prunes expired generations. `Ok(summary)`
/// reports what happened; errors mark the schedule failed.
pub async fn run_schedule(
    state: &Arc<AppState>,
    schedule: &SnapshotSchedule,
) -> Result<String, String> {
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    let now = Utc::now();
    let name = format!("{}-{}", schedule.prefix, now.format(SNAPSHOT_TIME_FORMAT));

    let create_result = {
        let pool = schedule.pool.clone();
        let name = name.clone();
        let recursive = schedule.recursive;
        tokio::task::spawn_blocking(move || {
            SnapshotService::create_snapshot(&pool, &name, None, recursive)
        })
        .await
        .map_err(|e| format!("Snapshot task panicked: {e}"))?
        .map_err(|e| e.to_string())
    };
    if let Err(e) = create_result {
        repo.mark_finished(&schedule.id, false, &e).await.ok();
        return Err(e);
    }

    // Prune: list, filter to THIS schedule's prefix (manual snapshots and
    // other schedules' prefixes are untouchable), select, destroy.
    let pool = schedule.pool.clone();
    let prefix = schedule.prefix.clone();
    let policy = GfsPolicy {
        hourly: schedule.gfs_hourly,
        daily: schedule.gfs_daily,
        weekly: schedule.gfs_weekly,
        monthly: schedule.gfs_monthly,
        yearly: schedule.gfs_yearly,
    };
    let prune_result = tokio::task::spawn_blocking(move || prune_pool(&pool, &prefix, &policy))
        .await
        .map_err(|e| format!("Prune task panicked: {e}"))?;

    let summary = match prune_result {
        Ok(deleted) => format!("Created {name}, pruned {} snapshot(s)", deleted.len()),
        Err(e) => format!("Created {name}; prune failed: {e}"),
    };
    repo.mark_finished(&schedule.id, true, &summary).await.ok();
    Ok(summary)
}

/// Lists a pool's snapshots, keeps only `{prefix}-…` auto ones, and destroys
/// the ones GFS selects. Returns the deleted names.
fn prune_pool(pool: &str, prefix: &str, policy: &GfsPolicy) -> Result<Vec<String>, String> {
    let listed = SnapshotService::list_snapshots(pool).map_err(|e| e.to_string())?;
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
        SnapshotService::delete_snapshot(pool, name).map_err(|e| e.to_string())?;
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
        assert_eq!(
            tier_for_age((now - at(3600, now)).num_seconds()),
            GfsTier::Hourly
        );
        assert_eq!(tier_for_age(2 * 24 * 3600), GfsTier::Daily);
        assert_eq!(tier_for_age(10 * 24 * 3600), GfsTier::Weekly);
        assert_eq!(tier_for_age(100 * 24 * 3600), GfsTier::Monthly);
        assert_eq!(tier_for_age(400 * 24 * 3600), GfsTier::Yearly);
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

    #[test]
    fn gfs_keeps_n_generations_per_tier() {
        let now = Utc::now();
        let hour = 3600;
        // 5 hourly generations, keep 2 → the 3 OLDEST (s5, s4, s3 — note s1
        // is the newest) disappear.
        let snaps: Vec<NamedSnapshot> = (1..=5)
            .map(|i| (format!("s{i}"), at(i * hour + 60, now)))
            .collect();
        let del = select_gfs_deletions(
            &snaps,
            &GfsPolicy {
                hourly: 2,
                ..Default::default()
            },
            now,
        );
        assert_eq!(
            names(del),
            vec!["s5".to_string(), "s4".to_string(), "s3".to_string()]
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
}
