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

    if let Err(e) = net::apply_interface_config(&name, &cfg) {
        return ResponseHelper::json_error(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' updated network configuration for interface '{}' (method: {})",
        admin_username, name, config.method
    ));
    ResponseHelper::json_success(ApiSuccess {
        success: true,
        message: Some(format!("Configuration applied to {}", name)),
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
        .route("/summary", axum::routing::get(handle_network_summary))
        .route("/services", axum::routing::get(handle_network_services))
        .route("/public-ip", axum::routing::get(handle_public_ip))
        .with_state(state)
}
