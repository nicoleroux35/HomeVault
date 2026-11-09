/* =============================================================================
 * File          : zfs_cache.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   ZFS L2ARC/SLOG endpoints: list every ZFS pool with its cache and log
 *   devices, attach a free flash device, and detach one again.
 *
 *   Admin-only, like every endpoint that runs `zpool` as root. The device is
 *   re-validated on the server against the disk inventory — the UI's list is a
 *   convenience, not the authority.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::zfs_cache, crate::disk, crate::handlers::AppState
 *
 * Usage:
 *   Routes /api/zfs-cache in src/web/mod.rs; see docs/guides/cache.md.
 * =============================================================================
 */

//! ZFS L2ARC/SLOG endpoints, see docs/guides/cache.md.
//!
//! * `GET  /api/zfs-cache`        — pools + their cache/log devices + free SSDs
//! * `POST /api/zfs-cache`        — attach `{ pool, kind, device }`
//! * `POST /api/zfs-cache/remove` — detach `{ pool, device }`

use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::handlers::admin_guard::require_admin;
use crate::utils::ResponseHelper;
use crate::zfs_cache::{self, ZfsCacheKind, ZfsPoolCache};

/// A free device the UI may offer as a cache device.
#[derive(Debug, Serialize)]
pub struct CacheCandidate {
    pub name: String,
    pub size: u64,
    pub model: String,
    /// True for a spinning disk: the server refuses it, and the UI greys it out
    /// with the reason instead of letting the admin submit a doomed request.
    pub rotational: bool,
}

#[derive(Debug, Serialize)]
pub struct ZfsCacheResponse {
    /// `zpool` is installed.
    pub available: bool,
    pub pools: Vec<ZfsPoolCache>,
    pub candidates: Vec<CacheCandidate>,
}

/// `GET /api/zfs-cache` — what each ZFS pool already accelerates with, and
/// which free disks could be attached.
pub async fn handle_zfs_cache_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let available = tokio::task::spawn_blocking(zfs_cache::zfs_available)
        .await
        .unwrap_or(false);
    if !available {
        return ResponseHelper::json_success(ZfsCacheResponse {
            available: false,
            pools: Vec::new(),
            candidates: Vec::new(),
        });
    }

    let pools = tokio::task::spawn_blocking(zfs_cache::status_all).await;
    let pools = match pools {
        Ok(Ok(pools)) => pools,
        Ok(Err(e)) => return ResponseHelper::json_error(e),
        Err(e) => return ResponseHelper::json_internal_error(format!("Status task failed: {e}")),
    };

    // The candidate list comes from the same inventory the Storage wizard
    // uses, so a disk that is already mounted, partitioned or in a pool is
    // never offered.
    let candidates = match crate::disk::DiskService::get_available_devices().await {
        Ok(devices) => devices
            .into_iter()
            .filter(|device| !device.in_use)
            .map(|device| CacheCandidate {
                name: device.name,
                size: device.size,
                model: device.model,
                rotational: device.rotational,
            })
            .collect(),
        // A failed inventory is not fatal: the page still shows what is
        // attached, it just cannot offer a new device.
        Err(_) => Vec::new(),
    };

    ResponseHelper::json_success(ZfsCacheResponse {
        available,
        pools,
        candidates,
    })
}

#[derive(Debug, Deserialize)]
pub struct AttachRequest {
    pub pool: String,
    /// `l2arc` (read cache) or `slog` (separate log).
    pub kind: String,
    pub device: String,
}

/// `POST /api/zfs-cache` — attach a flash device as a read cache or log.
pub async fn handle_zfs_cache_attach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let req: AttachRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };
    let Some(kind) = ZfsCacheKind::parse(&req.kind) else {
        return ResponseHelper::json_error(
            "Choose a read cache (L2ARC) or a separate log (SLOG)".to_string(),
        );
    };
    let device = match zfs_cache::validate_device(&req.device) {
        Ok(device) => device,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let pool = req.pool.trim().to_string();
    if pool.is_empty() {
        return ResponseHelper::json_error("Choose a pool".to_string());
    }

    // The pool must exist *and* be ZFS: `zpool add` on a Btrfs/mdadm pool
    // would fail with a confusing message.
    let pools = match tokio::task::spawn_blocking(crate::pools::PoolService::scan_pools).await {
        Ok(Ok(pools)) => pools,
        Ok(Err(e)) => return ResponseHelper::json_error(format!("Failed to scan pools: {e}")),
        Err(e) => return ResponseHelper::json_internal_error(format!("Pool task failed: {e}")),
    };
    let Some(target) = pools
        .iter()
        .find(|candidate| candidate.name == pool)
        .cloned()
    else {
        return ResponseHelper::json_error(format!("No pool named '{pool}'"));
    };
    if target.pool_type != crate::pools::PoolType::Zfs {
        return ResponseHelper::json_error(format!(
            "'{pool}' is not a ZFS pool: L2ARC and SLOG are ZFS features"
        ));
    }

    let existing = match zfs_cache::status(&pool) {
        Ok(status) => status.devices,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let inventory = crate::disk::DiskService::get_available_devices().await;
    let device_name = device.trim_start_matches("/dev/").to_string();
    let inventory_entry = inventory.as_ref().ok().and_then(|devices| {
        devices
            .iter()
            .find(|candidate| candidate.name == device || candidate.name == device_name)
            .cloned()
    });
    let Some(entry) = inventory_entry else {
        return ResponseHelper::json_error(format!(
            "{device} is not a whole disk known to this server; choose a disk from the list"
        ));
    };

    if let Err(refusal) = zfs_cache::check_device_for_pool(
        kind,
        &device,
        entry.in_use,
        entry.reason.as_deref(),
        std::path::Path::new(&device).exists(),
        entry.rotational,
        &existing,
    ) {
        return ResponseHelper::json_error(refusal);
    }

    let result = tokio::task::spawn_blocking({
        let pool = pool.clone();
        let device = device.clone();
        move || zfs_cache::add(&pool, kind, &device)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{admin}' attached {device} to pool '{pool}' as a {}",
                match kind {
                    ZfsCacheKind::L2Arc => "read cache (L2ARC)",
                    ZfsCacheKind::Slog => "separate log (SLOG)",
                }
            ));
            match zfs_cache::status(&pool) {
                Ok(status) => ResponseHelper::json_success(status),
                Err(e) => ResponseHelper::json_success(serde_json::json!({
                    "pool": pool,
                    "warning": format!("Attached, but the new status could not be read: {e}"),
                })),
            }
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_internal_error(format!("Attach task failed: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct DetachRequest {
    pub pool: String,
    pub device: String,
}

/// `POST /api/zfs-cache/remove` — detach a cache or log device.
pub async fn handle_zfs_cache_detach(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let req: DetachRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };
    let device = match zfs_cache::validate_device(&req.device) {
        Ok(device) => device,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let pool = req.pool.trim().to_string();

    // Only a device that really is a cache/log vdev of this pool may be
    // removed: the request must not be able to name a data disk.
    let current = match zfs_cache::status(&pool) {
        Ok(status) => status,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if !current.devices.iter().any(|entry| entry.device == device) {
        return ResponseHelper::json_error(format!(
            "{device} is not a cache or log device of '{pool}'"
        ));
    }

    let result = tokio::task::spawn_blocking({
        let pool = pool.clone();
        let device = device.clone();
        move || zfs_cache::remove(&pool, &device)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{admin}' detached {device} from pool '{pool}'"
            ));
            match zfs_cache::status(&pool) {
                Ok(status) => ResponseHelper::json_success(status),
                Err(e) => ResponseHelper::json_success(serde_json::json!({
                    "pool": pool,
                    "warning": format!("Detached, but the new status could not be read: {e}"),
                })),
            }
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_internal_error(format!("Detach task failed: {e}")),
    }
}
