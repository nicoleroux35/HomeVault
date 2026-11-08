/* =============================================================================
 * File          : restore_drill.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin endpoints for the non-destructive restore drill (P2-9): run it now
 *   and read the persisted last report. The drill copies the newest database
 *   snapshot and decodes the newest configuration archive; it never writes to
 *   the live data directory.
 *
 * Dependencies:
 *   - axum (>= 0.8), serde_json
 *
 * Usage:
 *   Routed under /api/settings/backup/restore-drill in src/web/mod.rs.
 * =============================================================================
 */

//! Admin endpoints for the restore drill.

use crate::config::paths;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

/// Configuration backups live beside the database snapshots, under
/// `data/config-backups/` (same directory the export handler writes).
fn config_backup_dir() -> std::path::PathBuf {
    paths::data_dir().join("config-backups")
}

/// `POST /api/settings/backup/restore-drill` — run the drill now.
///
/// The optional `x-backup-passphrase` header is used only to verify an
/// encrypted configuration archive; it is never stored or echoed.
pub async fn handle_restore_drill_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let passphrase = headers
        .get("x-backup-passphrase")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .filter(|value| !value.is_empty());

    let base = paths::base_dir();
    let report = crate::restore_drill::run(
        &paths::db_backup_dir(),
        &config_backup_dir(),
        state.db.pool().as_ref(),
        passphrase.as_deref(),
    )
    .await;

    let mut body = serde_json::json!({ "report": report });
    if let Err(e) = crate::restore_drill::save_report(&base, &report) {
        body["warning"] = serde_json::Value::String(format!(
            "the report could not be persisted: {e}"
        ));
    }
    ResponseHelper::json_success(body)
}

/// `GET /api/settings/backup/restore-drill` — the last report, if any.
pub async fn handle_restore_drill_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(serde_json::json!({
        "report": crate::restore_drill::load_report(&paths::base_dir()),
    }))
}
