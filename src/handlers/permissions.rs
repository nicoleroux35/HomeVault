/* =============================================================================
 * File          : permissions.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Access-control endpoints for shares: list permission entries, grant or
 *   revoke per-user/per-group permissions (optionally applied recursively
 *   to the share contents), sync them onto the filesystem, and resolve one
 *   user's effective permissions. Entry management is admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/permissions in src/web/mod.rs; rules enforced by
 *   PermissionService, share lookup by SharesService.
 * =============================================================================
 */

//! Share permission endpoints: grant/revoke per-user or per-group access to
//! a share (optionally recursive), list entries, and resolve a user's
//! effective permissions.

use crate::handlers::AppState;
use crate::permissions::PermissionService;
use crate::shares::{ShareType, SharesService};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Body for granting a permission to a user or group on a share.
#[derive(Debug, Deserialize)]
pub struct GrantPermissionRequest {
    pub grantee_type: String,
    pub grantee_name: String,
    pub permission: String,
    pub recursive: Option<bool>,
}

/// Body for revoking a permission from a user or group on a share.
#[derive(Debug, Deserialize)]
pub struct RevokePermissionRequest {
    pub grantee_type: String,
    pub grantee_name: String,
}

/// A permission entry returned from the API.
#[derive(Debug, Serialize)]
pub struct SharePermissionResponse {
    pub share_name: String,
    pub share_type: String,
    pub grantee_type: String,
    pub grantee_name: String,
    pub permission: String,
    pub recursive: bool,
}

async fn require_admin(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Some(ResponseHelper::json_error(
            "No session token provided".to_string(),
        ));
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin {
                return Some(ResponseHelper::json_error(
                    "Admin access required".to_string(),
                ));
            }
            None
        }
        Err(_) => Some(ResponseHelper::json_unauthorized(
            "Invalid session".to_string(),
        )),
    }
}

/// Lists all permissions for a specific share.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_list(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    if ShareType::parse(&share_type).is_none() {
        return ResponseHelper::json_error("Invalid share type".to_string());
    }

    match PermissionService::list_share_permissions(&share_name, &share_type, state.db.pool()).await
    {
        Ok(perms) => ResponseHelper::json_success(perms),
        Err(e) => ResponseHelper::json_error(format!("Failed to list permissions: {}", e)),
    }
}

/// Grants read/write/execute permission to a user or group on a share.
#[axum_macros::debug_handler]
pub async fn handle_share_permission_grant(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
    req: axum::extract::Json<GrantPermissionRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(parsed_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    if !PermissionService::validate_permission_string(&req.permission) {
        return ResponseHelper::json_error(format!(
            "Invalid permission: '{}'. Valid values: r, rw, rwx",
            req.permission
        ));
    }

    if req.grantee_type != "user" && req.grantee_type != "group" {
        return ResponseHelper::json_error(
            "Invalid grantee_type. Must be 'user' or 'group'".to_string(),
        );
    }

    let share_path = match SharesService::get_share(&share_name, parsed_type) {
        Ok(share) => share.path,
        Err(e) => return ResponseHelper::json_error(format!("Share not found: {}", e)),
    };
    let recursive = req.recursive.unwrap_or(true);

    match PermissionService::grant_permission(
        &share_name,
        &share_type,
        &share_path,
        &req.grantee_type,
        &req.grantee_name,
        &req.permission,
        recursive,
        state.db.pool(),
    )
    .await
    {
        Ok(perm) => ResponseHelper::json_success(SharePermissionResponse {
            share_name: perm.share_name,
            share_type: perm.share_type,
            grantee_type: perm.grantee_type,
            grantee_name: perm.grantee_name,
            permission: perm.permission,
            recursive: perm.recursive,
        }),
        Err(e) => ResponseHelper::json_error(format!("Failed to grant permission: {}", e)),
    }
}

/// Revokes a permission from a user or group on a share.
#[axum_macros::debug_handler]
pub async fn handle_share_permission_revoke(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
    req: axum::extract::Json<RevokePermissionRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(parsed_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    let share_path = match SharesService::get_share(&share_name, parsed_type) {
        Ok(share) => share.path,
        Err(e) => return ResponseHelper::json_error(format!("Share not found: {}", e)),
    };

    match PermissionService::revoke_permission(
        &share_name,
        &share_type,
        &share_path,
        &req.grantee_type,
        &req.grantee_name,
        state.db.pool(),
    )
    .await
    {
        Ok(_) => ResponseHelper::json_success("Permission revoked successfully".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke permission: {}", e)),
    }
}

/// Synchronizes database permissions to the filesystem ACLs for a share.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_sync(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(parsed_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    let share_path = match SharesService::get_share(&share_name, parsed_type) {
        Ok(share) => share.path,
        Err(e) => return ResponseHelper::json_error(format!("Share not found: {}", e)),
    };

    match PermissionService::sync_share_permissions(
        &share_name,
        &share_type,
        &share_path,
        state.db.pool(),
    )
    .await
    {
        Ok(_) => ResponseHelper::json_success("Permissions synced successfully".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to sync permissions: {}", e)),
    }
}

/// Returns all share permissions for a given user (admin or self-only access).
#[axum_macros::debug_handler]
pub async fn handle_user_permissions(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin && session.username != username {
                return ResponseHelper::json_error("Access denied".to_string());
            }
        }
        Err(_) => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    }

    match PermissionService::get_user_permissions(&username, state.db.pool()).await {
        Ok(perms) => ResponseHelper::json_success(perms),
        Err(e) => ResponseHelper::json_error(format!("Failed to get permissions: {}", e)),
    }
}
