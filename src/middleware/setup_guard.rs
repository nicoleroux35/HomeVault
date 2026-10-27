/* =============================================================================
 * File          : setup_guard.rs
 * Author        : Nicolas Le Roux
 * Created       : 12/10/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Middleware that keeps the unauthenticated first-run setup endpoints
 *   (creating the first admin account) reachable only from the local
 *   network (loopback, private, link-local, IPv6 unique-local) *and* only
 *   from a same-origin browser context. The peer address comes from the TCP
 *   connection itself, never from spoofable forwarding headers; the
 *   same-origin check defeats a cross-site page in the operator's own browser
 *   (the CORS layer is permissive, so without it any website could claim the
 *   first admin). Requests are also rate-limited per peer.
 *   HOMEVAULT_SETUP_ALLOW_REMOTE=1 opts out of the local-network check for a
 *   deliberate remote first-run setup (the same-origin check stays on).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Applied as a route_layer on the /auth/setup* bootstrap routes in
 *   src/web/mod.rs.
 * =============================================================================
 */

//! Restrict the unauthenticated first-run bootstrap endpoints to the local
//! network *and* to same-origin browser contexts, rate-limited per peer.
//!
//! `POST /auth/setup`, `POST /auth/setup/verify-totp` and `GET
//! /auth/check-username` are public by necessity — they exist precisely
//! because no admin account exists yet, so there is nobody to authenticate
//! against. On a shared or port-forwarded network that means the first client
//! to reach the appliance could claim the first admin account. This middleware
//! closes that window: those three endpoints answer only clients whose TCP
//! peer address is loopback or in a private / link-local / ULA range, which is
//! the only network a freshly-flashed appliance is legitimately set up from.
//!
//! The peer address is taken from the connection (`ConnectInfo`), **never**
//! from `X-Forwarded-For`/`X-Real-IP` — those are client-settable and would let
//! anyone forge a private source address when the appliance is reachable
//! directly. Operators who deliberately set up over a public address can opt
//! out with `HOMEVAULT_SETUP_ALLOW_REMOTE=1`.
//!
//! Deliberately **not** applied to `GET /auth/setup-status` (the login screen
//! must be able to read it from anywhere) or to the admin-authenticated
//! re-baseline endpoint (it already requires an admin session).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::{extract::ConnectInfo, http::HeaderMap, middleware::Next, response::Response};

use crate::middleware::rate_limit::RateLimitState;

/// Whether a peer address may reach the first-run bootstrap endpoints.
pub fn is_local_peer(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                // IPv4-mapped IPv6 (`::ffff:a.b.c.d`) is how a v4 client shows
                // up on a dual-stack socket — unwrap and re-check as IPv4.
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| v4.is_loopback() || v4.is_private() || v4.is_link_local())
                    .unwrap_or(false)
        }
    }
}

/// Explicit opt-out for the rare deliberate remote first-run setup.
fn remote_allowed_by_env() -> bool {
    matches!(
        std::env::var("HOMEVAULT_SETUP_ALLOW_REMOTE")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

// ---------------------------------------------------------------------------
// Same-origin check
// ---------------------------------------------------------------------------

/// Whether a bootstrap request may be treated as coming from the appliance's
/// own UI rather than from a third-party page open in the same browser.
///
/// The CORS layer in `web/mod.rs` is permissive (`allow_origin(Any)`) so the
/// desktop can talk to its own API from any host name. That would let *any*
/// website the operator happens to visit fire `POST /api/auth/setup` from
/// their browser — the local-network guard does not help, because the browser
/// itself sits on the LAN. This check closes that hole:
///
/// * an `Origin` header, when present, must match the request's `Host` header
///   or HTTP/2 `:authority` pseudo-header (same scheme-agnostic authority) — a
///   cross-site `fetch`/form always carries a foreign `Origin`;
/// * `Sec-Fetch-Site`, when the browser sends it, must be `same-origin` or
///   `none` (a user-initiated navigation / address-bar request). `cross-site`
///   and `same-site` (a sibling subdomain, which is a different origin) are
///   rejected;
/// * requests with neither header are allowed: non-browser clients (curl, a
///   provisioning script over SSH) send neither, and they are already gated by
///   the local-network check plus the setup lock.
fn is_same_origin(headers: &HeaderMap, uri_authority: Option<&str>) -> bool {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        let site = site.trim().to_ascii_lowercase();
        if !matches!(site.as_str(), "same-origin" | "none") {
            return false;
        }
    }

    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let origin = origin.trim();
    // `Origin: null` (sandboxed iframe, file://) can never be the appliance.
    if origin.is_empty() || origin.eq_ignore_ascii_case("null") {
        return false;
    }

    // Compare authorities only: `http://nas.local:8080` vs `Host: nas.local:8080`
    // (or the HTTP/2 `:authority` pseudo-header exposed on `request.uri()`).
    let Some(origin_host) = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split('/').next())
    else {
        return false;
    };
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| uri_authority.map(str::trim).filter(|s| !s.is_empty()));
    let Some(host) = host else {
        return false;
    };

    origin_host.eq_ignore_ascii_case(host)
}

// ---------------------------------------------------------------------------
// Per-peer rate limit
// ---------------------------------------------------------------------------

/// Bootstrap endpoints get their own limiter: 20 requests per minute per peer.
/// Generous enough for the wizard's live username checks and TOTP retries, far
/// below what a brute-force or a flood needs. The limiter lives in a `OnceLock`
/// rather than `AppState` so the guard stays a self-contained route layer (and
/// the many `AppState` fixtures in the test suites need no change).
///
/// `/auth/setup/progress` is deliberately **exempt** (see
/// [`setup_guard_middleware`]): it is wizard bookkeeping fired on every step
/// navigation, not a security-sensitive endpoint. Counting it made the wizard
/// exhaust its own budget — a few step round-trips and the next
/// `check-username` came back 429, which the UI then read as "username already
/// taken" and refused to leave the account step. The budget is for guessing
/// (`setup`, `verify-totp`) and enumeration (`check-username`).
fn setup_rate_limiter() -> &'static Arc<RateLimitState> {
    static LIMITER: std::sync::OnceLock<Arc<RateLimitState>> = std::sync::OnceLock::new();
    LIMITER.get_or_init(|| Arc::new(RateLimitState::new(20, 60)))
}

/// Whether a bootstrap request counts against [`setup_rate_limiter`].
///
/// Only the three endpoints a guess or an enumeration can target are metered;
/// `GET/PUT /auth/setup/progress` stores the wizard's own step counter and is
/// reachable only from the same guarded, local, same-origin context anyway.
fn counts_against_setup_budget(path: &str) -> bool {
    !path.trim_end_matches('/').ends_with("/auth/setup/progress")
}

/// Middleware enforcing [`is_local_peer`] + [`is_same_origin`] + the per-peer
/// rate limit on the setup bootstrap routes.
pub async fn setup_guard_middleware(request: axum::extract::Request, next: Next) -> Response {
    // Same-origin and rate limiting apply even when the local-network check is
    // deliberately lifted: they protect a different hole.
    if !is_same_origin(
        request.headers(),
        request.uri().authority().map(|a| a.as_str()),
    ) {
        tracing::warn!(
            origin = ?request.headers().get("origin"),
            "Rejected a cross-origin first-run setup request"
        );
        return crate::utils::ResponseHelper::json_forbidden(
            "First-run setup is only available from HomeVault's own interface".to_string(),
        );
    }

    let peer_key = match request.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(addr)) => addr.ip().to_string(),
        None => "in-process".to_string(),
    };
    if counts_against_setup_budget(request.uri().path())
        && setup_rate_limiter().check_rate_limit(&peer_key).is_err()
    {
        return crate::utils::ResponseHelper::json_error_with_status(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "Too many setup requests, please wait a minute".to_string(),
        );
    }

    if remote_allowed_by_env() {
        return next.run(request).await;
    }

    // `None` only for in-process calls (tests) that never went through a real
    // socket — a real deployment always has a peer address, so this cannot be
    // used to bypass the guard from the network.
    let allowed = match request.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(addr)) => is_local_peer(&addr.ip()),
        None => true,
    };

    if allowed {
        next.run(request).await
    } else {
        crate::utils::ResponseHelper::json_forbidden(
            "First-run setup is only available from the local network".to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn test_loopback_is_local() {
        assert!(is_local_peer(&v4("127.0.0.1")));
        assert!(is_local_peer(&v4("127.9.9.9")));
        assert!(is_local_peer(&v4("::1")));
    }

    #[test]
    fn test_private_ranges_are_local() {
        assert!(is_local_peer(&v4("10.0.0.5")));
        assert!(is_local_peer(&v4("172.16.4.4")));
        assert!(is_local_peer(&v4("172.31.255.254")));
        assert!(is_local_peer(&v4("192.168.1.20")));
    }

    #[test]
    fn test_link_local_is_local() {
        assert!(is_local_peer(&v4("169.254.1.1")));
        assert!(is_local_peer(&v4("fe80::1")));
    }

    #[test]
    fn test_unique_local_v6_is_local() {
        assert!(is_local_peer(&v4("fd00::1")));
        assert!(is_local_peer(&v4("fc00::abcd")));
    }

    #[test]
    fn test_ipv4_mapped_v6_is_unwrapped() {
        assert!(is_local_peer(&v4("::ffff:192.168.0.10")));
        assert!(is_local_peer(&v4("::ffff:127.0.0.1")));
        assert!(!is_local_peer(&v4("::ffff:8.8.8.8")));
    }

    #[test]
    fn test_public_addresses_are_rejected() {
        assert!(!is_local_peer(&v4("8.8.8.8")));
        assert!(!is_local_peer(&v4("1.1.1.1")));
        assert!(!is_local_peer(&v4("172.32.0.1")));
        assert!(!is_local_peer(&v4("2001:4860:4860::8888")));
    }

    #[test]
    fn test_progress_route_is_exempt_from_the_setup_budget() {
        // Wizard bookkeeping must not eat the budget meant for guessing /
        // enumeration — the wizard fires one PUT per step navigation and then
        // needs a check-username right after.
        assert!(!counts_against_setup_budget("/api/auth/setup/progress"));
        assert!(!counts_against_setup_budget("/api/auth/setup/progress/"));
        // The three endpoints a guess or an enumeration can target stay metered.
        assert!(counts_against_setup_budget("/api/auth/setup"));
        assert!(counts_against_setup_budget("/api/auth/setup/verify-totp"));
        assert!(counts_against_setup_budget("/api/auth/check-username"));
    }

    #[test]
    fn test_is_same_origin_accepts_http2_authority_when_host_header_is_absent() {
        let mut headers = HeaderMap::new();
        headers.insert("origin", "https://homevault.local:8443".parse().unwrap());
        headers.insert("sec-fetch-site", "same-origin".parse().unwrap());

        assert!(is_same_origin(&headers, Some("homevault.local:8443")));
        assert!(!is_same_origin(&headers, Some("evil.example:8443")));
        assert!(!is_same_origin(&headers, None));
    }
}
