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
use crate::shares::{Share, ShareType, SharesService};
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

/// Lists all permissions for a specific share.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_list(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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

/// The protocol bindings of one logical share (same name, same path).
async fn bindings_for(name: &str) -> Result<Vec<Share>, String> {
    let wanted = name.to_string();
    tokio::task::spawn_blocking(move || SharesService::get_shares())
        .await
        .map_err(|e| format!("share list worker failed: {e}"))?
        .map_err(|e| e.to_string())
        .map(|shares| shares.into_iter().filter(|share| share.name == wanted).collect())
}

/// Lists the union of grants across every protocol binding of a logical
/// share. One grant row per protocol is an implementation detail; the UI
/// shows one line per grantee.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_all_list(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let bindings = match bindings_for(&name).await {
        Ok(bindings) => bindings,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if bindings.is_empty() {
        return ResponseHelper::json_error(format!("Share '{name}' not found"));
    }
    let mut seen: std::collections::BTreeSet<(String, String, String, bool)> =
        std::collections::BTreeSet::new();
    let mut out: Vec<SharePermissionResponse> = Vec::new();
    for binding in bindings {
        match PermissionService::list_share_permissions(
            &binding.name,
            binding.share_type.as_str(),
            state.db.pool(),
        )
        .await
        {
            Ok(permissions) => {
                for permission in permissions {
                    let key = (
                        permission.grantee_type.clone(),
                        permission.grantee_name.clone(),
                        permission.permission.clone(),
                        permission.recursive,
                    );
                    if seen.insert(key) {
                        out.push(SharePermissionResponse {
                            share_name: permission.share_name,
                            share_type: permission.share_type,
                            grantee_type: permission.grantee_type,
                            grantee_name: permission.grantee_name,
                            permission: permission.permission,
                            recursive: permission.recursive,
                        });
                    }
                }
            }
            Err(e) => return ResponseHelper::json_error(format!("Failed to list permissions: {e}")),
        }
    }
    ResponseHelper::json_success(out)
}

/// Grants the same ACL to every protocol binding of a logical share.
#[axum_macros::debug_handler]
pub async fn handle_share_permission_all_grant(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<GrantPermissionRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let bindings = match bindings_for(&name).await {
        Ok(bindings) => bindings,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if bindings.is_empty() {
        return ResponseHelper::json_error(format!("Share '{name}' not found"));
    }
    let recursive = req.recursive.unwrap_or(true);
    for binding in bindings {
        if let Err(e) = PermissionService::grant_permission(
            &binding.name,
            binding.share_type.as_str(),
            &binding.path,
            &req.grantee_type,
            &req.grantee_name,
            &req.permission,
            recursive,
            state.db.pool(),
        )
        .await
        {
            return ResponseHelper::json_error(format!("Failed to grant permission: {e}"));
        }
    }
    ResponseHelper::json_success("Permission granted".to_string())
}

/// Revokes the grant from every protocol binding of a logical share.
#[axum_macros::debug_handler]
pub async fn handle_share_permission_all_revoke(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<RevokePermissionRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let bindings = match bindings_for(&name).await {
        Ok(bindings) => bindings,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if bindings.is_empty() {
        return ResponseHelper::json_error(format!("Share '{name}' not found"));
    }
    for binding in bindings {
        if let Err(e) = PermissionService::revoke_permission(
            &binding.name,
            binding.share_type.as_str(),
            &binding.path,
            &req.grantee_type,
            &req.grantee_name,
            state.db.pool(),
        )
        .await
        {
            return ResponseHelper::json_error(format!("Failed to revoke permission: {e}"));
        }
    }
    ResponseHelper::json_success("Permission revoked".to_string())
}

/// Re-applies the stored ACLs on every binding of a logical share.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_all_sync(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let bindings = match bindings_for(&name).await {
        Ok(bindings) => bindings,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if bindings.is_empty() {
        return ResponseHelper::json_error(format!("Share '{name}' not found"));
    }
    for binding in bindings {
        if let Err(e) = PermissionService::sync_share_permissions(
            &binding.name,
            binding.share_type.as_str(),
            &binding.path,
            state.db.pool(),
        )
        .await
        {
            return ResponseHelper::json_error(format!("Failed to sync permissions: {e}"));
        }
    }
    ResponseHelper::json_success("Permissions synced".to_string())
}

/// Body for replacing the ACL of one share.
#[derive(serde::Deserialize)]
pub struct SetAclRequest {
    pub entries: Vec<crate::acl::AclEntry>,
    #[serde(default)]
    pub default_entries: Vec<crate::acl::AclEntry>,
    #[serde(default)]
    pub recursive: bool,
}

/// Reads the live POSIX ACL of a share path (access + default entries).
#[axum_macros::debug_handler]
pub async fn handle_share_acl_get(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let Some(parsed_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let share = match SharesService::get_share(&share_name, parsed_type) {
        Ok(share) => share,
        Err(e) => return ResponseHelper::json_error(format!("Share not found: {e}")),
    };
    match tokio::task::spawn_blocking(move || crate::acl::AclService::get_acl(&share.path)).await {
        Ok(Ok(acl)) => ResponseHelper::json_success(acl),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to read ACL: {e}")),
        Err(e) => ResponseHelper::json_error(format!("ACL worker failed: {e}")),
    }
}

/// Replaces the ACL of a share (optionally over the whole tree). This is the
/// POSIX editor; Windows ACLs are deliberately not simulated (see the guide).
#[axum_macros::debug_handler]
pub async fn handle_share_acl_set(
    State(state): State<Arc<AppState>>,
    Path((share_name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
    req: axum::extract::Json<SetAclRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let Some(parsed_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let share = match SharesService::get_share(&share_name, parsed_type) {
        Ok(share) => share,
        Err(e) => return ResponseHelper::json_error(format!("Share not found: {e}")),
    };
    let result = tokio::task::spawn_blocking(move || {
        if req.recursive {
            crate::acl::AclService::remove_all_acl_recursive(&share.path)?;
            if !req.entries.is_empty() {
                crate::acl::AclService::set_acl_recursive(&share.path, &req.entries)?;
            }
            if !req.default_entries.is_empty() {
                crate::acl::AclService::set_default_acl_recursive(
                    &share.path,
                    &req.default_entries,
                )?;
            }
        } else {
            crate::acl::AclService::remove_all_acl(&share.path)?;
            if !req.entries.is_empty() {
                crate::acl::AclService::set_acl(&share.path, &req.entries)?;
            }
            if !req.default_entries.is_empty() {
                crate::acl::AclService::set_default_acl(&share.path, &req.default_entries)?;
            }
        }
        Ok::<_, crate::acl::AclError>(())
    })
    .await;
    match result {
        Ok(Ok(())) => ResponseHelper::json_success("ACL updated".to_string()),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to set ACL: {e}")),
        Err(e) => ResponseHelper::json_error(format!("ACL worker failed: {e}")),
    }
}

/// Flat matrix of every grant: one row per `(share, protocol, grantee)`.
/// The UI pivots it into a user × share table and reuses it for the
/// "all shares of this user" view.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_matrix(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let repo = crate::storage::SharePermissionRepository::new(state.db.pool());
    match repo.list().await {
        Ok(permissions) => ResponseHelper::json_success(
            permissions
                .into_iter()
                .map(|permission| SharePermissionResponse {
                    share_name: permission.share_name,
                    share_type: permission.share_type,
                    grantee_type: permission.grantee_type,
                    grantee_name: permission.grantee_name,
                    permission: permission.permission,
                    recursive: permission.recursive,
                })
                .collect::<Vec<_>>(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to list permissions: {e}")),
    }
}

/// CSV permission report (DSM's "Export Permissions Report"): one row per
/// grant, with the protocol and recursive flag. It deliberately contains no
/// password or token.
#[axum_macros::debug_handler]
pub async fn handle_share_permissions_report(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let repo = crate::storage::SharePermissionRepository::new(state.db.pool());
    let permissions = match repo.list().await {
        Ok(permissions) => permissions,
        Err(e) => return ResponseHelper::json_error(format!("Failed to build report: {e}")),
    };
    let mut csv = String::from("share,protocol,grantee_type,grantee_name,permission,recursive\n");
    for permission in permissions {
        let recursive = if permission.recursive { "yes" } else { "no" };
        // The share/grantee names were validated when they were written; the
        // CSV still quotes every field so a comma in a legacy row cannot
        // split the columns.
        csv.push_str(&format!(
            "\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\"\n",
            permission.share_name.replace('"', "\"\""),
            permission.share_type.replace('"', "\"\""),
            permission.grantee_type.replace('"', "\"\""),
            permission.grantee_name.replace('"', "\"\""),
            permission.permission.replace('"', "\"\""),
            recursive
        ));
    }
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            "text/csv; charset=utf-8",
        )
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            "attachment; filename=\"homevault-permissions.csv\"",
        )
        .body(axum::body::Body::from(csv))
        .unwrap_or_else(|_| ResponseHelper::json_error("Failed to build CSV".to_string()))
}

/// Effective permissions of one user, including group grants (the server-side
/// equivalent of DSM's Permission Inspector).
#[axum_macros::debug_handler]
pub async fn handle_user_permissions_effective(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match crate::handlers::admin_guard::require_auth(&state, &headers).await {
        Ok(caller) => caller,
        Err(resp) => return resp,
    };
    if caller != username && !state.auth.is_admin(&caller).await {
        return ResponseHelper::json_forbidden("Access denied".to_string());
    }
    match PermissionService::get_user_permissions(&username, state.db.pool()).await {
        Ok(permissions) => ResponseHelper::json_success(permissions),
        Err(e) => ResponseHelper::json_error(format!("Failed to resolve permissions: {e}")),
    }
}

/// Returns all share permissions for a given user (admin or self-only access).
#[axum_macros::debug_handler]
pub async fn handle_user_permissions(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match crate::handlers::admin_guard::require_auth(&state, &headers).await {
        Ok(caller) => caller,
        Err(resp) => return resp,
    };

    // Admin, or the account owner. A refusal is `403`, not `400`: the request
    // was well-formed, the caller simply may not read this account.
    if caller != username && !state.auth.is_admin(&caller).await {
        return ResponseHelper::json_forbidden("Access denied".to_string());
    }

    match PermissionService::get_user_permissions(&username, state.db.pool()).await {
        Ok(perms) => ResponseHelper::json_success(perms),
        Err(e) => ResponseHelper::json_error(format!("Failed to get permissions: {}", e)),
    }
}
