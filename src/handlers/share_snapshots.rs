/* =============================================================================
 * File          : share_snapshots.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-share snapshot endpoints (tasklist P1-12): list the snapshots that
 *   cover a share, create a manual one, delete one, and restore a file or
 *   folder from one back into the live share. Reads need a session; mutations
 *   are admin-only. The snapshot backend is resolved from the share's current
 *   path, so a moved share is followed instead of a stale stored path.
 *
 * Dependencies:
 *   - axum (>= 0.8), serde (>= 1.0)
 *
 * Usage:
 *   Routes under /api/shares/{name}/{share_type}/snapshots in src/web/mod.rs.
 * =============================================================================
 */

//! Per-share snapshots: list, manual create, delete, and file restore.

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::pools::PoolService;
use crate::shares::{ShareType, SharesService};
use crate::snapshots::{sanitize_relative_path, SnapError, SnapshotService};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateShareSnapshotRequest {
    /// Optional explicit name. Generated as `manual-YYYYMMDD-HHMMSS` when
    /// omitted, so the UI can snapshot with one click.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct RestoreShareSnapshotRequest {
    /// Path **inside the share** (relative, no `..`, no absolute path).
    pub path: String,
    /// Restore overwrites live files, so the client must say so explicitly.
    #[serde(default)]
    pub confirm: bool,
}

fn parse_share_type(raw: &str) -> Result<ShareType, Response> {
    ShareType::parse(raw).ok_or_else(|| {
        ResponseHelper::json_error(format!(
            "Invalid share type '{raw}': use smb, nfs, ftp, webdav or sftp"
        ))
    })
}

fn snapshot_error(prefix: &str, error: SnapError) -> Response {
    match error {
        SnapError::Unsupported(message) => ResponseHelper::json_unavailable(message),
        SnapError::PermissionDenied => {
            ResponseHelper::json_forbidden("That path is outside the share".to_string())
        }
        SnapError::NotFound(name) => {
            ResponseHelper::json_error(format!("Snapshot '{name}' not found"))
        }
        SnapError::PoolNotFound(name) => {
            ResponseHelper::json_error(format!("No snapshot-capable pool contains '{name}'"))
        }
        other => ResponseHelper::json_error(format!("{prefix}: {other}")),
    }
}

/// The pool whose mount point contains the share path (deepest wins for a
/// nested dataset). `SnapshotService` resolves the actual dataset/subvolume.
fn pool_for_share_path(path: &str) -> Result<String, SnapError> {
    let pools = PoolService::scan_pools()
        .map_err(|e| SnapError::CommandFailed(format!("Failed to scan pools: {e}")))?;
    let mut best: Option<(usize, String)> = None;
    for pool in pools {
        let Some(mount) = pool.mount_point.as_deref() else {
            continue;
        };
        let mount = if mount.len() > 1 {
            mount.trim_end_matches('/')
        } else {
            mount
        };
        if path != mount && !path.starts_with(&format!("{mount}/")) {
            continue;
        }
        if best
            .as_ref()
            .map(|(len, _)| mount.len() > *len)
            .unwrap_or(true)
        {
            best = Some((mount.len(), pool.name));
        }
    }
    best.map(|(_, name)| name)
        .ok_or_else(|| SnapError::PoolNotFound(path.to_string()))
}

/// Resolves the share and its pool on a blocking thread (config reads and a
/// pool scan), then returns everything the handlers need.
fn resolve_target(
    name: &str,
    share_type: ShareType,
) -> Result<(crate::shares::Share, String), SnapError> {
    let share = SharesService::get_share(name, share_type.clone()).map_err(|e| {
        SnapError::CommandFailed(format!(
            "Share '{}' ({}) not found: {e}",
            name,
            share_type.as_str()
        ))
    })?;
    let pool = pool_for_share_path(&share.path)?;
    Ok((share, pool))
}

/// `GET /api/shares/{name}/{share_type}/snapshots` — every snapshot whose
/// backend covers the share, newest first, with its read-only browse path.
pub async fn handle_share_snapshots_list(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let kind = match parse_share_type(&share_type) {
        Ok(kind) => kind,
        Err(response) => return response,
    };
    let resolved = tokio::task::spawn_blocking(move || {
        let (share, pool) = resolve_target(&name, kind)?;
        let snapshots = SnapshotService::list_share_snapshots(&pool, &share.path)?;
        Ok::<_, SnapError>(serde_json::json!({
            "share": {
                "name": share.name,
                "share_type": share.share_type.as_str(),
                "path": share.path,
            },
            "pool": pool,
            "snapshots": snapshots,
        }))
    })
    .await;
    match resolved {
        Ok(Ok(payload)) => ResponseHelper::json_success(payload),
        Ok(Err(e)) => snapshot_error("Failed to list share snapshots", e),
        Err(e) => ResponseHelper::json_error(format!("Snapshot task panicked: {e}")),
    }
}

/// `POST /api/shares/{name}/{share_type}/snapshots` — create a manual snapshot
/// of the share's dataset/subvolume (admin).
pub async fn handle_share_snapshot_create(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<CreateShareSnapshotRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let kind = match parse_share_type(&share_type) {
        Ok(kind) => kind,
        Err(response) => return response,
    };
    let snapshot_name = req
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("manual-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
    if !crate::snapshots::is_safe_snapshot_name(&snapshot_name) {
        return ResponseHelper::json_error(format!(
            "Invalid snapshot name '{snapshot_name}': use letters, digits, '-', '_' and '.'"
        ));
    }

    let requested = snapshot_name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let (share, pool) = resolve_target(&name, kind)?;
        SnapshotService::create_share_snapshot(&pool, &share.path, &requested, true)?;
        SnapshotService::list_share_snapshots(&pool, &share.path)
            .map(|snapshots| snapshots.into_iter().find(|s| s.snapshot.name == requested))
    })
    .await;
    match result {
        Ok(Ok(Some(snapshot))) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' created per-share snapshot '{}'",
                admin, snapshot.snapshot.name
            ));
            ResponseHelper::json_success(snapshot)
        }
        Ok(Ok(None)) => ResponseHelper::json_error(
            "The snapshot was created but is not visible yet — refresh in a moment".to_string(),
        ),
        Ok(Err(e)) => snapshot_error("Failed to create the share snapshot", e),
        Err(e) => ResponseHelper::json_error(format!("Snapshot task panicked: {e}")),
    }
}

/// `DELETE /api/shares/{name}/{share_type}/snapshots/{snapshot}` (admin).
pub async fn handle_share_snapshot_delete(
    State(state): State<Arc<AppState>>,
    Path((name, share_type, snapshot)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let kind = match parse_share_type(&share_type) {
        Ok(kind) => kind,
        Err(response) => return response,
    };
    if !crate::snapshots::is_safe_snapshot_name(&snapshot) {
        return ResponseHelper::json_error("Invalid snapshot name".to_string());
    }
    let requested = snapshot.clone();
    let result = tokio::task::spawn_blocking(move || {
        let (share, pool) = resolve_target(&name, kind)?;
        SnapshotService::delete_share_snapshot(&pool, &share.path, &requested)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted per-share snapshot '{}'",
                admin, snapshot
            ));
            ResponseHelper::json_success(serde_json::json!({ "deleted": snapshot }))
        }
        Ok(Err(e)) => snapshot_error("Failed to delete the share snapshot", e),
        Err(e) => ResponseHelper::json_error(format!("Snapshot task panicked: {e}")),
    }
}

/// `POST /api/shares/{name}/{share_type}/snapshots/{snapshot}/restore` —
/// copy one file or folder from the snapshot back into the live share.
pub async fn handle_share_snapshot_restore(
    State(state): State<Arc<AppState>>,
    Path((name, share_type, snapshot)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(req): Json<RestoreShareSnapshotRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let kind = match parse_share_type(&share_type) {
        Ok(kind) => kind,
        Err(response) => return response,
    };
    if !req.confirm {
        return ResponseHelper::json_error(
            "Restoring overwrites the live file — confirm the restore first".to_string(),
        );
    }
    if !crate::snapshots::is_safe_snapshot_name(&snapshot) {
        return ResponseHelper::json_error("Invalid snapshot name".to_string());
    }
    let relative = match sanitize_relative_path(req.path.trim()) {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e.to_string()),
    };
    if relative.as_os_str().is_empty() {
        return ResponseHelper::json_error(
            "Choose a file or folder inside the snapshot to restore".to_string(),
        );
    }
    let relative_text = relative.to_string_lossy().into_owned();
    let requested = snapshot.clone();
    let result = tokio::task::spawn_blocking(move || {
        let (share, pool) = resolve_target(&name, kind)?;
        SnapshotService::restore_share_snapshot(&pool, &share.path, &requested, &relative_text)
    })
    .await;
    match result {
        Ok(Ok(path)) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' restored '{}' from per-share snapshot '{}'",
                admin,
                path.display(),
                snapshot
            ));
            ResponseHelper::json_success(serde_json::json!({
                "restored": true,
                "snapshot": snapshot,
                "path": path.to_string_lossy(),
            }))
        }
        Ok(Err(e)) => snapshot_error("Failed to restore the share snapshot", e),
        Err(e) => ResponseHelper::json_error(format!("Restore task panicked: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_types_are_parsed_strictly() {
        assert_eq!(parse_share_type("smb").unwrap().as_str(), "smb");
        assert_eq!(parse_share_type("CIFS").unwrap().as_str(), "smb");
        assert!(parse_share_type("rsync").is_err());
    }
}
