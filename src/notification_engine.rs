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

use crate::email::{EmailRecipient, EmailService, SystemAlertEmail};
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
    let notification = match repo
        .create(title, message, severity, category, source)
        .await
    {
        Ok(notification) => notification,
        Err(e) => {
            tracing::warn!("Notification engine: failed to persist notification: {}", e);
            return;
        }
    };

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

    // Web Push runs beside the other channels under the same threshold gate,
    // and in its own task: the sender encrypts once per device and talks to a
    // third party, which must never delay the scheduler tick.
    {
        let payload = crate::web_push::PushPayload::for_notification(
            title,
            message,
            severity,
            &notification.id,
        );
        let pool = state.db.pool();
        tokio::spawn(async move { crate::web_push::dispatch(pool, payload).await });
    }

    if settings.enable_alerts.unwrap_or(false) && settings.smtp_enabled.unwrap_or(false) {
        if let Some(email) = &settings.email {
            let config = crate::email::EmailConfig::from(&*EMAIL_SETTINGS.read());
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
            // ROADMAP 13.3: sending runs `msmtp`/`mutt`/`sendmail` with a 30 s
            // deadline. On the runtime thread that blocked the whole scheduler
            // tick (and every other task) while the MTA hung.
            let sent = tokio::task::spawn_blocking(move || {
                EmailService::send_system_alert(&alert, &[recipient], &config)
            })
            .await;
            match sent {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("Notification engine: failed to send alert email: {}", e)
                }
                Err(e) => tracing::warn!("Notification engine: alert email task failed: {}", e),
            }
        }
    }

    // SMS sits beside email and the webhook, under the same threshold gate: a
    // muted event returned earlier, and a severity below the threshold never
    // reaches any channel.
    if let Some(sms) = &settings.sms {
        dispatch_sms(sms, severity, title, message).await;
    }

    if settings.webhook_enabled.unwrap_or(false) {
        if let Some(url) = &settings.webhook_url {
            // ROADMAP 13.5: re-validate at dispatch. A URL persisted before
            // save-time validation existed (or hand-edited into the file) must
            // not reach `reqwest`; an empty value means "no webhook".
            let url = match crate::handlers::settings::validate_webhook_url(url) {
                Ok(url) if !url.is_empty() => url,
                Ok(_) => return,
                Err(e) => {
                    tracing::warn!("Notification engine: invalid webhook URL: {}", e);
                    return;
                }
            };
            // The sink format decides the body shape: the historical generic
            // JSON, or Slack/Discord's incoming-webhook fields so a channel
            // URL can be pasted without a relay service.
            let format = settings
                .webhook_format
                .as_deref()
                .and_then(crate::notification_events::WebhookFormat::parse)
                .unwrap_or_default();
            let payload = format.payload(title, message, severity, category);
            // One shared client, with a deadline: `Client::new()` per alert
            // built a fresh connection pool each time and had no timeout, so a
            // sink that accepted the TCP connection and never answered kept
            // this future alive forever.
            match webhook_client().post(&url).json(&payload).send().await {
                Ok(resp) if resp.status().is_success() => {}
                Ok(resp) => {
                    tracing::warn!("Notification engine: webhook answered {}", resp.status())
                }
                Err(e) => tracing::warn!("Notification engine: failed to post webhook: {}", e),
            }
        }
    }
}

/// Sends one alert over the SMS channel, when it is configured and enabled.
///
/// The recipient, the body template and the truncation are decided by
/// `crate::sms` — pure and unit-tested there — so this only resolves the sealed
/// token and performs the POST. A failure is logged and never propagated: an
/// unreachable SMS gateway must not stop the email or webhook channels, and it
/// must certainly not stop the health/backup scheduler tick.
pub(crate) async fn dispatch_sms(
    settings: &crate::sms::SmsSettings,
    severity: &str,
    title: &str,
    message: &str,
) {
    if !settings.enabled {
        return;
    }

    // An unseal that fails means the appliance key changed (a restored data
    // directory on another machine). Sending without the credential would
    // produce a 401 the admin cannot explain, so it is refused here with a
    // sentence that names the cause.
    let token = if settings.token_sealed.trim().is_empty() {
        None
    } else {
        match crate::storage::secret_box::open(crate::sms::SMS_TOKEN_SCOPE, &settings.token_sealed)
        {
            Ok(token) => Some(token),
            Err(e) => {
                tracing::warn!(
                    "Notification engine: the stored SMS token cannot be opened ({}). \
                     Re-enter it in Settings → Notifications.",
                    e
                );
                return;
            }
        }
    };

    let request =
        match crate::sms::build_request(settings, token.as_deref(), severity, title, message) {
            Ok(request) => request,
            Err(e) => {
                tracing::warn!("Notification engine: SMS not sent: {}", e.message());
                return;
            }
        };

    let mut builder = webhook_client().post(&request.endpoint).body(request.body);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    match builder.send().await {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => tracing::warn!(
            "Notification engine: SMS gateway answered {}",
            resp.status()
        ),
        Err(e) => tracing::warn!("Notification engine: failed to send SMS: {}", e),
    }
}

/// Maximum time a webhook delivery may take, including the connect, the body
/// write and the response headers.
pub const WEBHOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The one HTTP client used for outgoing webhooks.
///
/// Cached so every alert reuses its connection pool, and bounded so a
/// black-holed sink cannot pin the notification (and backup scheduling) task.
/// The cache is keyed on the outbound-proxy setting, so saving a new proxy in
/// Settings takes effect on the next alert instead of after a restart.
pub fn webhook_client() -> Arc<reqwest::Client> {
    type Cache = std::sync::Mutex<Option<(String, Arc<reqwest::Client>)>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();

    let key = crate::proxy::cache_key();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((cached_key, client)) = guard.as_ref() {
        if *cached_key == key {
            return client.clone();
        }
    }
    let client = Arc::new(
        crate::proxy::apply_loaded(
            reqwest::Client::builder()
                .timeout(WEBHOOK_TIMEOUT)
                .connect_timeout(std::time::Duration::from_secs(5))
                .user_agent(concat!("HomeVault/", env!("CARGO_PKG_VERSION"))),
        )
        .build()
        .unwrap_or_else(|_| reqwest::Client::new()),
    );
    *guard = Some((key, client.clone()));
    client
}

#[cfg(test)]
mod tests {
    //! ROADMAP 13.5: the webhook path was an untimed `Client::new()` per
    //! alert and an unvalidated URL. These pin the two properties that matter
    //! and are testable without a network.
    //!
    //! Run: `cargo test notification_engine::tests`

    use super::*;

    #[test]
    fn test_the_webhook_client_has_a_deadline() {
        assert!(
            WEBHOOK_TIMEOUT <= std::time::Duration::from_secs(30),
            "a webhook must not hold the scheduler task for half a minute"
        );
        // Same instance on every call: this is what makes the connection pool
        // and the timeout apply to every alert (until the proxy setting moves,
        // which deliberately invalidates the cache).
        assert!(Arc::ptr_eq(&webhook_client(), &webhook_client()));
    }

    #[test]
    fn test_a_bad_webhook_url_is_refused_before_dispatch() {
        for bad in ["javascript:alert(1)", "ftp://example.com/hook", "not a url"] {
            assert!(
                crate::handlers::settings::validate_webhook_url(bad).is_err(),
                "'{bad}' must not reach the webhook client"
            );
        }
        assert_eq!(
            crate::handlers::settings::validate_webhook_url("  https://example.com/hook  ")
                .unwrap(),
            "https://example.com/hook"
        );
        // Empty clears the setting rather than being an error.
        assert_eq!(
            crate::handlers::settings::validate_webhook_url("   ").unwrap(),
            ""
        );
    }
}
