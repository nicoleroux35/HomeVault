/* =============================================================================
 * File          : journal_cleanup_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background scheduler for the System Journal auto-cleanup: on its poll
 *   interval, deletes HomeVault's own rotating log files once the
 *   configured number of days have elapsed since the last cleanup.
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
//! since the last cleanup (or immediately, if it has never run before).

use crate::handlers::journal::clear_app_logs;
use crate::storage::journal_cleanup_repo::JournalCleanupRepository;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 3600;

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

    if !settings.auto_cleanup_enabled {
        return Ok(());
    }

    let now = Utc::now();
    let last_cleanup = settings
        .last_cleanup_at
        .as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));

    let due = match last_cleanup {
        Some(last) => now >= last + chrono::Duration::days(settings.interval_days as i64),
        // Auto-cleanup is on but has never run. Do NOT clean now — that
        // would silently wipe the current day's audit log on every server
        // start (the scheduler's first tick fires immediately, and this
        // branch was reached on every boot because the settings row didn't
        // exist yet). Just persist a baseline so the first real cleanup
        // lands `interval_days` from now.
        None => {
            let _ = repo.set_last_cleanup(&now.to_rfc3339()).await;
            false
        }
    };

    if !due {
        return Ok(());
    }

    let result = tokio::task::spawn_blocking(clear_app_logs)
        .await
        .map_err(|e| e.to_string())?;

    match result {
        Ok(()) => {
            tracing::info!(
                "Scheduled journal cleanup completed (every {} days)",
                settings.interval_days
            );
        }
        Err(e) => {
            tracing::error!("Scheduled journal cleanup failed: {}", e);
        }
    }
    let _ = repo.set_last_cleanup(&now.to_rfc3339()).await;

    Ok(())
}
