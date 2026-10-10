use crate::i18n::{t, I18n};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::AppState;

/// Query parameters for the translations endpoint.
#[derive(Debug, Deserialize)]
pub struct TranslationsQuery {
    pub lang: Option<String>,
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const VERSION_CHECK_URL: &str = "https://api.github.com/repos/homevault/homevault/releases/latest";

/// How often to check for application updates.
///
/// Default is `Manual`: HomeVault makes **no** outbound call to GitHub until
/// the admin either presses "Check now" or opts into a recurring schedule.
/// Contacting a third party on a timer without consent is a privacy decision
/// (CLAUDE.md compliance baseline), so it is off by default.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateFrequency {
    #[default]
    Manual,
    Daily,
    Weekly,
    Monthly,
    Disabled,
}

impl UpdateFrequency {
    fn as_seconds(&self) -> Option<u64> {
        match self {
            UpdateFrequency::Manual => None,
            UpdateFrequency::Daily => Some(86400),
            UpdateFrequency::Weekly => Some(604800),
            UpdateFrequency::Monthly => Some(2592000),
            UpdateFrequency::Disabled => None,
        }
    }
}

fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 3 {
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts[2].parse().ok()?;
        Some((major, minor, patch))
    } else {
        None
    }
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    let latest_parsed = parse_version(latest);
    let current_parsed = parse_version(current);

    match (latest_parsed, current_parsed) {
        (Some((lm, ln, lp)), Some((cm, cn, cp))) => (lm, ln, lp) > (cm, cn, cp),
        _ => false,
    }
}

/// Body for setting the user's language preference.
#[derive(Debug, Deserialize)]
pub struct LanguageRequest {
    pub language: String,
}

#[derive(Debug, Serialize)]
struct LanguageResponse {
    pub language: String,
    pub message: String,
}

/// General server settings (name, timezone).
#[derive(Debug, Deserialize)]
pub struct GeneralSettings {
    pub server_name: Option<String>,
    pub timezone: Option<String>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            server_name: Some("HomeVault".to_string()),
            timezone: Some("UTC".to_string()),
        }
    }
}

/// Sanitizes a display name into a valid Linux hostname label: lowercase
/// ASCII letters/digits/hyphens only, no leading/trailing/repeated hyphen,
/// max 63 chars (the RFC 1123 label limit). Falls back to "homevault" if
/// nothing valid is left (e.g. a name made entirely of emoji).
fn sanitize_hostname(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let mut collapsed = String::with_capacity(mapped.len());
    let mut last_was_hyphen = false;
    for c in mapped.chars() {
        if c == '-' {
            if !last_was_hyphen {
                collapsed.push(c);
            }
            last_was_hyphen = true;
        } else {
            collapsed.push(c);
            last_was_hyphen = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    let truncated: String = trimmed.chars().take(63).collect();
    let truncated = truncated.trim_matches('-');
    if truncated.is_empty() {
        "homevault".to_string()
    } else {
        truncated.to_string()
    }
}

/// Applies the sanitized name as the real OS hostname via `hostnamectl`
/// (needs root - `homevault` has a scoped sudoers grant for exactly
/// `hostnamectl set-hostname *`, see `SetupService::configure_sudoers`).
/// This updates `/etc/hostname` and the live kernel hostname immediately,
/// but does not itself restart any already-running service that cached the
/// old name (Samba, mDNS, this very process's own already-open log file,
/// etc.) - callers should surface a restart-recommended notice.
fn apply_system_hostname(new_hostname: &str) -> Result<(), String> {
    let output = std::process::Command::new("sudo")
        .args(["hostnamectl", "set-hostname", new_hostname])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// SMTP email settings for outgoing notifications.
#[derive(Debug, Deserialize, Clone)]
pub struct EmailSettings {
    pub smtp_server: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub from_address: Option<String>,
    pub use_tls: Option<bool>,
}

impl Default for EmailSettings {
    fn default() -> Self {
        Self {
            smtp_server: None,
            smtp_port: Some(587),
            smtp_username: None,
            smtp_password: None,
            from_address: None,
            use_tls: Some(true),
        }
    }
}

/// Settings for automatic update checks.
#[derive(Debug, Deserialize)]
pub struct UpdateSettings {
    pub check_frequency: Option<UpdateFrequency>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_frequency: Some(UpdateFrequency::default()),
        }
    }
}

/// In-app notification settings (email, thresholds).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NotificationSettings {
    pub email: Option<String>,
    pub enable_alerts: Option<bool>,
    pub alert_threshold: Option<u8>,
    pub webhook_url: Option<String>,
    pub webhook_enabled: Option<bool>,
    pub smtp_enabled: Option<bool>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            email: None,
            enable_alerts: Some(true),
            alert_threshold: Some(80),
            webhook_url: None,
            webhook_enabled: Some(false),
            smtp_enabled: Some(false),
        }
    }
}

/// The shipped accent — `--hv-primary` ("HomeVault Blue"). The only palette
/// blue that clears WCAG 1.4.3 in *both* directions: white on it (6.15:1,
/// button fills) and it as text on `--hv-surface` and the `--hv-primary-*`
/// tints (4.99–5.68:1). The legacy default `#4a90d9` failed both (≤3.34:1).
pub const DEFAULT_ACCENT: &str = "#1E62AE";

/// UI appearance settings (theme, wallpaper, accent color, density).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AppearanceSettings {
    pub theme: Option<String>,
    pub wallpaper: Option<String>,
    pub accent_color: Option<String>,
    pub density: Option<String>,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: Some("auto".to_string()),
            wallpaper: Some(String::new()),
            accent_color: Some(DEFAULT_ACCENT.to_string()),
            density: Some("comfortable".to_string()),
        }
    }
}

/// Rewrite the legacy non-compliant default accent on every path that can
/// introduce it (shipped default, persisted load, import,
/// `PUT /settings/appearance`). `#4a90d9` fails WCAG 1.4.3 in both
/// directions — 3.34:1 behind white button text, ≤3.34:1 as text on the
/// light surfaces — while every other value is returned untouched (the
/// accent has no editing UI, so nothing an operator chose is overridden).
pub fn normalize_accent(accent: &str) -> String {
    if accent.eq_ignore_ascii_case("#4a90d9") {
        DEFAULT_ACCENT.to_string()
    } else {
        accent.to_string()
    }
}

/// The result of a version check against the GitHub releases API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub up_to_date: bool,
    pub release_date: Option<String>,
    pub release_notes: Option<String>,
    pub error: Option<String>,
}

// Public settings accessors for external modules
pub static GENERAL_SETTINGS: Lazy<RwLock<GeneralSettings>> =
    Lazy::new(|| RwLock::new(GeneralSettings::default()));

pub static EMAIL_SETTINGS: Lazy<RwLock<EmailSettings>> =
    Lazy::new(|| RwLock::new(EmailSettings::default()));

pub static UPDATE_SETTINGS: Lazy<RwLock<UpdateSettings>> =
    Lazy::new(|| RwLock::new(UpdateSettings::default()));

pub static NOTIFICATION_SETTINGS: Lazy<RwLock<NotificationSettings>> =
    Lazy::new(|| RwLock::new(NotificationSettings::default()));

pub static APPEARANCE_SETTINGS: Lazy<RwLock<AppearanceSettings>> =
    Lazy::new(|| RwLock::new(AppearanceSettings::default()));

pub static LAST_UPDATE_CHECK: Lazy<RwLock<Option<u64>>> = Lazy::new(|| RwLock::new(None));

pub static CACHED_UPDATE_RESULT: Lazy<RwLock<Option<UpdateCheckResult>>> =
    Lazy::new(|| RwLock::new(None));

/// Updates the server's default UI language (shown on the login page and used
/// as the fallback locale for backend-generated messages). This is server-wide
/// state, not a per-browser preference — each user's own display language is
/// tracked client-side (`hv-language` in localStorage) and never touches this
/// endpoint, so only admins may change the shared default here.
pub async fn handle_set_language(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<LanguageRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let language = &req.language;

    let valid_languages = ["en", "fr", "es", "de"];
    if !valid_languages.contains(&language.as_str()) {
        return ResponseHelper::json_error(format!(
            "Invalid language '{}'. Supported: {:?}",
            language, valid_languages
        ));
    }

    I18n::set_locale(language);
    tracing::info!("Language set to: {}", language);

    ResponseHelper::json_success(LanguageResponse {
        language: language.clone(),
        message: t("settings.settings_saved"),
    })
}

/// Gets the current language setting.
pub async fn handle_get_language(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(_) => {}
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }

    let current_lang = I18n::get_locale();
    ResponseHelper::json_success(serde_json::json!({
        "language": current_lang
    }))
}

/// Serves the flattened translation map for a given locale so the SPA can
/// localize its interface. Falls back to English for missing keys.
pub async fn handle_get_translations(
    Query(params): Query<TranslationsQuery>,
) -> Json<HashMap<String, String>> {
    let locale = params
        .lang
        .filter(|l| ["en", "fr", "es", "de"].contains(&l.as_str()))
        .unwrap_or_else(I18n::get_locale);

    Json(I18n::get_translations(&locale))
}

/// Updates general settings.
pub async fn handle_general_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<GeneralSettings>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let mut settings = GENERAL_SETTINGS.write();
    if let Some(server_name) = &req.server_name {
        settings.server_name = Some(server_name.clone());
    }
    if let Some(timezone) = &req.timezone {
        settings.timezone = Some(timezone.clone());
    }
    drop(settings);

    // The server name was previously a pure UI label with no connection to
    // the real OS hostname. If it changed, try to actually rename the host
    // too - best effort: a failure here (e.g. no sudo/hostnamectl in a dev
    // sandbox) must not block saving the display name itself.
    let mut restart_required = false;
    if let Some(server_name) = &req.server_name {
        let target = sanitize_hostname(server_name);
        let current = hostname::get()
            .map(|h| h.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if target != current {
            match apply_system_hostname(&target) {
                Ok(()) => {
                    restart_required = true;
                    crate::events::log_event(format!(
                        "Admin '{}' changed the server hostname to '{}'",
                        session.username, target
                    ));
                }
                Err(err) => {
                    crate::events::log_event_warn(format!(
                        "Admin '{}' saved a new server name but changing the OS hostname to '{}' failed: {}",
                        session.username, target, err
                    ));
                }
            }
        }
    }

    // Persist settings
    crate::settings_persistence::save_all_settings();

    crate::events::log_event(format!(
        "Admin '{}' saved General settings",
        session.username
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": t("settings.settings_saved"),
        "restart_required": restart_required,
        "hostname_notice": if restart_required { Some(t("settings.hostname_restart_notice")) } else { None },
    }))
}

/// Gets general settings. Admin-only, matching every sibling settings GET
/// handler in this file (email/appearance/notification/update/all) - this
/// one and `handle_autostart_get` were previously the only two that merely
/// checked for a valid session rather than an admin one.
pub async fn handle_general_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let settings = GENERAL_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "server_name": settings.server_name,
        "timezone": settings.timezone,
    }))
}

/// Body for enabling/disabling start-on-boot.
#[derive(Debug, Deserialize)]
pub struct AutostartRequest {
    pub enabled: bool,
}

/// Reports whether the `homevault.service` systemd unit is currently
/// enabled to start on boot. Admin-only, same auth bar as every other
/// settings GET handler in this file (see `handle_general_settings_get`'s
/// own doc comment for why this changed).
pub async fn handle_autostart_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    ResponseHelper::json_success(serde_json::json!({
        "enabled": crate::autostart::is_enabled()
    }))
}

/// Enables or disables start-on-boot for the HomeVault service. Admin-only,
/// since this changes real host systemd state.
pub async fn handle_autostart_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<AutostartRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    match crate::autostart::set_enabled(req.enabled) {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' {} HomeVault start-on-boot",
                session.username,
                if req.enabled { "enabled" } else { "disabled" }
            ));
            ResponseHelper::json_success(serde_json::json!({
                "enabled": req.enabled,
                "message": t("settings.autostart_updated")
            }))
        }
        Err(err) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' failed to {} HomeVault start-on-boot: {}",
                session.username,
                if req.enabled { "enable" } else { "disable" },
                err
            ));
            ResponseHelper::json_error(t("settings.autostart_update_failed"))
        }
    }
}

/// Updates email settings.
pub async fn handle_email_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<EmailSettings>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let mut settings = EMAIL_SETTINGS.write();
    if let Some(smtp_server) = &req.smtp_server {
        settings.smtp_server = Some(smtp_server.clone());
    }
    if let Some(smtp_port) = req.smtp_port {
        settings.smtp_port = Some(smtp_port);
    }
    if let Some(smtp_username) = &req.smtp_username {
        settings.smtp_username = Some(smtp_username.clone());
    }
    if let Some(smtp_password) = &req.smtp_password {
        settings.smtp_password = Some(smtp_password.clone());
    }
    if let Some(from_address) = &req.from_address {
        settings.from_address = Some(from_address.clone());
    }
    if let Some(use_tls) = req.use_tls {
        settings.use_tls = Some(use_tls);
    }
    drop(settings);

    // Persist settings
    crate::settings_persistence::save_all_settings();

    crate::events::log_event(format!("Admin '{}' saved Email settings", session.username));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": t("settings.settings_saved")
    }))
}

/// Gets email settings (without password).
pub async fn handle_email_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let settings = EMAIL_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "smtp_server": settings.smtp_server,
        "smtp_port": settings.smtp_port,
        "smtp_username": settings.smtp_username,
        "from_address": settings.from_address,
        "use_tls": settings.use_tls,
    }))
}

/// Sends a test email.
pub async fn handle_email_test(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let settings = EMAIL_SETTINGS.read();

    if settings.smtp_server.is_none() || settings.smtp_username.is_none() {
        return ResponseHelper::json_error("Email settings not configured".to_string());
    }

    let config = crate::email::EmailConfig {
        smtp_host: settings.smtp_server.clone().unwrap_or_default(),
        smtp_port: settings.smtp_port.unwrap_or(587),
        smtp_username: settings.smtp_username.clone().unwrap_or_default(),
        smtp_password: settings.smtp_password.clone().unwrap_or_default(),
        from_address: settings
            .from_address
            .clone()
            .unwrap_or_else(|| "homevault@localhost".to_string()),
        from_name: "HomeVault".to_string(),
        use_tls: settings.use_tls.unwrap_or(true),
    };
    drop(settings);

    let recipient_email = NOTIFICATION_SETTINGS.read().email.clone();
    let Some(recipient_email) = recipient_email else {
        return ResponseHelper::json_error(
            "No notification email address configured. Set one under Notifications first."
                .to_string(),
        );
    };

    let recipient = crate::email::EmailRecipient {
        email: recipient_email,
        name: None,
    };

    match crate::email::EmailService::send_test_email(&recipient, &config) {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({
            "success": true,
            "message": t("settings.test_sent")
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to send test email: {}", e)),
    }
}

/// Updates update settings.
pub async fn handle_update_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateSettings>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let mut settings = UPDATE_SETTINGS.write();
    if let Some(frequency) = &req.check_frequency {
        settings.check_frequency = Some(*frequency);
    }
    drop(settings);

    // Persist settings
    crate::settings_persistence::save_all_settings();

    crate::events::log_event(format!(
        "Admin '{}' saved Update settings",
        session.username
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": t("settings.settings_saved")
    }))
}

/// Gets update settings.
pub async fn handle_update_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let settings = UPDATE_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "check_frequency": settings.check_frequency,
    }))
}

/// Performs a version check on GitHub API.
async fn fetch_latest_version() -> Result<UpdateCheckResult, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("HomeVault/Server")
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(VERSION_CHECK_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v'))
        .unwrap_or(CURRENT_VERSION)
        .to_string();

    let up_to_date = !is_newer_version(&tag, CURRENT_VERSION);

    let release_date = json
        .get("published_at")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .map(|s| s.lines().take(20).collect::<Vec<_>>().join("\n"));

    Ok(UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: Some(tag),
        up_to_date,
        release_date,
        release_notes: body,
        error: None,
    })
}

/// Checks if an automatic update check should be performed.
pub fn should_check_for_updates() -> bool {
    let settings = UPDATE_SETTINGS.read();

    if settings.check_frequency == Some(UpdateFrequency::Disabled) {
        return false;
    }

    let last_check = LAST_UPDATE_CHECK.read();
    let Some(frequency) = &settings.check_frequency else {
        return true;
    };

    let Some(interval) = frequency.as_seconds() else {
        return false;
    };

    match *last_check {
        None => true,
        Some(ts) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            now - ts >= interval
        }
    }
}

/// Gets version info and checks for updates.
pub async fn handle_version_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(_) => {}
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    }

    let mut result = UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: None,
        up_to_date: true,
        release_date: None,
        release_notes: None,
        error: None,
    };

    match fetch_latest_version().await {
        Ok(fetched) => {
            result = fetched;

            *LAST_UPDATE_CHECK.write() = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            );
            *CACHED_UPDATE_RESULT.write() = Some(result.clone());
        }
        Err(e) => {
            tracing::warn!("Version check failed: {}", e);
            result.error = Some("Could not check for updates".to_string());
        }
    }

    ResponseHelper::json_success(&result)
}

/// Updates notification settings.
pub async fn handle_notification_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<NotificationSettings>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let mut settings = NOTIFICATION_SETTINGS.write();
    if let Some(email) = &req.email {
        settings.email = Some(email.clone());
    }
    if let Some(enable) = req.enable_alerts {
        settings.enable_alerts = Some(enable);
    }
    if let Some(threshold) = req.alert_threshold {
        settings.alert_threshold = Some(threshold.clamp(0, 100));
    }
    if let Some(smtp) = req.smtp_enabled {
        settings.smtp_enabled = Some(smtp);
    }
    if let Some(webhook_url) = &req.webhook_url {
        settings.webhook_url = Some(webhook_url.clone());
    }
    if let Some(webhook_enabled) = req.webhook_enabled {
        settings.webhook_enabled = Some(webhook_enabled);
    }
    drop(settings);

    // Persist settings
    crate::settings_persistence::save_all_settings();

    crate::events::log_event(format!(
        "Admin '{}' saved Notification settings",
        session.username
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": t("settings.settings_saved")
    }))
}

/// Gets notification settings.
pub async fn handle_notification_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let settings = NOTIFICATION_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "email": settings.email,
        "enable_alerts": settings.enable_alerts,
        "alert_threshold": settings.alert_threshold,
        "smtp_enabled": settings.smtp_enabled,
        "webhook_url": settings.webhook_url,
        "webhook_enabled": settings.webhook_enabled,
    }))
}

/// Updates appearance settings.
pub async fn handle_appearance_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<AppearanceSettings>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let mut settings = APPEARANCE_SETTINGS.write();
    if let Some(theme) = &req.theme {
        settings.theme = Some(theme.clone());
    }
    if let Some(wallpaper) = &req.wallpaper {
        settings.wallpaper = Some(wallpaper.clone());
    }
    if let Some(accent) = &req.accent_color {
        settings.accent_color = Some(normalize_accent(accent));
    }
    if let Some(density) = &req.density {
        settings.density = Some(density.clone());
    }
    drop(settings);

    // Persist settings
    crate::settings_persistence::save_all_settings();

    crate::events::log_event(format!(
        "Admin '{}' saved Appearance settings",
        session.username
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": t("settings.settings_saved")
    }))
}

/// Gets appearance settings.
pub async fn handle_appearance_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let settings = APPEARANCE_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "theme": settings.theme,
        "wallpaper": settings.wallpaper,
        "accent_color": settings.accent_color,
        "density": settings.density,
    }))
}

/// Returns all settings aggregated into a single response.
pub async fn handle_all_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let general = GENERAL_SETTINGS.read();
    let email = EMAIL_SETTINGS.read();
    let update = UPDATE_SETTINGS.read();
    let notification = NOTIFICATION_SETTINGS.read();
    let appearance = APPEARANCE_SETTINGS.read();

    ResponseHelper::json_success(serde_json::json!({
        "general": {
            "server_name": general.server_name,
            "timezone": general.timezone,
            "language": I18n::get_locale(),
        },
        "email": {
            "smtp_server": email.smtp_server,
            "smtp_port": email.smtp_port,
            "smtp_username": email.smtp_username,
            "from_address": email.from_address,
            "use_tls": email.use_tls,
        },
        "update": {
            "check_frequency": update.check_frequency,
        },
        "notifications": {
            "email": notification.email,
            "enable_alerts": notification.enable_alerts,
            "alert_threshold": notification.alert_threshold,
            "webhook_url": notification.webhook_url,
            "webhook_enabled": notification.webhook_enabled,
            "smtp_enabled": notification.smtp_enabled,
        },
        "appearance": {
            "theme": appearance.theme,
            "wallpaper": appearance.wallpaper,
            "accent_color": appearance.accent_color,
            "density": appearance.density,
        },
    }))
}

/// Returns public-safe settings (no auth required) — used for login page preferences.
pub async fn handle_public_settings_get() -> Response {
    let general = GENERAL_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "server_name": general.server_name.clone().unwrap_or_else(|| "HomeVault".to_string()),
        "language": I18n::get_locale(),
        "version": CURRENT_VERSION,
    }))
}

use crate::settings_persistence::{export_settings, import_settings, reset_settings};

/// Export all settings as JSON
pub async fn handle_settings_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    match export_settings() {
        Some(json) => {
            crate::events::log_event(format!("Admin '{}' exported settings", session.username));
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "data": serde_json::from_str::<serde_json::Value>(&json).unwrap_or(serde_json::json!({})),
            }))
        }
        None => ResponseHelper::json_error("Failed to export settings".to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ImportSettingsRequest {
    pub data: serde_json::Value,
}

/// Import settings from JSON
pub async fn handle_settings_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<ImportSettingsRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    let json_result = serde_json::to_string(&req.data);
    let json = match json_result {
        Ok(j) => j,
        Err(e) => return ResponseHelper::json_error(format!("JSON error: {}", e)),
    };

    match import_settings(&json) {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' imported settings",
                session.username
            ));
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "message": t("settings.settings_imported"),
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Import failed: {}", e)),
    }
}

/// Reset all settings to defaults
pub async fn handle_settings_reset(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let session = match state.auth.get_session(session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    if !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    match reset_settings() {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' reset all settings to defaults",
                session.username
            ));
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "message": t("settings.settings_reset"),
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Reset failed: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic unit tests for the Settings core: the version comparison
    //! used to decide "an update is available", the display-name →
    //! OS-hostname sanitiser, and the shipped defaults that the compliance
    //! baseline pins (accent/density, and `UpdateFrequency::Manual` so no
    //! outbound GitHub call happens without the admin opting in).
    //!
    //! Nothing here touches the process-global `*_SETTINGS` statics, the
    //! network, or `sudo hostnamectl` — the HTTP handlers and their
    //! auth/round-trip behaviour are covered by
    //! `tests/settings_core_integration.rs`.
    //!
    //! Run: `cargo test handlers::settings::tests`

    use super::*;

    // -- parse_version --------------------------------------------------

    #[test]
    fn test_parse_version_accepts_a_three_part_version() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.0.1"), Some((0, 0, 1)));
        // Extra parts past patch are ignored.
        assert_eq!(parse_version("2.4.6.8"), Some((2, 4, 6)));
    }

    #[test]
    fn test_parse_version_rejects_malformed_input() {
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("v1.2.3"), None); // the leading 'v' must be stripped by the caller
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version(""), None);
    }

    // -- is_newer_version ---------------------------------------------

    #[test]
    fn test_is_newer_version_compares_semantically() {
        assert!(is_newer_version("1.0.1", "1.0.0"));
        assert!(is_newer_version("1.1.0", "1.0.9"));
        assert!(is_newer_version("2.0.0", "1.9.9"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.1"));
        assert!(!is_newer_version("1.9.9", "2.0.0"));
    }

    #[test]
    fn test_is_newer_version_is_false_when_either_side_is_unparseable() {
        // A version check that can't parse a side must never claim an update
        // is available (which would nag the admin forever).
        assert!(!is_newer_version("garbage", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "garbage"));
        assert!(!is_newer_version("", ""));
    }

    // -- sanitize_hostname ------------------------------------------

    #[test]
    fn test_sanitize_hostname_lowercases_and_keeps_valid_labels() {
        assert_eq!(sanitize_hostname("HomeVault"), "homevault");
        assert_eq!(sanitize_hostname("nas-01"), "nas-01");
        assert_eq!(sanitize_hostname("Living Room NAS"), "living-room-nas");
    }

    #[test]
    fn test_sanitize_hostname_collapses_and_trims_hyphens() {
        assert_eq!(sanitize_hostname("a  b"), "a-b");
        assert_eq!(sanitize_hostname("__weird__name__"), "weird-name");
        assert_eq!(
            sanitize_hostname("-leading-and-trailing-"),
            "leading-and-trailing"
        );
        assert_eq!(
            sanitize_hostname("dots.and.slashes/here"),
            "dots-and-slashes-here"
        );
    }

    #[test]
    fn test_sanitize_hostname_falls_back_when_nothing_valid_remains() {
        assert_eq!(sanitize_hostname(""), "homevault");
        assert_eq!(sanitize_hostname("   "), "homevault");
        assert_eq!(sanitize_hostname("🎉🎉🎉"), "homevault");
        assert_eq!(sanitize_hostname("---"), "homevault");
    }

    #[test]
    fn test_sanitize_hostname_caps_at_the_rfc1123_label_limit() {
        let long = "x".repeat(200);
        let out = sanitize_hostname(&long);
        assert_eq!(out.len(), 63);
        assert!(out.chars().all(|c| c == 'x'));
    }

    // -- shipped defaults (compliance baseline) --------------------

    #[test]
    fn test_update_frequency_defaults_to_manual_no_outbound_call() {
        // G2 in the CLAUDE.md compliance baseline: HomeVault must not
        // contact api.github.com on a timer without the admin opting in.
        assert_eq!(UpdateFrequency::default(), UpdateFrequency::Manual);
        assert_eq!(UpdateFrequency::Manual.as_seconds(), None);
        assert_eq!(UpdateFrequency::Disabled.as_seconds(), None);
        assert_eq!(UpdateFrequency::Daily.as_seconds(), Some(86_400));
        assert_eq!(UpdateFrequency::Weekly.as_seconds(), Some(604_800));
        assert_eq!(UpdateFrequency::Monthly.as_seconds(), Some(2_592_000));
    }

    #[test]
    fn test_update_frequency_serde_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&UpdateFrequency::Manual).unwrap(),
            "\"manual\""
        );
        assert_eq!(
            serde_json::from_str::<UpdateFrequency>("\"weekly\"").unwrap(),
            UpdateFrequency::Weekly
        );
    }

    #[test]
    fn test_appearance_defaults_are_the_privacy_safe_bundled_look() {
        let a = AppearanceSettings::default();
        assert_eq!(a.theme.as_deref(), Some("auto"));
        assert_eq!(a.density.as_deref(), Some("comfortable"));
        // HomeVault Blue clears §1.4.3 both behind white text (6.15:1) and
        // as text on the light surfaces/tints (4.99–5.68:1).
        assert_eq!(a.accent_color.as_deref(), Some("#1E62AE"));
        // No wallpaper by default — the desktop paints its own gradient.
        assert_eq!(a.wallpaper.as_deref(), Some(""));
    }

    #[test]
    fn test_normalize_accent_rewrites_only_the_legacy_default() {
        // The legacy default fails §1.4.3 both ways and must never reach a
        // client again, in any letter case…
        assert_eq!(normalize_accent("#4a90d9"), DEFAULT_ACCENT);
        assert_eq!(normalize_accent("#4A90D9"), DEFAULT_ACCENT);
        // …while every other value (round-tripped export, operator-set) is kept.
        assert_eq!(normalize_accent("#1E62AE"), "#1E62AE");
        assert_eq!(normalize_accent("#e2725b"), "#e2725b");
        assert_eq!(normalize_accent("#123456"), "#123456");
    }

    #[test]
    fn test_general_defaults() {
        let g = GeneralSettings::default();
        assert_eq!(g.server_name.as_deref(), Some("HomeVault"));
        assert_eq!(g.timezone.as_deref(), Some("UTC"));
    }
}
