//! HTTP handlers for standalone LUKS2 disk encryption (manual unlock only).

use crate::disk::DiskService;
use crate::encryption;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::EncryptedVolumeRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct EncryptedVolumeView {
    #[serde(flatten)]
    volume: crate::storage::EncryptedVolume,
    unlocked: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateEncryptedVolumeRequest {
    pub device: String,
    pub filesystem: String,
    pub label: String,
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
pub struct UnlockRequest {
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteVolumeRequest {
    pub confirm: bool,
}

const VALID_FILESYSTEMS: &[&str] = &["ext4", "xfs", "btrfs"];

/// Lists encrypted volumes with their live locked/unlocked status.
pub async fn handle_encryption_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volumes = match repo.list_all().await {
        Ok(v) => v,
        Err(e) => return ResponseHelper::json_error(format!("Failed to list volumes: {}", e)),
    };

    let mut views = Vec::with_capacity(volumes.len());
    for volume in volumes {
        let unlocked = encryption::is_open(&volume.mapper_name).await;
        views.push(EncryptedVolumeView { volume, unlocked });
    }

    ResponseHelper::json_success(views)
}

/// Lists block devices available to be encrypted (unused, not part of any
/// RAID array or existing filesystem) — reuses `DiskService::get_available_devices`.
pub async fn handle_encryption_available_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::get_available_devices().await {
        Ok(devices) => ResponseHelper::json_success(
            devices
                .into_iter()
                .filter(|d| !d.in_use)
                .collect::<Vec<_>>(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to list available devices: {}", e)),
    }
}

/// Formats an available device as a new LUKS2 volume and mounts it.
pub async fn handle_encryption_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateEncryptedVolumeRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if req.passphrase.len() < 8 {
        return ResponseHelper::json_error("Passphrase must be at least 8 characters".to_string());
    }
    if !VALID_FILESYSTEMS.contains(&req.filesystem.as_str()) {
        return ResponseHelper::json_error(format!(
            "Invalid filesystem: {}. Supported: ext4, xfs, btrfs",
            req.filesystem
        ));
    }
    if req.label.trim().is_empty()
        || !req
            .label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return ResponseHelper::json_error(
            "Invalid label. Use letters, numbers, hyphens and underscores only.".to_string(),
        );
    }

    // The device string comes straight off the wire — without this check a
    // caller could pass "/dev/sda" and `cryptsetup luksFormat` would happily
    // destroy the boot disk. Only devices `DiskService` reports as present
    // and unused (system disk skipped, mounted / RAID-member devices flagged
    // `in_use`) may be encrypted — the same set the picker offers.
    match DiskService::get_available_devices().await {
        Ok(devices) => {
            let ok = devices.iter().any(|d| d.name == req.device && !d.in_use);
            if !ok {
                return ResponseHelper::json_error(format!(
                    "Device '{}' is not an available (present and unused) block device",
                    req.device
                ));
            }
        }
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to verify device: {}", e));
        }
    }

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let mapper_name = format!("hv-{}", Uuid::new_v4());

    if let Err(e) = encryption::format_and_open(&req.device, &mapper_name, &req.passphrase).await {
        return ResponseHelper::json_error(format!("Failed to encrypt device: {}", e));
    }

    let mapper_path = format!("/dev/mapper/{}", mapper_name);
    if let Err(e) = DiskService::format_device(&mapper_path, &req.filesystem).await {
        let _ = encryption::close(&mapper_name).await;
        return ResponseHelper::json_error(format!("Failed to format encrypted volume: {}", e));
    }

    let mount_point = format!(
        "{}/{}",
        crate::config::paths::ENCRYPTED_MOUNT_BASE,
        req.label
    );
    if let Err(e) = encryption::mount(&mapper_name, &mount_point).await {
        let _ = encryption::close(&mapper_name).await;
        return ResponseHelper::json_error(format!(
            "Encrypted volume created but failed to mount: {}",
            e
        ));
    }

    // Every failure branch from here on must unmount + close the mapper —
    // otherwise the device is left "already mapped or mounted" with no
    // corresponding DB row, so the volume becomes invisible to HomeVault
    // (repo.create never ran) while still occupying the device, and every
    // retry then fails outright until it's cleaned up by hand. Confirmed
    // live: this happened for real when the fstab-write bug (fixed
    // alongside this) caused every encryption attempt to fail right here.
    let uuid = match encryption::luks_uuid(&req.device).await {
        Ok(u) => u,
        Err(e) => {
            let _ = encryption::unmount(&mount_point).await;
            let _ = encryption::close(&mapper_name).await;
            return ResponseHelper::json_error(format!("Failed to read LUKS UUID: {}", e));
        }
    };

    if let Err(e) =
        encryption::append_fstab_entry(&mapper_name, &mount_point, &req.filesystem, &req.label)
    {
        let _ = encryption::unmount(&mount_point).await;
        let _ = encryption::close(&mapper_name).await;
        return ResponseHelper::json_error(format!(
            "Volume created but failed to persist fstab entry: {}",
            e
        ));
    }

    match repo
        .create(
            &req.device,
            &mapper_name,
            &uuid,
            &req.filesystem,
            &req.label,
            &mount_point,
        )
        .await
    {
        Ok(volume) => {
            crate::events::log_event(format!(
                "Admin '{}' created encrypted volume '{}' ({}, on {})",
                admin_username, req.label, req.filesystem, req.device
            ));
            ResponseHelper::json_success(volume)
        }
        Err(e) => {
            let _ = encryption::unmount(&mount_point).await;
            let _ = encryption::close(&mapper_name).await;
            ResponseHelper::json_error(format!("Failed to save volume metadata: {}", e))
        }
    }
}

/// Unlocks and mounts a locked volume with the supplied passphrase.
pub async fn handle_encryption_unlock(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<UnlockRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    if let Err(e) =
        encryption::open(&volume.device_path, &volume.mapper_name, &req.passphrase).await
    {
        return ResponseHelper::json_error(format!("Failed to unlock volume: {}", e));
    }

    if let Err(e) = encryption::mount(&volume.mapper_name, &volume.mount_point).await {
        return ResponseHelper::json_error(format!("Unlocked but failed to mount: {}", e));
    }

    crate::events::log_event(format!(
        "Admin '{}' unlocked encrypted volume '{}'",
        admin_username, volume.label
    ));
    ResponseHelper::json_success("Volume unlocked".to_string())
}

/// Unmounts and locks a volume.
pub async fn handle_encryption_lock(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    let _ = encryption::unmount(&volume.mount_point).await;

    match encryption::close(&volume.mapper_name).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' locked encrypted volume '{}'",
                admin_username, volume.label
            ));
            ResponseHelper::json_success("Volume locked".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to lock volume: {}", e)),
    }
}

/// Permanently deletes an encrypted volume's data (erases its LUKS key
/// slots) and removes its metadata. Requires explicit confirmation.
pub async fn handle_encryption_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<DeleteVolumeRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !req.confirm {
        return ResponseHelper::json_error("Deletion requires confirm: true".to_string());
    }

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    let _ = encryption::unmount(&volume.mount_point).await;
    if encryption::is_open(&volume.mapper_name).await {
        let _ = encryption::close(&volume.mapper_name).await;
    }
    if let Err(e) = encryption::erase(&volume.device_path).await {
        return ResponseHelper::json_error(format!("Failed to erase volume: {}", e));
    }
    let _ = encryption::remove_fstab_entry(&volume.label);

    match repo.delete(&id).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' permanently deleted encrypted volume '{}'",
                admin_username, volume.label
            ));
            ResponseHelper::json_success("Volume deleted".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete volume metadata: {}", e)),
    }
}
