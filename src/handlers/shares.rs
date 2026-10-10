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

/// Best-effort SFTP jail rebuild after an SFTP-affecting mutation (share
/// create/update/delete/toggle, config save, key set). Never fails the
/// caller's own response — a jail-provisioning error (e.g. `mount --bind`
/// failing for one stale share) shouldn't block the share/config write that
/// already succeeded; it's logged and left for the next rebuild to retry.
async fn rebuild_sftp_jails_best_effort(state: &AppState) {
    if let Err(e) = SftpJailService::rebuild_jails(state.db.pool()).await {
        tracing::warn!("SFTP: jail rebuild failed: {}", e);
    }
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

    match SharesService::get_shares() {
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

    match SharesService::get_share(&name, share_type) {
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

    match SharesService::update_share(&name, share_type.clone(), update) {
        Ok(_) => {
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
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

    match SharesService::create_share(&share) {
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

    match SharesService::delete_share(&name, share_type.clone()) {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted share '{}'",
                actor_username(&headers, &state).await,
                name
            ));
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
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

    match SharesService::set_share_enabled(&name, share_type.clone(), enabled) {
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

    match SharesService::get_smb_config() {
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

    match SharesService::save_smb_config(&req) {
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

    match SharesService::get_nfs_config() {
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

    match SharesService::save_nfs_config(&req) {
        Ok(_) => ResponseHelper::json_success("NFS config updated".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to save NFS config: {}", e)),
    }
}

/// Starts a network share service (smbd, nfs-server, vsftpd, homevault-webdav).
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

    match SharesService::start_service(share_type) {
        Ok(_) => ResponseHelper::json_success("Service started".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to start service: {}", e)),
    }
}

/// Stops a network service (smbd, nfs-server, vsftpd, homevault-webdav).
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

    match SharesService::stop_service(share_type) {
        Ok(_) => ResponseHelper::json_success("Service stopped".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to stop service: {}", e)),
    }
}

/// Restarts a network service (smbd, nfs-server, vsftpd, homevault-webdav).
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

    match SharesService::restart_service(share_type) {
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

    match SharesService::get_ftp_config() {
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

    match SharesService::save_ftp_config(&req) {
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

    match SharesService::get_webdav_config() {
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

    match SharesService::save_webdav_config(&req) {
        Ok(_) => ResponseHelper::json_success("WebDAV config updated".to_string()),
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

    match SharesService::get_sftp_config() {
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

    match SharesService::save_sftp_config(&req) {
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

    match SftpJailService::list_grantees(state.db.pool()).await {
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

    match SftpJailService::set_authorized_key(&req.username, &req.public_key) {
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

    match SftpJailService::rebuild_jails(state.db.pool()).await {
        Ok(_) => ResponseHelper::json_success("SFTP access refreshed".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to refresh SFTP access: {}", e)),
    }
}
