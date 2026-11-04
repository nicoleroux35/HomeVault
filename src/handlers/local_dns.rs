/* =============================================================================
 * File          : local_dns.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Endpoints for the appliance's own DNS server: read the configuration and
 *   the live state, and save a configuration (which applies it at once).
 *
 *   Reads need a session; the save is admin-only, because it takes over name
 *   resolution for the whole LAN — a wrong record there breaks every device on
 *   the network, not just this screen.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::dhcp_server (the shared dnsmasq daemon),
 *     crate::local_dns
 *
 * Usage:
 *   Routes under /api/network/dns-server in src/web/mod.rs; see
 *   docs/guides/local-dns.md.
 * ============================================================================= */

//! The local DNS server endpoints, see docs/guides/local-dns.md.
//!
//! * `GET /api/network/dns-server` — configuration, live state, capability
//! * `PUT /api/network/dns-server` — validate, save and apply

use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::dhcp_server::DhcpServerService;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::local_dns::{self, LocalDnsConfig};
use crate::utils::ResponseHelper;

/// GET /api/network/dns-server — configuration, live state, and what the screen
/// needs to render a real choice: the interfaces that could answer, and the
/// host's current upstream resolvers as suggestions.
///
/// `available` is the `dnsmasq` capability gate, shared with the DHCP card: the
/// endpoint never fails because the package is missing, it says so.
pub async fn handle_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let config = match local_dns::load_persisted().await {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };

    let available = DhcpServerService::is_available().await;
    let running = DhcpServerService::is_running().await;
    // The switch that tells DHCP clients to resolve here only means something
    // while the address server is on; the card says so instead of hiding it.
    let dhcp_enabled = DhcpServerService::load_persisted()
        .await
        .map(|config| config.dhcp_enabled)
        .unwrap_or(false);

    let mut interfaces = local_dns::interface_candidates();
    // A saved interface that no longer exists is still shown, so the screen
    // reports the truth instead of silently selecting another one.
    if !config.interface.is_empty() && !interfaces.iter().any(|i| i == &config.interface) {
        interfaces.push(config.interface.clone());
    }

    ResponseHelper::json_success(LocalDnsGetResponse {
        state: LocalDnsState {
            running,
            serving: running && config.enabled,
        },
        config,
        available,
        interfaces,
        system_forwarders: local_dns::system_forwarders(),
        dhcp_enabled,
    })
}

/// PUT /api/network/dns-server — validate, persist, then apply.
///
/// The guard runs before the body is parsed, so an unauthenticated caller gets
/// the API's `401` envelope and learns nothing about the payload shape.
///
/// Applying means reconciling the shared `dnsmasq` daemon: the previous
/// configuration is put back if the host refuses the new one, so "saved" and
/// "serving" can never disagree silently.
pub async fn handle_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let req: LocalDnsUpdateRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            return ResponseHelper::json_error(format!("Malformed request body: {}", e));
        }
    };

    let previous = match local_dns::load_persisted().await {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };

    let next = match local_dns::validate_config(&req.config) {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_error(e.0),
    };

    if let Err(e) = local_dns::save_persisted(&next).await {
        return ResponseHelper::json_internal_error(e);
    }

    let running = match DhcpServerService::reconcile_daemon().await {
        Ok(running) => running,
        Err(e) => {
            // The host refused it: put the previous document back and re-apply,
            // so the screen never shows a configuration that is not in force.
            if let Err(restore) = local_dns::save_persisted(&previous).await {
                tracing::error!(
                    "Restoring the previous local DNS settings failed: {}",
                    restore
                );
            }
            if let Err(restore) = DhcpServerService::reconcile_daemon().await {
                tracing::error!(
                    "Re-applying the previous DNS configuration failed: {}",
                    restore
                );
            }
            return ResponseHelper::json_internal_error(format!(
                "The DNS configuration was refused by the host: {}",
                e
            ));
        }
    };

    crate::events::log_event(format!(
        "Admin '{}' {} the local DNS server ({} local name(s), {} upstream(s), {} answered domain(s))",
        admin,
        if next.enabled { "enabled" } else { "disabled" },
        next.records.len(),
        next.forwarders.len(),
        next.overrides.len()
    ));

    ResponseHelper::json_success(LocalDnsUpdateResponse {
        config: next.clone(),
        state: LocalDnsState {
            running,
            serving: running && next.enabled,
        },
        message: if next.enabled {
            "The local DNS server is configured".to_string()
        } else {
            "The local DNS server is disabled".to_string()
        },
    })
}

/// The live state of the daemon, as far as this feature is concerned.
#[derive(Debug, Serialize)]
pub struct LocalDnsState {
    /// `dnsmasq` is running on the host (it also serves DHCP/TFTP).
    pub running: bool,
    /// Running *and* this feature is enabled: queries are being answered.
    pub serving: bool,
}

#[derive(Debug, Serialize)]
pub struct LocalDnsGetResponse {
    pub config: LocalDnsConfig,
    pub state: LocalDnsState,
    /// `dnsmasq` is installed.
    pub available: bool,
    /// Interfaces that could answer, for the screen's chooser.
    pub interfaces: Vec<String>,
    /// The host's current upstream resolvers, offered as suggestions.
    pub system_forwarders: Vec<String>,
    /// Whether the DHCP server is on, which is when
    /// `config.serve_dhcp_clients` does anything.
    pub dhcp_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct LocalDnsUpdateRequest {
    pub config: LocalDnsConfig,
}

#[derive(Debug, Serialize)]
pub struct LocalDnsUpdateResponse {
    pub config: LocalDnsConfig,
    pub state: LocalDnsState,
    pub message: String,
}
