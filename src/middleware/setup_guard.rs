//! Restrict the unauthenticated first-run bootstrap endpoints to the local
//! network.
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

use axum::{extract::ConnectInfo, middleware::Next, response::Response};

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

/// Middleware enforcing [`is_local_peer`] on the setup bootstrap routes.
pub async fn setup_guard_middleware(request: axum::extract::Request, next: Next) -> Response {
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
}
