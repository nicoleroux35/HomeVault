/* =============================================================================
 * File          : db_backup.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Admin endpoints for HomeVault's own database snapshots: read the current
 *   state (settings, existing snapshots, total size, the reason the last
 *   attempt failed), take a snapshot on demand, and change the
 *   retention/interval. The database runs in WAL mode, so a snapshot is the
 *   only thing an operator can copy and restore — the `.db` file alone contains
 *   no tables.
 *
 *   Every handler loads a fresh service (the schedule lives in
 *   `data/db_backup.json`) but the snapshot itself is serialised by a
 *   process-wide lock inside `DbBackupService`, so a manual click during a
 *   scheduled run waits instead of producing a second `VACUUM INTO`.
 *
 * Dependencies:
 *   - axum (>= 0.8), serde (>= 1.0)
 *
 * Usage:
 *   Routed under /api/settings/backup/db in src/web/mod.rs; the service itself
 *   lives in src/db_backup.rs.
 * =============================================================================
 */

//! Admin endpoints for the application-database snapshots.

use crate::db_backup::{DbBackupService, DbBackupSettings};
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use serde::Deserialize;
use std::sync::Arc;

/// Body for `PUT /api/settings/backup/db`.
#[derive(Debug, Deserialize)]
pub struct DbBackupSettingsRequest {
    pub interval_hours: u32,
    pub retain: u32,
}

/// Reports the snapshot settings and every snapshot currently on disk.
pub async fn handle_db_backup_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let service = DbBackupService::load(state.db.clone()).await;
    ResponseHelper::json_success(service.status().await)
}

/// Takes a snapshot immediately.
pub async fn handle_db_backup_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let service = DbBackupService::load(state.db.clone()).await;
    match service.backup_now().await {
        Ok(entry) => {
            crate::events::log_event(format!(
                "Database snapshot written: {} ({} bytes)",
                entry.name, entry.size_bytes
            ));
            ResponseHelper::json_success(serde_json::json!({
                "backup": entry,
                "status": service.status().await,
            }))
        }
        Err(e) => {
            crate::events::log_event_warn(format!("Database snapshot failed: {}", e));
            ResponseHelper::json_internal_error(e)
        }
    }
}

/// Updates the snapshot interval and retention.
pub async fn handle_db_backup_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<DbBackupSettingsRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let service = DbBackupService::load(state.db.clone()).await;
    let next = DbBackupSettings {
        interval_hours: req.interval_hours,
        retain: req.retain,
    };
    match service.update_settings(next).await {
        Ok(()) => {
            // Applying a lower retention immediately is what an operator
            // expects after saving the form.
            service.prune(service.settings().retain).await;
            ResponseHelper::json_success(service.status().await)
        }
        Err(e) => ResponseHelper::json_error(e),
    }
}
