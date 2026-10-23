/* =============================================================================
 * File          : journal_cleanup_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Background scheduler for the System Journal auto-cleanup: on its poll
 *   interval, deletes HomeVault's own rotating log files once the
 *   configured number of days have elapsed since the last successful
 *   cleanup.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs.
 * =============================================================================
 */

//! Background scheduler for the System Journal app's auto-cleanup setting.
//!
//! Mirrors `cache_scheduler`: polls every `POLL_INTERVAL_SECS` and, if
//! auto-cleanup is enabled in `journal_cleanup_settings`, deletes
//! HomeVault's own rotating log files once `interval_days` have elapsed
//! since the last cleanup (or sets a baseline, if it has never run before).
//!
//! The one rule that matters here: **only a successful cleanup advances
//! `last_cleanup_at`**. Recording a failed run as done — which this module
//! used to do — silently postpones the retry by a whole `interval_days`
//! (ROADMAP 20.3), so a disk-full or permission failure meant the logs were
//! simply never pruned again for the configured period.

use crate::handlers::journal::clear_app_logs;
use crate::storage::journal_cleanup_repo::JournalCleanupRepository;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 3600;

/// What one scheduler tick has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickPlan {
    /// Auto-cleanup is off — nothing to do, ever.
    Disabled,
    /// Enabled but never run. Do NOT clean now: that would silently wipe the
    /// current day's audit log on every server start (the first tick fires
    /// immediately, and this branch is reached on every boot because the
    /// settings row does not exist yet). Persist a baseline instead, so the
    /// first real cleanup lands `interval_days` from now.
    Baseline,
    /// Enabled, due — clean, then advance the clock *on success only*.
    Due,
    /// Enabled, but the interval has not elapsed yet.
    NotDue,
}

/// Decide what this tick must do. Pure, so the "a failure is not recorded as
/// done" rule is testable without a database or a log directory.
fn plan_tick(
    enabled: bool,
    last_cleanup: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    interval_days: u32,
) -> TickPlan {
    if !enabled {
        return TickPlan::Disabled;
    }
    match last_cleanup {
        // `interval_days` is clamped to >= 1 by the repo, so this can never
        // mean "due on every tick".
        Some(last) => {
            if now >= last + chrono::Duration::days(interval_days.max(1) as i64) {
                TickPlan::Due
            } else {
                TickPlan::NotDue
            }
        }
        None => TickPlan::Baseline,
    }
}

/// Runs forever, checking for a due cleanup every `POLL_INTERVAL_SECS`.
/// Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<crate::handlers::AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = run_due(&state).await {
            tracing::warn!("Journal cleanup scheduler tick failed: {}", e);
        }
    }
}

async fn run_due(state: &Arc<crate::handlers::AppState>) -> Result<(), String> {
    let repo = JournalCleanupRepository::new(state.db.pool());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;

    let now = Utc::now();
    let last_cleanup = settings
        .last_cleanup_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));

    match plan_tick(
        settings.auto_cleanup_enabled,
        last_cleanup,
        now,
        settings.interval_days,
    ) {
        TickPlan::Disabled | TickPlan::NotDue => Ok(()),
        TickPlan::Baseline => repo
            .set_last_cleanup(&now.to_rfc3339())
            .await
            .map_err(|e| format!("Failed to record the cleanup baseline: {}", e)),
        TickPlan::Due => {
            let result = tokio::task::spawn_blocking(clear_app_logs)
                .await
                .map_err(|e| e.to_string())?;

            match result {
                Ok(()) => {
                    tracing::info!(
                        "Scheduled journal cleanup completed (every {} days)",
                        settings.interval_days
                    );
                    // The clock only advances because the cleanup worked.
                    repo.set_last_cleanup(&now.to_rfc3339())
                        .await
                        .map_err(|e| {
                            format!("Cleanup succeeded but could not be recorded: {}", e)
                        })
                }
                Err(e) => {
                    // Nothing is recorded: the next tick (one hour from now)
                    // retries instead of waiting out `interval_days`.
                    tracing::error!(
                        "Scheduled journal cleanup failed, it will be retried on the next tick: {}",
                        e
                    );
                    Err(e)
                }
            }
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

    #[test]
    fn test_plan_tick_is_disabled_when_the_setting_is_off() {
        // Even a run that is years overdue must not fire while the admin has
        // switched auto-cleanup off.
        assert_eq!(plan_tick(false, Some(at(1)), at(20), 1), TickPlan::Disabled);
        assert_eq!(plan_tick(false, None, at(20), 1), TickPlan::Disabled);
    }

    #[test]
    fn test_plan_tick_establishes_a_baseline_instead_of_cleaning_a_first_run() {
        // The regression of 2026-09-07: a missing `last_cleanup_at` used to
        // mean "due", and the first tick fires at boot, so the day's audit
        // log was wiped on every restart.
        assert_eq!(plan_tick(true, None, at(20), 90), TickPlan::Baseline);
    }

    #[test]
    fn test_plan_tick_is_due_exactly_at_the_interval_boundary() {
        assert_eq!(plan_tick(true, Some(at(1)), at(10), 9), TickPlan::Due);
        assert_eq!(plan_tick(true, Some(at(1)), at(9), 9), TickPlan::NotDue);
        assert_eq!(plan_tick(true, Some(at(1)), at(11), 9), TickPlan::Due);
    }

    #[test]
    fn test_plan_tick_treats_a_zero_interval_as_one_day() {
        // The repo clamps the stored value, but a hand-edited row must not
        // turn into "clean on every tick": zero behaves as one day.
        assert_eq!(plan_tick(true, Some(at(10)), at(10), 0), TickPlan::NotDue);
        assert_eq!(plan_tick(true, Some(at(10)), at(11), 0), TickPlan::Due);
    }
}
