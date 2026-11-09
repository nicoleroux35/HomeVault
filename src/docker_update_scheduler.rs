/* =============================================================================
 * File          : docker_update_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026
 * Last modified : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Opt-in scheduler for catalog app updates. It polls every five minutes and,
 *   when the saved interval is due, runs the same `update_all_pending` path as
 *   the admin "Update all" action. The first tick after enabling records a
 *   baseline instead of restarting containers immediately; a failed run still
 *   advances the clock so a broken Docker daemon is not hammered every tick.
 *
 * Dependencies:
 *   - tokio, chrono, serde
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; settings and manual runs go
 *   through src/handlers/docker_update_schedule.rs.
 * =============================================================================
 */

//! Opt-in scheduled Docker/catalog updates.

use crate::handlers::docker_catalog::CatalogUpdateReport;
use crate::handlers::AppState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 300;
const MIN_INTERVAL_HOURS: u32 = 6;
const MAX_INTERVAL_HOURS: u32 = 168;
const DEFAULT_INTERVAL_HOURS: u32 = 24;

/// Saved schedule plus the last outcome, shown on the Docker catalog card.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DockerUpdateSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval_hours: u32,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn default_interval() -> u32 {
    DEFAULT_INTERVAL_HOURS
}

impl Default for DockerUpdateSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: DEFAULT_INTERVAL_HOURS,
            last_run_at: None,
            last_result: None,
            last_error: None,
        }
    }
}

/// What one scheduler tick has to do. Pure so the "no surprise restart" and
/// "a failure still advances the clock" rules are testable without Docker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickPlan {
    Disabled,
    /// Enabled but never run: record a baseline and wait one full interval, so
    /// flipping the switch does not restart containers immediately.
    Baseline,
    Due,
    NotDue,
}

fn plan_tick(
    enabled: bool,
    last_run: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    interval_hours: u32,
) -> TickPlan {
    if !enabled {
        return TickPlan::Disabled;
    }
    match last_run {
        None => TickPlan::Baseline,
        Some(last) => {
            let interval = interval_hours.clamp(MIN_INTERVAL_HOURS, MAX_INTERVAL_HOURS) as i64;
            if now >= last + chrono::Duration::hours(interval) {
                TickPlan::Due
            } else {
                TickPlan::NotDue
            }
        }
    }
}

/// Settings file beside the database (`data/docker_updates.json`).
pub fn settings_path() -> PathBuf {
    crate::config::paths::data_dir().join("docker_updates.json")
}

/// Loads the schedule, normalizing an out-of-range interval. A missing or
/// malformed file degrades to "off", never to "update everything now".
pub fn load_settings() -> DockerUpdateSettings {
    let mut settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<DockerUpdateSettings>(&raw).ok())
        .unwrap_or_default();
    if !(MIN_INTERVAL_HOURS..=MAX_INTERVAL_HOURS).contains(&settings.interval_hours) {
        settings.interval_hours = DEFAULT_INTERVAL_HOURS;
    }
    settings
}

/// Writes the schedule atomically (temp + rename).
pub fn save_settings(settings: &DockerUpdateSettings) -> Result<(), String> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Validates the two editable fields. Returns the normalized interval.
pub fn validate_interval(hours: u32) -> Result<u32, String> {
    if (MIN_INTERVAL_HOURS..=MAX_INTERVAL_HOURS).contains(&hours) {
        Ok(hours)
    } else {
        Err(format!(
            "interval_hours must be between {} and {}",
            MIN_INTERVAL_HOURS, MAX_INTERVAL_HOURS
        ))
    }
}

/// Runs one update pass now and records the outcome. `actor` is `scheduler` or
/// the admin's name; the shared updater uses it only for the audit sentence.
pub async fn run_now(
    state: &Arc<AppState>,
    actor: &str,
) -> Result<CatalogUpdateReport, String> {
    if !crate::docker::DockerService::is_available() {
        return Err("Docker is not available on this host".to_string());
    }
    let report = crate::handlers::docker_catalog::update_all_pending(state, actor).await;

    let mut settings = load_settings();
    settings.last_run_at = Some(Utc::now().to_rfc3339());
    settings.last_result = Some(format!(
        "{} updated, {} failed",
        report.updated.len(),
        report.failed.len()
    ));
    settings.last_error = if report.failed.is_empty() {
        None
    } else {
        Some(format!("{} app(s) failed to update", report.failed.len()))
    };
    save_settings(&settings)?;
    Ok(report)
}

/// Runs forever, checking the saved interval every five minutes.
pub async fn run_scheduler_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = run_due(&state).await {
            tracing::warn!("Docker update scheduler tick failed: {}", e);
        }
    }
}

async fn run_due(state: &Arc<AppState>) -> Result<(), String> {
    let mut settings = load_settings();
    let now = Utc::now();
    let last_run = settings
        .last_run_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc));

    match plan_tick(
        settings.enabled,
        last_run,
        now,
        settings.interval_hours,
    ) {
        TickPlan::Disabled | TickPlan::NotDue => Ok(()),
        TickPlan::Baseline => {
            settings.last_run_at = Some(now.to_rfc3339());
            settings.last_result =
                Some("baseline set; the first scheduled update runs after one interval".to_string());
            save_settings(&settings)
        }
        TickPlan::Due => match run_now(state, "scheduler").await {
            Ok(_) => Ok(()),
            Err(e) => {
                // Advance the clock even on failure: a broken Docker daemon
                // must not be retried every five minutes.
                settings.last_run_at = Some(now.to_rfc3339());
                settings.last_error = Some(e.clone());
                let _ = save_settings(&settings);
                crate::events::log_event_warn(format!("Scheduled Docker update failed: {e}"));
                Err(e)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn test_plan_tick_disabled_never_runs() {
        assert_eq!(
            plan_tick(false, None, at("2026-11-09T10:00:00+00:00"), 24),
            TickPlan::Disabled
        );
        assert_eq!(
            plan_tick(
                false,
                Some(at("2026-11-01T10:00:00+00:00")),
                at("2026-11-09T10:00:00+00:00"),
                24
            ),
            TickPlan::Disabled
        );
    }

    #[test]
    fn test_plan_tick_first_enable_sets_a_baseline() {
        assert_eq!(
            plan_tick(true, None, at("2026-11-09T10:00:00+00:00"), 24),
            TickPlan::Baseline
        );
    }

    #[test]
    fn test_plan_tick_due_and_not_due() {
        let last = Some(at("2026-11-08T10:00:00+00:00"));
        assert_eq!(
            plan_tick(true, last, at("2026-11-09T09:59:59+00:00"), 24),
            TickPlan::NotDue
        );
        assert_eq!(
            plan_tick(true, last, at("2026-11-09T10:00:00+00:00"), 24),
            TickPlan::Due
        );
    }

    #[test]
    fn test_validate_interval_bounds() {
        assert!(validate_interval(6).is_ok());
        assert!(validate_interval(168).is_ok());
        assert!(validate_interval(5).is_err());
        assert!(validate_interval(169).is_err());
        assert!(validate_interval(0).is_err());
    }

    #[test]
    fn test_settings_default_is_off_and_conservative() {
        let settings = DockerUpdateSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.interval_hours, 24);
        assert!(settings.last_run_at.is_none());
    }
}
