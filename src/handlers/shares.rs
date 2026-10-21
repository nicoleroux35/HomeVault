/* =============================================================================
 * File          : shares.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Network shares across all supported protocols (SMB, NFS, FTP, SFTP,
 *   WebDAV): create, update, enable/disable and delete shares, per-protocol
 *   configuration get/update, and share-service start/stop/restart. After
 *   any SFTP-affecting change the SFTP chroot jails are rebuilt best-effort
 *   so a stale jail never blocks the write that already succeeded.
 *   Mutations are admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Routed in src/web/mod.rs (see docs/API.md, Shares section); protocol
 *   work delegated to SharesService and SftpJailService.
 * =============================================================================
 */

//! Network share endpoints across all supported protocols (SMB, NFS, FTP,
//! SFTP with chroot jails, WebDAV): CRUD, per-protocol configuration and
//! service control.

use crate::handlers::AppState;
use crate::sftp::SftpJailService;
use crate::shares::{
    FtpConfig, NfsConfig, SftpConfig, Share, ShareType, ShareUpdate, SharesService, SmbConfig,
    WebdavConfig,
};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use std::sync::Arc;
use std::time::Duration;

/// Hard ceiling for one share operation. Every `SharesService` call reads or
/// rewrites a root-owned file and/or shells out to `sudo
/// systemctl`/`tee`/`exportfs`; an unbounded wait would pin a request (and a
/// blocking-pool thread) forever.
const SHARE_OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for a full SFTP jail reconciliation (many `sudo` calls).
const JAIL_REBUILD_TIMEOUT: Duration = Duration::from_secs(120);

/// Run a blocking `SharesService` operation on the blocking thread pool with a
/// hard timeout. `SharesService` is entirely synchronous (file IO + `sudo`
/// subprocesses), so calling it directly from an async handler would block a
/// runtime worker — this is the single funnel every handler uses.
async fn run_share_op<T, E, F>(op: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    match tokio::time::timeout(SHARE_OP_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(result)) => result.map_err(|e| e.to_string()),
        Ok(Err(join_err)) => Err(format!("{op} worker failed: {join_err}")),
        Err(_) => Err(format!(
            "{op} timed out after {}s",
            SHARE_OP_TIMEOUT.as_secs()
        )),
    }
}

/// Run the async SFTP jail reconciliation under the same hard ceiling.
async fn run_jail_rebuild(state: &AppState) -> Result<(), String> {
    match tokio::time::timeout(
        JAIL_REBUILD_TIMEOUT,
        SftpJailService::rebuild_jails(state.db.pool()),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!(
            "SFTP jail rebuild timed out after {}s",
            JAIL_REBUILD_TIMEOUT.as_secs()
        )),
    }
}

/// Best-effort SFTP jail rebuild after an SFTP-affecting mutation (share
/// create/update/delete/toggle, config save, key set). Never fails the
/// caller's own response — a jail-provisioning error (e.g. `mount --bind`
/// failing for one stale share) shouldn't block the share/config write that
/// already succeeded; it's logged and left for the next rebuild to retry.
async fn rebuild_sftp_jails_best_effort(state: &AppState) {
    if let Err(e) = run_jail_rebuild(state).await {
        tracing::warn!("SFTP: jail rebuild failed: {}", e);
    }
}

/// Rebuilds the in-process WebDAV listener after a share change, so a new or
/// renamed share is reachable immediately.
///
/// Only a *running* server is reloaded (`crate::webdav::reload_if_running`):
/// editing shares must never start a service the admin had turned off.
async fn reload_webdav_best_effort(state: &std::sync::Arc<AppState>) {
    crate::webdav::reload_if_running(state).await;
}

/// Body for creating a new share.
#[derive(serde::Deserialize)]
pub struct CreateShareRequest {
    pub name: String,
    pub share_type: String,
    pub path: String,
    pub comment: String,
    pub options: Option<Vec<String>>,
}

/// Body for updating an existing share. All fields are optional.
#[derive(serde::Deserialize)]
pub struct UpdateShareRequest {
    pub path: Option<String>,
    pub enabled: Option<bool>,
    pub comment: Option<String>,
    pub options: Option<Vec<String>>,
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

/// Best-effort username for event logging, after `require_admin` already
/// confirmed the caller has a valid admin session.
async fn actor_username(headers: &HeaderMap, state: &AppState) -> String {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    state
        .auth
        .get_username_from_token(session_token)
        .await
        .unwrap_or_else(|| "unknown".to_string())
}

/// Lists all Samba/NFS shares.
#[axum_macros::debug_handler]
pub async fn handle_shares_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("list shares", SharesService::get_shares).await {
        Ok(shares) => ResponseHelper::json_success(shares),
        Err(e) => ResponseHelper::json_error(format!("Failed to list shares: {}", e)),
    }
}

/// Gets details for a single share by name and type.
#[axum_macros::debug_handler]
pub async fn handle_share_get(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    let share_type_for_op = share_type.clone();
    let name_for_op = name.clone();
    match run_share_op("get share", move || {
        SharesService::get_share(&name_for_op, share_type_for_op)
    })
    .await
    {
        Ok(share) => ResponseHelper::json_success(share),
        Err(e) => ResponseHelper::json_error(format!("Share not found: {}", e)),
    }
}

/// Updates an existing share's path, enabled flag, comment, and options.
#[axum_macros::debug_handler]
pub async fn handle_share_update(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
    req: axum::extract::Json<UpdateShareRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    let update = ShareUpdate {
        path: req.path.clone(),
        enabled: req.enabled,
        comment: req.comment.clone(),
        options: req.options.clone(),
    };

    let name_for_op = name.clone();
    let type_for_op = share_type.clone();
    match run_share_op("update share", move || {
        SharesService::update_share(&name_for_op, type_for_op, update)
    })
    .await
    {
        Ok(_) => {
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            if share_type == ShareType::Webdav {
                reload_webdav_best_effort(&state).await;
            }
            ResponseHelper::json_success("Share updated successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update share: {}", e)),
    }
}

/// Creates a new share (SMB, NFS, FTP, or WebDAV).
#[axum_macros::debug_handler]
pub async fn handle_share_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<CreateShareRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&req.share_type) else {
        return ResponseHelper::json_error(
            "Invalid share type. Use 'smb', 'nfs', 'ftp', 'webdav' or 'sftp'".to_string(),
        );
    };

    let share = Share {
        name: req.name.clone(),
        share_type: share_type.clone(),
        path: req.path.clone(),
        enabled: true,
        comment: req.comment.clone(),
        options: req.options.clone().unwrap_or_default(),
    };

    match run_share_op("create share", move || SharesService::create_share(&share)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created {} share '{}' at '{}'",
                actor_username(&headers, &state).await,
                req.share_type.to_uppercase(),
                req.name,
                req.path
            ));
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            if share_type == ShareType::Webdav {
                reload_webdav_best_effort(&state).await;
            }
            ResponseHelper::json_success("Share created successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create share: {}", e)),
    }
}

/// Deletes a share by name and type.
#[axum_macros::debug_handler]
pub async fn handle_share_delete(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    let name_for_op = name.clone();
    let type_for_op = share_type.clone();
    match run_share_op("delete share", move || {
        SharesService::delete_share(&name_for_op, type_for_op)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted share '{}'",
                actor_username(&headers, &state).await,
                name
            ));
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            if share_type == ShareType::Webdav {
                reload_webdav_best_effort(&state).await;
            }
            ResponseHelper::json_success("Share deleted successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete share: {}", e)),
    }
}

/// Enables or disables a share.
#[axum_macros::debug_handler]
pub async fn handle_share_toggle(
    State(state): State<Arc<AppState>>,
    Path((name, share_type, enabled)): Path<(String, String, bool)>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    let name_for_op = name.clone();
    let type_for_op = share_type.clone();
    match run_share_op("toggle share", move || {
        SharesService::set_share_enabled(&name_for_op, type_for_op, enabled)
    })
    .await
    {
        Ok(_) => {
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            let msg = if enabled {
                "Share enabled"
            } else {
                "Share disabled"
            };
            ResponseHelper::json_success(msg.to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to toggle share: {}", e)),
    }
}

/// Returns the current SMB server configuration.
#[axum_macros::debug_handler]
pub async fn handle_smb_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("load SMB config", SharesService::get_smb_config).await {
        Ok(config) => ResponseHelper::json_success(config),
        Err(e) => ResponseHelper::json_error(format!("Failed to load SMB config: {}", e)),
    }
}

/// Updates the SMB server configuration.
#[axum_macros::debug_handler]
pub async fn handle_smb_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<SmbConfig>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("save SMB config", move || {
        SharesService::save_smb_config(&req)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("SMB config updated".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to save SMB config: {}", e)),
    }
}

/// Returns the current NFS server configuration.
#[axum_macros::debug_handler]
pub async fn handle_nfs_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("load NFS config", SharesService::get_nfs_config).await {
        Ok(config) => ResponseHelper::json_success(config),
        Err(e) => ResponseHelper::json_error(format!("Failed to load NFS config: {}", e)),
    }
}

/// Updates the NFS server configuration.
#[axum_macros::debug_handler]
pub async fn handle_nfs_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<NfsConfig>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("save NFS config", move || {
        SharesService::save_nfs_config(&req)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("NFS config updated".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to save NFS config: {}", e)),
    }
}

/// Starts a network share service (smbd, nfs-server, vsftpd, SFTP; WebDAV is
/// in-process and handled before this call).
#[axum_macros::debug_handler]
pub async fn handle_share_service_start(
    State(state): State<Arc<AppState>>,
    Path(share_type): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    // WebDAV is served in-process, not by a systemd unit — driving it through
    // `systemctl` would fail with "Unit homevault-webdav.service not found".
    if share_type == ShareType::Webdav {
        return match crate::webdav::WEBDAV.start(&state).await {
            Ok(0) => ResponseHelper::json_success(
                "No enabled WebDAV share yet — the server is not listening".to_string(),
            ),
            Ok(port) => {
                ResponseHelper::json_success(format!("WebDAV service started on port {port}"))
            }
            Err(e) => ResponseHelper::json_error(format!("Failed to start WebDAV: {}", e)),
        };
    }

    let type_for_op = share_type.clone();
    match run_share_op("start service", move || {
        SharesService::start_service(type_for_op)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("Service started".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to start service: {}", e)),
    }
}

/// Stops a network service (smbd, nfs-server, vsftpd, WebDAV, SFTP).
#[axum_macros::debug_handler]
pub async fn handle_share_service_stop(
    State(state): State<Arc<AppState>>,
    Path(share_type): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    if share_type == ShareType::Webdav {
        return match crate::webdav::WEBDAV.stop().await {
            Ok(_) => ResponseHelper::json_success("WebDAV service stopped".to_string()),
            Err(e) => ResponseHelper::json_error(format!("Failed to stop WebDAV: {}", e)),
        };
    }

    let type_for_op = share_type.clone();
    match run_share_op("stop service", move || {
        SharesService::stop_service(type_for_op)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("Service stopped".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to stop service: {}", e)),
    }
}

/// Restarts a network service (smbd, nfs-server, vsftpd, WebDAV, SFTP).
#[axum_macros::debug_handler]
pub async fn handle_share_service_restart(
    State(state): State<Arc<AppState>>,
    Path(share_type): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };

    if share_type == ShareType::Webdav {
        return match crate::webdav::WEBDAV.reload(&state).await {
            Ok(0) => ResponseHelper::json_success(
                "No enabled WebDAV share — the server is not listening".to_string(),
            ),
            Ok(port) => {
                ResponseHelper::json_success(format!("WebDAV service restarted on port {port}"))
            }
            Err(e) => ResponseHelper::json_error(format!("Failed to restart WebDAV: {}", e)),
        };
    }

    let type_for_op = share_type.clone();
    match run_share_op("restart service", move || {
        SharesService::restart_service(type_for_op)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("Service restarted".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to restart service: {}", e)),
    }
}

/// Returns the current FTP server configuration.
#[axum_macros::debug_handler]
pub async fn handle_ftp_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("load FTP config", SharesService::get_ftp_config).await {
        Ok(config) => ResponseHelper::json_success(config),
        Err(e) => ResponseHelper::json_error(format!("Failed to load FTP config: {}", e)),
    }
}

/// Updates the FTP server configuration.
#[axum_macros::debug_handler]
pub async fn handle_ftp_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<FtpConfig>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("save FTP config", move || {
        SharesService::save_ftp_config(&req)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("FTP config updated".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to save FTP config: {}", e)),
    }
}

/// Returns the current WebDAV server configuration.
#[axum_macros::debug_handler]
pub async fn handle_webdav_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("load WebDAV config", SharesService::get_webdav_config).await {
        Ok(config) => ResponseHelper::json_success(config),
        Err(e) => ResponseHelper::json_error(format!("Failed to load WebDAV config: {}", e)),
    }
}

/// Updates the WebDAV server configuration.
#[axum_macros::debug_handler]
pub async fn handle_webdav_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<WebdavConfig>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("save WebDAV config", move || {
        SharesService::save_webdav_config(&req)
    })
    .await
    {
        Ok(_) => {
            // Rebuild the in-process listener from the new config: saving the
            // settings is the admin's explicit "apply" action, so it starts
            // (or restarts) the server just like the old systemd unit did.
            if let Err(e) = crate::webdav::WEBDAV.reload(&state).await {
                tracing::warn!("WebDAV: reload after config save failed: {e}");
            }
            ResponseHelper::json_success("WebDAV config updated".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save WebDAV config: {}", e)),
    }
}

/// Returns the current SFTP server configuration.
#[axum_macros::debug_handler]
pub async fn handle_sftp_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("load SFTP config", SharesService::get_sftp_config).await {
        Ok(config) => ResponseHelper::json_success(config),
        Err(e) => ResponseHelper::json_error(format!("Failed to load SFTP config: {}", e)),
    }
}

/// Updates the SFTP server configuration (regenerates the dedicated sshd
/// instance's config file and restarts it if already running).
#[axum_macros::debug_handler]
pub async fn handle_sftp_config_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<SftpConfig>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_share_op("save SFTP config", move || {
        SharesService::save_sftp_config(&req)
    })
    .await
    {
        Ok(_) => {
            rebuild_sftp_jails_best_effort(&state).await;
            ResponseHelper::json_success("SFTP config updated".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save SFTP config: {}", e)),
    }
}

/// Lists every user with SFTP access to at least one share (resolved from
/// the same per-share ACL grants already shown in the "Manage Access"
/// modal), for the SFTP tab's key-management panel.
#[axum_macros::debug_handler]
pub async fn handle_sftp_grantees_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match tokio::time::timeout(
        JAIL_REBUILD_TIMEOUT,
        SftpJailService::list_grantees(state.db.pool()),
    )
    .await
    .map_err(|_| "SFTP grantee list timed out".to_string())
    .and_then(|res| res.map_err(|e| e.to_string()))
    {
        Ok(grantees) => ResponseHelper::json_success(grantees),
        Err(e) => ResponseHelper::json_error(format!("Failed to list SFTP users: {}", e)),
    }
}

/// Body for setting a user's SFTP public key.
#[derive(serde::Deserialize)]
pub struct SetSftpKeyRequest {
    pub username: String,
    pub public_key: String,
}

/// Sets (overwrites) a user's SFTP public key, provisioning their Linux
/// account if this is their first SFTP grant.
#[axum_macros::debug_handler]
pub async fn handle_sftp_key_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<SetSftpKeyRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    // `set_authorized_key` provisions a real Linux account for the username.
    // Refuse a name that isn't an actual HomeVault user, so a typo can't
    // leave an orphan system account behind.
    if !state.auth.user_exists(&req.username).await {
        return ResponseHelper::json_error(format!("User '{}' not found", req.username));
    }

    let username_for_op = req.username.clone();
    let key_for_op = req.public_key.clone();
    match run_share_op("set SFTP key", move || {
        SftpJailService::set_authorized_key(&username_for_op, &key_for_op)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' set an SFTP public key for user '{}'",
                actor_username(&headers, &state).await,
                req.username
            ));
            ResponseHelper::json_success("SFTP key saved".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save SFTP key: {}", e)),
    }
}

/// Reconciles every user's SFTP chroot jail against current share
/// permissions. Exposed as an explicit "Refresh SFTP Access" action since
/// permission grants/revokes happen in the separately frozen
/// `src/handlers/permissions.rs`, which this feature doesn't hook into.
#[axum_macros::debug_handler]
pub async fn handle_sftp_rebuild(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match run_jail_rebuild(&state).await {
        Ok(_) => ResponseHelper::json_success("SFTP access refreshed".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to refresh SFTP access: {}", e)),
    }
}
