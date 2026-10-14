/* =============================================================================
 * File          : mdns.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Read-only mDNS status endpoint: the address HomeVault is reachable at
 *   (homevault.local) and the share protocols currently advertised on the
 *   LAN. Publishing is automatic (background reconciler in crate::mdns);
 *   there is nothing to configure. Session-auth rather than admin-only,
 *   since the same information is already broadcast unauthenticated on the
 *   local segment.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   GET /api/mdns/status in src/web/mod.rs.
 * =============================================================================
 */

//! Read-only mDNS status endpoint — lets any signed-in user see the address
//! HomeVault is reachable at (`homevault.local`) and which share protocols
//! are currently advertised on the LAN. Publishing itself is automatic (see
//! `crate::mdns` — a background reconciler); there is nothing to configure.
//!
//! `require_auth` rather than `require_admin`: mDNS already broadcasts this
//! exact information (service names + ports) unauthenticated to every device
//! on the local segment, so gating it behind admin would be theatre — but it
//! is host-shaped info, so it stays behind a session.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

/// `GET /api/mdns/status` — `{ available, running, host_name, fqdn,
/// http_url, services[] }`. Never fails on a host without Avahi; it just
/// reports `available: false` and still returns the canonical address.
pub async fn handle_mdns_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let port = crate::config::Settings::load().port;
    match tokio::task::spawn_blocking(move || crate::mdns::status(port)).await {
        Ok(status) => ResponseHelper::json_success(status),
        Err(e) => ResponseHelper::json_error(format!("mDNS status task failed: {e}")),
    }
}
