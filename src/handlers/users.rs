/* =============================================================================
 * File          : users.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin user-management endpoints: list, get, create, update and delete
 *   users, change passwords and roles (with host admin-group sync), lock or
 *   unlock accounts, force-disable 2FA, sync group memberships and manage
 *   home-folder settings and quotas. Listing, profile updates and password
 *   changes also accept the account owner; every mutation that touches
 *   another account is admin-only, and user creation is rate-limited
 *   (10 per minute per admin).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/users* in src/web/mod.rs; payload types live in
 *   users_models.rs.
 * =============================================================================
 */

//! Admin user-management endpoints (with self-service for one's own
//! profile, password and home folder).

use crate::groups::GroupService;
use crate::handlers::{users_models::*, AppState};
use crate::storage::user_home_repo::UserHomeRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

/// Query param controlling whether the home folder is deleted along with the user.
#[derive(Debug, Deserialize)]
pub struct DeleteUserQuery {
    #[serde(default)]
    pub delete_home_folder: bool,
}

/// Lists all users with their roles, group memberships, and home-folder status.
#[axum_macros::debug_handler]
pub async fn handle_users_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
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
                    let repo = UserHomeRepository::new(state.db.pool());
                    let mut user_list: Vec<UserListItem> = Vec::new();

                    for user in users {
                        let groups = GroupService::get_user_groups(&user.username)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|g| !GroupService::is_admin_group(g))
                            .collect::<Vec<_>>();

                        let home = repo.find_by_username(&user.username).await.ok().flatten();
                        let two_fa_enabled = state
                            .auth
                            .is_2fa_enabled(&user.username)
                            .await
                            .unwrap_or(false);

                        let role = user.role_name().to_string();
                        let username_clone = user.username.clone();
                        user_list.push(UserListItem {
                            username: user.username,
                            fullname: user.fullname,
                            email: user.email,
                            role,
                            role_id: user.role_id,
                            locked: user.locked,
                            created_at: user.created_at,
                            groups,
                            home_enabled: home.as_ref().map(|h| h.enabled).unwrap_or(false),
                            home_disk: home.as_ref().and_then(|h| h.disk.clone()),
                            home_path: home.as_ref().and_then(|h| h.path.clone()),
                            home_size_bytes: home.as_ref().and_then(|h| h.size_limit_bytes),
                            avatar_seed: user.avatar_seed.clone(),
                            avatar_url: format!("/api/users/{}/avatar.svg", username_clone),
                            two_fa_enabled,
                        });
                    }
                    ResponseHelper::json_success(user_list)
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to get users: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Gets details for a single user. Admins can see anyone; users can only see themselves.
#[axum_macros::debug_handler]
pub async fn handle_user_get(
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

            match state.auth.get_user(&username).await {
                Ok(user) => {
                    let groups = GroupService::get_user_groups(&user.username)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|g| !GroupService::is_admin_group(g))
                        .collect::<Vec<_>>();

                    let repo = UserHomeRepository::new(state.db.pool());
                    let home = repo.find_by_username(&username).await.ok().flatten();

                    let user_data = serde_json::json!({
                        "username": user.username,
                        "fullname": user.fullname,
                        "email": user.email,
                        "role": user.role_name().to_string(),
                        "role_id": user.role_id,
                        "locked": user.locked,
                        "created_at": user.created_at,
                        "groups": groups,
                        "home_enabled": home.as_ref().map(|h| h.enabled).unwrap_or(false),
                        "home_disk": home.as_ref().and_then(|h| h.disk.clone()),
                        "home_path": home.as_ref().and_then(|h| h.path.clone()),
                        "home_size_bytes": home.as_ref().and_then(|h| h.size_limit_bytes),
                    });
                    ResponseHelper::json_success(user_data)
                }
                Err(e) => ResponseHelper::json_error(format!("User not found: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Creates a new user with validation, rate-limiting, group assignment, and optional home folder.
#[axum_macros::debug_handler]
pub async fn handle_user_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<CreateUserRequest>,
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

            let rate_limit_key = format!("user_create:{}", session.username);
            if state
                .admin_rate_limiter
                .check_rate_limit(&rate_limit_key)
                .is_err()
            {
                return ResponseHelper::json_error(
                    "Rate limit exceeded. Maximum 10 user creations per minute.".to_string(),
                );
            }

            if let Err(e) = crate::setup::SetupService::validate_username(&req.username) {
                return ResponseHelper::json_error(e);
            }

            if let Err(e) = crate::setup::SetupService::validate_password(&req.password) {
                return ResponseHelper::json_error(e);
            }

            if let Err(e) = crate::setup::SetupService::validate_fullname(&req.fullname) {
                return ResponseHelper::json_error(e);
            }

            if !req.email.is_empty() {
                if !is_valid_email(&req.email) {
                    return ResponseHelper::json_error("Invalid email format".to_string());
                }
                if req.email.len() > 128 {
                    return ResponseHelper::json_error(
                        "Email cannot exceed 128 characters".to_string(),
                    );
                }
            }

            if req.role_id != 1 && req.role_id != 2 {
                return ResponseHelper::json_error(
                    "Invalid role_id: must be 1 (user) or 2 (admin)".to_string(),
                );
            }

            if let Some(sz) = req.home_size_bytes {
                if sz < 0 {
                    return ResponseHelper::json_error(
                        "Home directory size cannot be negative".to_string(),
                    );
                }
            }

            match state.auth.get_user_count().await {
                Ok(count) if count >= 10000 => {
                    return ResponseHelper::json_error(
                        "Maximum users limit (10000) reached".to_string(),
                    );
                }
                Err(e) => {
                    return ResponseHelper::json_error(format!(
                        "Failed to check user count: {}",
                        e
                    ));
                }
                _ => {}
            }

            for group_name in &req.groups {
                if GroupService::get_group(group_name).is_err() {
                    return ResponseHelper::json_error(format!(
                        "Group '{}' does not exist",
                        group_name
                    ));
                }
            }

            if req.groups.len() > 50 {
                return ResponseHelper::json_error("Maximum 50 groups per user".to_string());
            }

            // Create user in DB
            match state
                .auth
                .create_user(
                    req.username.clone(),
                    req.password.clone(),
                    req.fullname.clone(),
                    req.email.clone(),
                    req.role_id,
                )
                .await
            {
                Ok(_) => {
                    crate::events::log_event(format!(
                        "Admin '{}' created user '{}' (role: {})",
                        session.username,
                        req.username,
                        if req.role_id == crate::storage::models::ADMIN_ROLE_ID {
                            "admin"
                        } else {
                            "user"
                        }
                    ));

                    // Add user to requested groups (the reserved admin group is managed
                    // exclusively through the role selector below, not this checklist)
                    for group_name in &req.groups {
                        if !GroupService::is_admin_group(group_name) {
                            let _ = GroupService::add_user_to_group(&req.username, group_name);
                        }
                    }

                    // Grant real host sudo access when created directly as an admin,
                    // matching what promoting an existing user to admin already does.
                    if req.role_id == crate::storage::models::ADMIN_ROLE_ID {
                        let _ = state.db.sync_admin_user_to_group(&req.username).await;
                    }

                    // Handle home folder if requested
                    if req.home_enabled {
                        match super::user_homes::apply_home_folder(
                            &state,
                            &req.username,
                            true,
                            req.home_disk.as_deref(),
                            req.home_path.as_deref(),
                            req.home_size_bytes,
                        )
                        .await
                        {
                            Ok(Some(warning)) => {
                                tracing::warn!(
                                    "Home folder for user '{}': {}",
                                    req.username,
                                    warning
                                );
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to create home folder for user '{}': {}",
                                    req.username,
                                    e
                                );
                            }
                        }
                    }

                    ResponseHelper::json_success("User created successfully".to_string())
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to create user: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

fn is_valid_email(email: &str) -> bool {
    if email.is_empty() || email.len() > 254 {
        return false;
    }

    let parts: Vec<&str> = email.split('@').collect();
    if parts.len() != 2 {
        return false;
    }

    let local = parts[0];
    let domain = parts[1];

    if local.is_empty() || local.len() > 64 {
        return false;
    }

    if domain.is_empty() || !domain.contains('.') {
        return false;
    }

    for c in email.chars() {
        if !c.is_ascii_alphanumeric() && !matches!(c, '@' | '.' | '_' | '-' | '+') {
            return false;
        }
    }

    true
}

/// Updates a user's profile (fullname, email). Users can update themselves; admins can update anyone.
#[axum_macros::debug_handler]
pub async fn handle_user_update(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateUserRequest>,
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

            match state
                .auth
                .update_user(&username, req.fullname.clone(), req.email.clone())
                .await
            {
                Ok(_) => ResponseHelper::json_success("User updated successfully".to_string()),
                Err(e) => ResponseHelper::json_error(format!("Failed to update user: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Admin-only: updates user profile, home-folder settings in one call.
#[axum_macros::debug_handler]
pub async fn handle_user_update_full(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateUserFullRequest>,
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

            // Update user profile
            if let Err(e) = state
                .auth
                .update_user(&username, req.fullname.clone(), req.email.clone())
                .await
            {
                return ResponseHelper::json_error(format!("Failed to update user: {}", e));
            }

            if let Some(sz) = req.home_size_bytes {
                if sz < 0 {
                    return ResponseHelper::json_error(
                        "Home directory size cannot be negative".to_string(),
                    );
                }
            }

            // Update home folder
            match super::user_homes::apply_home_folder(
                &state,
                &username,
                req.home_enabled,
                req.home_disk.as_deref(),
                req.home_path.as_deref(),
                req.home_size_bytes,
            )
            .await
            {
                Ok(warning) => ResponseHelper::json_success(
                    warning.unwrap_or_else(|| "User updated successfully".to_string()),
                ),
                Err(e) => ResponseHelper::json_error(format!("Failed to update user: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Reconciles a user's group membership: adds missing groups, removes extraneous ones.
#[axum_macros::debug_handler]
pub async fn handle_user_update_groups(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateGroupMembershipRequest>,
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

            // Get current groups and sync
            let current_groups = GroupService::get_user_groups(&username).unwrap_or_default();

            // `current_groups` (from `id -nG`) includes every group the
            // account is really in — the user's own primary group, `docker`,
            // `sudo`, anything — not just HomeVault-managed ones. This
            // checklist only ever offers HomeVault-managed groups, so it must
            // never try to remove membership in anything outside that set;
            // otherwise a group list that happens to omit e.g. the user's own
            // primary group (never offered as a checkbox in the first place)
            // would make this endpoint attempt to strip it.
            let managed_groups: std::collections::HashSet<String> = GroupService::list_groups()
                .unwrap_or_default()
                .into_iter()
                .map(|g| g.display_name)
                .collect();

            // Collected rather than discarded: a HomeVault account with no
            // real Linux user (the common case for an ordinary account with
            // no admin role and no home folder) makes every usermod/gpasswd
            // call below fail, and silently swallowing that here previously
            // made this endpoint report success while changing nothing at
            // all on disk — confirmed live against a real DB-only account.
            let mut errors: Vec<String> = Vec::new();

            for g in &current_groups {
                if managed_groups.contains(g)
                    && !req.groups.contains(g)
                    && !GroupService::is_admin_group(g)
                {
                    if let Err(e) = GroupService::remove_user_from_group(&username, g) {
                        errors.push(e.to_string());
                    }
                }
            }

            // If we're about to add the user to at least one group, make
            // sure a real Linux account exists first — see
            // `GroupService::ensure_linux_user`. A pure removal (or a save
            // with nothing new selected) never needs one.
            let will_add = req
                .groups
                .iter()
                .any(|g| !current_groups.contains(g) && !GroupService::is_admin_group(g));
            let mut provisioning_failed = false;
            if will_add {
                if let Err(e) = GroupService::ensure_linux_user(&username) {
                    errors.push(e.to_string());
                    provisioning_failed = true;
                }
            }

            if !provisioning_failed {
                for g in &req.groups {
                    if !current_groups.contains(g) && !GroupService::is_admin_group(g) {
                        if let Err(e) = GroupService::add_user_to_group(&username, g) {
                            errors.push(e.to_string());
                        }
                    }
                }
            }

            if !errors.is_empty() {
                return ResponseHelper::json_error(format!(
                    "Failed to update group membership: {}",
                    errors.join("; ")
                ));
            }

            crate::events::log_event(format!(
                "Admin '{}' updated group membership for user '{}' (groups: {})",
                session.username,
                username,
                if req.groups.is_empty() {
                    "none".to_string()
                } else {
                    req.groups.join(", ")
                }
            ));

            ResponseHelper::json_success("Group membership updated".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Updates a user's password. Admins can set without current password; users must confirm theirs.
#[axum_macros::debug_handler]
pub async fn handle_user_update_password(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdatePasswordRequest>,
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

            let current_password = if session.is_admin {
                None
            } else {
                // Self-service change by a non-admin: a stolen session token
                // alone must not be enough to permanently take over the
                // account, so the current password is mandatory here (unlike
                // an admin resetting someone else's password, which stays
                // exempt above).
                match req.current_password.as_deref() {
                    Some(pwd) if !pwd.is_empty() => Some(pwd),
                    _ => {
                        return ResponseHelper::json_error(
                            "Current password is required".to_string(),
                        )
                    }
                }
            };

            match state
                .auth
                .update_password(&username, current_password, &req.new_password)
                .await
            {
                Ok(_) => {
                    if session.is_admin && session.username != username {
                        crate::events::log_event(format!(
                            "Admin '{}' changed password for user '{}'",
                            session.username, username
                        ));
                    } else {
                        crate::events::log_event(format!(
                            "User '{}' changed their password",
                            username
                        ));
                    }
                    ResponseHelper::json_success("Password updated successfully".to_string())
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to update password: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Changes a user's role. Includes lockout protection: prevents removing the last admin.
#[axum_macros::debug_handler]
pub async fn handle_user_update_role(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateRoleRequest>,
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

            if req.role_id != 1 && req.role_id != 2 {
                return ResponseHelper::json_error(
                    "Invalid role_id: must be 1 (user) or 2 (admin)".to_string(),
                );
            }

            if session.username == username && req.role_id != crate::storage::models::ADMIN_ROLE_ID
            {
                return ResponseHelper::json_error(
                    "Cannot remove your own admin privileges".to_string(),
                );
            }

            let target = match state.auth.get_user(&username).await {
                Ok(u) => u,
                Err(_) => {
                    return ResponseHelper::json_error(format!("User '{}' not found", username));
                }
            };

            // Only a currently-admin target demoted to a non-admin role can
            // actually reduce the admin count — checking this unconditionally
            // for any non-self, non-admin `role_id` (the previous behavior)
            // wrongly blocked a no-op update on an already-regular user
            // whenever the system had exactly one admin. Confirmed by
            // `tests/users_integration.rs::test_user_update_role_noop_on_regular_user_blocked_by_last_admin_guard_bug`.
            if req.role_id != crate::storage::models::ADMIN_ROLE_ID && target.is_admin() {
                match state.auth.count_admins().await {
                    Ok(admin_count) if admin_count <= 1 => {
                        return ResponseHelper::json_error(
                            "Cannot remove last admin. Promote another user to admin first."
                                .to_string(),
                        );
                    }
                    Err(e) => {
                        return ResponseHelper::json_error(format!(
                            "Failed to check admin count: {}",
                            e
                        ));
                    }
                    _ => {}
                }
            }

            match state.auth.update_role(&username, req.role_id).await {
                Ok(_) => {
                    crate::events::log_event_warn(format!(
                        "Admin '{}' changed the role of user '{}' to {}",
                        session.username,
                        username,
                        if req.role_id == crate::storage::models::ADMIN_ROLE_ID {
                            "admin"
                        } else {
                            "user"
                        }
                    ));

                    if req.role_id == crate::storage::models::ADMIN_ROLE_ID {
                        let _ = state.db.sync_admin_user_to_group(&username).await;
                    } else {
                        // Mirror the promotion above: demoting a user must also
                        // revoke the real Linux sudo access granted by
                        // sync_admin_user_to_group, not just the DB role_id —
                        // otherwise the account keeps real host-level admin
                        // rights the app itself no longer believes it has.
                        let _ = GroupService::remove_user_from_group(
                            &username,
                            crate::storage::database::ADMIN_GROUP_NAME,
                        );
                    }

                    ResponseHelper::json_success("Role updated successfully".to_string())
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to update role: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Locks or unlocks a user account. Locked users cannot authenticate.
#[axum_macros::debug_handler]
pub async fn handle_user_set_locked(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<SetLockedRequest>,
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

            // Locking is one-way from the admin's own perspective: there is no
            // recovery path other than another admin unlocking you, so refuse
            // anything that could leave zero admins able to log in.
            if req.locked {
                if session.username == username {
                    return ResponseHelper::json_error("Cannot lock your own account".to_string());
                }

                let target = match state.auth.get_user(&username).await {
                    Ok(u) => u,
                    Err(_) => {
                        return ResponseHelper::json_error(format!(
                            "User '{}' not found",
                            username
                        ));
                    }
                };

                if target.is_admin() {
                    match state.auth.get_all_users().await {
                        Ok(users) => {
                            let other_active_admins = users
                                .iter()
                                .filter(|u| u.is_admin() && !u.locked && u.username != username)
                                .count();
                            if other_active_admins == 0 {
                                return ResponseHelper::json_error(
                                    "Cannot lock the last active admin. Unlock or promote another user to admin first."
                                        .to_string(),
                                );
                            }
                        }
                        Err(e) => {
                            return ResponseHelper::json_error(format!(
                                "Failed to check admin count: {}",
                                e
                            ));
                        }
                    }
                }
            }

            match state.auth.set_user_locked(&username, req.locked).await {
                Ok(_) => {
                    let msg = if req.locked {
                        "User locked"
                    } else {
                        "User unlocked"
                    };
                    crate::events::log_event_warn(format!(
                        "Admin '{}' {} user '{}'",
                        session.username,
                        if req.locked { "locked" } else { "unlocked" },
                        username
                    ));
                    ResponseHelper::json_success(msg.to_string())
                }
                Err(e) => {
                    ResponseHelper::json_error(format!("Failed to update lock status: {}", e))
                }
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Admin-only account recovery: force-disables a user's 2FA without knowing
/// their password. Needed when a user loses their authenticator and has no
/// backup codes, since the self-service disable flow requires the password.
#[axum_macros::debug_handler]
pub async fn handle_admin_disable_2fa(
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
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }

            match state.auth.disable_2fa(&username).await {
                Ok(_) => {
                    crate::events::log_event_warn(format!(
                        "Admin '{}' disabled 2FA for user '{}'",
                        session.username, username
                    ));
                    ResponseHelper::json_success(serde_json::json!({ "enabled": false }))
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to disable 2FA: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Permanently deletes a user, invalidates sessions, removes group memberships, and optionally deletes the home folder.
///
/// Guards against self-deletion and deletion of the last remaining admin.
#[axum_macros::debug_handler]
pub async fn handle_user_delete(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<DeleteUserQuery>,
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

            if session.username == username {
                return ResponseHelper::json_error("Cannot delete your own account".to_string());
            }

            let user = match state.auth.get_user(&username).await {
                Ok(u) => u,
                Err(_) => {
                    return ResponseHelper::json_error(format!("User '{}' not found", username));
                }
            };

            if user.is_admin() {
                match state.auth.count_admins().await {
                    Ok(admin_count) if admin_count <= 1 => {
                        return ResponseHelper::json_error(
                            "Cannot delete the last admin. Promote another user first.".to_string(),
                        );
                    }
                    Err(e) => {
                        return ResponseHelper::json_error(format!(
                            "Failed to check admin count: {}",
                            e
                        ));
                    }
                    _ => {}
                }
            }

            if let Err(e) = state.auth.invalidate_user_sessions(&username).await {
                tracing::warn!("Failed to invalidate sessions for '{}': {}", username, e);
            }

            // The account is being permanently removed: don't leave its shell
            // (if any) running as an orphan behind it.
            state.pty_manager.kill_session(&username).await;

            let groups = GroupService::get_user_groups(&username).unwrap_or_default();
            for g in groups {
                let _ = GroupService::remove_user_from_group(&username, &g);
            }

            // Revoke the ACL permissions granted *directly* to this user, on
            // the filesystem and in `share_permissions` (group permissions are
            // left intact — the user was just removed from its groups above,
            // and the remaining members must keep theirs). Without this the
            // rows outlive the account and a future account recreated with the
            // same username silently inherits the deleted user's access.
            use crate::permissions::PermissionService;
            match PermissionService::revoke_all_user_permissions(&username, state.db.pool()).await {
                Ok(0) => {}
                Ok(revoked) => tracing::info!(
                    "Revoked {} permission(s) for deleted user '{}'",
                    revoked,
                    username
                ),
                Err(e) => tracing::warn!(
                    "Failed to revoke share permissions for user '{}': {}",
                    username,
                    e
                ),
            }

            if query.delete_home_folder {
                let repo = UserHomeRepository::new(state.db.pool());
                if let Ok(Some(home)) = repo.find_by_username(&username).await {
                    if let Some(ref path) = home.path {
                        match tokio::fs::remove_dir_all(path).await {
                            Ok(_) => {
                                tracing::info!(
                                    "Deleted home folder '{}' for user '{}'",
                                    path,
                                    username
                                );
                            }
                            Err(e) => {
                                tracing::warn!("Failed to delete home folder '{}': {}", path, e);
                            }
                        }
                    }
                }
                let _ = repo.delete(&username).await;
            }

            match state.auth.delete_user(&username).await {
                Ok(_) => {
                    // Clean up any configured transfer quota so a future
                    // account created with this same username doesn't
                    // silently inherit a stale limit/usage from a
                    // completely different, deleted account.
                    let quota_repo =
                        crate::storage::transfer_quota_repo::TransferQuotaRepository::new(
                            state.db.pool(),
                        );
                    let _ = quota_repo.delete("user", &username).await;

                    // Same reasoning as the transfer-quota cleanup just
                    // above: without this, a deleted user's SFTP chroot
                    // jail (bind mount + fstab entry + authorized_keys)
                    // stays behind indefinitely, and a future account
                    // created with this same username would silently
                    // inherit it. Best-effort by design, matching every
                    // other cleanup call in this branch.
                    crate::sftp::SftpJailService::purge_user(&username);

                    // Recents are personal data too: a recreated same-name
                    // account must not inherit the deleted user's locations.
                    let recent_repo =
                        crate::storage::RecentRepository::new(state.db.pool());
                    let _ = recent_repo.purge_user(&username).await;

                    crate::events::log_event_warn(format!(
                        "Admin '{}' deleted user '{}'{}",
                        session.username,
                        username,
                        if query.delete_home_folder {
                            " (including home folder)"
                        } else {
                            ""
                        }
                    ));

                    ResponseHelper::json_success("User deleted successfully".to_string())
                }
                Err(e) => ResponseHelper::json_error(format!("Failed to delete user: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Returns the total number of registered users (admin only).
#[axum_macros::debug_handler]
pub async fn handle_user_count(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
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

            match state.auth.get_user_count().await {
                Ok(count) => ResponseHelper::json_success(UserCountResponse { count }),
                Err(e) => ResponseHelper::json_error(format!("Failed to get user count: {}", e)),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

/// Checks whether a given username already exists (admin only).
#[axum_macros::debug_handler]
pub async fn handle_user_exists(
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
            if !session.is_admin {
                return ResponseHelper::json_error("Admin access required".to_string());
            }

            let exists = state.auth.user_exists(&username).await;
            ResponseHelper::json_success(serde_json::json!({ "exists": exists }))
        }
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_email_accepts_a_plain_address() {
        assert!(is_valid_email("user@example.com"));
    }

    #[test]
    fn test_is_valid_email_accepts_allowed_special_characters() {
        assert!(is_valid_email("user.name+tag@example.co.uk"));
        assert!(is_valid_email("first_last-2@sub.example.com"));
    }

    #[test]
    fn test_is_valid_email_rejects_empty() {
        assert!(!is_valid_email(""));
    }

    #[test]
    fn test_is_valid_email_rejects_over_254_chars() {
        let local = "a".repeat(60);
        let domain_label = "b".repeat(190);
        let email = format!("{}@{}.com", local, domain_label);
        assert!(email.len() > 254);
        assert!(!is_valid_email(&email));
    }

    #[test]
    fn test_is_valid_email_rejects_missing_at_sign() {
        assert!(!is_valid_email("user.example.com"));
    }

    #[test]
    fn test_is_valid_email_rejects_multiple_at_signs() {
        assert!(!is_valid_email("user@middle@example.com"));
    }

    #[test]
    fn test_is_valid_email_rejects_empty_local_part() {
        assert!(!is_valid_email("@example.com"));
    }

    #[test]
    fn test_is_valid_email_rejects_local_part_over_64_chars() {
        let local = "a".repeat(65);
        assert!(!is_valid_email(&format!("{}@example.com", local)));
    }

    #[test]
    fn test_is_valid_email_accepts_local_part_at_64_chars() {
        let local = "a".repeat(64);
        assert!(is_valid_email(&format!("{}@example.com", local)));
    }

    #[test]
    fn test_is_valid_email_rejects_empty_domain() {
        assert!(!is_valid_email("user@"));
    }

    #[test]
    fn test_is_valid_email_rejects_domain_without_dot() {
        assert!(!is_valid_email("user@localhost"));
    }

    #[test]
    fn test_is_valid_email_rejects_disallowed_characters() {
        assert!(!is_valid_email("user name@example.com"));
        assert!(!is_valid_email("user!@example.com"));
        assert!(!is_valid_email("user#tag@example.com"));
    }

    #[test]
    fn test_is_valid_email_rejects_non_ascii() {
        assert!(!is_valid_email("üser@example.com"));
    }
}
