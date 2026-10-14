/* =============================================================================
 * File          : monitor_layout.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-account desktop monitor-panel layout (position and visibility of
 *   the floating Resource/RAID/Weather panels), synced server-side so the
 *   same account sees the same layout from any browser origin (LAN IP,
 *   public IP, ...). Read and write are session-auth and scoped to the
 *   caller's own layout.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   GET/PUT layout routes in src/web/mod.rs; consumed by the frontend
 *   monitorStore.
 * =============================================================================
 */

//! Per-account desktop monitor layout (position + visibility of the floating
//! Resource/RAID/Weather/... monitor panels), synced server-side so the same
//! account sees the same layout regardless of which browser origin (LAN IP,
//! public IP, ...) it's accessed from - the frontend's `monitorStore` also
//! keeps a localStorage copy per origin for instant, offline-tolerant reads,
//! but this table is the cross-origin source of truth.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json as AxumJson};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct MonitorLayoutResponse {
    /// Opaque JSON blob (the frontend's own `MonitorPositions` shape); `None`
    /// if this account has never saved a layout server-side yet.
    pub layout_json: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetMonitorLayoutRequest {
    pub layout_json: String,
}

/// Fetch the calling account's own saved monitor layout, if any.
pub async fn handle_get_monitor_layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    match sqlx::query_scalar::<_, String>(
        "SELECT layout_json FROM user_monitor_layout WHERE username = ?",
    )
    .bind(&username)
    .fetch_optional(state.db.pool().as_ref())
    .await
    {
        Ok(layout_json) => ResponseHelper::json_success(MonitorLayoutResponse { layout_json }),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}

/// Upsert the calling account's own monitor layout.
pub async fn handle_set_monitor_layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<SetMonitorLayoutRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let result = sqlx::query(
        "INSERT INTO user_monitor_layout (username, layout_json, updated_at)
         VALUES (?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(username) DO UPDATE SET layout_json = excluded.layout_json, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(&username)
    .bind(&payload.layout_json)
    .execute(state.db.pool().as_ref())
    .await;

    match result {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({})),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}
