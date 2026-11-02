use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::ups_server::{
    ShutdownPolicy, UpsClient, UpsServerConfig, UpsServerService, UpsServerState,
};

/// GET /api/power/ups-server
/// Get UPS server configuration and state
pub async fn handle_ups_server_get(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<UpsServerGetResponse>, (StatusCode, String)> {
    let config = load_config().await?;
    let state = UpsServerService::get_state().await;
    let available = UpsServerService::is_available().await;

    Ok(Json(UpsServerGetResponse {
        available,
        config,
        state,
    }))
}

/// PUT /api/power/ups-server
/// Update UPS server configuration
pub async fn handle_ups_server_update(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<UpsServerUpdateRequest>,
) -> Result<Json<UpsServerUpdateResponse>, (StatusCode, String)> {
    // Validate configuration
    UpsServerService::validate_config(&req.config)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Save configuration
    save_config(&req.config)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // If enabled, restart the service
    let requires_restart = if req.config.enabled {
        if UpsServerService::is_running().await {
            UpsServerService::stop_server()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }

        UpsServerService::start_server(&req.config)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        false // Already restarted
    } else {
        // Disabled - stop if running
        if UpsServerService::is_running().await {
            UpsServerService::stop_server()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        }
        false
    };

    Ok(Json(UpsServerUpdateResponse { requires_restart }))
}

/// POST /api/power/ups-server/start
/// Start the UPS server
pub async fn handle_ups_server_start(
    State(_state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let config = load_config().await?;

    if !config.enabled {
        return Err((
            StatusCode::BAD_REQUEST,
            "UPS server is not enabled in configuration".to_string(),
        ));
    }

    UpsServerService::start_server(&config)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::OK)
}

/// POST /api/power/ups-server/stop
/// Stop the UPS server
pub async fn handle_ups_server_stop(
    State(_state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    UpsServerService::stop_server()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::OK)
}

/// POST /api/power/ups-server/shutdown
/// Execute orderly shutdown
pub async fn handle_ups_server_shutdown(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<ShutdownRequest>,
) -> Result<Json<ShutdownResponse>, (StatusCode, String)> {
    let config = load_config().await?;

    // Execute shutdown stages
    let results = UpsServerService::execute_shutdown(&config.shutdown_policy).await;

    let all_success = results.iter().all(|r| r.success);

    Ok(Json(ShutdownResponse {
        success: all_success,
        stages: results
            .into_iter()
            .map(|r| ShutdownStageResponse {
                stage: r.stage,
                success: r.success,
                duration_ms: r.duration_ms,
                error: r.error,
            })
            .collect(),
    }))
}

// Request/Response types

#[derive(Debug, Serialize)]
pub struct UpsServerGetResponse {
    pub available: bool,
    pub config: UpsServerConfig,
    pub state: UpsServerState,
}

#[derive(Debug, Deserialize)]
pub struct UpsServerUpdateRequest {
    pub config: UpsServerConfig,
}

#[derive(Debug, Serialize)]
pub struct UpsServerUpdateResponse {
    pub requires_restart: bool,
}

#[derive(Debug, Deserialize)]
pub struct ShutdownRequest {
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
pub struct ShutdownResponse {
    pub success: bool,
    pub stages: Vec<ShutdownStageResponse>,
}

#[derive(Debug, Serialize)]
pub struct ShutdownStageResponse {
    pub stage: String,
    pub success: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// Load UPS server configuration from disk
async fn load_config() -> Result<UpsServerConfig, (StatusCode, String)> {
    let path = config_path();

    if !path.exists() {
        return Ok(UpsServerConfig::default());
    }

    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read config: {}", e),
            )
        })?;

    serde_json::from_str(&content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to parse config: {}", e),
        )
    })
}

/// Save UPS server configuration to disk
async fn save_config(config: &UpsServerConfig) -> Result<(), String> {
    let path = config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create config directory: {}", e))?;
    }

    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed to serialize config: {}", e))?;

    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("failed to write config: {}", e))?;

    Ok(())
}

fn config_path() -> std::path::PathBuf {
    let data_dir = std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string());
    std::path::PathBuf::from(data_dir).join("data/ups_server.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_path_uses_data_directory() {
        std::env::set_var("DATA_DIRECTORY", "/tmp/test");
        let path = config_path();
        assert_eq!(path, std::path::PathBuf::from("/tmp/test/data/ups_server.json"));
        std::env::remove_var("DATA_DIRECTORY");
    }

    #[test]
    fn test_config_path_default() {
        std::env::remove_var("DATA_DIRECTORY");
        let path = config_path();
        assert_eq!(
            path,
            std::path::PathBuf::from("/var/lib/homevault/data/ups_server.json")
        );
    }

    #[tokio::test]
    async fn test_save_and_load_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", temp_dir.path());

        let config = UpsServerConfig {
            enabled: true,
            bind_interface: "eth0".to_string(),
            bind_address: Some("192.168.1.1".parse().unwrap()),
            port: 3493,
            allowed_clients: vec![],
            shutdown_policy: ShutdownPolicy::default(),
            max_clients: 5,
        };

        // Save
        save_config(&config).await.unwrap();

        // Load
        let loaded = load_config().await.unwrap();

        assert_eq!(loaded.enabled, config.enabled);
        assert_eq!(loaded.bind_interface, config.bind_interface);
        assert_eq!(loaded.port, config.port);
        assert_eq!(loaded.max_clients, config.max_clients);

        std::env::remove_var("DATA_DIRECTORY");
    }

    #[tokio::test]
    async fn test_load_config_returns_default_when_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", temp_dir.path());

        let loaded = load_config().await.unwrap();

        assert!(!loaded.enabled);
        assert_eq!(loaded.port, 3493);

        std::env::remove_var("DATA_DIRECTORY");
    }
}
