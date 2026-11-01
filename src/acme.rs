/* =============================================================================
 * File          : acme.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Certificate automation: request, renew and install a publicly trusted
 *   TLS certificate from an ACME certificate authority (RFC 8555) over
 *   HTTP-01 or DNS-01, without leaving HomeVault. The private key never
 *   leaves {DATA_DIRECTORY}/data/acme/ and is never stored, logged or
 *   returned by the API.
 *
 * Dependencies:
 *   - rcgen (>= 0.14, CSR + P-256 key generation)
 *   - reqwest (>= 0.13, JSON + rustls already used by the update checker)
 *
 * Usage:
 *   handlers::acme exposes the four endpoints; the daily worker lives in
 *   acme_worker::run; the HTTPS listener picks the issued certificate up
 *   from data/acme/tls/{cert,key}.pem (see crate::tls and
 *   docs/guides/acme.md).
 * =============================================================================
 */

//! Automatic certificates (ACME) for HomeVault's built-in HTTPS listener.
//!
//! An admin enables automation in **Settings → Security → HTTPS/TLS**, enters
//! the domain HomeVault is reached on, and picks a challenge:
//!
//! * **HTTP-01** — the CA fetches `http://<domain>/.well-known/acme-challenge/
//!   <token>`; HomeVault answers it from a role-tagged route on its existing
//!   plain-HTTP listener, so port 80 must reach this box.
//! * **DNS-01** — HomeVault publishes a `_acme-challenge` TXT record through
//!   the configured provider (or asks the admin to paste it once for the
//!   `manual` provider). Works behind a router that forwards nothing, and for
//!   wildcard names.
//!
//! Four states, each meaningful on its own:
//!
//! | Status | Meaning |
//! |--------|---------|
//! | `disabled` | The admin has not enabled automation |
//! | `idle` | Enabled, no certificate attempted yet |
//! | `valid` | A certificate is installed and not close to expiry |
//! | `expiring` | Installed, but inside the renewal window — renewal will retry |
//! | `error` | The last attempt failed; `last_error` carries the CA's reason |
//!
//! Secrets: the ACME account key and the issued private key live under
//! `{DATA_DIRECTORY}/data/acme/` (`0700` dir, `0600` key). `acme.json` holds
//! the account URI, the domain list, the provider *name* and the last status —
//! **never** a token or a private key, so the settings export needs no extra
//! redaction, and the API answers `"set"` for a configured token.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::storage::json::JsonStorage;

mod client;

pub use client::{
    build_jws, clear_pending_run, ecdsa_der_to_raw, load_or_create_account_key,
    load_or_create_account_key_in, load_pending_run, parse_directory, save_pending_run,
    write_private, AccountKey, Cloudflare, Directory, DnsPublisher, IssueOutcome, Issuer,
    OrderStart, PendingRun, ReqwestTransport, Session, Transport, POLL_ATTEMPTS, POLL_DELAY,
};

/// Settings file key -> `{DATA_DIRECTORY}/data/acme.json`.
const STORE_KEY: &str = "acme";

/// Directory holding the account key, the issued key pair and the account URI.
const ACME_DIR: &str = "acme";

/// Directory (under `{DATA_DIRECTORY}/data/`) where the HTTPS listener reads the
/// certificate once automation has issued one. Same file names `crate::tls`
/// resolves in [`crate::tls::TlsMode`] automation mode.
pub const ISSUED_CERT_FILE: &str = "cert.pem";

/// File name of the issued private key next to [`ISSUED_CERT_FILE`].
pub const ISSUED_KEY_FILE: &str = "key.pem";

/// Renew once the certificate has fewer than this many days left. Public CAs
/// issue 90-day certificates and expect renewal after two thirds of the
/// lifetime, i.e. around 30 days left.
pub const RENEW_WHEN_DAYS_LEFT: i64 = 30;

/// How often the worker looks at the certificate. A day is enough: renewal
/// happens a month before expiry, so being a few hours late is irrelevant, and
/// a home box must not poll a CA.
pub const WORKER_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Per-request deadline towards the CA. The DNS propagation wait is separate
/// (see [`DEFAULT_DNS_PROPAGATION_SECONDS`]).
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a DNS-01 TXT record is given to propagate before validation.
pub const DEFAULT_DNS_PROPAGATION_SECONDS: u64 = 60;

/// The longest a single issuance run may take end to end.
pub const ISSUANCE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// `true` while an issuance/renewal run holds the lock, so two callers (the
/// worker and an admin's "Renew now") never drive the same order at once.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// The directory CA used when the admin leaves the directory URL empty.
pub const DEFAULT_DIRECTORY_URL: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// The staging directory, for trying the flow without burning a rate limit.
pub const STAGING_DIRECTORY_URL: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Which challenge proves control of the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ChallengeType {
    /// Answered on the plain-HTTP listener — needs port 80 to reach HomeVault.
    #[default]
    #[serde(rename = "http-01")]
    Http01,
    /// A TXT record published through the provider — works behind NAT.
    #[serde(rename = "dns-01")]
    Dns01,
}

/// DNS provider used to publish the `_acme-challenge` TXT record.
///
/// `manual` is the universal fallback: HomeVault prints the record to create
/// and waits for the admin to confirm, so no API credential is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DnsProvider {
    #[default]
    Manual,
    Cloudflare,
}

impl DnsProvider {
    /// Every provider, in the order the Settings card lists them.
    pub const ALL: &'static [DnsProvider] = &[DnsProvider::Manual, DnsProvider::Cloudflare];

    /// The provider that can publish records on its own (everything but
    /// `manual`).
    pub fn is_automated(self) -> bool {
        !matches!(self, DnsProvider::Manual)
    }

    /// Stable API/enum value.
    pub fn as_str(self) -> &'static str {
        match self {
            DnsProvider::Manual => "manual",
            DnsProvider::Cloudflare => "cloudflare",
        }
    }
}

/// Persisted automation settings (`data/acme.json`).
///
/// Deliberately **not** `#[serde(deny_unknown_fields)]`: a file written by a
/// newer build must stay readable by an older one during a rollback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcmeSettings {
    pub enabled: bool,
    /// Domain the certificate is issued for (no wildcard, no port, no scheme).
    pub domain: String,
    /// Directory URL of the CA. Empty means [`DEFAULT_DIRECTORY_URL`].
    #[serde(default)]
    pub directory_url: String,
    /// Accept the CA's terms (sent once, on account creation).
    #[serde(default)]
    pub tos_agreed: bool,
    /// Contact address the CA may use to warn about expiry.
    #[serde(default)]
    pub contact_email: String,
    #[serde(default)]
    pub challenge: ChallengeType,
    #[serde(default)]
    pub provider: DnsProvider,
    /// Provider API token. Write-only from the API's point of view: `GET`
    /// answers `"set"`, never the value.
    #[serde(default)]
    pub dns_api_token: String,
    /// Written by the worker/handler, read by the UI.
    #[serde(default)]
    pub status: AcmeStatus,
    #[serde(default)]
    pub last_error: Option<String>,
    /// RFC 3339, UTC.
    #[serde(default)]
    pub last_attempt: Option<String>,
    /// RFC 3339, UTC — the expiry of the last successfully installed cert.
    #[serde(default)]
    pub last_success: Option<String>,
    /// ACME account URI (`kid`), set after successful registration. Not a
    /// secret, but meaningless outside this instance.
    #[serde(default)]
    pub account_url: Option<String>,
    /// DNS-01 only, `manual` provider: the record an admin has to create and
    /// the value to publish. Cleared as soon as the challenge is validated.
    #[serde(default)]
    pub pending_dns_record: Option<PendingDnsRecord>,
    /// Seconds to wait for DNS propagation before asking the CA to validate.
    #[serde(default = "default_propagation")]
    pub dns_propagation_seconds: u64,
    /// Consecutive failed attempts, used for the retry backoff. Reset on a
    /// successful issuance.
    #[serde(default)]
    pub consecutive_failures: u32,
}

fn default_propagation() -> u64 {
    DEFAULT_DNS_PROPAGATION_SECONDS
}

impl Default for AcmeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            domain: String::new(),
            directory_url: String::new(),
            tos_agreed: false,
            contact_email: String::new(),
            challenge: ChallengeType::default(),
            provider: DnsProvider::default(),
            dns_api_token: String::new(),
            status: AcmeStatus::default(),
            last_error: None,
            last_attempt: None,
            last_success: None,
            account_url: None,
            pending_dns_record: None,
            dns_propagation_seconds: default_propagation(),
            consecutive_failures: 0,
        }
    }
}

/// One `_acme-challenge` record awaiting manual creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDnsRecord {
    pub name: String,
    pub value: String,
}

/// Lifecycle state shown in the UI. Derived only from what actually happened —
/// never optimistically set before the CA said so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AcmeStatus {
    /// Automation is off.
    #[default]
    Disabled,
    /// On, but no issuance attempted yet.
    Idle,
    /// Pending an admin action (a manual DNS record to create).
    Pending,
    /// Certificate installed, comfortably valid.
    Valid,
    /// Installed but inside the renewal window.
    Expiring,
    /// The last attempt failed; see `last_error`.
    Error,
}

impl AcmeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AcmeStatus::Disabled => "disabled",
            AcmeStatus::Idle => "idle",
            AcmeStatus::Pending => "pending",
            AcmeStatus::Valid => "valid",
            AcmeStatus::Expiring => "expiring",
            AcmeStatus::Error => "error",
        }
    }
}

// --------------------------------------------------------------------------
// Paths
// --------------------------------------------------------------------------

fn store() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// `{DATA_DIRECTORY}/data/acme/` — account key, issued pair, account URI.
pub fn acme_dir() -> PathBuf {
    crate::config::paths::data_dir().join(ACME_DIR)
}

/// `{DATA_DIRECTORY}/data/acme/tls/` — where the listener looks for the issued
/// certificate once automation owns TLS.
pub fn issued_dir() -> PathBuf {
    acme_dir().join("tls")
}

/// Path of the issued certificate (`data/acme/tls/cert.pem`).
pub fn issued_cert_path() -> PathBuf {
    issued_dir().join(ISSUED_CERT_FILE)
}

/// Path of the issued private key (`data/acme/tls/key.pem`, `0600`).
pub fn issued_key_path() -> PathBuf {
    issued_dir().join(ISSUED_KEY_FILE)
}

/// Path of the ACME account key (`data/acme/account.key`, `0600`).
pub fn account_key_path() -> PathBuf {
    acme_dir().join("account.key")
}

// --------------------------------------------------------------------------
// Persistence
// --------------------------------------------------------------------------

/// Load the saved automation settings, or the disabled default.
///
/// A corrupt `acme.json` degrades to "disabled" with a warning instead of
/// failing: a broken automation file must never stop the web server.
pub fn load_settings() -> AcmeSettings {
    match store().load::<AcmeSettings>(STORE_KEY) {
        Ok(Some(s)) => s,
        Ok(None) => AcmeSettings::default(),
        Err(e) => {
            tracing::warn!("ACME: cannot read data/acme.json, treating it as disabled: {e}");
            AcmeSettings::default()
        }
    }
}

pub fn save_settings(s: &AcmeSettings) -> Result<(), String> {
    store().save(STORE_KEY, s).map_err(|e| e.to_string())
}

/// Delete the persisted automation settings (Settings → reset).
pub fn clear_settings() {
    let _ = store().delete(STORE_KEY);
    client::clear_pending_run();
}

/// Whether automation currently owns the HTTPS certificate. The listener asks
/// this at startup; it is true only when automation is enabled **and** a
/// certificate has actually been issued, so a failed first run keeps serving
/// the previous (self-signed or manual) certificate instead of nothing.
pub fn automation_active() -> bool {
    let s = load_settings();
    s.enabled
        && !s.domain.trim().is_empty()
        && issued_cert_path().exists()
        && issued_key_path().exists()
}

/// Recompute [`AcmeSettings::status`] from what is actually on disk, and
/// report whether it changed.
///
/// The status is never set optimistically: `valid` and `expiring` come from
/// reading the installed certificate, `pending` from a persisted manual run,
/// and `error` from a recorded failure. That is what makes the badge on the
/// Settings card trustworthy.
pub fn refresh_status(settings: &mut AcmeSettings) -> bool {
    let before = settings.status;
    let next = if !settings.enabled {
        AcmeStatus::Disabled
    } else if load_pending_run().is_some() {
        AcmeStatus::Pending
    } else if issued_cert_path().exists() && issued_key_path().exists() {
        match crate::tls::cert_days_remaining(&issued_cert_path()) {
            Some(days) if days > RENEW_WHEN_DAYS_LEFT => AcmeStatus::Valid,
            // Expired counts as expiring: both mean "renew now", and the UI
            // says "expiring" rather than pretending the certificate is fine.
            Some(_) => AcmeStatus::Expiring,
            None => AcmeStatus::Error,
        }
    } else if settings.last_error.is_some() {
        AcmeStatus::Error
    } else {
        AcmeStatus::Idle
    };
    settings.status = next;
    before != next
}

/// Whether the worker should attempt an issuance right now.
///
/// A missing certificate is always worth an attempt; an installed one only
/// inside the renewal window. A pending manual run blocks the worker — the
/// admin still has to create the record — and a recent failure backs off so a
/// permanently broken configuration does not hammer the CA.
pub fn should_renew(settings: &AcmeSettings) -> bool {
    if !settings.enabled {
        return false;
    }
    if load_pending_run().is_some() {
        return false;
    }
    let due = if issued_cert_path().exists() && issued_key_path().exists() {
        crate::tls::cert_days_remaining(&issued_cert_path())
            .map(|days| days <= RENEW_WHEN_DAYS_LEFT)
            .unwrap_or(true)
    } else {
        true
    };
    if !due {
        return false;
    }
    if settings.consecutive_failures == u32::MAX {
        // A final error (the CA refused the domain) does not fix itself: wait
        // for the admin to change something, which resets the counter.
        return false;
    }
    if settings.consecutive_failures == 0 {
        return true;
    }
    let wait = retry_delay_seconds(settings.consecutive_failures);
    match settings.last_attempt.as_deref().and_then(parse_rfc3339_utc) {
        Some(last) => {
            let elapsed = time::OffsetDateTime::now_utc().unix_timestamp() - last;
            elapsed >= wait as i64
        }
        // No usable timestamp: the last attempt is unknown, so try.
        None => true,
    }
}

/// Parse an RFC 3339 UTC timestamp into a Unix second count.
fn parse_rfc3339_utc(value: &str) -> Option<i64> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

/// Parse an RFC 3339 UTC timestamp into a Unix second count.
///
/// Public because other timestamped settings (dynamic DNS) store the same
/// format and must age it the same way instead of re-implementing the parser.
pub fn parse_rfc3339_seconds(value: &str) -> Option<i64> {
    parse_rfc3339_utc(value)
}

/// Record the outcome of an attempt: status, timestamps, error and backoff.
pub fn record_success(settings: &mut AcmeSettings) {
    settings.status = AcmeStatus::Valid;
    settings.last_error = None;
    settings.consecutive_failures = 0;
    settings.last_success = Some(now_rfc3339());
    settings.last_attempt = settings.last_success.clone();
    settings.pending_dns_record = None;
}

/// Record a failure. `retryable` decides whether the worker keeps trying or
/// waits for the admin to fix the configuration.
pub fn record_failure(settings: &mut AcmeSettings, message: &str, retryable: bool) {
    settings.status = AcmeStatus::Error;
    settings.last_error = Some(message.to_string());
    settings.last_attempt = Some(now_rfc3339());
    // A final error still counts as a failure for the backoff, but the worker
    // stops retrying on its own (see `should_renew` + the worker's own check).
    settings.consecutive_failures = if retryable {
        settings.consecutive_failures.saturating_add(1)
    } else {
        u32::MAX
    };
}

/// Current time as RFC 3339 in UTC, the format every timestamp here uses.
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// --------------------------------------------------------------------------
// Validation
// --------------------------------------------------------------------------

/// A domain name acceptable for a certificate request: DNS labels only, at
/// least two of them, no wildcard, no trailing dot, no scheme, no port.
pub fn validate_domain(domain: &str) -> Result<String, String> {
    let d = domain.trim().to_ascii_lowercase();
    if d.is_empty() {
        return Err("a domain name is required".to_string());
    }
    if d.len() > 253 {
        return Err("the domain name is too long".to_string());
    }
    if d.contains('*') {
        return Err("wildcard domains are not supported; enter the exact name".to_string());
    }
    if d.contains('/') || d.contains(':') {
        return Err("enter a bare host name, without a scheme, port or path".to_string());
    }
    if d.ends_with('.') {
        return Err("do not end the name with a dot".to_string());
    }
    let labels: Vec<&str> = d.split('.').collect();
    if labels.len() < 2 {
        return Err("the name needs at least one dot (for example nas.example.com)".to_string());
    }
    for label in labels {
        if label.is_empty() || label.len() > 63 {
            return Err("each part of the name must be 1 to 63 characters".to_string());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("parts of the name cannot start or end with a hyphen".to_string());
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("the name may only contain letters, digits, hyphens and dots".to_string());
        }
    }
    if d.parse::<std::net::IpAddr>().is_ok() {
        return Err("enter a name, not an IP address".to_string());
    }
    Ok(d)
}

/// An e-mail address that can be sent to a CA as a contact. Deliberately
/// permissive — the CA validates it again — but it must not be able to inject
/// anything into a `mailto:` URI.
pub fn validate_contact_email(email: &str) -> Result<(), String> {
    let e = email.trim();
    if e.is_empty() {
        return Err("a contact address is required".to_string());
    }
    if e.contains("mailto:") || e.contains(':') || e.chars().any(char::is_whitespace) {
        return Err("enter a bare e-mail address".to_string());
    }
    let (local, domain) = e
        .split_once('@')
        .ok_or_else(|| "enter a valid e-mail address".to_string())?;
    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return Err("enter a valid e-mail address".to_string());
    }
    Ok(())
}

/// A CA directory URL: HTTPS only. Plain HTTP would let anyone on the path
/// stand in for the CA and vouch for a certificate they hold the key to.
pub fn validate_directory_url(url: &str) -> Result<String, String> {
    let u = url.trim();
    if u.is_empty() {
        return Ok(DEFAULT_DIRECTORY_URL.to_string());
    }
    if !u.starts_with("https://") {
        return Err("the certificate authority URL must start with https://".to_string());
    }
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the certificate authority URL contains whitespace".to_string());
    }
    Ok(u.to_string())
}

/// Validate a proposed settings change. `http_port` is unused today but kept so
/// a future "offer to move the listener to 80" check has the value it needs.
pub fn validate_settings(new: &AcmeSettings, _http_port: u16) -> Result<(), String> {
    validate_domain(&new.domain)?;
    validate_contact_email(&new.contact_email)?;
    validate_directory_url(&new.directory_url)?;
    if !new.tos_agreed {
        return Err("the certificate authority's terms must be accepted".to_string());
    }
    if new.challenge == ChallengeType::Dns01
        && new.provider.is_automated()
        && new.dns_api_token.trim().is_empty()
    {
        return Err("this DNS provider needs an API token".to_string());
    }
    if new.dns_propagation_seconds == 0 {
        return Err("the DNS propagation wait must be at least 1 second".to_string());
    }
    if new.dns_propagation_seconds > 3600 {
        return Err("the DNS propagation wait cannot exceed 3600 seconds".to_string());
    }
    Ok(())
}

/// Strip every secret from settings before they leave the process.
///
/// This is the **only** way settings reach an HTTP response: `dns_api_token`
/// becomes `"set"`/`""`, so a token can be overwritten but never read back.
pub fn redacted(mut s: AcmeSettings) -> AcmeSettings {
    s.dns_api_token = if s.dns_api_token.trim().is_empty() {
        String::new()
    } else {
        "set".to_string()
    };
    s
}

/// Origin the CA will connect to when validating with HTTP-01.
pub fn http01_origin(domain: &str) -> String {
    format!("http://{domain}")
}

// --------------------------------------------------------------------------
// HTTP-01 challenge registry
// --------------------------------------------------------------------------

/// One challenge answer the plain-HTTP listener must serve:
/// `http://<domain>/.well-known/acme-challenge/<token>` → `<key_authorization>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChallenge {
    pub token: String,
    pub key_authorization: String,
}

/// The key authorizations currently being validated.
///
/// Bounded on purpose: a CA never validates more than a handful of names at
/// once, and an unbounded map reachable by the validation endpoint would be a
/// memory-growth lever for anyone who can reach the box.
#[derive(Debug, Default)]
pub struct ChallengeRegistry {
    entries: BTreeMap<String, String>,
}

/// More than this many live challenges means the caller is not an ACME
/// validation — refuse instead of growing.
pub const MAX_PENDING_CHALLENGES: usize = 8;

impl ChallengeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an answer. Refuses an invalid token or an overfull registry.
    pub fn insert(&mut self, token: &str, key_authorization: &str) -> Result<(), String> {
        if !is_valid_token(token) {
            return Err("invalid challenge token".to_string());
        }
        if key_authorization.trim().is_empty() || key_authorization.len() > 512 {
            return Err("invalid key authorization".to_string());
        }
        if !self.entries.contains_key(token) && self.entries.len() >= MAX_PENDING_CHALLENGES {
            return Err("too many challenge validations in progress".to_string());
        }
        self.entries
            .insert(token.to_string(), key_authorization.to_string());
        Ok(())
    }

    /// The answer for a token, if one is pending.
    pub fn get(&self, token: &str) -> Option<&String> {
        self.entries.get(token)
    }

    /// Drop one answer (after validation, whether it succeeded or not).
    pub fn remove(&mut self, token: &str) {
        self.entries.remove(token);
    }

    /// Drop every answer (on failure or shutdown).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The process-wide registry the plain-HTTP challenge route reads.
///
/// One instance for the whole process because the route handler and the
/// issuance task must see the same map, and the route has no access to the
/// settings structure.
static REGISTRY: OnceLock<Mutex<ChallengeRegistry>> = OnceLock::new();

fn registry() -> &'static Mutex<ChallengeRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(ChallengeRegistry::new()))
}

/// Publish a challenge answer for the CA to fetch.
pub fn register_challenge(token: &str, key_authorization: &str) -> Result<(), String> {
    let mut registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.insert(token, key_authorization)
}

/// The answer for `token`, if a validation is in progress.
pub fn challenge_answer(token: &str) -> Option<String> {
    let registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.get(token).cloned()
}

/// Drop one answer once the CA is done with it.
pub fn remove_challenge(token: &str) {
    let mut registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.remove(token);
}

/// Drop every pending answer (shutdown, settings reset).
pub fn clear_challenges() {
    let mut registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry.clear();
}

/// ACME tokens are base64url without padding; require the shape so a path
/// segment cannot be used to read or hash anything else.
pub fn is_valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The router path prefix the challenge route is mounted under.
pub const CHALLENGE_PATH_PREFIX: &str = "/.well-known/acme-challenge/";

/// Extract the token from a request URI, or `None` when the URI is not a
/// challenge request (the caller falls through to the normal router).
pub fn challenge_token_from_uri(uri: &str) -> Option<&str> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    path.strip_prefix(CHALLENGE_PATH_PREFIX)
        .filter(|token| is_valid_token(token))
}

// --------------------------------------------------------------------------
// Issuance (RFC 8555)
// --------------------------------------------------------------------------

/// Outcome of one issuance attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCertificate {
    /// PEM of the issued leaf (plus intermediates, if the CA sent them).
    pub cert_pem: String,
    /// RFC 3339 expiry of the leaf.
    pub not_after: String,
}

/// Anything that can go wrong talking to a CA. The message is shown to the
/// admin verbatim — it is the CA's own reason, which is what makes a failure
/// actionable — so it must never contain a secret (see [`describe_problem`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeError {
    pub message: String,
    /// `true` when retrying later could succeed (network, rate limit, DNS not
    /// propagated yet) — the worker keeps retrying; a validation refusal does
    /// not fix itself and is reported as final.
    pub retryable: bool,
}

impl AcmeError {
    pub fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self {
            message: message.into(),
            retryable,
        }
    }

    pub fn final_error(message: impl Into<String>) -> Self {
        Self::new(message, false)
    }
}

impl std::fmt::Display for AcmeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Turn an ACME problem document (`application/problem+json`) into a single
/// human sentence. Unknown shapes fall back to the HTTP status and the raw
/// `detail`, never to a dump of the whole document.
pub fn describe_problem(status: u16, body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let detail = value
        .get("detail")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let kind = value
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_string();
    match (kind.is_empty(), detail.is_empty()) {
        (false, false) => format!("{detail} ({kind})"),
        (false, true) => format!("the certificate authority refused with {status} ({kind})"),
        (true, false) => format!("{detail} ({status})"),
        (true, true) => format!("the certificate authority answered {status}"),
    }
}

/// Whether an HTTP status from the CA means "try again later".
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}

/// Path component the HTTP-01 route is matched on, ready to be mounted.
pub fn challenge_route_path() -> String {
    format!("{CHALLENGE_PATH_PREFIX}{{token}}")
}

/// Number of seconds to sleep before the nth retry of a failed attempt:
/// 1 min, 5 min, 30 min, then hourly — capped, so a CA outage does not turn
/// into a tight retry loop and a permanently broken configuration does not
/// hammer the CA.
pub fn retry_delay_seconds(attempt: u32) -> u64 {
    match attempt {
        0 | 1 => 60,
        2 => 300,
        3 => 1800,
        _ => 3600,
    }
}

/// The DNS record name a DNS-01 validation expects for `domain`.
pub fn challenge_record_name(domain: &str) -> String {
    let d = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    format!("_acme-challenge.{d}")
}

/// The ACME TXT value for a DNS-01 challenge: base64url (no padding) of
/// SHA-256 of the key authorization — the form RFC 8555 §8.4 specifies.
pub fn dns01_txt_value(key_authorization: &str) -> String {
    let digest = sha256(key_authorization.as_bytes());
    base64url_nopad(&digest)
}

/// Minimal, dependency-free SHA-256 (FIPS 180-4) for the DNS-01 digest. Kept
/// here rather than pulling `sha2` in for one 32-byte hash the crate already
/// computes elsewhere, so this module stays testable in isolation.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let (blocks, _) = message.as_chunks::<64>();
    for chunk in blocks {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// base64url without padding (RFC 7515 §2), as ACME uses for TXT values.
pub fn base64url_nopad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 0x3f] as char);
        out.push(TABLE[(n >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 0x3f] as char);
        }
    }
    out
}

/// Take the single issuance lock. Returns `false` when a run is already in
/// progress — the caller reports "already running" instead of queueing a
/// second order against the same domain.
pub fn try_begin_run() -> bool {
    !RUNNING.swap(true, Ordering::SeqCst)
}

/// Release the issuance lock. Always called, including on error paths.
pub fn end_run() {
    RUNNING.store(false, Ordering::SeqCst);
}

/// Whether an issuance run is in progress right now.
pub fn run_in_progress() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Directory the worker reads its settings from — exposed for tests that need
/// to point the module at a scratch tree.
pub fn settings_file_path() -> PathBuf {
    crate::config::paths::data_dir().join(format!("{STORE_KEY}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_domain_accepts_a_normal_name() {
        assert_eq!(
            validate_domain("NAS.Example.com").unwrap(),
            "nas.example.com"
        );
        assert_eq!(
            validate_domain(" nas.example.co.uk ").unwrap(),
            "nas.example.co.uk"
        );
    }

    #[test]
    fn test_validate_domain_rejects_scheme_port_and_path() {
        assert!(validate_domain("https://nas.example.com").is_err());
        assert!(validate_domain("nas.example.com:443").is_err());
        assert!(validate_domain("nas.example.com/ui").is_err());
    }

    #[test]
    fn test_validate_domain_rejects_wildcard_and_trailing_dot() {
        assert!(validate_domain("*.example.com").is_err());
        assert!(validate_domain("nas.example.com.").is_err());
    }

    #[test]
    fn test_validate_domain_rejects_single_label_and_ip() {
        assert!(validate_domain("localhost").is_err());
        assert!(validate_domain("192.168.1.10").is_err());
        assert!(validate_domain("").is_err());
    }

    #[test]
    fn test_validate_domain_rejects_bad_labels() {
        assert!(validate_domain("-bad.example.com").is_err());
        assert!(validate_domain("bad-.example.com").is_err());
        assert!(validate_domain("exa mple.com").is_err());
        assert!(validate_domain("exa_mple.com").is_err());
        assert!(validate_domain("a..b.com").is_err());
        let long = format!("{}.example.com", "a".repeat(64));
        assert!(validate_domain(&long).is_err());
    }

    #[test]
    fn test_validate_contact_email_rejects_injection_and_bad_shapes() {
        assert!(validate_contact_email("admin@example.com").is_ok());
        assert!(validate_contact_email("mailto:admin@example.com").is_err());
        assert!(validate_contact_email("admin@example.com extra").is_err());
        assert!(validate_contact_email("admin").is_err());
        assert!(validate_contact_email("@example.com").is_err());
        assert!(validate_contact_email("").is_err());
    }

    #[test]
    fn test_validate_directory_url_requires_https() {
        assert_eq!(
            validate_directory_url("").unwrap(),
            DEFAULT_DIRECTORY_URL.to_string()
        );
        assert!(validate_directory_url("http://ca.example/dir").is_err());
        assert!(validate_directory_url("https://ca.example/dir").is_ok());
        assert!(validate_directory_url("https://ca.example/a b").is_err());
    }

    #[test]
    fn test_validate_settings_requires_tos_and_token_only_when_needed() {
        let mut s = AcmeSettings {
            enabled: true,
            domain: "nas.example.com".to_string(),
            contact_email: "admin@example.com".to_string(),
            ..Default::default()
        };
        assert!(
            validate_settings(&s, 8080).is_err(),
            "terms must be accepted"
        );

        s.tos_agreed = true;
        assert!(validate_settings(&s, 8080).is_ok());

        s.challenge = ChallengeType::Dns01;
        s.provider = DnsProvider::Manual;
        assert!(
            validate_settings(&s, 8080).is_ok(),
            "the manual provider needs no token"
        );

        s.provider = DnsProvider::Cloudflare;
        assert!(validate_settings(&s, 8080).is_err());
        s.dns_api_token = "token".to_string();
        assert!(validate_settings(&s, 8080).is_ok());
    }

    #[test]
    fn test_validate_settings_bounds_the_propagation_wait() {
        let mut s = AcmeSettings {
            enabled: true,
            domain: "nas.example.com".to_string(),
            contact_email: "admin@example.com".to_string(),
            tos_agreed: true,
            ..Default::default()
        };
        s.dns_propagation_seconds = 0;
        assert!(validate_settings(&s, 8080).is_err());
        s.dns_propagation_seconds = 3601;
        assert!(validate_settings(&s, 8080).is_err());
        s.dns_propagation_seconds = 3600;
        assert!(validate_settings(&s, 8080).is_ok());
    }

    #[test]
    fn test_redacted_never_returns_the_token() {
        let s = AcmeSettings {
            dns_api_token: "super-secret".to_string(),
            ..Default::default()
        };
        let r = redacted(s);
        assert_eq!(r.dns_api_token, "set");
        assert!(!serde_json::to_string(&r).unwrap().contains("super-secret"));
    }

    #[test]
    fn test_redacted_empty_token_stays_empty() {
        let s = AcmeSettings {
            dns_api_token: "  ".to_string(),
            ..Default::default()
        };
        assert_eq!(redacted(s).dns_api_token, "");
    }

    #[test]
    fn test_challenge_registry_accepts_and_answers_one_token() {
        let mut r = ChallengeRegistry::new();
        r.insert("abc-_123", "token.thumbprint").unwrap();
        assert_eq!(r.get("abc-_123").unwrap(), "token.thumbprint");
        assert_eq!(r.len(), 1);
        r.remove("abc-_123");
        assert!(r.get("abc-_123").is_none());
        assert!(r.is_empty());
    }

    #[test]
    fn test_challenge_registry_rejects_bad_tokens() {
        let mut r = ChallengeRegistry::new();
        assert!(r.insert("bad/token", "value").is_err());
        assert!(r.insert("../escape", "value").is_err());
        assert!(r.insert("", "value").is_err());
        assert!(r.insert("ok", "").is_err());
        assert!(r.insert("ok", &"a".repeat(600)).is_err());
        assert!(r.is_empty());
    }

    #[test]
    fn test_challenge_registry_is_bounded() {
        let mut r = ChallengeRegistry::new();
        for i in 0..MAX_PENDING_CHALLENGES {
            r.insert(&format!("token{i}"), "value").unwrap();
        }
        assert!(r.insert("one-too-many", "value").is_err());
        // Re-registering an existing token is not growth.
        assert!(r.insert("token0", "value").is_ok());
        assert_eq!(r.len(), MAX_PENDING_CHALLENGES);
    }

    #[test]
    fn test_challenge_token_from_uri() {
        assert_eq!(
            challenge_token_from_uri("/.well-known/acme-challenge/abc"),
            Some("abc")
        );
        assert_eq!(
            challenge_token_from_uri("/.well-known/acme-challenge/abc?x=1"),
            Some("abc")
        );
        assert_eq!(
            challenge_token_from_uri("/.well-known/acme-challenge/a/b"),
            None
        );
        assert_eq!(challenge_token_from_uri("/api/auth/login"), None);
        assert_eq!(
            challenge_token_from_uri("/.well-known/acme-challenge/"),
            None
        );
        assert_eq!(
            challenge_token_from_uri("/.well-known/acme-challenge/%2e%2e"),
            None
        );
    }

    #[test]
    fn test_is_valid_token_shape() {
        assert!(is_valid_token("abc-_123"));
        assert!(!is_valid_token(""));
        assert!(!is_valid_token("abc.def"));
        assert!(!is_valid_token(&"a".repeat(129)));
    }

    #[test]
    fn test_sha256_matches_known_vectors() {
        // FIPS / RFC test vectors, so the digest cannot drift silently.
        assert_eq!(
            base64url_nopad(&sha256(b"")),
            "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"
        );
        assert_eq!(
            base64url_nopad(&sha256(b"abc")),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
        assert_eq!(
            base64url_nopad(&sha256(&vec![b'a'; 1_000_000])),
            "zcduXJkU-5KBocfihNc-Z_GAmkiklyAOBG05zMcRLNA"
        );
    }

    #[test]
    fn test_base64url_nopad_padding_cases() {
        assert_eq!(base64url_nopad(b""), "");
        assert_eq!(base64url_nopad(b"f"), "Zg");
        assert_eq!(base64url_nopad(b"fo"), "Zm8");
        assert_eq!(base64url_nopad(b"foo"), "Zm9v");
        assert_eq!(base64url_nopad(b"foob"), "Zm9vYg");
        assert_eq!(base64url_nopad(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn test_dns01_txt_value_is_the_sha256_of_the_key_authorization() {
        let value = dns01_txt_value("token.thumbprint");
        assert_eq!(value, base64url_nopad(&sha256(b"token.thumbprint")));
        assert!(!value.contains('='), "TXT values carry no padding");
        assert!(value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_challenge_record_name() {
        assert_eq!(
            challenge_record_name("NAS.Example.com."),
            "_acme-challenge.nas.example.com"
        );
    }

    #[test]
    fn test_http01_origin() {
        assert_eq!(http01_origin("nas.example.com"), "http://nas.example.com");
    }

    #[test]
    fn test_describe_problem_prefers_detail_and_kind() {
        let body = r#"{"type":"urn:ietf:params:acme:error:unauthorized","detail":"Invalid response from http://x","status":403}"#;
        assert_eq!(
            describe_problem(403, body),
            "Invalid response from http://x (unauthorized)"
        );
        let bare = r#"{"type":"urn:ietf:params:acme:error:rateLimited"}"#;
        assert_eq!(
            describe_problem(429, bare),
            "the certificate authority refused with 429 (rateLimited)"
        );
        assert_eq!(
            describe_problem(500, "not json"),
            "the certificate authority answered 500"
        );
    }

    #[test]
    fn test_is_retryable_status() {
        for s in [429u16, 500, 502, 503, 504] {
            assert!(is_retryable_status(s), "{s} must be retryable");
        }
        for s in [200u16, 400, 401, 403, 404] {
            assert!(!is_retryable_status(s), "{s} must not be retryable");
        }
    }

    #[test]
    fn test_retry_delay_grows_then_caps() {
        assert_eq!(retry_delay_seconds(1), 60);
        assert_eq!(retry_delay_seconds(2), 300);
        assert_eq!(retry_delay_seconds(3), 1800);
        assert_eq!(retry_delay_seconds(10), 3600);
        assert_eq!(retry_delay_seconds(u32::MAX), 3600);
    }

    #[test]
    fn test_status_strings_are_stable() {
        assert_eq!(AcmeStatus::Disabled.as_str(), "disabled");
        assert_eq!(AcmeStatus::Idle.as_str(), "idle");
        assert_eq!(AcmeStatus::Pending.as_str(), "pending");
        assert_eq!(AcmeStatus::Valid.as_str(), "valid");
        assert_eq!(AcmeStatus::Expiring.as_str(), "expiring");
        assert_eq!(AcmeStatus::Error.as_str(), "error");
    }

    #[test]
    fn test_settings_roundtrip_through_json() {
        let s = AcmeSettings {
            enabled: true,
            domain: "nas.example.com".to_string(),
            challenge: ChallengeType::Dns01,
            provider: DnsProvider::Cloudflare,
            status: AcmeStatus::Valid,
            ..Default::default()
        };
        let text = serde_json::to_string(&s).unwrap();
        let back: AcmeSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);
        assert!(text.contains("\"dns-01\""));
        assert!(text.contains("\"cloudflare\""));
    }

    #[test]
    fn test_a_legacy_settings_file_reads_with_safe_defaults() {
        let legacy = r#"{"enabled":true,"domain":"nas.example.com"}"#;
        let parsed: AcmeSettings = serde_json::from_str(legacy).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.challenge, ChallengeType::Http01);
        assert_eq!(parsed.provider, DnsProvider::Manual);
        assert_eq!(
            parsed.dns_propagation_seconds,
            DEFAULT_DNS_PROPAGATION_SECONDS
        );
        assert!(!parsed.tos_agreed);
    }

    #[test]
    fn test_run_lock_is_exclusive() {
        // The lock is process-global; this test owns it and always releases it.
        if run_in_progress() {
            end_run();
        }
        assert!(try_begin_run(), "first caller takes the lock");
        assert!(!try_begin_run(), "a second concurrent run is refused");
        end_run();
        assert!(try_begin_run(), "the lock is reusable after release");
        end_run();
    }

    #[test]
    fn test_challenge_route_path_is_a_single_segment() {
        assert_eq!(
            challenge_route_path(),
            "/.well-known/acme-challenge/{token}"
        );
    }
}
