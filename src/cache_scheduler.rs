//! Background scheduler for the cache-tier mover.
//!
//! Mirrors `restic_scheduler`: polls every `POLL_INTERVAL_SECS` and runs the
//! mover when the configured cron schedule is due (only if the mover is
//! enabled in `cache_settings`).

use crate::cache::run_mover;
use crate::sse::ProgressEmitter;
use crate::storage::cache_repo::CacheRepository;
use crate::restic_scheduler::compute_next_run;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 30;

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

    let next = compute_next_run(&settings.mover_cron, after)
        .ok()
        .flatten();

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

        let job_id = ProgressEmitter::cache_mover();
        match run_mover(state.db.pool(), Some(job_id)).await {
            Ok((_id, summary)) => {
                tracing::info!(
                    "Scheduled cache mover completed: {} files, {} bytes",
                    summary.files_moved,
                    summary.bytes_moved
                );
                let _ = repo.set_mover_last_run(&now.to_rfc3339()).await;
            }
            Err(e) => {
                tracing::error!("Scheduled cache mover failed: {}", e);
                let _ = repo.set_mover_last_run(&now.to_rfc3339()).await;
            }
        }
    }

    Ok(())
}
