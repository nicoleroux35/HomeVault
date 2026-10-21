/* =============================================================================
 * File          : groups.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Endpoints managing the server's Unix groups through GroupService: list
 *   groups and their members, create and delete groups, add and remove
 *   member users, and list a user's current groups. Listing is open to any
 *   authenticated user; creating, deleting and changing membership is
 *   admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Routes /api/groups* in src/web/mod.rs; group creation/deletion maps to
 *   real host groups (see src/groups.rs).
 * =============================================================================
 */

//! Unix group management endpoints (list, create, delete, membership) on
//! top of the host's groups via GroupService.

use crate::groups::{
    run_group_op, AddMemberRequest, CreateGroupRequest, GroupService, UpdateGroupMetaRequest,
};
use crate::handlers::admin_guard::{require_admin, require_auth};
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
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    match state.auth.get_all_users().await {
        Ok(users) => {
            let list: Vec<_> = users.into_iter().map(|u| u.username).collect();
            ResponseHelper::json_success(list)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list users: {}", e)),
    }
}
/// Lists all system groups (read, admin, or any user).
#[axum_macros::debug_handler]
pub async fn handle_groups_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    match run_group_op("list groups", GroupService::list_groups).await {
        Ok(mut groups) => {
            // Descriptions live in the `hv_groups` mirror only (the OS group
            // database has no such field). A group created straight on the
            // host simply has none — never an error.
            let repo = crate::storage::group_repo::HvGroupRepository::new(state.db.pool());
            if let Ok(rows) = repo.list_non_system().await {
                for group in groups.iter_mut() {
                    group.description = rows
                        .iter()
                        .find(|r| r.name == group.display_name)
                        .and_then(|r| r.description.clone());
                }
            }
            ResponseHelper::json_success(groups)
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
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    match run_group_op("get group", move || GroupService::get_group(&name)).await {
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let members = match GroupService::normalize_member_list(&req.members) {
        Ok(members) => members,
        Err(e) => return ResponseHelper::json_error(e),
    };

    // Every member must be a real HomeVault account. `create_group` provisions
    // a Linux account for each one, so an unchecked name would let a typo (or
    // a deliberate request) create an OS account that has no HomeVault user
    // behind it — and no way to manage it from this page.
    for member in &members {
        if !state.auth.user_exists(member).await {
            return ResponseHelper::json_error(format!("User '{}' not found", member));
        }
    }

    let name = req.name.trim().to_string();
    let members_for_op = members.clone();
    let outcome = run_group_op("create group", move || {
        GroupService::create_group(&name, &members_for_op)
    })
    .await;

    match outcome {
        Ok(group) => {
            // Mirror the group in `hv_groups` so it has a description slot and
            // a stable row for renames. Best-effort: the OS group already
            // exists and must not be rolled back over a mirror hiccup.
            let repo = crate::storage::group_repo::HvGroupRepository::new(state.db.pool());
            if let Err(e) = repo
                .create(req.name.trim(), req.description.as_deref())
                .await
            {
                tracing::warn!(
                    "Group '{}' created on the host but not mirrored in hv_groups: {}",
                    req.name,
                    e
                );
            }

            crate::events::log_event(format!(
                "Admin '{}' created group '{}'",
                admin_username, req.name
            ));
            ResponseHelper::json_success(group)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create group: {}", e)),
    }
}
/// Renames a group and/or sets its description (2026-10-20).
///
/// A rename is not just `groupmod -n`: everything that keys on the group's
/// **display name** has to follow, or the group silently loses access it was
/// granted. Migrated here, in one place, after the OS rename succeeded:
///
///   - `hv_groups` mirror (name + full_name, gid preserved),
///   - `resource_group_shares` (group-level resource grants, migration 051),
///   - `transfer_quotas` (`subject = 'group'`, so a rename never resurrects a
///     default limit or leaks the old one's usage),
///   - `share_permissions` (`grantee_type = 'group'`, the SMB/NFS ACL grants).
///
/// The admin group is refused: it is the passwordless-sudo grant, and
/// renaming it would leave the sudoers rule dangling.
#[axum_macros::debug_handler]
pub async fn handle_group_update(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateGroupMetaRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let old_display = GroupService::get_display_name(&name);
    if GroupService::is_admin_group(&old_display) {
        return ResponseHelper::json_error("The admin group cannot be renamed".to_string());
    }

    // The group must exist on the host before anything is written: a
    // description-only call would otherwise create an `hv_groups` row for a
    // group that is not there (and then fail on the read-back).
    let lookup = old_display.clone();
    match run_group_op("get group", move || GroupService::get_group(&lookup)).await {
        Ok(_) => {}
        Err(e) => return ResponseHelper::json_error(format!("Group not found: {}", e)),
    }

    let mut current_display = old_display.clone();

    // 1. Rename first: if the OS refuses, nothing else must have changed.
    if let Some(target) = req.name.as_deref() {
        let target = target.trim();
        if !target.is_empty() && target != old_display {
            let old_for_op = old_display.clone();
            let new_for_op = target.to_string();
            match run_group_op("rename group", move || {
                GroupService::rename_group(&old_for_op, &new_for_op)
            })
            .await
            {
                Ok(_) => {
                    current_display = GroupService::get_display_name(target);
                    crate::events::log_event(format!(
                        "Admin '{}' renamed group '{}' to '{}'",
                        admin_username, old_display, current_display
                    ));
                }
                Err(e) => {
                    return ResponseHelper::json_error(format!("Failed to rename group: {}", e))
                }
            }

            // 2. Follow the display-name references. Each is best-effort and
            // logged: the OS rename already happened, so a partial failure
            // here is an inconsistency to surface, not a rollback.
            let old_name = old_display.clone();
            let new_name = current_display.clone();
            let repo = crate::storage::group_repo::HvGroupRepository::new(state.db.pool());
            if let Err(e) = repo.rename(&old_name, &new_name).await {
                tracing::warn!(
                    "hv_groups mirror rename '{}' → '{}' failed: {}",
                    old_name,
                    new_name,
                    e
                );
            }

            let resource_repo =
                crate::storage::resource_share_repo::ResourceShareRepository::new(state.db.pool());
            if let Err(e) = resource_repo.rename_group_name(&old_name, &new_name).await {
                tracing::warn!(
                    "resource_group_shares rename '{}' → '{}' failed: {}",
                    old_name,
                    new_name,
                    e
                );
            }

            if let Err(e) = sqlx::query(
                "UPDATE transfer_quotas SET subject_name = ? WHERE subject_type = 'group' AND subject_name = ?",
            )
            .bind(&new_name)
            .bind(&old_name)
            .execute(state.db.pool().as_ref())
            .await
            {
                tracing::warn!(
                    "transfer_quotas rename '{}' → '{}' failed: {}",
                    old_name,
                    new_name,
                    e
                );
            }

            if let Err(e) = sqlx::query(
                "UPDATE share_permissions SET grantee_name = ? WHERE grantee_type = 'group' AND grantee_name = ?",
            )
            .bind(&new_name)
            .bind(&old_name)
            .execute(state.db.pool().as_ref())
            .await
            {
                tracing::warn!(
                    "share_permissions rename '{}' → '{}' failed: {}",
                    old_name,
                    new_name,
                    e
                );
            }
        }
    }

    // 3. Description: `None` = leave alone, `Some("")` = clear.
    if let Some(description) = req.description.as_deref() {
        let value = if description.trim().is_empty() {
            None
        } else {
            Some(description.trim())
        };
        let repo = crate::storage::group_repo::HvGroupRepository::new(state.db.pool());
        match repo.update_description(&current_display, value).await {
            Ok(true) => {}
            Ok(false) => {
                // No mirror row (group created straight on the host): create
                // one so the description has somewhere to live.
                if let Err(e) = repo.create(&current_display, value).await {
                    return ResponseHelper::json_error(format!(
                        "Failed to store the description: {}",
                        e
                    ));
                }
            }
            Err(e) => {
                return ResponseHelper::json_error(format!(
                    "Failed to store the description: {}",
                    e
                ))
            }
        }
    }

    let final_name = current_display.clone();
    match run_group_op("get group", move || GroupService::get_group(&final_name)).await {
        Ok(group) => ResponseHelper::json_success(group),
        Err(e) => ResponseHelper::json_error(format!("Group not found: {}", e)),
    }
}
/// Deletes a group. The admin group is protected and cannot be removed.
#[axum_macros::debug_handler]
pub async fn handle_group_delete(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let full_group_name = GroupService::get_full_group_name(&name);

    // Prevent deletion of the protected admin group
    if full_group_name == ADMIN_GROUP_NAME {
        return ResponseHelper::json_error("The admin group cannot be deleted".to_string());
    }

    let name_for_op = name.clone();
    match run_group_op("delete group", move || {
        GroupService::delete_group(&name_for_op)
    })
    .await
    {
        Ok(_) => {
            // Every table that keys on the group's *display name* must follow
            // the OS delete, or a later group created with the same name
            // silently inherits the deleted one's access and settings.
            if let Err(e) = purge_group_references(
                state.db.pool().as_ref(),
                &GroupService::get_display_name(&name),
            )
            .await
            {
                tracing::warn!(
                    "Group '{}' deleted but some references could not be purged: {}",
                    name,
                    e
                );
            }

            crate::events::log_event_warn(format!(
                "Admin '{}' deleted group '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Group deleted successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete group: {}", e)),
    }
}

/// Drop every database reference keyed on a group's display name.
///
/// Runs after the OS delete in [`handle_group_delete`]. Split out so the
/// cleanup itself is testable without a real `groupdel` (which needs root):
///
///   - `transfer_quotas` (`subject_type = 'group'`),
///   - `resource_group_shares` (group-level resource grants, migration 051),
///   - `share_permissions` (`grantee_type = 'group'`, the SMB/NFS ACL grants),
///   - `hv_groups` (the description mirror).
///
/// A rename migrates the same four references (`handle_group_update`); this is
/// the delete half of that invariant.
pub async fn purge_group_references(
    pool: &sqlx::SqlitePool,
    display_name: &str,
) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    let quota_repo = crate::storage::transfer_quota_repo::TransferQuotaRepository::new(
        std::sync::Arc::new(pool.clone()),
    );
    if let Err(e) = quota_repo.delete("group", display_name).await {
        failures.push(format!("transfer_quotas: {}", e));
    }

    let resource_repo = crate::storage::resource_share_repo::ResourceShareRepository::new(
        std::sync::Arc::new(pool.clone()),
    );
    if let Err(e) = resource_repo.revoke_all_for_group(display_name).await {
        failures.push(format!("resource_group_shares: {}", e));
    }

    if let Err(e) = sqlx::query(
        "DELETE FROM share_permissions WHERE grantee_type = 'group' AND grantee_name = ?",
    )
    .bind(display_name)
    .execute(pool)
    .await
    {
        failures.push(format!("share_permissions: {}", e));
    }

    let group_repo =
        crate::storage::group_repo::HvGroupRepository::new(std::sync::Arc::new(pool.clone()));
    if let Err(e) = group_repo.delete(display_name).await {
        failures.push(format!("hv_groups: {}", e));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let username = req.username.trim().to_string();
    if username.is_empty() {
        return ResponseHelper::json_error("Username cannot be empty".to_string());
    }

    // The member must be a HomeVault account: adding an arbitrary name would
    // provision a Linux account (`ensure_linux_user`) with no user behind it.
    if !state.auth.user_exists(&username).await {
        return ResponseHelper::json_error(format!("User '{}' not found", username));
    }

    let full_group_name = GroupService::get_full_group_name(&group_name);

    if full_group_name == ADMIN_GROUP_NAME && !state.auth.is_admin(&username).await {
        return ResponseHelper::json_error(
            "Only users with admin role can be added to the admin group".to_string(),
        );
    }

    let group_for_op = group_name.clone();
    let user_for_op = username.clone();
    match run_group_op("add group member", move || {
        GroupService::ensure_linux_user(&user_for_op)?;
        GroupService::add_user_to_group(&user_for_op, &group_for_op)
    })
    .await
    {
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
    let admin_username = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let full_group_name = GroupService::get_full_group_name(&group_name);

    // Admin-group membership is derived from each user's DB role_id (see
    // handle_user_update_role's sync, which is the only path that ever *adds*
    // someone here) and handle_group_add_member above already refuses to add
    // anyone who isn't already a DB admin. Removing directly through this
    // generic endpoint would strip real host sudo while leaving the DB role
    // (and therefore the Users page's admin badge and the user's own session
    // privileges) unchanged, permanently desyncing the two pages with none of
    // handle_user_update_role's last-admin/self-demotion protections. Demote
    // via the Users page's role toggle instead, which keeps both in sync.
    if full_group_name == ADMIN_GROUP_NAME {
        return ResponseHelper::json_error(
            "Cannot remove a member from the admin group here — change their role from the Users page instead".to_string(),
        );
    }

    let group_for_op = group_name.clone();
    let user_for_op = username.clone();
    match run_group_op("remove group member", move || {
        GroupService::remove_user_from_group(&user_for_op, &group_for_op)
    })
    .await
    {
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
    let caller = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    // A member's group list is only theirs to read: another account's
    // memberships are an admin concern (`require_admin`), matching the rest of
    // this module. Listing the groups themselves stays open to any signed-in
    // user (`handle_groups_list`).
    if caller != username && !state.auth.is_admin(&caller).await {
        return ResponseHelper::json_forbidden("Access denied".to_string());
    }

    let lookup = username.clone();
    match run_group_op("list user groups", move || {
        GroupService::get_user_groups(&lookup)
    })
    .await
    {
        Ok(groups) => ResponseHelper::json_success(groups),
        Err(e) => ResponseHelper::json_error(format!("Failed to get user groups: {}", e)),
    }
}
