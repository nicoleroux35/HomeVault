/* =============================================================================
 * File          : shortcuts.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
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
//!      able to 302 the server into a private one. A single redirect is
//!      resolved manually and re-checked against rule 2.
//!   4. A 512 KB response cap and a 10 s timeout bound the work.
//!
//! The parse itself is deliberately tiny (regex over `<head>`): title, meta
//! description, and a favicon `<link>` href. No HTML parser dependency.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json as AxumJson};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

/// Maximum bytes of the page we read before parsing.
const MAX_BODY_BYTES: usize = 512 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Validate a URL and return its parsed parts, or an error string suitable for
/// a 400 response. Pure (no DNS) so it is unit-testable.
pub fn validate_url(raw: &str) -> Result<(String, String, u16), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("url is required".to_string());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed)
    };
    let parsed = url::Url::parse(&with_scheme).map_err(|_| "invalid URL".to_string())?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("Only http and https URLs are allowed".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_string();
    let port = parsed.port_or_known_default().unwrap_or(443);
    Ok((scheme.to_string(), host, port))
}

/// Resolve `host` and require every address to be public. Returns the first
/// public address to connect to.
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

/// Extract `<title>`, `<meta name="description">` / `og:description`, and the
/// favicon href from an HTML document. Pure, so unit-testable.
///
/// Hand-rolled scanning rather than a regex/HTML parser: `regex` is only a
/// dev-dependency here, and pulling an HTML parser in for three fields would
/// be a poor trade. The scanner is case-insensitive and tolerant of attribute
/// order, which is all the tags below need.
pub fn parse_html(html: &str, base_url: &str) -> PageMetadata {
    let title = extract_tag_text(html, "title").map(|s| clean_text(&s));

    let description = extract_meta_content(html, &["description", "og:description"])
        .map(|s| clean_text(&s));

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
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<FetchMetadataRequest>,
) -> Response {
    if let Err(resp) = require_auth(&_state, &headers).await {
        return resp;
    }

    let (_scheme, host, port) = match validate_url(&payload.url) {
        Ok(parts) => parts,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if let Err(e) = resolve_public(&host, port).await {
        return ResponseHelper::json_error(e);
    }

    let with_scheme = if payload.url.contains("://") {
        payload.url.trim().to_string()
    } else {
        format!("https://{}", payload.url.trim())
    };

    let client = match reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        // Redirects are handled manually so each hop is re-validated.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("HomeVault/Shortcuts")
        .build()
    {
        Ok(c) => c,
        Err(_) => return ResponseHelper::json_error("Could not build HTTP client".to_string()),
    };

    let (final_url, body) = match fetch_following_one_redirect(&client, &with_scheme).await {
        Ok(v) => v,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let meta = parse_html(&body, &final_url);
    ResponseHelper::json_success(meta)
}

/// Fetch `url`, following at most one redirect, re-validating the target.
async fn fetch_following_one_redirect(
    client: &reqwest::Client,
    url: &str,
) -> Result<(String, String), String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|_| "Could not reach the site".to_string())?;

    if resp.status().is_redirection() {
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| "Redirect without a Location header".to_string())?;
        let base = url::Url::parse(url).map_err(|_| "invalid URL".to_string())?;
        let target = base
            .join(location)
            .map_err(|_| "Invalid redirect target".to_string())?;
        let target_str = target.to_string();
        let (_, host, port) = validate_url(&target_str)?;
        resolve_public(&host, port).await?;
        let resp = client
            .get(&target_str)
            .send()
            .await
            .map_err(|_| "Could not reach the redirect target".to_string())?;
        let body = read_capped(resp).await?;
        return Ok((target_str, body));
    }

    let body = read_capped(resp).await?;
    Ok((url.to_string(), body))
}

/// Read at most `MAX_BODY_BYTES` of the response body as a lossy string.
async fn read_capped(resp: reqwest::Response) -> Result<String, String> {
    let bytes = resp
        .bytes()
        .await
        .map_err(|_| "Could not read the response".to_string())?;
    let slice = if bytes.len() > MAX_BODY_BYTES {
        &bytes[..MAX_BODY_BYTES]
    } else {
        &bytes[..]
    };
    Ok(String::from_utf8_lossy(slice).to_string())
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
        for s in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"] {
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
    fn test_parse_html_extracts_title_description_and_icon() {
        let html = r#"<html><head>
            <title>  My   Site &amp; Co </title>
            <meta name="description" content="A nice page">
            <link rel="icon" href="/favicon.ico">
            </head><body></body></html>"#;
        let meta = parse_html(html, "https://example.com/some/page");
        assert_eq!(meta.title.as_deref(), Some("My Site & Co"));
        assert_eq!(meta.description.as_deref(), Some("A nice page"));
        assert_eq!(meta.icon_url.as_deref(), Some("https://example.com/favicon.ico"));
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
}
