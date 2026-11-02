/* =============================================================================
 * File          : connected_clients.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   "Who is connected" endpoints: list the SMB/NFS/SFTP/FTP clients of this
 *   box (with the provenance of each source) and disconnect one. Admin-only:
 *   the list names every peer and every share on the appliance.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::connected_clients
 *
 * Usage:
 *   Routes /api/security/connected-clients* in src/web/mod.rs; see
 *   docs/guides/security.md.
 * =============================================================================
 */

//! Connected-client endpoints (admin-only), see docs/guides/security.md.
//!
//! * `GET  /api/security/connected-clients`      — the list + per-source status
//! * `POST /api/security/connected-clients/kick` — disconnect one client

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};
use serde::Deserialize;

use crate::connected_clients::{self, KickError};
use crate::handlers::admin_guard::require_admin;
use crate::utils::ResponseHelper;

use super::AppState;

/// `GET /api/security/connected-clients`
///
/// Always `200`: a box with no Samba installed reports the source as
/// unavailable and shows whatever the other protocols have. An empty list is a
/// fact, not an error.
pub async fn handle_connected_clients_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let collected = connected_clients::collect().await;
    ResponseHelper::json_success(serde_json::json!({
        "clients": collected.clients,
        "sources": collected.sources,
        "kickable": collected.kickable(),
        "checked_at": chrono::Utc::now().to_rfc3339(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct KickRequest {
    /// The client id from the last listing (`smb:<pid>:<ip>`, …).
    pub id: String,
}

/// `POST /api/security/connected-clients/kick`
///
/// The body names a client, never a pid: `connected_clients::kick` re-collects
/// and takes the pid from the fresh observation, so a stale or forged id cannot
/// signal an unrelated process.
pub async fn handle_connected_client_kick(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<KickRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let id = body.id.trim();
    if id.is_empty() {
        return ResponseHelper::json_error("A client id is required".to_string());
    }
    match connected_clients::kick(id).await {
        Ok(detail) => {
            crate::events::log_event(format!(
                "Admin '{}' disconnected a client: {}",
                username, detail
            ));
            ResponseHelper::json_success(serde_json::json!({
                "disconnected": true,
                "detail": detail,
            }))
        }
        Err(KickError::Unknown(id)) => ResponseHelper::json_error(KickError::Unknown(id).message()),
        Err(KickError::NotKickable(protocol)) => {
            ResponseHelper::json_error(KickError::NotKickable(protocol).message())
        }
        Err(KickError::Failed(reason)) => {
            ResponseHelper::json_error(format!("Could not disconnect the client: {reason}"))
        }
    }
}
