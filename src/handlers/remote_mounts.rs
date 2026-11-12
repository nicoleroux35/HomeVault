/* =============================================================================
 * File          : remote_mounts.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   CIFS/NFS network-location endpoints for the Explorer: list, add, edit,
 *   remove, mount and unmount remote shares.
 *
 *   Every read and write needs an **admin** session: a mount runs root-owned
 *   `mount`, exposes a remote filesystem to the whole appliance and carries a
 *   credential. Listing them is admin-only too, so a member cannot enumerate
 *   the other NAS hosts this appliance talks to.
 *
 *   Per `docs/API.md`'s capability contract: a refusal about the *request* is
 *   `400`, a missing `mount.cifs`/`mount.nfs` is `503`, and an unexpected host
 *   failure is `500` with the tool's own stderr.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::remote_mounts, crate::handlers::AppState
 *
 * Usage:
 *   Routes under /api/remote-mounts in src/web/mod.rs; see
 *   docs/guides/remote-mounts.md.
 * =============================================================================
 */

//! CIFS/NFS network-location endpoints, see docs/guides/remote-mounts.md.
//!
//! * `GET    /api/remote-mounts`              — configured locations + state
//! * `POST   /api/remote-mounts`              — add one (admin)
//! * `PUT    /api/remote-mounts/{name}`       — edit one (admin)
//! * `DELETE /api/remote-mounts/{name}`       — forget one (admin)
//! * `POST   /api/remote-mounts/{name}/mount` — mount it (admin)
//! * `POST   /api/remote-mounts/{name}/unmount` — unmount it (admin)
//! * `POST   /api/remote-mounts/reconcile`    — mount every enabled one (admin)

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;

use super::AppState;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::remote_mounts::{self, RemoteKind, RemoteMount};
use crate::utils::ResponseHelper;

/// The body of a create/edit request.
///
/// `password` is write-only: it is never returned, and leaving it out on an
/// edit keeps the stored one (typing a password again just to change a mount
/// option would be a worse trade than an explicit "change password" field).
#[derive(Debug, Default, Deserialize)]
pub struct RemoteMountRequest {
    /// Slug used as the folder name. Ignored on edit (the path names it).
    #[serde(default)]
    pub name: String,
    /// `cifs`/`smb` or `nfs`; defaults to `cifs`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub server: String,
    /// CIFS share name or NFS export path.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Mount it immediately after saving.
    #[serde(default)]
    pub mount_now: bool,
}

impl RemoteMountRequest {
    fn into_mount(self, name: String, previous: Option<&RemoteMount>) -> RemoteMount {
        let kind = self
            .kind
            .as_deref()
            .and_then(RemoteKind::parse)
            .or_else(|| previous.map(|p| p.kind))
            .unwrap_or_default();
        RemoteMount {
            name,
            kind,
            server: if self.server.trim().is_empty() {
                previous.map(|p| p.server.clone()).unwrap_or_default()
            } else {
                self.server
            },
            source: if self.source.trim().is_empty() {
                previous.map(|p| p.source.clone()).unwrap_or_default()
            } else {
                self.source
            },
            // A blank field means "keep" on edit and "none" on create.
            username: match self.username {
                Some(value) if !value.trim().is_empty() => Some(value),
                Some(_) => None,
                None => previous.and_then(|p| p.username.clone()),
            },
            domain: match self.domain {
                Some(value) if !value.trim().is_empty() => Some(value),
                Some(_) => None,
                None => previous.and_then(|p| p.domain.clone()),
            },
            options: if self.options.is_empty() {
                previous.map(|p| p.options.clone()).unwrap_or_default()
            } else {
                self.options
            },
            read_only: self
                .read_only
                .or_else(|| previous.map(|p| p.read_only))
                .unwrap_or(true),
            enabled: self
                .enabled
                .or_else(|| previous.map(|p| p.enabled))
                .unwrap_or(true),
            created_at: previous.map(|p| p.created_at.clone()).unwrap_or_default(),
            last_error: None,
        }
    }
}

/// `GET /api/remote-mounts` — every configured location, its live state and
/// which protocols this host can actually mount.
pub async fn handle_remote_mounts_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    // Only an admin may learn which remote hosts this appliance mounts.
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let (kinds, mounts) = match tokio::task::spawn_blocking(|| {
        (remote_mounts::available_kinds(), remote_mounts::list())
    })
    .await
    {
        Ok(pair) => pair,
        Err(e) => {
            return ResponseHelper::json_internal_error(format!("List worker failed: {e}"));
        }
    };

    ResponseHelper::json_success(serde_json::json!({
        "available": !kinds.is_empty(),
        "kinds": kinds,
        "root": remote_mounts::REMOTE_ROOT,
        "mounts": mounts,
    }))
}

/// `POST /api/remote-mounts` — add a network location.
pub async fn handle_remote_mount_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    // The guard runs before the body is parsed, so an unauthenticated caller
    // can never probe the payload shape.
    let req: RemoteMountRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    let name = match remote_mounts::validate_name(&req.name) {
        Ok(name) => name,
        Err(message) => return ResponseHelper::json_error(message),
    };
    if remote_mounts::load().iter().any(|m| m.name == name) {
        return ResponseHelper::json_error(format!(
            "A network location named '{name}' already exists"
        ));
    }

    let password = req.password.clone().filter(|p| !p.is_empty());
    let mount_now = req.mount_now;
    let mut mount = req.into_mount(name.clone(), None);
    // Validate the request before the capability probe: an injected server,
    // share or option is a `400` even when the protocol helper is absent
    // (otherwise a host without `mount.cifs` answers `503` and hides the
    // real input error).
    if let Err(message) = remote_mounts::validate_fields(&mut mount) {
        return ResponseHelper::json_error(message);
    }
    if !remote_mounts::kind_available(mount.kind) {
        return ResponseHelper::json_unavailable(format!(
            "{} is not installed on this server",
            mount.kind.helper()
        ));
    }

    let result = tokio::task::spawn_blocking(move || {
        let view = remote_mounts::upsert(mount, password.as_deref())?;
        if mount_now {
            return remote_mounts::mount_one(&name);
        }
        Ok(view)
    })
    .await;

    match result {
        Ok(Ok(view)) => {
            crate::events::log_event(format!(
                "Admin '{admin}' added the {} network location '{}' ({}:{})",
                view.mount.kind.as_str(),
                view.mount.name,
                view.mount.server,
                view.mount.source
            ));
            ResponseHelper::json_success(view)
        }
        Ok(Err(message)) => ResponseHelper::json_error(message),
        Err(e) => ResponseHelper::json_internal_error(format!("Save worker failed: {e}")),
    }
}

/// `PUT /api/remote-mounts/{name}` — edit one (and remount when it was up).
pub async fn handle_remote_mount_update(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let req: RemoteMountRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };
    let name = match remote_mounts::validate_name(&name) {
        Ok(name) => name,
        Err(message) => return ResponseHelper::json_error(message),
    };
    let password = req.password.clone().filter(|p| !p.is_empty());
    let remount = req.mount_now;

    let result = tokio::task::spawn_blocking(move || {
        let previous = remote_mounts::load().into_iter().find(|m| m.name == name);
        let Some(previous) = previous else {
            return Err(format!("No network location named '{name}'"));
        };
        // Validate the edited definition before touching a live mount: an
        // invalid server/share/option must not unmount the working one.
        let mut mount = req.into_mount(name.clone(), Some(&previous));
        remote_mounts::validate_fields(&mut mount)?;
        // Apply the edit, then bring the new definition up: an option or
        // host change must not leave the old mount serving stale data.
        let was_mounted = remote_mounts::list()
            .iter()
            .any(|view| view.mount.name == name && view.mounted);
        if was_mounted {
            remote_mounts::unmount_one(&name)?;
        }
        remote_mounts::upsert(mount, password.as_deref())?;
        if was_mounted || remount {
            remote_mounts::mount_one(&name)
        } else {
            remote_mounts::list()
                .into_iter()
                .find(|view| view.mount.name == name)
                .ok_or_else(|| format!("Network location '{name}' disappeared"))
        }
    })
    .await;

    match result {
        Ok(Ok(view)) => {
            crate::events::log_event(format!(
                "Admin '{admin}' edited the network location '{}'",
                view.mount.name
            ));
            ResponseHelper::json_success(view)
        }
        Ok(Err(message)) => ResponseHelper::json_error(message),
        Err(e) => ResponseHelper::json_internal_error(format!("Update worker failed: {e}")),
    }
}

/// `DELETE /api/remote-mounts/{name}` — unmount (when up) and forget it.
pub async fn handle_remote_mount_delete(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let name = match remote_mounts::validate_name(&name) {
        Ok(name) => name,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let result = tokio::task::spawn_blocking({
        let name = name.clone();
        move || remote_mounts::remove(&name)
    })
    .await;
    match result {
        Ok(Ok(true)) => {
            crate::events::log_event(format!(
                "Admin '{admin}' removed the network location '{name}'"
            ));
            ResponseHelper::json_success(serde_json::json!({ "removed": name }))
        }
        Ok(Ok(false)) => ResponseHelper::json_error(format!("No network location named '{name}'")),
        Ok(Err(message)) => ResponseHelper::json_error(message),
        Err(e) => ResponseHelper::json_internal_error(format!("Remove worker failed: {e}")),
    }
}

/// `POST /api/remote-mounts/{name}/mount` — bring one up.
pub async fn handle_remote_mount_mount(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let name = match remote_mounts::validate_name(&name) {
        Ok(name) => name,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let result = tokio::task::spawn_blocking(move || remote_mounts::mount_one(&name)).await;
    match result {
        Ok(Ok(view)) => {
            crate::events::log_event(format!(
                "Admin '{admin}' mounted the network location '{}'",
                view.mount.name
            ));
            ResponseHelper::json_success(view)
        }
        Ok(Err(message)) => ResponseHelper::json_error(message),
        Err(e) => ResponseHelper::json_internal_error(format!("Mount worker failed: {e}")),
    }
}

/// `POST /api/remote-mounts/{name}/unmount` — take one down.
pub async fn handle_remote_mount_unmount(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let name = match remote_mounts::validate_name(&name) {
        Ok(name) => name,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let result = tokio::task::spawn_blocking({
        let name = name.clone();
        move || remote_mounts::unmount_one(&name)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{admin}' unmounted the network location '{name}'"
            ));
            ResponseHelper::json_success(serde_json::json!({ "unmounted": name }))
        }
        Ok(Err(message)) => ResponseHelper::json_error(message),
        Err(e) => ResponseHelper::json_internal_error(format!("Unmount worker failed: {e}")),
    }
}

/// `POST /api/remote-mounts/reconcile` — mount every enabled location that is
/// not up yet (the same pass the server runs at startup).
pub async fn handle_remote_mounts_reconcile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let result = tokio::task::spawn_blocking(remote_mounts::reconcile).await;
    match result {
        Ok(failures) => {
            if !failures.is_empty() {
                crate::events::log_event_warn(format!(
                    "Admin '{admin}' reconnected network locations: {} failed",
                    failures.len()
                ));
            }
            ResponseHelper::json_success(serde_json::json!({
                "failed": failures
                    .into_iter()
                    .map(|(name, error)| serde_json::json!({ "name": name, "error": error }))
                    .collect::<Vec<_>>(),
                "mounts": remote_mounts::list(),
            }))
        }
        Err(e) => ResponseHelper::json_internal_error(format!("Reconcile worker failed: {e}")),
    }
}
