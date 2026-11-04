/* =============================================================================
 * File          : upnp.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPnP-IGD endpoints (phase C3): read the router's mappings and external
 *   address (read-only discovery), add one mapping, remove one. Reads need a
 *   session; every mutation is admin-only and must carry an explicit
 *   confirmation, because it opens or closes a door on the router.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::upnp, crate::network
 *
 * Usage:
 *   Routes under /api/network/upnp in src/web/mod.rs; see docs/guides/upnp.md.
 * ============================================================================= */

//! The UPnP-IGD endpoints, see docs/guides/upnp.md.
//!
//! * `GET    /api/network/upnp`           — capability, gateway, external IP,
//!   local address, mappings
//! * `POST   /api/network/upnp/mappings`  — add one, then return the new state
//! * `DELETE /api/network/upnp/mappings`  — remove one, then return the state

use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response};
use serde::Serialize;
use std::sync::Arc;

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::upnp::{self, NewMapping, PortMapping, RemoveMapping, UpnpSnapshot};
use crate::utils::ResponseHelper;

/// Run a blocking `upnpc` call off the async runtime.
async fn blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(e) => Err(format!("The UPnP worker stopped unexpectedly: {e}")),
    }
}

/// Read the router and, in the same worker, the appliance's own LAN address
/// (the form's sensible default for the internal client).
async fn read_router() -> (Result<UpnpSnapshot, String>, Option<String>) {
    match tokio::task::spawn_blocking(|| {
        let snapshot = upnp::snapshot();
        let suggested = crate::network::discover_lan_ipv4_addresses()
            .into_iter()
            .next()
            .map(|address| address.to_string());
        (snapshot, suggested)
    })
    .await
    {
        Ok(result) => result,
        Err(e) => (
            Err(format!("The UPnP worker stopped unexpectedly: {e}")),
            None,
        ),
    }
}

/// `GET /api/network/upnp`
pub async fn handle_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    if !upnp::available() {
        return ResponseHelper::json_success(UpnpGetResponse {
            available: false,
            gateway: None,
            external_ip: None,
            local_address: None,
            mappings: Vec::new(),
            error: None,
            suggested_host: None,
        });
    }

    let (snapshot, suggested_host) = read_router().await;
    let (snapshot, error) = match snapshot {
        Ok(snapshot) => (snapshot, None),
        Err(error) => (UpnpSnapshot::default(), Some(error)),
    };

    ResponseHelper::json_success(UpnpGetResponse {
        available: true,
        gateway: snapshot.gateway,
        external_ip: snapshot.external_ip,
        local_address: snapshot.local_address,
        mappings: snapshot.mappings,
        error,
        suggested_host,
    })
}

/// `POST /api/network/upnp/mappings`
pub async fn handle_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let input: NewMapping = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };
    let validated = match upnp::validate_new(&input) {
        Ok(validated) => validated,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if !upnp::available() {
        return ResponseHelper::json_unavailable(format!(
            "The UPnP client (upnpc) is not installed; install the {} package.",
            upnp::REQUIRED_PACKAGE
        ));
    }

    let written = validated.clone();
    if let Err(e) = blocking(move || upnp::add(&written)).await {
        return ResponseHelper::json_error_with_status(axum::http::StatusCode::BAD_GATEWAY, e);
    }

    crate::events::log_event(format!(
        "Admin '{admin}' opened {} {} -> {}:{} on the router (lease {})",
        validated.mapping.protocol.as_upnpc(),
        validated.mapping.external_port,
        validated.mapping.internal_host,
        validated.mapping.internal_port,
        lease_label(validated.mapping.lease_seconds)
    ));

    mutation_response("The mapping is added.").await
}

/// `DELETE /api/network/upnp/mappings`
pub async fn handle_remove(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let input: RemoveMapping = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };
    if let Err(e) = upnp::validate_remove(&input) {
        return ResponseHelper::json_error(e);
    }
    if !upnp::available() {
        return ResponseHelper::json_unavailable(format!(
            "The UPnP client (upnpc) is not installed; install the {} package.",
            upnp::REQUIRED_PACKAGE
        ));
    }

    let protocol = input.protocol;
    let external_port = input.external_port;
    if let Err(e) = blocking(move || upnp::remove(protocol, external_port)).await {
        return ResponseHelper::json_error_with_status(axum::http::StatusCode::BAD_GATEWAY, e);
    }

    crate::events::log_event(format!(
        "Admin '{admin}' removed the router mapping {} {}",
        input.protocol.as_upnpc(),
        input.external_port
    ));

    mutation_response("The mapping is removed.").await
}

/// Answer after a successful write: the router's refreshed state, or the
/// reason it could not be re-read (the write itself succeeded).
async fn mutation_response(message: &str) -> Response {
    let (snapshot, _suggested) = read_router().await;
    let (snapshot, error) = match snapshot {
        Ok(snapshot) => (Some(snapshot), None),
        Err(error) => (None, Some(error)),
    };
    ResponseHelper::json_success(UpnpMutationResponse {
        snapshot,
        error,
        message: message.to_string(),
    })
}

fn lease_label(seconds: u32) -> String {
    if seconds == 0 {
        "permanent".to_string()
    } else {
        format!("{seconds}s")
    }
}

#[derive(Debug, Serialize)]
pub struct UpnpGetResponse {
    /// `upnpc` is installed.
    pub available: bool,
    pub gateway: Option<String>,
    pub external_ip: Option<String>,
    pub local_address: Option<String>,
    pub mappings: Vec<PortMapping>,
    /// Why discovery failed, when it did. Never a capability failure: that is
    /// `available: false`.
    pub error: Option<String>,
    /// The appliance's own LAN address, offered as the form's default.
    pub suggested_host: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpnpMutationResponse {
    pub snapshot: Option<UpnpSnapshot>,
    pub error: Option<String>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lease_label_is_readable() {
        assert_eq!(lease_label(0), "permanent");
        assert_eq!(lease_label(3600), "3600s");
    }
}
