/* =============================================================================
 * File          : origin.rs
 * Author        : Nicolas Le Roux
 * Created       : 23/10/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Server-side resolution of the host name used to build app-proxy
 *   redirects. The `Host` request header is only honoured when it names this
 *   appliance (configured host, OS host name, `<host>.local`, `localhost`,
 *   loopback, the primary LAN address or an operator-declared trusted host);
 *   anything else falls back to the configured canonical host, so a forged or
 *   proxy-supplied `Host` can never turn an auth redirect into an open
 *   redirect.
 *
 * Dependencies:
 *   - hostname (>= 0.4)
 *
 * Usage:
 *   `origin::resolve_redirect_host(...)` in app_proxy/serve.rs (gate
 *   challenge) and handlers/app_auth.rs (post-auth redirect);
 *   `origin::authority(host, port)` to format `host:port` for IPv6.
 * =============================================================================
 */

//! Trusted-origin resolution for the per-app reverse proxy.
//!
//! Both sides of the app-auth round-trip build an absolute URL back to the
//! appliance: the gate's `302` to `/app-auth` on the main origin, and
//! `/app-auth`'s `302` to the app's proxy origin. The host in those URLs used
//! to come straight from the request's `Host` header, which makes the redirect
//! attacker-influenced whenever a forged header reaches the handler (or a
//! reverse proxy in front re-writes it) — an open redirect that also carries a
//! freshly minted auth cookie.
//!
//! Resolution is therefore server-side only:
//!
//! 1. The candidate host is normalised and validated as a host (name, IPv4 or
//!    bracketed IPv6 — never a URL, userinfo, path or list).
//! 2. It is accepted **only** when it is one of this appliance's own names:
//!    `HOMEVAULT_HOST`, the OS host name and its `.local` form, `localhost`,
//!    loopback addresses, the primary LAN address, or an entry listed in
//!    `HOMEVAULT_TRUSTED_HOSTS` (comma-separated; the escape hatch for a custom
//!    DNS name or a second NIC).
//! 3. Otherwise the canonical host is used. A request can therefore never make
//!    the server redirect to a host it does not already answer as.

use std::net::{IpAddr, UdpSocket};

/// Canonical public host name for redirects (env: `HOMEVAULT_HOST`). Defaults
/// to the OS host name.
pub const HOST_ENV: &str = "HOMEVAULT_HOST";

/// Extra host names this appliance is reachable as, comma-separated
/// (env: `HOMEVAULT_TRUSTED_HOSTS`) — for a custom DNS name or a second
/// interface that autodetection cannot know about.
pub const TRUSTED_HOSTS_ENV: &str = "HOMEVAULT_TRUSTED_HOSTS";

/// The host a redirect is built with: the configured `HOMEVAULT_HOST`, else
/// the OS host name, else `localhost`.
pub fn canonical_host() -> String {
    if let Ok(raw) = std::env::var(HOST_ENV) {
        if let Some(host) = normalize_host(&raw) {
            return host;
        }
    }
    os_hostname().unwrap_or_else(|| "localhost".to_string())
}

/// Every host name/address this appliance answers as, derived from server-side
/// state only. Cheap enough for the auth paths that use it (one `gethostname`
/// call, one env read and one UDP bind), and never cached so a rename or a
/// settings change takes effect without a restart.
pub fn trusted_hosts() -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    let mut push = |host: String| {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    };

    push(canonical_host());
    if let Some(host) = os_hostname() {
        push(host.clone());
        push(format!("{host}.local"));
    }
    push("localhost".to_string());
    push("127.0.0.1".to_string());
    push("[::1]".to_string());
    if let Some(ip) = primary_lan_ip() {
        push(ip.to_string());
    }
    if let Ok(extra) = std::env::var(TRUSTED_HOSTS_ENV) {
        for entry in extra.split(',') {
            if let Some(host) = normalize_host(entry) {
                push(host);
            }
        }
    }
    hosts
}

/// Host to use in a redirect back to this appliance.
///
/// `request_host` is the request's `Host` header: it is honoured only when it
/// is one of [`trusted_hosts`], otherwise the canonical host replaces it.
pub fn resolve_redirect_host(request_host: Option<&str>) -> String {
    resolve_with(request_host, &trusted_hosts(), &canonical_host())
}

/// Pure core of [`resolve_redirect_host`], so both branches are unit-testable
/// without touching the process environment.
pub fn resolve_with(candidate: Option<&str>, trusted: &[String], canonical: &str) -> String {
    match candidate.and_then(normalize_host) {
        Some(host) if trusted.iter().any(|t| t == &host) => host,
        _ => canonical.to_string(),
    }
}

/// `host:port`, bracketing an IPv6 literal (`[::1]:8080`) so the authority
/// component is always well formed.
pub fn authority(host: &str, port: u16) -> String {
    if host.starts_with('[') || host.parse::<std::net::Ipv6Addr>().is_ok() {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        format!("[{bare}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Normalise and validate a host: lower-case, no port, IPv6 bracketed.
/// Returns `None` for anything that is not a bare host name or IP literal
/// (URLs, userinfo, paths, queries, lists, whitespace, over-long labels).
pub fn normalize_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 263 {
        return None;
    }
    // A host is a name or an IP literal. A URL, a credential, a path, a query
    // or a comma-separated list must never pass through to a `Location`.
    if raw.contains(|c: char| {
        c.is_whitespace() || matches!(c, '/' | '\\' | '?' | '#' | '@' | ',' | ';' | '"' | '\'')
    }) {
        return None;
    }

    let lower = raw.to_ascii_lowercase();

    // Bracketed IPv6, optionally with a port: `[::1]` / `[::1]:8080`.
    if let Some(rest) = lower.strip_prefix('[') {
        let (inner, tail) = rest.split_once(']')?;
        if !(tail.is_empty()
            || (tail.starts_with(':') && tail[1..].chars().all(|c| c.is_ascii_digit())))
        {
            return None;
        }
        let ip: std::net::Ipv6Addr = inner.parse().ok()?;
        return Some(format!("[{ip}]"));
    }

    // `host:port` — exactly one colon. Two or more is a bare IPv6 literal,
    // which a browser always sends bracketed in `Host`; refuse rather than
    // guess.
    let host = match lower.rsplit_once(':') {
        Some((name, port))
            if !name.contains(':')
                && !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit()) =>
        {
            name
        }
        Some(_) => return None,
        None => lower.as_str(),
    };

    // An absolute FQDN may carry one trailing dot.
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return None;
    }

    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return Some(ip.to_string());
    }

    if host.len() > 253 {
        return None;
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return None;
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
    }
    Some(host.to_string())
}

/// The OS host name, normalised when possible.
fn os_hostname() -> Option<String> {
    let raw = hostname::get().ok()?.to_string_lossy().to_ascii_lowercase();
    if raw.is_empty() {
        return None;
    }
    Some(normalize_host(&raw).unwrap_or(raw))
}

/// The address clients reach this appliance on, discovered without a
/// subprocess or a route table read: connecting a UDP socket to a TEST-NET-1
/// address only makes the kernel pick the outgoing source address (no packet
/// is sent).
fn primary_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if v4.is_loopback() || v4.is_unspecified() => None,
        ip => Some(ip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_normalize_host_accepts_names_and_ip_literals() {
        assert_eq!(normalize_host("HomeVault"), Some("homevault".into()));
        assert_eq!(
            normalize_host("homevault.lan"),
            Some("homevault.lan".into())
        );
        assert_eq!(
            normalize_host("homevault.lan:8080"),
            Some("homevault.lan".into())
        );
        assert_eq!(
            normalize_host("192.168.1.10:47001"),
            Some("192.168.1.10".into())
        );
        assert_eq!(normalize_host("[::1]:8080"), Some("[::1]".into()));
        assert_eq!(normalize_host("[fd00::1]"), Some("[fd00::1]".into()));
        // Case-insensitive, and one trailing dot (absolute FQDN) is allowed.
        assert_eq!(normalize_host("NAS.Local."), Some("nas.local".into()));
    }

    #[test]
    fn test_normalize_host_rejects_urls_paths_and_lists() {
        for bad in [
            "",
            "   ",
            "evil.example/path",
            "http://evil.example",
            "user@evil.example",
            "evil.example?x=1",
            "evil.example#frag",
            "a.example,b.example",
            "bad host",
            "-leading.example",
            "trailing-.example",
            "double..example",
            "evil.example:notaport",
            "::1",     // bare IPv6: browsers always bracket it in `Host`
            "[::1]:x", // bracket with a non-numeric port
            "[not-an-ip]",
        ] {
            assert_eq!(normalize_host(bad), None, "accepted: {bad:?}");
        }
    }

    #[test]
    fn test_normalize_host_rejects_over_long_values() {
        let long_label = "a".repeat(64);
        assert_eq!(normalize_host(&format!("{long_label}.example")), None);
        assert_eq!(normalize_host(&"a".repeat(300)), None);
    }

    /// The core of 6.3: a host the server does not already answer as never
    /// reaches the `Location` header.
    #[test]
    fn test_resolve_ignores_untrusted_host() {
        let trusted = trusted(&["homevault.lan", "192.168.1.10"]);
        assert_eq!(
            resolve_with(Some("evil.example"), &trusted, "homevault.lan"),
            "homevault.lan"
        );
        // A forged port, userinfo or path cannot smuggle a host through.
        assert_eq!(
            resolve_with(Some("evil.example:8080"), &trusted, "homevault.lan"),
            "homevault.lan"
        );
        assert_eq!(
            resolve_with(
                Some("homevault.lan@evil.example"),
                &trusted,
                "homevault.lan"
            ),
            "homevault.lan"
        );
        // No header at all falls back too.
        assert_eq!(
            resolve_with(None, &trusted, "homevault.lan"),
            "homevault.lan"
        );
    }

    #[test]
    fn test_resolve_keeps_a_trusted_host() {
        let trusted = trusted(&["homevault.lan", "192.168.1.10", "[fd00::1]"]);
        assert_eq!(
            resolve_with(Some("HOMEVAULT.LAN:8080"), &trusted, "homevault.lan"),
            "homevault.lan"
        );
        assert_eq!(
            resolve_with(Some("192.168.1.10"), &trusted, "homevault.lan"),
            "192.168.1.10"
        );
        assert_eq!(
            resolve_with(Some("[fd00::1]:47001"), &trusted, "homevault.lan"),
            "[fd00::1]"
        );
    }

    /// Loopback is always reachable as the appliance itself (local tooling,
    /// the setup wizard and the integration tests all use it).
    #[test]
    fn test_loopback_is_trusted() {
        let hosts = trusted_hosts();
        assert!(hosts.iter().any(|h| h == "127.0.0.1"));
        assert!(hosts.iter().any(|h| h == "localhost"));
        assert_eq!(
            resolve_with(Some("127.0.0.1:8080"), &hosts, "homevault.lan"),
            "127.0.0.1"
        );
    }

    #[test]
    fn test_authority_brackets_ipv6() {
        assert_eq!(authority("homevault.lan", 8080), "homevault.lan:8080");
        assert_eq!(authority("192.168.1.10", 47000), "192.168.1.10:47000");
        assert_eq!(authority("[::1]", 8080), "[::1]:8080");
        assert_eq!(authority("fd00::1", 8080), "[fd00::1]:8080");
    }

    #[test]
    fn test_canonical_host_is_always_usable() {
        // Whatever the host provides, the canonical host must be a value a
        // redirect can be built from.
        let canonical = canonical_host();
        assert!(normalize_host(&canonical).is_some(), "got: {canonical}");
    }
}
