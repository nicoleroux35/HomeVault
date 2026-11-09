/* =============================================================================
 * File          : usb_copy.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   USB one-touch copy endpoints: CRUD the copy profiles, list the removable
 *   devices currently attached and start a copy for a profile.
 *
 *   Reads need a session; every mutation is admin-only. The copy itself writes
 *   into a share as root and mounts/unmounts a device, so an unauthenticated
 *   caller must never reach it.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::usb_copy
 *
 * Usage:
 *   Routes under /api/usb-copy in src/web/mod.rs; see docs/guides/usb-copy.md.
 * =============================================================================
 */

//! USB one-touch copy endpoints, see docs/guides/usb-copy.md.
//!
//! * `GET    /api/usb-copy/profiles`              — list
//! * `POST   /api/usb-copy/profiles`              — create
//! * `PUT    /api/usb-copy/profiles/{id}`         — update
//! * `DELETE /api/usb-copy/profiles/{id}`         — delete
//! * `GET    /api/usb-copy/devices`               — attached removable devices
//! * `POST   /api/usb-copy/execute/{profile_id}`  — start a copy

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Serialize;
use std::sync::Arc;

use super::AppState;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::transfer_history::{record_transfer, NewTransfer, TransferOperation, TransferStatus};
use crate::usb_copy::{CopyStatus, UsbCopyProfile, UsbCopyService};
use crate::utils::ResponseHelper;

/// Re-wraps a `(status, message)` pair from a helper as the JSON error
/// envelope shared by the rest of the API.
fn fail(err: (StatusCode, String)) -> Response {
    ResponseHelper::json_error_with_status(err.0, err.1)
}

/// GET /api/usb-copy/profiles
pub async fn handle_profiles_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    match load_profiles().await {
        Ok(profiles) => ResponseHelper::json_success(profiles),
        Err(e) => fail(e),
    }
}

/// POST /api/usb-copy/profiles
pub async fn handle_profile_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let profile: UsbCopyProfile = match serde_json::from_slice(&body) {
        Ok(profile) => profile,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    if let Err(e) = UsbCopyService::validate_profile(&profile) {
        return ResponseHelper::json_error(e);
    }

    if let Err(e) = save_profile(&profile).await {
        return fail(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' created USB copy profile '{}'",
        admin, profile.name
    ));
    ResponseHelper::json_success(profile)
}

/// PUT /api/usb-copy/profiles/:id
pub async fn handle_profile_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let mut profile: UsbCopyProfile = match serde_json::from_slice(&body) {
        Ok(profile) => profile,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    // The id comes from the path, never from the body: a body id that
    // disagreed with the URL used to silently move the profile.
    profile.id = id;

    if let Err(e) = UsbCopyService::validate_profile(&profile) {
        return ResponseHelper::json_error(e);
    }

    if let Err(e) = save_profile(&profile).await {
        return fail(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' updated USB copy profile '{}'",
        admin, profile.name
    ));
    ResponseHelper::json_success(profile)
}

/// DELETE /api/usb-copy/profiles/:id
pub async fn handle_profile_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if let Err(e) = delete_profile(&id).await {
        return fail(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' deleted USB copy profile '{}'",
        admin, id
    ));
    ResponseHelper::json_success(serde_json::json!({ "deleted": id }))
}

/// GET /api/usb-copy/devices
pub async fn handle_devices_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let mut devices = match UsbCopyService::list_devices().await {
        Ok(devices) => devices,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };

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

    ResponseHelper::json_success(devices)
}

/// POST /api/usb-copy/execute/:profile_id
///
/// The copy runs in the background; the handler answers as soon as it is
/// scheduled. The profile's `last_run` / `last_status` are written by the task
/// itself, so a copy that fails still leaves its outcome visible.
pub async fn handle_execute(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(profile_id): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let profiles = match load_profiles().await {
        Ok(profiles) => profiles,
        Err(e) => return fail(e),
    };
    let profile = match profiles.iter().find(|p| p.id == profile_id) {
        Some(profile) => profile,
        None => {
            return ResponseHelper::json_error_with_status(
                StatusCode::NOT_FOUND,
                "Profile not found".to_string(),
            )
        }
    };

    if !profile.enabled {
        return ResponseHelper::json_error(format!("Profile '{}' is disabled", profile.name));
    }

    let devices = match UsbCopyService::list_devices().await {
        Ok(devices) => devices,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };

    let device = match devices
        .iter()
        .find(|d| d.uuid.as_ref() == Some(&profile.device_uuid))
    {
        Some(device) => device,
        None => {
            return ResponseHelper::json_error_with_status(
                StatusCode::NOT_FOUND,
                "Device not found".to_string(),
            )
        }
    };

    // Refuse to read from a disk that belongs to an array: copying the raw
    // members out would look like a backup and be useless as one.
    if UsbCopyService::is_pool_member(&device.device)
        .await
        .unwrap_or(false)
    {
        return ResponseHelper::json_error("Cannot copy from pool member device".to_string());
    }

    let profile_clone = profile.clone();
    let device_clone = device.clone();
    let history_pool = state.db.pool();
    let history_profile = profile.clone();

    crate::events::log_event(format!(
        "Admin '{}' started USB copy profile '{}' from {}",
        admin, profile.name, device.device
    ));

    tokio::spawn(async move {
        let result = UsbCopyService::execute_copy(&profile_clone, &device_clone).await;

        let mut updated_profile = profile_clone.clone();
        updated_profile.last_run = Some(std::time::SystemTime::now());
        let (run_status, transfer_status, verified, bytes, error) = match result {
            Ok(status) => {
                let (bytes, verified) = match &status {
                    CopyStatus::Success { bytes_copied, .. } => {
                        (*bytes_copied, profile_clone.verify_checksums)
                    }
                    _ => (0, false),
                };
                let transfer_status = match &status {
                    CopyStatus::Success { .. } => TransferStatus::Complete,
                    CopyStatus::Cancelled => TransferStatus::Cancelled,
                    CopyStatus::Failed { .. } => TransferStatus::Failed,
                };
                (status, transfer_status, verified, bytes, None)
            }
            Err(error) => (
                CopyStatus::Failed {
                    error: error.clone(),
                },
                TransferStatus::Failed,
                false,
                0,
                Some(error),
            ),
        };
        updated_profile.last_status = Some(run_status);

        // FIAB-13: one audit row per run, visible in the Explorer transfer
        // history. A per-file row would flood the table on a photo import.
        record_transfer(
            history_pool,
            NewTransfer {
                job_id: None,
                username: "system".to_string(),
                operation: TransferOperation::UsbCopy,
                source_path: Some(history_profile.source_path.clone()),
                dest_path: Some(history_profile.destination_path.clone()),
                bytes_total: bytes,
                duration_ms: None,
                method: Some(history_profile.strategy.as_str().to_string()),
                verified,
                status: transfer_status,
                error_message: error,
            },
        )
        .await;

        if let Err(e) = save_profile(&updated_profile).await {
            tracing::warn!("USB copy: could not persist the run outcome: {:?}", e);
        }
    });

    ResponseHelper::json_success(ExecuteResponse {
        status: "started".to_string(),
        message: "Copy operation started in background".to_string(),
    })
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

    write_profiles(&profiles).await
}

async fn delete_profile(id: &str) -> Result<(), (StatusCode, String)> {
    let mut profiles = load_profiles().await.unwrap_or_default();

    profiles.retain(|p| p.id != id);

    write_profiles(&profiles).await
}

async fn write_profiles(profiles: &[UsbCopyProfile]) -> Result<(), (StatusCode, String)> {
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

    let content = serde_json::to_string_pretty(profiles).map_err(|e| {
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
