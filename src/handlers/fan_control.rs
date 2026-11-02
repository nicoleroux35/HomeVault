use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::fan_control::{FanChannel, FanConfig, FanControlService};

/// GET /api/system/fans
pub async fn handle_channels_list(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<FanChannelsResponse>, (StatusCode, String)> {
    let is_laptop = FanControlService::is_laptop().await;

    if is_laptop {
        return Ok(Json(FanChannelsResponse {
            channels: Vec::new(),
            is_laptop: true,
            message: Some("Fan control is disabled on laptops".to_string()),
        }));
    }

    let channels = FanControlService::detect_channels()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(FanChannelsResponse {
        channels,
        is_laptop: false,
        message: None,
    }))
}

/// GET /api/system/fans/config
pub async fn handle_config_get(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<FanConfig>, (StatusCode, String)> {
    let config = load_config().await.unwrap_or_default();
    Ok(Json(config))
}

/// PUT /api/system/fans/config
pub async fn handle_config_update(
    State(_state): State<Arc<AppState>>,
    Json(config): Json<FanConfig>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Check if laptop
    if FanControlService::is_laptop().await {
        return Err((
            StatusCode::BAD_REQUEST,
            "Fan control is disabled on laptops".to_string(),
        ));
    }

    // Apply config
    FanControlService::apply_config(&config)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Save config
    save_config(&config).await?;

    Ok(StatusCode::OK)
}

/// POST /api/system/fans/restore-auto
pub async fn handle_restore_auto(
    State(_state): State<Arc<AppState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    FanControlService::restore_auto()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(StatusCode::OK)
}

// Response types

#[derive(Debug, Serialize)]
pub struct FanChannelsResponse {
    pub channels: Vec<FanChannel>,
    pub is_laptop: bool,
    pub message: Option<String>,
}

// Helper functions

async fn load_config() -> Result<FanConfig, (StatusCode, String)> {
    let path = config_path();

    if !path.exists() {
        return Ok(FanConfig::default());
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

async fn save_config(config: &FanConfig) -> Result<(), (StatusCode, String)> {
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
    std::path::PathBuf::from(data_dir).join("data/fan_control.json")
}
