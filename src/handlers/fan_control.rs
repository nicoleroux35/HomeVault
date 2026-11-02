/* =============================================================================
 * File          : fan_control.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Fan and thermal endpoints: read the detected hwmon channels and the saved
 *   profile, save a new profile (which writes the PWM registers), and hand
 *   control back to the firmware.
 *
 *   Reads need a session; every mutation is admin-only. Writing a PWM value on
 *   a machine with no thermal headroom can cook the disks, so an
 *   unauthenticated caller must never reach it.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::fan_control
 *
 * Usage:
 *   Routes under /api/system/fans in src/web/mod.rs; see
 *   docs/guides/fan-control.md.
 * =============================================================================
 */

//! Fan and thermal endpoints, see docs/guides/fan-control.md.
//!
//! * `GET  /api/system/fans`              — detected channels + laptop flag
//! * `GET  /api/system/fans/config`       — saved profile
//! * `PUT  /api/system/fans/config`       — save and apply a profile
//! * `POST /api/system/fans/restore-auto` — hand control back to the firmware

use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response};
use serde::Serialize;
use std::sync::Arc;

use super::AppState;
use crate::fan_control::{FanChannel, FanConfig, FanControlService};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::utils::ResponseHelper;

/// Re-wraps a `(status, message)` pair from a helper as the JSON error
/// envelope shared by the rest of the API.
fn fail(err: (axum::http::StatusCode, String)) -> Response {
    ResponseHelper::json_error_with_status(err.0, err.1)
}

/// GET /api/system/fans — the channels this host exposes.
pub async fn handle_channels_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    if FanControlService::is_laptop().await {
        return ResponseHelper::json_success(FanChannelsResponse {
            channels: Vec::new(),
            is_laptop: true,
            message: Some("Fan control is disabled on laptops".to_string()),
        });
    }

    match FanControlService::detect_channels().await {
        Ok(channels) => ResponseHelper::json_success(FanChannelsResponse {
            channels,
            is_laptop: false,
            message: None,
        }),
        Err(e) => ResponseHelper::json_internal_error(e),
    }
}

/// GET /api/system/fans/config
pub async fn handle_config_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    match load_config().await {
        Ok(config) => ResponseHelper::json_success(config),
        Err(e) => fail(e),
    }
}

/// PUT /api/system/fans/config — save and apply.
pub async fn handle_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let config: FanConfig = match serde_json::from_slice(&body) {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    if FanControlService::is_laptop().await {
        return ResponseHelper::json_error("Fan control is disabled on laptops".to_string());
    }

    // Apply first, save second: a config the host refused must not become the
    // one a restart would re-apply.
    if let Err(e) = FanControlService::apply_config(&config).await {
        return ResponseHelper::json_internal_error(e);
    }

    if let Err(e) = save_config(&config).await {
        return fail(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' applied the {:?} fan profile",
        admin, config.profile
    ));
    ResponseHelper::json_success(serde_json::json!({ "applied": true }))
}

/// POST /api/system/fans/restore-auto
pub async fn handle_restore_auto(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if FanControlService::is_laptop().await {
        return ResponseHelper::json_error("Fan control is disabled on laptops".to_string());
    }

    if let Err(e) = FanControlService::restore_auto().await {
        return ResponseHelper::json_internal_error(e);
    }

    // The saved profile must follow the host: leaving "Performance" on disk
    // after handing control back to the firmware would re-apply it on reboot.
    let mut config = load_config().await.unwrap_or_default();
    config.profile = crate::fan_control::FanProfile::Auto;
    config.channel_overrides.clear();
    if let Err(e) = save_config(&config).await {
        return fail(e);
    }

    crate::events::log_event(format!("Admin '{}' restored automatic fan control", admin));
    ResponseHelper::json_success(serde_json::json!({ "restored": true }))
}

// Response types

#[derive(Debug, Serialize)]
pub struct FanChannelsResponse {
    pub channels: Vec<FanChannel>,
    pub is_laptop: bool,
    pub message: Option<String>,
}

// Helper functions

async fn load_config() -> Result<FanConfig, (axum::http::StatusCode, String)> {
    let path = config_path();

    if !path.exists() {
        return Ok(FanConfig::default());
    }

    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read config: {}", e),
        )
    })?;

    serde_json::from_str(&content).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse config: {}", e),
        )
    })
}

async fn save_config(config: &FanConfig) -> Result<(), (axum::http::StatusCode, String)> {
    let path = config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create directory: {}", e),
            )
        })?;
    }

    let content = serde_json::to_string_pretty(config).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize config: {}", e),
        )
    })?;

    tokio::fs::write(&path, content).await.map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
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
