use crate::groups::{AddMemberRequest, CreateGroupRequest, GroupListItem, GroupService};
use crate::handlers::AppState;
use crate::storage::database::ADMIN_GROUP_NAME;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use std::sync::Arc;

#[axum_macros::debug_handler]
pub async fn handle_groups_list_users(
    State(state): State<Arc<AppState>>,
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
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }

            match state.auth.get_all_users().await {
                Ok(users) => {
                    let list: Vec<_> = users.into_iter().map(|u| u.username).collect();
                    ResponseHelper::json_success(list)
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to list users: {}", e)),
            }
        }
        Err(_) => ResponseHelper::json_unauthorized("Invalid session".to_string()),
    }
}

/// Lists all system groups (read, admin, or any user).
#[axum_macros::debug_handler]
pub async fn handle_groups_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if state.auth.get_session(session_token).await.is_err() {
        return ResponseHelper::json_unauthorized("Invalid session".to_string());
    }

    match GroupService::list_groups() {
        Ok(groups) => {
            let response: Vec<GroupListItem> = groups;
            ResponseHelper::json_success(response)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list groups: {}", e)),
    }
}

/// Returns details of a single group by name.
#[axum_macros::debug_handler]
pub async fn handle_group_get(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if state.auth.get_session(session_token).await.is_err() {
        return ResponseHelper::json_unauthorized("Invalid session".to_string());
    }

    match GroupService::get_group(&name) {
        Ok(group) => ResponseHelper::json_success(group),
        Err(e) => ResponseHelper::json_error(format!("Group not found: {}", e)),
    }
}

/// Creates a new system group with the given members.
#[axum_macros::debug_handler]
pub async fn handle_group_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<CreateGroupRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let admin_username = match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }
            session.username
        }
        Err(_) => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    match GroupService::create_group(&req.name, &req.members) {
        Ok(group) => {
            crate::events::log_event(format!(
                "Admin '{}' created group '{}'",
                admin_username, req.name
            ));
            ResponseHelper::json_success(group)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create group: {}", e)),
    }
}

/// Deletes a group. The admin group is protected and cannot be removed.
#[axum_macros::debug_handler]
pub async fn handle_group_delete(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let admin_username = match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }
            session.username
        }
        Err(_) => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let full_group_name = GroupService::get_full_group_name(&name);

    // Prevent deletion of the protected admin group
    if full_group_name == ADMIN_GROUP_NAME {
        return ResponseHelper::json_error("The admin group cannot be deleted".to_string());
    }

    match GroupService::delete_group(&name) {
        Ok(_) => {
            // Clean up any configured transfer quota so a future group
            // created with this same display name doesn't silently
            // inherit a stale limit/usage from a completely different,
            // deleted group.
            let quota_repo =
                crate::storage::transfer_quota_repo::TransferQuotaRepository::new(state.db.pool());
            let _ = quota_repo.delete("group", &name).await;

            crate::events::log_event_warn(format!(
                "Admin '{}' deleted group '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Group deleted successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete group: {}", e)),
    }
}

/// Adds a user to a group. Admin-group membership requires admin role.
#[axum_macros::debug_handler]
pub async fn handle_group_add_member(
    State(state): State<Arc<AppState>>,
    Path(group_name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<AddMemberRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    // Every caller needs a real, valid admin session — not just the
    // admin-group branch below. This mirrors handle_group_remove_member's
    // bar; previously this check only ran for the admin group, so a
    // non-empty-but-invalid token could add a user to any other group.
    let admin_username = match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }
            session.username
        }
        Err(_) => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let username = &req.username;
    let full_group_name = GroupService::get_full_group_name(&group_name);

    if full_group_name == ADMIN_GROUP_NAME {
        let is_admin = state.auth.is_admin(username).await;
        if !is_admin {
            return ResponseHelper::json_error(
                "Only users with admin role can be added to the admin group".to_string(),
            );
        }
    }

    if let Err(e) = GroupService::ensure_linux_user(username) {
        return ResponseHelper::json_error(format!(
            "Failed to provision Linux account for '{}': {}",
            username, e
        ));
    }

    match GroupService::add_user_to_group(username, &group_name) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' added user '{}' to group '{}'",
                admin_username, username, group_name
            ));
            ResponseHelper::json_success("User added to group".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to add user to group: {}", e)),
    }
}

/// Removes a user from a group (admin only).
#[axum_macros::debug_handler]
pub async fn handle_group_remove_member(
    State(state): State<Arc<AppState>>,
    Path((group_name, username)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let admin_username = match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }
            session.username
        }
        Err(_) => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let full_group_name = GroupService::get_full_group_name(&group_name);

    // Admin-group membership is derived from each user's DB role_id (see
    // handle_user_update_role's sync_admin_user_to_group call, which is the
    // only path that ever *adds* someone here) and handle_group_add_member
    // above already refuses to add anyone who isn't already a DB admin.
    // Removing directly through this generic endpoint would strip real host
    // sudo while leaving the DB role (and therefore the Users page's admin
    // badge and the user's own session privileges) unchanged, permanently
    // desyncing the two pages with none of handle_user_update_role's
    // last-admin/self-demotion protections. Demote via the Users page's
    // role toggle instead, which keeps both in sync.
    if full_group_name == ADMIN_GROUP_NAME {
        return ResponseHelper::json_error(
            "Cannot remove a member from the admin group here — change their role from the Users page instead".to_string(),
        );
    }

    match GroupService::remove_user_from_group(&username, &group_name) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' removed user '{}' from group '{}'",
                admin_username, username, group_name
            ));
            ResponseHelper::json_success("User removed from group".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove user from group: {}", e)),
    }
}

/// Returns the group names a given user belongs to.
#[axum_macros::debug_handler]
pub async fn handle_user_groups(
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

    if state.auth.get_session(session_token).await.is_err() {
        return ResponseHelper::json_unauthorized("Invalid session".to_string());
    }

    match GroupService::get_user_groups(&username) {
        Ok(groups) => ResponseHelper::json_success(groups),
        Err(e) => ResponseHelper::json_error(format!("Failed to get user groups: {}", e)),
    }
}
