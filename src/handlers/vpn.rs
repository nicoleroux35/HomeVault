/* =============================================================================
 * File          : vpn.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   VPN endpoints for WireGuard and OpenVPN: server status and capability
 *   detection, WireGuard peer management (create with client config
 *   download, list, revoke), OpenVPN profile creation and start/stop, and
 *   connection listing. Every endpoint is admin-only; peer metadata is
 *   stored via VpnPeerRepository, tunnels driven by crate::vpn.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - uuid (>= 1.0)
 *
 * Usage:
 *   Routes /api/vpn in src/web/mod.rs.
 * =============================================================================
 */

//! VPN endpoints for WireGuard and OpenVPN (status, peers, profiles) —
//! all admin-only.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::handlers::AppStateArc;
use crate::storage::vpn_peer_repo::{VpnPeerRepository, VpnWireGuardPeer};
use crate::vpn as vpn_mod;

// Re-export VPN types for convenience
pub use crate::vpn::{OpenVPNConfig, VpnCapabilities, VpnConnection, VpnStatus, VpnType};

// ---------------------------------------------------------------------------
// API Types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct VpnListResponse {
    pub connections: Vec<VpnConnection>,
}

#[derive(Debug, Serialize)]
pub struct VpnConnectionResponse {
    pub connection: VpnConnection,
}

#[derive(Debug, Serialize)]
pub struct VpnCapabilitiesResponse {
    pub capabilities: VpnCapabilities,
}

#[derive(Debug, Serialize)]
pub struct ApiSuccess {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub success: bool,
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct WireGuardPeerCreateRequest {
    /// The only thing the admin has to supply — everything else (keys,
    /// address, port, endpoint) is generated/detected automatically. See
    /// `src/vpn.rs`'s WireGuard section doc comment for the full design.
    pub name: String,
}

/// One peer as shown in the list view: DB-persisted identity/name joined
/// with its live connection status from the kernel.
#[derive(Debug, Serialize)]
pub struct WireGuardPeerView {
    pub id: String,
    pub name: String,
    pub peer_ip: String,
    pub created_at: String,
    pub connected: bool,
    /// Unix epoch seconds, `None` if this peer has never completed a
    /// handshake.
    pub latest_handshake: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct OpenVPNConfigRequest {
    pub name: String,
    pub config_content: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

type HandlerResult = Result<Json<ApiSuccess>, (axum::http::StatusCode, Json<ApiError>)>;

// ---------------------------------------------------------------------------
// Auth & validation helpers
// ---------------------------------------------------------------------------

/// Require a valid, authenticated **admin** session. Returns the username on
/// success, or an HTTP 401/403 error response otherwise.
///
/// Every VPN operation is privileged (it writes to `/etc` and launches
/// network interfaces as root), so the whole surface is admin-only.
async fn require_admin(
    state: &AppStateArc,
    headers: &HeaderMap,
) -> Result<String, (axum::http::StatusCode, Json<ApiError>)> {
    let token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let username = match state.auth.get_username_from_token(token).await {
        Some(u) => u,
        None => {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                Json(ApiError {
                    success: false,
                    error: "No session token provided".to_string(),
                }),
            ))
        }
    };

    if !state.auth.is_admin(&username).await {
        tracing::warn!("Non-admin user '{}' denied access to VPN API", username);
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            Json(ApiError {
                success: false,
                error: "Admin privileges required for VPN management".to_string(),
            }),
        ));
    }

    Ok(username)
}

/// Validate and normalize a VPN connection name. Rejects anything that could be
/// used for path traversal (`/`, `..`, spaces, etc.) since the name is later
/// interpolated into filesystem paths such as `/etc/wireguard/{name}.conf`.
fn sanitize_name(name: &str) -> Result<String, (axum::http::StatusCode, Json<ApiError>)> {
    if name.is_empty() || name.len() > 64 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ApiError {
                success: false,
                error: "Invalid connection name (must be 1-64 characters)".to_string(),
            }),
        ));
    }

    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ApiError {
                success: false,
                error: "Connection name may only contain letters, numbers, '_' and '-'".to_string(),
            }),
        ));
    }

    Ok(name.to_string())
}

/// Validate a peer's display name. Unlike `sanitize_name` (still used for
/// OpenVPN, where the name becomes a filename), a peer name is only ever
/// stored in the database and shown in the UI — so ordinary display text
/// ("John's iPhone", "Salon TV") is fine, only empty/oversized/control-
/// character input is rejected.
fn sanitize_peer_name(name: &str) -> Result<String, (axum::http::StatusCode, Json<ApiError>)> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return Err(err(
            axum::http::StatusCode::BAD_REQUEST,
            "Device name must be 1-64 characters".to_string(),
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(err(
            axum::http::StatusCode::BAD_REQUEST,
            "Device name contains invalid characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn ok(message: String) -> Json<ApiSuccess> {
    Json(ApiSuccess {
        success: true,
        message: Some(message),
        data: None,
    })
}

fn err(
    status: axum::http::StatusCode,
    message: String,
) -> (axum::http::StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            success: false,
            error: message,
        }),
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/vpn/capabilities - Get VPN capabilities
///
/// Returns the same `{success, data}` envelope every other handler in this
/// file (and the app's generic `api.ts` client) expects. This used to return
/// a bare `{"capabilities": ...}` struct with no `success` field, which the
/// frontend's `response.success && response.data` check always read as a
/// failure — the VPN page could never detect WireGuard/OpenVPN were
/// installed, regardless of the real host state.
pub async fn handle_vpn_capabilities(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> HandlerResult {
    let _username = require_admin(&state, &headers).await?;
    let capabilities = vpn_mod::get_vpn_capabilities();
    Ok(Json(ApiSuccess {
        success: true,
        message: None,
        data: Some(serde_json::json!({ "capabilities": capabilities })),
    }))
}

/// GET /api/vpn/connections - List all VPN connections
/// See `handle_vpn_capabilities` for why this uses the `ApiSuccess` envelope.
pub async fn handle_vpn_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> HandlerResult {
    let _username = require_admin(&state, &headers).await?;
    let connections = vpn_mod::list_all_vpn_connections();
    Ok(Json(ApiSuccess {
        success: true,
        message: None,
        data: Some(serde_json::json!({ "connections": connections })),
    }))
}

/// GET /api/vpn/connections/:id - Get specific VPN connection
/// See `handle_vpn_capabilities` for why this uses the `ApiSuccess` envelope.
pub async fn handle_vpn_get(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> HandlerResult {
    let _username = require_admin(&state, &headers).await?;

    let id =
        sanitize_name(&id).map_err(|e| err(axum::http::StatusCode::BAD_REQUEST, e.1 .0.error))?;

    let connection = vpn_mod::get_vpn_connection(&id).ok_or_else(|| {
        err(
            axum::http::StatusCode::NOT_FOUND,
            format!("VPN connection '{}' not found", id),
        )
    })?;

    Ok(Json(ApiSuccess {
        success: true,
        message: None,
        data: Some(serde_json::json!({ "connection": connection })),
    }))
}

/// GET /api/vpn/wireguard/server - WireGuard server status (public key,
/// port, detected public endpoint). Provisions the server on first call if
/// it doesn't exist yet, so the status card has something real to show
/// even before any peer has been created.
pub async fn handle_wg_server_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> HandlerResult {
    let _username = require_admin(&state, &headers).await?;

    let server = vpn_mod::ensure_wireguard_server()
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(ApiSuccess {
        success: true,
        message: None,
        data: Some(serde_json::json!({ "server": server })),
    }))
}

/// GET /api/vpn/wireguard/peers - List provisioned peers with live status.
pub async fn handle_wg_peers_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> HandlerResult {
    let _username = require_admin(&state, &headers).await?;

    let repo = VpnPeerRepository::new(state.db.pool());
    let peers = repo
        .list()
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let live = vpn_mod::list_wireguard_peer_live_status();
    let now = chrono::Utc::now().timestamp() as u64;

    let views: Vec<WireGuardPeerView> = peers
        .into_iter()
        .map(|p| {
            let status = live.iter().find(|l| l.public_key == p.public_key);
            let latest_handshake = status.map(|s| s.latest_handshake).filter(|&t| t > 0);
            // WireGuard peers re-handshake at least every 3 minutes while
            // active; anything more stale than that is no longer connected.
            let connected = latest_handshake.is_some_and(|t| now.saturating_sub(t) < 180);
            WireGuardPeerView {
                id: p.id,
                name: p.name,
                peer_ip: p.peer_ip,
                created_at: p.created_at,
                connected,
                latest_handshake,
            }
        })
        .collect();

    Ok(Json(ApiSuccess {
        success: true,
        message: None,
        data: Some(serde_json::json!({ "peers": views })),
    }))
}

/// POST /api/vpn/wireguard/peers - Provision a new peer. The only input is
/// a display name; the response's `client_config`/`qr_payload` is the one
/// and only time the peer's private key is ever shown — it is never
/// persisted server-side (see `sqlx/migrations/025_vpn_wireguard_peers.sql`).
pub async fn handle_wg_peer_create(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(request): Json<WireGuardPeerCreateRequest>,
) -> HandlerResult {
    let username = require_admin(&state, &headers).await?;
    let name = sanitize_peer_name(&request.name)?;

    let repo = VpnPeerRepository::new(state.db.pool());
    let existing = repo
        .list()
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let used_ips: Vec<String> = existing.iter().map(|p| p.peer_ip.clone()).collect();
    let peer_ip = vpn_mod::next_available_peer_ip(&used_ips)
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let provisioned = vpn_mod::create_wireguard_peer(&peer_ip)
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let peer = VpnWireGuardPeer {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        public_key: provisioned.public_key.clone(),
        private_key: provisioned.private_key.clone(),
        preshared_key: provisioned.preshared_key.clone(),
        peer_ip: provisioned.peer_ip.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        created_by: username.clone(),
    };
    if let Err(e) = repo.insert(&peer).await {
        // The peer is already live on the interface at this point; roll
        // that back rather than leaving an unregistered, unrevokable peer
        // attached to the server.
        let _ = vpn_mod::revoke_wireguard_peer(&provisioned.public_key);
        return Err(err(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        ));
    }

    crate::events::log_event(format!(
        "Admin '{}' added WireGuard device '{}' ({})",
        username, name, peer.peer_ip
    ));

    Ok(Json(ApiSuccess {
        success: true,
        message: Some(format!("Device '{}' added", name)),
        data: Some(serde_json::json!({
            "id": peer.id,
            "name": peer.name,
            "peer_ip": peer.peer_ip,
            "client_config": provisioned.client_config,
        })),
    }))
}

/// GET /api/vpn/wireguard/peers/{id}/config - Re-display a peer's client
/// config (QR code / download). The keys were stored at creation time so
/// the config can be shown again even after the initial setup modal closes.
pub async fn handle_wg_peer_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> HandlerResult {
    let _username = require_admin(&state, &headers).await?;

    let repo = VpnPeerRepository::new(state.db.pool());
    let peer = repo
        .find_by_id(&id)
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            err(
                axum::http::StatusCode::NOT_FOUND,
                "Device not found".to_string(),
            )
        })?;

    // Regenerate the full client config from stored keys + live server status.
    let client_config =
        vpn_mod::get_wireguard_peer_config(&peer.peer_ip, &peer.private_key, &peer.preshared_key)
            .await
            .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(ApiSuccess {
        success: true,
        message: None,
        data: Some(serde_json::json!({
            "id": peer.id,
            "name": peer.name,
            "peer_ip": peer.peer_ip,
            "client_config": client_config,
        })),
    }))
}

/// DELETE /api/vpn/wireguard/peers/:id - Revoke a peer's access immediately.
pub async fn handle_wg_peer_revoke(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> HandlerResult {
    let username = require_admin(&state, &headers).await?;

    let repo = VpnPeerRepository::new(state.db.pool());
    let peer = repo
        .find_by_id(&id)
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            err(
                axum::http::StatusCode::NOT_FOUND,
                "Device not found".to_string(),
            )
        })?;

    vpn_mod::revoke_wireguard_peer(&peer.public_key)
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    repo.delete(&id)
        .await
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    crate::events::log_event_warn(format!(
        "Admin '{}' revoked WireGuard device '{}'",
        username, peer.name
    ));
    Ok(ok(format!("Device '{}' revoked", peer.name)))
}

/// POST /api/vpn/openvpn - Create OpenVPN connection
pub async fn handle_openvpn_create(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(request): Json<OpenVPNConfigRequest>,
) -> HandlerResult {
    let username = require_admin(&state, &headers).await?;
    let name = sanitize_name(&request.name)
        .map_err(|e| err(axum::http::StatusCode::BAD_REQUEST, e.1 .0.error))?;

    let config_path = vpn_mod::save_openvpn_config(&name, &request.config_content)
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    crate::events::log_event(format!(
        "Admin '{}' created OpenVPN connection '{}'",
        username, name
    ));
    Ok(Json(ApiSuccess {
        success: true,
        message: Some(format!("OpenVPN connection '{}' created", name)),
        data: Some(serde_json::json!({
            "config_path": config_path,
        })),
    }))
}

/// POST /api/vpn/openvpn/:name/start - Start OpenVPN connection
pub async fn handle_openvpn_start(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> HandlerResult {
    let username = require_admin(&state, &headers).await?;
    let name =
        sanitize_name(&name).map_err(|e| err(axum::http::StatusCode::BAD_REQUEST, e.1 .0.error))?;

    let config_file = format!("/etc/openvpn/{}.ovpn", name);

    let auth_user_pass = if let (Some(user), Some(pass)) = (
        payload.get("username").and_then(|v| v.as_str()),
        payload.get("password").and_then(|v| v.as_str()),
    ) {
        Some((user.to_string(), pass.to_string()))
    } else {
        None
    };

    let config = vpn_mod::OpenVPNConfig {
        name: name.clone(),
        config_file,
        auth_user_pass,
    };

    vpn_mod::start_openvpn_connection(&config)
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    crate::events::log_event(format!(
        "Admin '{}' started OpenVPN connection '{}'",
        username, name
    ));
    Ok(ok(format!("OpenVPN connection '{}' started", name)))
}

/// POST /api/vpn/openvpn/:name/stop - Stop OpenVPN connection
pub async fn handle_openvpn_stop(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> HandlerResult {
    let username = require_admin(&state, &headers).await?;
    let name =
        sanitize_name(&name).map_err(|e| err(axum::http::StatusCode::BAD_REQUEST, e.1 .0.error))?;

    vpn_mod::stop_openvpn_connection(&name)
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    crate::events::log_event(format!(
        "Admin '{}' stopped OpenVPN connection '{}'",
        username, name
    ));
    Ok(ok(format!("OpenVPN connection '{}' stopped", name)))
}

/// DELETE /api/vpn/openvpn/:name - Delete OpenVPN connection
pub async fn handle_openvpn_delete(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> HandlerResult {
    let username = require_admin(&state, &headers).await?;
    let name =
        sanitize_name(&name).map_err(|e| err(axum::http::StatusCode::BAD_REQUEST, e.1 .0.error))?;

    vpn_mod::delete_openvpn_connection(&name)
        .map_err(|e| err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    crate::events::log_event_warn(format!(
        "Admin '{}' deleted OpenVPN connection '{}'",
        username, name
    ));
    Ok(ok(format!("OpenVPN connection '{}' deleted", name)))
}

// ---------------------------------------------------------------------------
// Router helper
// ---------------------------------------------------------------------------

pub fn create_vpn_router(state: AppStateArc) -> Router {
    Router::new()
        // Capabilities
        .route("/capabilities", axum::routing::get(handle_vpn_capabilities))
        // List connections
        .route("/connections", axum::routing::get(handle_vpn_list))
        .route("/connections/{id}", axum::routing::get(handle_vpn_get))
        // WireGuard (server + provisioned peers)
        .route(
            "/wireguard/server",
            axum::routing::get(handle_wg_server_status),
        )
        .route("/wireguard/peers", axum::routing::get(handle_wg_peers_list))
        .route(
            "/wireguard/peers",
            axum::routing::post(handle_wg_peer_create),
        )
        .route(
            "/wireguard/peers/{id}",
            axum::routing::get(handle_wg_peer_config),
        )
        .route(
            "/wireguard/peers/{id}",
            axum::routing::delete(handle_wg_peer_revoke),
        )
        // OpenVPN
        .route("/openvpn", axum::routing::post(handle_openvpn_create))
        .route(
            "/openvpn/{name}/start",
            axum::routing::post(handle_openvpn_start),
        )
        .route(
            "/openvpn/{name}/stop",
            axum::routing::post(handle_openvpn_stop),
        )
        .route(
            "/openvpn/{name}",
            axum::routing::delete(handle_openvpn_delete),
        )
        .with_state(state)
}
