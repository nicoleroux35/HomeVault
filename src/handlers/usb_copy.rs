use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::usb_copy::{CopyStatus, RemovableDevice, UsbCopyProfile, UsbCopyService};

/// GET /api/usb-copy/profiles
pub async fn handle_profiles_list(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<UsbCopyProfile>>, (StatusCode, String)> {
    let profiles = load_profiles().await?;
    Ok(Json(profiles))
}

/// POST /api/usb-copy/profiles
pub async fn handle_profile_create(
    State(_state): State<Arc<AppState>>,
    Json(profile): Json<UsbCopyProfile>,
) -> Result<Json<UsbCopyProfile>, (StatusCode, String)> {
    // Validate
    UsbCopyService::validate_profile(&profile)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Save
    save_profile(&profile).await?;

    Ok(Json(profile))
}

/// PUT /api/usb-copy/profiles/:id
pub async fn handle_profile_update(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut profile): Json<UsbCopyProfile>,
) -> Result<Json<UsbCopyProfile>, (StatusCode, String)> {
    // Ensure ID matches
    profile.id = id;

    // Validate
    UsbCopyService::validate_profile(&profile)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Update
    save_profile(&profile).await?;

    Ok(Json(profile))
}

/// DELETE /api/usb-copy/profiles/:id
pub async fn handle_profile_delete(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    delete_profile(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/usb-copy/devices
pub async fn handle_devices_list(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<Vec<RemovableDevice>>, (StatusCode, String)> {
    let mut devices = UsbCopyService::list_devices()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Load profiles to check for matches
    let profiles = load_profiles().await.unwrap_or_default();

    // Match devices to profiles
    for device in &mut devices {
        if let Some(uuid) = &device.uuid {
            device.matching_profile = profiles
                .iter()
                .find(|p| p.enabled && &p.device_uuid == uuid)
                .map(|p| p.id.clone());
        }
    }

    Ok(Json(devices))
}

/// POST /api/usb-copy/execute/:profile_id
pub async fn handle_execute(
    State(_state): State<Arc<AppState>>,
    Path(profile_id): Path<String>,
) -> Result<Json<ExecuteResponse>, (StatusCode, String)> {
    // Load profile
    let profiles = load_profiles().await?;
    let profile = profiles
        .iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Profile not found".to_string()))?;

    // Find matching device
    let devices = UsbCopyService::list_devices()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let device = devices
        .iter()
        .find(|d| d.uuid.as_ref() == Some(&profile.device_uuid))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Device not found".to_string()))?;

    // Check if device is a pool member
    let is_pool = UsbCopyService::is_pool_member(&device.device)
        .await
        .unwrap_or(false);

    if is_pool {
        return Err((
            StatusCode::BAD_REQUEST,
            "Cannot copy from pool member device".to_string(),
        ));
    }

    // Execute copy in background
    let profile_clone = profile.clone();
    let device_clone = device.clone();

    tokio::spawn(async move {
        let result = UsbCopyService::execute_copy(&profile_clone, &device_clone).await;

        // Update profile with last run status
        let mut updated_profile = profile_clone.clone();
        updated_profile.last_run = Some(std::time::SystemTime::now());
        updated_profile.last_status = Some(result.unwrap_or(CopyStatus::Failed {
            error: "Unknown error".to_string(),
        }));

        let _ = save_profile(&updated_profile).await;
    });

    Ok(Json(ExecuteResponse {
        status: "started".to_string(),
        message: "Copy operation started in background".to_string(),
    }))
}

// Helper functions

async fn load_profiles() -> Result<Vec<UsbCopyProfile>, (StatusCode, String)> {
    let path = profiles_path();

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read profiles: {}", e),
        )
    })?;

    serde_json::from_str(&content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse profiles: {}", e),
        )
    })
}

async fn save_profile(profile: &UsbCopyProfile) -> Result<(), (StatusCode, String)> {
    let mut profiles = load_profiles().await.unwrap_or_default();

    // Update or insert
    if let Some(pos) = profiles.iter().position(|p| p.id == profile.id) {
        profiles[pos] = profile.clone();
    } else {
        profiles.push(profile.clone());
    }

    let path = profiles_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create directory: {}", e),
            )
        })?;
    }

    let content = serde_json::to_string_pretty(&profiles).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize profiles: {}", e),
        )
    })?;

    tokio::fs::write(&path, content).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write profiles: {}", e),
        )
    })?;

    Ok(())
}

async fn delete_profile(id: &str) -> Result<(), (StatusCode, String)> {
    let mut profiles = load_profiles().await.unwrap_or_default();

    profiles.retain(|p| p.id != id);

    let path = profiles_path();
    let content = serde_json::to_string_pretty(&profiles).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize profiles: {}", e),
        )
    })?;

    tokio::fs::write(&path, content).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write profiles: {}", e),
        )
    })?;

    Ok(())
}

fn profiles_path() -> std::path::PathBuf {
    let data_dir =
        std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string());
    std::path::PathBuf::from(data_dir).join("data/usb_copy_profiles.json")
}

#[derive(Debug, Serialize)]
pub struct ExecuteResponse {
    pub status: String,
    pub message: String,
}
