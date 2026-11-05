/* =============================================================================
 * File          : health_report.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Weekly aggregate health report (tasklist P2-12). Every field is optional:
 *   a missing measurement is rendered as "unknown", never as "OK".
 *
 * Dependencies:
 *   - chrono, crate::storage::disk_health_repo, crate::pools
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; see docs/guides/health.md.
 * =============================================================================
 */

//! Weekly health aggregation and notification.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::handlers::AppState;
use crate::storage::atomic::AtomicWriter;
use crate::storage::disk_health_repo::DiskHealthRepository;

const STARTUP_DELAY: Duration = Duration::from_secs(15 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const WEEK_SECONDS: i64 = 7 * 24 * 60 * 60;

/// One week's numbers. `None` means "not measured", which must never be
/// rendered as a pass.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WeeklyReport {
    pub generated_at: String,
    /// Active disk findings, by severity.
    pub disk_findings: Option<usize>,
    pub critical_findings: Option<usize>,
    /// The newest sample timestamp across all disks.
    pub last_smart_sample: Option<String>,
    /// Free bytes per mounted pool.
    pub pool_free: Option<Vec<(String, u64)>>,
    /// The last scrub marker, when one is recorded.
    pub pending_scrub: Option<String>,
    /// Last `restic check` result, when one is recorded.
    pub last_restic_check: Option<String>,
}

fn state_path() -> std::path::PathBuf {
    crate::config::paths::data_dir()
        .join("state")
        .join("weekly-health-report")
}

/// Render the report as human-readable lines.
pub fn render(report: &WeeklyReport) -> String {
    let unknown = "unknown".to_string();
    let findings = report
        .disk_findings
        .map(|n| n.to_string())
        .unwrap_or_else(|| unknown.clone());
    let critical = report
        .critical_findings
        .map(|n| n.to_string())
        .unwrap_or_else(|| unknown.clone());
    let pools = report
        .pool_free
        .as_ref()
        .map(|pools| {
            pools
                .iter()
                .map(|(name, free)| {
                    format!("{name}: {:.1} GiB free", *free as f64 / 1024f64.powi(3))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| unknown.clone());
    format!(
        "Weekly HomeVault health report ({})\n\
         Disk findings: {findings} ({critical} critical)\n\
         Last SMART sample: {}\n\
         Pools: {pools}\n\
         Pending scrub: {}\n\
         Last restic check: {}",
        report.generated_at,
        report.last_smart_sample.as_deref().unwrap_or(&unknown),
        report.pending_scrub.as_deref().unwrap_or(&unknown),
        report.last_restic_check.as_deref().unwrap_or(&unknown),
    )
}

fn due(now: DateTime<Utc>, last: Option<DateTime<Utc>>) -> bool {
    match last {
        None => true,
        Some(last) => now.signed_duration_since(last).num_seconds() >= WEEK_SECONDS,
    }
}

fn last_sent() -> Option<DateTime<Utc>> {
    let raw = std::fs::read_to_string(state_path()).ok()?;
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

async fn collect(state: &Arc<AppState>) -> WeeklyReport {
    let repo = DiskHealthRepository::new(state.db.pool());
    let findings = repo.list_active_findings().await.unwrap_or_default();
    let critical = findings.iter().filter(|f| f.severity == "critical").count();

    let pool_free = tokio::task::spawn_blocking(crate::pools::PoolService::scan_pools)
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|pools| {
            pools
                .into_iter()
                .filter_map(|p| {
                    p.mount_point
                        .as_ref()
                        .map(|_| (p.name.clone(), p.available_bytes))
                })
                .collect::<Vec<_>>()
        });

    let pending_scrub =
        crate::scrub::load_interrupted().map(|s| format!("{} ({})", s.pool, s.kind.as_str()));

    let last_restic_check =
        crate::storage::scheduled_task_repo::ScheduledTaskRepository::new(state.db.pool())
            .last_run_for_type("restic_check")
            .await
            .ok()
            .flatten()
            .map(|run| {
                format!(
                    "{} ({})",
                    run.status,
                    run.finished_at.as_deref().unwrap_or(&run.started_at)
                )
            });

    WeeklyReport {
        generated_at: Utc::now().to_rfc3339(),
        disk_findings: Some(findings.len()),
        critical_findings: Some(critical),
        last_smart_sample: repo.last_sample_at_any().await.ok().flatten(),
        pool_free,
        pending_scrub,
        last_restic_check,
    }
}

/// Whether the weekly report is enabled. Default on; `0`/`false`/`no`/`off`
/// disables it without removing the scheduled task.
pub fn report_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

/// Generate and send one report if a week has elapsed. Returns whether it
/// sent anything.
pub async fn maybe_send(state: &Arc<AppState>) -> Result<bool, String> {
    if !report_enabled(
        std::env::var("HOMEVAULT_WEEKLY_HEALTH_REPORT")
            .ok()
            .as_deref(),
    ) {
        return Ok(false);
    }
    let now = Utc::now();
    if !due(now, last_sent()) {
        return Ok(false);
    }
    let report = collect(state).await;
    let message = render(&report);
    crate::notification_engine::notify(
        state,
        "health.storage",
        "Weekly health report",
        &message,
        "info",
        "health",
        "weekly_health_report",
    )
    .await;
    if let Some(parent) = state_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = AtomicWriter::write(&state_path(), &now.to_rfc3339());
    crate::events::log_event("Weekly health report sent");
    Ok(true)
}

pub async fn run_scheduler_loop(state: Arc<AppState>) {
    tokio::time::sleep(STARTUP_DELAY).await;
    loop {
        if let Err(e) = maybe_send(&state).await {
            tracing::warn!("Weekly health report failed: {e}");
        }
        tokio::time::sleep(CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_measurements_render_as_unknown_never_ok() {
        let report = WeeklyReport {
            generated_at: "2026-11-05T00:00:00Z".to_string(),
            ..Default::default()
        };
        let text = render(&report);
        assert!(text.contains("Disk findings: unknown (unknown critical)"));
        assert!(text.contains("Pools: unknown"));
        assert!(text.contains("Last SMART sample: unknown"));
        assert!(!text.contains(" OK"));
    }

    #[test]
    fn measured_values_are_rendered() {
        let report = WeeklyReport {
            generated_at: "2026-11-05T00:00:00Z".to_string(),
            disk_findings: Some(2),
            critical_findings: Some(1),
            last_smart_sample: Some("2026-11-04T00:00:00Z".to_string()),
            pool_free: Some(vec![("tank".to_string(), 100 * 1024 * 1024 * 1024)]),
            pending_scrub: None,
            last_restic_check: None,
        };
        let text = render(&report);
        assert!(text.contains("Disk findings: 2 (1 critical)"));
        assert!(text.contains("tank: 100.0 GiB free"));
    }

    #[test]
    fn a_report_is_due_every_seven_days() {
        let now = Utc::now();
        assert!(due(now, None));
        assert!(!due(now, Some(now - chrono::Duration::days(6))));
        assert!(due(now, Some(now - chrono::Duration::days(8))));
    }

    #[test]
    fn the_report_can_be_disabled_by_environment() {
        assert!(report_enabled(None));
        assert!(report_enabled(Some("1")));
        assert!(!report_enabled(Some("off")));
        assert!(!report_enabled(Some(" false ")));
    }
}
