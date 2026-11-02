/* =============================================================================
 * File          : trash_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background scheduler for the Explorer Trash retention policy: on its poll
 *   interval, permanently removes trashed items older than the configured
 *   number of days and, if the trash still exceeds its size ceiling, the
 *   oldest items until it fits.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs.
 * =============================================================================
 */

//! Background scheduler for the Explorer Trash retention policy.
//!
//! Mirrors `journal_cleanup_scheduler`: polls every `POLL_INTERVAL_SECS` and,
//! at most once per `PURGE_CADENCE_DAYS`, runs the retention pass over every
//! trash root. The pass itself lives in `crate::trash` so the Explorer's
//! "empty trash" and this automatic pass share one implementation.
//!
//! Two rules that matter here:
//! - A policy with both limits at 0 means "keep everything": the scheduler
//!   does nothing at all, including setting a baseline.
//! - **Only a successful pass advances `last_purge_at`**, so a disk-full or
//!   permission failure is retried on the next hourly tick instead of being
//!   postponed by a whole cadence.

use crate::browser::TrashPolicy;
use crate::storage::trash_policy_repo::TrashPolicyRepository;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 3600;

/// How often the retention pass is allowed to run.
const PURGE_CADENCE_DAYS: u32 = 1;

/// What one scheduler tick has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickPlan {
    /// Both limits are disabled — nothing to do, ever.
    Skipped,
    /// Enabled but never run: record a baseline now so the first pass lands a
    /// full cadence from now, rather than thrashing on every restart.
    Baseline,
    /// Due — purge, then advance the clock *on success only*.
    Due,
    /// The cadence has not elapsed yet.
    NotDue,
}

/// Decide what this tick must do. Pure, so the "both limits off" and "a
/// failure is not recorded as done" rules are testable without a database or
/// a filesystem.
fn plan_tick(
    policy: &TrashPolicy,
    last_purge: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    cadence_days: u32,
) -> TickPlan {
    if policy.retention_days == 0 && policy.max_bytes == 0 {
        return TickPlan::Skipped;
    }
    match last_purge {
        Some(last) => {
            if now >= last + chrono::Duration::days(cadence_days.max(1) as i64) {
                TickPlan::Due
            } else {
                TickPlan::NotDue
            }
        }
        None => TickPlan::Baseline,
    }
}

/// Runs forever, checking for a due retention pass every `POLL_INTERVAL_SECS`.
/// Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<crate::handlers::AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = run_due(&state).await {
            tracing::warn!("Trash retention scheduler tick failed: {}", e);
        }
    }
}

async fn run_due(state: &Arc<crate::handlers::AppState>) -> Result<(), String> {
    let repo = TrashPolicyRepository::new(state.db.pool());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;
    let policy = settings.policy();

    let now = Utc::now();
    let last_purge = settings
        .last_purge_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));

    match plan_tick(&policy, last_purge, now, PURGE_CADENCE_DAYS) {
        TickPlan::Skipped | TickPlan::NotDue => Ok(()),
        TickPlan::Baseline => repo
            .set_last_purge(&now.to_rfc3339())
            .await
            .map_err(|e| format!("Failed to record the retention baseline: {}", e)),
        TickPlan::Due => {
            let state = state.clone();
            let policy = policy;
            let result =
                tokio::task::spawn_blocking(move || crate::trash::purge_all(&state, &policy))
                    .await
                    .map_err(|e| e.to_string())?;

            tracing::info!(
                "Trash retention pass removed {} item(s), reclaimed {} byte(s)",
                result.purged,
                result.freed_bytes
            );
            // The clock only advances because the pass actually ran.
            repo.set_last_purge(&now.to_rfc3339())
                .await
                .map_err(|e| format!("Retention pass ran but could not be recorded: {}", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap()
    }

    fn policy(retention_days: u32, max_bytes: u64) -> TrashPolicy {
        TrashPolicy {
            retention_days,
            max_bytes,
        }
    }

    #[test]
    fn test_both_limits_disabled_skips_everything() {
        // "Keep forever" must never record a baseline or run a pass.
        let p = policy(0, 0);
        assert_eq!(plan_tick(&p, None, at(20), 1), TickPlan::Skipped);
        assert_eq!(plan_tick(&p, Some(at(1)), at(20), 1), TickPlan::Skipped);
    }

    #[test]
    fn test_first_run_establishes_a_baseline() {
        assert_eq!(
            plan_tick(&policy(30, 0), None, at(20), 1),
            TickPlan::Baseline
        );
    }

    #[test]
    fn test_due_exactly_at_the_cadence_boundary() {
        let p = policy(30, 0);
        assert_eq!(plan_tick(&p, Some(at(1)), at(2), 1), TickPlan::Due);
        assert_eq!(plan_tick(&p, Some(at(1)), at(1), 1), TickPlan::NotDue);
    }

    #[test]
    fn test_a_size_only_policy_still_runs() {
        // retention disabled but a size ceiling set must still trigger a pass.
        assert_eq!(
            plan_tick(&policy(0, 1024), None, at(20), 1),
            TickPlan::Baseline
        );
    }
}
