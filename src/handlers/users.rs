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

use crate::groups::{run_group_op, GroupService};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::{users_models::*, AppState};
use crate::host_accounts::HostAccountService;
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
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let users = match state.auth.get_all_users().await {
        Ok(users) => users,
        Err(e) => return ResponseHelper::json_error(format!("Failed to get users: {}", e)),
    };

    // Resolve every account's memberships in a single blocking hop: the
    // per-row `id -nG` used to run straight on the async runtime, once per
    // user, so a slow NSS backend stalled the request (and its Tokio worker)
    // for every account in the list.
    let usernames: Vec<String> = users.iter().map(|u| u.username.clone()).collect();
    let mut groups_by_user = match run_group_op("list user groups", move || {
        Ok(GroupService::get_users_groups(&usernames))
    })
    .await
    {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!("Could not resolve group memberships: {}", e);
            std::collections::HashMap::new()
        }
    };

    let repo = UserHomeRepository::new(state.db.pool());
    let mut user_list: Vec<UserListItem> = Vec::new();

    for user in users {
        let groups = groups_by_user
            .remove(&user.username)
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
        let is_host_account = user.is_host_mirror();
        user_list.push(UserListItem {
            username: user.username,
            fullname: user.fullname,
            email: user.email,
            role,
            role_id: user.role_id,
            locked: user.locked,
            expires_at: user.expires_at,
            created_at: user.created_at,
            groups,
            home_enabled: home.as_ref().map(|h| h.enabled).unwrap_or(false),
            home_disk: home.as_ref().and_then(|h| h.disk.clone()),
            home_path: home.as_ref().and_then(|h| h.path.clone()),
            home_size_bytes: home.as_ref().and_then(|h| h.size_limit_bytes),
            avatar_seed: user.avatar_seed.clone(),
            avatar_url: format!("/api/users/{}/avatar.svg", username_clone),
            two_fa_enabled,
            is_host_account,
        });
    }

    ResponseHelper::json_success(user_list)
}
/// Gets details for a single user. Admins can see anyone; users can only see themselves.
#[axum_macros::debug_handler]
pub async fn handle_user_get(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if caller != username && !state.auth.is_admin(&caller).await {
        return ResponseHelper::json_forbidden("Access denied".to_string());
    }

    let user = match state.auth.get_user(&username).await {
        Ok(user) => user,
        Err(e) => return ResponseHelper::json_error(format!("User not found: {}", e)),
    };

    let lookup = user.username.clone();
    let groups = match run_group_op("list user groups", move || {
        GroupService::get_user_groups(&lookup)
    })
    .await
    {
        Ok(groups) => groups,
        Err(e) => {
            tracing::warn!("Could not resolve groups for '{}': {}", username, e);
            Vec::new()
        }
    }
    .into_iter()
    .filter(|g| !GroupService::is_admin_group(g))
    .collect::<Vec<_>>();

    let repo = UserHomeRepository::new(state.db.pool());
    let home = repo.find_by_username(&username).await.ok().flatten();
    let is_host_account = user.is_host_mirror();

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
        "is_host_account": is_host_account,
    });
    ResponseHelper::json_success(user_data)
}
/// Creates a new user with validation, rate-limiting, group assignment, and optional home folder.
#[axum_macros::debug_handler]
pub async fn handle_user_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<CreateUserRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let rate_limit_key = format!("user_create:{}", admin_username);
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
            return ResponseHelper::json_error("Email cannot exceed 128 characters".to_string());
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

    // The cap itself lives in `AuthService::create_user` (100 accounts); the
    // old pre-check here advertised 10 000 and could never fire.
    let requested_groups = match GroupService::normalize_member_list(&req.groups) {
        Ok(groups) => groups,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if requested_groups.len() > 50 {
        return ResponseHelper::json_error("Maximum 50 groups per user".to_string());
    }

    // Validate every group name before creating anything: a typo must not
    // leave a half-provisioned account behind.
    let groups_to_validate = requested_groups.clone();
    let requested_groups = match run_group_op("validate groups", move || {
        for name in &groups_to_validate {
            GroupService::get_group(name)?;
        }
        Ok(groups_to_validate)
    })
    .await
    {
        Ok(groups) => groups,
        Err(crate::groups::GroupError::NotFound(name)) => {
            return ResponseHelper::json_error(format!("Group '{}' does not exist", name))
        }
        Err(e) => return ResponseHelper::json_error(format!("Failed to validate groups: {}", e)),
    };

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
                admin_username,
                req.username,
                if req.role_id == crate::storage::models::ADMIN_ROLE_ID {
                    "admin"
                } else {
                    "user"
                }
            ));

            // Add user to requested groups (the reserved admin group is managed
            // exclusively through the role selector below, not this checklist).
            // One blocking hop for the whole list, and every failure is logged
            // instead of silently discarded: an account reported as created but
            // missing its groups is exactly the drift this endpoint must not
            // hide.
            let username_for_groups = req.username.clone();
            let groups_for_user = requested_groups.clone();
            if let Err(e) = run_group_op("assign groups", move || {
                for group_name in &groups_for_user {
                    if GroupService::is_admin_group(group_name) {
                        continue;
                    }
                    if let Err(e) =
                        GroupService::add_user_to_group(&username_for_groups, group_name)
                    {
                        tracing::warn!(
                            "User '{}' created but not added to group '{}': {}",
                            username_for_groups,
                            group_name,
                            e
                        );
                    }
                }
                Ok(())
            })
            .await
            {
                tracing::warn!(
                    "User '{}' created but its groups could not be applied: {}",
                    req.username,
                    e
                );
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
                        tracing::warn!("Home folder for user '{}': {}", req.username, warning);
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
    let caller = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if caller != username && !state.auth.is_admin(&caller).await {
        return ResponseHelper::json_forbidden("Access denied".to_string());
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
/// Admin-only: updates user profile, home-folder settings in one call.
#[axum_macros::debug_handler]
pub async fn handle_user_update_full(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateUserFullRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
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
/// Reconciles a user's group membership: adds missing groups, removes extraneous ones.
#[axum_macros::debug_handler]
pub async fn handle_user_update_groups(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateGroupMembershipRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let requested = match GroupService::normalize_member_list(&req.groups) {
        Ok(groups) => groups,
        Err(e) => return ResponseHelper::json_error(e),
    };

    // Every `usermod`/`gpasswd` below is blocking; the whole reconciliation
    // runs in one blocking hop under a hard ceiling instead of pinning a
    // Tokio worker per command.
    let target = username.clone();
    let requested_for_op = requested.clone();
    let outcome = run_group_op("update group membership", move || {
        // `current_groups` (from `id -nG`) includes every group the account is
        // really in — the user's own primary group, `docker`, `sudo`, anything
        // — not just HomeVault-managed ones. This checklist only ever offers
        // HomeVault-managed groups, so it must never try to remove membership
        // in anything outside that set; otherwise a group list that happens to
        // omit e.g. the user's own primary group (never offered as a checkbox
        // in the first place) would make this endpoint attempt to strip it.
        let current_groups = GroupService::get_user_groups(&target).unwrap_or_default();

        let managed_groups: std::collections::HashSet<String> = GroupService::list_groups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.display_name)
            .collect();

        // A name that is neither a HomeVault group nor the reserved admin
        // group cannot be granted: report it instead of letting `usermod`
        // fail with a raw stderr line.
        for group in &requested_for_op {
            if !managed_groups.contains(group) && !GroupService::is_admin_group(group) {
                return Err(crate::groups::GroupError::NotFound(group.clone()));
            }
        }

        // Collected rather than discarded: a HomeVault account with no
        // real Linux user (the common case for an ordinary account with
        // no admin role and no home folder) makes every usermod/gpasswd
        // call below fail, and silently swallowing that here previously
        // made this endpoint report success while changing nothing at
        // all on disk — confirmed live against a real DB-only account.
        let mut errors: Vec<String> = Vec::new();

        for g in &current_groups {
            if managed_groups.contains(g)
                && !requested_for_op.contains(g)
                && !GroupService::is_admin_group(g)
            {
                if let Err(e) = GroupService::remove_user_from_group(&target, g) {
                    errors.push(e.to_string());
                }
            }
        }

        // If we're about to add the user to at least one group, make
        // sure a real Linux account exists first — see
        // `GroupService::ensure_linux_user`. A pure removal (or a save
        // with nothing new selected) never needs one.
        let will_add = requested_for_op
            .iter()
            .any(|g| !current_groups.contains(g) && !GroupService::is_admin_group(g));
        let mut provisioning_failed = false;
        if will_add {
            if let Err(e) = GroupService::ensure_linux_user(&target) {
                errors.push(e.to_string());
                provisioning_failed = true;
            }
        }

        if !provisioning_failed {
            for g in &requested_for_op {
                if !current_groups.contains(g) && !GroupService::is_admin_group(g) {
                    if let Err(e) = GroupService::add_user_to_group(&target, g) {
                        errors.push(e.to_string());
                    }
                }
            }
        }

        Ok(errors)
    })
    .await;

    let errors = match outcome {
        Ok(errors) => errors,
        Err(crate::groups::GroupError::NotFound(group)) => {
            return ResponseHelper::json_error(format!("Group '{}' does not exist", group))
        }
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to update group membership: {}", e))
        }
    };

    if !errors.is_empty() {
        return ResponseHelper::json_error(format!(
            "Failed to update group membership: {}",
            errors.join("; ")
        ));
    }

    crate::events::log_event(format!(
        "Admin '{}' updated group membership for user '{}' (groups: {})",
        admin_username,
        username,
        if requested.is_empty() {
            "none".to_string()
        } else {
            requested.join(", ")
        }
    ));

    ResponseHelper::json_success("Group membership updated".to_string())
}

/// Mask a live session token down to an audit reference: the admin can spot
/// "which session is which" between two list calls, but cannot replay it.
fn mask_session_token(token: &str) -> String {
    let mut masked: String = token.chars().take(8).collect();
    masked.push('…');
    masked
}

/// Admin view of a target user's active sessions (audit + incident tooling);
/// pairs with the bulk revoke below — surgical revocation of one particular
/// device would need the full token, and the admin must never hold a usable
/// credential for somebody else's session, so the read masks to 8 chars + `…`.
#[axum_macros::debug_handler]
pub async fn handle_admin_user_sessions(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let user_repo = crate::storage::user_repo::UserRepository::new(state.db.pool());
    if user_repo
        .find_by_username(&username)
        .await
        .is_ok_and(|u| u.is_none())
    {
        return ResponseHelper::json_error(format!("User '{}' not found", username));
    }

    let session_repo = crate::storage::session_repo::SessionRepository::new(state.db.pool());
    let sessions = match session_repo.find_all_by_username(&username).await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to fetch sessions: {}", e)),
    };

    let infos: Vec<AdminSessionInfo> = sessions
        .into_iter()
        .map(|s| AdminSessionInfo {
            id: mask_session_token(&s.token),
            ip_address: s.ip_address,
            created_at: s.created_at.to_rfc3339(),
            expires_at: s.expires_at.to_rfc3339(),
        })
        .collect();
    let total = infos.len();
    ResponseHelper::json_success(AdminUserSessionsResponse {
        sessions: infos,
        total,
    })
}
/// Set (or clear, `null` = never) the target user's expiry horizon
/// (`users.expires_at`, migration 050). Admin-only; deliberately a separate
/// endpoint from the profile update so a profile save can never shift the
/// horizon as a side effect. Takes effect on the next sign-in attempt
/// immediately, and on already-held sessions at the next sweep (boot +
/// periodic loop calling `AuthService::revoke_expired_account_sessions`).
#[axum_macros::debug_handler]
pub async fn handle_user_set_expiry(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<SetExpiryRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let user_repo = crate::storage::user_repo::UserRepository::new(state.db.pool());
    if user_repo
        .find_by_username(&username)
        .await
        .is_ok_and(|u| u.is_none())
    {
        return ResponseHelper::json_error(format!("User '{}' not found", username));
    }

    let parsed = match req.expires_at.as_deref() {
        None | Some("") => Ok(None),
        Some(raw) => match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(dt) if dt >= chrono::Utc::now() => Ok(Some(dt.with_timezone(&chrono::Utc))),
            // A horizon already in the past would only make the
            // account expire with no warning — refuse it so the
            // mistake surfaces in the UI instead.
            Ok(_) => Err("Expiry must be in the future".to_string()),
            Err(_) => {
                Err("Invalid timestamp (expected RFC 3339, e.g. 2027-01-31T00:00:00Z)".to_string())
            }
        },
    };
    let expires_at = match parsed {
        Ok(v) => v,
        Err(msg) => return ResponseHelper::json_error(msg),
    };

    match user_repo.update_expires_at(&username, expires_at).await {
        Ok(()) => {
            match expires_at {
                Some(at) => crate::events::log_event(format!(
                    "Admin '{}' set user '{}' to expire at {}",
                    admin_username,
                    username,
                    at.to_rfc3339()
                )),
                None => crate::events::log_event(format!(
                    "Admin '{}' cleared the expiry of user '{}'",
                    admin_username, username
                )),
            }
            ResponseHelper::json_success(serde_json::json!({
                "expires_at": expires_at.map(|dt| dt.to_rfc3339()),
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to set expiry: {}", e)),
    }
}
/// Force logout everywhere for a target user: revokes ALL of their sessions
/// (including their current one — the admin's purpose here is incident
/// response, not politeness; the user can sign back in at any time).
#[axum_macros::debug_handler]
pub async fn handle_admin_revoke_user_sessions(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let session_repo = crate::storage::session_repo::SessionRepository::new(state.db.pool());
    // delete_by_username returns no count; enumerate the target's
    // tokens and delete_many them instead, so the event log and the
    // response carry a revoked number.
    let revoked = match session_repo.find_all_by_username(&username).await {
        Ok(sessions) => {
            let tokens: Vec<String> = sessions.into_iter().map(|s| s.token).collect();
            session_repo.delete_many(&tokens).await
        }
        Err(e) => Err(e),
    };
    match revoked {
        Ok(deleted) => {
            crate::events::log_event(format!(
                "Admin '{}' revoked all {} session(s) of user '{}'",
                admin_username, deleted, username
            ));
            ResponseHelper::json_success(serde_json::json!({ "revoked": deleted }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke sessions: {}", e)),
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
    let caller = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let caller_is_admin = state.auth.is_admin(&caller).await;
    if !caller_is_admin && caller != username {
        return ResponseHelper::json_forbidden("Access denied".to_string());
    }

    let current_password = if caller_is_admin {
        None
    } else {
        // Self-service change by a non-admin: a stolen session token
        // alone must not be enough to permanently take over the
        // account, so the current password is mandatory here (unlike
        // an admin resetting someone else's password, which stays
        // exempt above).
        match req.current_password.as_deref() {
            Some(pwd) if !pwd.is_empty() => Some(pwd),
            _ => return ResponseHelper::json_error("Current password is required".to_string()),
        }
    };

    match state
        .auth
        .update_password(
            &username,
            current_password,
            &req.new_password,
            // An admin resetting someone else's password chose (or let
            // the server generate) an out-of-band secret: its owner
            // must change it at the next sign-in. A self change — the
            // current password was verified or the session is the
            // admin's own — is owner-chosen, which also clears any
            // pending requirement.
            caller_is_admin && caller != username,
        )
        .await
    {
        Ok(_) => {
            if caller_is_admin && caller != username {
                crate::events::log_event(format!(
                    "Admin '{}' changed password for user '{}'",
                    caller, username
                ));
            } else {
                crate::events::log_event(format!("User '{}' changed their password", username));
            }
            ResponseHelper::json_success("Password updated successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update password: {}", e)),
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if req.role_id != 1 && req.role_id != 2 {
        return ResponseHelper::json_error(
            "Invalid role_id: must be 1 (user) or 2 (admin)".to_string(),
        );
    }

    if admin_username == username && req.role_id != crate::storage::models::ADMIN_ROLE_ID {
        return ResponseHelper::json_error("Cannot remove your own admin privileges".to_string());
    }

    let target = match state.auth.get_user(&username).await {
        Ok(u) => u,
        Err(_) => return ResponseHelper::json_error(format!("User '{}' not found", username)),
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
                    "Cannot remove last admin. Promote another user to admin first.".to_string(),
                );
            }
            Err(e) => {
                return ResponseHelper::json_error(format!("Failed to check admin count: {}", e));
            }
            _ => {}
        }
    }

    // `update_role` owns the host side now: it revokes
    // `homevault_admin`/`sudo`/`wheel` *before* rewriting the role (so a
    // failed revoke leaves the two in agreement), provisions the account on
    // promotion, and signs the demoted user out so their session's stale
    // `is_admin` snapshot cannot be used. Calling the host sync a second time
    // here was a duplicate that only ever cleared `homevault_admin`.
    match state.auth.update_role(&username, req.role_id).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' changed the role of user '{}' to {}",
                admin_username,
                username,
                if req.role_id == crate::storage::models::ADMIN_ROLE_ID {
                    "admin"
                } else {
                    "user"
                }
            ));
            ResponseHelper::json_success("Role updated successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update role: {}", e)),
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    // Locking is one-way from the admin's own perspective: there is no
    // recovery path other than another admin unlocking you, so refuse
    // anything that could leave zero admins able to log in.
    if req.locked {
        if admin_username == username {
            return ResponseHelper::json_error("Cannot lock your own account".to_string());
        }

        let target = match state.auth.get_user(&username).await {
            Ok(u) => u,
            Err(_) => return ResponseHelper::json_error(format!("User '{}' not found", username)),
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
                admin_username,
                if req.locked { "locked" } else { "unlocked" },
                username
            ));
            ResponseHelper::json_success(msg.to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update lock status: {}", e)),
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    match state.auth.disable_2fa(&username).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' disabled 2FA for user '{}'",
                admin_username, username
            ));
            ResponseHelper::json_success(serde_json::json!({ "enabled": false }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to disable 2FA: {}", e)),
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if admin_username == username {
        return ResponseHelper::json_error("Cannot delete your own account".to_string());
    }

    let user = match state.auth.get_user(&username).await {
        Ok(u) => u,
        Err(_) => return ResponseHelper::json_error(format!("User '{}' not found", username)),
    };

    if user.is_admin() {
        match state.auth.count_admins().await {
            Ok(admin_count) if admin_count <= 1 => {
                return ResponseHelper::json_error(
                    "Cannot delete the last admin. Promote another user first.".to_string(),
                );
            }
            Err(e) => {
                return ResponseHelper::json_error(format!("Failed to check admin count: {}", e));
            }
            _ => {}
        }
    }

    let host_account_exists = HostAccountService::exists(&username).await;

    // A host (PAM) account's credential lives in `/etc/shadow` and predates
    // HomeVault: it was not created by this page, and removing the Linux
    // account would destroy an OS account the admin never asked HomeVault to
    // own. Deleting only the mirror row would be worse — `authenticate`
    // recreates it at the next sign-in, with its role re-derived from host
    // group membership. So refuse while the account is still there, and let
    // the admin decide on the server.
    if user.is_host_mirror() && host_account_exists {
        return ResponseHelper::json_error(format!(
            "'{}' is a Linux host account, not a HomeVault one. Remove it on the server (userdel {}) before deleting it here.",
            username, username
        ));
    }

    if let Err(e) = state.auth.invalidate_user_sessions(&username).await {
        tracing::warn!("Failed to invalidate sessions for '{}': {}", username, e);
    }

    // The account is being permanently removed: don't leave its shell
    // (if any) running as an orphan behind it.
    state.pty_manager.kill_session(&username).await;

    // Remove the Linux account before the database row, so a refusal (the
    // account is still logged in, `userdel` lacks privileges) leaves both
    // sides consistent and the operation retryable. Until 2026-10-21 nothing
    // removed it at all: a deleted admin kept a working host login — and, via
    // `homevault_admin` membership, a working HomeVault *admin* login that the
    // next PAM sign-in silently recreated.
    if host_account_exists {
        if let Err(e) = HostAccountService::delete_account(&username).await {
            return ResponseHelper::json_error(format!(
                "Could not remove the Linux account for '{}': {}. The HomeVault account was kept so the removal can be retried.",
                username, e
            ));
        }
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

    // Home folder: remove the files only when asked, but always drop the
    // configuration row. A recreated same-name account must not inherit the
    // deleted one's disk/path/quota settings (same reasoning as the transfer
    // quota and recents cleanup below).
    let home_repo = UserHomeRepository::new(state.db.pool());
    let recorded_home = home_repo
        .find_by_username(&username)
        .await
        .ok()
        .flatten()
        .and_then(|home| home.path)
        .filter(|path| !path.trim().is_empty())
        .map(std::path::PathBuf::from);

    if query.delete_home_folder {
        // The account's Linux home is only known here when no folder was
        // configured explicitly (`useradd -m -d /home/homevault/<user>`);
        // otherwise the recorded path wins.
        let default_home = HostAccountService::home_dir(&username);
        let path = recorded_home.clone().or_else(|| {
            if default_home.is_dir() {
                Some(default_home)
            } else {
                None
            }
        });

        match path {
            Some(path) => match tokio::fs::remove_dir_all(&path).await {
                Ok(_) => {
                    tracing::info!(
                        "Deleted home folder '{}' for user '{}'",
                        path.display(),
                        username
                    )
                }
                Err(e) => {
                    tracing::warn!("Failed to delete home folder '{}': {}", path.display(), e);
                }
            },
            None => tracing::info!("User '{}' had no home folder to delete", username),
        }
    }

    let _ = home_repo.delete(&username).await;

    match state.auth.delete_user(&username).await {
        Ok(_) => {
            // Clean up any configured transfer quota so a future
            // account created with this same username doesn't
            // silently inherit a stale limit/usage from a
            // completely different, deleted account.
            let quota_repo =
                crate::storage::transfer_quota_repo::TransferQuotaRepository::new(state.db.pool());
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
            let recent_repo = crate::storage::RecentRepository::new(state.db.pool());
            let _ = recent_repo.purge_user(&username).await;

            crate::events::log_event_warn(format!(
                "Admin '{}' deleted user '{}'{}",
                admin_username,
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
/// Returns the total number of registered users (admin only).
#[axum_macros::debug_handler]
pub async fn handle_user_count(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    match state.auth.get_user_count().await {
        Ok(count) => ResponseHelper::json_success(UserCountResponse { count }),
        Err(e) => ResponseHelper::json_error(format!("Failed to get user count: {}", e)),
    }
}
/// Checks whether a given username already exists (admin only).
#[axum_macros::debug_handler]
pub async fn handle_user_exists(
    State(state): State<Arc<AppState>>,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let exists = state.auth.user_exists(&username).await;
    ResponseHelper::json_success(serde_json::json!({ "exists": exists }))
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
