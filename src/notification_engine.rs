/* =============================================================================
 * File          : notification_engine.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background loop turning health-check results and backup job outcomes
 *   into persisted notifications and, when configured, email/webhook
 *   alerts. Runs on a fixed interval (the same scheduler-loop shape as the
 *   other background workers) with per-rule severity thresholds.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; rules configured via the
 *   notification settings endpoints.
 * =============================================================================
 */

//! Background loop that turns existing health checks and backup job results
//! into persisted notifications (and, optionally, email/webhook alerts).
//!
//! Modeled directly on `src/restic_scheduler.rs`'s `run_scheduler_loop`
//! pattern (same `tokio::time::interval` + `tokio::spawn` shape). Backup
//! results are read via `ResticRepository::list_backup_jobs` — a read-only
//! poll — so `src/restic_scheduler.rs` itself (frozen, part of the Backup
//! page) is never touched.

use crate::email::{EmailConfig, EmailRecipient, EmailService, SystemAlertEmail};
use crate::handlers::settings::{EMAIL_SETTINGS, NOTIFICATION_SETTINGS};
use crate::handlers::AppState;
use crate::health::{HealthService, HealthStatus};
use crate::storage::restic_repo::ResticRepository;
use crate::storage::NotificationRepository;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const POLL_INTERVAL_SECS: u64 = 60;
/// App-update checks hit external image registries, so they run far less
/// often than the 60s health/backup poll.
const APP_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Tracks state across ticks so the same problem doesn't re-notify every
/// minute, and so only *new* backup jobs (not ones already seen) trigger a
/// notification.
struct EngineState {
    open_health_problems: HashSet<String>,
    last_seen_job_id: HashMap<String, String>,
    /// Catalog apps currently known to have an update pending — so the
    /// notification fires once when an update appears, not every check.
    open_app_updates: HashSet<String>,
    last_app_update_check: Option<Instant>,
}

/// Runs forever, checking health and backup job state every
/// `POLL_INTERVAL_SECS`. Intended to be `tokio::spawn`ed once at startup.
pub async fn run_notification_loop(state: Arc<AppState>) {
    let engine_state = Arc::new(Mutex::new(EngineState {
        open_health_problems: HashSet::new(),
        last_seen_job_id: HashMap::new(),
        open_app_updates: HashSet::new(),
        last_app_update_check: None,
    }));

    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        check_health(&state, &engine_state).await;
        check_backup_jobs(&state, &engine_state).await;
        check_app_updates(&state, &engine_state).await;
    }
}

/// Every `APP_UPDATE_CHECK_INTERVAL`, check installed catalog apps for
/// image/package updates and raise a notification the first time each one
/// appears.
async fn check_app_updates(state: &Arc<AppState>, engine_state: &Arc<Mutex<EngineState>>) {
    {
        let mut guard = engine_state.lock().await;
        let due = guard
            .last_app_update_check
            .map(|t| t.elapsed() >= APP_UPDATE_CHECK_INTERVAL)
            .unwrap_or(true);
        if !due {
            return;
        }
        guard.last_app_update_check = Some(Instant::now());
    }

    let stores: Vec<(String, String)> = crate::storage::AppStoreRepository::new(state.db.pool())
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id, s.name))
        .collect();

    let updates = crate::app_catalog::updates::check_installed_apps(&stores).await;
    let current: HashSet<String> = updates.iter().map(|u| u.id.clone()).collect();

    let mut to_notify = Vec::new();
    {
        let mut guard = engine_state.lock().await;
        for u in &updates {
            if !guard.open_app_updates.contains(&u.id) {
                to_notify.push((
                    u.name.clone(),
                    if u.package_update_available {
                        format!(
                            "A new version of {} is available (v{})",
                            u.name, u.available_version
                        )
                    } else {
                        format!("A newer image is available for {}", u.name)
                    },
                ));
            }
        }
        guard.open_app_updates = current;
    }

    for (name, message) in to_notify {
        notify(
            state,
            "app.updates",
            &format!("App update: {}", name),
            &message,
            "info",
            "docker",
            "app_update_engine",
        )
        .await;
    }
}

async fn check_health(state: &Arc<AppState>, engine_state: &Arc<Mutex<EngineState>>) {
    // `run_all_checks` offloads its subprocess work to `spawn_blocking`
    // internally — it must not be re-wrapped here.
    let pool = state.db.pool();
    let checks = match HealthService::run_all_checks(&pool).await {
        Ok(health) => health.checks,
        Err(e) => {
            tracing::warn!("Notification engine: health check run failed: {}", e);
            return;
        }
    };

    let mut current_problems = HashSet::new();
    let mut to_notify = Vec::new();

    for check in &checks {
        if matches!(
            check.status,
            HealthStatus::Warning | HealthStatus::Unhealthy
        ) {
            current_problems.insert(check.name.clone());
        }
    }

    {
        let mut guard = engine_state.lock().await;
        for check in &checks {
            let is_problem = matches!(
                check.status,
                HealthStatus::Warning | HealthStatus::Unhealthy
            );
            let was_open = guard.open_health_problems.contains(&check.name);
            if is_problem && !was_open {
                let severity = if check.status == HealthStatus::Unhealthy {
                    "error"
                } else {
                    "warning"
                };
                to_notify.push((
                    check.name.clone(),
                    check.message.clone(),
                    severity.to_string(),
                ));
            } else if !is_problem && was_open {
                to_notify.push((
                    check.name.clone(),
                    format!("{}: resolved", check.name),
                    "info".to_string(),
                ));
            }
        }
        guard.open_health_problems = current_problems;
    }

    for (name, message, severity) in to_notify {
        notify(
            state,
            &crate::notification_events::health_event_id(&name),
            &format!("Health: {}", name),
            &message,
            &severity,
            "health",
            "health_engine",
        )
        .await;
    }
}

async fn check_backup_jobs(state: &Arc<AppState>, engine_state: &Arc<Mutex<EngineState>>) {
    let repo_repo = ResticRepository::new(state.db.pool());
    let repos = match repo_repo.list_all().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Notification engine: failed to list restic repos: {}", e);
            return;
        }
    };

    for repo in repos {
        let jobs = match repo_repo.list_backup_jobs(&repo.id, 5).await {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                    "Notification engine: failed to list backup jobs for {}: {}",
                    repo.name,
                    e
                );
                continue;
            }
        };

        let Some(latest) = jobs.first() else { continue };
        if latest.status == "running" {
            continue;
        }

        let mut guard = engine_state.lock().await;
        let already_seen = guard.last_seen_job_id.get(&repo.id) == Some(&latest.id);
        if already_seen {
            continue;
        }
        guard
            .last_seen_job_id
            .insert(repo.id.clone(), latest.id.clone());
        drop(guard);

        let severity = if latest.status == "failed" {
            "error"
        } else {
            "info"
        };
        let message = if latest.status == "failed" {
            format!("Backup '{}' failed", repo.name)
        } else {
            format!("Backup '{}' completed", repo.name)
        };
        notify(
            state,
            "backup.jobs",
            &format!("Backup: {}", repo.name),
            &message,
            severity,
            "backup",
            "backup_engine",
        )
        .await;
    }
}

/// Persists a notification and, if configured, dispatches it via email
/// and/or webhook. `event_id` is the per-event toggle the admin can mute
/// (see `notification_events::EVENT_CATALOG`) — a muted event is not
/// persisted nor dispatched at all.
///
/// `pub(crate)` so other background engines (e.g. the snapshot scheduler)
/// can raise events through the same mute/persist/dispatch path instead of
/// writing notification rows directly.
pub(crate) async fn notify(
    state: &Arc<AppState>,
    event_id: &str,
    title: &str,
    message: &str,
    severity: &str,
    category: &str,
    source: &str,
) {
    let settings_snapshot = NOTIFICATION_SETTINGS.read().clone();
    if !crate::notification_events::is_event_enabled(
        event_id,
        settings_snapshot.event_toggles.as_ref(),
    ) {
        tracing::info!("Notification event '{}' is muted; skipping", event_id);
        return;
    }

    let repo = NotificationRepository::new(state.db.pool());
    if let Err(e) = repo
        .create(title, message, severity, category, source)
        .await
    {
        tracing::warn!("Notification engine: failed to persist notification: {}", e);
        return;
    }

    let settings = settings_snapshot;
    let severity_rank = match severity {
        "critical" => 3,
        "error" => 2,
        "warning" => 1,
        _ => 0,
    };
    let threshold_rank = match settings.alert_threshold.unwrap_or(0) {
        t if t >= 75 => 3,
        t if t >= 50 => 2,
        t if t >= 25 => 1,
        _ => 0,
    };
    if severity_rank < threshold_rank {
        return;
    }

    if settings.enable_alerts.unwrap_or(false) && settings.smtp_enabled.unwrap_or(false) {
        if let Some(email) = &settings.email {
            let email_settings = EMAIL_SETTINGS.read().clone();
            let config = EmailConfig {
                smtp_host: email_settings.smtp_server.clone().unwrap_or_default(),
                smtp_port: email_settings.smtp_port.unwrap_or(587),
                smtp_username: email_settings.smtp_username.clone().unwrap_or_default(),
                smtp_password: email_settings.smtp_password.clone().unwrap_or_default(),
                from_address: email_settings
                    .from_address
                    .clone()
                    .unwrap_or_else(|| "homevault@localhost".to_string()),
                from_name: "HomeVault".to_string(),
                use_tls: email_settings.use_tls.unwrap_or(true),
            };
            let alert = SystemAlertEmail {
                severity: severity.to_string(),
                title: title.to_string(),
                message: message.to_string(),
                details: None,
            };
            let recipient = EmailRecipient {
                email: email.clone(),
                name: None,
            };
            if let Err(e) = EmailService::send_system_alert(&alert, &[recipient], &config) {
                tracing::warn!("Notification engine: failed to send alert email: {}", e);
            }
        }
    }

    if settings.webhook_enabled.unwrap_or(false) {
        if let Some(url) = &settings.webhook_url {
            // The sink format decides the body shape: the historical generic
            // JSON, or Slack/Discord's incoming-webhook fields so a channel
            // URL can be pasted without a relay service.
            let format = settings
                .webhook_format
                .as_deref()
                .and_then(crate::notification_events::WebhookFormat::parse)
                .unwrap_or_default();
            let payload = format.payload(title, message, severity, category);
            let client = reqwest::Client::new();
            if let Err(e) = client.post(url).json(&payload).send().await {
                tracing::warn!("Notification engine: failed to post webhook: {}", e);
            }
        }
    }
}
