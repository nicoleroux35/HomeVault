use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::dhcp_server::{DhcpLease, DhcpServerConfig, DhcpServerService, DhcpServerState};

/// GET /api/network/dhcp-server
pub async fn handle_get(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<DhcpServerGetResponse>, (StatusCode, String)> {
    let config = load_config().await.unwrap_or_default();
    let available = DhcpServerService::is_available().await;
    let running = DhcpServerService::is_running().await;

    let leases = if running {
        DhcpServerService::get_leases().await.unwrap_or_default()
    } else {
        Vec::new()
    };

    let state = DhcpServerState {
        running,
        leases,
        conflict_detected: false,
        last_error: None,
    };

    Ok(Json(DhcpServerGetResponse {
        config,
        state,
        available,
    }))
}

/// PUT /api/network/dhcp-server
pub async fn handle_update(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<DhcpServerUpdateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Validate
    DhcpServerService::validate_config(&req.config)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Check for conflicts if enabling DHCP
    if req.config.dhcp_enabled {
        let conflict = DhcpServerService::detect_conflict(&req.config.interface)
            .await
            .unwrap_or(false);

        if conflict {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "DHCP server already active on interface {}",
                    req.config.interface
                ),
            ));
        }
    }

    // Save config
    save_config(&req.config).await?;

    // Apply changes
    if req.config.dhcp_enabled || req.config.tftp_enabled {
        // Stop first if running
        if DhcpServerService::is_running().await {
            DhcpServerService::stop_server()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }

        // Start with new config
        DhcpServerService::start_server(&req.config)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    } else {
        // Disabled - stop if running
        if DhcpServerService::is_running().await {
            DhcpServerService::stop_server()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }
    }

    Ok(StatusCode::OK)
}

/// POST /api/network/dhcp-server/start
pub async fn handle_start(
    State(_state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let config = load_config().await?;

    if !config.dhcp_enabled && !config.tftp_enabled {
        return Err((
            StatusCode::BAD_REQUEST,
            "DHCP/TFTP server is not enabled in configuration".to_string(),
        ));
    }

    DhcpServerService::start_server(&config)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::OK)
}

/// POST /api/network/dhcp-server/stop
pub async fn handle_stop(
    State(_state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    DhcpServerService::stop_server()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::OK)
}

/// GET /api/network/dhcp-server/leases
pub async fn handle_leases(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<DhcpLease>>, (StatusCode, String)> {
    let leases = DhcpServerService::get_leases()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(leases))
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
