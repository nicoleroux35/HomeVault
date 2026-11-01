/* =============================================================================
 * File          : rsync.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Rsync server endpoints: read the module list and the share candidates,
 *   add/toggle/delete modules, manage the daemon settings and set a user's
 *   rsync password. Every mutation is admin-only; a password is written and
 *   never echoed back.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::rsync_server, crate::storage::UserRepository
 *
 * Usage:
 *   Routes under /api/shares/rsync in src/web/mod.rs; see
 *   docs/guides/rsync.md.
 * =============================================================================
 */

//! Rsync server endpoints (admin-only), see docs/guides/rsync.md.
//!
//! * `GET    /api/shares/rsync`                      — modules, settings, candidates
//! * `PUT    /api/shares/rsync/config`               — daemon settings
//! * `POST   /api/shares/rsync/modules`              — add/replace a module
//! * `PUT    /api/shares/rsync/modules/{name}`       — toggle / read-only / users
//! * `DELETE /api/shares/rsync/modules/{name}`       — remove a module
//! * `PUT    /api/shares/rsync/users/{user}/secret`  — set a password
//! * `DELETE /api/shares/rsync/users/{user}/secret`  — forget a password

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;

use crate::handlers::admin_guard::require_admin;
use crate::rsync_server::{self, RsyncModule, RsyncSettings};
use crate::storage::UserRepository;
use crate::utils::ResponseHelper;

use super::AppState;

/// Ceiling for one rsync operation (validation + a few file writes + one
/// `systemctl` call). A hung `sudo` must not pin the request.
const RSYNC_OP_TIMEOUT: Duration = Duration::from_secs(90);

async fn run_op<T, F>(op: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(RSYNC_OP_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(format!("{op} worker failed: {e}")),
        Err(_) => Err(format!(
            "{op} timed out after {}s",
            RSYNC_OP_TIMEOUT.as_secs()
        )),
    }
}

/// Whether the `rsync` binary this unit starts is present.
fn binary_available() -> bool {
    crate::dependencies::binary_available("rsync", "/usr/bin/rsync")
}

fn settings_json(settings: &RsyncSettings) -> serde_json::Value {
    serde_json::json!({
        "enabled": settings.enabled,
        "port": settings.port,
        "max_connections": settings.max_connections,
        "chroot": settings.chroot,
    })
}

/// Build the full read payload. Blocking (it reads the Samba config and the
/// host's share inventory) — always called from `spawn_blocking`.
fn read_payload() -> Result<serde_json::Value, String> {
    let state = rsync_server::load_state()?;
    let candidates = rsync_server::share_candidates(&state);
    Ok(serde_json::json!({
        "settings": settings_json(&state.settings),
        "modules": state.modules,
        "share_candidates": candidates,
        // The set of usernames that *have* a secret — never the secret.
        "users_with_secret": state.secrets.keys().cloned().collect::<Vec<_>>(),
        "service_active": rsync_server::service_active(),
        "binary_available": binary_available(),
        "config_path": rsync_server::conf_path().to_string_lossy(),
        "module_limit": rsync_server::MAX_MODULES,
        "default_port": rsync_server::DEFAULT_PORT,
        "connection_limit": rsync_server::MAX_CONNECTIONS_LIMIT,
        // The documented, key-only path that needs no daemon at all.
        "ssh_path": "rsync -e 'ssh -p <SFTP port>' <user>@<host>:/<share>/ <local>/",
        "ssh_hint": "Files can also be pulled with `rsync -e ssh` over the SFTP account: it needs no rsync daemon and reuses the SFTP key you already configured.",
    }))
}

/// `GET /api/shares/rsync`
pub async fn handle_rsync_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let users = UserRepository::new(state.db.pool())
        .find_all()
        .await
        .map(|users| users.into_iter().map(|u| u.username).collect::<Vec<_>>())
        .unwrap_or_default();
    match run_op("load rsync state", read_payload).await {
        Ok(mut payload) => {
            // The user picker is the HomeVault account list: rsync `auth users`
            // values are HomeVault usernames, not host accounts.
            payload["available_users"] = serde_json::json!(users);
            ResponseHelper::json_success(payload)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to load the Rsync server: {e}")),
    }
}

/// `PUT /api/shares/rsync/config`
#[derive(Debug, Deserialize)]
pub struct RsyncSettingsRequest {
    pub enabled: bool,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub chroot: Option<bool>,
}

pub async fn handle_rsync_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RsyncSettingsRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let current = match run_op("read rsync settings", rsync_server::load_state).await {
        Ok(state) => state.settings,
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the settings: {e}")),
    };
    let settings = RsyncSettings {
        enabled: body.enabled,
        port: body.port.unwrap_or(current.port),
        max_connections: body.max_connections.unwrap_or(current.max_connections),
        chroot: body.chroot.unwrap_or(current.chroot),
    };
    match run_op("save rsync settings", move || {
        rsync_server::update_settings(settings)
    })
    .await
    {
        Ok(state) => {
            crate::events::log_event(format!(
                "Admin '{}' {} the Rsync server on port {}",
                admin,
                if state.settings.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                state.settings.port
            ));
            match run_op("load rsync state", read_payload).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(format!(
                    "Saved, but the state could not be re-read: {e}"
                )),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the settings: {e}")),
    }
}

/// `POST /api/shares/rsync/modules`
#[derive(Debug, Deserialize)]
pub struct RsyncModuleRequest {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

pub async fn handle_rsync_module_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RsyncModuleRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let module = RsyncModule {
        name: body.name.trim().to_string(),
        path: body.path.trim().to_string(),
        comment: body.comment.trim().to_string(),
        read_only: body.read_only,
        users: body.users,
        enabled: body.enabled,
    };
    let log_name = module.name.clone();
    match run_op("save rsync module", move || {
        rsync_server::upsert_module(module)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' saved the Rsync module '{}'",
                admin, log_name
            ));
            match run_op("load rsync state", read_payload).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(format!("Saved, but: {e}")),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the module: {e}")),
    }
}

/// `PUT /api/shares/rsync/modules/{name}`
#[derive(Debug, Deserialize)]
pub struct RsyncModuleUpdateRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub users: Option<Vec<String>>,
    #[serde(default)]
    pub comment: Option<String>,
}

pub async fn handle_rsync_module_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<RsyncModuleUpdateRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let log_name = name.clone();
    match run_op("update rsync module", move || {
        rsync_server::set_module_enabled(
            &name,
            body.enabled,
            body.read_only,
            body.users,
            body.comment,
        )
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' updated the Rsync module '{}'",
                admin, log_name
            ));
            match run_op("load rsync state", read_payload).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(format!("Saved, but: {e}")),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update the module: {e}")),
    }
}

/// `DELETE /api/shares/rsync/modules/{name}`
pub async fn handle_rsync_module_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let log_name = name.clone();
    match run_op("delete rsync module", move || {
        rsync_server::delete_module(&name)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' removed the Rsync module '{}'",
                admin, log_name
            ));
            match run_op("load rsync state", read_payload).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(format!("Deleted, but: {e}")),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete the module: {e}")),
    }
}

/// `PUT /api/shares/rsync/users/{user}/secret`
#[derive(Debug, Deserialize)]
pub struct RsyncSecretRequest {
    pub password: String,
}

pub async fn handle_rsync_secret_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user): Path<String>,
    Json(body): Json<RsyncSecretRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let log_user = user.clone();
    match run_op("set rsync password", move || {
        rsync_server::set_user_secret(&user, &body.password)
    })
    .await
    {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' set an Rsync password for '{}'",
                admin, log_user
            ));
            // The password itself is deliberately absent from the response.
            ResponseHelper::json_success(serde_json::json!({ "user": log_user, "has_secret": true }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to set the password: {e}")),
    }
}

/// `DELETE /api/shares/rsync/users/{user}/secret`
pub async fn handle_rsync_secret_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let log_user = user.clone();
    match run_op("delete rsync password", move || {
        rsync_server::delete_user_secret(&user)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' removed the Rsync password of '{}'",
                admin, log_user
            ));
            ResponseHelper::json_success(serde_json::json!({ "user": log_user, "has_secret": false }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove the password: {e}")),
    }
}
