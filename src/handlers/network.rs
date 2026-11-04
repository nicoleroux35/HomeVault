/* =============================================================================
 * File          : network.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Network endpoints: interface inventory and per-interface details,
 *   configuration read/update (IP, gateway, DNS), interface up/down,
 *   Wi-Fi scan/connect/disconnect, diagnostics (ping, DNS resolution),
 *   public-IP lookup and system service status. Reads are session-auth;
 *   configuration changes and Wi-Fi association are admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/network in src/web/mod.rs; hardware facts come from
 *   crate::network and ProcParser.
 * =============================================================================
 */

//! Network endpoints: interfaces, configuration, Wi-Fi, diagnostics —
//! reads session-auth, changes admin-only.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppStateArc;
use crate::network as net;
use crate::utils::proc::ProcParser;
use crate::utils::response::ResponseHelper;

// Re-export network types for convenience
pub use crate::network::{
    InterfaceConfig, InterfaceStatus, InterfaceType, NetworkInterface, WifiConnection, WifiNetwork,
};

// ---------------------------------------------------------------------------
// API Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct InterfacesResponse {
    pub interfaces: Vec<NetworkInterface>,
}

#[derive(Debug, Serialize)]
pub struct InterfaceResponse {
    pub interface: NetworkInterface,
}

#[derive(Debug, Deserialize)]
pub struct InterfaceConfigRequest {
    pub method: String,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub netmask: Option<String>,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub dns_servers: Vec<String>,
    #[serde(default)]
    pub ipv6_method: Option<String>,
    #[serde(default)]
    pub ipv6: Option<String>,
    #[serde(default)]
    pub ipv6_prefix: Option<u8>,
    #[serde(default)]
    pub ipv6_gateway: Option<String>,
    #[serde(default)]
    pub ipv6_dns_servers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WifiScanResponse {
    pub networks: Vec<WifiNetwork>,
}

#[derive(Debug, Serialize)]
pub struct WifiStatusResponse {
    pub connected: Option<WifiConnection>,
    pub interface: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiSuccess {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticsResponse {
    pub success: bool,
    pub gateway_reachable: bool,
    pub dns_working: bool,
    pub internet_reachable: bool,
    pub latency_ms: Option<u64>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectionSummaryResponse {
    pub local_ip: Option<String>,
    pub gateway: Option<String>,
    pub dns_servers: Vec<String>,
    pub public_ip: Option<String>,
    pub primary_interface: Option<String>,
    pub connection_type: String,
}

#[derive(Debug, Serialize)]
pub struct PublicIpResponse {
    pub ip: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/network/interfaces - List all network interfaces
///
/// `net::list_interfaces()` shells out to `ip`/`ethtool` (several times per
/// interface, including virtual Docker interfaces) via `std::process::Command`
/// — genuinely blocking calls, not `tokio::process::Command`. This endpoint
/// is polled every 10s while the Network page is open
/// (frontend/src/routes/Network.svelte), so running it inline would
/// repeatedly stall a Tokio worker thread for the combined duration of every
/// subprocess spawned across every interface. Deported via `spawn_blocking`
/// to keep the exact existing behavior while taking it off the async runtime.
pub async fn handle_network_interfaces(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let interfaces = match tokio::task::spawn_blocking(net::list_interfaces).await {
        Ok(interfaces) => interfaces,
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    ResponseHelper::json_success(InterfacesResponse { interfaces })
}

/// GET /api/network/interfaces/:name - Get specific interface details
pub async fn handle_network_interface_get(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let interface =
        match tokio::task::spawn_blocking(move || net::get_interface_details(&name)).await {
            Ok(interface) => interface,
            Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
        };
    ResponseHelper::json_success(InterfaceResponse { interface })
}

/// GET /api/network/interfaces/:name/config - Get interface IP configuration
pub async fn handle_network_interface_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let config = match tokio::task::spawn_blocking(move || net::get_interface_config(&name)).await {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    ResponseHelper::json_success(config)
}

/// PUT /api/network/interfaces/:name/config - Apply IP configuration to interface
pub async fn handle_network_interface_config_update(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(config): Json<InterfaceConfigRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let method = match config.method.as_str() {
        "dhcp" => net::IpMethod::Dhcp,
        "static" => net::IpMethod::Static,
        _ => {
            return ResponseHelper::json_error(
                "Invalid method. Use 'dhcp' or 'static'".to_string(),
            );
        }
    };

    let ipv6_method = config.ipv6_method.as_ref().and_then(|m| match m.as_str() {
        "auto" => Some(net::Ipv6Method::Auto),
        "dhcp" => Some(net::Ipv6Method::Dhcp),
        "static" => Some(net::Ipv6Method::Static),
        "disabled" => Some(net::Ipv6Method::Disabled),
        _ => None,
    });

    let cfg = InterfaceConfig {
        method,
        ip: config.ip,
        netmask: config.netmask,
        gateway: config.gateway,
        dns_servers: config.dns_servers,
        ipv6_method,
        ipv6: config.ipv6,
        ipv6_prefix: config.ipv6_prefix,
        ipv6_gateway: config.ipv6_gateway,
        ipv6_dns_servers: config.ipv6_dns_servers,
    };

    // The whole put is blocking (reads, `ip`/`dhclient`/`netplan` shells, a
    // 10 s verify-poll on the reconcile) — spawn_blocking, same as the read
    // path and every other hardware-facing handler.
    //
    // Success tuple: (persisted?, rolled-back?). The order inside is the
    // safety contract:
    //   1. capture the PRE-change live state (the rollback target),
    //   2. live-apply (on failure nothing is touched — plain error),
    //   3. persist (best effort — never fails the request),
    //   4. reconcile the netplan file (apply + verify; on failure everything
    //      is rolled back to step 1's state and an error is reported).
    // `name` is needed again below (log/event lines) — the closure takes its
    // own copy.
    let iface_name = name.clone();
    let (persisted, rolled_back) =
        match tokio::task::spawn_blocking(move || -> Result<(bool, Option<String>), String> {
            let name = iface_name;
            let previous_config = net::get_interface_config(&name);

            net::apply_interface_config(&name, &cfg)?;

            let persisted = match net::persist_interface_config(&name, &cfg) {
                Ok(change) => Some(change),
                Err(e) => {
                    tracing::warn!("Network config persistence for '{}' failed: {}", name, e);
                    None
                }
            };

            let rolled_back = match &persisted {
                Some(change) => {
                    net::apply_persisted_config(&name, &cfg, change, &previous_config).err()
                }
                None => None,
            };

            Ok((persisted.is_some(), rolled_back))
        })
        .await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return ResponseHelper::json_error(e),
            Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
        };

    // A reconcile failure means the change did NOT survive: everything went
    // back to the pre-change state, so say so explicitly instead of letting
    // the operator discover that "success" was a mirage.
    if let Some(err) = rolled_back {
        crate::events::log_event(format!(
            "Admin '{}' updated network configuration for interface '{}' (method: {}) — netplan reconcile failed, change rolled back: {}",
            admin_username, name, config.method, err
        ));
        return ResponseHelper::json_error(err);
    }

    crate::events::log_event(format!(
        "Admin '{}' updated network configuration for interface '{}' (method: {})",
        admin_username, name, config.method
    ));
    ResponseHelper::json_success(ApiSuccess {
        success: true,
        message: Some(if persisted {
            format!("Configuration applied to {} (persisted)", name)
        } else {
            format!(
                "Configuration applied to {} (temporary — persistence unavailable)",
                name
            )
        }),
    })
}

/// PUT /api/network/interfaces/:name/state - Enable or disable interface
pub async fn handle_network_interface_state(
    State(app_state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(state): Json<serde_json::Value>,
) -> Response {
    let admin_username = match require_admin(&app_state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let Some(up) = state.get("up").and_then(|v| v.as_bool()) else {
        return ResponseHelper::json_error("Missing or invalid 'up' field".to_string());
    };

    if let Err(e) = net::set_interface_state(&name, up) {
        return ResponseHelper::json_error(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' set network interface '{}' {}",
        admin_username,
        name,
        if up { "up" } else { "down" }
    ));
    ResponseHelper::json_success(ApiSuccess {
        success: true,
        message: Some(format!(
            "Interface {} set to {}",
            name,
            if up { "up" } else { "down" }
        )),
    })
}

/// GET /api/network/wifi/scan - Scan for WiFi networks
pub async fn handle_network_wifi_scan(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let networks = match tokio::task::spawn_blocking(net::scan_wifi_networks).await {
        Ok(networks) => networks,
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    ResponseHelper::json_success(WifiScanResponse { networks })
}

/// GET /api/network/wifi/status - Get current WiFi connection status
///
/// Also polled every 10s alongside `handle_network_interfaces` while the
/// Network page is open — same rationale for `spawn_blocking` as above.
pub async fn handle_network_wifi_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let (interface, connected) =
        match tokio::task::spawn_blocking(|| (net::get_wifi_interface(), net::get_wifi_status()))
            .await
        {
            Ok(result) => result,
            Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
        };
    ResponseHelper::json_success(WifiStatusResponse {
        connected,
        interface,
    })
}

/// POST /api/network/wifi/connect - Connect to a WiFi network
pub async fn handle_network_wifi_connect(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let Some(ssid) = payload.get("ssid").and_then(|v| v.as_str()) else {
        return ResponseHelper::json_error("Missing 'ssid' field".to_string());
    };

    let password = payload
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let security = payload
        .get("security")
        .and_then(|v| v.as_str())
        .unwrap_or("wpa2");

    if let Err(e) = net::wifi_connect(ssid, password, security) {
        return ResponseHelper::json_error(e);
    }

    // Never log the password — only the SSID and security type.
    crate::events::log_event(format!(
        "Admin '{}' connected to WiFi network '{}'",
        admin_username, ssid
    ));
    ResponseHelper::json_success(ApiSuccess {
        success: true,
        message: Some(format!("Connected to {}", ssid)),
    })
}

/// POST /api/network/wifi/disconnect - Disconnect from current WiFi
pub async fn handle_network_wifi_disconnect(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let Some(ssid) = payload.get("ssid").and_then(|v| v.as_str()) else {
        return ResponseHelper::json_error("Missing 'ssid' field".to_string());
    };

    if let Err(e) = net::wifi_disconnect(ssid) {
        return ResponseHelper::json_error(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' disconnected from WiFi network '{}'",
        admin_username, ssid
    ));
    ResponseHelper::json_success(ApiSuccess {
        success: true,
        message: Some(format!("Disconnected from {}", ssid)),
    })
}

/// GET /api/network/diagnostics - Run network diagnostics
///
/// Involves several `ping`/`ip`/DNS lookups — genuinely blocking and slow
/// enough (network round trips, not just local subprocess spawns) that it
/// must never run directly on a Tokio worker thread.
pub async fn handle_network_diagnostics(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let result = match tokio::task::spawn_blocking(net::run_diagnostics).await {
        Ok(result) => result,
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    ResponseHelper::json_success(DiagnosticsResponse {
        success: result.success,
        gateway_reachable: result.gateway_reachable,
        dns_working: result.dns_working,
        internet_reachable: result.internet_reachable,
        latency_ms: result.latency_ms,
        message: result.message,
    })
}

/// GET /api/network/summary - Get connection summary
///
/// Also polled every 10s alongside `handle_network_interfaces` while the
/// Network page is open — same rationale for `spawn_blocking` as above.
pub async fn handle_network_summary(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let summary = match tokio::task::spawn_blocking(net::get_connection_summary).await {
        Ok(summary) => summary,
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    ResponseHelper::json_success(ConnectionSummaryResponse {
        local_ip: summary.local_ip,
        gateway: summary.gateway,
        dns_servers: summary.dns_servers,
        public_ip: summary.public_ip,
        primary_interface: summary.primary_interface,
        connection_type: summary.connection_type,
    })
}

/// GET /api/network/services - Get status of network services (SMB, NFS, FTP, WebDAV, SSH)
pub async fn handle_network_services(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let services = match tokio::task::spawn_blocking(ProcParser::get_network_services).await {
        Ok(services) => services,
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    ResponseHelper::json_success(serde_json::json!({ "services": services }))
}

/// GET /api/network/public-ip - Get public IP address
pub async fn handle_public_ip(State(state): State<AppStateArc>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let ip = net::discover_public_ip().await.map(|addr| addr.to_string());
    ResponseHelper::json_success(PublicIpResponse { ip })
}

// ---------------------------------------------------------------------------
// Link aggregation and VLAN devices (B5 + B6)
//
// Both features create a virtual interface and then hand it to the **same**
// configuration path a physical NIC uses, so an address on a bond is applied,
// verified and rolled back exactly like an address on `eth0`. The one thing
// they add is the session-safety refusal: a change that would reconfigure the
// interface carrying the operator's own address is rejected before a single
// `ip` command runs, because no verify/rollback step can report a change that
// cut the connection.
// ---------------------------------------------------------------------------

/// GET /api/network/bonds — the host's bonds and its candidate members.
pub async fn handle_network_bonds_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    match tokio::task::spawn_blocking(|| {
        // Members can only come from physical NICs, presented so the form
        // never offers an impossible choice (a Docker bridge is an Ethernet
        // device to the kernel but has no cable behind it).
        (net::list_bonds(), net::bond_member_candidates())
    })
    .await
    {
        Ok((bonds, candidates)) => ResponseHelper::json_success(serde_json::json!({
            "bonds": bonds,
            "candidates": candidates,
            "modes": [
                {
                    "id": net::BondMode::Lacp.kernel_name(),
                    "needs_switch_configuration": net::BondMode::Lacp.needs_switch_configuration(),
                },
                {
                    "id": net::BondMode::ActiveBackup.kernel_name(),
                    "needs_switch_configuration": net::BondMode::ActiveBackup.needs_switch_configuration(),
                },
            ],
        })),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct BondCreateRequest {
    /// Device name to create (`bond0`). Optional: the first free `bondN` is
    /// used when the client does not name one, so the common case has nothing
    /// to get wrong.
    #[serde(default)]
    pub name: Option<String>,
    /// `802.3ad` or `active-backup`.
    pub mode: String,
    pub members: Vec<String>,
}

/// POST /api/network/bonds — create a bond, or add members to a live one.
pub async fn handle_network_bond_create(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    Json(req): Json<BondCreateRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let peer_ip = peer.0;
    let outcome =
        tokio::task::spawn_blocking(move || -> Result<(String, bool, Vec<String>), String> {
            let interfaces = net::list_interfaces();
            let existing = net::list_bonds();
            let name = match req.name.clone() {
                Some(name) => name,
                None => {
                    let mut index = 0u8;
                    loop {
                        let candidate = format!("bond{}", index);
                        if !interfaces.iter().any(|i| i.name == candidate) {
                            break candidate;
                        }
                        if index == 99 {
                            return Err("No free bond name is left (bond0…bond99)".to_string());
                        }
                        index += 1;
                    }
                }
            };
            let spec = net::validate_bond_spec(&name, &req.mode, &req.members, &interfaces)?;

            // Session safety: every member is about to change role. If the
            // operator is talking to us through one of them, detaching it would
            // cut the request's own reply path.
            let already_a_bond = existing.iter().any(|b| b.name == spec.name);
            if !already_a_bond {
                net::refuse_if_carries_session_address(peer_ip, &spec.members)?;
            }

            let attached = net::create_bond_device(&spec)?;

            let kind = net::VirtualDeviceKind::Bond {
                mode: spec.mode,
                members: existing
                    .iter()
                    .find(|b| b.name == spec.name)
                    .map(|b| b.members.clone())
                    .unwrap_or_else(|| spec.members.clone()),
            };
            let persisted = match net::persist_virtual_device(&spec.name, &kind) {
                Ok(change) => {
                    // Apply + verify the file we just wrote; a failure rolls the
                    // netplan half back and reports it instead of pretending the
                    // device survives a reboot.
                    if let Err(e) = net::netplan_apply_and_verify(&spec.name) {
                        return Err(e);
                    }
                    change.backend
                }
                Err(e) => {
                    tracing::warn!("bond persistence for '{}' failed: {}", spec.name, e);
                    "none"
                }
            };
            Ok((spec.name, persisted != "none", attached))
        })
        .await;

    let (name, persisted, attached) = match outcome {
        Ok(Ok(value)) => value,
        Ok(Err(e)) => return ResponseHelper::json_error(e),
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {}", e)),
    };
    crate::events::log_event(format!(
        "Admin '{}' created or extended the bond '{}' (added: {})",
        admin_username,
        name,
        if attached.is_empty() {
            "nothing".to_string()
        } else {
            attached.join(", ")
        }
    ));
    ResponseHelper::json_success(ApiSuccess {
        success: true,
        message: Some(if persisted {
            format!("Bond {} created (persisted)", name)
        } else {
            format!(
                "Bond {} created (temporary — persistence unavailable)",
                name
            )
        }),
    })
}

/// DELETE /api/network/bonds/{name} — delete a bond, handing its NICs back.
pub async fn handle_network_bond_delete(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    Path(name): Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let peer_ip = peer.0;
    let to_delete = name.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let bond = net::list_bonds()
            .into_iter()
            .find(|b| b.name == to_delete)
            .ok_or_else(|| format!("No bond named '{}' exists", to_delete))?;
        net::refuse_if_carries_session_address(peer_ip, &[bond.name.clone()])?;
        net::delete_virtual_device(&to_delete)
    })
    .await;
    match outcome {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{}' deleted the bond '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(ApiSuccess {
                success: true,
                message: Some(format!("Bond {} deleted", name)),
            })
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// GET /api/network/vlans — the host's VLAN devices and the parents they may
/// be created on.
pub async fn handle_network_vlans_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    match tokio::task::spawn_blocking(|| (net::list_vlans(), net::vlan_parent_candidates())).await {
        Ok((vlans, parents)) => ResponseHelper::json_success(serde_json::json!({
            "vlans": vlans,
            "parents": parents,
            "max_vid": net::MAX_VLAN_ID,
        })),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct VlanCreateRequest {
    pub parent: String,
    pub vid: u16,
}

/// POST /api/network/vlans — create `<parent>.<vid>`.
pub async fn handle_network_vlan_create(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    Json(req): Json<VlanCreateRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    // Creating a VLAN on the interface that carries the caller's own address
    // is the one case where `netplan apply` can move the reply path: a
    // `dhcp4: true` VLAN on a parent that is re-rendered by netplan may take
    // a different lease than the session is using. Refused up front, like the
    // bond path.
    let peer_ip = peer.0;
    let outcome = tokio::task::spawn_blocking(move || -> Result<(String, bool), String> {
        let interfaces = net::list_interfaces();
        let spec = net::validate_vlan_spec(&req.parent, req.vid, &interfaces)?;
        net::refuse_if_carries_session_address(peer_ip, &[spec.parent.clone()])?;
        net::create_vlan_device(&spec)?;
        let persisted = match net::persist_virtual_device(
            &spec.name,
            &net::VirtualDeviceKind::Vlan {
                parent: spec.parent.clone(),
                vid: spec.vid,
            },
        ) {
            Ok(change) => {
                net::netplan_apply_and_verify(&spec.name)?;
                change.backend != "none"
            }
            Err(e) => {
                tracing::warn!("VLAN persistence for '{}' failed: {}", spec.name, e);
                false
            }
        };
        Ok((spec.name, persisted))
    })
    .await;
    match outcome {
        Ok(Ok((name, persisted))) => {
            crate::events::log_event(format!(
                "Admin '{}' created the VLAN interface '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(ApiSuccess {
                success: true,
                message: Some(if persisted {
                    format!("VLAN {} created (persisted)", name)
                } else {
                    format!(
                        "VLAN {} created (temporary — persistence unavailable)",
                        name
                    )
                }),
            })
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

/// DELETE /api/network/vlans/{name}
pub async fn handle_network_vlan_delete(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    Path(name): Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let peer_ip = peer.0;
    let to_delete = name.clone();
    let outcome = tokio::task::spawn_blocking(move || -> Result<(), String> {
        if !net::list_vlans().iter().any(|v| v.name == to_delete) {
            return Err(format!("No VLAN interface named '{}' exists", to_delete));
        }
        net::refuse_if_carries_session_address(peer_ip, &[to_delete.clone()])?;
        net::delete_virtual_device(&to_delete)
    })
    .await;
    match outcome {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{}' deleted the VLAN interface '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(ApiSuccess {
                success: true,
                message: Some(format!("VLAN {} deleted", name)),
            })
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Internal error: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Router helper
// ---------------------------------------------------------------------------

pub fn create_network_router(state: AppStateArc) -> Router {
    Router::new()
        // Interface listing
        .route("/interfaces", axum::routing::get(handle_network_interfaces))
        // Per-interface details
        .route(
            "/interfaces/{name}",
            axum::routing::get(handle_network_interface_get),
        )
        // Per-interface config (get + update)
        .route(
            "/interfaces/{name}/config",
            axum::routing::get(handle_network_interface_config),
        )
        .route(
            "/interfaces/{name}/config",
            axum::routing::put(handle_network_interface_config_update),
        )
        // Per-interface state (up/down)
        .route(
            "/interfaces/{name}/state",
            axum::routing::put(handle_network_interface_state),
        )
        // WiFi — POST because triggering a scan is a mutation-like operation
        .route("/wifi/scan", axum::routing::post(handle_network_wifi_scan))
        .route(
            "/wifi/status",
            axum::routing::get(handle_network_wifi_status),
        )
        .route(
            "/wifi/connect",
            axum::routing::post(handle_network_wifi_connect),
        )
        .route(
            "/wifi/disconnect",
            axum::routing::post(handle_network_wifi_disconnect),
        )
        // Diagnostics & Summary
        .route(
            "/diagnostics",
            axum::routing::get(handle_network_diagnostics),
        )
        // Link aggregation and VLAN devices (B5 + B6). Reads need a session;
        // every mutation is admin-only — they bring links up and down.
        .route(
            "/bonds",
            axum::routing::get(handle_network_bonds_list).post(handle_network_bond_create),
        )
        .route(
            "/bonds/{name}",
            axum::routing::delete(handle_network_bond_delete),
        )
        .route(
            "/vlans",
            axum::routing::get(handle_network_vlans_list).post(handle_network_vlan_create),
        )
        .route(
            "/vlans/{name}",
            axum::routing::delete(handle_network_vlan_delete),
        )
        .route("/summary", axum::routing::get(handle_network_summary))
        .route("/services", axum::routing::get(handle_network_services))
        .route("/public-ip", axum::routing::get(handle_public_ip))
        // Per-service bandwidth control (P3.2). Reads need a session; every
        // mutation is admin-only because the policy drives root-owned `tc`.
        .route(
            "/traffic-control",
            axum::routing::get(crate::handlers::traffic_control::handle_traffic_control_get)
                .put(crate::handlers::traffic_control::handle_traffic_control_put),
        )
        .route(
            "/traffic-control/pause",
            axum::routing::post(crate::handlers::traffic_control::handle_traffic_control_pause),
        )
        .route(
            "/traffic-control/apply",
            axum::routing::post(crate::handlers::traffic_control::handle_traffic_control_apply),
        )
        .with_state(state)
}
