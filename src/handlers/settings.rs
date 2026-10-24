/* =============================================================================
 * File          : settings.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Server and account settings endpoints: user language + UI translations,
 *   general settings (display name sanitized into a real OS hostname via
 *   hostnamectl), autostart, SMTP email, privacy-conservative update checks
 *   (off by default — no outbound call without admin consent), notification
 *   thresholds, appearance (theme/accent — non-WCAG-compliant legacy
 *   accents are rewritten on every load path), plus export/import/reset and
 *   the public pre-login settings subset. Most endpoints are admin-only;
 *   translations, own language and the public subset work pre-login.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - parking_lot (>= 0.12)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/settings in src/web/mod.rs; see docs/guides/settings.md
 *   (8 rail groups) and docs/guides/i18n.md (translations).
 * =============================================================================
 */

//! Server/account settings endpoints: language & translations, hostname,
//! email, update checks, notifications, appearance, export/import/reset.

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
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Environment override for the update-check endpoint.
///
/// It exists for two reasons: the privacy invariant ("no outbound call unless
/// the admin opted in") can be tested against a listener the test controls
/// instead of the real GitHub API, and an offline or mirrored install can
/// point the check at a reachable host. Unset (or empty) = the public API.
const VERSION_CHECK_URL_ENV: &str = "HOMEVAULT_VERSION_CHECK_URL";

fn version_check_url() -> String {
    std::env::var(VERSION_CHECK_URL_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| VERSION_CHECK_URL.to_string())
}

/// Outbound update-check requests this process has **attempted** (success or
/// failure). Incremented immediately before the HTTP call, so it is the
/// observable form of the "no third-party call without consent" rule: with
/// `Manual` / `Disabled` it must never move.
static UPDATE_CHECK_ATTEMPTS: AtomicU64 = AtomicU64::new(0);

/// How many outbound update checks this process has attempted.
pub fn update_check_attempts() -> u64 {
    UPDATE_CHECK_ATTEMPTS.load(Ordering::Relaxed)
}

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
    /// Server-wide default UI language (`en`, `fr`, `es`, `de`).
    ///
    /// ROADMAP 13.4: this was in-process state only, so the admin's choice
    /// reverted to `en` on the next restart. It is now part of the persisted
    /// General block and re-applied at boot (`init_persistence`), before the
    /// first page is served.
    #[serde(default)]
    pub language: Option<String>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            server_name: Some("HomeVault".to_string()),
            timezone: Some("UTC".to_string()),
            language: Some(DEFAULT_LANGUAGE.to_string()),
        }
    }
}

/// The languages HomeVault ships translations for. The single list every
/// entry point validates against (`POST /settings/language`, the General save,
/// the startup read).
pub const SUPPORTED_LANGUAGES: [&str; 4] = ["en", "fr", "es", "de"];

/// Locale served when no (valid) language is persisted.
pub const DEFAULT_LANGUAGE: &str = "en";

/// Normalise a caller-supplied language to a supported tag.
///
/// Accepts the full tag as well as its primary subtag (`fr-BE` → `fr`), which
/// is what a browser sends; anything else is refused so a typo cannot be
/// persisted and then silently fall back to English on the next boot.
pub fn normalize_language(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Language must not be empty".to_string());
    }
    let primary = trimmed
        .split(['-', '_'])
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    if SUPPORTED_LANGUAGES.contains(&primary.as_str()) {
        Ok(primary)
    } else {
        Err(format!(
            "Unsupported language '{}'. Supported: {:?}",
            trimmed, SUPPORTED_LANGUAGES
        ))
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

/// The host's own timezone, read from `/etc/timezone` (Debian family) or the
/// `zoneinfo/` suffix of the `/etc/localtime` symlink. Empty when neither is
/// readable.
///
/// Blocking file reads: called at startup (to seed the setting on a fresh
/// install) and from the Dashboard's already-offloaded server-details builder,
/// never from a handler's async body.
pub fn system_timezone() -> String {
    let tz = std::fs::read_to_string("/etc/timezone")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !tz.is_empty() {
        return tz;
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(idx) = s.find("zoneinfo/") {
            return s[idx + "zoneinfo/".len()..].to_string();
        }
    }
    String::new()
}

/// The appliance timezone everything else reads: the persisted General
/// setting when it names a valid IANA zone, `UTC` otherwise.
///
/// ROADMAP 12.2/13.4: this is the single value the scheduled-task engine
/// evaluates a task's cron in when the task has no zone of its own. It is a
/// pure in-memory read (no `/etc` access on a hot path — `tick_due` calls it
/// every 30 s), which is why `init_persistence` seeds the setting from the
/// host on a fresh install and `handle_general_settings_update` applies it to
/// the host with `timedatectl`.
pub fn effective_timezone() -> String {
    if let Some(tz) = GENERAL_SETTINGS.read().timezone.as_deref() {
        if chrono_tz::Tz::from_str(tz.trim()).is_ok() {
            return tz.trim().to_string();
        }
    }
    "UTC".to_string()
}

/// Applies the timezone as the real OS timezone via `timedatectl`
/// (`homevault` has a scoped sudoers grant for exactly
/// `timedatectl set-timezone *`). Needs root.
fn apply_system_timezone(timezone: &str) -> Result<(), String> {
    let output = std::process::Command::new("sudo")
        .args(["timedatectl", "set-timezone", timezone])
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

impl From<&EmailSettings> for crate::email::EmailConfig {
    /// The one place saved settings become a transport configuration. Both the
    /// test-email endpoint and the notification engine go through it, so the
    /// two cannot disagree about what "configured" means.
    fn from(settings: &EmailSettings) -> Self {
        Self {
            smtp_host: settings.smtp_server.clone().unwrap_or_default(),
            smtp_port: settings.smtp_port.unwrap_or(587),
            smtp_username: settings.smtp_username.clone().unwrap_or_default(),
            smtp_password: settings.smtp_password.clone().unwrap_or_default(),
            from_address: settings
                .from_address
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "homevault@localhost".to_string()),
            from_name: "HomeVault".to_string(),
            use_tls: settings.use_tls.unwrap_or(true),
        }
    }
}

/// Conservative email-address check for the two fields that end up as an
/// envelope/SMTP parameter (`from_address`, the notification recipient).
///
/// Not RFC 5322: it rejects what would break the argv/header shape — empty
/// local part or domain, whitespace, control characters and the separators
/// `<>(),;` — and accepts ordinary addresses, including the `homevault@localhost`
/// default. The point is to refuse an obviously wrong value at save time
/// instead of at 3 a.m. in a notification loop.
pub fn is_valid_email_address(addr: &str) -> bool {
    let addr = addr.trim();
    if addr.is_empty() || addr.len() > 254 {
        return false;
    }
    if addr
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')' | ',' | ';' | '"'))
    {
        return false;
    }
    let Some((local, domain)) = addr.split_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return false;
    }
    // A domain needs at least one dot — except the host-local default, which
    // is a legitimate `sendmail` recipient on this appliance.
    if domain != "localhost" && !domain.contains('.') {
        return false;
    }
    true
}

/// Validate an outgoing webhook URL (ROADMAP 13.5).
///
/// The stored value is handed to `reqwest` only when an alert fires, so an
/// unchecked URL is a notification that silently never arrives. Rules:
///
/// * empty clears the field (the card's way of disabling the webhook);
/// * `http` / `https` only — a `file:`/`data:` URL would make the server read
///   or POST to something that is not a webhook at all;
/// * a host is required and embedded credentials (`user:pass@`) are refused:
///   the URL is displayed back in the card, so a password in it would be shown
///   in clear text to whoever opens Settings, and the engine adds its own
///   auth headers;
/// * control characters, whitespace and anything longer than 2048 bytes are
///   refused (the de-facto header/URL bound).
///
/// Returns the trimmed URL so the caller stores exactly what was validated.
pub fn validate_webhook_url(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Ok(String::new());
    }
    if url.len() > 2048 {
        return Err("Webhook URL is too long (max 2048 characters)".to_string());
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Webhook URL must not contain spaces or control characters".to_string());
    }
    let parsed = url::Url::parse(url)
        .map_err(|e| format!("Invalid webhook URL '{}': {}", url, e))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "Webhook URL must use http or https (got '{}')",
                other
            ))
        }
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err("Webhook URL must name a host".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(
            "Webhook URL must not embed credentials — use a URL the receiver authenticates"
                .to_string(),
        );
    }
    Ok(url.to_string())
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

/// In-app notification settings (email, thresholds, per-event toggles).
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NotificationSettings {
    pub email: Option<String>,
    pub enable_alerts: Option<bool>,
    pub alert_threshold: Option<u8>,
    pub webhook_url: Option<String>,
    pub webhook_enabled: Option<bool>,
    pub smtp_enabled: Option<bool>,
    /// Per-event opt-outs keyed by event id (see
    /// `crate::notification_events::EVENT_CATALOG`). Absent = enabled.
    #[serde(default)]
    pub event_toggles: Option<std::collections::HashMap<String, bool>>,
    /// Outgoing webhook payload shape (generic | slack | discord).
    #[serde(default)]
    pub webhook_format: Option<String>,
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
            event_toggles: None,
            webhook_format: None,
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

    let language = match normalize_language(&req.language) {
        Ok(language) => language,
        Err(e) => return ResponseHelper::json_error(e),
    };

    // ROADMAP 13.4: persist it, or the choice reverts to `en` on the next
    // restart. `GENERAL_SETTINGS.language` is the stored copy; the i18n locale
    // is the live one, and both must move together.
    GENERAL_SETTINGS.write().language = Some(language.clone());
    I18n::set_locale(&language);
    crate::settings_persistence::save_all_settings();
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
        "language": current_lang,
        // The persisted value is echoed too: an admin checking the card should
        // be able to tell "this is the stored choice" from "this is what this
        // process happens to use", which ROADMAP 13.4 separated.
        "persisted": GENERAL_SETTINGS
            .read()
            .language
            .clone()
            .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string()),
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

    // The timezone is validated **before** anything is persisted or applied:
    // an unknown zone used to be stored verbatim and read by nothing, so the
    // admin had no way to tell it apart from a working one (ROADMAP 13.4).
    if let Some(timezone) = req.timezone.as_deref() {
        let trimmed = timezone.trim();
        if trimmed.is_empty() {
            return ResponseHelper::json_error("Timezone must not be empty".to_string());
        }
        if let Err(e) = chrono_tz::Tz::from_str(trimmed) {
            return ResponseHelper::json_error(format!(
                "Unknown timezone '{}' ({e}) — use an IANA name such as 'Europe/Paris'",
                trimmed
            ));
        }
    }

    // Same rule for the language (ROADMAP 13.4): the aggregate save carries it
    // too when the card saves everything at once, and an unsupported tag must
    // be refused here rather than persisted and silently ignored at boot.
    let language = match req.language.as_deref() {
        Some(raw) => match normalize_language(raw) {
            Ok(language) => Some(language),
            Err(e) => return ResponseHelper::json_error(e),
        },
        None => None,
    };

    // Scoped block, not a `drop()`: the guard is `!Send`, and the async fn
    // must not carry it across the `spawn_blocking` await below.
    {
        let mut settings = GENERAL_SETTINGS.write();
        if let Some(server_name) = &req.server_name {
            settings.server_name = Some(server_name.clone());
        }
        if let Some(timezone) = &req.timezone {
            settings.timezone = Some(timezone.trim().to_string());
        }
        if let Some(language) = &language {
            settings.language = Some(language.clone());
        }
    }
    if let Some(language) = &language {
        I18n::set_locale(language);
    }

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

    // Apply the timezone to the host (ROADMAP 13.4): it was stored and never
    // read, so "02:00 daily" was scheduled against whatever `/etc/localtime`
    // happened to say while the UI claimed the chosen zone. The setting is
    // now the single source of truth — the scheduler reads it, `timedatectl`
    // makes the rest of the host agree. Best effort, but a failure is
    // **reported**: the previous behaviour was a silent success.
    let mut timezone_notice = None;
    if let Some(timezone) = req.timezone.as_deref() {
        let target = timezone.trim().to_string();
        // Off the async runtime: `sudo timedatectl` can block for seconds
        // (X.1's `spawn_blocking` shape).
        let applied = tokio::task::spawn_blocking(move || apply_system_timezone(&target)).await;
        match applied {
            Ok(Ok(())) => {
                crate::events::log_event(format!(
                    "Admin '{}' changed the system timezone to '{}'",
                    session.username,
                    timezone.trim()
                ));
            }
            Ok(Err(err)) => {
                crate::events::log_event_warn(format!(
                    "Admin '{}' saved timezone '{}' but changing the OS timezone failed: {}",
                    session.username,
                    timezone.trim(),
                    err
                ));
                timezone_notice = Some(crate::i18n::t_var(
                    "settings.timezone_apply_failed",
                    "error",
                    err,
                ));
            }
            Err(join_err) => {
                crate::events::log_event_warn(format!(
                    "Admin '{}' saved timezone '{}' but the change task failed: {}",
                    session.username,
                    timezone.trim(),
                    join_err
                ));
                timezone_notice = Some(crate::i18n::t_var(
                    "settings.timezone_apply_failed",
                    "error",
                    join_err,
                ));
            }
        }
    }

    crate::events::log_event(format!(
        "Admin '{}' saved General settings",
        session.username
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": t("settings.settings_saved"),
        "restart_required": restart_required,
        "hostname_notice": if restart_required { Some(t("settings.hostname_restart_notice")) } else { None },
        "timezone_notice": timezone_notice,
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

/// Every zone name the scheduler can actually evaluate, so the Settings
/// screen offers the whole IANA database instead of a hardcoded shortlist.
///
/// The shortlist was the last place where the UI lied: the backend accepted
/// any name (`validate_timezone`), so an admin in Lisbon or Toulouse simply
/// had no way to pick their zone. `require_auth`, not admin: the list is the
/// public IANA database and the task editor reads the same names — the
/// *setting* itself stays admin-only.
pub async fn handle_timezones_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return response;
    }

    ResponseHelper::json_success(serde_json::json!({ "timezones": timezone_names() }))
}

/// The names the picker offers: everything that carries a region prefix
/// (`Europe/Paris`, `America/New_York`, `Etc/GMT+5`) plus `UTC`, sorted.
///
/// Legacy aliases without a region (`CET`, `GB-Eire`, `EST5EDT`…) are still
/// **accepted** on write — they are only kept out of the picker, where they
/// would duplicate every entry under a name nobody looks for.
fn timezone_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = chrono_tz::TZ_VARIANTS
        .iter()
        .map(|tz| tz.name())
        .filter(|name| name.contains('/') || *name == "UTC")
        .collect();
    names.sort_unstable();
    names.dedup();
    names
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

    // ROADMAP 13.3: these values are now the ones that reach msmtp, so they are
    // validated *before* they are persisted — a host with a space or a newline
    // would otherwise become a directive in the generated configuration.
    let mut next = EMAIL_SETTINGS.read().clone();
    if let Some(server) = &req.smtp_server {
        next.smtp_server = Some(server.trim().to_string());
    }
    if let Some(port) = req.smtp_port {
        next.smtp_port = Some(port);
    }
    if let Some(username) = &req.smtp_username {
        next.smtp_username = Some(username.trim().to_string());
    }
    if let Some(password) = &req.smtp_password {
        next.smtp_password = Some(password.clone());
    }
    if let Some(from) = &req.from_address {
        next.from_address = Some(from.trim().to_string());
    }
    if let Some(use_tls) = req.use_tls {
        next.use_tls = Some(use_tls);
    }

    if let Some(from) = next.from_address.as_deref().map(str::trim) {
        if !from.is_empty() && !is_valid_email_address(from) {
            return ResponseHelper::json_error(format!(
                "Invalid From address '{}' — expected an address such as bot@example.com",
                from
            ));
        }
    }
    if next.smtp_port == Some(0) {
        return ResponseHelper::json_error("SMTP port must be 1-65535".to_string());
    }
    // Only a *configured* relay is checked for insertion safety; an empty host
    // means "local MTA" and needs no msmtp configuration at all.
    let probe = crate::email::EmailConfig::from(&next);
    if probe.is_explicit() {
        if let Err(e) = probe.validate() {
            return ResponseHelper::json_error(format!("Invalid email settings: {e}"));
        }
    }

    *EMAIL_SETTINGS.write() = next;

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

    let settings = EMAIL_SETTINGS.read().clone();

    if settings.smtp_server.is_none() && settings.smtp_username.is_none() {
        return ResponseHelper::json_error("Email settings not configured".to_string());
    }

    let config = crate::email::EmailConfig::from(&settings);
    if config.is_explicit() {
        if let Err(e) = config.validate() {
            return ResponseHelper::json_error(format!("Email settings are invalid: {e}"));
        }
    }

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

    // ROADMAP 21.1 (same root cause as 13.3): sending shells out to an MTA with
    // a 30 s deadline, so it must not run on the async runtime.
    let sent =
        tokio::task::spawn_blocking(move || crate::email::EmailService::send_test_email(&recipient, &config))
            .await;

    match sent {
        Ok(Ok(_)) => ResponseHelper::json_success(serde_json::json!({
            "success": true,
            "message": t("settings.test_sent")
        })),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to send test email: {}", e)),
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
    // Counted before any I/O: a failed attempt is still an attempt, and the
    // privacy invariant is about *contacting* the third party at all.
    UPDATE_CHECK_ATTEMPTS.fetch_add(1, Ordering::Relaxed);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("HomeVault/Server")
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(version_check_url())
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

/// Decides whether a *scheduled* check is due — pure, so the matrix is unit
/// tested without touching the process-global settings or the network.
///
/// `None` (never configured) is treated exactly like `Manual`: an absent
/// preference is not consent. `Manual` and `Disabled` have no interval, so a
/// timer never fires for them; the only way to contact the update endpoint in
/// those modes is the explicit "Check now" action.
fn is_check_due(frequency: Option<UpdateFrequency>, last_check: Option<u64>, now: u64) -> bool {
    let Some(frequency) = frequency else {
        return false;
    };
    let Some(interval) = frequency.as_seconds() else {
        return false;
    };

    match last_check {
        None => true,
        Some(ts) => now.saturating_sub(ts) >= interval,
    }
}

/// Checks if an automatic update check should be performed.
///
/// This is the server-side gate for the privacy setting: `Manual` and
/// `Disabled` answer `false` unconditionally, and an opted-in schedule only
/// answers `true` once its interval has elapsed. It is enforced here, not in
/// the UI, so a client that calls the endpoint cannot talk HomeVault into
/// contacting GitHub.
pub fn should_check_for_updates() -> bool {
    let settings = UPDATE_SETTINGS.read();
    let frequency = settings.check_frequency;
    drop(settings);

    let last_check = *LAST_UPDATE_CHECK.read();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    is_check_due(frequency, last_check, now)
}

/// Records that a check just ran, so the schedule interval starts from now
/// (a *failed* attempt counts too — otherwise a blocked GitHub would be
/// retried on every Settings open).
fn mark_update_check_attempted() {
    *LAST_UPDATE_CHECK.write() = Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    );
}

/// The version payload with no fresh answer: the version we run, nothing more.
fn bare_version_info() -> UpdateCheckResult {
    UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: None,
        up_to_date: true,
        release_date: None,
        release_notes: None,
        error: None,
    }
}

/// Gets version info. Contacts the update endpoint **only** when the admin's
/// saved frequency says a scheduled check is due; otherwise it serves the last
/// cached answer (or the bare current version). Opening Settings is therefore
/// never an outbound call by itself.
pub async fn handle_version_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    serve_version_info(state, headers, false).await
}

/// "Check now" — the explicit admin action, and the only path that forces an
/// outbound call regardless of the saved frequency.
pub async fn handle_check_version_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    serve_version_info(state, headers, true).await
}

async fn serve_version_info(state: Arc<AppState>, headers: HeaderMap, force: bool) -> Response {
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

    // Forcing a third-party call is an admin decision; reading the cached
    // answer stays available to any signed-in user (the desktop footer shows
    // the running version).
    if force && !session.is_admin {
        return ResponseHelper::json_error("Admin access required".to_string());
    }

    if force || should_check_for_updates() {
        return match fetch_latest_version().await {
            Ok(fetched) => {
                mark_update_check_attempted();
                *CACHED_UPDATE_RESULT.write() = Some(fetched.clone());
                ResponseHelper::json_success(&fetched)
            }
            Err(e) => {
                tracing::warn!("Version check failed: {}", e);
                mark_update_check_attempted();
                let mut result = bare_version_info();
                result.error = Some("Could not check for updates".to_string());
                ResponseHelper::json_success(&result)
            }
        };
    }

    // No check due: serve the cache, refreshed against the version we are
    // actually running (a cached answer must not claim an old current
    // version after an upgrade).
    let mut result = CACHED_UPDATE_RESULT
        .read()
        .clone()
        .unwrap_or_else(bare_version_info);
    result.current_version = CURRENT_VERSION.to_string();
    result.up_to_date = match result.latest_version.as_deref() {
        Some(latest) => !is_newer_version(latest, CURRENT_VERSION),
        None => true,
    };
    result.error = None;

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

    // ROADMAP 13.5: the webhook URL used to be stored verbatim and handed to
    // `reqwest` only when an alert fired, so a typo surfaced hours later as a
    // silently dropped notification. Validate before persisting, into a copy,
    // so a refused field cannot leave the in-memory settings half-updated.
    let mut next = NOTIFICATION_SETTINGS.read().clone();
    if let Some(email) = &req.email {
        let email = email.trim();
        if !email.is_empty() && !is_valid_email_address(email) {
            return ResponseHelper::json_error(format!(
                "Invalid notification email '{}' — expected an address such as admin@example.com",
                email
            ));
        }
        next.email = Some(email.to_string());
    }
    if let Some(webhook_url) = &req.webhook_url {
        match validate_webhook_url(webhook_url) {
            Ok(url) => next.webhook_url = Some(url),
            Err(e) => return ResponseHelper::json_error(e),
        }
    }
    if let Some(enable) = req.enable_alerts {
        next.enable_alerts = Some(enable);
    }
    if let Some(threshold) = req.alert_threshold {
        next.alert_threshold = Some(threshold.clamp(0, 100));
    }
    if let Some(smtp) = req.smtp_enabled {
        next.smtp_enabled = Some(smtp);
    }
    if let Some(webhook_enabled) = req.webhook_enabled {
        next.webhook_enabled = Some(webhook_enabled);
    }
    if let Some(toggles) = &req.event_toggles {
        // Drop ids that aren't in the catalog — stale keys from removed
        // events shouldn't persist forever, and unknown ids are ignored by
        // `is_event_enabled` anyway.
        next.event_toggles = Some(
            toggles
                .iter()
                .filter(|(id, _)| {
                    crate::notification_events::EVENT_CATALOG
                        .iter()
                        .any(|e| e.id == &***id)
                })
                .map(|(id, enabled)| (id.clone(), *enabled))
                .collect(),
        );
    }
    if let Some(format) = &req.webhook_format {
        if crate::notification_events::WebhookFormat::parse(format).is_none() {
            return ResponseHelper::json_error(
                "Invalid webhook format: expected generic, slack or discord".to_string(),
            );
        }
        next.webhook_format = Some(format.clone());
    }
    *NOTIFICATION_SETTINGS.write() = next;

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
        "event_toggles": settings.event_toggles,
        "webhook_format": settings.webhook_format,
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
            // The stored choice (ROADMAP 13.4), not merely this process's
            // current locale — the Settings card edits the former.
            "language": general
                .language
                .clone()
                .unwrap_or_else(|| I18n::get_locale()),
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
            // ROADMAP 13.5: `reset_settings()` only clears `settings.json` and
            // the in-memory statics. The families stored elsewhere (antivirus,
            // anti-ransomware, disk spindown, database backups, `ups.conf`) are
            // reset here, and anything that failed is named in `warnings`
            // instead of being reported as a clean reset.
            let warnings = reset_domain_settings(&state).await;
            for warning in &warnings {
                crate::events::log_event_warn(format!(
                    "Settings reset: {} did not reset: {}",
                    session.username, warning
                ));
            }
            crate::events::log_event_warn(format!(
                "Admin '{}' reset all settings to defaults",
                session.username
            ));
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "message": t("settings.settings_reset"),
                "warnings": warnings,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Reset failed: {}", e)),
    }
}

/// Reset every settings family that does **not** live in `settings.json`.
///
/// ROADMAP 13.5: "Reset to Defaults" only cleared the JSON statics, so the
/// antivirus schedule and quarantine path, the anti-ransomware thresholds, the
/// per-disk spindown rows, the database-backup interval and the UPS device list
/// all survived a reset the UI described as complete. Each family is reset
/// through its own repository; failures are collected per domain and returned
/// rather than swallowed, because a partial reset that reports success is the
/// same class of bug as the UPS reload one.
pub async fn reset_domain_settings(state: &AppState) -> Vec<String> {
    let mut failures: Vec<String> = Vec::new();

    // Antivirus: SQLite singleton row (scan history and quarantined threats are
    // evidence, not settings — they are deliberately left alone).
    let av = crate::storage::AntivirusRepository::new(state.db.pool());
    let av_defaults = crate::storage::AvSettings::default();
    if let Err(e) = av
        .save_settings(
            &av_defaults.quarantine_dir,
            av_defaults.scheduled_scan_enabled,
            &av_defaults.scheduled_scan_cron,
            av_defaults.scheduled_scan_path.as_deref(),
            av_defaults.scan_archives,
        )
        .await
    {
        failures.push(format!("antivirus: {e}"));
    }

    // Anti-ransomware: settings row only; protected paths are admin data.
    let rw = crate::storage::RansomwareRepository::new(state.db.pool());
    let rw_defaults = crate::storage::RansomwareSettings::default();
    if let Err(e) = rw
        .save_settings(
            rw_defaults.enabled,
            rw_defaults.honeypots_enabled,
            rw_defaults.rate_threshold_events,
            rw_defaults.rate_threshold_seconds,
            &rw_defaults.auto_response,
        )
        .await
    {
        failures.push(format!("anti-ransomware: {e}"));
    }

    // Per-disk spindown: rows are the setting, so they are dropped. Nothing is
    // re-applied here — the defaults mean "no `hdparm` override", and the
    // startup path re-applies whatever is left on boot.
    let disk_power = crate::storage::DiskPowerRepository::new(state.db.pool());
    if let Err(e) = disk_power.delete_all().await {
        failures.push(format!("disk spindown: {e}"));
    }

    // Database snapshots: the schedule file (`data/db_backup.json`).
    let db_backup = crate::db_backup::DbBackupService::load(state.db.clone()).await;
    if let Err(e) = db_backup.reset_settings().await {
        failures.push(e);
    }

    // UPS: restore the shipped `ups.conf` section and reload NUT. Blocking
    // (`sudo tee` + `systemctl`), so off the runtime.
    match tokio::task::spawn_blocking(crate::ups::reset_config).await {
        Ok(Ok(reload_warnings)) => {
            for warning in reload_warnings {
                failures.push(format!("UPS reload: {warning}"));
            }
        }
        Ok(Err(e)) => failures.push(format!("UPS: {e}")),
        Err(e) => failures.push(format!("UPS: {e}")),
    }

    failures
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

    // -- the scheduled-check gate (ROADMAP 13.1) ---------------------

    #[test]
    fn test_no_scheduled_check_without_an_opted_in_frequency() {
        // Manual and Disabled both have no interval: the timer never fires,
        // whatever the last-check timestamp says. This is the server-side
        // half of the privacy baseline — the UI cannot talk the server into
        // an outbound call.
        let now = 1_800_000_000;
        assert!(!is_check_due(Some(UpdateFrequency::Manual), None, now));
        assert!(!is_check_due(
            Some(UpdateFrequency::Manual),
            Some(now - 10 * 86_400),
            now
        ));
        assert!(!is_check_due(Some(UpdateFrequency::Disabled), None, now));
        assert!(!is_check_due(
            Some(UpdateFrequency::Disabled),
            Some(now - 10 * 86_400),
            now
        ));
        // An absent preference is not consent either.
        assert!(!is_check_due(None, None, now));
        assert!(!is_check_due(None, Some(now - 10 * 86_400), now));
    }

    #[test]
    fn test_an_opted_in_schedule_fires_only_once_its_interval_elapsed() {
        let now = 1_800_000_000;
        // Never checked → due on the next tick.
        assert!(is_check_due(Some(UpdateFrequency::Daily), None, now));
        // Just checked → not due.
        assert!(!is_check_due(Some(UpdateFrequency::Daily), Some(now), now));
        assert!(!is_check_due(
            Some(UpdateFrequency::Daily),
            Some(now - 86_399),
            now
        ));
        // Exactly one interval later → due.
        assert!(is_check_due(
            Some(UpdateFrequency::Daily),
            Some(now - 86_400),
            now
        ));
        assert!(is_check_due(
            Some(UpdateFrequency::Weekly),
            Some(now - 604_800),
            now
        ));
        // A clock that went backwards must not fire the timer.
        assert!(!is_check_due(
            Some(UpdateFrequency::Daily),
            Some(now + 5_000),
            now
        ));
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

    // -- timezone_names (the picker's list) -----------------------------

    #[test]
    fn test_timezone_names_cover_the_zones_the_shortlist_missed() {
        let names = timezone_names();
        // The zones the Settings screen used to hardcode stay offered…
        for expected in ["UTC", "Europe/Paris", "America/New_York", "Asia/Tokyo"] {
            assert!(names.contains(&expected), "{expected} must be offered");
        }
        // …and so do the ones a 14-entry shortlist made unpickable.
        for expected in ["Europe/Lisbon", "Africa/Casablanca", "America/Sao_Paulo"] {
            assert!(names.contains(&expected), "{expected} must be offered");
        }
        assert!(
            names.len() > 300,
            "the picker must offer the IANA database, got {}",
            names.len()
        );
    }

    #[test]
    fn test_timezone_names_are_sorted_unique_and_parsable() {
        let names = timezone_names();
        // Sorted and unique in one assertion: every neighbour is strictly
        // smaller, so a duplicate (which would render twice) fails too.
        assert!(
            names.windows(2).all(|w| w[0] < w[1]),
            "the list must be sorted and deduplicated"
        );
        // Everything offered must be a name the scheduler can evaluate —
        // otherwise the screen would offer a value `validate_timezone`
        // refuses (the exact mismatch this endpoint exists to remove).
        for name in &names {
            assert!(
                name.parse::<chrono_tz::Tz>().is_ok(),
                "{name} must parse as an IANA zone"
            );
        }
        // Legacy aliases without a region are accepted on write but kept out
        // of the picker (they would duplicate every entry under a name
        // nobody looks for).
        assert!(!names.contains(&"CET"));
        assert!(!names.contains(&"GB-Eire"));
    }

    // -- ROADMAP 13.3/13.4/13.5: settings validation ---------------------

    #[test]
    fn test_language_normalisation_accepts_a_region_tag() {
        assert_eq!(normalize_language("en").unwrap(), "en");
        assert_eq!(normalize_language(" FR ").unwrap(), "fr");
        // A browser sends `fr-BE`; only the primary subtag is stored.
        assert_eq!(normalize_language("fr-BE").unwrap(), "fr");
        assert_eq!(normalize_language("de_AT").unwrap(), "de");
        assert_eq!(normalize_language("es-419").unwrap(), "es");
    }

    #[test]
    fn test_language_normalisation_refuses_an_unsupported_tag() {
        for bad in ["", "  ", "it", "xx", "en-US-extra-extra"] {
            assert!(
                normalize_language(bad).is_err(),
                "'{bad}' must not be persisted as the default language"
            );
        }
    }

    #[test]
    fn test_general_settings_default_language_is_the_shipped_one() {
        assert_eq!(GeneralSettings::default().language.as_deref(), Some(DEFAULT_LANGUAGE));
        assert!(SUPPORTED_LANGUAGES.contains(&DEFAULT_LANGUAGE));
    }

    #[test]
    fn test_email_address_check_accepts_ordinary_addresses() {
        for ok in [
            "bot@example.com",
            "first.last+tag@sub.example.co.uk",
            "homevault@localhost",
        ] {
            assert!(is_valid_email_address(ok), "'{ok}' is a valid address");
        }
    }

    #[test]
    fn test_email_address_check_refuses_what_would_break_the_argv_or_header() {
        for bad in [
            "",
            "no-at-sign",
            "@example.com",
            "bot@",
            "bot@@example.com",
            "bot@example",
            "bot @example.com",
            "bot@example.com\nBcc: victim@example.com",
            "bot@example.com,other@example.com",
            "\"Bot\" <bot@example.com>",
        ] {
            assert!(
                !is_valid_email_address(bad),
                "'{bad}' must be refused at save time"
            );
        }
    }

    #[test]
    fn test_webhook_url_accepts_http_and_https() {
        assert_eq!(
            validate_webhook_url("https://hooks.example.com/services/abc").unwrap(),
            "https://hooks.example.com/services/abc"
        );
        assert_eq!(
            validate_webhook_url("  http://192.168.1.10:9000/hook  ").unwrap(),
            "http://192.168.1.10:9000/hook"
        );
        // Empty means "clear the field", not an error.
        assert_eq!(validate_webhook_url("").unwrap(), "");
        assert_eq!(validate_webhook_url("   ").unwrap(), "");
    }

    #[test]
    fn test_webhook_url_refuses_schemes_and_shapes_that_are_not_a_webhook() {
        for bad in [
            "javascript:alert(1)",
            "file:///etc/shadow",
            "ftp://example.com/hook",
            "data:text/plain,hi",
            "not a url",
            "https://",
            "https://user:pass@example.com/hook",
            "https://example.com/ hook",
            "https://example.com/\nhook",
        ] {
            assert!(
                validate_webhook_url(bad).is_err(),
                "'{bad}' must be refused before it is persisted"
            );
        }
        // The 2048-byte bound.
        let long = format!("https://example.com/{}", "a".repeat(2048));
        assert!(validate_webhook_url(&long).is_err());
    }

    #[test]
    fn test_email_settings_become_the_transport_configuration() {
        // ROADMAP 13.3: one conversion feeds both the test button and the
        // notification engine.
        let settings = EmailSettings {
            smtp_server: Some("smtp.example.com".to_string()),
            smtp_port: Some(465),
            smtp_username: Some("bot@example.com".to_string()),
            smtp_password: Some("secret".to_string()),
            from_address: Some("bot@example.com".to_string()),
            use_tls: Some(true),
        };
        let config = crate::email::EmailConfig::from(&settings);
        assert!(config.is_explicit());
        assert_eq!(config.smtp_host, "smtp.example.com");
        assert_eq!(config.smtp_port, 465);
        assert_eq!(config.from_address, "bot@example.com");
        assert!(config.use_tls);

        // An empty host keeps the legacy local-MTA behaviour.
        let local = crate::email::EmailConfig::from(&EmailSettings::default());
        assert!(!local.is_explicit());
        assert_eq!(local.from_address, "homevault@localhost");
    }
}
