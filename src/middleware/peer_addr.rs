/* =============================================================================
 * File          : peer_addr.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 17/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Optional extractor for the TCP peer address. Axum's own `ConnectInfo`
 *   fails the request with a 500 when the router was not built with
 *   `into_make_service_with_connect_info` (which is the case for the
 *   in-process `oneshot` routers used by the integration tests), so handlers
 *   that merely want to *record* or throttle by the peer address cannot use it
 *   directly. `PeerAddr` always succeeds and carries `None` when the metadata
 *   is absent.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   `async fn handler(peer: PeerAddr, ...)`; pass `peer.0` to
 *   `handlers::auth::client_ip`.
 * =============================================================================
 */

//! Optional TCP peer address extractor.

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::request::Parts;

/// The connection's peer address, when the server exposed it.
///
/// Unlike [`ConnectInfo`], a missing value is not an error: in-process callers
/// (tests, `oneshot`) simply get `None`.
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub Option<IpAddr>);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for PeerAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip());
        Ok(PeerAddr(ip))
    }
}
