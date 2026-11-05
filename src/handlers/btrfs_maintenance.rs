/* =============================================================================
 * File          : btrfs_maintenance.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface for per-path Btrfs compression policies (tasklist P2-11):
 *   list, create/update, delete, and an explicit "apply now" action.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::btrfs_maintenance,
 *     crate::storage::btrfs_policy_repo
 *
 * Usage:
 *   Routes under /api/storage/btrfs in src/web/mod.rs; see
 *   docs/guides/storage.md.
 * =============================================================================
 */

//! Btrfs compression-policy endpoints.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

use super::admin_guard::{require_admin, require_auth};
use super::AppState;
use crate::storage::btrfs_policy_repo::BtrfsPolicyRepository;
use crate::utils::ResponseHelper;

#[derive(Debug, Deserialize)]
pub struct PolicyQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct PolicyInput {
    pub path: String,
    pub level: String,
    #[serde(default = "default_true")]
    pub defrag: bool,
}

fn default_true() -> bool {
    true
}

fn normalize_level(level: &str) -> Result<String, String> {
    if level.trim().eq_ignore_ascii_case("none") {
        Ok("none".to_string())
    } else {
        crate::btrfs_maintenance::validate_compression_level(level)
    }
}

/// GET /api/storage/btrfs/policies — list policies (session).
pub async fn handle_policies_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let repo = BtrfsPolicyRepository::new(state.db.pool());
    match repo.list().await {
        Ok(policies) => ResponseHelper::json_success(policies),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// PUT /api/storage/btrfs/policies — create or update (admin).
pub async fn handle_policy_upsert(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<PolicyInput>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let path = match crate::btrfs_maintenance::validate_policy_path(&input.path) {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let level = match normalize_level(&input.level) {
        Ok(level) => level,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let target = std::path::Path::new(&path);
    if !target.is_dir() {
        return ResponseHelper::json_error(
            "the policy path must be an existing directory".to_string(),
        );
    }
    let repo = BtrfsPolicyRepository::new(state.db.pool());
    match repo.upsert(&path, &level, input.defrag).await {
        Ok(policy) => ResponseHelper::json_success(policy),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// DELETE /api/storage/btrfs/policies?path=… — remove a policy (admin).
pub async fn handle_policy_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PolicyQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let path = match crate::btrfs_maintenance::validate_policy_path(&query.path) {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let repo = BtrfsPolicyRepository::new(state.db.pool());
    match repo.delete(&path).await {
        Ok(true) => ResponseHelper::json_success(serde_json::json!({ "deleted": path })),
        Ok(false) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No such policy".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// POST /api/storage/btrfs/policies/apply — apply one policy now (admin).
pub async fn handle_policy_apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::Json(query): axum::Json<PolicyQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let path = match crate::btrfs_maintenance::validate_policy_path(&query.path) {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let repo = BtrfsPolicyRepository::new(state.db.pool());
    let policy = match repo.get(&path).await {
        Ok(Some(policy)) => policy,
        Ok(None) => {
            return ResponseHelper::json_error_with_status(
                StatusCode::NOT_FOUND,
                "No such policy".to_string(),
            )
        }
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };
    match crate::btrfs_maintenance::apply_compression_policy(&policy).await {
        Ok(message) => {
            let _ = repo.mark_applied(&path).await;
            ResponseHelper::json_success(serde_json::json!({
                "path": path,
                "message": message,
            }))
        }
        Err(e) => ResponseHelper::json_error(e),
    }
}
