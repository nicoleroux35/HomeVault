/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The generic reverse proxy (phase C2): user-defined host + path → upstream
 *   rules, served by one host-routed listener. The per-app proxy in
 *   `crate::app_proxy` stays what it is; this module is for LAN services that
 *   are not catalog apps.
 *
 *   Two safety rules shape it:
 *   - the upstream must be an intranet address (loopback/private/link-local/
 *     ULA) at save time *and* on every connection, so a rule cannot be used
 *     as an SSRF pivot;
 *   - an auth-gated rule never lets the browser send the API session to the
 *     upstream; a one-time ticket exchange mints a rule-scoped session on the
 *     rule's own host (see docs/analysis/c2-reverse-proxy-design.md).
 *
 * Dependencies:
 *   - hyper (>= 1), tokio (>= 1.52), sqlx (>= 0.9), uuid, once_cell
 *
 * Usage:
 *   handlers/reverse_proxy.rs for the CRUD API; `REVERSE_PROXY` is started
 *   from src/web/mod.rs. See docs/guides/reverse-proxy.md.
 * ============================================================================= */

//! User-defined host → upstream reverse-proxy rules and their listener.

pub mod proxy;
pub mod rules;
pub mod ticket;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::app_proxy::app_auth::APP_AUTH;
use crate::app_proxy::origin;

use ticket::TICKETS;

/// Environment override for the listener port; the default is the ordinary
/// HTTP port a LAN service is expected on.
pub const LISTEN_PORT_ENV: &str = "HOMEVAULT_REVERSE_PROXY_PORT";
pub const DEFAULT_LISTEN_PORT: u16 = 80;

/// A home LAN never needs more, and an unbounded list is an unbounded match
/// loop and an unbounded admin form.
pub const MAX_RULES: usize = 100;

/// Prefix limits: long enough for a real API path, short enough to read.
const MAX_PATH_LEN: usize = 256;

/// The reserved path the listener uses to redeem a ticket. It is refused as a
/// rule prefix so a user cannot shadow the auth endpoint.
pub const AUTH_CALLBACK_PATH: &str = "/__hv_proxy_auth";

/// One stored rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyRule {
    pub id: i64,
    pub host: String,
    /// `""` (whole host) or `/<prefix>` without a trailing slash.
    pub path: String,
    /// Canonical `host:port`, IPv6 bracketed.
    pub upstream: String,
    /// The upstream speaks HTTPS. The listener itself stays plain HTTP.
    pub tls: bool,
    /// Require a rule-scoped session before forwarding.
    pub auth_required: bool,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl ProxyRule {
    /// The key the app-proxy session store is scoped to. Distinct from every
    /// catalog app id, so a token can never be replayed across the two.
    pub fn ns_id(&self) -> String {
        ns_id(self.id)
    }
}

/// Namespace used by `APP_AUTH` for a rule.
pub fn ns_id(rule_id: i64) -> String {
    format!("proxy:{rule_id}")
}

/// The mutable half of a rule, as the API accepts it.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyRuleInput {
    pub host: String,
    #[serde(default)]
    pub path: String,
    pub upstream: String,
    #[serde(default)]
    pub tls: bool,
    #[serde(default = "default_true")]
    pub auth_required: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// A parsed upstream authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
}

impl Upstream {
    /// Parse `host:port` — an IPv4 literal, a DNS name, or a bracketed IPv6
    /// literal. A scheme, path, userinfo or unbracketed IPv6 is refused with a
    /// sentence that says which shape is valid.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("The upstream address is required.".to_string());
        }
        if raw.contains("://") {
            return Err(
                "The upstream must be host:port, without http:// or https:// — use the \
                 HTTPS switch for a TLS upstream."
                    .to_string(),
            );
        }
        if raw.len() > 300
            || raw.contains(|c: char| {
                c.is_whitespace() || matches!(c, '/' | '\\' | '?' | '#' | '@' | ',' | ';')
            })
        {
            return Err("The upstream must be a bare host:port, with no path.".to_string());
        }

        let (host_raw, port_raw) = if let Some(rest) = raw.strip_prefix('[') {
            let Some((inner, tail)) = rest.split_once(']') else {
                return Err("The IPv6 address is missing its closing bracket.".to_string());
            };
            let Some(port) = tail.strip_prefix(':') else {
                return Err("The upstream needs a port, for example [::1]:8080.".to_string());
            };
            (format!("[{inner}]"), port)
        } else {
            let Some((host, port)) = raw.rsplit_once(':') else {
                return Err("The upstream needs a port, for example 192.168.1.50:8123.".to_string());
            };
            if host.contains(':') {
                return Err("Bracket an IPv6 upstream, for example [fd00::10]:8080.".to_string());
            }
            (host.to_string(), port)
        };

        if port_raw.is_empty() || !port_raw.chars().all(|c| c.is_ascii_digit()) {
            return Err("The upstream port must be a number between 1 and 65535.".to_string());
        }
        let port: u16 = port_raw
            .parse()
            .map_err(|_| "The upstream port must be a number between 1 and 65535.".to_string())?;
        if port == 0 {
            return Err("The upstream port must be a number between 1 and 65535.".to_string());
        }

        let host = origin::normalize_host(&host_raw)
            .ok_or_else(|| "The upstream host is not a valid name or address.".to_string())?;

        Ok(Self { host, port })
    }

    /// Canonical `host:port` for storage and `Host` rewriting.
    pub fn authority(&self) -> String {
        origin::authority(&self.host, self.port)
    }
}

/// Validate the syntax of a rule. DNS resolution and loop checks happen in the
/// handler, where they can be async and can see the listener's own port.
pub fn validate_input(input: &ProxyRuleInput) -> Result<ProxyRule, String> {
    let host = validate_rule_host(&input.host)?;
    let path = validate_prefix(&input.path)?;
    let upstream = Upstream::parse(&input.upstream)?.authority();

    if path.eq_ignore_ascii_case(AUTH_CALLBACK_PATH) {
        return Err("That path is reserved by the proxy itself.".to_string());
    }

    Ok(ProxyRule {
        id: 0,
        host,
        path,
        upstream,
        tls: input.tls,
        auth_required: input.auth_required,
        enabled: input.enabled,
        created_at: String::new(),
        updated_at: String::new(),
    })
}

/// A rule host is a bare name or IP literal: no port, no scheme, no wildcard.
pub fn validate_rule_host(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("The host name is required.".to_string());
    }
    if raw.contains('*') {
        return Err("Wildcards are not supported; give the exact host name.".to_string());
    }
    if raw.contains("://") {
        return Err("The host name must not include http:// or https://.".to_string());
    }
    // A bracketed IPv6 literal is the only place a colon is legal.
    if raw.starts_with('[') {
        if !raw.ends_with(']') {
            return Err(
                "The host name must not include a port; the port belongs to the upstream."
                    .to_string(),
            );
        }
    } else if raw.contains(':') {
        return Err(
            "The host name must not include a port; the port belongs to the upstream.".to_string(),
        );
    }

    origin::normalize_host(raw)
        .ok_or_else(|| "The host name is not a valid DNS name, IPv4 or bracketed IPv6.".to_string())
}

/// Normalise a path prefix: `""` for the whole host, otherwise `/…` without a
/// trailing slash. Query strings, dot segments and control characters are
/// refused rather than escaped, because this value is matched, not rendered.
pub fn validate_prefix(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "/" {
        return Ok(String::new());
    }
    if !raw.starts_with('/') {
        return Err("A path prefix must start with /.".to_string());
    }
    if raw.len() > MAX_PATH_LEN {
        return Err(format!(
            "A path prefix is limited to {MAX_PATH_LEN} characters."
        ));
    }
    if raw.contains(|c: char| {
        c.is_control() || c.is_whitespace() || matches!(c, '?' | '#' | '\\' | '"' | '\'')
    }) {
        return Err(
            "A path prefix must not contain spaces, quotes, a query string or a fragment."
                .to_string(),
        );
    }
    if raw
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return Err("A path prefix must not contain . or .. segments.".to_string());
    }
    if raw.contains("//") {
        return Err("A path prefix must not contain empty segments.".to_string());
    }
    let trimmed = raw.trim_end_matches('/');
    Ok(trimmed.to_string())
}

/// Select the rule for a request: exact normalised host, then the longest
/// segment-boundary path prefix. Disabled rules never match.
pub fn select_rule<'a>(rules: &'a [ProxyRule], host: &str, path: &str) -> Option<&'a ProxyRule> {
    let host = origin::normalize_host(host)?;
    rules
        .iter()
        .filter(|rule| rule.enabled && rule.host == host)
        .filter(|rule| prefix_matches(&rule.path, path))
        .max_by_key(|rule| rule.path.len())
}

/// `""` matches everything; otherwise the prefix must end on a segment
/// boundary, so `/api` matches `/api` and `/api/x` but not `/apix`.
fn prefix_matches(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    path == prefix
        || (path.starts_with(prefix)
            && path
                .as_bytes()
                .get(prefix.len())
                .is_some_and(|b| *b == b'/'))
}

/// Whether an address is inside the intranet ranges this feature will reach.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let segments = v6.segments();
            v6.is_loopback() || (segments[0] & 0xfe00) == 0xfc00 || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Check every resolved address and return the ones that are safe to dial.
///
/// `local` is the appliance's own address list: with it, a rule that points at
/// the listener or at the dashboard is refused instead of recursing.
pub fn check_resolved_addresses(
    addresses: &[IpAddr],
    upstream_port: u16,
    listen_port: u16,
    main_port: u16,
    local: &[IpAddr],
) -> Result<Vec<IpAddr>, String> {
    if addresses.is_empty() {
        return Err("The upstream name did not resolve to any address.".to_string());
    }
    for address in addresses {
        if !is_private_ip(*address) {
            return Err(format!(
                "The upstream resolves to {address}, which is not a private or loopback address. \
                 A proxy rule may only reach the local network."
            ));
        }
        let is_local = address.is_loopback() || local.contains(address);
        if is_local && upstream_port == listen_port {
            return Err(
                "The upstream points back at this proxy's own port; that would loop forever."
                    .to_string(),
            );
        }
        if is_local && main_port != 0 && upstream_port == main_port {
            return Err(
                "The upstream points back at the HomeVault dashboard port; use the service's own \
                 address instead."
                    .to_string(),
            );
        }
    }
    Ok(addresses.to_vec())
}

/// Resolve an upstream, refusing public addresses and self-loops.
pub async fn resolve_upstream(
    upstream: &Upstream,
    listen_port: u16,
    main_port: u16,
    local: &[IpAddr],
) -> Result<Vec<SocketAddr>, String> {
    let addresses: Vec<IpAddr> =
        if let Ok(ip) = upstream.host.trim_matches(['[', ']']).parse::<IpAddr>() {
            vec![ip]
        } else {
            let resolved = tokio::net::lookup_host((upstream.host.as_str(), upstream.port))
                .await
                .map_err(|e| format!("The upstream name could not be resolved: {e}"))?;
            resolved.map(|addr| addr.ip()).collect()
        };
    let checked =
        check_resolved_addresses(&addresses, upstream.port, listen_port, main_port, local)?;
    Ok(checked
        .into_iter()
        .map(|ip| SocketAddr::new(ip, upstream.port))
        .collect())
}

/// The listener's own state, reported by `GET /api/proxy/rules`.
#[derive(Debug, Clone, Serialize)]
pub struct ListenerStatus {
    pub listening: bool,
    /// The port actually bound (the configured one when the bind failed).
    pub port: u16,
    pub error: Option<String>,
}

/// Process-wide manager. One listener, one rule snapshot.
pub struct ReverseProxyManager {
    listen_port: AtomicU16,
    main_port: AtomicU16,
    rules: RwLock<Vec<ProxyRule>>,
    status: RwLock<ListenerStatus>,
    started: AtomicBool,
    /// The appliance's own addresses, refreshed at start and on every reload.
    local_ips: RwLock<Vec<IpAddr>>,
}

impl ReverseProxyManager {
    pub fn new(listen_port: u16) -> Self {
        Self {
            listen_port: AtomicU16::new(listen_port),
            main_port: AtomicU16::new(0),
            rules: RwLock::new(Vec::new()),
            status: RwLock::new(ListenerStatus {
                listening: false,
                port: listen_port,
                error: None,
            }),
            started: AtomicBool::new(false),
            local_ips: RwLock::new(vec![IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)]),
        }
    }

    /// Called once at startup with the dashboard port, so a loop back into it
    /// can be refused.
    pub fn init(&self, main_port: u16) {
        self.main_port.store(main_port, Ordering::SeqCst);
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port.load(Ordering::SeqCst)
    }

    pub fn main_port(&self) -> u16 {
        self.main_port.load(Ordering::SeqCst)
    }

    pub async fn status(&self) -> ListenerStatus {
        self.status.read().await.clone()
    }

    /// The port a request is actually served on: the bound port once the
    /// listener is up, the configured one otherwise. Loop protection uses
    /// this, so a test (or an ephemeral bind) cannot miss itself.
    pub async fn effective_listen_port(&self) -> u16 {
        let status = self.status.read().await;
        if status.listening {
            status.port
        } else {
            self.listen_port()
        }
    }

    pub async fn rules_snapshot(&self) -> Vec<ProxyRule> {
        self.rules.read().await.clone()
    }

    pub async fn local_ips(&self) -> Vec<IpAddr> {
        self.local_ips.read().await.clone()
    }

    /// Load enabled rules from SQLite and replace the snapshot. Sessions and
    /// tickets for a rule that was edited, disabled or deleted are revoked, so
    /// an old grant can never reach a new upstream.
    pub async fn reload(&self, pool: &SqlitePool) -> Result<(), String> {
        let next = rules::list_enabled(pool)
            .await
            .map_err(|e| format!("the proxy rules could not be loaded: {e}"))?;
        let previous = self.rules.read().await.clone();
        for old in &previous {
            match next.iter().find(|rule| rule.id == old.id) {
                Some(current) if current == old => {}
                _ => revoke_rule_credentials(old.id),
            }
        }
        *self.rules.write().await = next;
        self.refresh_local_ips().await;
        Ok(())
    }

    async fn refresh_local_ips(&self) {
        let interfaces = tokio::task::spawn_blocking(crate::network::discover_lan_ipv4_addresses)
            .await
            .unwrap_or_default();
        let mut ips: Vec<IpAddr> = interfaces.into_iter().map(IpAddr::V4).collect();
        ips.push(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        ips.push(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
        *self.local_ips.write().await = ips;
    }

    /// Start the listener and load the rules. A bind failure is reported, not
    /// fatal: the CRUD API and the status still work.
    pub async fn start(self: &Arc<Self>, pool: Arc<SqlitePool>) -> Result<(), String> {
        if let Err(e) = self.reload(&pool).await {
            tracing::warn!("reverse proxy: {e}");
        }
        self.bind().await
    }

    /// Start the listener with an explicit snapshot, without SQLite. Used by
    /// the integration tests; production uses [`Self::start`].
    pub async fn start_with_rules(self: &Arc<Self>, rules: Vec<ProxyRule>) -> Result<(), String> {
        *self.rules.write().await = rules;
        self.bind().await
    }

    async fn bind(self: &Arc<Self>) -> Result<(), String> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let port = self.listen_port();
        match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => {
                let bound = listener
                    .local_addr()
                    .map(|addr| addr.port())
                    .unwrap_or(port);
                *self.status.write().await = ListenerStatus {
                    listening: true,
                    port: bound,
                    error: None,
                };
                let manager = self.clone();
                tokio::spawn(async move { manager.accept_loop(listener).await });
                Ok(())
            }
            Err(e) => {
                self.started.store(false, Ordering::SeqCst);
                let message = format!("the proxy listener could not bind port {port}: {e}");
                *self.status.write().await = ListenerStatus {
                    listening: false,
                    port,
                    error: Some(message.clone()),
                };
                Err(message)
            }
        }
    }

    async fn accept_loop(self: Arc<Self>, listener: tokio::net::TcpListener) {
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;

        tracing::info!(
            "reverse proxy listening on 0.0.0.0:{}",
            listener
                .local_addr()
                .map(|addr| addr.port())
                .unwrap_or_default()
        );

        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                continue;
            };
            let manager = self.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |req| {
                    let manager = manager.clone();
                    async move {
                        Ok::<_, std::convert::Infallible>(
                            proxy::handle(manager, req, peer.ip()).await,
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .with_upgrades()
                    .await;
            });
        }
    }
}

/// Drop every credential a rule may have minted.
pub fn revoke_rule_credentials(rule_id: i64) {
    APP_AUTH.revoke_app(&ns_id(rule_id));
    TICKETS.revoke_rule(rule_id);
}

/// Process-wide singleton, mirroring `APP_PROXY`.
pub static REVERSE_PROXY: Lazy<Arc<ReverseProxyManager>> =
    Lazy::new(|| Arc::new(ReverseProxyManager::new(configured_listen_port())));

/// The port from `HOMEVAULT_REVERSE_PROXY_PORT`, else 80. An invalid value is
/// logged and ignored rather than making the feature silently bind elsewhere.
pub fn configured_listen_port() -> u16 {
    match std::env::var(LISTEN_PORT_ENV) {
        Ok(raw) => match raw.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                tracing::warn!(
                    "{LISTEN_PORT_ENV}='{raw}' is not a port; using {DEFAULT_LISTEN_PORT}"
                );
                DEFAULT_LISTEN_PORT
            }
        },
        Err(_) => DEFAULT_LISTEN_PORT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn rule(id: i64, host: &str, path: &str) -> ProxyRule {
        ProxyRule {
            id,
            host: host.to_string(),
            path: path.to_string(),
            upstream: "127.0.0.1:9000".to_string(),
            tls: false,
            auth_required: true,
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn input(host: &str, path: &str, upstream: &str) -> ProxyRuleInput {
        ProxyRuleInput {
            host: host.to_string(),
            path: path.to_string(),
            upstream: upstream.to_string(),
            tls: false,
            auth_required: true,
            enabled: true,
        }
    }

    // ---- host validation ---------------------------------------------------

    #[test]
    fn test_host_normalises_case_and_trailing_dot() {
        assert_eq!(validate_rule_host(" HA.Home.LAN. ").unwrap(), "ha.home.lan");
    }

    #[test]
    fn test_host_refuses_a_port() {
        assert!(validate_rule_host("ha.home.lan:8123").is_err());
    }

    #[test]
    fn test_host_refuses_a_scheme_and_wildcard() {
        assert!(validate_rule_host("http://ha.home.lan").is_err());
        assert!(validate_rule_host("*.home.lan").is_err());
    }

    #[test]
    fn test_host_accepts_ip_literals() {
        assert_eq!(validate_rule_host("192.168.1.10").unwrap(), "192.168.1.10");
        assert_eq!(validate_rule_host("[fd00::10]").unwrap(), "[fd00::10]");
    }

    // ---- path validation ---------------------------------------------------

    #[test]
    fn test_prefix_empty_and_root_become_whole_host() {
        assert_eq!(validate_prefix("").unwrap(), "");
        assert_eq!(validate_prefix("/").unwrap(), "");
        assert_eq!(validate_prefix(" /api/ ").unwrap(), "/api");
    }

    #[test]
    fn test_prefix_refuses_injection_shapes() {
        for bad in [
            "api", "/a b", "/a?x=1", "/a#b", "/../etc", "/./x", "//x", "/a\\b",
        ] {
            assert!(validate_prefix(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_prefix_is_bounded() {
        assert!(validate_prefix(&format!("/{}", "a".repeat(300))).is_err());
    }

    #[test]
    fn test_rule_validation_refuses_the_auth_callback_path() {
        assert!(validate_input(&input("ha.home.lan", AUTH_CALLBACK_PATH, "127.0.0.1:1")).is_err());
    }

    // ---- upstream parsing --------------------------------------------------

    #[test]
    fn test_upstream_parses_ipv4_and_name() {
        assert_eq!(
            Upstream::parse("192.168.1.50:8123").unwrap(),
            Upstream {
                host: "192.168.1.50".to_string(),
                port: 8123
            }
        );
        assert_eq!(
            Upstream::parse("nas.lan:80").unwrap().authority(),
            "nas.lan:80"
        );
    }

    #[test]
    fn test_upstream_brackets_ipv6() {
        assert_eq!(
            Upstream::parse("[fd00::10]:8080").unwrap().authority(),
            "[fd00::10]:8080"
        );
        assert!(Upstream::parse("fd00::10:8080").is_err());
    }

    #[test]
    fn test_upstream_refuses_scheme_path_and_bad_port() {
        for bad in [
            "http://192.168.1.5:80",
            "192.168.1.5:80/api",
            "192.168.1.5",
            "192.168.1.5:0",
            "192.168.1.5:99999",
            "192.168.1.5:abc",
        ] {
            assert!(Upstream::parse(bad).is_err(), "{bad}");
        }
    }

    // ---- selection ---------------------------------------------------------

    #[test]
    fn test_select_rule_prefers_the_longest_path() {
        let rules = vec![
            rule(1, "ha.home.lan", ""),
            rule(2, "ha.home.lan", "/api"),
            rule(3, "ha.home.lan", "/api/admin"),
        ];
        assert_eq!(
            select_rule(&rules, "HA.home.lan:80", "/api/admin/x")
                .unwrap()
                .id,
            3
        );
        assert_eq!(select_rule(&rules, "ha.home.lan", "/api/x").unwrap().id, 2);
        assert_eq!(select_rule(&rules, "ha.home.lan", "/").unwrap().id, 1);
    }

    #[test]
    fn test_select_rule_respects_segment_boundaries() {
        let rules = vec![rule(1, "ha.home.lan", "/api")];
        assert!(select_rule(&rules, "ha.home.lan", "/apix").is_none());
    }

    #[test]
    fn test_select_rule_ignores_disabled_and_other_hosts() {
        let mut disabled = rule(1, "ha.home.lan", "");
        disabled.enabled = false;
        let rules = vec![disabled, rule(2, "other.lan", "")];
        assert!(select_rule(&rules, "ha.home.lan", "/").is_none());
        assert!(select_rule(&rules, "other.lan", "/").is_some());
        assert!(select_rule(&rules, "unknown.lan", "/").is_none());
    }

    // ---- address policy ----------------------------------------------------

    #[test]
    fn test_private_ranges_are_accepted() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.50",
            "169.254.1.2",
            "::1",
            "fd00::10",
            "fe80::1",
            "::ffff:192.168.1.9",
        ] {
            assert!(is_private_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn test_public_and_unspecified_ranges_are_refused() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "0.0.0.0",
            "2001:4860:4860::8888",
            "::",
        ] {
            assert!(!is_private_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn test_public_address_is_refused_with_its_name() {
        let err = check_resolved_addresses(&["8.8.8.8".parse().unwrap()], 8080, 80, 8080, &[])
            .unwrap_err();
        assert!(err.contains("8.8.8.8"), "{err}");
    }

    #[test]
    fn test_loop_back_into_the_listener_is_refused() {
        let local: Vec<IpAddr> = vec!["192.168.1.10".parse().unwrap()];
        let err =
            check_resolved_addresses(&["192.168.1.10".parse().unwrap()], 80, 80, 8080, &local)
                .unwrap_err();
        assert!(err.contains("loop"), "{err}");
    }

    #[test]
    fn test_loop_back_into_the_dashboard_is_refused() {
        let local: Vec<IpAddr> = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
        let err =
            check_resolved_addresses(&[IpAddr::V4(Ipv4Addr::LOCALHOST)], 8080, 80, 8080, &local)
                .unwrap_err();
        assert!(err.contains("dashboard"), "{err}");
    }

    #[test]
    fn test_a_private_service_is_accepted() {
        let checked = check_resolved_addresses(
            &["192.168.1.50".parse().unwrap()],
            8123,
            80,
            8080,
            &["192.168.1.10".parse().unwrap()],
        )
        .unwrap();
        assert_eq!(checked.len(), 1);
    }

    #[test]
    fn test_mixed_resolution_is_refused() {
        let err = check_resolved_addresses(
            &["192.168.1.50".parse().unwrap(), "8.8.8.8".parse().unwrap()],
            8123,
            80,
            8080,
            &[],
        )
        .unwrap_err();
        assert!(err.contains("8.8.8.8"), "{err}");
    }
}
