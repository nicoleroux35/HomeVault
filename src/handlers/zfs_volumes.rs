/* =============================================================================
 * File          : zfs_volumes.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface for thin/thick ZFS zvols (tasklist P2-3): list with
 *   saturation flags, create and destroy.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::zfs_volumes
 *
 * Usage:
 *   Routes under /api/storage/zfs/volumes in src/web/mod.rs; see
 *   docs/guides/storage.md.
 * =============================================================================
 */

//! ZFS zvol endpoints.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

use super::admin_guard::{require_admin, require_auth};
use super::AppState;
use crate::utils::ResponseHelper;

#[derive(Debug, Deserialize)]
pub struct VolumeInput {
    pub name: String,
    pub size_bytes: u64,
    /// Sparse volume with `refreservation=none` (default).
    #[serde(default = "default_true")]
    pub thin: bool,
}

fn default_true() -> bool {
    true
}

/// GET /api/storage/zfs/volumes — list volumes (session).
pub async fn handle_volumes_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let _ = &state;
    match crate::zfs_volumes::list_volumes().await {
        Ok(volumes) => ResponseHelper::json_success(volumes),
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// POST /api/storage/zfs/volumes — create a volume (admin).
pub async fn handle_volume_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<VolumeInput>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    match crate::zfs_volumes::create_volume(&input.name, input.size_bytes, input.thin).await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({
            "name": input.name,
            "thin": input.thin,
        })),
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// DELETE /api/storage/zfs/volumes/{name} — destroy a volume (admin).
pub async fn handle_volume_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    match crate::zfs_volumes::destroy_volume(&name).await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "deleted": name })),
        Err(e) => ResponseHelper::json_error(e),
    }
}
