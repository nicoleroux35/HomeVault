/* =============================================================================
 * File          : wol.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Wake-on-LAN endpoints: list/save/delete the hosts this box may wake, list
 *   the interfaces a magic packet could leave through, and send one now.
 *   Admin-only — a wake packet is a network action taken by the appliance.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::wol, crate::storage::WolHostRepository
 *
 * Usage:
 *   Routes /api/power/wol in src/web/mod.rs; see docs/guides/power.md.
 * =============================================================================
 */

//! Wake-on-LAN endpoints (admin-only), see docs/guides/power.md.
//!
//! * `GET    /api/power/wol/hosts`      — saved hosts + the live interface list
//! * `POST   /api/power/wol/hosts`      — add a host
//! * `PUT    /api/power/wol/hosts/{id}` — edit a host
//! * `DELETE /api/power/wol/hosts/{id}` — forget a host
//! * `POST   /api/power/wol/wake`       — send a magic packet now

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;

use crate::handlers::admin_guard::require_admin;
use crate::storage::WolHostRepository;
use crate::utils::ResponseHelper;
use crate::wol::{self, WolHost};

use super::AppState;

/// Payload of one host, as the card sends it.
#[derive(Debug, Deserialize)]
pub struct WolHostRequest {
    pub label: String,
    pub mac: String,
    /// Optional IP literal to send to instead of the interface's directed
    /// broadcast (a relay, another VLAN, a test sink).
    #[serde(default)]
    pub broadcast: Option<String>,
}

/// Payload of a wake request: a saved host, or a one-off MAC.
#[derive(Debug, Deserialize)]
pub struct WakeRequest {
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub mac: Option<String>,
}

/// The interface evidence the card shows: which segment a packet would use.
fn interface_evidence() -> Vec<serde_json::Value> {
    wol::live_interfaces()
        .into_iter()
        .map(|iface| {
            serde_json::json!({
                "name": iface.name,
                "address": iface.address.to_string(),
                "prefix": iface.prefix,
                "broadcast": iface.broadcast.to_string(),
                "up": iface.up,
            })
        })
        .collect()
}

fn hosts_payload(hosts: &[WolHost]) -> serde_json::Value {
    serde_json::json!({
        "hosts": hosts,
        "interfaces": interface_evidence(),
        "ports": wol::WOL_PORTS,
        "max_hosts": wol::MAX_HOSTS,
        "max_wake_delay_seconds": wol::MAX_WAKE_DELAY_SECONDS,
    })
}

/// `GET /api/power/wol/hosts`
pub async fn handle_wol_hosts_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = WolHostRepository::new(state.db.pool());
    match repo.list().await {
        Ok(hosts) => ResponseHelper::json_success(hosts_payload(&hosts)),
        Err(e) => ResponseHelper::json_error(format!("Failed to list wake-on-LAN hosts: {e}")),
    }
}

/// Validate the incoming host and produce the canonical stored form.
fn normalized(body: &WolHostRequest) -> Result<(String, String, Option<String>), String> {
    let label = wol::validate_label(&body.label)?;
    let mac = wol::format_mac(&wol::parse_mac(&body.mac)?);
    let broadcast = wol::validate_broadcast(body.broadcast.as_deref().unwrap_or(""))?;
    Ok((label, mac, broadcast))
}

/// `POST /api/power/wol/hosts`
pub async fn handle_wol_host_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WolHostRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let (label, mac, broadcast) = match normalized(&body) {
        Ok(parts) => parts,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let repo = WolHostRepository::new(state.db.pool());
    match repo.list().await {
        Ok(hosts) if hosts.len() >= wol::MAX_HOSTS => {
            return ResponseHelper::json_error(format!(
                "The list is full ({} hosts) — delete one first",
                wol::MAX_HOSTS
            ))
        }
        Ok(_) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the host list: {e}")),
    }
    match repo.label_taken(&label, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!("A host named '{label}' already exists"))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }

    match repo.create(&label, &mac, broadcast.as_deref()).await {
        Ok(host) => {
            crate::events::log_event(format!(
                "Admin '{}' saved wake-on-LAN host '{}' ({})",
                username, host.label, host.mac
            ));
            ResponseHelper::json_success(serde_json::json!({ "host": host }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the host: {e}")),
    }
}

/// `PUT /api/power/wol/hosts/{id}`
pub async fn handle_wol_host_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<WolHostRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let (label, mac, broadcast) = match normalized(&body) {
        Ok(parts) => parts,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let repo = WolHostRepository::new(state.db.pool());
    match repo.label_taken(&label, Some(id)).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!("A host named '{label}' already exists"))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }
    match repo.update(id, &label, &mac, broadcast.as_deref()).await {
        Ok(host) => {
            crate::events::log_event(format!(
                "Admin '{}' updated wake-on-LAN host '{}' ({})",
                username, host.label, host.mac
            ));
            ResponseHelper::json_success(serde_json::json!({ "host": host }))
        }
        Err(crate::storage::DbError::NotFound) => {
            ResponseHelper::json_error("That host no longer exists".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the host: {e}")),
    }
}

/// `DELETE /api/power/wol/hosts/{id}`
pub async fn handle_wol_host_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let repo = WolHostRepository::new(state.db.pool());
    let existing = repo.find(id).await.ok().flatten();
    match repo.delete(id).await {
        Ok(()) => {
            if let Some(host) = existing {
                crate::events::log_event(format!(
                    "Admin '{}' removed wake-on-LAN host '{}' ({})",
                    username, host.label, host.mac
                ));
            }
            ResponseHelper::json_success(serde_json::json!({ "deleted": true }))
        }
        Err(crate::storage::DbError::NotFound) => {
            ResponseHelper::json_error("That host no longer exists".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete the host: {e}")),
    }
}

/// `POST /api/power/wol/wake`
///
/// Answers `sent`, never `woken`: see [`crate::wol::WakeReport::message`].
pub async fn handle_wol_wake(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<WakeRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    // A saved host, or a one-off MAC for "will this even work?".
    let host = match (body.id, body.mac.as_deref()) {
        (Some(id), _) => {
            let repo = WolHostRepository::new(state.db.pool());
            match repo.find(id).await {
                Ok(Some(host)) => host,
                Ok(None) => {
                    return ResponseHelper::json_error("That host no longer exists".to_string())
                }
                Err(e) => {
                    return ResponseHelper::json_error(format!("Failed to read the host: {e}"))
                }
            }
        }
        (None, Some(mac)) => {
            let mac = match wol::parse_mac(mac) {
                Ok(mac) => wol::format_mac(&mac),
                Err(e) => return ResponseHelper::json_error(e),
            };
            WolHost {
                id: 0,
                label: mac.clone(),
                mac,
                broadcast: None,
                created_at: String::new(),
                updated_at: String::new(),
            }
        }
        (None, None) => {
            return ResponseHelper::json_error(
                "Choose a saved host or enter a MAC address".to_string(),
            )
        }
    };

    match wol::wake(&host, &wol::live_interfaces()).await {
        Ok(report) => {
            crate::events::log_event(format!(
                "Admin '{}' sent a wake-on-LAN packet to '{}' ({}) via {}",
                username,
                host.label,
                report.mac,
                report.destination
            ));
            ResponseHelper::json_success(serde_json::json!({
                "sent": true,
                "report": report,
                // The card shows this sentence verbatim: the only honest
                // claim an unacknowledged datagram supports.
                "message": report.message(),
            }))
        }
        Err(reason) => ResponseHelper::json_error(format!(
            "The magic packet could not be sent: {reason}"
        )),
    }
}
