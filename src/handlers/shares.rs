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

/// Best-effort FTP jail rebuild after a grant or FTP share change.
async fn rebuild_ftp_jails_best_effort(state: &AppState) {
    if !crate::host_accounts::host_sync_enabled() {
        return;
    }
    match tokio::time::timeout(
        crate::ftp::FTP_REBUILD_TIMEOUT,
        crate::ftp::FtpJailService::rebuild_jails(state.db.pool()),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("FTP: jail rebuild failed: {}", e),
        Err(_) => tracing::warn!(
            "FTP: jail rebuild timed out after {}s",
            crate::ftp::FTP_REBUILD_TIMEOUT.as_secs()
        ),
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

/// A logical share: one name + one path exposed by one or more protocols.
///
/// The persisted model is still one record per protocol; this view is what
/// the Shares page renders and what the "bind protocol" endpoints act on, so
/// a home user creates a folder once instead of repeating it per protocol.
#[derive(serde::Serialize)]
pub struct ShareDefinition {
    pub name: String,
    pub path: String,
    pub comment: String,
    pub protocols: Vec<String>,
    pub enabled: bool,
    pub conflicts: Vec<String>,
}

/// Body for creating one logical share over several protocols.
#[derive(serde::Deserialize)]
pub struct CreateShareDefinitionRequest {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub comment: String,
    pub protocols: Vec<String>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub guest_ok: Option<bool>,
    #[serde(default)]
    pub restrictions: Option<crate::shares::ShareRestrictions>,
}

/// Body for binding one more protocol to an existing logical share.
#[derive(serde::Deserialize)]
pub struct BindProtocolRequest {
    pub share_type: String,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub guest_ok: Option<bool>,
    #[serde(default)]
    pub restrictions: Option<crate::shares::ShareRestrictions>,
}

/// Body for creating a new share.
#[derive(serde::Deserialize)]
pub struct CreateShareRequest {
    pub name: String,
    pub share_type: String,
    pub path: String,
    pub comment: String,
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub guest_ok: Option<bool>,
    #[serde(default)]
    pub restrictions: Option<crate::shares::ShareRestrictions>,
}

/// Body for updating an existing share. All fields are optional.
#[derive(serde::Deserialize)]
pub struct UpdateShareRequest {
    pub path: Option<String>,
    pub enabled: Option<bool>,
    pub comment: Option<String>,
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub guest_ok: Option<Option<bool>>,
    #[serde(default)]
    pub restrictions: Option<crate::shares::ShareRestrictions>,
}

/// Lists all Samba/NFS shares.
#[axum_macros::debug_handler]
pub async fn handle_shares_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    match run_share_op("list shares", SharesService::get_shares).await {
        Ok(shares) => ResponseHelper::json_success(shares),
        Err(e) => ResponseHelper::json_error(format!("Failed to list shares: {}", e)),
    }
}

/// Groups the per-protocol records into the logical shares the UI shows.
fn group_definitions(shares: Vec<Share>) -> Vec<ShareDefinition> {
    use std::collections::BTreeMap;
    let mut by_key: BTreeMap<(String, String), ShareDefinition> = BTreeMap::new();
    let mut paths_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for share in &shares {
        paths_by_name
            .entry(share.name.clone())
            .or_default()
            .push(share.path.clone());
    }
    for share in shares {
        let key = (share.name.clone(), share.path.clone());
        let entry = by_key.entry(key).or_insert_with(|| ShareDefinition {
            name: share.name.clone(),
            path: share.path.clone(),
            comment: share.comment.clone(),
            protocols: Vec::new(),
            enabled: false,
            conflicts: Vec::new(),
        });
        entry.enabled |= share.enabled;
        if !entry.protocols.contains(&share.share_type.as_str().to_string()) {
            entry.protocols.push(share.share_type.as_str().to_string());
        }
        if entry.comment.is_empty() && !share.comment.is_empty() {
            entry.comment = share.comment.clone();
        }
    }
    for definition in by_key.values_mut() {
        definition.protocols.sort();
        let mut paths = paths_by_name
            .get(&definition.name)
            .cloned()
            .unwrap_or_default();
        paths.sort();
        paths.dedup();
        definition.conflicts = paths
            .into_iter()
            .filter(|path| path != &definition.path)
            .collect();
    }
    by_key.into_values().collect()
}

/// Lists logical shares (one row per name + path, with its protocol badges).
#[axum_macros::debug_handler]
pub async fn handle_share_definitions_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    match run_share_op("list share definitions", SharesService::get_shares).await {
        Ok(shares) => ResponseHelper::json_success(group_definitions(shares)),
        Err(e) => ResponseHelper::json_error(format!("Failed to list shares: {e}")),
    }
}

/// Creates the same folder once and binds it to several protocols.
#[axum_macros::debug_handler]
pub async fn handle_share_definition_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<CreateShareDefinitionRequest>,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    if let Err(e) = crate::shares::validate_share_name(&req.name) {
        return ResponseHelper::json_error(format!("name: {e}"));
    }
    if let Err(e) = crate::shares::validate_share_path(&req.path) {
        return ResponseHelper::json_error(format!("path: {e}"));
    }
    if let Err(e) = crate::shares::validate_share_comment(&req.comment) {
        return ResponseHelper::json_error(format!("comment: {e}"));
    }
    let mut protocols: Vec<ShareType> = Vec::new();
    for raw in &req.protocols {
        let Some(kind) = ShareType::parse(raw) else {
            return ResponseHelper::json_error(format!("Invalid share type '{raw}'"));
        };
        if protocols.contains(&kind) {
            return ResponseHelper::json_error(format!("Duplicate share type '{raw}'"));
        }
        protocols.push(kind);
    }
    if protocols.is_empty() {
        return ResponseHelper::json_error("At least one protocol is required".to_string());
    }

    // A logical share has one path. Refusing a second path for the same name
    // is what makes "create once, bind many protocols" safe instead of
    // silently producing two folders with the same name and different data.
    let name = req.name.clone();
    let path = req.path.clone();
    let existing = match run_share_op("list shares", SharesService::get_shares).await {
        Ok(shares) => shares,
        Err(e) => return ResponseHelper::json_error(format!("Failed to list shares: {e}")),
    };
    if let Some(other) = existing
        .iter()
        .find(|share| share.name == name && share.path != path)
    {
        return ResponseHelper::json_error(format!(
            "Share name '{}' already exists at '{}'; a logical share has one path",
            name, other.path
        ));
    }

    let mut created: Vec<ShareType> = Vec::new();
    for kind in &protocols {
        let share = Share {
            name: req.name.clone(),
            share_type: kind.clone(),
            path: req.path.clone(),
            enabled: true,
            comment: req.comment.clone(),
            options: Vec::new(),
            read_only: req.read_only.unwrap_or(false),
            guest_ok: req.guest_ok,
            restrictions: req.restrictions.unwrap_or_default(),
        };
        if let Err(e) = run_share_op("create share", move || SharesService::create_share(&share)).await
        {
            let rollback_name = req.name.clone();
            for kind in created {
                let rollback_name = rollback_name.clone();
                let _ = run_share_op("rollback share", move || {
                    SharesService::delete_share(&rollback_name, kind)
                })
                .await;
            }
            return ResponseHelper::json_error(format!("Failed to create share: {e}"));
        }
        created.push(kind.clone());
    }

    crate::events::log_event(format!(
        "Admin '{}' created logical share '{}' at '{}' over {:?}",
        admin, req.name, req.path, created
    ));
    if created.contains(&ShareType::Sftp) {
        rebuild_sftp_jails_best_effort(&state).await;
    }
    if created.contains(&ShareType::Webdav) {
        reload_webdav_best_effort(&state).await;
    }
    if created.contains(&ShareType::Ftp) {
        rebuild_ftp_jails_best_effort(&state).await;
    }
    ResponseHelper::json_success("Share created successfully".to_string())
}

/// Binds one more protocol to an existing logical share.
#[axum_macros::debug_handler]
pub async fn handle_share_bind_protocol(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<BindProtocolRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let Some(kind) = ShareType::parse(&req.share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let existing = match run_share_op("list shares", SharesService::get_shares).await {
        Ok(shares) => shares,
        Err(e) => return ResponseHelper::json_error(format!("Failed to list shares: {e}")),
    };
    let Some(source) = existing.iter().find(|share| share.name == name) else {
        return ResponseHelper::json_error(format!("Share '{name}' not found"));
    };
    if existing
        .iter()
        .any(|share| share.name == name && share.share_type == kind)
    {
        return ResponseHelper::json_error(format!(
            "Share '{name}' already has a {} binding",
            req.share_type
        ));
    }
    let share = Share {
        name: name.clone(),
        share_type: kind.clone(),
        path: source.path.clone(),
        enabled: true,
        comment: source.comment.clone(),
        options: Vec::new(),
        read_only: req.read_only.unwrap_or(false),
        guest_ok: req.guest_ok,
        restrictions: req.restrictions.unwrap_or_default(),
    };
    match run_share_op("bind share protocol", move || SharesService::create_share(&share)).await {
        Ok(_) => {
            if kind == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            if kind == ShareType::Webdav {
                reload_webdav_best_effort(&state).await;
            }
            if kind == ShareType::Ftp {
                rebuild_ftp_jails_best_effort(&state).await;
            }
            if kind == ShareType::Smb {
                if let Err(e) =
                    crate::permissions::PermissionService::refresh_smb_access(
                        &name,
                        state.db.pool(),
                    )
                    .await
                {
                    tracing::warn!("Failed to refresh SMB access for '{}': {}", name, e);
                }
            }
            ResponseHelper::json_success("Protocol bound".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to bind protocol: {e}")),
    }
}

/// Removes one protocol binding from a logical share, leaving the folder and
/// the other protocol bindings untouched.
#[axum_macros::debug_handler]
pub async fn handle_share_unbind_protocol(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let Some(kind) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let name_for_op = name.clone();
    let kind_for_op = kind.clone();
    match run_share_op("unbind share protocol", move || {
        SharesService::delete_share(&name_for_op, kind_for_op)
    })
    .await
    {
        Ok(_) => {
            if kind == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            if kind == ShareType::Webdav {
                reload_webdav_best_effort(&state).await;
            }
            if kind == ShareType::Ftp {
                rebuild_ftp_jails_best_effort(&state).await;
            }
            ResponseHelper::json_success("Protocol unbound".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to unbind protocol: {e}")),
    }
}

/// Body for joining an AD domain.
#[derive(serde::Deserialize)]
pub struct DomainJoinRequest {
    pub username: String,
    pub password: String,
}

/// Joins the configured AD realm with a domain account.
#[axum_macros::debug_handler]
pub async fn handle_smb_domain_join(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<DomainJoinRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let config = match run_share_op("load SMB config", SharesService::get_smb_config).await {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_error(format!("Failed to load SMB config: {e}")),
    };
    if !matches!(
        config.security_mode.to_lowercase().as_str(),
        "ads" | "domain"
    ) || config.realm.trim().is_empty()
    {
        return ResponseHelper::json_error(
            "Configure security_mode=ads and a realm before joining a domain".to_string(),
        );
    }
    match run_share_op("join AD domain", move || {
        crate::shares::join_smb_domain(&req.username, &req.password)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("Domain joined".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Domain join failed: {e}")),
    }
}

/// Leaves the current AD domain.
#[axum_macros::debug_handler]
pub async fn handle_smb_domain_leave(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    match run_share_op("leave AD domain", crate::shares::leave_smb_domain).await {
        Ok(_) => ResponseHelper::json_success("Domain left".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Domain leave failed: {e}")),
    }
}

/// Returns one NFS share's per-client rules.
#[axum_macros::debug_handler]
pub async fn handle_nfs_rules_get(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    match run_share_op("load NFS rules", move || {
        Ok::<_, crate::shares::ShareError>(SharesService::get_nfs_rules(&name))
    })
    .await
    {
        Ok(rules) => ResponseHelper::json_success(rules),
        Err(e) => ResponseHelper::json_error(format!("Failed to load NFS rules: {e}")),
    }
}

/// Replaces one NFS share's per-client rules (`[]` restores the global set).
#[axum_macros::debug_handler]
pub async fn handle_nfs_rules_set(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<Vec<crate::shares::NfsClientRule>>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let rules = req.0;
    match run_share_op("save NFS rules", move || {
        SharesService::save_nfs_rules(&name, &rules)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success("NFS rules updated".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to save NFS rules: {e}")),
    }
}

/// Gets details for a single share by name and type.
#[axum_macros::debug_handler]
pub async fn handle_share_get(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
        read_only: req.read_only,
        guest_ok: req.guest_ok,
        restrictions: req.restrictions,
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
            if share_type == ShareType::Smb {
                if let Err(e) =
                    crate::permissions::PermissionService::refresh_smb_access(&name, state.db.pool())
                        .await
                {
                    tracing::warn!("Failed to refresh SMB access for '{}': {}", name, e);
                }
            }
            if share_type == ShareType::Ftp {
                rebuild_ftp_jails_best_effort(&state).await;
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
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

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
        read_only: req.read_only.unwrap_or(false),
        guest_ok: req.guest_ok,
        restrictions: req.restrictions.unwrap_or_default(),
    };

    match run_share_op("create share", move || SharesService::create_share(&share)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created {} share '{}' at '{}'",
                admin,
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
            if share_type == ShareType::Ftp {
                rebuild_ftp_jails_best_effort(&state).await;
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
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

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
            crate::events::log_event_warn(format!("Admin '{}' deleted share '{}'", admin, name));
            if share_type == ShareType::Sftp {
                rebuild_sftp_jails_best_effort(&state).await;
            }
            if share_type == ShareType::Webdav {
                reload_webdav_best_effort(&state).await;
            }
            if share_type == ShareType::Ftp {
                rebuild_ftp_jails_best_effort(&state).await;
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
            if share_type == ShareType::Ftp {
                rebuild_ftp_jails_best_effort(&state).await;
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let ws_discovery = req.ws_discovery;
    let workgroup = req.workgroup.clone();
    match run_share_op("save SMB config", move || {
        SharesService::save_smb_config(&req)
    })
    .await
    {
        Ok(_) => {
            // WS-Discovery is a HomeVault setting, not a smb.conf directive.
            // Best-effort: a missing wsdd2 must not turn a valid SMB save into
            // an error; the card reads the reason from its status endpoint.
            match tokio::task::spawn_blocking(move || {
                crate::ws_discovery::apply(ws_discovery, &workgroup)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("Windows network discovery could not be applied: {}", e)
                }
                Err(e) => tracing::warn!("Windows network discovery task failed: {}", e),
            }
            ResponseHelper::json_success("SMB config updated".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save SMB config: {}", e)),
    }
}

/// `GET /api/shares/ws-discovery` — status of the Windows discovery daemon.
///
/// Read-only and admin-only: the daemon only advertises the appliance's name
/// and workgroup, never a share or a file.
pub async fn handle_ws_discovery_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(crate::ws_discovery::status())
}

/// Body for the per-share recycle-bin toggle.
#[derive(Debug, serde::Deserialize)]
pub struct RecycleToggleRequest {
    pub enabled: bool,
}

/// Body for the per-share Time Machine toggle. `quota_gb = 0` (or absent)
/// means "no maximum size"; the value is validated before it reaches
/// `smb.conf`.
#[derive(Debug, serde::Deserialize)]
pub struct TimeMachineRequest {
    pub enabled: bool,
    #[serde(default)]
    pub quota_gb: u32,
}

/// Returns, for every configured SMB share, whether its recycle bin is on.
#[axum_macros::debug_handler]
pub async fn handle_smb_recycle_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    match run_share_op("load SMB recycle state", || {
        Ok::<_, crate::shares::ShareError>(SharesService::get_smb_recycle_map())
    })
    .await
    {
        Ok(map) => ResponseHelper::json_success(map),
        Err(e) => ResponseHelper::json_error(format!("Failed to load recycle state: {}", e)),
    }
}

/// Turns the recycle bin on or off for one SMB share.
#[axum_macros::debug_handler]
pub async fn handle_smb_recycle_set(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<RecycleToggleRequest>,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let name_for_op = name.clone();
    let enabled = req.enabled;
    match run_share_op("set SMB recycle", move || {
        SharesService::set_smb_recycle(&name_for_op, enabled)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' {} the recycle bin for SMB share '{}'",
                admin,
                if enabled { "enabled" } else { "disabled" },
                name
            ));
            ResponseHelper::json_success(serde_json::json!({ "enabled": enabled }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update recycle bin: {}", e)),
    }
}

/// Returns every SMB share's Time Machine state (toggle + quota), derived from
/// the live `smb.conf` like the recycle map.
/// Lists which SMB shares expose Previous Versions.
#[axum_macros::debug_handler]
pub async fn handle_smb_previous_versions_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    match run_share_op("load SMB Previous Versions state", || {
        Ok::<_, crate::shares::ShareError>(SharesService::get_smb_previous_versions_map())
    })
    .await
    {
        Ok(map) => ResponseHelper::json_success(map),
        Err(e) => ResponseHelper::json_error(format!("Failed to load Previous Versions: {e}")),
    }
}

/// Turns Previous Versions on/off for one SMB share.
#[axum_macros::debug_handler]
pub async fn handle_smb_previous_versions_set(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<RecycleToggleRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let enabled = req.enabled;
    match run_share_op("set SMB Previous Versions", move || {
        SharesService::set_smb_previous_versions(&name, enabled)
    })
    .await
    {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({ "enabled": enabled })),
        Err(e) => ResponseHelper::json_error(format!("Failed to set Previous Versions: {e}")),
    }
}

#[axum_macros::debug_handler]
pub async fn handle_smb_time_machine_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    match run_share_op("load SMB Time Machine state", || {
        Ok::<_, crate::shares::ShareError>(SharesService::get_smb_time_machine_map())
    })
    .await
    {
        Ok(map) => ResponseHelper::json_success(map),
        Err(e) => ResponseHelper::json_error(format!("Failed to load Time Machine state: {}", e)),
    }
}

/// Turns Time Machine on or off for one SMB share, and stores the optional
/// maximum size macOS must respect.
#[axum_macros::debug_handler]
pub async fn handle_smb_time_machine_set(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    req: axum::extract::Json<TimeMachineRequest>,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    // Validate before the blocking write: a quota outside the accepted range
    // must come back as a clear message, not as a half-written `smb.conf`.
    if let Err(e) = crate::shares::validate_time_machine_quota(req.quota_gb) {
        return ResponseHelper::json_error(e);
    }

    let name_for_op = name.clone();
    let enabled = req.enabled;
    let quota_gb = if enabled { req.quota_gb } else { 0 };
    match run_share_op("set SMB Time Machine", move || {
        SharesService::set_smb_time_machine(&name_for_op, enabled, quota_gb)
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' {} Time Machine on SMB share '{}'{}",
                admin,
                if enabled { "enabled" } else { "disabled" },
                name,
                if enabled && quota_gb > 0 {
                    format!(" (quota {quota_gb} GiB)")
                } else {
                    String::new()
                }
            ));
            ResponseHelper::json_success(serde_json::json!({
                "enabled": enabled,
                "quota_gb": quota_gb,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update Time Machine: {}", e)),
    }
}

/// Returns the current NFS server configuration.
#[axum_macros::debug_handler]
pub async fn handle_nfs_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    match run_share_op("save FTP config", move || {
        SharesService::save_ftp_config(&req)
    })
    .await
    {
        Ok(_) => {
            rebuild_ftp_jails_best_effort(&state).await;
            ResponseHelper::json_success("FTP config updated".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save FTP config: {}", e)),
    }
}

/// Returns the current WebDAV server configuration.
#[axum_macros::debug_handler]
pub async fn handle_webdav_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

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
                admin, req.username
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    match run_jail_rebuild(&state).await {
        Ok(_) => ResponseHelper::json_success("SFTP access refreshed".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to refresh SFTP access: {}", e)),
    }
}
