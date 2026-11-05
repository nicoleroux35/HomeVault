/* =============================================================================
 * File          : disk_health_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Daily S.M.A.R.T. sampler. Reads the last successful sample timestamp from
 *   `data/state/disk-health-last-sample`, samples at most once per 20 hours,
 *   and retries every hour otherwise.
 *
 * Dependencies:
 *   - tokio, chrono, crate::disk_health
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs.
 * =============================================================================
 */

//! Daily disk-health sampler.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;

use crate::handlers::AppState;
use crate::storage::atomic::AtomicWriter;

const POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// A sample younger than this is reused, so restarts do not hammer smartctl.
const MIN_SAMPLE_AGE_HOURS: i64 = 20;
const STARTUP_DELAY: Duration = Duration::from_secs(60);

fn last_sample_path() -> std::path::PathBuf {
    crate::config::paths::data_dir()
        .join("state")
        .join("disk-health-last-sample")
}

fn sample_is_due(now: DateTime<Utc>) -> bool {
    sample_is_due_at(&last_sample_path(), now)
}

fn sample_is_due_at(path: &std::path::Path, now: DateTime<Utc>) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(last) = DateTime::parse_from_rfc3339(raw.trim()) else {
        return true;
    };
    now.signed_duration_since(last.with_timezone(&Utc))
        .num_hours()
        >= MIN_SAMPLE_AGE_HOURS
}

fn record_sample_time(now: DateTime<Utc>) {
    let path = last_sample_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = AtomicWriter::write(&path, &now.to_rfc3339());
}

pub async fn run_scheduler_loop(state: Arc<AppState>) {
    tokio::time::sleep(STARTUP_DELAY).await;
    loop {
        if sample_is_due(Utc::now()) {
            match crate::disk_health::sample_once(&state).await {
                Ok(report) => {
                    record_sample_time(Utc::now());
                    tracing::info!(
                        disks = report.sampled_disks,
                        attributes = report.sampled_attributes,
                        findings = report.findings,
                        "Disk-health sample completed"
                    );
                    if !report.errors.is_empty() {
                        tracing::warn!(
                            "Disk-health sample had {} issue(s): {}",
                            report.errors.len(),
                            report.errors.join("; ")
                        );
                    }
                }
                Err(e) => tracing::warn!("Disk-health sample failed: {e}"),
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    #[test]
    fn a_missing_or_malformed_timestamp_is_due() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        assert!(sample_is_due_at(&path, Utc::now()));

        std::fs::write(&path, "not a date").unwrap();
        assert!(sample_is_due_at(&path, Utc::now()));
    }

    #[test]
    fn a_recent_sample_is_reused_and_an_old_one_is_due() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last");
        let now = Utc::now();

        std::fs::write(&path, (now - ChronoDuration::hours(19)).to_rfc3339()).unwrap();
        assert!(!sample_is_due_at(&path, now));

        std::fs::write(&path, (now - ChronoDuration::hours(21)).to_rfc3339()).unwrap();
        assert!(sample_is_due_at(&path, now));
    }
}
