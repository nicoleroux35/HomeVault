/* =============================================================================
 * File          : dhcp_server.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   DHCP/TFTP/PXE server endpoints: read the configuration and the live lease
 *   table, save a new configuration, start and stop the dnsmasq instance.
 *
 *   Reads need a session; every mutation is admin-only, because starting or
 *   stopping dnsmasq runs a root-owned `systemctl` command and a wrong
 *   configuration on a live LAN can hand out the wrong addresses.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::dhcp_server
 *
 * Usage:
 *   Routes under /api/network/dhcp-server in src/web/mod.rs; see
 *   docs/guides/dhcp.md.
 * =============================================================================
 */

//! DHCP/TFTP/PXE server endpoints, see docs/guides/dhcp.md.
//!
//! * `GET  /api/network/dhcp-server`         — configuration, state, capability
//! * `PUT  /api/network/dhcp-server`         — save (and apply) a configuration
//! * `POST /api/network/dhcp-server/start`   — start dnsmasq from the saved config
//! * `POST /api/network/dhcp-server/stop`    — stop dnsmasq
//! * `GET  /api/network/dhcp-server/leases`  — live lease table

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::dhcp_server::{DhcpServerConfig, DhcpServerService, DhcpServerState};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::utils::ResponseHelper;

/// Re-wraps a `(status, message)` pair from a helper as the JSON error
/// envelope the rest of the API answers with, so a failed DHCP call and a
/// refused DHCP call look the same to the client.
fn fail(err: (StatusCode, String)) -> Response {
    ResponseHelper::json_error_with_status(err.0, err.1)
}

/// GET /api/network/dhcp-server — configuration, live state and whether
/// dnsmasq is installed. Any signed-in user may look; only an admin may change.
pub async fn handle_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let config = match load_config().await {
        Ok(config) => config,
        Err(e) => return fail(e),
    };
    let available = DhcpServerService::is_available().await;
    let running = DhcpServerService::is_running().await;

    let leases = if running {
        DhcpServerService::get_leases().await.unwrap_or_default()
    } else {
        Vec::new()
    };

    let state_payload = DhcpServerState {
        running,
        leases,
        conflict_detected: false,
        last_error: None,
    };

    ResponseHelper::json_success(DhcpServerGetResponse {
        config,
        state: state_payload,
        available,
    })
}

/// PUT /api/network/dhcp-server — validate, save, then apply.
///
/// The validation and the conflict check both run before anything touches the
/// disk, so a refused configuration leaves the previous one intact.
pub async fn handle_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let req: DhcpServerUpdateRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    if let Err(e) = DhcpServerService::validate_config(&req.config) {
        return ResponseHelper::json_error(e);
    }

    // Check for conflicts if enabling DHCP
    if req.config.dhcp_enabled {
        let conflict = DhcpServerService::detect_conflict(&req.config.interface)
            .await
            .unwrap_or(false);

        if conflict {
            return ResponseHelper::json_error_with_status(
                StatusCode::CONFLICT,
                format!(
                    "DHCP server already active on interface {}",
                    req.config.interface
                ),
            );
        }
    }

    if let Err(e) = save_config(&req.config).await {
        return fail(e);
    }

    // Apply changes
    if req.config.dhcp_enabled || req.config.tftp_enabled {
        // Stop first if running
        if DhcpServerService::is_running().await {
            if let Err(e) = DhcpServerService::stop_server().await {
                return ResponseHelper::json_internal_error(e);
            }
        }

        // Start with new config
        if let Err(e) = DhcpServerService::start_server(&req.config).await {
            return ResponseHelper::json_internal_error(e);
        }
    } else {
        // Disabled - stop if running
        if DhcpServerService::is_running().await {
            if let Err(e) = DhcpServerService::stop_server().await {
                return ResponseHelper::json_internal_error(e);
            }
        }
    }

    let enabled = if req.config.dhcp_enabled {
        "enabled"
    } else {
        "disabled"
    };
    crate::events::log_event(format!(
        "Admin '{}' saved the DHCP server configuration ({})",
        admin, enabled
    ));

    ResponseHelper::json_success(serde_json::json!({ "saved": true }))
}

/// POST /api/network/dhcp-server/start
pub async fn handle_start(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let config = match load_config().await {
        Ok(config) => config,
        Err(e) => return fail(e),
    };

    if !config.dhcp_enabled && !config.tftp_enabled {
        return ResponseHelper::json_error(
            "DHCP/TFTP server is not enabled in configuration".to_string(),
        );
    }

    if let Err(e) = DhcpServerService::start_server(&config).await {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!("Admin '{}' started the DHCP server", admin));
    ResponseHelper::json_success(serde_json::json!({ "running": true }))
}

/// POST /api/network/dhcp-server/stop
pub async fn handle_stop(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if let Err(e) = DhcpServerService::stop_server().await {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!("Admin '{}' stopped the DHCP server", admin));
    ResponseHelper::json_success(serde_json::json!({ "running": false }))
}

/// GET /api/network/dhcp-server/leases — the live lease table.
pub async fn handle_leases(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    match DhcpServerService::get_leases().await {
        Ok(leases) => ResponseHelper::json_success(leases),
        Err(e) => ResponseHelper::json_internal_error(e),
    }
}

// Request/Response types

#[derive(Debug, Serialize)]
pub struct DhcpServerGetResponse {
    pub config: DhcpServerConfig,
    pub state: DhcpServerState,
    pub available: bool,
}

#[derive(Debug, Deserialize)]
pub struct DhcpServerUpdateRequest {
    pub config: DhcpServerConfig,
}

// Helper functions

async fn load_config() -> Result<DhcpServerConfig, (StatusCode, String)> {
    let path = config_path();

    if !path.exists() {
        return Ok(DhcpServerConfig::default());
    }

    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read config: {}", e),
        )
    })?;

    serde_json::from_str(&content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse config: {}", e),
        )
    })
}

async fn save_config(config: &DhcpServerConfig) -> Result<(), (StatusCode, String)> {
    let path = config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create directory: {}", e),
            )
        })?;
    }

    let content = serde_json::to_string_pretty(config).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize config: {}", e),
        )
    })?;

    tokio::fs::write(&path, content).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write config: {}", e),
        )
    })?;

    Ok(())
}

fn config_path() -> std::path::PathBuf {
    let data_dir =
        std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string());
    std::path::PathBuf::from(data_dir).join("data/dhcp_server.json")
}
