/* =============================================================================
 * File          : oidc.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   OpenID Connect sign-in (ROADMAP P3.1, stage 2). An admin points HomeVault at
 *   an identity provider (Keycloak, Authentik, Authelia, Entra ID, Google…),
 *   registers the callback URL once, and people sign in with the account they
 *   already have there. LDAP and OIDC share the same rules: a remote principal
 *   is mirrored, never merged into a local account, and the group→role mapping
 *   is the same code path.
 *
 *   Everything except the three `fetch_*`/`exchange_*` helpers is pure: URL,
 *   scope and claim validation, PKCE, the bounded pending-login store and the
 *   whole ID-token verification are unit-tested without a network. The HTTP
 *   calls are exercised end to end against a fake provider in
 *   `tests/oidc_integration.rs`.
 *
 * Dependencies:
 *   - ring (already used by `passkey`), base64, reqwest, serde, serde_json, url
 *
 * Usage:
 *   `handlers::oidc` exposes the endpoints; see docs/guides/directory-sso.md.
 * =============================================================================
 */

//! OpenID Connect sign-in (see docs/guides/directory-sso.md).
//!
//! Rules this module is built around:
//!
//! * **The browser is not trusted to say who it is.** The authorization code is
//!   exchanged server-side, the ID token's signature is verified against the
//!   provider's published keys, and `iss`/`aud`/`exp`/`nonce` are all checked
//!   before a single claim is read.
//! * **No algorithm confusion.** A token's `alg` must be one this module can
//!   verify *and* must match the key type it is checked against. `none` and
//!   HS256 are refused outright — never "fall back" to a weaker check.
//! * **PKCE everywhere.** Even though the client has a secret, every flow uses
//!   S256, so an intercepted code cannot be redeemed without the verifier.
//! * **Bounded everything.** Pending sign-ins expire after ten minutes and are
//!   capped; the discovery and key caches are capped and time-limited; claim
//!   values are length-checked before they reach the database.
//! * **A remote principal is not a local account**, exactly like LDAP: the
//!   mirrored `users` row carries a sentinel hash and the login path is chosen
//!   by `users.auth_source`, never by the request.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};

use crate::directory::{normalize_role, GroupRole};
use crate::storage::json::JsonStorage;

/// The provider name stored in `remote_identities.provider`.
pub const PROVIDER_OIDC: &str = "oidc";

/// The one callback route the identity provider is registered against.
pub const CALLBACK_PATH: &str = "/api/auth/oidc/callback";

/// The cookie that binds a pending sign-in to the browser that started it, so
/// a link an attacker crafted cannot complete a flow on someone else's behalf.
pub const STATE_COOKIE: &str = "hv_oidc_state";

/// Where the settings document and the sealed client secret live.
pub const STORE_KEY: &str = "oidc";
pub const CLIENT_SECRET_SCOPE: &str = "oidc-client-secret";

/// Bounds, enforced on save *and* on load: a hand-edited JSON file must not be
/// able to widen them.
pub const MAX_LABEL_LEN: usize = 64;
pub const MAX_ISSUER_LEN: usize = 512;
pub const MAX_CLIENT_ID_LEN: usize = 256;
pub const MAX_REDIRECT_LEN: usize = 1024;
pub const MAX_SCOPES_LEN: usize = 512;
pub const MAX_SCOPE_TOKEN_LEN: usize = 128;
pub const MAX_GROUP_ROLES: usize = 64;
pub const MAX_GROUP_NAME_LEN: usize = 1024;
pub const MAX_GROUPS_STORED: usize = 256;

/// How long a started sign-in stays redeemable, and how many may be pending.
pub const PENDING_TTL: Duration = Duration::from_secs(600);
pub const MAX_PENDING: usize = 256;

/// Discovery documents and signing keys change rarely; one hour keeps a login
/// path fast without pinning a rotated key forever.
pub const METADATA_TTL: Duration = Duration::from_secs(3600);
const MAX_CACHED_PROVIDERS: usize = 8;

/// Nothing here is allowed to hang a login screen.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// How far a provider's clock may be from ours before an otherwise valid token
/// is refused.
pub const CLOCK_SKEW_SECONDS: i64 = 120;

/// The default label on the sign-in button.
pub const DEFAULT_LABEL: &str = "Single sign-on";

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Everything the OIDC integration needs, persisted as one JSON document under
/// `{DATA_DIRECTORY}/data/oidc.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OidcSettings {
    /// Master switch. Off means no OIDC sign-in at all; local accounts are
    /// unaffected either way.
    pub enabled: bool,
    /// What the sign-in button says ("Sign in with Authentik").
    pub label: String,
    /// The issuer exactly as the provider reports it, e.g.
    /// `https://auth.example.com/application/o/homevault`.
    pub issuer: String,
    /// The client (application) identifier registered at the provider.
    pub client_id: String,
    /// The client secret, sealed. `None` means "public client" (PKCE only).
    pub client_secret_sealed: Option<String>,
    /// The exact callback URL registered at the provider. Never derived from
    /// the `Host` header: a forged header must not choose where a code goes.
    pub redirect_url: String,
    /// Space-separated scopes; must contain `openid`.
    pub scopes: String,
    /// Claim holding the login name, and the fallbacks.
    pub attr_username: String,
    pub attr_display_name: String,
    pub attr_email: String,
    pub attr_groups: String,
    /// Role for a principal no mapped group claims.
    pub default_role: String,
    /// Group → role table. The highest role wins.
    pub group_roles: Vec<GroupRole>,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
}

impl Default for OidcSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            label: DEFAULT_LABEL.to_string(),
            issuer: String::new(),
            client_id: String::new(),
            client_secret_sealed: None,
            redirect_url: String::new(),
            scopes: "openid profile email".to_string(),
            attr_username: "preferred_username".to_string(),
            attr_display_name: "name".to_string(),
            attr_email: "email".to_string(),
            attr_groups: "groups".to_string(),
            default_role: crate::directory::ROLE_USER.to_string(),
            group_roles: Vec::new(),
            updated_at: None,
            updated_by: None,
        }
    }
}

impl OidcSettings {
    /// Whether a sign-in can even be attempted.
    pub fn usable(&self) -> bool {
        self.enabled && validate(self).is_ok()
    }

    /// Whether the flow needs a client secret (a confidential client).
    pub fn has_client_secret(&self) -> bool {
        self.client_secret_sealed.is_some()
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Whether a host is the loopback interface. Plain HTTP is tolerated there and
/// nowhere else: a secret sent to a LAN address would be readable in transit.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Whether a URL is an endpoint we are willing to send a browser or a secret
/// to: HTTPS, or plain HTTP to the loopback interface only.
fn valid_endpoint(url: &str) -> bool {
    let url = url.trim();
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => is_loopback_host(host),
        _ => false,
    }
}

/// Whether a string is a claim name we are willing to read.
///
/// OIDC claim names are not LDAP attribute descriptions: `preferred_username`
/// and `given_name` are the common ones, so the underscore must be allowed
/// (the directory validator deliberately refuses it).
pub fn valid_claim_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Whether the issuer is a usable HTTPS URL with no query, fragment or
/// credentials embedded in it.
pub fn valid_issuer(issuer: &str) -> bool {
    let issuer = issuer.trim();
    if issuer.is_empty() || issuer.len() > MAX_ISSUER_LEN {
        return false;
    }
    let Ok(parsed) = url::Url::parse(issuer) else {
        return false;
    };
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return false;
    }
    valid_endpoint(issuer)
}

/// Whether the callback URL is the one this build actually serves.
///
/// Requiring the exact suffix catches the usual paste error (the provider's
/// *login* URL instead of the callback) before a single sign-in fails at the
/// identity provider with an opaque redirect-URI mismatch.
pub fn valid_redirect_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_REDIRECT_LEN {
        return false;
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return false;
    }
    valid_endpoint(url) && parsed.path().ends_with(CALLBACK_PATH)
}

/// Whether a scope string is well formed and asks for `openid`.
///
/// Without `openid` an OIDC provider is just an OAuth server and returns no ID
/// token, which would fail after the user has already typed their password.
pub fn valid_scopes(scopes: &str) -> bool {
    let scopes = scopes.trim();
    if scopes.is_empty() || scopes.len() > MAX_SCOPES_LEN {
        return false;
    }
    let tokens: Vec<&str> = scopes.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    let mut has_openid = false;
    for token in &tokens {
        if token.len() > MAX_SCOPE_TOKEN_LEN {
            return false;
        }
        // RFC 6749 §3.3: printable ASCII minus space, `"` and `\`.
        if !token
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
        {
            return false;
        }
        if *token == "openid" {
            has_openid = true;
        }
    }
    has_openid
}

/// Validate a whole settings document.
///
/// Returns the first problem found, worded so the screen can show it as-is. A
/// disabled integration may be half-filled: the admin may be preparing it, and
/// nothing is contacted until it is switched on.
pub fn validate(settings: &OidcSettings) -> Result<(), String> {
    let label = settings.label.trim();
    if label.is_empty() || label.len() > MAX_LABEL_LEN {
        return Err(format!(
            "The button label must be 1 to {MAX_LABEL_LEN} characters"
        ));
    }
    if label.chars().any(|c| c.is_control()) {
        return Err("The button label contains a control character".to_string());
    }
    if !settings.enabled {
        return Ok(());
    }
    if !valid_issuer(&settings.issuer) {
        return Err(
            "The issuer must be an https:// address with no query string, e.g. https://auth.example.com/application/o/homevault"
                .to_string(),
        );
    }
    let client_id = settings.client_id.trim();
    if client_id.is_empty() || client_id.len() > MAX_CLIENT_ID_LEN {
        return Err("The client ID is required".to_string());
    }
    if client_id.chars().any(|c| c.is_control()) {
        return Err("The client ID contains a control character".to_string());
    }
    if !valid_redirect_url(&settings.redirect_url) {
        return Err(format!(
            "The callback URL must be an https:// address ending in {CALLBACK_PATH}, exactly as registered at the provider"
        ));
    }
    if !valid_scopes(&settings.scopes) {
        return Err(
            "The scopes must be space-separated and include 'openid', e.g. 'openid profile email'"
                .to_string(),
        );
    }
    for (label, claim) in [
        ("login name", &settings.attr_username),
        ("display name", &settings.attr_display_name),
        ("e-mail", &settings.attr_email),
        ("group", &settings.attr_groups),
    ] {
        if !valid_claim_name(claim) {
            return Err(format!(
                "The {label} claim '{}' is not a valid claim name",
                claim.trim()
            ));
        }
    }
    normalize_role(&settings.default_role)?;
    crate::directory::validate_group_roles(&settings.group_roles, MAX_GROUP_ROLES)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PKCE, state and the pending-login store
// ---------------------------------------------------------------------------

/// Base64url without padding, the encoding OAuth uses.
pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode base64url, accepting optional `=` padding (some providers add it).
pub fn b64url_decode(value: &str) -> Option<Vec<u8>> {
    let trimmed = value.trim_end_matches('=');
    URL_SAFE_NO_PAD.decode(trimmed).ok()
}

/// The S256 challenge for a verifier: base64url(SHA-256(verifier)).
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    b64url(digest.as_ref())
}

/// A fresh random token, base64url-encoded.
pub fn random_token(bytes: usize) -> Result<String, String> {
    use ring::rand::SecureRandom as _;
    let mut buffer = vec![0u8; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut buffer)
        .map_err(|_| "the system random source is unavailable".to_string())?;
    Ok(b64url(&buffer))
}

/// A sign-in that has been started but not yet completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLogin {
    /// The PKCE verifier the challenge was derived from.
    pub verifier: String,
    /// The nonce that must come back inside the ID token.
    pub nonce: String,
    /// Where to send the browser afterwards (always a local path).
    pub return_to: String,
    pub issued_at: Instant,
}

/// A bounded, single-use, self-expiring store.
///
/// Entries are removed the moment they are redeemed, so a replayed callback
/// finds nothing. When the store is full the **oldest** entry is dropped rather
/// than refusing the new one: an attacker who can start flows must not be able
/// to keep legitimate users from starting theirs.
#[derive(Debug)]
pub struct ExpiringStore<T> {
    entries: Mutex<HashMap<String, (T, Instant)>>,
    ttl: Duration,
    capacity: usize,
}

impl<T> ExpiringStore<T> {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            capacity: capacity.max(1),
        }
    }

    pub fn insert(&self, key: &str, value: T) -> Result<(), String> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "the pending sign-in store is unavailable".to_string())?;
        self.prune(&mut entries, Instant::now());
        entries.insert(key.to_string(), (value, Instant::now()));
        Ok(())
    }

    fn prune(&self, entries: &mut HashMap<String, (T, Instant)>, now: Instant) {
        entries.retain(|_, (_, issued)| now.duration_since(*issued) < self.ttl);
        while entries.len() >= self.capacity {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, (_, issued))| *issued)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
    }

    /// Remove and return an entry. `None` when it is unknown, already used or
    /// expired — all three are the same answer on purpose.
    pub fn take(&self, key: &str) -> Option<T> {
        self.take_at(key, Instant::now())
    }

    fn take_at(&self, key: &str, now: Instant) -> Option<T> {
        let mut entries = self.entries.lock().ok()?;
        let (value, issued) = entries.remove(key)?;
        if now.duration_since(issued) >= self.ttl {
            return None;
        }
        Some(value)
    }

    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The store of started but unredeemed sign-ins.
pub type PendingStore = ExpiringStore<PendingLogin>;

/// The process-wide store used by the HTTP handlers.
pub static PENDING: LazyLock<PendingStore> =
    LazyLock::new(|| PendingStore::new(PENDING_TTL, MAX_PENDING));

/// How long a minted session stays claimable by the login screen, and how many
/// may be waiting. The window is short because it only has to survive one
/// redirect.
pub const EXCHANGE_TTL: Duration = Duration::from_secs(120);
pub const MAX_EXCHANGE: usize = 256;

/// A session that has been created but not yet handed to the browser's
/// `localStorage` (the SPA authenticates with a header, not the cookie).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedSession {
    pub token: String,
    pub csrf_token: String,
    pub username: String,
    pub is_admin: bool,
}

/// Sessions created by a callback, waiting for the login screen to claim them
/// exactly once.
pub static ISSUED: LazyLock<ExpiringStore<IssuedSession>> =
    LazyLock::new(|| ExpiringStore::new(EXCHANGE_TTL, MAX_EXCHANGE));

/// Build the URL the browser is sent to.
pub fn authorize_url(
    authorization_endpoint: &str,
    settings: &OidcSettings,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> Result<String, String> {
    let mut url = url::Url::parse(authorization_endpoint.trim())
        .map_err(|e| format!("the provider's authorization endpoint is not a URL: {e}"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", settings.client_id.trim())
        .append_pair("redirect_uri", settings.redirect_url.trim())
        .append_pair("scope", settings.scopes.trim())
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}

// ---------------------------------------------------------------------------
// Provider metadata
// ---------------------------------------------------------------------------

/// The subset of the discovery document this module uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    #[serde(default)]
    pub id_token_signing_alg_values_supported: Vec<String>,
}

/// One published signing key (RFC 7517).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default, rename = "use")]
    pub usage: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<Jwk>,
}

/// Why an OIDC operation did not produce a principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OidcError {
    /// The integration is switched off or not configured.
    NotConfigured,
    /// The provider could not be reached or answered nonsense. Never the
    /// user's fault, and never a reason to add to the lockout ladder.
    Unavailable(String),
    /// The provider answered, but the response was not acceptable.
    Refused(String),
}

impl OidcError {
    /// The sentence a user sees.
    pub fn message(&self) -> String {
        match self {
            OidcError::NotConfigured => "Single sign-on is not configured".to_string(),
            OidcError::Unavailable(detail) => format!(
                "The identity provider is unavailable ({detail}) — sign in with a local account"
            ),
            OidcError::Refused(detail) => format!("Single sign-on was refused: {detail}"),
        }
    }
}

struct Cached<T> {
    value: T,
    fetched: Instant,
}

impl<T> Cached<T> {
    fn fresh(&self, now: Instant) -> bool {
        now.duration_since(self.fetched) < METADATA_TTL
    }
}

static DISCOVERY_CACHE: LazyLock<Mutex<HashMap<String, Cached<Discovery>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static JWKS_CACHE: LazyLock<Mutex<HashMap<String, Cached<Vec<Jwk>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cache_get<T: Clone>(cache: &Mutex<HashMap<String, Cached<T>>>, key: &str) -> Option<T> {
    let cache = cache.lock().ok()?;
    let entry = cache.get(key)?;
    if entry.fresh(Instant::now()) {
        Some(entry.value.clone())
    } else {
        None
    }
}

fn cache_put<T>(cache: &Mutex<HashMap<String, Cached<T>>>, key: String, value: T) {
    if let Ok(mut cache) = cache.lock() {
        let now = Instant::now();
        cache.retain(|_, entry| entry.fresh(now));
        while cache.len() >= MAX_CACHED_PROVIDERS {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.fetched)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            cache.remove(&oldest);
        }
        cache.insert(
            key,
            Cached {
                value,
                fetched: now,
            },
        );
    }
}

/// Drop every cached discovery document and key set. Called when the settings
/// are saved, so a corrected issuer is never masked by a stale cache.
pub fn clear_caches() {
    if let Ok(mut cache) = DISCOVERY_CACHE.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = JWKS_CACHE.lock() {
        cache.clear();
    }
}

fn http_client() -> Result<reqwest::Client, OidcError> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            // A discovery document that redirects is a red flag, not a feature:
            // following it could be steered at an attacker's key set.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()
    });
    client.clone().ok_or_else(|| {
        OidcError::Unavailable("the HTTP client could not be initialised".to_string())
    })
}

/// Fetch and validate the provider's discovery document.
pub async fn fetch_discovery(issuer: &str) -> Result<Discovery, OidcError> {
    let base = issuer.trim().trim_end_matches('/');
    let url = format!("{base}/.well-known/openid-configuration");
    let response = http_client()?
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            OidcError::Unavailable(format!("could not reach the discovery document: {e}"))
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(OidcError::Unavailable(format!(
            "the discovery document answered {}",
            status.as_u16()
        )));
    }
    let document: Discovery = response.json().await.map_err(|e| {
        OidcError::Refused(format!("the discovery document is not valid JSON: {e}"))
    })?;
    if document.issuer.trim().trim_end_matches('/') != base {
        return Err(OidcError::Refused(format!(
            "the discovery document names issuer '{}', not '{base}'",
            document.issuer.trim()
        )));
    }
    for (label, endpoint) in [
        ("authorization", &document.authorization_endpoint),
        ("token", &document.token_endpoint),
        ("key set", &document.jwks_uri),
    ] {
        if !valid_endpoint(endpoint) {
            return Err(OidcError::Refused(format!(
                "the discovery document's {label} endpoint is not an https:// address"
            )));
        }
    }
    if let Some(userinfo) = &document.userinfo_endpoint {
        if !valid_endpoint(userinfo) {
            return Err(OidcError::Refused(
                "the discovery document's userinfo endpoint is not an https:// address".to_string(),
            ));
        }
    }
    Ok(document)
}

/// The discovery document for an issuer, cached for [`METADATA_TTL`].
pub async fn discovery_for(issuer: &str) -> Result<Discovery, OidcError> {
    let key = issuer.trim().trim_end_matches('/').to_string();
    if let Some(document) = cache_get(&DISCOVERY_CACHE, &key) {
        return Ok(document);
    }
    let document = fetch_discovery(&key).await?;
    cache_put(&DISCOVERY_CACHE, key, document.clone());
    Ok(document)
}

/// Fetch the provider's published signing keys.
pub async fn fetch_jwks(uri: &str) -> Result<Vec<Jwk>, OidcError> {
    let response = http_client()?
        .get(uri.trim())
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| OidcError::Unavailable(format!("could not reach the key set: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(OidcError::Unavailable(format!(
            "the key set answered {}",
            status.as_u16()
        )));
    }
    let document: JwksDocument = response
        .json()
        .await
        .map_err(|e| OidcError::Refused(format!("the key set is not valid JSON: {e}")))?;
    if document.keys.is_empty() {
        return Err(OidcError::Refused(
            "the provider published no signing keys".to_string(),
        ));
    }
    Ok(document.keys)
}

/// The key set for a URI, cached for [`METADATA_TTL`]. `force` refetches, which
/// is how an unknown `kid` (a key rotation) gets one second chance.
pub async fn jwks_for(uri: &str, force: bool) -> Result<Vec<Jwk>, OidcError> {
    let key = uri.trim().to_string();
    if !force {
        if let Some(keys) = cache_get(&JWKS_CACHE, &key) {
            return Ok(keys);
        }
    }
    let keys = fetch_jwks(&key).await?;
    cache_put(&JWKS_CACHE, key, keys.clone());
    Ok(keys)
}

/// Redeem an authorization code for an ID token.
///
/// Uses `client_secret_basic` when a secret is configured and a PKCE-only
/// public client otherwise. The secret never appears in a URL.
pub async fn exchange_code(
    settings: &OidcSettings,
    discovery: &Discovery,
    code: &str,
    verifier: &str,
    client_secret: Option<&str>,
) -> Result<String, OidcError> {
    // Built by hand rather than through `RequestBuilder::form`, which needs a
    // reqwest feature this build does not enable.
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", settings.redirect_url.trim())
        .append_pair("code_verifier", verifier)
        .append_pair("client_id", settings.client_id.trim())
        .finish();
    let mut request = http_client()?
        .post(discovery.token_endpoint.trim())
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);
    if let Some(secret) = client_secret {
        request = request.basic_auth(settings.client_id.trim(), Some(secret));
    }
    let response = request
        .send()
        .await
        .map_err(|e| OidcError::Unavailable(format!("could not reach the token endpoint: {e}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(OidcError::Refused(format!(
            "the identity provider refused the code exchange ({}): {}",
            status.as_u16(),
            truncate(&body, 200)
        )));
    }
    let token: TokenResponse = serde_json::from_str(&body)
        .map_err(|e| OidcError::Refused(format!("the token response is not valid JSON: {e}")))?;
    token
        .id_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            OidcError::Refused(
                "the identity provider returned no ID token — is the 'openid' scope allowed?"
                    .to_string(),
            )
        })
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// ID token verification
// ---------------------------------------------------------------------------

/// What a callback expects to find inside the ID token.
#[derive(Debug, Clone)]
pub struct ExpectedToken<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    /// Unix seconds. Passed in so the checks are testable without sleeping.
    pub now: i64,
}

/// A verified ID token.
#[derive(Debug, Clone, PartialEq)]
pub struct IdTokenClaims {
    pub subject: String,
    pub issuer: String,
    pub audience: Vec<String>,
    pub nonce: Option<String>,
    /// Every claim, so the configured ones can be read.
    pub claims: serde_json::Value,
}

/// Verify the ID token's signature and the claims that decide *who* it was for.
///
/// The signature is checked before any claim is trusted. Only `RS256`, `ES256`
/// and `EdDSA` are accepted: `none` and the HMAC algorithms are refused, and a
/// key whose type does not match the declared `alg` is skipped rather than
/// tried (that mismatch is the classic algorithm-confusion attack).
pub fn verify_id_token(
    token: &str,
    keys: &[Jwk],
    expected: &ExpectedToken<'_>,
) -> Result<IdTokenClaims, OidcError> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    if parts.len() != 3 {
        return Err(OidcError::Refused(
            "the ID token is not a signed JWT".to_string(),
        ));
    }
    let (header_b64, payload_b64, signature_b64) = (parts[0], parts[1], parts[2]);
    let header: serde_json::Value =
        serde_json::from_slice(&b64url_decode(header_b64).ok_or_else(|| {
            OidcError::Refused("the ID token header is not base64url".to_string())
        })?)
        .map_err(|e| OidcError::Refused(format!("the ID token header is not JSON: {e}")))?;
    let algorithm = header
        .get("alg")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if !matches!(algorithm, "RS256" | "ES256" | "EdDSA") {
        return Err(OidcError::Refused(format!(
            "the ID token is signed with '{algorithm}', which this build does not accept"
        )));
    }
    let key_id = header.get("kid").and_then(|value| value.as_str());
    let signature = b64url_decode(signature_b64)
        .ok_or_else(|| OidcError::Refused("the ID token signature is not base64url".to_string()))?;
    let signing_input = format!("{header_b64}.{payload_b64}");

    let mut verified = false;
    for key in keys {
        if let Some(kid) = key_id {
            if key.kid.as_deref() != Some(kid) {
                continue;
            }
        }
        if let Some(declared) = key.alg.as_deref() {
            if !declared.eq_ignore_ascii_case(algorithm) {
                continue;
            }
        }
        if key.usage.as_deref().is_some_and(|usage| usage != "sig") {
            continue;
        }
        let Some((verification, public_key)) = verification_key(algorithm, key) else {
            continue;
        };
        if ring::signature::UnparsedPublicKey::new(verification, &public_key)
            .verify(signing_input.as_bytes(), &signature)
            .is_ok()
        {
            verified = true;
            break;
        }
    }
    if !verified {
        return Err(OidcError::Refused(
            "the ID token signature could not be verified".to_string(),
        ));
    }

    let claims: serde_json::Value =
        serde_json::from_slice(&b64url_decode(payload_b64).ok_or_else(|| {
            OidcError::Refused("the ID token payload is not base64url".to_string())
        })?)
        .map_err(|e| OidcError::Refused(format!("the ID token payload is not JSON: {e}")))?;

    // `iss`: the provider that signed it must be the one we asked.
    let issuer = claims
        .get("iss")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if issuer.trim_end_matches('/') != expected.issuer.trim().trim_end_matches('/') {
        return Err(OidcError::Refused(format!(
            "the ID token was issued by '{issuer}', not '{}'",
            expected.issuer.trim()
        )));
    }

    // `aud`: the token must be meant for this client, not another application
    // the user happens to have signed in to at the same provider.
    let audience: Vec<String> = match claims.get("aud") {
        Some(serde_json::Value::String(one)) => vec![one.clone()],
        Some(serde_json::Value::Array(many)) => many
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    if !audience
        .iter()
        .any(|value| value == expected.client_id.trim())
    {
        return Err(OidcError::Refused(
            "the ID token was issued for another application".to_string(),
        ));
    }
    if let Some(azp) = claims.get("azp").and_then(|value| value.as_str()) {
        if azp != expected.client_id.trim() {
            return Err(OidcError::Refused(
                "the ID token's authorized party is another application".to_string(),
            ));
        }
    }

    // `exp` / `iat` / `nbf`: refuse a stale token, and one minted in the future
    // beyond the tolerated clock skew.
    let expires = claims
        .get("exp")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| OidcError::Refused("the ID token has no expiry".to_string()))?;
    if expires + CLOCK_SKEW_SECONDS < expected.now {
        return Err(OidcError::Refused("the ID token has expired".to_string()));
    }
    let issued = claims
        .get("iat")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| OidcError::Refused("the ID token has no issue time".to_string()))?;
    if issued > expected.now + CLOCK_SKEW_SECONDS {
        return Err(OidcError::Refused(
            "the ID token was issued in the future".to_string(),
        ));
    }
    if let Some(not_before) = claims.get("nbf").and_then(serde_json::Value::as_i64) {
        if not_before > expected.now + CLOCK_SKEW_SECONDS {
            return Err(OidcError::Refused(
                "the ID token is not valid yet".to_string(),
            ));
        }
    }

    // `nonce`: proves this token belongs to the sign-in this browser started,
    // not to an earlier one replayed at the callback.
    let nonce = claims
        .get("nonce")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if nonce.is_empty() || nonce != expected.nonce {
        return Err(OidcError::Refused(
            "the ID token was not issued for this sign-in".to_string(),
        ));
    }

    let subject = claims
        .get("sub")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if subject.is_empty() {
        return Err(OidcError::Refused(
            "the ID token carries no subject".to_string(),
        ));
    }

    Ok(IdTokenClaims {
        subject,
        issuer,
        audience,
        nonce: Some(nonce.to_string()),
        claims,
    })
}

/// Read the `kid` out of a JWT header without trusting anything else in it.
///
/// Used only to decide whether a failed verification is worth one key-set
/// refetch (a rotation); nothing is authorized from this value.
pub fn token_key_id(token: &str) -> Option<String> {
    let header = token.trim().split('.').next()?;
    let bytes = b64url_decode(header)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("kid")
        .and_then(|kid| kid.as_str())
        .map(str::to_string)
}

/// Turn a JWK into the public key bytes ring expects, for the given JWT `alg`.
fn verification_key(
    algorithm: &str,
    key: &Jwk,
) -> Option<(&'static dyn ring::signature::VerificationAlgorithm, Vec<u8>)> {
    match algorithm {
        "RS256" if key.kty == "RSA" => {
            let modulus = b64url_decode(key.n.as_deref()?)?;
            let exponent = b64url_decode(key.e.as_deref()?)?;
            if modulus.is_empty() || exponent.is_empty() {
                return None;
            }
            Some((
                &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                rsa_public_key_der(&modulus, &exponent),
            ))
        }
        "ES256" if key.kty == "EC" && key.crv.as_deref() == Some("P-256") => {
            let x = b64url_decode(key.x.as_deref()?)?;
            let y = b64url_decode(key.y.as_deref()?)?;
            if x.len() != 32 || y.len() != 32 {
                return None;
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            Some((&ring::signature::ECDSA_P256_SHA256_FIXED, point))
        }
        "EdDSA" if key.kty == "OKP" && key.crv.as_deref() == Some("Ed25519") => {
            let x = b64url_decode(key.x.as_deref()?)?;
            if x.len() != 32 {
                return None;
            }
            Some((&ring::signature::ED25519, x))
        }
        _ => None,
    }
}

/// DER `SEQUENCE { INTEGER n, INTEGER e }` — the RSAPublicKey ring verifies.
///
/// JWK integers are unsigned big-endian; DER wants a leading zero when the top
/// bit is set, and no leading zeros otherwise.
fn rsa_public_key_der(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&der_integer(modulus));
    body.extend_from_slice(&der_integer(exponent));
    let mut out = Vec::with_capacity(body.len() + 4);
    out.push(0x30);
    der_length(&mut out, body.len());
    out.extend_from_slice(&body);
    out
}

fn der_integer(value: &[u8]) -> Vec<u8> {
    let mut start = 0;
    while start + 1 < value.len() && value[start] == 0 {
        start += 1;
    }
    let value = &value[start..];
    let negative = value.first().is_some_and(|byte| byte & 0x80 != 0);
    let mut out = Vec::with_capacity(value.len() + 4);
    out.push(0x02);
    der_length(&mut out, value.len() + usize::from(negative));
    if negative {
        out.push(0);
    }
    out.extend_from_slice(value);
    out
}

fn der_length(out: &mut Vec<u8>, length: usize) {
    if length < 0x80 {
        out.push(length as u8);
        return;
    }
    let bytes: Vec<u8> = length
        .to_be_bytes()
        .into_iter()
        .skip_while(|byte| *byte == 0)
        .collect();
    out.push(0x80 | bytes.len() as u8);
    out.extend_from_slice(&bytes);
}

// ---------------------------------------------------------------------------
// Claims → principal
// ---------------------------------------------------------------------------

/// A remote principal as the ID token describes it. Never carries a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcPrincipal {
    /// `{issuer}#{sub}` — the provider's stable key, namespaced by issuer.
    pub subject: String,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub groups: Vec<String>,
}

fn claim_str<'a>(claims: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    claims
        .get(name.trim())
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Read a group claim that is either an array of strings or a single
/// space-separated string. Anything else is ignored rather than guessed at.
pub fn claim_groups(claims: &serde_json::Value, name: &str) -> Vec<String> {
    let Some(value) = claims.get(name.trim()) else {
        return Vec::new();
    };
    let mut groups: Vec<String> = Vec::new();
    let mut push = |candidate: &str| {
        let candidate = candidate.trim();
        if !candidate.is_empty() && candidate.len() <= MAX_GROUP_NAME_LEN {
            groups.push(candidate.to_string());
        }
    };
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(item) = item.as_str() {
                    push(item);
                }
            }
        }
        serde_json::Value::String(list) => {
            for item in list.split_whitespace() {
                push(item);
            }
        }
        _ => {}
    }
    groups.sort();
    groups.dedup();
    groups.truncate(MAX_GROUPS_STORED);
    groups
}

/// Turn the claim the admin chose into a HomeVault account name.
///
/// Directory names are free-form (spaces, dots, an e-mail address); HomeVault
/// account names are `[A-Za-z_][A-Za-z0-9_]{2,63}`. Anything unusable becomes
/// `_`, an e-mail loses its domain, and a leading digit gains an underscore.
/// Returns `None` when nothing usable is left, so the caller can name the
/// claim that needs changing instead of creating a strange account.
pub fn sanitize_username(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let local = raw.split('@').next().unwrap_or("").trim();
    if local.is_empty() {
        return None;
    }
    let mut name = String::with_capacity(local.len());
    let mut previous_underscore = false;
    for character in local.chars() {
        if character.is_alphanumeric() || character == '_' {
            name.push(character);
            previous_underscore = false;
        } else if !previous_underscore {
            name.push('_');
            previous_underscore = true;
        }
    }
    let name = name.trim_matches('_');
    if name.is_empty() {
        return None;
    }
    let mut name = name.to_string();
    if let Some(first) = name.chars().next() {
        if first.is_ascii_digit() {
            name.insert(0, '_');
        }
    }
    name.truncate(64);
    if crate::setup::SetupService::is_valid_username(&name) {
        Some(name)
    } else {
        None
    }
}

/// Build the principal from a verified ID token's claims.
pub fn principal_from_claims(
    claims: &serde_json::Value,
    settings: &OidcSettings,
) -> Result<OidcPrincipal, OidcError> {
    let subject = claims
        .get("sub")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if subject.is_empty() {
        return Err(OidcError::Refused(
            "the ID token carries no subject".to_string(),
        ));
    }
    let issuer = claims
        .get("iss")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/');
    let raw_username = claim_str(claims, &settings.attr_username)
        .or_else(|| claim_str(claims, "preferred_username"))
        .or_else(|| claim_str(claims, "email"))
        .or_else(|| claim_str(claims, "upn"))
        .unwrap_or_default();
    let username = sanitize_username(raw_username).ok_or_else(|| {
        OidcError::Refused(format!(
            "the '{}' claim ('{raw_username}') does not hold a usable account name — choose a claim such as 'preferred_username'",
            settings.attr_username.trim()
        ))
    })?;
    let display_name = claim_str(claims, &settings.attr_display_name)
        .map(str::to_string)
        .filter(|value| value.len() <= 128)
        .unwrap_or_else(|| username.clone());
    let email = claim_str(claims, &settings.attr_email)
        .map(str::to_string)
        .filter(|value| value.len() <= 254)
        .unwrap_or_default();
    Ok(OidcPrincipal {
        subject: format!("{issuer}#{subject}"),
        username,
        display_name,
        email,
        groups: claim_groups(claims, &settings.attr_groups),
    })
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn data_directory() -> String {
    std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string())
}

fn storage() -> JsonStorage {
    JsonStorage::new(data_directory())
}

/// Read the settings, falling back to "off" when the document is missing or
/// unreadable — a corrupt file must never lock anyone out.
pub fn load() -> OidcSettings {
    match storage().load::<OidcSettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => OidcSettings::default(),
        Err(e) => {
            tracing::warn!("could not read the OIDC settings: {e}");
            OidcSettings::default()
        }
    }
}

/// Persist the settings (atomic write) and drop the provider caches, so a
/// corrected issuer takes effect on the next sign-in.
pub fn save(settings: &OidcSettings) -> Result<(), String> {
    storage()
        .save(STORE_KEY, settings)
        .map_err(|e| format!("could not save the single sign-on settings: {e}"))?;
    clear_caches();
    Ok(())
}

/// Seal the client secret.
pub fn seal_client_secret(secret: &str) -> Result<String, String> {
    crate::storage::secret_box::seal(CLIENT_SECRET_SCOPE, secret)
}

/// Open the client secret, or `None` when none is stored (public client).
pub fn open_client_secret(sealed: &Option<String>) -> Result<Option<String>, String> {
    match sealed {
        None => Ok(None),
        Some(blob) => crate::storage::secret_box::open(CLIENT_SECRET_SCOPE, blob)
            .map(Some)
            .map_err(|e| format!("could not read the client secret: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 2026-01-01T00:00:00Z — the "now" every fixed token is checked against,
    /// so the tests never depend on the wall clock.
    const NOW: i64 = 1_767_225_600;

    fn settings() -> OidcSettings {
        OidcSettings {
            enabled: true,
            label: "Sign in with Authentik".to_string(),
            issuer: "https://idp.example.test".to_string(),
            client_id: "homevault".to_string(),
            client_secret_sealed: Some("sealed".to_string()),
            redirect_url: "https://nas.example.test/api/auth/oidc/callback".to_string(),
            ..OidcSettings::default()
        }
    }

    fn expected<'a>(settings: &'a OidcSettings, nonce: &'a str) -> ExpectedToken<'a> {
        ExpectedToken {
            issuer: &settings.issuer,
            client_id: &settings.client_id,
            nonce,
            now: NOW,
        }
    }

    // -- URLs and scopes ---------------------------------------------------

    #[test]
    fn test_valid_issuer() {
        assert!(valid_issuer("https://idp.example.test"));
        assert!(valid_issuer(
            "https://auth.example.test/application/o/homevault/"
        ));
        assert!(valid_issuer("https://idp.example.test:8443/realm"));
        // Loopback may be plain HTTP (a local provider during setup and tests).
        assert!(valid_issuer("http://127.0.0.1:9000"));
        assert!(valid_issuer("http://localhost:9000/issuer"));
        assert!(valid_issuer("http://[::1]:9000"));
    }

    #[test]
    fn test_valid_issuer_refuses_unsafe_shapes() {
        assert!(!valid_issuer(""));
        assert!(!valid_issuer("   "));
        assert!(
            !valid_issuer("http://idp.example.test"),
            "plain HTTP off loopback"
        );
        assert!(
            !valid_issuer("http://192.168.1.10:9000"),
            "plain HTTP on the LAN"
        );
        assert!(!valid_issuer("ftp://idp.example.test"));
        assert!(!valid_issuer("https://idp.example.test?x=1"));
        assert!(!valid_issuer("https://idp.example.test#fragment"));
        assert!(!valid_issuer("https://user:pass@idp.example.test"));
        assert!(!valid_issuer(&format!("https://{}.test", "a".repeat(600))));
        assert!(!valid_issuer("not a url"));
    }

    #[test]
    fn test_valid_redirect_url() {
        assert!(valid_redirect_url(
            "https://nas.example.test/api/auth/oidc/callback"
        ));
        assert!(valid_redirect_url(
            "https://nas.example.test:8443/homevault/api/auth/oidc/callback"
        ));
        assert!(valid_redirect_url(
            "http://127.0.0.1:8080/api/auth/oidc/callback"
        ));
        // The usual paste error: the provider's login URL, not our callback.
        assert!(!valid_redirect_url("https://nas.example.test/login"));
        assert!(!valid_redirect_url(
            "https://nas.example.test/api/auth/oidc/callback?x=1"
        ));
        assert!(!valid_redirect_url(
            "http://nas.example.test/api/auth/oidc/callback"
        ));
        assert!(!valid_redirect_url("/api/auth/oidc/callback"));
    }

    #[test]
    fn test_valid_claim_name() {
        assert!(valid_claim_name("preferred_username"));
        assert!(valid_claim_name("given_name"));
        assert!(valid_claim_name("email"));
        assert!(valid_claim_name(
            "urn:example:groups".replace(':', "_").as_str()
        ));
        assert!(!valid_claim_name(""));
        assert!(!valid_claim_name("preferred username"));
        assert!(!valid_claim_name("groups/roles"));
        assert!(!valid_claim_name(&"a".repeat(65)));
    }

    #[test]
    fn test_valid_scopes() {
        assert!(valid_scopes("openid"));
        assert!(valid_scopes("openid profile email"));
        assert!(valid_scopes("  openid   groups  "));
        assert!(!valid_scopes(""));
        assert!(!valid_scopes("profile email"));
        assert!(!valid_scopes("openid \"profile\""));
        assert!(!valid_scopes("openid prof\\ile"));
        assert!(!valid_scopes(&format!(
            "openid {}",
            "a".repeat(MAX_SCOPE_TOKEN_LEN + 1)
        )));
    }

    #[test]
    fn test_validate_allows_a_half_filled_disabled_document() {
        let settings = OidcSettings::default();
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn test_validate_accepts_a_complete_document() {
        assert!(validate(&settings()).is_ok());
    }

    #[test]
    fn test_validate_refusals() {
        let mut broken = settings();
        broken.issuer = String::new();
        assert!(validate(&broken).unwrap_err().contains("issuer"));

        let mut broken = settings();
        broken.client_id = "  ".to_string();
        assert!(validate(&broken).unwrap_err().contains("client ID"));

        let mut broken = settings();
        broken.redirect_url = "https://nas.example.test/login".to_string();
        assert!(validate(&broken).unwrap_err().contains(CALLBACK_PATH));

        let mut broken = settings();
        broken.scopes = "profile".to_string();
        assert!(validate(&broken).unwrap_err().contains("openid"));

        let mut broken = settings();
        broken.label = String::new();
        assert!(validate(&broken).unwrap_err().contains("label"));

        let mut broken = settings();
        broken.attr_username = "preferred username".to_string();
        assert!(validate(&broken).unwrap_err().contains("login name"));

        let mut broken = settings();
        broken.default_role = "superuser".to_string();
        assert!(validate(&broken).unwrap_err().contains("superuser"));

        let mut broken = settings();
        broken.group_roles = vec![
            GroupRole {
                group: "admins".into(),
                role: "admin".into(),
            },
            GroupRole {
                group: "ADMINS".into(),
                role: "user".into(),
            },
        ];
        assert!(validate(&broken).unwrap_err().contains("mapped twice"));

        let mut broken = settings();
        broken.group_roles = (0..MAX_GROUP_ROLES + 1)
            .map(|index| GroupRole {
                group: format!("group-{index}"),
                role: "user".into(),
            })
            .collect();
        assert!(validate(&broken).unwrap_err().contains("At most"));
    }

    // -- PKCE and the pending store ---------------------------------------

    #[test]
    fn test_pkce_challenge_matches_rfc_7636() {
        // RFC 7636 appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn test_b64url_round_trip_and_padding() {
        let encoded = b64url(b"hello world");
        assert!(!encoded.contains('='));
        assert_eq!(b64url_decode(&encoded).unwrap(), b"hello world");
        // Some providers pad; we accept it.
        assert_eq!(
            b64url_decode(&format!("{encoded}==")).unwrap(),
            b"hello world"
        );
        assert!(b64url_decode("!!!not base64!!!").is_none());
    }

    #[test]
    fn test_random_token_is_unique_and_hex_safe() {
        let first = random_token(32).unwrap();
        let second = random_token(32).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 43, "32 bytes without padding");
        assert!(first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_pending_store_is_single_use() {
        let store: PendingStore = ExpiringStore::new(Duration::from_secs(60), 8);
        store
            .insert(
                "state-1",
                PendingLogin {
                    verifier: "v".into(),
                    nonce: "n".into(),
                    return_to: "/desktop".into(),
                    issued_at: Instant::now(),
                },
            )
            .unwrap();
        assert_eq!(store.len(), 1);
        let taken = store.take("state-1").expect("first take");
        assert_eq!(taken.nonce, "n");
        assert!(store.take("state-1").is_none(), "replay finds nothing");
    }

    #[test]
    fn test_pending_store_expires() {
        let store: PendingStore = ExpiringStore::new(Duration::from_secs(10), 8);
        let issued = Instant::now();
        store
            .insert(
                "state-1",
                PendingLogin {
                    verifier: "v".into(),
                    nonce: "n".into(),
                    return_to: "/desktop".into(),
                    issued_at: issued,
                },
            )
            .unwrap();
        assert!(store
            .take_at("state-1", issued + Duration::from_secs(11))
            .is_none());
    }

    #[test]
    fn test_pending_store_is_bounded_and_drops_the_oldest() {
        let store: PendingStore = ExpiringStore::new(Duration::from_secs(600), 2);
        let base = Instant::now();
        for index in 0..3 {
            store
                .insert(
                    &format!("state-{index}"),
                    PendingLogin {
                        verifier: "v".into(),
                        nonce: format!("n-{index}"),
                        return_to: "/desktop".into(),
                        issued_at: base,
                    },
                )
                .unwrap();
        }
        assert_eq!(store.len(), 2);
        assert!(store.take("state-0").is_none(), "the oldest was dropped");
        assert!(store.take("state-1").is_some());
        assert!(store.take("state-2").is_some());
    }

    #[test]
    fn test_authorize_url_carries_the_flow_parameters() {
        let settings = OidcSettings {
            redirect_url: "https://nas.example.test/api/auth/oidc/callback".to_string(),
            ..settings()
        };
        let url = authorize_url(
            "https://idp.example.test/authorize",
            &settings,
            "state-1",
            "nonce-1",
            "challenge-1",
        )
        .unwrap();
        assert!(url.starts_with("https://idp.example.test/authorize?"));
        for needle in [
            "response_type=code",
            "client_id=homevault",
            "code_challenge_method=S256",
            "code_challenge=challenge-1",
            "state=state-1",
            "nonce=nonce-1",
        ] {
            assert!(url.contains(needle), "{needle} missing from {url}");
        }
        // The callback is percent-encoded, not spliced in raw.
        assert!(url.contains(
            "redirect_uri=https%3A%2F%2Fnas.example.test%2Fapi%2Fauth%2Foidc%2Fcallback"
        ));
        assert!(!url.contains("client_secret"));
    }

    #[test]
    fn test_authorize_url_keeps_an_existing_query() {
        let url = authorize_url(
            "https://idp.example.test/authorize?tenant=home",
            &settings(),
            "s",
            "n",
            "c",
        )
        .unwrap();
        assert!(url.contains("tenant=home"));
        assert!(url.contains("state=s"));
    }

    // -- Claims ------------------------------------------------------------

    #[test]
    fn test_sanitize_username() {
        assert_eq!(sanitize_username("alice").as_deref(), Some("alice"));
        assert_eq!(sanitize_username("  Alice  ").as_deref(), Some("Alice"));
        assert_eq!(
            sanitize_username("alice@example.test").as_deref(),
            Some("alice")
        );
        assert_eq!(sanitize_username("john.doe").as_deref(), Some("john_doe"));
        assert_eq!(sanitize_username("john--doe").as_deref(), Some("john_doe"));
        assert_eq!(sanitize_username("9lives").as_deref(), Some("_9lives"));
        assert_eq!(sanitize_username("  "), None);
        assert_eq!(sanitize_username("@example.test"), None);
        assert_eq!(sanitize_username("ab"), None, "too short");
        assert_eq!(sanitize_username("root"), None, "reserved");
    }

    #[test]
    fn test_claim_groups_accepts_arrays_and_space_separated_strings() {
        let array = json!({ "groups": ["admins", "users", "admins"] });
        assert_eq!(claim_groups(&array, "groups"), vec!["admins", "users"]);

        let spaced = json!({ "groups": "admins   operations" });
        assert_eq!(
            claim_groups(&spaced, "groups"),
            vec!["admins", "operations"]
        );

        // An object (Keycloak's realm_access) is ignored, never guessed at.
        let object = json!({ "groups": { "roles": ["admins"] } });
        assert!(claim_groups(&object, "groups").is_empty());
        assert!(claim_groups(&json!({}), "groups").is_empty());
    }

    #[test]
    fn test_claim_groups_is_capped() {
        let many: Vec<String> = (0..MAX_GROUPS_STORED + 50)
            .map(|i| format!("g{i:04}"))
            .collect();
        let claims = json!({ "groups": many });
        assert_eq!(claim_groups(&claims, "groups").len(), MAX_GROUPS_STORED);
    }

    #[test]
    fn test_principal_from_claims() {
        let claims = json!({
            "iss": "https://idp.example.test/",
            "sub": "1234",
            "preferred_username": "alice",
            "name": "Alice Martin",
            "email": "alice@example.test",
            "groups": ["admins"],
        });
        let principal = principal_from_claims(&claims, &settings()).unwrap();
        assert_eq!(principal.subject, "https://idp.example.test#1234");
        assert_eq!(principal.username, "alice");
        assert_eq!(principal.display_name, "Alice Martin");
        assert_eq!(principal.email, "alice@example.test");
        assert_eq!(principal.groups, vec!["admins"]);
    }

    #[test]
    fn test_principal_falls_back_to_configurable_claims() {
        let settings = OidcSettings {
            attr_username: "email".to_string(),
            attr_display_name: "missing".to_string(),
            ..settings()
        };
        let claims = json!({
            "iss": "https://idp.example.test",
            "sub": "1234",
            "email": "bob@example.test",
            "groups": "users",
        });
        let principal = principal_from_claims(&claims, &settings).unwrap();
        assert_eq!(principal.username, "bob");
        assert_eq!(
            principal.display_name, "bob",
            "display name falls back to the account name"
        );
        assert_eq!(principal.groups, vec!["users"]);
    }

    #[test]
    fn test_principal_refuses_an_unusable_name() {
        let claims = json!({
            "iss": "https://idp.example.test",
            "sub": "1234",
            "preferred_username": "x",
        });
        let error = principal_from_claims(&claims, &settings()).unwrap_err();
        assert!(error.message().contains("usable account name"));
    }

    #[test]
    fn test_principal_requires_a_subject() {
        let claims = json!({ "iss": "https://idp.example.test", "preferred_username": "alice" });
        assert!(principal_from_claims(&claims, &settings()).is_err());
    }

    // -- ID token verification --------------------------------------------

    fn encode(value: &serde_json::Value) -> String {
        b64url(&serde_json::to_vec(value).unwrap())
    }

    fn claims(issuer: &str, audience: &str, nonce: &str) -> serde_json::Value {
        json!({
            "iss": issuer,
            "aud": audience,
            "sub": "alice",
            "iat": NOW - 10,
            "exp": NOW + 300,
            "nonce": nonce,
        })
    }

    /// A real P-256 key pair, its JWK and a signer for raw JWT signatures.
    fn es256_keypair() -> (Jwk, ring::signature::EcdsaKeyPair) {
        use ring::signature::{EcdsaKeyPair, KeyPair};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                .expect("generate");
        let pair = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
            &rng,
        )
        .expect("load");
        let public = pair.public_key().as_ref();
        assert_eq!(public.len(), 65);
        let key = Jwk {
            kty: "EC".to_string(),
            kid: Some("ec-test".to_string()),
            alg: Some("ES256".to_string()),
            usage: None,
            n: None,
            e: None,
            crv: Some("P-256".to_string()),
            x: Some(b64url(&public[1..33])),
            y: Some(b64url(&public[33..65])),
        };
        (key, pair)
    }

    fn eddsa_keypair() -> (Jwk, ring::signature::Ed25519KeyPair) {
        use ring::signature::KeyPair as _;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("generate");
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("load");
        let key = Jwk {
            kty: "OKP".to_string(),
            kid: Some("ed-test".to_string()),
            alg: Some("EdDSA".to_string()),
            usage: None,
            n: None,
            e: None,
            crv: Some("Ed25519".to_string()),
            x: Some(b64url(pair.public_key().as_ref())),
            y: None,
        };
        (key, pair)
    }

    fn signed(
        algorithm: &str,
        kid: &str,
        claims: &serde_json::Value,
        sign: impl Fn(&[u8]) -> Vec<u8>,
    ) -> String {
        let header = json!({ "alg": algorithm, "typ": "JWT", "kid": kid });
        let signing_input = format!("{}.{}", encode(&header), encode(claims));
        let signature = sign(signing_input.as_bytes());
        format!("{signing_input}.{}", b64url(&signature))
    }

    #[test]
    fn test_verify_es256() {
        let settings = settings();
        let (key, pair) = es256_keypair();
        let rng = ring::rand::SystemRandom::new();
        let token = signed(
            "ES256",
            "ec-test",
            &claims(&settings.issuer, "homevault", "nonce-1"),
            |input| pair.sign(&rng, input).unwrap().as_ref().to_vec(),
        );
        let verified = verify_id_token(&token, &[key], &expected(&settings, "nonce-1")).unwrap();
        assert_eq!(verified.subject, "alice");
        assert_eq!(verified.issuer, settings.issuer);
        assert_eq!(verified.audience, vec!["homevault"]);
    }

    #[test]
    fn test_verify_eddsa() {
        let settings = settings();
        let (key, pair) = eddsa_keypair();
        let token = signed(
            "EdDSA",
            "ed-test",
            &claims(&settings.issuer, "homevault", "nonce-1"),
            |input| pair.sign(input).as_ref().to_vec(),
        );
        assert!(verify_id_token(&token, &[key], &expected(&settings, "nonce-1")).is_ok());
    }

    /// A real RS256 token, generated once with `openssl` and embedded here: ring
    /// cannot sign RSA, and a fabricated signature would prove nothing about
    /// the DER encoding this module builds.
    const RS256_N: &str = "7I5KJtsR2XM6ylYuA0Ox3wzUlZ7EtZVf9YJJCAC1I0m2fSxmzZg2KrOMj6ijjJYAMHsyz9OgzpB4mTjI3jo2ZZBijIlnwsCVMXfh9vhzti5NQYs_uNu_6-gQBX52xktRcE7AdGnlIlHfyJtK27L9jmfek7XBZwRbwBlCEwn7mOVLicw7WDAQuiqJVimDHz0szuLd9yPK-2cMG5zwVrlythe4Hm_fgTxK-LfUNYX6CWw-nsPFFts8HfzZnRnRr2WtkvVSKE3QH1bKIu6Wid_ioy6-enyVFBhvakMbjpMw5vKVQUttfOnsWHU3KUaNuNIMTWl0hbQiIg-6EV0GqiZbYw";
    const RS256_E: &str = "AQAB";
    const RS256_TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6InJzYS10ZXN0In0.eyJpc3MiOiJodHRwczovL2lkcC5leGFtcGxlLnRlc3QiLCJhdWQiOiJob21ldmF1bHQtdGVzdCIsInN1YiI6ImFsaWNlIiwicHJlZmVycmVkX3VzZXJuYW1lIjoiYWxpY2UiLCJuYW1lIjoiQWxpY2UgTWFydGluIiwiZW1haWwiOiJhbGljZUBleGFtcGxlLnRlc3QiLCJncm91cHMiOlsiYWRtaW5zIiwidXNlcnMiXSwiaWF0IjoxNjAwMDAwMDAwLCJleHAiOjQxMDI0NDQ4MDAsIm5vbmNlIjoibm9uY2UtcnMyNTYtdGVzdCJ9.6tak_wVWOmKv8xtScI4AECo3xpsBpcCzvoITzwuy-MpnOs0_N_s8_no6axY0VDTZf2oRMJ9QKS0nfQd-Zi5szrwRuxoLvT_5xI8bzFeoD_oYqofJWc2F7i54wQLwwL40cPDeEeGlQRqgRm4TW-M4uo6fs13-l_g-pz9peFVgmsCU8-MpiigkoZmDJz1xUWAps6Y8peqAhpHHFrr1C6tzz-gZ0xlp37St5rG7x_sjVXK8GMGNTaYogkUof8cpCbzrGwW4YERoPNGUazyhsabaHwVy8jIjs7eBsodolNeLnCG5E8sKGr_cIQp9yFOsp5TjVdkYZEauD1ApncfSZSoolg";

    fn rs256_key() -> Jwk {
        Jwk {
            kty: "RSA".to_string(),
            kid: Some("rsa-test".to_string()),
            alg: Some("RS256".to_string()),
            usage: Some("sig".to_string()),
            n: Some(RS256_N.to_string()),
            e: Some(RS256_E.to_string()),
            crv: None,
            x: None,
            y: None,
        }
    }

    #[test]
    fn test_verify_rs256_with_a_real_signature() {
        let settings = settings();
        let expected = ExpectedToken {
            issuer: "https://idp.example.test",
            client_id: "homevault-test",
            nonce: "nonce-rs256-test",
            now: NOW,
        };
        let verified = verify_id_token(RS256_TOKEN, &[rs256_key()], &expected).unwrap();
        assert_eq!(verified.subject, "alice");
        let principal = principal_from_claims(&verified.claims, &settings).unwrap();
        assert_eq!(principal.username, "alice");
        assert_eq!(principal.display_name, "Alice Martin");
        assert_eq!(principal.groups, vec!["admins", "users"]);
        // One admin group is enough.
        assert_eq!(
            crate::directory::resolve_groups_role(
                &principal.groups,
                &settings.group_roles,
                &settings.default_role
            ),
            "user",
            "no mappings configured yet"
        );
    }

    #[test]
    fn test_verify_refuses_an_altered_payload() {
        let mut parts: Vec<&str> = RS256_TOKEN.split('.').collect();
        let altered = encode(&json!({
            "iss": "https://idp.example.test",
            "aud": "homevault-test",
            "sub": "mallory",
            "iat": NOW - 10,
            "exp": NOW + 300,
            "nonce": "nonce-rs256-test",
        }));
        parts[1] = &altered;
        let token = parts.join(".");
        let expected = ExpectedToken {
            issuer: "https://idp.example.test",
            client_id: "homevault-test",
            nonce: "nonce-rs256-test",
            now: NOW,
        };
        let error = verify_id_token(&token, &[rs256_key()], &expected).unwrap_err();
        assert!(error.message().contains("signature"));
    }

    #[test]
    fn test_verify_refuses_none_and_hmac() {
        let settings = settings();
        for algorithm in ["none", "HS256", "RS512"] {
            let token = signed(
                algorithm,
                "rsa-test",
                &claims(&settings.issuer, "homevault", "nonce-1"),
                |_| vec![0; 32],
            );
            let error = verify_id_token(&token, &[rs256_key()], &expected(&settings, "nonce-1"))
                .unwrap_err();
            assert!(
                error.message().contains("does not accept"),
                "{algorithm}: {error:?}"
            );
        }
    }

    #[test]
    fn test_verify_refuses_a_key_of_the_wrong_type() {
        // The classic algorithm-confusion attempt: declare ES256 but hand over
        // an RSA key (or vice versa). The key is skipped, not reinterpreted.
        let settings = settings();
        let (ec_key, _) = es256_keypair();
        let token = signed(
            "ES256",
            "ec-test",
            &claims(&settings.issuer, "homevault", "nonce-1"),
            |_| vec![0; 64],
        );
        assert!(verify_id_token(&token, &[ec_key], &expected(&settings, "nonce-1")).is_err());
        assert!(verify_id_token(&token, &[rs256_key()], &expected(&settings, "nonce-1")).is_err());
    }

    #[test]
    fn test_verify_refuses_a_wrong_nonce_issuer_or_audience() {
        let settings = settings();
        let (key, pair) = es256_keypair();
        let rng = ring::rand::SystemRandom::new();
        let token = signed(
            "ES256",
            "ec-test",
            &claims(&settings.issuer, "homevault", "nonce-1"),
            |input| pair.sign(&rng, input).unwrap().as_ref().to_vec(),
        );
        assert!(verify_id_token(
            &token,
            std::slice::from_ref(&key),
            &expected(&settings, "other")
        )
        .is_err());

        let wrong_issuer = ExpectedToken {
            issuer: "https://evil.example.test",
            client_id: "homevault",
            nonce: "nonce-1",
            now: NOW,
        };
        assert!(
            verify_id_token(&token, std::slice::from_ref(&key), &wrong_issuer)
                .unwrap_err()
                .message()
                .contains("issued by")
        );

        let wrong_audience = ExpectedToken {
            issuer: &settings.issuer,
            client_id: "another-app",
            nonce: "nonce-1",
            now: NOW,
        };
        assert!(
            verify_id_token(&token, std::slice::from_ref(&key), &wrong_audience)
                .unwrap_err()
                .message()
                .contains("another application")
        );
    }

    #[test]
    fn test_verify_refuses_an_expired_token() {
        let settings = settings();
        let (key, pair) = es256_keypair();
        let rng = ring::rand::SystemRandom::new();
        let token = signed(
            "ES256",
            "ec-test",
            &claims(&settings.issuer, "homevault", "nonce-1"),
            |input| pair.sign(&rng, input).unwrap().as_ref().to_vec(),
        );
        let much_later = ExpectedToken {
            issuer: &settings.issuer,
            client_id: "homevault",
            nonce: "nonce-1",
            now: NOW + 3_600,
        };
        assert!(verify_id_token(&token, &[key], &much_later)
            .unwrap_err()
            .message()
            .contains("expired"));
    }

    #[test]
    fn test_verify_refuses_a_token_without_nonce_or_expiry() {
        let settings = settings();
        let (key, pair) = es256_keypair();
        let rng = ring::rand::SystemRandom::new();
        let missing_nonce = json!({
            "iss": settings.issuer, "aud": "homevault", "sub": "alice",
            "iat": NOW - 10, "exp": NOW + 300,
        });
        let token = signed("ES256", "ec-test", &missing_nonce, |input| {
            pair.sign(&rng, input).unwrap().as_ref().to_vec()
        });
        assert!(verify_id_token(
            &token,
            std::slice::from_ref(&key),
            &expected(&settings, "nonce-1")
        )
        .is_err());

        let missing_exp = json!({
            "iss": settings.issuer, "aud": "homevault", "sub": "alice",
            "iat": NOW - 10, "nonce": "nonce-1",
        });
        let token = signed("ES256", "ec-test", &missing_exp, |input| {
            pair.sign(&rng, input).unwrap().as_ref().to_vec()
        });
        assert!(
            verify_id_token(&token, &[key], &expected(&settings, "nonce-1"))
                .unwrap_err()
                .message()
                .contains("expiry")
        );
    }

    #[test]
    fn test_verify_accepts_a_trailing_slash_difference_on_issuer() {
        let settings = settings();
        let (key, pair) = es256_keypair();
        let rng = ring::rand::SystemRandom::new();
        let token = signed(
            "ES256",
            "ec-test",
            &claims("https://idp.example.test/", "homevault", "nonce-1"),
            |input| pair.sign(&rng, input).unwrap().as_ref().to_vec(),
        );
        assert!(verify_id_token(&token, &[key], &expected(&settings, "nonce-1")).is_ok());
    }

    #[test]
    fn test_token_key_id() {
        let token = signed("ES256", "ec-test", &json!({ "sub": "x" }), |_| vec![0; 64]);
        assert_eq!(token_key_id(&token).as_deref(), Some("ec-test"));
        assert_eq!(token_key_id("not.a.jwt"), None);
        assert_eq!(token_key_id(""), None);
    }

    #[test]
    fn test_rsa_public_key_der_shape() {
        // A modulus whose top bit is set needs a leading zero byte.
        let der = rsa_public_key_der(&[0x80, 0x01], &[0x01, 0x00, 0x01]);
        assert_eq!(der[0], 0x30);
        assert_eq!(der[1] as usize, der.len() - 2);
        assert_eq!(&der[2..5], &[0x02, 0x03, 0x00]);
        assert_eq!(der[5], 0x80);
        // A leading zero in the JWK integer is dropped, not doubled.
        let der = rsa_public_key_der(&[0x00, 0x01, 0x02], &[0x01, 0x00, 0x01]);
        assert_eq!(&der[2..5], &[0x02, 0x02, 0x01]);
    }

    #[test]
    fn test_error_messages_never_leak_a_token() {
        let error = OidcError::Refused("the ID token has expired".to_string());
        assert!(!error.message().contains('.'), "no JWT-shaped text");
        assert!(OidcError::Unavailable("timeout".into())
            .message()
            .contains("local account"));
    }
}
