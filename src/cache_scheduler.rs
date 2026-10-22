/* =============================================================================
 * File          : cache_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background scheduler for the cache-tier mover: polls on a fixed
 *   interval and runs the mover when the configured cron schedule is due
 *   (only when the mover is enabled in the cache settings).
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs.
 * =============================================================================
 */

//! Background scheduler for the cache-tier mover.
//!
//! Mirrors `restic_scheduler`: polls every `POLL_INTERVAL_SECS` and runs the
//! mover when the configured cron schedule is due (only if the mover is
//! enabled in `cache_settings`).

use crate::cache::run_mover;
use crate::restic_scheduler::compute_next_run;
use crate::sse::ProgressEmitter;
use crate::storage::cache_repo::CacheRepository;
use chrono::Utc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL_INTERVAL_SECS: u64 = 30;

/// Minimum delay before retrying a scheduled run that failed.
///
/// A failure no longer advances `mover_last_run_at` (a failed run is not a run
/// — ROADMAP, "also in this pass"), but without a floor the 30 s poll would
/// retry immediately and forever. 15 minutes keeps a transient failure
/// (destination full, a pool briefly unmounted) recoverable without hammering
/// the disks.
const FAILED_RETRY_SECS: u64 = 900;

/// When the last *scheduled* attempt failed. Process-local on purpose: a
/// restart clears it, and the very next tick may retry.
static LAST_FAILED_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// Whether a retry is allowed given the last failure.
fn retry_allowed(last_failure: Option<Instant>, now: Instant, min_delay: Duration) -> bool {
    match last_failure {
        None => true,
        Some(at) => now.saturating_duration_since(at) >= min_delay,
    }
}

fn record_failure(at: Instant) {
    if let Ok(mut slot) = LAST_FAILED_AT.lock() {
        *slot = Some(at);
    }
}

fn clear_failure() {
    if let Ok(mut slot) = LAST_FAILED_AT.lock() {
        *slot = None;
    }
}

fn last_failure() -> Option<Instant> {
    LAST_FAILED_AT.lock().ok().and_then(|slot| *slot)
}

/// Runs forever, checking for a due mover schedule every `POLL_INTERVAL_SECS`.
/// Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<crate::handlers::AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = run_due(&state).await {
            tracing::warn!("Cache mover scheduler tick failed: {}", e);
        }
    }
}

async fn run_due(state: &Arc<crate::handlers::AppState>) -> Result<(), String> {
    let repo = CacheRepository::new(state.db.pool());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;

    if !settings.mover_enabled {
        return Ok(());
    }

    let now = Utc::now();
    let after = settings
        .mover_last_run_at
        .as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| now - chrono::Duration::days(2));

    let next = compute_next_run(&settings.mover_cron, after).ok().flatten();

    let should_run = match next {
        Some(n) => n <= now,
        // Couldn't parse the cron expression — don't spin forever.
        None => false,
    };

    if should_run {
        if crate::cache::mover_is_running() {
            // A manual run is already in flight; don't advance
            // `mover_last_run_at` for a run that never happened — retry on
            // the next poll tick instead of skipping until the next
            // scheduled occurrence.
            return Ok(());
        }

        // A previous scheduled attempt failed: wait out the backoff instead of
        // re-running every poll tick.
        if !retry_allowed(
            last_failure(),
            Instant::now(),
            Duration::from_secs(FAILED_RETRY_SECS),
        ) {
            return Ok(());
        }

        let job_id = ProgressEmitter::cache_mover();
        match run_mover(state.db.pool(), Some(job_id)).await {
            Ok((_id, summary)) => {
                tracing::info!(
                    "Scheduled cache mover completed: {} files, {} bytes",
                    summary.files_moved,
                    summary.bytes_moved
                );
                clear_failure();
                // Only a run that actually completed moves the schedule
                // cursor: a failed one must be retried, not skipped until the
                // next cron occurrence.
                let _ = repo.set_mover_last_run(&now.to_rfc3339()).await;
            }
            Err(e) => {
                tracing::error!("Scheduled cache mover failed: {}", e);
                record_failure(Instant::now());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_process_may_retry_immediately() {
        let now = Instant::now();
        assert!(retry_allowed(None, now, Duration::from_secs(900)));
    }

    #[test]
    fn a_recent_failure_holds_off_the_retry() {
        let now = Instant::now();
        let recent = now - Duration::from_secs(60);
        assert!(!retry_allowed(Some(recent), now, Duration::from_secs(900)));
    }

    #[test]
    fn the_retry_fires_once_the_backoff_elapsed() {
        let now = Instant::now();
        let old = now - Duration::from_secs(901);
        assert!(retry_allowed(Some(old), now, Duration::from_secs(900)));
    }

    #[test]
    fn failure_state_round_trips() {
        clear_failure();
        assert!(last_failure().is_none());
        let at = Instant::now();
        record_failure(at);
        assert!(last_failure().is_some());
        clear_failure();
        assert!(last_failure().is_none());
    }
}
