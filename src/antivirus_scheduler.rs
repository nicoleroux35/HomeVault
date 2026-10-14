/* =============================================================================
 * File          : antivirus_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background scheduler for scheduled antivirus scans: polls on a fixed
 *   interval and starts a scan when the configured cron is due (only when
 *   scheduled scanning is enabled and no scan is running). Kept separate
 *   from the scan service so a hung scan can never block the scheduler.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs.
 * =============================================================================
 */

//! Background scheduler for scheduled antivirus scans.
//!
//! Structural copy of `cache_scheduler`/`restic_scheduler`: polls every
//! `POLL_INTERVAL_SECS` and runs a scan when the configured cron schedule is
//! due (only if scheduled scanning is enabled and a scan isn't already
//! running). Kept as its own file rather than folded into another scheduler
//! so a heavy antivirus scan tick never blocks/is blocked by an unrelated
//! domain's tick (e.g. the cache mover).

use crate::antivirus::{self, av_scan_is_running};
use crate::restic_scheduler::compute_next_run;
use crate::storage::AntivirusRepository;
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 30;

/// Runs forever, checking for a due scheduled scan every `POLL_INTERVAL_SECS`.
/// Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<crate::handlers::AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = run_due(&state).await {
            tracing::warn!("Antivirus scheduler tick failed: {}", e);
        }
    }
}

async fn run_due(state: &Arc<crate::handlers::AppState>) -> Result<(), String> {
    if !antivirus::clamscan_available() || av_scan_is_running() {
        return Ok(());
    }

    let repo = AntivirusRepository::new(state.db.pool());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;

    if !settings.scheduled_scan_enabled {
        return Ok(());
    }

    let Some(scan_path) = settings.scheduled_scan_path.clone() else {
        return Ok(());
    };

    let now = Utc::now();
    let after = settings
        .last_scan_at
        .as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| now - chrono::Duration::days(8));

    let next = compute_next_run(&settings.scheduled_scan_cron, after)
        .ok()
        .flatten();

    let should_run = match next {
        Some(n) => n <= now,
        None => false,
    };

    if !should_run {
        return Ok(());
    }

    let job_id = antivirus::create_scan_job();
    let pool = state.db.pool();
    tokio::spawn(async move {
        if let Err(e) = antivirus::run_scan(pool, scan_path, "scheduled", Some(job_id)).await {
            tracing::warn!("Scheduled antivirus scan failed: {}", e);
        }
    });

    Ok(())
}
