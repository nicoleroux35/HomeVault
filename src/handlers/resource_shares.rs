/* =============================================================================
 * File          : resource_shares.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-member resource access on top of the coarse role model: an admin
 *   may grant a member read/write access to one specific app, folder or
 *   share without making them an admin. Members see exactly the resources
 *   they hold a grant for; admins see and manage everything. The same rule
 *   is enforced server-side by visible_resources()/can_access() — filtering
 *   in the UI alone is not a boundary.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/shares/access (grant/revoke/list) in src/web/mod.rs; grants
 *   stored via ResourceShareRepository, checked by every listing and the
 *   WebSocket event fan-out.
 * =============================================================================
 */

//! Per-member resource access (apps, folders, shares).
//!
//! HomeVault's coarse model is role-based (`user` / `admin`). This module adds
//! the finer layer an appliance owner expects: "member `alice` may use app
//! `jellyfin`, open folder `/srv/media`, and mount share `photos`", without
//! making her an admin.
//!
//! The grants live in `resource_shares` (see
//! `storage/resource_share_repo.rs`). The rule enforced here and by every
//! consumer is:
//!
//!   - an **admin** (HomeVault's equivalent of the appliance owner) sees and
//!     manages everything, and needs no grant row;
//!   - a **member** sees exactly the `(kind, resource)` pairs they hold a row
//!     for — never more.
//!
//! `visible_resources()` / `can_access()` are the server-side filter that
//! listings (app catalog, explorer roots, share list) and the WebSocket event
//! fan-out must call. Filtering in the UI only is not a boundary.

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::resource_share_repo::{
    ResourceGroupShare, ResourceKind, ResourcePermission, ResourceShare, ResourceShareRepository,
};
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json as AxumJson};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Body of `POST /api/shares/access`.
#[derive(Debug, Deserialize)]
pub struct GrantRequest {
    pub member: String,
    /// `"app"`, `"folder"` or `"share"`.
    pub kind: String,
    pub resource: String,
    /// `"read"`, `"write"` or `"full"`. Defaults to `read`.
    #[serde(default)]
    pub permission: Option<String>,
}

/// Query of `DELETE /api/shares/access`.
#[derive(Debug, Deserialize)]
pub struct RevokeQuery {
    pub member: String,
    pub kind: String,
    pub resource: String,
}

/// Body of `POST /api/shares/access/groups`.
#[derive(Debug, Deserialize)]
pub struct GroupGrantRequest {
    /// HomeVault group display name (no `hv_` prefix), e.g. `family`.
    pub group: String,
    pub kind: String,
    pub resource: String,
    #[serde(default)]
    pub permission: Option<String>,
}

/// Query of `DELETE /api/shares/access/groups`.
#[derive(Debug, Deserialize)]
pub struct GroupRevokeQuery {
    pub group: String,
    pub kind: String,
    pub resource: String,
}

/// Query of `GET /api/shares/access/groups` (admin listing).
#[derive(Debug, Deserialize)]
pub struct GroupListQuery {
    #[serde(default)]
    pub group: Option<String>,
}

/// Query of `GET /api/shares/access` (admin listing).
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Narrow the listing to one member.
    #[serde(default)]
    pub member: Option<String>,
}

/// The caller's own effective access, as returned by
/// `GET /api/shares/access/mine`. `all` is `true` for an admin (the owner),
/// which is why the three lists can be empty while the member still sees
/// everything.
#[derive(Debug, Serialize)]
pub struct MyAccessResponse {
    pub username: String,
    pub is_admin: bool,
    pub all: bool,
    pub apps: Vec<ResourceShare>,
    pub folders: Vec<ResourceShare>,
    pub shares: Vec<ResourceShare>,
    /// Grants inherited from the member's groups (`resource_group_shares`,
    /// migration 051) — shown so the UI can explain *why* a resource is
    /// visible, and so revoking a direct row never looks like a bug.
    pub inherited: Vec<ResourceGroupShare>,
}

/// `GET /api/shares/access` — admin: every grant (optionally for one member).
pub async fn handle_list_access(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ResourceShareRepository::new(state.db.pool());
    match repo.list_all(q.member.as_deref()).await {
        Ok(grants) => ResponseHelper::json_success(grants),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}

/// `GET /api/shares/access/mine` — the caller's own effective access.
///
/// Admins get `all: true` and empty lists (they see everything implicitly);
/// members get exactly their grant rows, grouped by kind. This is the endpoint
/// the frontend uses to hide apps/folders/shares the account cannot use — and
/// because the same repository backs the server-side checks, the UI can never
/// widen access.
pub async fn handle_my_access(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let is_admin = state
        .auth
        .get_session(
            headers
                .get("x-session-token")
                .and_then(|h| h.to_str().ok())
                .unwrap_or(""),
        )
        .await
        .map(|s| s.is_admin)
        .unwrap_or(false);

    let repo = ResourceShareRepository::new(state.db.pool());
    let mut apps = Vec::new();
    let mut folders = Vec::new();
    let mut shares = Vec::new();
    let mut inherited = Vec::new();

    if !is_admin {
        for (kind, bucket) in [
            (ResourceKind::App, &mut apps),
            (ResourceKind::Folder, &mut folders),
            (ResourceKind::Share, &mut shares),
        ] {
            match repo.list_for_member(&username, Some(kind)).await {
                Ok(list) => *bucket = list,
                Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
            }
        }

        // Group-inherited grants (migration 051). Resolved from the member's
        // live OS groups, so a membership change takes effect on the next call
        // with nothing to invalidate.
        let groups = member_groups(&username).await;
        for group in &groups {
            match repo.list_groups_all(Some(group)).await {
                Ok(list) => inherited.extend(list),
                Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
            }
        }
    }

    ResponseHelper::json_success(MyAccessResponse {
        username,
        is_admin,
        all: is_admin,
        apps,
        folders,
        shares,
        inherited,
    })
}

/// `POST /api/shares/access` — admin: grant (or update) a member's access.
pub async fn handle_grant_access(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<GrantRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if payload.member.trim().is_empty() || payload.resource.trim().is_empty() {
        return ResponseHelper::json_error("member and resource are required".to_string());
    }
    let Some(kind) = ResourceKind::parse(&payload.kind) else {
        return ResponseHelper::json_error(
            "Invalid kind: must be app, folder or share".to_string(),
        );
    };
    let permission = match payload.permission.as_deref() {
        None => ResourcePermission::Read,
        Some(p) => match ResourcePermission::parse(p) {
            Some(p) => p,
            None => {
                return ResponseHelper::json_error(
                    "Invalid permission: must be read, write or full".to_string(),
                )
            }
        },
    };

    let repo = ResourceShareRepository::new(state.db.pool());
    match repo
        .upsert(
            &admin,
            payload.member.trim(),
            kind,
            payload.resource.trim(),
            permission,
        )
        .await
    {
        Ok(grant) => {
            crate::events::log_event(format!(
                "Admin '{}' granted '{}' {} access to {} '{}'",
                admin,
                grant.member,
                permission.as_str(),
                kind.as_str(),
                grant.resource
            ));
            ResponseHelper::json_success(grant)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to grant access: {}", e)),
    }
}

/// `DELETE /api/shares/access` — admin: revoke a member's access.
pub async fn handle_revoke_access(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<RevokeQuery>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let Some(kind) = ResourceKind::parse(&q.kind) else {
        return ResponseHelper::json_error(
            "Invalid kind: must be app, folder or share".to_string(),
        );
    };

    let repo = ResourceShareRepository::new(state.db.pool());
    match repo.revoke(&q.member, kind, &q.resource).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{}' revoked '{}' access to {} '{}'",
                admin,
                q.member,
                kind.as_str(),
                q.resource
            ));
            ResponseHelper::json_success(serde_json::json!({ "revoked": true }))
        }
        Ok(false) => ResponseHelper::json_error("No such grant".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke access: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Group-level grants (migration 051)
// ---------------------------------------------------------------------------

/// `GET /api/shares/access/groups` — admin: every group grant (optionally one group).
pub async fn handle_list_group_access(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<GroupListQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ResourceShareRepository::new(state.db.pool());
    match repo.list_groups_all(q.group.as_deref()).await {
        Ok(grants) => ResponseHelper::json_success(grants),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}

/// `POST /api/shares/access/groups` — admin: grant (or update) a group's access.
pub async fn handle_grant_group_access(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<GroupGrantRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if payload.group.trim().is_empty() || payload.resource.trim().is_empty() {
        return ResponseHelper::json_error("group and resource are required".to_string());
    }
    let Some(kind) = ResourceKind::parse(&payload.kind) else {
        return ResponseHelper::json_error(
            "Invalid kind: must be app, folder or share".to_string(),
        );
    };
    let permission = match payload.permission.as_deref() {
        None => ResourcePermission::Read,
        Some(p) => match ResourcePermission::parse(p) {
            Some(p) => p,
            None => {
                return ResponseHelper::json_error(
                    "Invalid permission: must be read, write or full".to_string(),
                )
            }
        },
    };

    let repo = ResourceShareRepository::new(state.db.pool());
    match repo
        .upsert_group(
            &admin,
            payload.group.trim(),
            kind,
            payload.resource.trim(),
            permission,
        )
        .await
    {
        Ok(grant) => {
            crate::events::log_event(format!(
                "Admin '{}' granted group '{}' {} access to {} '{}'",
                admin,
                grant.group_name,
                permission.as_str(),
                kind.as_str(),
                grant.resource
            ));
            ResponseHelper::json_success(grant)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to grant group access: {}", e)),
    }
}

/// `DELETE /api/shares/access/groups` — admin: revoke a group's access.
pub async fn handle_revoke_group_access(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<GroupRevokeQuery>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let Some(kind) = ResourceKind::parse(&q.kind) else {
        return ResponseHelper::json_error(
            "Invalid kind: must be app, folder or share".to_string(),
        );
    };

    let repo = ResourceShareRepository::new(state.db.pool());
    match repo.revoke_group(&q.group, kind, &q.resource).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{}' revoked group '{}' access to {} '{}'",
                admin,
                q.group,
                kind.as_str(),
                q.resource
            ));
            ResponseHelper::json_success(serde_json::json!({ "revoked": true }))
        }
        Ok(false) => ResponseHelper::json_error("No such group grant".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke group access: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Server-side filter helpers for other modules
// ---------------------------------------------------------------------------

/// The app ids `username` may use. `None` means "no filter" (admin/owner).
///
/// Callers that list apps should do:
/// ```ignore
/// match visible_app_ids(&state, &username).await {
///     Ok(Some(ids)) => apps.retain(|a| ids.contains(&a.id)),
///     Ok(None) => {} // admin: keep everything
///     Err(_) => return ResponseHelper::json_error(...),
/// }
/// ```
pub async fn visible_app_ids(
    state: &Arc<AppState>,
    username: &str,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    if is_admin(state, username).await {
        return Ok(None);
    }
    let rows = effective_visible_resources(state, username, ResourceKind::App).await?;
    Ok(Some(rows.into_iter().map(|(r, _)| r).collect()))
}

/// Whether `username` may see `resource` of `kind`. Admins always can.
pub async fn can_access(
    state: &Arc<AppState>,
    username: &str,
    kind: ResourceKind,
    resource: &str,
) -> bool {
    if is_admin(state, username).await {
        return true;
    }
    effective_visible_resources(state, username, kind)
        .await
        .map(|rows| {
            rows.into_iter().any(|(r, level)| {
                r == resource
                    && ResourcePermission::parse(&level)
                        .map(|p| p.implies(ResourcePermission::Read))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// The member's live OS group names (`id -nG`, filtered to HomeVault groups).
///
/// Blocking call, so it runs on the blocking pool — same pattern as
/// `transfer_quota` uses for group quotas. A failure is treated as "no
/// groups": the caller then sees only its direct grants, i.e. fails closed.
pub async fn member_groups(username: &str) -> Vec<String> {
    let owned = username.to_string();
    tokio::task::spawn_blocking(move || crate::groups::GroupService::get_user_groups(&owned))
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

/// The member's *effective* `resource -> permission` map for `kind`: direct
/// grants ∪ group grants, strongest level winning.
///
/// This is the one place the union rule lives, so every consumer (app
/// catalog, explorer roots, share list, WebSocket fan-out, `can_access`)
/// agrees. Nothing is denormalised into `resource_shares`, so leaving a group
/// revokes the inherited access on the very next call.
pub async fn effective_visible_resources(
    state: &Arc<AppState>,
    username: &str,
    kind: ResourceKind,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let repo = ResourceShareRepository::new(state.db.pool());
    let direct = repo
        .visible_resources(username, kind)
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let groups = member_groups(username).await;
    let inherited = repo
        .visible_resources_for_groups(&groups, kind)
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    Ok(ResourceShareRepository::merge_strongest(direct, inherited))
}

/// Whether `username` holds the admin role.
///
/// Delegates to `AuthService::is_admin`, the same predicate `require_admin`
/// uses, so a HomeVault account and a privileged host (PAM) account are
/// treated identically here and on every guarded endpoint (ROADMAP X.7).
async fn is_admin(state: &Arc<AppState>, username: &str) -> bool {
    state.auth.is_admin(username).await
}

/// Filter a WebSocket event payload for a member.
///
/// Events carry an optional `resource_kind` / `resource` pair. When present,
/// the event is only delivered to a member who can access that resource;
/// events without a resource tag are delivered to everyone (they are not
/// resource-scoped). This is the whitelist the event fan-out applies — a
/// member never receives an event about an app/folder/share they cannot see.
pub async fn event_visible_to(
    state: &Arc<AppState>,
    username: &str,
    event: &serde_json::Value,
) -> bool {
    let kind = event.get("resource_kind").and_then(|v| v.as_str());
    let resource = event.get("resource").and_then(|v| v.as_str());
    match (kind, resource) {
        (Some(k), Some(r)) => match ResourceKind::parse(k) {
            Some(kind) => can_access(state, username, kind, r).await,
            None => false,
        },
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_request_defaults_to_read() {
        let req: GrantRequest =
            serde_json::from_str(r#"{"member":"alice","kind":"app","resource":"jellyfin"}"#)
                .unwrap();
        assert_eq!(req.permission, None);
        assert_eq!(
            req.permission
                .as_deref()
                .map(ResourcePermission::parse)
                .unwrap_or(Some(ResourcePermission::Read)),
            Some(ResourcePermission::Read)
        );
    }

    #[test]
    fn test_grant_request_rejects_unknown_kind_via_parse() {
        let req: GrantRequest =
            serde_json::from_str(r#"{"member":"a","kind":"disk","resource":"x"}"#).unwrap();
        assert_eq!(ResourceKind::parse(&req.kind), None);
    }
}
