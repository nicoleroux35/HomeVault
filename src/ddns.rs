/* =============================================================================
 * File          : ddns.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Dynamic DNS: keep a public name pointed at this box's current address so a
 *   certificate stays valid and a shared link keeps working after the ISP
 *   changes the address. Pure settings/validation/request-building, one worker
 *   that only calls out when the address actually changed.
 *
 * Dependencies:
 *   - crate::outbound (HTTP), crate::storage::json
 *
 * Usage:
 *   handlers::ddns exposes the endpoints; ddns::run is spawned from web::run;
 *   see docs/guides/ddns.md.
 * =============================================================================
 */

//! Dynamic DNS for HomeVault (see docs/guides/ddns.md).
//!
//! The feature answers one question: *does the name I use to reach this box
//! still point here?* Three pieces:
//!
//! * a settings file (`data/ddns.json`) naming the provider and the name;
//! * a worker that reads the current public address and calls the provider
//!   **only when the address changed** — a home box must not hammer a free
//!   provider, and a provider that rate-limits drops updates silently;
//! * a per-provider request builder, kept pure so the exact URL, method and
//!   body can be asserted in tests without a network.
//!
//! The provider token is write-only from the API's point of view: `GET`
//! answers `"set"`, and the value never appears in a log line.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::outbound::{HttpRequest, HttpResponse, Transport};
use crate::storage::json::JsonStorage;

/// Storage key -> `{DATA_DIRECTORY}/data/ddns.json`.
const STORE_KEY: &str = "ddns";

/// How often the worker looks at the public address. Five minutes is well
/// inside a residential lease, and a DNS record for a home box has no
/// meaningful use for a faster update.
pub const UPDATE_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Per-request deadline towards a provider.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Longest a published record may go without being refreshed, even when the
/// address has not changed.
///
/// A change-only updater is not enough: providers expire a record that is
/// never re-asserted (DuckDNS deletes after ~30 days, DynDNS-style services
/// drop it after their own TTL), and a lease renewal that hands back the same
/// address still resets the provider's clock. One refresh a day costs nothing
/// and keeps the name resolving.
pub const REFRESH_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Which service receives the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DdnsProvider {
    /// A `*.duckdns.org` subdomain.
    #[default]
    DuckDns,
    /// A `*.dynv6.net` (or a custom host on dynv6) name.
    Dynv6,
    /// An A record managed through the Cloudflare API.
    Cloudflare,
    /// An OVHcloud DynHost entry (`username` + `token` as the DynHost
    /// password; HTTP Basic authentication).
    Ovh,
    /// A Namecheap Dynamic DNS record (host + registered `domain` + DDNS
    /// password).
    Namecheap,
    /// A deSEC `dedyn.io` (or custom domain) record; the token travels as
    /// `Authorization: Token …`.
    Desec,
    /// Any provider that accepts an HTTP GET with the address in the URL.
    Custom,
}

impl DdnsProvider {
    pub const ALL: &'static [DdnsProvider] = &[
        DdnsProvider::DuckDns,
        DdnsProvider::Dynv6,
        DdnsProvider::Cloudflare,
        DdnsProvider::Ovh,
        DdnsProvider::Namecheap,
        DdnsProvider::Desec,
        DdnsProvider::Custom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            DdnsProvider::DuckDns => "duckdns",
            DdnsProvider::Dynv6 => "dynv6",
            DdnsProvider::Cloudflare => "cloudflare",
            DdnsProvider::Ovh => "ovh",
            DdnsProvider::Namecheap => "namecheap",
            DdnsProvider::Desec => "desec",
            DdnsProvider::Custom => "custom",
        }
    }

    /// Whether this provider talks to a JSON API rather than a signed update
    /// URL (which changes how a success is recognised).
    pub fn uses_api(self) -> bool {
        matches!(self, DdnsProvider::Cloudflare)
    }
}

/// Lifecycle state shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DdnsStatus {
    #[default]
    Disabled,
    /// Enabled, nothing has been sent yet.
    Unknown,
    /// The provider holds the current address.
    UpToDate,
    /// The last call failed; `last_error` says why.
    Error,
}

impl DdnsStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DdnsStatus::Disabled => "disabled",
            DdnsStatus::Unknown => "unknown",
            DdnsStatus::UpToDate => "up-to-date",
            DdnsStatus::Error => "error",
        }
    }
}

/// Persisted settings (`data/ddns.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DdnsSettings {
    pub enabled: bool,
    #[serde(default)]
    pub provider: DdnsProvider,
    /// The name the provider publishes, e.g. `myhome.duckdns.org`.
    #[serde(default)]
    pub hostname: String,
    /// Provider credential. Write-only from the API's point of view.
    #[serde(default)]
    pub token: String,
    /// `ovh` only: the DynHost login (not a secret, so it is returned).
    #[serde(default)]
    pub username: String,
    /// `namecheap` only: the registered domain the host belongs to. Needed
    /// because the update API takes the host and the domain separately, and
    /// the public suffix cannot be inferred from the host name alone
    /// (`home.example.co.uk`).
    #[serde(default)]
    pub domain: String,
    /// `custom` only: the URL to call, with `{ip}` and optionally `{host}`.
    #[serde(default)]
    pub custom_update_url: String,
    #[serde(default)]
    pub status: DdnsStatus,
    /// The address the provider is believed to hold.
    #[serde(default)]
    pub last_ip: Option<String>,
    /// RFC 3339, UTC.
    #[serde(default)]
    pub last_update: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for DdnsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: DdnsProvider::default(),
            hostname: String::new(),
            token: String::new(),
            username: String::new(),
            domain: String::new(),
            custom_update_url: String::new(),
            status: DdnsStatus::Disabled,
            last_ip: None,
            last_update: None,
            last_error: None,
        }
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

/// Load the saved settings, or the disabled default. A corrupt file degrades
/// to "disabled" instead of failing: dynamic DNS must never stop the server.
pub fn load_settings() -> DdnsSettings {
    match store().load::<DdnsSettings>(STORE_KEY) {
        Ok(Some(s)) => normalize(s),
        Ok(None) => DdnsSettings::default(),
        Err(e) => {
            tracing::warn!("DDNS: cannot read data/ddns.json, treating it as disabled: {e}");
            DdnsSettings::default()
        }
    }
}

pub fn save_settings(s: &DdnsSettings) -> Result<(), String> {
    store()
        .save(STORE_KEY, &normalize(s.clone()))
        .map_err(|e| e.to_string())
}

/// Keep `status` consistent with `enabled`: a disabled entry always reads
/// `disabled`, and an enabled one that was never used cannot claim to be
/// disabled (that is what "no update sent yet" means).
pub fn normalize(mut s: DdnsSettings) -> DdnsSettings {
    s.status = match (s.enabled, s.status) {
        (false, _) => DdnsStatus::Disabled,
        (true, DdnsStatus::Disabled) => DdnsStatus::Unknown,
        (true, other) => other,
    };
    s
}

pub fn clear_settings() {
    let _ = store().delete(STORE_KEY);
}

// --------------------------------------------------------------------------
// Validation
// --------------------------------------------------------------------------

/// A host the provider can publish: DNS labels only, at least two of them.
pub fn validate_hostname(hostname: &str) -> Result<String, String> {
    let h = hostname.trim().to_ascii_lowercase();
    if h.is_empty() {
        return Err("the dynamic DNS name is required".to_string());
    }
    if h.len() > 253 {
        return Err("the dynamic DNS name is too long".to_string());
    }
    if h.contains('/') || h.contains(':') || h.contains('*') {
        return Err("enter a bare host name, without a scheme, port or wildcard".to_string());
    }
    if h.ends_with('.') {
        return Err("do not end the name with a dot".to_string());
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() < 2 {
        return Err("the name needs at least one dot (for example myhome.example.com)".to_string());
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
    Ok(h)
}

/// Validate a proposed configuration. Only enforced while enabled: an admin
/// may disable a half-configured entry and come back to it.
pub fn validate_settings(s: &DdnsSettings) -> Result<(), String> {
    if !s.enabled {
        return Ok(());
    }
    let hostname = validate_hostname(&s.hostname)?;
    match s.provider {
        DdnsProvider::Custom => {
            validate_custom_url(&s.custom_update_url)?;
        }
        DdnsProvider::Ovh => {
            if s.username.trim().is_empty() {
                return Err("OVH DynHost needs a username".to_string());
            }
            if s.token.trim().is_empty() {
                return Err("OVH DynHost needs a password".to_string());
            }
        }
        DdnsProvider::Namecheap => {
            if s.domain.trim().is_empty() {
                return Err("Namecheap needs the registered domain".to_string());
            }
            let domain = validate_hostname(&s.domain)?;
            if hostname != domain && !hostname.ends_with(&format!(".{domain}")) {
                return Err("the host name must be inside the registered domain".to_string());
            }
            if s.token.trim().is_empty() {
                return Err("Namecheap needs a password".to_string());
            }
        }
        _ if s.token.trim().is_empty() => {
            return Err(format!("{} needs an API token", s.provider.as_str()))
        }
        _ => {}
    }
    Ok(())
}

/// A custom update URL must be HTTPS and must carry `{ip}`: otherwise the
/// update cannot convey the address, and plain HTTP would publish it to
/// anyone on the path.
pub fn validate_custom_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("the update URL is required".to_string());
    }
    if !u.starts_with("https://") {
        return Err("the update URL must start with https://".to_string());
    }
    if !u.contains("{ip}") {
        return Err("the update URL must contain {ip}".to_string());
    }
    if u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the update URL contains whitespace".to_string());
    }
    if u.contains("{host}") && !u.contains("{ip}") {
        return Err("the update URL must contain {ip}".to_string());
    }
    Ok(())
}

/// Strip the token before settings leave the process.
pub fn redacted(mut s: DdnsSettings) -> DdnsSettings {
    s.token = if s.token.trim().is_empty() {
        String::new()
    } else {
        "set".to_string()
    };
    s
}

// --------------------------------------------------------------------------
// Request building
// --------------------------------------------------------------------------

/// How a provider's answer proves the record was updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuccessCheck {
    /// The HTTP status is the whole answer (dynv6, Cloudflare, custom).
    StatusOnly,
    /// The body must equal this text (DuckDNS `OK`).
    Exact(String),
    /// The body must start with one of these words (`good` / `nochg`).
    PrefixAny(Vec<String>),
    /// The body must contain this case-insensitive fragment (Namecheap XML).
    Contains(String),
}

/// What the provider expects for one update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCall {
    pub request: HttpRequest,
    pub success: SuccessCheck,
}

/// Build the update call for the configured provider and address.
///
/// Kept pure on purpose: this is where a token ends up in a URL, so it is the
/// one place worth asserting byte for byte in a test.
pub fn build_update(settings: &DdnsSettings, ip: &str) -> Result<UpdateCall, String> {
    let host = validate_hostname(&settings.hostname)?;
    if !is_ipv4(ip) {
        return Err("the address to publish is not an IPv4 address".to_string());
    }
    match settings.provider {
        DdnsProvider::DuckDns => {
            if settings.token.trim().is_empty() {
                return Err("DuckDNS needs a token".to_string());
            }
            // DuckDNS takes the *subdomain*, not the full name.
            let sub = host
                .strip_suffix(".duckdns.org")
                .unwrap_or(&host)
                .to_string();
            Ok(UpdateCall {
                request: HttpRequest::get(&format!(
                    "https://www.duckdns.org/update?domains={sub}&token={}&ip={ip}",
                    settings.token.trim()
                )),
                success: SuccessCheck::Exact("OK".to_string()),
            })
        }
        DdnsProvider::Dynv6 => {
            if settings.token.trim().is_empty() {
                return Err("dynv6 needs a token".to_string());
            }
            Ok(UpdateCall {
                request: HttpRequest::get(&format!(
                    "https://dynv6.com/api/update?hostname={host}&token={}&ipv4={ip}",
                    settings.token.trim()
                )),
                // dynv6 answers "addresses updated" / "no change"; both are a
                // success, and neither is "OK".
                success: SuccessCheck::StatusOnly,
            })
        }
        DdnsProvider::Cloudflare => Ok(UpdateCall {
            // The A record is found and patched through the API; the URL is
            // only the entry point, so no token goes in it.
            request: HttpRequest::get(&format!(
                "https://api.cloudflare.com/client/v4/zones?name={host}"
            ))
            .with_bearer(settings.token.trim()),
            success: SuccessCheck::StatusOnly,
        }),
        DdnsProvider::Ovh => {
            let username = settings.username.trim();
            if username.is_empty() {
                return Err("OVH DynHost needs a username".to_string());
            }
            if settings.token.trim().is_empty() {
                return Err("OVH DynHost needs a password".to_string());
            }
            Ok(UpdateCall {
                request: HttpRequest::get(&format!(
                    "https://www.ovh.com/nic/update?system=dyndns&hostname={host}&myip={ip}"
                ))
                .with_basic_auth(username, settings.token.trim()),
                success: SuccessCheck::PrefixAny(vec!["good".to_string(), "nochg".to_string()]),
            })
        }
        DdnsProvider::Namecheap => {
            if settings.domain.trim().is_empty() {
                return Err("Namecheap needs the registered domain".to_string());
            }
            let domain = validate_hostname(&settings.domain)?;
            if host != domain && !host.ends_with(&format!(".{domain}")) {
                return Err("the host name must be inside the registered domain".to_string());
            }
            if settings.token.trim().is_empty() {
                return Err("Namecheap needs a password".to_string());
            }
            let provider_host = if host == domain {
                "@".to_string()
            } else {
                host.strip_suffix(&format!(".{domain}"))
                    .unwrap_or(&host)
                    .to_string()
            };
            Ok(UpdateCall {
                request: HttpRequest::get(&format!(
                    "https://dynamicdns.park-your-domain.com/update?host={provider_host}&domain={domain}&password={}&ip={ip}",
                    settings.token.trim()
                )),
                // Namecheap answers 200 with XML even on a refusal.
                success: SuccessCheck::Contains("<errcount>0</errcount>".to_string()),
            })
        }
        DdnsProvider::Desec => {
            if settings.token.trim().is_empty() {
                return Err("deSEC needs a token".to_string());
            }
            Ok(UpdateCall {
                request: HttpRequest::get(&format!(
                    "https://update.dedyn.io/nic/update?hostname={host}&myip={ip}"
                ))
                .with_authorization(&format!("Token {}", settings.token.trim())),
                success: SuccessCheck::PrefixAny(vec!["good".to_string(), "nochg".to_string()]),
            })
        }
        DdnsProvider::Custom => {
            validate_custom_url(&settings.custom_update_url)?;
            let url = settings
                .custom_update_url
                .trim()
                .replace("{ip}", ip)
                .replace("{host}", &host);
            Ok(UpdateCall {
                request: HttpRequest::get(&url),
                success: SuccessCheck::StatusOnly,
            })
        }
    }
}

/// Whether a provider's plain-text answer means "the record is current".
pub fn interpret_response(call: &UpdateCall, response: &HttpResponse) -> Result<(), String> {
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "the provider answered HTTP {}: {}",
            response.status,
            first_line(&response.body)
        ));
    }
    let body = response.body.trim();
    match &call.success {
        // The status is the whole answer.
        SuccessCheck::StatusOnly => Ok(()),
        // Some plain-text providers answer 200 with a refusal in the body;
        // the status alone is not enough.
        SuccessCheck::Exact(marker) if body.is_empty() => Err(format!(
            "the provider answered with no body; expected {marker}"
        )),
        SuccessCheck::Exact(marker) if !body.eq_ignore_ascii_case(marker) => Err(format!(
            "the provider refused the update: {}",
            first_line(body)
        )),
        SuccessCheck::Exact(_) => Ok(()),
        SuccessCheck::PrefixAny(prefixes) if body.is_empty() => Err(format!(
            "the provider answered with no body; expected one of {}",
            prefixes.join(" / ")
        )),
        SuccessCheck::PrefixAny(prefixes)
            if !prefixes.iter().any(|prefix| {
                body.to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            }) =>
        {
            Err(format!(
                "the provider refused the update: {}",
                first_line(body)
            ))
        }
        SuccessCheck::PrefixAny(_) => Ok(()),
        SuccessCheck::Contains(marker)
            if !body
                .to_ascii_lowercase()
                .contains(&marker.to_ascii_lowercase()) =>
        {
            Err(format!(
                "the provider refused the update: {}",
                first_line(body)
            ))
        }
        SuccessCheck::Contains(_) => Ok(()),
    }
}

/// First line of a provider body, truncated: enough to act on, never a dump.
fn first_line(body: &str) -> String {
    let line = body.lines().next().unwrap_or("").trim();
    if line.chars().count() > 200 {
        line.chars().take(200).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

// --------------------------------------------------------------------------
// Cloudflare A-record update
// --------------------------------------------------------------------------

/// Update (or create) the A record for `settings.hostname` through the
/// Cloudflare API. Split from [`build_update`] because it needs two lookups
/// before the write.
pub async fn update_cloudflare(
    transport: &dyn Transport,
    settings: &DdnsSettings,
    ip: &str,
) -> Result<(), String> {
    let token = settings.token.trim();
    let host = validate_hostname(&settings.hostname)?;
    let mut zone_id = None;
    for candidate in crate::acme::Cloudflare::zone_candidates(&host) {
        let listing = transport
            .send(
                HttpRequest::get(&format!(
                    "https://api.cloudflare.com/client/v4/zones?name={candidate}"
                ))
                .with_bearer(token),
            )
            .await
            .map_err(|e| e.message)?;
        if let Some(id) = listing.json()["result"]
            .as_array()
            .and_then(|zones| zones.first())
            .and_then(|zone| zone["id"].as_str())
        {
            zone_id = Some(id.to_string());
            break;
        }
    }
    let zone_id = zone_id.ok_or_else(|| format!("the token cannot see any zone for {host}"))?;

    let records = transport
        .send(
            HttpRequest::get(&format!(
                "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records?type=A&name={host}"
            ))
            .with_bearer(token),
        )
        .await
        .map_err(|e| e.message)?;
    let existing = records.json()["result"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|r| r["id"].as_str())
        .map(str::to_string);

    let body = serde_json::json!({
        "type": "A",
        "name": host,
        "content": ip,
        "ttl": 120,
        "proxied": false,
    })
    .to_string();
    let request = match existing {
        Some(id) => HttpRequest::put(
            &format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{id}"),
            body,
            "application/json",
        ),
        None => HttpRequest::post(
            &format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records"),
            body,
            "application/json",
        ),
    }
    .with_bearer(token);
    let response = transport.send(request).await.map_err(|e| e.message)?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "the provider refused the record: {}",
            first_line(&response.body)
        ));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Public address detection
// --------------------------------------------------------------------------

/// The address this box would use to reach the internet.
///
/// A UDP socket `connect` sends nothing: it only asks the kernel which source
/// address it would pick, which is exactly the address a provider should
/// publish. No external service is contacted, so detection works offline and
/// leaks nothing.
pub fn detect_public_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("1.1.1.1:53").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    match ip {
        std::net::IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => {
            Some(v4.to_string())
        }
        _ => None,
    }
}

/// Whether an update has to be sent because the address the provider holds
/// differs from the current one.
pub fn needs_update(last_ip: Option<&str>, current: &str) -> bool {
    match last_ip {
        Some(last) => last.trim() != current.trim(),
        None => true,
    }
}

/// Whether a periodic refresh is due, even though the address is unchanged.
///
/// `last_update` is the RFC 3339 stamp of the last accepted update. A missing
/// or unparsable stamp counts as due: the record's age is unknown, and
/// re-asserting it is harmless.
pub fn refresh_due(last_update: Option<&str>, now_unix: i64) -> bool {
    let Some(stamp) = last_update else {
        return true;
    };
    match crate::acme::parse_rfc3339_seconds(stamp) {
        Some(last) => now_unix.saturating_sub(last) >= REFRESH_EVERY.as_secs() as i64,
        None => true,
    }
}

/// Whether the worker should call the provider on this pass: the address
/// changed, or the record is old enough that it has to be re-asserted.
pub fn should_update(
    last_ip: Option<&str>,
    last_update: Option<&str>,
    current: &str,
    now_unix: i64,
) -> bool {
    needs_update(last_ip, current) || refresh_due(last_update, now_unix)
}

fn is_ipv4(value: &str) -> bool {
    value.parse::<std::net::Ipv4Addr>().is_ok()
}

// --------------------------------------------------------------------------
// Update + worker
// --------------------------------------------------------------------------

/// Send one update and return the address that was published.
pub async fn update_now(
    transport: &dyn Transport,
    settings: &DdnsSettings,
    ip: &str,
) -> Result<(), String> {
    validate_settings(settings)?;
    match settings.provider {
        DdnsProvider::Cloudflare => update_cloudflare(transport, settings, ip).await,
        _ => {
            let call = build_update(settings, ip)?;
            let response = transport
                .send(call.request.clone())
                .await
                .map_err(|e| e.message)?;
            interpret_response(&call, &response)
        }
    }
}

/// One worker pass: detect, compare, update, record. Returns the address that
/// is now published, when an update was sent.
pub async fn tick(transport: &dyn Transport) -> Result<Option<String>, String> {
    let mut settings = load_settings();
    if !settings.enabled {
        return Ok(None);
    }
    let ip = match detect_public_ip() {
        Some(ip) => ip,
        None => {
            settings.status = DdnsStatus::Error;
            settings.last_error =
                Some("no public IPv4 address is available on this host".to_string());
            let _ = save_settings(&settings);
            return Err("no public IPv4 address is available on this host".to_string());
        }
    };
    // An update is sent when the address moved, and at least once per
    // [`REFRESH_EVERY`] so the provider keeps the record alive.
    if !should_update(
        settings.last_ip.as_deref(),
        settings.last_update.as_deref(),
        &ip,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    ) {
        if settings.status != DdnsStatus::UpToDate {
            settings.status = DdnsStatus::UpToDate;
            settings.last_error = None;
            let _ = save_settings(&settings);
        }
        return Ok(None);
    }
    match update_now(transport, &settings, &ip).await {
        Ok(()) => {
            settings.status = DdnsStatus::UpToDate;
            settings.last_ip = Some(ip.clone());
            settings.last_update = Some(crate::acme::now_rfc3339());
            settings.last_error = None;
            let _ = save_settings(&settings);
            Ok(Some(ip))
        }
        Err(e) => {
            settings.status = DdnsStatus::Error;
            settings.last_error = Some(e.clone());
            let _ = save_settings(&settings);
            Err(e)
        }
    }
}

/// The background loop. A failure is recorded and retried on the next pass;
/// the loop itself never exits.
pub async fn run() {
    // Give the server time to come up before the first outbound call.
    tokio::time::sleep(Duration::from_secs(45)).await;
    loop {
        let transport = match crate::outbound::ReqwestTransport::new() {
            Ok(transport) => transport,
            Err(e) => {
                tracing::warn!("DDNS: cannot build the HTTP client: {}", e.message);
                tokio::time::sleep(UPDATE_INTERVAL).await;
                continue;
            }
        };
        if let Err(e) = tick(&transport).await {
            tracing::warn!("DDNS update failed: {e}");
        }
        tokio::time::sleep(UPDATE_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::BoxFuture;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn base() -> DdnsSettings {
        DdnsSettings {
            enabled: true,
            provider: DdnsProvider::DuckDns,
            hostname: "myhome.duckdns.org".to_string(),
            token: "secret".to_string(),
            ..Default::default()
        }
    }

    struct Scripted {
        replies: Mutex<VecDeque<HttpResponse>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Scripted {
        fn new(replies: Vec<HttpResponse>) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(Vec::new()),
            })
        }
    }

    impl Transport for Scripted {
        fn send<'a>(
            &'a self,
            request: HttpRequest,
        ) -> BoxFuture<'a, Result<HttpResponse, crate::acme::AcmeError>> {
            self.seen.lock().unwrap().push(request);
            let next = self.replies.lock().unwrap().pop_front();
            Box::pin(async move {
                next.ok_or_else(|| crate::acme::AcmeError::new("unexpected request", false))
            })
        }
    }

    fn reply(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            body: body.to_string(),
            headers: Default::default(),
        }
    }

    #[test]
    fn test_validate_hostname_accepts_a_normal_name() {
        assert_eq!(
            validate_hostname(" MyHome.DuckDNS.org ").unwrap(),
            "myhome.duckdns.org"
        );
    }

    #[test]
    fn test_validate_hostname_rejects_bad_shapes() {
        for bad in [
            "",
            "localhost",
            "https://myhome.duckdns.org",
            "myhome.duckdns.org:80",
            "*.duckdns.org",
            "my home.duckdns.org",
            "myhome.duckdns.org.",
            "-bad.example.com",
            "a..b.com",
        ] {
            assert!(validate_hostname(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn test_disabled_settings_are_never_validated() {
        let s = DdnsSettings::default();
        assert!(
            validate_settings(&s).is_ok(),
            "a disabled entry may be incomplete"
        );
    }

    #[test]
    fn test_every_provider_but_custom_needs_a_token() {
        for provider in [
            DdnsProvider::DuckDns,
            DdnsProvider::Dynv6,
            DdnsProvider::Cloudflare,
            DdnsProvider::Desec,
        ] {
            let s = DdnsSettings {
                enabled: true,
                provider,
                hostname: "myhome.example.com".to_string(),
                ..Default::default()
            };
            assert!(
                validate_settings(&s).is_err(),
                "{provider:?} without a token"
            );
        }
    }

    #[test]
    fn test_custom_provider_requires_https_and_an_ip_placeholder() {
        let mut s = DdnsSettings {
            enabled: true,
            provider: DdnsProvider::Custom,
            hostname: "myhome.example.com".to_string(),
            custom_update_url: "http://example.com/update?ip={ip}".to_string(),
            ..Default::default()
        };
        assert!(validate_settings(&s).is_err(), "plain HTTP is refused");
        s.custom_update_url = "https://example.com/update?host={host}".to_string();
        assert!(validate_settings(&s).is_err(), "no address placeholder");
        s.custom_update_url = "https://example.com/update?host={host}&ip={ip}".to_string();
        assert!(validate_settings(&s).is_ok());
    }

    #[test]
    fn test_duckdns_url_carries_the_subdomain_the_token_and_the_address() {
        let call = build_update(&base(), "203.0.113.7").unwrap();
        assert_eq!(
            call.request.url,
            "https://www.duckdns.org/update?domains=myhome&token=secret&ip=203.0.113.7"
        );
        assert_eq!(call.success, SuccessCheck::Exact("OK".to_string()));
    }

    #[test]
    fn test_dynv6_url_carries_the_full_hostname() {
        let s = DdnsSettings {
            provider: DdnsProvider::Dynv6,
            hostname: "home.dynv6.net".to_string(),
            token: "tok".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert_eq!(
            call.request.url,
            "https://dynv6.com/api/update?hostname=home.dynv6.net&token=tok&ipv4=198.51.100.4"
        );
    }

    #[test]
    fn test_ovh_uses_basic_auth_and_keeps_the_password_out_of_the_url() {
        let s = DdnsSettings {
            provider: DdnsProvider::Ovh,
            hostname: "home.example.com".to_string(),
            username: "dynhost-user".to_string(),
            token: "ovh-password".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert_eq!(
            call.request.url,
            "https://www.ovh.com/nic/update?system=dyndns&hostname=home.example.com&myip=198.51.100.4"
        );
        assert!(!call.request.url.contains("ovh-password"));
        assert_eq!(
            call.request.basic_auth,
            Some(("dynhost-user".to_string(), "ovh-password".to_string()))
        );
        assert_eq!(
            call.success,
            SuccessCheck::PrefixAny(vec!["good".into(), "nochg".into()])
        );
    }

    #[test]
    fn test_ovh_requires_a_username() {
        let s = DdnsSettings {
            provider: DdnsProvider::Ovh,
            hostname: "home.example.com".to_string(),
            token: "ovh-password".to_string(),
            ..base()
        };
        assert!(validate_settings(&s).is_err());
        assert!(build_update(&s, "198.51.100.4").is_err());
    }

    #[test]
    fn test_namecheap_splits_the_host_from_the_registered_domain() {
        let s = DdnsSettings {
            provider: DdnsProvider::Namecheap,
            hostname: "home.example.com".to_string(),
            domain: "example.com".to_string(),
            token: "nc-password".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert_eq!(
            call.request.url,
            "https://dynamicdns.park-your-domain.com/update?host=home&domain=example.com&password=nc-password&ip=198.51.100.4"
        );
        assert_eq!(
            call.success,
            SuccessCheck::Contains("<errcount>0</errcount>".to_string())
        );

        // The apex is spelled `@`, which is what Namecheap expects.
        let apex = DdnsSettings {
            hostname: "example.com".to_string(),
            ..s
        };
        let call = build_update(&apex, "198.51.100.4").unwrap();
        assert!(call.request.url.contains("host=@&domain=example.com"));
    }

    #[test]
    fn test_namecheap_refuses_a_host_outside_the_configured_domain() {
        let s = DdnsSettings {
            provider: DdnsProvider::Namecheap,
            hostname: "home.example.net".to_string(),
            domain: "example.com".to_string(),
            token: "nc-password".to_string(),
            ..base()
        };
        assert!(validate_settings(&s).is_err());
        assert!(build_update(&s, "198.51.100.4").is_err());
    }

    #[test]
    fn test_desec_uses_a_token_authorization_header() {
        let s = DdnsSettings {
            provider: DdnsProvider::Desec,
            hostname: "home.dedyn.io".to_string(),
            token: "desec-token".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert_eq!(
            call.request.url,
            "https://update.dedyn.io/nic/update?hostname=home.dedyn.io&myip=198.51.100.4"
        );
        assert_eq!(
            call.request.authorization.as_deref(),
            Some("Token desec-token")
        );
        assert!(call.request.bearer.is_none());
        assert!(!call.request.url.contains("desec-token"));
    }

    #[test]
    fn test_good_and_nochg_are_both_successes() {
        let s = DdnsSettings {
            provider: DdnsProvider::Desec,
            hostname: "home.dedyn.io".to_string(),
            token: "desec-token".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert!(interpret_response(&call, &reply(200, "good 198.51.100.4")).is_ok());
        assert!(interpret_response(&call, &reply(200, "nochg 198.51.100.4")).is_ok());
        assert!(interpret_response(&call, &reply(200, "badauth")).is_err());
    }

    #[test]
    fn test_namecheap_refusal_is_detected_in_a_200_xml_body() {
        let s = DdnsSettings {
            provider: DdnsProvider::Namecheap,
            hostname: "home.example.com".to_string(),
            domain: "example.com".to_string(),
            token: "nc-password".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert!(interpret_response(
            &call,
            &reply(
                200,
                "<interface-response><ErrCount>0</ErrCount><Done>true</Done></interface-response>"
            )
        )
        .is_ok());
        assert!(interpret_response(
            &call,
            &reply(200, "<interface-response><ErrCount>1</ErrCount><errors><Err1>Invalid password</Err1></errors></interface-response>")
        )
        .is_err());
    }

    #[test]
    fn test_cloudflare_never_puts_the_token_in_the_url() {
        let s = DdnsSettings {
            provider: DdnsProvider::Cloudflare,
            hostname: "home.example.com".to_string(),
            token: "cf-secret".to_string(),
            ..base()
        };
        let call = build_update(&s, "198.51.100.4").unwrap();
        assert!(!call.request.url.contains("cf-secret"));
        assert_eq!(call.request.bearer.as_deref(), Some("cf-secret"));
    }

    #[test]
    fn test_custom_url_substitutes_both_placeholders() {
        let s = DdnsSettings {
            provider: DdnsProvider::Custom,
            hostname: "home.example.com".to_string(),
            custom_update_url: "https://dns.example.com/u?h={host}&a={ip}".to_string(),
            ..base()
        };
        let call = build_update(&s, "203.0.113.9").unwrap();
        assert_eq!(
            call.request.url,
            "https://dns.example.com/u?h=home.example.com&a=203.0.113.9"
        );
    }

    #[test]
    fn test_build_update_refuses_an_address_that_is_not_ipv4() {
        assert!(build_update(&base(), "2001:db8::1").is_err());
        assert!(build_update(&base(), "not-an-ip").is_err());
    }

    #[test]
    fn test_a_plain_text_refusal_is_an_error_even_with_200() {
        let call = build_update(&base(), "203.0.113.7").unwrap();
        assert!(interpret_response(&call, &reply(200, "OK")).is_ok());
        assert!(interpret_response(&call, &reply(200, "KO")).is_err());
        assert!(interpret_response(&call, &reply(200, "")).is_err());
        assert!(interpret_response(&call, &reply(500, "boom")).is_err());
    }

    #[test]
    fn test_redaction_hides_the_token_but_keeps_its_presence_visible() {
        let r = redacted(base());
        assert_eq!(r.token, "set");
        assert!(!serde_json::to_string(&r).unwrap().contains("\"secret\""));
        let mut s = base();
        s.token = "  ".to_string();
        assert_eq!(redacted(s).token, "");
    }

    #[test]
    fn test_needs_update_only_when_the_address_changed() {
        assert!(needs_update(None, "203.0.113.7"));
        assert!(needs_update(Some("198.51.100.4"), "203.0.113.7"));
        assert!(!needs_update(Some("203.0.113.7"), "203.0.113.7"));
    }

    #[test]
    fn test_settings_roundtrip_and_legacy_defaults() {
        let s = base();
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"duckdns\""));
        let back: DdnsSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back, s);

        let legacy = normalize(
            serde_json::from_str::<DdnsSettings>(
                r#"{"enabled":true,"hostname":"myhome.duckdns.org"}"#,
            )
            .unwrap(),
        );
        assert_eq!(legacy.provider, DdnsProvider::DuckDns);
        assert_eq!(legacy.status, DdnsStatus::Unknown);
        assert!(legacy.token.is_empty());
    }

    #[tokio::test]
    async fn test_update_now_posts_the_built_request_and_reports_a_refusal() {
        let ca = Scripted::new(vec![reply(200, "OK"), reply(200, "KO")]);
        update_now(&*ca, &base(), "203.0.113.7").await.unwrap();
        {
            let seen = ca.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert!(seen[0].url.contains("ip=203.0.113.7"));
        }

        let err = update_now(&*ca, &base(), "203.0.113.8").await.unwrap_err();
        assert!(err.contains("refused"), "{err}");
    }

    #[tokio::test]
    async fn test_cloudflare_patches_an_existing_a_record() {
        let ca = Scripted::new(vec![
            reply(200, r#"{"result":[{"id":"zone-1"}]}"#),
            reply(200, r#"{"result":[{"id":"record-1"}]}"#),
            reply(200, r#"{"success":true}"#),
        ]);
        let s = DdnsSettings {
            provider: DdnsProvider::Cloudflare,
            hostname: "home.example.com".to_string(),
            token: "cf-secret".to_string(),
            ..base()
        };
        update_now(&*ca, &s, "203.0.113.7").await.unwrap();
        let seen = ca.seen.lock().unwrap();
        assert!(seen[0].url.ends_with("/zones?name=home.example.com"));
        assert!(seen[1].url.contains("type=A&name=home.example.com"));
        assert!(seen[2].url.ends_with("/dns_records/record-1"));
        assert_eq!(seen[2].method, crate::outbound::HttpMethod::Put);
        assert!(seen[2].body.as_deref().unwrap().contains("203.0.113.7"));
    }

    #[tokio::test]
    async fn test_cloudflare_creates_the_record_when_it_is_missing() {
        let ca = Scripted::new(vec![
            reply(200, r#"{"result":[{"id":"zone-1"}]}"#),
            reply(200, r#"{"result":[]}"#),
            reply(200, r#"{"success":true}"#),
        ]);
        let s = DdnsSettings {
            provider: DdnsProvider::Cloudflare,
            hostname: "home.example.com".to_string(),
            token: "cf-secret".to_string(),
            ..base()
        };
        update_now(&*ca, &s, "203.0.113.7").await.unwrap();
        let seen = ca.seen.lock().unwrap();
        assert_eq!(seen[2].method, crate::outbound::HttpMethod::Post);
        assert!(seen[2].url.ends_with("/dns_records"));
    }
}
