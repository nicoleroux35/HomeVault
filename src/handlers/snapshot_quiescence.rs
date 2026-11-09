/* =============================================================================
 * File          : snapshot_quiescence.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface for snapshot quiescence hooks (tasklist P2-13): list, create,
 *   update and delete the pre/post argv run around every scheduled snapshot.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::snapshot_quiescence,
 *     crate::storage::quiescence_hook_repo
 *
 * Usage:
 *   Routes under /api/storage/snapshot-hooks in src/web/mod.rs; see
 *   docs/guides/snapshots.md.
 * =============================================================================
 */

//! Snapshot quiescence-hook endpoints.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::admin_guard::{require_admin, require_auth};
use super::AppState;
use crate::storage::quiescence_hook_repo::{QuiescenceHookRepository, QuiescenceHookRow};
use crate::utils::ResponseHelper;

#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub name: String,
    #[serde(default)]
    pub pre: Vec<String>,
    #[serde(default)]
    pub post: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct HookView {
    pub id: String,
    pub name: String,
    pub pre: Vec<String>,
    pub post: Vec<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<QuiescenceHookRow> for HookView {
    fn from(row: QuiescenceHookRow) -> Self {
        let pre = row.pre();
        let post = row.post();
        Self {
            id: row.id,
            name: row.name,
            pre,
            post,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// GET /api/storage/snapshot-hooks — list hooks (session).
pub async fn handle_hooks_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let repo = QuiescenceHookRepository::new(state.db.pool());
    match repo.list().await {
        Ok(rows) => {
            ResponseHelper::json_success(rows.into_iter().map(HookView::from).collect::<Vec<_>>())
        }
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// POST /api/storage/snapshot-hooks — create (admin).
pub async fn handle_hook_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<HookInput>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let name = input.name.trim();
    if let Err(e) = crate::snapshot_quiescence::validate_hook(name, &input.pre, &input.post) {
        return ResponseHelper::json_error(e);
    }
    let repo = QuiescenceHookRepository::new(state.db.pool());
    match repo.create(name, &input.pre, &input.post).await {
        Ok(row) => ResponseHelper::json_success(HookView::from(row)),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// PUT /api/storage/snapshot-hooks/{id} — update (admin).
pub async fn handle_hook_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::Json(input): axum::Json<HookInput>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let name = input.name.trim();
    if let Err(e) = crate::snapshot_quiescence::validate_hook(name, &input.pre, &input.post) {
        return ResponseHelper::json_error(e);
    }
    let repo = QuiescenceHookRepository::new(state.db.pool());
    match repo
        .update(&id, name, &input.pre, &input.post, input.enabled)
        .await
    {
        Ok(row) => ResponseHelper::json_success(HookView::from(row)),
        Err(sqlx::Error::RowNotFound) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No such hook".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// DELETE /api/storage/snapshot-hooks/{id} — remove (admin).
pub async fn handle_hook_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = QuiescenceHookRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => ResponseHelper::json_success(serde_json::json!({ "deleted": id })),
        Ok(false) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No such hook".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}
