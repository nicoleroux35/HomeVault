/* =============================================================================
 * File          : email_oauth.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SMTP OAuth2 for Gmail and Microsoft 365 (P1-16): the authorization-code
 *   flow, sealed refresh-token storage, a bounded in-memory access-token cache
 *   and the msmtp auth mechanism each provider needs.
 *
 *   The refresh token and the client secret are sealed with the appliance key
 *   (`hv1:`) before they touch `data/email_oauth.json`, and neither is ever
 *   returned by an endpoint or written to a log line. A send only ever sees the
 *   short-lived access token, which lives in memory and in the generated
 *   `msmtp.conf` (0600) that msmtp deletes by being rewritten on the next send.
 *
 * Dependencies:
 *   - serde, serde_json, url, reqwest (blocking), chrono,
 *     crate::storage::secret_box, crate::storage::json
 *
 * Usage:
 *   handlers::email_oauth exposes the /api/settings/email/oauth routes;
 *   `access_token()` at send time. See docs/guides/email-oauth.md.
 * =============================================================================
 */

//! SMTP OAuth2 (Gmail / Microsoft 365), see docs/guides/email-oauth.md.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::storage::json::JsonStorage;
use crate::storage::secret_box;

/// Storage key -> `{DATA_DIRECTORY}/data/email_oauth.json`.
const STORE_KEY: &str = "email_oauth";

/// Seal scopes. Changing either invalidates every stored token, which is the
/// intended effect of a deliberate code change.
const CLIENT_SECRET_SCOPE: &str = "email-oauth:client-secret";
const REFRESH_TOKEN_SCOPE: &str = "email-oauth:refresh-token";

/// How long a started authorization flow may be redeemed.
pub const STATE_TTL: Duration = Duration::from_secs(600);

/// Cookie that binds the callback to the browser that started the flow.
pub const STATE_COOKIE: &str = "hv_email_oauth_state";

/// Deadline for one token request. The token endpoints answer in well under a
/// second; a proxy that has not answered in twenty has a real problem.
pub const TOKEN_TIMEOUT: Duration = Duration::from_secs(20);

/// How much of a provider error is quoted. Enough for `invalid_grant` and its
/// one-line description, short enough not to paste a wall of text into a card.
const MAX_ERROR_CHARS: usize = 300;

/// Largest token response accepted. A real answer is a few hundred bytes.
const MAX_TOKEN_BODY: usize = 64 * 1024;

/// Refresh this many seconds before the access token actually expires, so a
/// send that starts just before the deadline does not race it.
const EXPIRY_MARGIN_SECS: i64 = 60;

/// Fallback lifetime when the provider omits `expires_in` (RFC 6749 says it is
/// recommended, not required). One hour is what both providers issue.
const DEFAULT_EXPIRES_IN: i64 = 3600;

// --------------------------------------------------------------------------
// Providers
// --------------------------------------------------------------------------

/// The two providers with an SMTP OAuth2 story a home appliance actually uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OAuthProvider {
    #[default]
    Google,
    Microsoft,
}

impl OAuthProvider {
    pub const ALL: &'static [OAuthProvider] = &[OAuthProvider::Google, OAuthProvider::Microsoft];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Microsoft => "microsoft",
        }
    }

    /// Lenient parse for a stored value (never makes the feature unloadable).
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "microsoft" | "m365" | "office365" | "outlook" => Self::Microsoft,
            _ => Self::Google,
        }
    }

    /// Strict parse for a request: an unknown provider is refused.
    pub fn parse_strict(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "google" | "gmail" => Ok(Self::Google),
            "microsoft" | "m365" | "office365" | "outlook" => Ok(Self::Microsoft),
            other => Err(format!(
                "unknown provider '{other}' (expected google or microsoft)"
            )),
        }
    }

    pub fn authorization_endpoint(self) -> &'static str {
        match self {
            Self::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
        }
    }

    pub fn token_endpoint(self) -> &'static str {
        match self {
            Self::Google => "https://oauth2.googleapis.com/token",
            Self::Microsoft => "https://login.microsoftonline.com/common/oauth2/v2.0/token",
        }
    }

    /// The narrowest scope that still allows sending mail.
    ///
    /// Gmail's SMTP scope is all-or-nothing (`https://mail.google.com/`); it has
    /// no send-only scope. Microsoft offers `SMTP.Send`, which is send-only.
    pub fn scope(self) -> &'static str {
        match self {
            Self::Google => "https://mail.google.com/",
            Self::Microsoft => "offline_access https://outlook.office.com/SMTP.Send",
        }
    }

    /// msmtp's auth mechanism. Microsoft's SMTP AUTH implementation expects
    /// `AUTH XOAUTH2`; Google accepts both, and OAUTHBEARER is the newer one.
    pub fn auth_method(self) -> &'static str {
        match self {
            Self::Google => "oauthbearer",
            Self::Microsoft => "xoauth2",
        }
    }

    /// The relay host/port the provider expects, shown as a hint in the UI.
    pub fn smtp_defaults(self) -> (&'static str, u16) {
        match self {
            Self::Google => ("smtp.gmail.com", 587),
            Self::Microsoft => ("smtp.office365.com", 587),
        }
    }
}

// --------------------------------------------------------------------------
// Settings
// --------------------------------------------------------------------------

/// Persisted OAuth2 configuration (`data/email_oauth.json`).
///
/// The two secrets are sealed blobs, never plaintext; `client_id` is public
/// (it appears in the browser's authorization URL) and therefore stored as-is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmailOAuthSettings {
    #[serde(default)]
    pub provider: OAuthProvider,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret_sealed: Option<String>,
    #[serde(default)]
    pub refresh_token_sealed: Option<String>,
    /// Exact callback registered at the provider. Never derived from a `Host`
    /// header: a forged header must not choose where a code is delivered.
    #[serde(default)]
    pub redirect_url: String,
    /// RFC 3339: when the current refresh token was stored.
    #[serde(default)]
    pub connected_at: Option<String>,
    /// Last refresh/exchange failure, already bounded and token-free.
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for EmailOAuthSettings {
    fn default() -> Self {
        Self {
            provider: OAuthProvider::default(),
            client_id: String::new(),
            client_secret_sealed: None,
            refresh_token_sealed: None,
            redirect_url: String::new(),
            connected_at: None,
            last_error: None,
        }
    }
}

impl EmailOAuthSettings {
    /// Client id, client secret and callback are present: a flow can start.
    pub fn is_configured(&self) -> bool {
        !self.client_id.trim().is_empty()
            && self.client_secret_sealed.is_some()
            && !self.redirect_url.trim().is_empty()
    }

    /// A refresh token is stored: sends use OAuth2 instead of a password.
    pub fn is_connected(&self) -> bool {
        self.refresh_token_sealed.is_some()
    }

    /// What the endpoints may return: never a secret, never a token.
    pub fn public_status(&self) -> PublicStatus {
        let (smtp_host, smtp_port) = self.provider.smtp_defaults();
        PublicStatus {
            provider: self.provider.as_str().to_string(),
            client_id: self.client_id.clone(),
            redirect_url: self.redirect_url.clone(),
            configured: self.is_configured(),
            connected: self.is_connected(),
            connected_at: self.connected_at.clone(),
            last_error: self.last_error.clone(),
            auth_method: self.provider.auth_method().to_string(),
            smtp_host: smtp_host.to_string(),
            smtp_port,
        }
    }
}

/// The admin-facing shape. Deliberately has no `client_secret` and no token.
#[derive(Debug, Clone, Serialize)]
pub struct PublicStatus {
    pub provider: String,
    pub client_id: String,
    pub redirect_url: String,
    pub configured: bool,
    pub connected: bool,
    pub connected_at: Option<String>,
    pub last_error: Option<String>,
    pub auth_method: String,
    pub smtp_host: String,
    pub smtp_port: u16,
}

// --------------------------------------------------------------------------
// Validation (pure)
// --------------------------------------------------------------------------

/// `[A-Za-z0-9._~-]`, at most 512 characters. Covers Google client ids
/// (`….apps.googleusercontent.com`) and Microsoft GUIDs.
pub fn validate_client_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("The client ID must not be empty".to_string());
    }
    if value.len() > 512 {
        return Err("The client ID is too long".to_string());
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
    {
        return Err(
            "The client ID may only contain letters, digits, '.', '_', '~' and '-'".to_string(),
        );
    }
    Ok(value.to_string())
}

/// The client secret is printed material: refuse control characters only, so a
/// provider-specific alphabet never has to be guessed.
pub fn validate_client_secret(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("The client secret must not be empty".to_string());
    }
    if value.len() > 512 {
        return Err("The client secret is too long".to_string());
    }
    if value.chars().any(char::is_control) {
        return Err("The client secret must not contain control characters".to_string());
    }
    Ok(value.to_string())
}

/// The callback must be an absolute http(s) URL with no credentials and no
/// fragment: it is registered verbatim at the provider and compared by the
/// token exchange.
pub fn validate_redirect_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("The redirect URL must not be empty".to_string());
    }
    if value.len() > 1024 {
        return Err("The redirect URL is too long".to_string());
    }
    if value.chars().any(char::is_control) {
        return Err("The redirect URL must not contain control characters".to_string());
    }
    let parsed = url::Url::parse(value)
        .map_err(|_| "The redirect URL must be an absolute URL".to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("The redirect URL must start with http:// or https://".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("The redirect URL must not embed credentials".to_string());
    }
    if parsed.fragment().is_some() {
        return Err("The redirect URL must not contain a fragment".to_string());
    }
    Ok(value.to_string())
}

/// Build the provider's authorization URL. Pure, so the exact query parameters
/// (offline access, consent, the callback) are unit-testable.
pub fn authorize_url(
    provider: OAuthProvider,
    client_id: &str,
    redirect_url: &str,
    state: &str,
) -> Result<String, String> {
    let client_id = validate_client_id(client_id)?;
    let redirect_url = validate_redirect_url(redirect_url)?;
    if state.trim().is_empty() {
        return Err("The authorization state must not be empty".to_string());
    }
    let mut url = url::Url::parse(provider.authorization_endpoint())
        .map_err(|e| format!("Invalid provider endpoint: {e}"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("client_id", &client_id);
        query.append_pair("redirect_uri", &redirect_url);
        query.append_pair("response_type", "code");
        query.append_pair("scope", provider.scope());
        query.append_pair("state", state);
        match provider {
            // `access_type=offline` is what makes Google return a refresh
            // token at all; `prompt=consent` forces it on a repeat connection
            // (Google omits it when the app was already approved).
            OAuthProvider::Google => {
                query.append_pair("access_type", "offline");
                query.append_pair("prompt", "consent");
                query.append_pair("include_granted_scopes", "true");
            }
            // Microsoft: `offline_access` is in the scope; no prompt needed.
            OAuthProvider::Microsoft => {}
        }
    }
    Ok(url.to_string())
}

// --------------------------------------------------------------------------
// Token responses (pure)
// --------------------------------------------------------------------------

/// One successful token answer. Only the access token is ever used directly;
/// a rotated refresh token is resealed and stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
}

/// Parse the provider's JSON. On an error answer, `error_description` (or
/// `error`) becomes the message; the body itself is never quoted, so a token
/// accidentally present in an awkward response cannot reach a log line.
pub fn parse_token_response(body: &str) -> Result<TokenResponse, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| "The provider answer was not JSON".to_string())?;
    let access_token = value
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(access_token) = access_token else {
        let detail = value
            .get("error_description")
            .and_then(Value::as_str)
            .or_else(|| value.get("error").and_then(Value::as_str))
            .map(bounded_error)
            .unwrap_or_else(|| "The provider refused the token request".to_string());
        return Err(detail);
    };
    let refresh_token = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let expires_in = value
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_EXPIRES_IN)
        .clamp(60, 7 * 24 * 3600);
    Ok(TokenResponse {
        access_token: access_token.to_string(),
        refresh_token,
        expires_in,
    })
}

/// Strip control characters, collapse the length and never include a token.
fn bounded_error(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.chars().count() <= MAX_ERROR_CHARS {
        cleaned.to_string()
    } else {
        cleaned.chars().take(MAX_ERROR_CHARS).collect::<String>() + "…"
    }
}

// --------------------------------------------------------------------------
// Persistence
// --------------------------------------------------------------------------

fn store() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// Serialises every read-modify-write of `data/email_oauth.json`.
static SETTINGS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Serialises token refreshes, so ten concurrent alerts make one token call.
static REFRESH_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// The cached access token. Deliberately process-local: a restart costs one
/// refresh, and nothing short-lived is written to disk.
static CACHE: LazyLock<Mutex<Option<CachedToken>>> = LazyLock::new(|| Mutex::new(None));

/// Started flows, keyed by state. Single-use and time-bounded.
static PENDING: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

/// Load the saved settings, or the empty default. A corrupt file degrades to
/// "not configured" instead of breaking mail delivery (which would fall back to
/// password auth, exactly as before this feature existed).
pub fn load_settings() -> EmailOAuthSettings {
    match store().load::<EmailOAuthSettings>(STORE_KEY) {
        Ok(Some(mut settings)) => {
            settings.client_id = settings.client_id.trim().to_string();
            settings.redirect_url = settings.redirect_url.trim().to_string();
            settings
        }
        Ok(None) => EmailOAuthSettings::default(),
        Err(e) => {
            tracing::warn!(
                "Email OAuth2: cannot read data/email_oauth.json, treating it as unconfigured: {e}"
            );
            EmailOAuthSettings::default()
        }
    }
}

/// Persist settings. Callers must have sealed the secrets already.
pub fn save_settings(settings: &EmailOAuthSettings) -> Result<(), String> {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    store()
        .save(STORE_KEY, settings)
        .map_err(|e| e.to_string())?;
    // Any configuration change invalidates the in-memory token.
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

/// Store the admin's configuration. `client_secret` is `None` when the field
/// was left blank (= keep the stored one). Changing provider or client id
/// clears the refresh token, because it belongs to the old registration.
pub fn apply_configuration(
    provider: OAuthProvider,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_url: &str,
) -> Result<EmailOAuthSettings, String> {
    let client_id = validate_client_id(client_id)?;
    let redirect_url = validate_redirect_url(redirect_url)?;
    let secret = match client_secret {
        Some(value) => Some(validate_client_secret(value)?),
        None => None,
    };

    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut settings = match store().load::<EmailOAuthSettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => EmailOAuthSettings::default(),
        Err(e) => return Err(format!("Cannot read the saved configuration: {e}")),
    };
    let registration_changed = settings.provider != provider || settings.client_id != client_id;
    settings.provider = provider;
    settings.client_id = client_id;
    settings.redirect_url = redirect_url;
    if let Some(secret) = secret {
        settings.client_secret_sealed = Some(secret_box::seal(CLIENT_SECRET_SCOPE, &secret)?);
    }
    if registration_changed {
        settings.refresh_token_sealed = None;
        settings.connected_at = None;
    }
    settings.last_error = None;
    store()
        .save(STORE_KEY, &settings)
        .map_err(|e| e.to_string())?;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(settings)
}

/// Forget the client registration and the refresh token.
pub fn disconnect() -> Result<(), String> {
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    store().delete(STORE_KEY).map_err(|e| e.to_string())?;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = HashMap::new();
    Ok(())
}

/// Open the client secret. Never logged, never returned.
pub fn client_secret(settings: &EmailOAuthSettings) -> Result<Option<String>, String> {
    match settings.client_secret_sealed.as_deref() {
        Some(sealed) => secret_box::open(CLIENT_SECRET_SCOPE, sealed).map(Some),
        None => Ok(None),
    }
}

/// Open the refresh token. Never logged, never returned.
fn refresh_token(settings: &EmailOAuthSettings) -> Result<Option<String>, String> {
    match settings.refresh_token_sealed.as_deref() {
        Some(sealed) => secret_box::open(REFRESH_TOKEN_SCOPE, sealed).map(Some),
        None => Ok(None),
    }
}

/// Record a failure the admin can see, without ever being able to see a token.
pub fn record_error(message: &str) {
    let message = bounded_error(message);
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(Some(mut settings)) = store().load::<EmailOAuthSettings>(STORE_KEY) else {
        return;
    };
    settings.last_error = Some(message);
    let _ = store().save(STORE_KEY, &settings);
}

/// Store a refresh token after a successful exchange (or rotation).
fn store_refresh_token(token: &str, rotated: bool) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("The provider returned an empty refresh token".to_string());
    }
    let sealed = secret_box::seal(REFRESH_TOKEN_SCOPE, token)?;
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut settings = match store().load::<EmailOAuthSettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => return Err("The OAuth2 configuration disappeared".to_string()),
        Err(e) => return Err(format!("Cannot read the saved configuration: {e}")),
    };
    settings.refresh_token_sealed = Some(sealed);
    // A rotation is not a new connection: keep the original date, which is
    // what the card shows ("connected since").
    if !rotated || settings.connected_at.is_none() {
        settings.connected_at = Some(chrono::Utc::now().to_rfc3339());
    }
    settings.last_error = None;
    store()
        .save(STORE_KEY, &settings)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Public wrapper used by the callback: an initial connection is not a
/// rotation, so the card shows the date it was established.
pub fn store_refresh_token_after_exchange(token: &str) -> Result<(), String> {
    store_refresh_token(token, false)
}

// --------------------------------------------------------------------------
// Authorization state (OIDC's model: single-use, browser-bound)
// --------------------------------------------------------------------------

/// Register a new state and return it. The caller sets it as an HttpOnly cookie
/// and puts it in the authorization URL; the callback must present both.
pub fn new_state() -> Result<String, String> {
    let state = crate::oidc::random_token(32)?;
    let now = Instant::now();
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    pending.retain(|_, issued| now.duration_since(*issued) < STATE_TTL);
    pending.insert(state.clone(), now);
    Ok(state)
}

/// Redeem a state once. Expired or unknown states are refused.
pub fn take_state(state: &str) -> bool {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(issued) = pending.remove(state) else {
        return false;
    };
    Instant::now().duration_since(issued) < STATE_TTL
}

// --------------------------------------------------------------------------
// HTTP (blocking — always called from spawn_blocking or a blocking worker)
// --------------------------------------------------------------------------

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(TOKEN_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("HomeVault/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Cannot create the token client: {e}"))
}

/// One form POST to a token endpoint, with a bounded body read.
fn post_token(endpoint: &str, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let response = client()?
        .post(endpoint)
        .form(form)
        .send()
        .map_err(|e| format!("The token request failed: {e}"))?;
    let bytes = response
        .bytes()
        .map_err(|e| format!("Cannot read the token answer: {e}"))?;
    let slice = &bytes[..bytes.len().min(MAX_TOKEN_BODY)];
    let body = String::from_utf8_lossy(slice);
    parse_token_response(&body)
}

/// Exchange an authorization code for tokens.
pub fn exchange_code_blocking(
    settings: &EmailOAuthSettings,
    code: &str,
) -> Result<TokenResponse, String> {
    let client_id = validate_client_id(&settings.client_id)?;
    let redirect_url = validate_redirect_url(&settings.redirect_url)?;
    let secret =
        client_secret(settings)?.ok_or_else(|| "No client secret is stored".to_string())?;
    let code = code.trim();
    if code.is_empty() {
        return Err("The provider returned no authorization code".to_string());
    }
    post_token(
        settings.provider.token_endpoint(),
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", &client_id),
            ("client_secret", &secret),
            ("redirect_uri", &redirect_url),
        ],
    )
}

/// Refresh the access token from the stored refresh token.
pub fn refresh_blocking(
    settings: &EmailOAuthSettings,
    refresh_token: &str,
) -> Result<TokenResponse, String> {
    let client_id = validate_client_id(&settings.client_id)?;
    let secret =
        client_secret(settings)?.ok_or_else(|| "No client secret is stored".to_string())?;
    post_token(
        settings.provider.token_endpoint(),
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &client_id),
            ("client_secret", &secret),
            ("scope", settings.provider.scope()),
        ],
    )
}

// --------------------------------------------------------------------------
// Delivery seam used by email.rs
// --------------------------------------------------------------------------

/// What a send needs from this module: a fresh access token and the msmtp auth
/// mechanism. The refresh token and client secret never leave this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthAccess {
    pub access_token: String,
    pub method: &'static str,
}

/// Return a fresh access token, or `None` when OAuth2 is not connected (the
/// caller then uses the password path exactly as before).
///
/// A configured-but-failing connection is an **error**, never a silent
/// fallback to the password: the stored password may still be valid, but
/// hiding `invalid_grant` behind "authentication failed" is what makes an
/// expired consent impossible to diagnose.
pub fn access_token() -> Result<Option<OAuthAccess>, String> {
    let settings = load_settings();
    if !settings.is_configured() || !settings.is_connected() {
        return Ok(None);
    }

    let _refresh = REFRESH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = cache.as_ref() {
            if cached.expires_at > Instant::now() {
                return Ok(Some(OAuthAccess {
                    access_token: cached.access_token.clone(),
                    method: settings.provider.auth_method(),
                }));
            }
        }
    }

    let refresh =
        refresh_token(&settings)?.ok_or_else(|| "The refresh token is missing".to_string())?;
    let response = match refresh_blocking(&settings, &refresh) {
        Ok(response) => response,
        Err(e) => {
            record_error(&e);
            return Err(e);
        }
    };
    if let Some(rotated) = &response.refresh_token {
        // Google does not rotate by default; Microsoft can. Losing a rotated
        // token would force the admin to reconnect, so persist it first.
        if let Err(e) = store_refresh_token(rotated, true) {
            tracing::warn!("Email OAuth2: could not store the rotated refresh token: {e}");
        }
    }
    let lifetime = response.expires_in.max(60) - EXPIRY_MARGIN_SECS;
    let expires_at = Instant::now() + Duration::from_secs(lifetime.max(30) as u64);
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(CachedToken {
        access_token: response.access_token.clone(),
        expires_at,
    });
    Ok(Some(OAuthAccess {
        access_token: response.access_token,
        method: settings.provider.auth_method(),
    }))
}

/// The `client_secret`/`refresh_token` values must never appear in a log line.
/// Kept as a reminder for future edits; tests assert the public shape instead.
#[cfg(test)]
fn contains_secret(settings: &EmailOAuthSettings, haystack: &str) -> bool {
    [
        &settings.client_secret_sealed,
        &settings.refresh_token_sealed,
    ]
    .into_iter()
    .flatten()
    .any(|sealed| haystack.contains(sealed.as_str()))
}

/// Reset the in-process cache. Called by tests, and after a configuration
/// change through [`save_settings`]/[`apply_configuration`]/[`disconnect`].
pub fn clear_cache() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> EmailOAuthSettings {
        EmailOAuthSettings {
            provider: OAuthProvider::Google,
            client_id: "123.apps.googleusercontent.com".to_string(),
            client_secret_sealed: Some("hv1:sealed-client".to_string()),
            refresh_token_sealed: Some("hv1:sealed-refresh".to_string()),
            redirect_url: "https://nas.example.com/api/settings/email/oauth/callback".to_string(),
            connected_at: Some("2026-11-09T10:00:00+00:00".to_string()),
            last_error: None,
        }
    }

    #[test]
    fn test_provider_parse_and_defaults() {
        assert_eq!(OAuthProvider::parse("gmail"), OAuthProvider::Google);
        assert_eq!(OAuthProvider::parse("M365"), OAuthProvider::Microsoft);
        // Unknown stored values fall back instead of making the file unusable.
        assert_eq!(OAuthProvider::parse("nope"), OAuthProvider::Google);
        assert!(OAuthProvider::parse_strict("nope").is_err());
        assert_eq!(
            OAuthProvider::Microsoft.smtp_defaults(),
            ("smtp.office365.com", 587)
        );
        assert_eq!(OAuthProvider::Google.auth_method(), "oauthbearer");
        assert_eq!(OAuthProvider::Microsoft.auth_method(), "xoauth2");
        assert!(OAuthProvider::Google.scope().contains("mail.google.com"));
        assert!(OAuthProvider::Microsoft.scope().contains("SMTP.Send"));
    }

    #[test]
    fn test_google_authorization_url_requests_offline_access() {
        let url = authorize_url(
            OAuthProvider::Google,
            "123.apps.googleusercontent.com",
            "https://nas.example.com/api/settings/email/oauth/callback",
            "state-1",
        )
        .unwrap();
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("state=state-1"));
        // The callback is percent-encoded, not spliced raw.
        assert!(url.contains("redirect_uri=https%3A%2F%2Fnas.example.com%2Fapi%2Fsettings%2Femail%2Foauth%2Fcallback"));
    }

    #[test]
    fn test_microsoft_authorization_url_uses_its_own_endpoint() {
        let url = authorize_url(
            OAuthProvider::Microsoft,
            "00000000-0000-0000-0000-000000000000",
            "https://nas.example.com/api/settings/email/oauth/callback",
            "state-2",
        )
        .unwrap();
        assert!(url.starts_with("https://login.microsoftonline.com/"));
        assert!(url.contains("offline_access"));
        assert!(url.contains("SMTP.Send"));
        assert!(!url.contains("prompt=consent"));
    }

    #[test]
    fn test_authorization_url_refuses_bad_inputs() {
        assert!(authorize_url(
            OAuthProvider::Google,
            "bad id",
            "https://nas.example.com/cb",
            "s"
        )
        .is_err());
        assert!(authorize_url(
            OAuthProvider::Google,
            "client",
            "ftp://nas.example.com/cb",
            "s"
        )
        .is_err());
        assert!(authorize_url(OAuthProvider::Google, "client", "/relative", "s").is_err());
        assert!(authorize_url(
            OAuthProvider::Google,
            "client",
            "https://nas.example.com/cb",
            "  "
        )
        .is_err());
    }

    #[test]
    fn test_redirect_url_validation() {
        assert_eq!(
            validate_redirect_url("https://nas.example.com/api/settings/email/oauth/callback")
                .unwrap(),
            "https://nas.example.com/api/settings/email/oauth/callback"
        );
        for bad in [
            "",
            "/api/settings/email/oauth/callback",
            "ftp://nas.example.com/cb",
            "https://user:pass@nas.example.com/cb",
            "https://nas.example.com/cb#frag",
            "https://nas.example.com/a b",
        ] {
            assert!(validate_redirect_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_client_id_and_secret_validation() {
        assert!(validate_client_id("123.apps.googleusercontent.com").is_ok());
        assert!(validate_client_id("00000000-0000-0000-0000-000000000000").is_ok());
        for bad in ["", "with space", "with/slash", "line\nbreak"] {
            assert!(validate_client_id(bad).is_err(), "{bad:?}");
        }
        assert!(validate_client_secret("~abc.DEF_123-xyz").is_ok());
        assert!(validate_client_secret("").is_err());
        assert!(validate_client_secret("line\nbreak").is_err());
    }

    #[test]
    fn test_token_response_parses_success_and_rotation() {
        let response = parse_token_response(
            r#"{"access_token":"ya29.token","refresh_token":"1//rotated","expires_in":3600,"token_type":"Bearer"}"#,
        )
        .unwrap();
        assert_eq!(response.access_token, "ya29.token");
        assert_eq!(response.refresh_token.as_deref(), Some("1//rotated"));
        assert_eq!(response.expires_in, 3600);

        // Missing refresh_token is normal on a refresh answer.
        let response = parse_token_response(r#"{"access_token":"a","expires_in":1800}"#).unwrap();
        assert_eq!(response.refresh_token, None);
        assert_eq!(response.expires_in, 1800);

        // A missing/absurd expires_in is bounded, never a 0-second cache.
        let response = parse_token_response(r#"{"access_token":"a"}"#).unwrap();
        assert_eq!(response.expires_in, DEFAULT_EXPIRES_IN);
        let response = parse_token_response(r#"{"access_token":"a","expires_in":1}"#).unwrap();
        assert_eq!(response.expires_in, 60);
    }

    #[test]
    fn test_token_error_is_bounded_and_never_quotes_a_token() {
        let err = parse_token_response(
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        )
        .unwrap_err();
        assert!(err.contains("expired or revoked"), "{err}");
        assert!(!err.contains("access_token"));

        let err = parse_token_response("not-json").unwrap_err();
        assert!(err.contains("not JSON"), "{err}");

        let long = "x".repeat(MAX_ERROR_CHARS + 50);
        let err =
            parse_token_response(&format!(r#"{{"error_description":"{}"}}"#, long)).unwrap_err();
        assert!(err.chars().count() <= MAX_ERROR_CHARS + 1, "{err}");
    }

    #[test]
    fn test_public_status_has_no_secret_and_no_token() {
        let settings = settings();
        let json = serde_json::to_string(&settings.public_status()).unwrap();
        assert!(json.contains("\"connected\":true"));
        assert!(json.contains("\"configured\":true"));
        assert!(json.contains("oauthbearer"));
        assert!(!json.contains("sealed"));
        assert!(!contains_secret(&settings, &json));
        assert!(!json.contains("client_secret"));
        assert!(!json.contains("refresh_token"));
    }

    #[test]
    fn test_configured_and_connected_are_distinct() {
        let mut settings = settings();
        assert!(settings.is_configured());
        assert!(settings.is_connected());
        settings.refresh_token_sealed = None;
        assert!(settings.is_configured());
        assert!(!settings.is_connected());
        settings.client_secret_sealed = None;
        assert!(!settings.is_configured());
    }

    #[test]
    fn test_state_is_single_use_and_expires() {
        let state = new_state().unwrap();
        assert!(take_state(&state));
        assert!(!take_state(&state));
        assert!(!take_state("unknown"));
    }
}
