/* =============================================================================
 * File          : shortcuts.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Server-side page-metadata fetcher for pinned desktop shortcuts (title,
 *   description, favicon). The endpoint fetches a user-supplied URL and is
 *   therefore a deliberate SSRF sink: scheme allow-list, every resolved
 *   address must be public, redirects resolved manually and re-checked,
 *   response capped in size and time (details in the module doc — those
 *   guards are the security boundary). Session-auth.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Metadata-fetch route in src/web/mod.rs; called by the desktop shortcut
 *   creation flow.
 * =============================================================================
 */

//! Web shortcuts — fetch a page's metadata (title, description, favicon) so a
//! pinned desktop shortcut can render a real icon instead of a bare URL.
//!
//! This endpoint makes the server perform an outbound HTTP request to a
//! user-supplied URL, which is an SSRF sink. The guards below are the
//! boundary, not a nicety:
//!
//!   1. Scheme allow-list — `http` / `https` only (no `file:`, `gopher:`, …).
//!   2. Host resolution + **every** resolved address must be public. A host
//!      that resolves to a loopback / private / link-local / unique-local /
//!      multicast / unspecified address is rejected. This blocks the classic
//!      `http://169.254.169.254/` cloud-metadata and `http://127.0.0.1:8080/`
//!      admin-panel probes.
//!   3. Redirects are **not** followed automatically: a public URL must not be
//!      able to 302 the server into a private one. Up to `MAX_REDIRECTS` hops
//!      are resolved manually and **every** hop is re-checked against rule 2
//!      before it is dialled (a short-link or CDN chain used to stop after one
//!      hop, and the second redirect's empty body was parsed as the page).
//!   4. A 512 KB response cap and a 10 s timeout bound the work. The cap is
//!      enforced while the body is read (`read_capped_stream`), not after
//!      `Response::bytes()` has materialised the whole thing, and the 10 s
//!      budget covers the **whole chain**, not each hop.
//!   5. The client resolves through `PublicOnlyResolver`, so the address that
//!      is actually dialled is one that passed `is_public_ip` in the very same
//!      resolution — a hostile DNS answer cannot change between the pre-check
//!      and the connect (DNS rebinding).
//!
//! The parse itself is deliberately tiny (scanner over `<head>`): title, meta
//! description, and a favicon `<link>` href. No HTML parser dependency.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json as AxumJson};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// Maximum bytes of the page we read before parsing. The reader stops at the
/// cap while the body is streaming in (`read_capped_stream`).
const MAX_BODY_BYTES: usize = 512 * 1024;
/// Wall-clock budget for the whole fetch, redirect chain included. A per-hop
/// deadline would let a five-hop chain hold the request for a minute.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Redirects actually followed. A chain longer than this is refused by name
/// instead of being silently truncated to its last redirect body.
const MAX_REDIRECTS: usize = 5;
/// The statuses that carry a real "go elsewhere". 300 (no single target),
/// 304 (not modified) and 305/306 (deprecated) are **not** followed: they used
/// to be treated as a redirect and answered "Redirect without a Location".
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

#[derive(Debug, Deserialize)]
pub struct FetchMetadataRequest {
    pub url: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct PageMetadata {
    /// The final URL after an optional single redirect.
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Absolute favicon URL, resolved against the final URL.
    pub icon_url: Option<String>,
    /// The origin (`scheme://host[:port]`), handy for the shortcut tile.
    pub site_name: Option<String>,
}

/// `true` when `ip` is safe to connect to from the server (a public address).
///
/// Everything that could reach the host itself, the LAN, or a cloud metadata
/// service is rejected. IPv6 addresses are unwrapped from `::ffff:` v4-mapped
/// form first, so `::ffff:127.0.0.1` cannot slip through as "public".
pub fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast())
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(&IpAddr::V4(v4));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique-local (fc00::/7) and link-local (fe80::/10).
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// Validate a URL and return it parsed, or an error string suitable for a 400
/// response. Pure (no DNS) so it is unit-testable. The returned URL is
/// normalised by the `url` crate (`example.com` → `https://example.com/`), so
/// the redirect join and the reported `url` field work from one canonical
/// form instead of re-formatting the raw input.
pub fn parse_url(raw: &str) -> Result<url::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("url is required".to_string());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&with_scheme).map_err(|_| "invalid URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http and https URLs are allowed".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("URL has no host".to_string());
    }
    Ok(parsed)
}

/// Validate a URL and return its parts (scheme, host, port).
pub fn validate_url(raw: &str) -> Result<(String, String, u16), String> {
    let parsed = parse_url(raw)?;
    let host = parsed.host_str().unwrap_or_default().to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    Ok((parsed.scheme().to_string(), host, port))
}

/// Resolve `host` and require every address to be public. Returns the first
/// public address to connect to.
///
/// Two layers guard the connection: this pre-check produces the precise error
/// message (and covers literal IPs, which hyper dials without asking a
/// resolver), while [`PublicOnlyResolver`] re-validates the answer reqwest
/// actually dials. Neither layer alone is enough — see the module doc.
async fn resolve_public(host: &str, port: u16) -> Result<IpAddr, String> {
    // A literal IP is checked directly (no DNS). `Url::host_str()` keeps the
    // brackets around an IPv6 literal (`[::1]`), so strip them first or the
    // parse fails and the address escapes the private-IP check.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return if is_public_ip(&ip) {
            Ok(ip)
        } else {
            Err("Refusing to fetch a private or local address".to_string())
        };
    }
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| "Could not resolve host".to_string())?;
    let all: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
    if all.is_empty() {
        return Err("Could not resolve host".to_string());
    }
    if all.iter().any(|ip| !is_public_ip(ip)) {
        return Err("Refusing to fetch a host that resolves to a private address".to_string());
    }
    Ok(all[0])
}

/// Connect-time resolver: refuses any answer containing a non-public address.
///
/// reqwest resolves when it dials, so validating before the request (rule 2)
/// leaves a window in which a hostile DNS answer can change. This resolver
/// closes it: the addresses it hands to the connector are the ones it just
/// validated, and there is no second resolution.
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = resolve_public_addrs(&host).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Every address of `host`, each one public. Ports stay `0`: reqwest replaces
/// them with the URL's own port (or the scheme's default).
async fn resolve_public_addrs(host: &str) -> Result<Vec<SocketAddr>, std::io::Error> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
    if addrs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{host} has no address"),
        ));
    }
    if addrs.iter().any(|a| !is_public_ip(&a.ip())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{host} resolves to a private or local address"),
        ));
    }
    Ok(addrs)
}

/// The one client of the module: reqwest's connection pool is what makes a
/// second metadata fetch cheap, and the old shape built a fresh client per
/// request (and so a fresh pool, a fresh TLS session and a fresh DNS cache).
static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(build_client);

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        // Redirects are handled manually so each hop is re-validated.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("HomeVault/Shortcuts")
        .dns_resolver(PublicOnlyResolver)
        .build()
        .map_err(|e| format!("Could not build the HTTP client: {e}"))
}

fn shared_client() -> Result<&'static reqwest::Client, String> {
    CLIENT.as_ref().map_err(|e| e.clone())
}

/// Extract `<title>`, `<meta name="description">` / `og:description`, and the
/// favicon href from an HTML document. Pure, so unit-testable.
///
/// Hand-rolled scanning rather than a regex/HTML parser: `regex` is only a
/// dev-dependency here, and pulling an HTML parser in for three fields would
/// be a poor trade. The scanner is case-insensitive and tolerant of attribute
/// order, which is all the tags below need.
pub fn parse_html(html: &str, base_url: &str) -> PageMetadata {
    let title = extract_tag_text(html, "title").map(|s| clean_text(&s));

    let description =
        extract_meta_content(html, &["description", "og:description"]).map(|s| clean_text(&s));

    let icon_raw = extract_icon_href(html);

    let icon_url = icon_raw.and_then(|href| {
        url::Url::parse(base_url)
            .ok()
            .and_then(|base| base.join(href.trim()).ok())
            .map(|u| u.to_string())
    });

    let site_name = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()));

    PageMetadata {
        url: base_url.to_string(),
        title: title.filter(|s| !s.is_empty()),
        description: description.filter(|s| !s.is_empty()),
        icon_url,
        site_name,
    }
}

/// The text content of the first `<tag …>…</tag>` (case-insensitive).
fn extract_tag_text(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let start = lower.find(&open)?;
    let after_open = lower[start..].find('>')? + start + 1;
    let close = format!("</{tag}");
    let end = lower[after_open..].find(&close)? + after_open;
    Some(html[after_open..end].to_string())
}

/// The `content` attribute of the first `<meta>` whose `name`/`property` is
/// one of `names`, in either attribute order.
fn extract_meta_content(html: &str, names: &[&str]) -> Option<String> {
    for tag in find_tags(html, "meta") {
        let Some(key) = attr_value(&tag, "name").or_else(|| attr_value(&tag, "property")) else {
            continue;
        };
        if names.iter().any(|n| n.eq_ignore_ascii_case(&key)) {
            if let Some(content) = attr_value(&tag, "content") {
                return Some(content);
            }
        }
    }
    None
}

/// The `href` of the first `<link>` whose `rel` contains `icon`, in either
/// attribute order.
fn extract_icon_href(html: &str) -> Option<String> {
    for tag in find_tags(html, "link") {
        let rel = attr_value(&tag, "rel")?;
        if rel.to_ascii_lowercase().contains("icon") {
            if let Some(href) = attr_value(&tag, "href") {
                return Some(href);
            }
        }
    }
    None
}

/// Every `<tag …>` opening tag (without the closing `>`), case-insensitive.
fn find_tags(html: &str, tag: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("<{tag}");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(&needle) {
        let start = from + rel;
        // Require a delimiter after the tag name so `<link` does not match
        // `<linkage`.
        let after = start + needle.len();
        let next = lower[after..].chars().next();
        let is_tag = matches!(next, Some(c) if c.is_whitespace() || c == '>' || c == '/');
        if is_tag {
            if let Some(end_rel) = lower[after..].find('>') {
                out.push(html[start..after + end_rel].to_string());
                from = after + end_rel + 1;
                continue;
            }
        }
        from = after;
    }
    out
}

/// The value of `name` in a tag's attributes, quoted with `"` or `'`.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{name}=");
    let idx = lower.find(&needle)? + needle.len();
    let rest = &tag[idx..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let after = &rest[1..];
    let end = after.find(quote)?;
    Some(after[..end].to_string())
}

/// Collapse whitespace and decode the handful of entities that show up in
/// titles. Anything longer than 300 chars is truncated.
fn clean_text(s: &str) -> String {
    let decoded = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 300 {
        collapsed.chars().take(300).collect()
    } else {
        collapsed
    }
}

/// `POST /api/shortcuts/fetch-metadata` — fetch a page's title/description/favicon.
pub async fn handle_fetch_metadata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<FetchMetadataRequest>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let start = match parse_url(&payload.url) {
        Ok(url) => url,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let client = match shared_client() {
        Ok(client) => client,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let (final_url, body) = match fetch_page(client, &start).await {
        Ok(v) => v,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let meta = parse_html(&body, &final_url);
    ResponseHelper::json_success(meta)
}

/// `true` for the statuses that name another URL and must be followed.
fn is_followable_redirect(status: u16) -> bool {
    REDIRECT_STATUSES.contains(&status)
}

/// The absolute target of a redirect, or `None` when the response ends the
/// chain. Pure (no I/O), so the chain rules are unit-testable: which statuses
/// are followed, the relative / scheme-relative / cross-host join, and the
/// per-hop scheme allow-list (a `Location: file:///etc/passwd` must be refused
/// exactly like a top-level `file:` URL).
fn redirect_target(
    current: &url::Url,
    status: u16,
    location: Option<&str>,
) -> Result<Option<url::Url>, String> {
    if !is_followable_redirect(status) {
        return Ok(None);
    }
    let location = location
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| "Redirect without a Location header".to_string())?;
    let target = current
        .join(location)
        .map_err(|_| "Invalid redirect target".to_string())?;
    if !matches!(target.scheme(), "http" | "https") {
        return Err("Only http and https URLs are allowed".to_string());
    }
    if target.host_str().is_none() {
        return Err("URL has no host".to_string());
    }
    Ok(Some(target))
}

/// The host and port of a hop's URL. Literal IPv6 hosts keep their brackets
/// here (`resolve_public` strips them).
fn hop_destination(url: &url::Url) -> (String, u16) {
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    (host, port)
}

fn too_many_redirects() -> String {
    format!("Too many redirects (at most {MAX_REDIRECTS} are followed)")
}

/// Fetch `start`, following up to [`MAX_REDIRECTS`] redirects. Every hop is
/// re-validated (scheme, host, resolved addresses) **before** it is dialled,
/// and the whole chain shares one deadline.
async fn fetch_page(
    client: &reqwest::Client,
    start: &url::Url,
) -> Result<(String, String), String> {
    let deadline = tokio::time::Instant::now() + FETCH_TIMEOUT;
    let mut current = start.clone();
    let mut hops = 0usize;

    loop {
        let (host, port) = hop_destination(&current);
        resolve_public(&host, port).await?;

        let resp = send_get(client, &current, deadline).await?;
        let status = resp.status().as_u16();
        // Owned, so the header borrow ends before `resp` is consumed below.
        let location: Option<String> = match resp.headers().get(reqwest::header::LOCATION) {
            None => None,
            Some(value) => Some(
                value
                    .to_str()
                    .map_err(|_| "Invalid redirect target".to_string())?
                    .to_string(),
            ),
        };

        match redirect_target(&current, status, location.as_deref())? {
            None => {
                let body = read_capped_at(resp, deadline).await?;
                return Ok((current.to_string(), body));
            }
            Some(next) => {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(too_many_redirects());
                }
                current = next;
            }
        }
    }
}

/// One GET inside the chain's deadline.
async fn send_get(
    client: &reqwest::Client,
    url: &url::Url,
    deadline: tokio::time::Instant,
) -> Result<reqwest::Response, String> {
    match tokio::time::timeout_at(deadline, client.get(url.clone()).send()).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(_)) => Err("Could not reach the site".to_string()),
        Err(_) => Err("The site took too long to respond".to_string()),
    }
}

/// Read the body, still inside the chain's deadline: a slow-drip body is a
/// timeout, not an excuse to hold the request open.
async fn read_capped_at(
    resp: reqwest::Response,
    deadline: tokio::time::Instant,
) -> Result<String, String> {
    match tokio::time::timeout_at(deadline, read_capped(resp)).await {
        Ok(result) => result,
        Err(_) => Err("The site took too long to send its page".to_string()),
    }
}

/// Read at most `MAX_BODY_BYTES` of the response body as a lossy string.
async fn read_capped(resp: reqwest::Response) -> Result<String, String> {
    read_capped_stream(resp.bytes_stream()).await
}

/// Accumulate a body stream, stopping at `MAX_BODY_BYTES`.
///
/// The old implementation awaited `Response::bytes()`, which materialises the
/// **whole** body before the truncation ran — the 512 KB cap described in the
/// docs bounded the parser, not the server's memory (a multi-GB response was
/// fully resident first). This stops pulling as soon as the cap is reached and
/// drops the rest of the stream.
async fn read_capped_stream<S, E>(stream: S) -> Result<String, String>
where
    S: futures_util::Stream<Item = Result<axum::body::Bytes, E>>,
{
    futures_util::pin_mut!(stream);
    let mut body: Vec<u8> = Vec::with_capacity(64 * 1024);
    while body.len() < MAX_BODY_BYTES {
        match stream.next().await {
            Some(Ok(chunk)) => {
                let remaining = MAX_BODY_BYTES - body.len();
                if chunk.len() >= remaining {
                    body.extend_from_slice(&chunk[..remaining]);
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Some(Err(_)) => return Err("Could not read the response".to_string()),
            None => break,
        }
    }
    Ok(String::from_utf8_lossy(&body).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_public_ip_rejects_private_and_local() {
        for s in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            let ip: IpAddr = s.parse().unwrap();
            assert!(!is_public_ip(&ip), "{s} must be rejected");
        }
    }

    #[test]
    fn test_is_public_ip_accepts_public() {
        for s in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            let ip: IpAddr = s.parse().unwrap();
            assert!(is_public_ip(&ip), "{s} must be accepted");
        }
    }

    #[test]
    fn test_validate_url_scheme_and_host() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://example.com").is_ok());
        // Bare host gets an https scheme.
        assert_eq!(validate_url("example.com").unwrap().0, "https");
        // Non-http schemes are refused.
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("ftp://example.com").is_err());
        assert!(validate_url("gopher://example.com").is_err());
        assert!(validate_url("").is_err());
    }

    #[test]
    fn test_parse_url_returns_a_normalised_absolute_url() {
        // One canonical form for the redirect join and for the `url` field the
        // screen stores, instead of re-formatting the raw input (ROADMAP 27.2).
        assert_eq!(
            parse_url("example.com").unwrap().as_str(),
            "https://example.com/"
        );
        assert_eq!(
            parse_url("  HTTPS://Example.COM/a/b  ").unwrap().as_str(),
            "https://example.com/a/b"
        );
        assert_eq!(
            parse_url("http://10.0.0.1/x")
                .unwrap()
                .port_or_known_default(),
            Some(80)
        );
        // A missing host is refused rather than reaching the resolver.
        assert!(parse_url("https://").is_err());
        assert!(parse_url("   ").is_err());
    }

    #[test]
    fn test_only_the_redirect_statuses_that_name_a_target_are_followed() {
        for status in REDIRECT_STATUSES {
            assert!(is_followable_redirect(status), "{status} must be followed");
        }
        // 300 has no single target, 304 is "not modified", 305/306 are
        // deprecated — all used to be read as a redirect and answered
        // "Redirect without a Location header".
        for status in [200u16, 204, 300, 304, 305, 306, 400, 404, 500] {
            assert!(
                !is_followable_redirect(status),
                "{status} must end the chain"
            );
        }
    }

    #[test]
    fn test_redirect_target_handles_relative_protocol_relative_and_cross_host() {
        let base = url::Url::parse("https://example.com/dir/page").unwrap();

        let root = redirect_target(&base, 302, Some("/next")).unwrap().unwrap();
        assert_eq!(root.as_str(), "https://example.com/next");

        let sibling = redirect_target(&base, 301, Some("child")).unwrap().unwrap();
        assert_eq!(sibling.as_str(), "https://example.com/dir/child");

        let protocol_relative = redirect_target(&base, 307, Some("//cdn.example.org/x"))
            .unwrap()
            .unwrap();
        assert_eq!(protocol_relative.as_str(), "https://cdn.example.org/x");

        let cross_host = redirect_target(&base, 308, Some("http://other.example/y"))
            .unwrap()
            .unwrap();
        assert_eq!(cross_host.as_str(), "http://other.example/y");

        // A non-redirect status is the end of the chain, Location or not.
        assert!(redirect_target(&base, 200, Some("/ignored"))
            .unwrap()
            .is_none());
        assert!(redirect_target(&base, 304, None).unwrap().is_none());
    }

    #[test]
    fn test_a_whole_redirect_chain_is_walked_hop_by_hop() {
        // 27.2: the old code followed exactly ONE hop, so a second redirect's
        // empty body was parsed as "the page" and the metadata came back empty
        // on every short-link / CDN chain. Each hop is re-validated from the
        // absolute URL the previous hop produced.
        let mut current = url::Url::parse("https://short.example/abc").unwrap();
        let hops = [
            (302u16, "https://cdn.example/abc"),
            (301, "/real/article"),
            (307, "https://www.example/article?id=1"),
        ];
        let mut seen = Vec::new();
        for (status, location) in hops {
            current = redirect_target(&current, status, Some(location))
                .unwrap()
                .expect("a redirect must name its next hop");
            seen.push(current.to_string());
        }
        assert_eq!(
            seen,
            vec![
                "https://cdn.example/abc".to_string(),
                "https://cdn.example/real/article".to_string(),
                "https://www.example/article?id=1".to_string(),
            ]
        );
    }

    #[test]
    fn test_redirect_target_refuses_a_bad_scheme_or_a_missing_location() {
        let base = url::Url::parse("https://example.com/").unwrap();

        // The per-hop allow-list is the same rule as the top-level one: a
        // public URL must not be able to bounce the server onto `file:`.
        for location in ["file:///etc/passwd", "javascript:alert(1)", "gopher://x/"] {
            let err = redirect_target(&base, 302, Some(location)).unwrap_err();
            assert!(err.contains("http"), "{location}: {err}");
        }

        for missing in [None, Some(""), Some("   ")] {
            let err = redirect_target(&base, 302, missing).unwrap_err();
            assert!(err.contains("Location"), "{missing:?}: {err}");
        }
    }

    #[test]
    fn test_a_chain_longer_than_the_cap_is_refused_by_name() {
        assert!(
            MAX_REDIRECTS >= 3,
            "a real CDN/short-link chain needs a few hops"
        );
        let message = too_many_redirects();
        assert!(message.contains(&MAX_REDIRECTS.to_string()), "{message}");
        assert!(message.to_lowercase().contains("redirect"), "{message}");
    }

    #[test]
    fn test_hop_destination_keeps_the_url_port_and_brackets_ipv6() {
        let url = url::Url::parse("http://example.com").unwrap();
        assert_eq!(hop_destination(&url), ("example.com".to_string(), 80));
        let url = url::Url::parse("https://example.com:8443/x").unwrap();
        assert_eq!(hop_destination(&url), ("example.com".to_string(), 8443));
        let url = url::Url::parse("http://[::1]:8080/x").unwrap();
        assert_eq!(hop_destination(&url), ("[::1]".to_string(), 8080));
    }

    #[test]
    fn test_parse_html_extracts_title_description_and_icon() {
        let html = r#"<html><head>
            <title>  My   Site &amp; Co </title>
            <meta name="description" content="A nice page">
            <link rel="icon" href="/favicon.ico">
            </head><body></body></html>"#;
        let meta = parse_html(html, "https://example.com/some/page");
        assert_eq!(meta.title.as_deref(), Some("My Site & Co"));
        assert_eq!(meta.description.as_deref(), Some("A nice page"));
        assert_eq!(
            meta.icon_url.as_deref(),
            Some("https://example.com/favicon.ico")
        );
        assert_eq!(meta.site_name.as_deref(), Some("example.com"));
    }

    #[test]
    fn test_parse_html_handles_missing_head_fields() {
        let meta = parse_html("<html><body>hi</body></html>", "https://example.com/");
        assert_eq!(meta.title, None);
        assert_eq!(meta.description, None);
        assert_eq!(meta.icon_url, None);
        assert_eq!(meta.site_name.as_deref(), Some("example.com"));
    }

    #[test]
    fn test_parse_html_supports_og_description_and_reversed_attrs() {
        let html = r#"<head><meta content="og desc" property="og:description">
            <link href="https://cdn.example.com/i.png" rel="shortcut icon"></head>"#;
        let meta = parse_html(html, "https://example.com/");
        assert_eq!(meta.description.as_deref(), Some("og desc"));
        assert_eq!(
            meta.icon_url.as_deref(),
            Some("https://cdn.example.com/i.png")
        );
    }

    #[test]
    fn test_clean_text_truncates_long_values() {
        let long = "a".repeat(500);
        assert_eq!(clean_text(&long).chars().count(), 300);
    }

    #[tokio::test]
    async fn test_public_only_resolver_refuses_private_literals() {
        // The connect-time half of the SSRF guard (27.2): whatever reqwest is
        // about to dial has just been validated in the same resolution, so a
        // rebinding answer cannot reach a private address.
        for host in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.10",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            let err = resolve_public_addrs(host)
                .await
                .expect_err(&format!("{host} must be refused at connect time"));
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{host}");
        }

        let public = resolve_public_addrs("1.1.1.1")
            .await
            .expect("a public literal is allowed");
        assert_eq!(public.len(), 1);
        assert!(is_public_ip(&public[0].ip()));
        // Port 0 is what reqwest replaces with the URL's own port.
        assert_eq!(public[0].port(), 0);
    }

    #[tokio::test]
    async fn test_read_capped_stops_reading_at_the_cap() {
        use futures_util::stream::poll_fn;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // 27.1: the body used to be fully buffered and only then truncated.
        // A body whose first chunk already fills the cap must not be polled a
        // second time — that is what keeps a multi-GB response off the heap.
        let extra_polls = Arc::new(AtomicUsize::new(0));
        let counter = extra_polls.clone();
        let mut first = Some(axum::body::Bytes::from(vec![b'a'; MAX_BODY_BYTES]));
        let stream = poll_fn(move |_cx| {
            if let Some(chunk) = first.take() {
                return std::task::Poll::Ready(Some(Ok::<_, std::io::Error>(chunk)));
            }
            counter.fetch_add(1, Ordering::SeqCst);
            std::task::Poll::Ready(Some(Ok(axum::body::Bytes::from(vec![b'b'; 1024]))))
        });

        let body = read_capped_stream(stream).await.unwrap();
        assert_eq!(body.len(), MAX_BODY_BYTES);
        assert_eq!(
            extra_polls.load(Ordering::SeqCst),
            0,
            "the reader must stop at the cap instead of draining the rest of the body"
        );
    }

    #[tokio::test]
    async fn test_read_capped_truncates_a_chunk_that_crosses_the_cap() {
        use futures_util::stream;

        let head = vec![b'a'; MAX_BODY_BYTES - 10];
        let tail = vec![b'b'; 4096];
        let chunks = vec![
            Ok::<_, std::io::Error>(axum::body::Bytes::from(head)),
            Ok::<_, std::io::Error>(axum::body::Bytes::from(tail)),
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"never read")),
        ];
        let body = read_capped_stream(stream::iter(chunks)).await.unwrap();
        assert_eq!(body.len(), MAX_BODY_BYTES);
        assert!(body.starts_with('a'));
        // The 10 bytes taken from the second chunk are its prefix, not garbage.
        assert!(body.ends_with(&"b".repeat(10)));
        assert!(!body.contains("never read"));
    }

    #[tokio::test]
    async fn test_read_capped_keeps_a_small_body_whole() {
        use futures_util::stream;

        let chunks = vec![
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"<title>Hello")),
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"</title>")),
        ];
        let body = read_capped_stream(stream::iter(chunks)).await.unwrap();
        assert_eq!(body, "<title>Hello</title>");
    }

    #[tokio::test]
    async fn test_read_capped_reports_a_broken_stream() {
        use futures_util::stream;

        let chunks = vec![
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"<title>partial")),
            Err(std::io::Error::other("connection reset")),
        ];
        let err = read_capped_stream(stream::iter(chunks)).await.unwrap_err();
        assert!(err.contains("read"), "{err}");
    }
}
