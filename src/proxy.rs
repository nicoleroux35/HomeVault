/* =============================================================================
 * File          : proxy.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026
 * Last modified : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Outbound HTTP(S) proxy for HomeVault's third-party calls (certificate
 *   automation, dynamic DNS, update checks, notifications and Web Push). One
 *   persisted setting, one client-builder helper, and local ranges that
 *   cannot be proxied however the admin configures the list.
 *
 * Dependencies:
 *   - serde (>= 1.0), reqwest (>= 0.13), crate::storage::json
 *
 * Usage:
 *   `proxy::apply(builder, &proxy::load_settings())` before every
 *   `reqwest::Client::builder()` that talks to the internet. The Settings
 *   card writes through `handlers::proxy`; see docs/guides/settings.md.
 * =============================================================================
 */

//! Outbound HTTP(S) proxy configuration (Settings → Security → Outbound proxy).
//!
//! The setting is deliberately small: an on/off switch, one proxy URL per
//! scheme and an optional `no_proxy` list. Credentials embedded in a proxy URL
//! are refused — a secret in a URL ends up in a log line, an error message or
//! an export, and HomeVault never stores one that way.
//!
//! Loopback and `localhost` are always bypassed, whatever the admin types.
//! Proxying the appliance's own addresses would break health checks and the
//! app proxy, and it would send internal traffic to a third party.

use serde::{Deserialize, Serialize};

use crate::storage::json::JsonStorage;

/// Key under `{DATA_DIRECTORY}/data/`.
pub const STORE_KEY: &str = "proxy";

/// Destinations that are **always** direct, whatever the admin types.
///
/// Loopback, the link-local block, every RFC 1918 private range, IPv6 ULA and
/// link-local, and mDNS `.local` names are local traffic: sending them to a
/// third-party proxy would leak the home network's topology and break the
/// appliance's own health checks. `localhost` also covers `*.localhost` in
/// reqwest's matcher; the numeric forms cover a caller that bypasses DNS.
pub const ALWAYS_DIRECT: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "::1",
    "0.0.0.0",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
    ".local",
];

/// A proxy URL longer than this is not a proxy URL.
pub const MAX_URL_LEN: usize = 512;

/// One `no_proxy` token longer than this is not a host name.
pub const MAX_TOKEN_LEN: usize = 253;

/// Persisted settings (`data/proxy.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProxySettings {
    /// When off, every outbound client explicitly disables system proxy
    /// environment variables, so the appliance behaves the same on every host.
    #[serde(default)]
    pub enabled: bool,
    /// Proxy for `http://` destinations, e.g. `http://proxy.lan:3128`.
    #[serde(default)]
    pub http_proxy: String,
    /// Proxy for `https://` destinations (CONNECT tunnel).
    #[serde(default)]
    pub https_proxy: String,
    /// Comma-separated hosts/CIDRs that go direct. [`ALWAYS_DIRECT`] is always
    /// added on top of this list.
    #[serde(default)]
    pub no_proxy: String,
}

fn store() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// Load the saved settings, or the disabled default. A corrupt file degrades
/// to "disabled" instead of failing: a bad proxy setting must never stop the
/// server or silently redirect traffic.
pub fn load_settings() -> ProxySettings {
    match store().load::<ProxySettings>(STORE_KEY) {
        Ok(Some(settings)) => normalize(settings),
        Ok(None) => ProxySettings::default(),
        Err(e) => {
            tracing::warn!("Proxy: cannot read data/proxy.json, treating it as disabled: {e}");
            ProxySettings::default()
        }
    }
}

/// Persist the settings. Values are trimmed so the file and the API agree.
pub fn save_settings(settings: &ProxySettings) -> Result<(), String> {
    store()
        .save(STORE_KEY, &normalize(settings.clone()))
        .map_err(|e| e.to_string())
}

/// Reset to the shipped default (`disabled`).
pub fn clear_settings() {
    let _ = store().delete(STORE_KEY);
}

fn normalize(mut settings: ProxySettings) -> ProxySettings {
    settings.http_proxy = settings.http_proxy.trim().to_string();
    settings.https_proxy = settings.https_proxy.trim().to_string();
    settings.no_proxy = settings.no_proxy.trim().to_string();
    settings
}

/// A stable key for caches that must rebuild when the setting changes.
pub fn cache_key() -> String {
    let settings = load_settings();
    format!(
        "{}|{}|{}|{}",
        settings.enabled, settings.http_proxy, settings.https_proxy, settings.no_proxy
    )
}

/// The `no_proxy` string actually handed to reqwest: the admin's list plus the
/// loopback exemptions, de-duplicated case-insensitively and in a stable order.
pub fn effective_no_proxy(user_list: &str) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in ALWAYS_DIRECT.iter().copied().chain(user_list.split(',')) {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        // reqwest accepts a leading dot (`.example.com`) and treats it as
        // "this domain and its subdomains"; it does not understand a `*.`
        // wildcard. Normalise the shape users type so it cannot be silently
        // ignored.
        let token = token
            .strip_prefix("*.")
            .map(|suffix| format!(".{suffix}"))
            .unwrap_or_else(|| token.to_string());
        let folded = token.to_ascii_lowercase();
        if seen.contains(&folded) {
            continue;
        }
        seen.push(folded);
        entries.push(token);
    }
    entries.join(",")
}

/// Validate a proxy URL. Only `http://` and `https://` are supported by this
/// build (reqwest is compiled without the SOCKS feature); credentials in the
/// URL are refused so a secret never enters `data/proxy.json` or an error.
pub fn validate_proxy_url(field: &str, raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Ok(String::new());
    }
    if url.len() > MAX_URL_LEN {
        return Err(format!("{field} is longer than {MAX_URL_LEN} characters"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("{field} contains whitespace"));
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("socks") {
        return Err(format!(
            "{field}: SOCKS proxies are not supported by this build — use an http:// or https:// proxy"
        ));
    }
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("{field} must start with http:// or https://"));
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("{field} is not a valid URL: {e}"))?;
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(format!("{field} must name a proxy host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!(
            "{field} must not embed credentials — proxy authentication is not supported"
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!("{field} must not contain a query or fragment"));
    }
    Ok(url.to_string())
}

/// Validate the whole configuration. A disabled entry may be empty; a value
/// that is present is always checked, so a typo cannot be saved and only fail
/// later on a live certificate renewal.
pub fn validate_settings(settings: &ProxySettings) -> Result<(), String> {
    validate_proxy_url("http_proxy", &settings.http_proxy)?;
    validate_proxy_url("https_proxy", &settings.https_proxy)?;
    validate_no_proxy(&settings.no_proxy)?;
    if settings.enabled
        && settings.http_proxy.trim().is_empty()
        && settings.https_proxy.trim().is_empty()
    {
        return Err("enable at least one proxy URL (http_proxy or https_proxy)".to_string());
    }
    Ok(())
}

/// Validate the optional `no_proxy` list (comma-separated hosts, domains,
/// IP addresses or CIDRs).
pub fn validate_no_proxy(raw: &str) -> Result<(), String> {
    for token in raw.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if token.len() > MAX_TOKEN_LEN {
            return Err(format!(
                "each no_proxy entry must be at most {MAX_TOKEN_LEN} characters"
            ));
        }
        if token.contains("://") {
            return Err("no_proxy entries are bare hosts, not URLs".to_string());
        }
        if !token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '/' | '*' | '_'))
        {
            return Err(format!(
                "no_proxy entry '{token}' may only contain letters, digits, dots, hyphens, colons, slashes, '*' and '_'"
            ));
        }
        // reqwest only understands the `*` wildcard on its own; a user typing
        // `*.example.com` would otherwise be silently ignored. The only
        // accepted shapes are `*`, `*.domain` and a bare host/CIDR.
        if token.contains('*') && token != "*" && !token.starts_with("*.") {
            return Err(format!(
                "no_proxy entry '{token}': use '*', '*.example.com' or a bare host"
            ));
        }
    }
    Ok(())
}

/// Build the reqwest proxy list. Pure apart from constructing `reqwest::Proxy`
/// values, so the decision (which URL, which exemption list) is testable.
pub fn proxies(settings: &ProxySettings) -> Result<Vec<reqwest::Proxy>, String> {
    if !settings.enabled {
        return Ok(Vec::new());
    }
    validate_settings(settings)?;
    let exemptions = reqwest::NoProxy::from_string(&effective_no_proxy(&settings.no_proxy));
    let mut list = Vec::new();
    let http = settings.http_proxy.trim();
    if !http.is_empty() {
        let proxy = reqwest::Proxy::http(http)
            .map_err(|e| format!("http_proxy is not a usable proxy URL: {e}"))?
            .no_proxy(exemptions.clone());
        list.push(proxy);
    }
    let https = settings.https_proxy.trim();
    if !https.is_empty() {
        let proxy = reqwest::Proxy::https(https)
            .map_err(|e| format!("https_proxy is not a usable proxy URL: {e}"))?
            .no_proxy(exemptions);
        list.push(proxy);
    }
    Ok(list)
}

/// Apply the configuration to a client builder.
///
/// When the setting is off — or the stored file is invalid — the builder is
/// explicitly told `no_proxy()`, so `HTTP_PROXY`/`HTTPS_PROXY` in the service
/// environment cannot redirect traffic behind the admin's back. A bad stored
/// file is logged and ignored rather than crashing a background worker.
pub fn apply(builder: reqwest::ClientBuilder, settings: &ProxySettings) -> reqwest::ClientBuilder {
    match proxies(settings) {
        Ok(list) if !list.is_empty() => list.into_iter().fold(builder, |b, proxy| b.proxy(proxy)),
        Ok(_) => builder.no_proxy(),
        Err(e) => {
            tracing::warn!("Proxy: ignoring the stored configuration: {e}");
            builder.no_proxy()
        }
    }
}

/// Apply the settings currently on disk.
pub fn apply_loaded(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    apply(builder, &load_settings())
}

#[cfg(test)]
mod tests {
    //! Validation and the loopback exemption are the security-relevant parts:
    //! a proxy URL is user input handed to an HTTP client, and the exemption
    //! list is what keeps internal traffic internal.
    //!
    //! Run: `cargo test proxy::tests`

    use super::*;

    #[test]
    fn test_validate_proxy_url_accepts_http_and_https() {
        assert_eq!(
            validate_proxy_url("http_proxy", " http://proxy.lan:3128 ").unwrap(),
            "http://proxy.lan:3128"
        );
        assert_eq!(
            validate_proxy_url("https_proxy", "https://proxy.example.com").unwrap(),
            "https://proxy.example.com"
        );
        assert_eq!(validate_proxy_url("http_proxy", "").unwrap(), "");
    }

    #[test]
    fn test_validate_proxy_url_refuses_credentials_and_unsupported_schemes() {
        for (field, value, needle) in [
            ("http_proxy", "socks5://127.0.0.1:1080", "SOCKS"),
            ("http_proxy", "ftp://proxy.lan:21", "http:// or https://"),
            (
                "http_proxy",
                "http://user:secret@proxy.lan:3128",
                "credentials",
            ),
            ("http_proxy", "http://proxy.lan:3128/path?x=1", "query"),
            ("http_proxy", "http://proxy lan:3128", "whitespace"),
            ("http_proxy", "http://", "valid URL"),
        ] {
            let err = validate_proxy_url(field, value).unwrap_err();
            assert!(
                err.contains(needle),
                "{value} must be refused with '{needle}': {err}"
            );
        }
    }

    #[test]
    fn test_validate_settings_requires_a_url_only_when_enabled() {
        let disabled = ProxySettings::default();
        assert!(validate_settings(&disabled).is_ok());

        let enabled_empty = ProxySettings {
            enabled: true,
            ..Default::default()
        };
        assert!(validate_settings(&enabled_empty)
            .unwrap_err()
            .contains("at least one proxy URL"));

        let enabled_http = ProxySettings {
            enabled: true,
            http_proxy: "http://proxy.lan:3128".to_string(),
            ..Default::default()
        };
        assert!(validate_settings(&enabled_http).is_ok());
    }

    #[test]
    fn test_validate_no_proxy_accepts_hosts_cidrs_and_wildcards() {
        for value in [
            "",
            "example.com",
            "example.com, 192.168.1.0/24, ::1",
            "*.lan",
            "nas_home.lan",
        ] {
            assert!(validate_no_proxy(value).is_ok(), "{value}");
        }
    }

    #[test]
    fn test_validate_no_proxy_refuses_urls_whitespace_and_stray_wildcards() {
        assert!(validate_no_proxy("http://example.com").is_err());
        assert!(validate_no_proxy("example.com;rm -rf /").is_err());
        assert!(validate_no_proxy("foo*bar.example.com").is_err());
    }

    #[test]
    fn test_effective_no_proxy_normalises_a_star_dot_wildcard() {
        // reqwest only understands a leading dot, not `*.`; the server must
        // translate instead of silently dropping the entry.
        let effective = effective_no_proxy("*.example.com");
        assert!(effective.contains(".example.com"), "{effective}");
        assert!(!effective.contains("*.example.com"), "{effective}");
    }

    #[test]
    fn test_effective_no_proxy_always_exempts_loopback() {
        let effective = effective_no_proxy("");
        for exempt in ALWAYS_DIRECT {
            assert!(effective.split(',').any(|e| e == *exempt), "{effective}");
        }
        // The private ranges are part of the always-direct contract.
        assert!(effective.contains("192.168.0.0/16"), "{effective}");
        assert!(effective.contains(".local"), "{effective}");

        // A user list is merged, not replaced, and duplicate entries collapse.
        let merged = effective_no_proxy("example.com,127.0.0.1,example.com");
        assert!(merged.starts_with("localhost,127.0.0.1,::1,0.0.0.0"));
        assert_eq!(merged.matches("example.com").count(), 1, "{merged}");
    }

    #[test]
    fn test_proxies_are_empty_when_disabled_and_apply_the_exemption_list() {
        let disabled = ProxySettings::default();
        assert!(proxies(&disabled).unwrap().is_empty());

        let enabled = ProxySettings {
            enabled: true,
            http_proxy: "http://proxy.lan:3128".to_string(),
            https_proxy: "http://proxy.lan:3128".to_string(),
            no_proxy: "example.com".to_string(),
        };
        assert_eq!(proxies(&enabled).unwrap().len(), 2);

        // A configuration that validates on save but is corrupted on disk is
        // refused by `proxies` rather than handed to reqwest.
        let invalid = ProxySettings {
            enabled: true,
            http_proxy: "socks5://127.0.0.1:1080".to_string(),
            ..Default::default()
        };
        assert!(proxies(&invalid).is_err());
    }

    #[test]
    fn test_a_proxy_file_without_the_new_fields_still_loads() {
        // Forward compatibility: an older or hand-written file only carrying
        // the switch must not fail to deserialize.
        let parsed: ProxySettings =
            serde_json::from_str(r#"{"enabled":true}"#).expect("serde defaults");
        assert!(parsed.enabled);
        assert!(parsed.http_proxy.is_empty());
        assert!(parsed.https_proxy.is_empty());
        assert!(parsed.no_proxy.is_empty());
    }

    #[test]
    fn test_normalize_trims_every_stored_value() {
        let normalized = normalize(ProxySettings {
            enabled: true,
            http_proxy: "  http://proxy.lan:3128 ".to_string(),
            https_proxy: "\thttp://proxy.lan:3128\n".to_string(),
            no_proxy: " example.com ".to_string(),
        });
        assert_eq!(normalized.http_proxy, "http://proxy.lan:3128");
        assert_eq!(normalized.https_proxy, "http://proxy.lan:3128");
        assert_eq!(normalized.no_proxy, "example.com");
    }
}
