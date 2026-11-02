/* =============================================================================
 * File          : backup_target.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup-destination endpoints: read the settings and tenants, create a
 *   tenant (the token and repository password are returned exactly once),
 *   update its quota/verification, rotate its credentials, delete it, and run
 *   an integrity check on demand. Reads need a session, mutations are
 *   admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::backup_target, crate::storage
 *
 * Usage:
 *   Routes under /api/backup-target in src/web/mod.rs; see
 *   docs/guides/backup-target.md.
 * =============================================================================
 */

//! Backup-destination endpoints, see docs/guides/backup-target.md.
//!
//! * `GET    /api/backup-target`                    — settings + tenants
//! * `PUT    /api/backup-target/config`             — enable/port/listen
//! * `POST   /api/backup-target/tenants`            — create (secrets once)
//! * `PUT    /api/backup-target/tenants/{id}`       — quota / verification
//! * `POST   /api/backup-target/tenants/{id}/rotate`— new token + password
//! * `DELETE /api/backup-target/tenants/{id}`       — delete the tenant row
//! * `POST   /api/backup-target/tenants/{id}/verify`— integrity check now

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;

use crate::backup_target::{self, BackupTargetSettings, BackupTenant};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::storage::secret_box;
use crate::storage::BackupTenantRepository;
use crate::utils::ResponseHelper;

use super::AppState;

/// Ceiling for one operation (a few file writes and one `systemctl`).
const BACKUP_OP_TIMEOUT: Duration = Duration::from_secs(90);
/// A verification runs `restic check`, which is far slower.
const VERIFY_OP_TIMEOUT: Duration = Duration::from_secs(660);

async fn run_op<T, F>(op: &str, timeout: Duration, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    match tokio::time::timeout(timeout, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(format!("{op} worker failed: {e}")),
        Err(_) => Err(format!("{op} timed out after {}s", timeout.as_secs())),
    }
}

/// The advertised host: the request's own Host header when it is a name, the
/// mDNS name otherwise. Only used to *suggest* the repository URL.
fn advertised_host(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next())
        .filter(|host| !host.is_empty() && *host != "localhost" && *host != "127.0.0.1")
        .map(str::to_string)
        .unwrap_or_else(crate::mdns::mdns_hostname)
}

async fn payload(state: &Arc<AppState>, headers: &HeaderMap) -> Result<serde_json::Value, String> {
    let repo = BackupTenantRepository::new(state.db.pool());
    let tenants = repo.list().await.map_err(|e| e.to_string())?;
    let settings = backup_target::load_settings();
    let host = advertised_host(headers);
    let usage = backup_target::tenant_usage(&tenants);
    Ok(serde_json::json!({
        "settings": {
            "enabled": settings.enabled,
            "port": settings.port,
            "listen": settings.listen,
        },
        "tenants": tenants
            .iter()
            .map(|tenant| backup_target::tenant_json(
                tenant,
                usage.get(&tenant.id).copied().unwrap_or(0),
                &settings,
                &host,
            ))
            .collect::<Vec<_>>(),
        "rest_server_available": backup_target::rest_server_available(),
        "restic_available": crate::restic::ResticService::is_available(),
        "service_active": backup_target::service_active(),
        "default_port": backup_target::DEFAULT_PORT,
        "repos_root": backup_target::repos_root().to_string_lossy(),
    }))
}

/// `GET /api/backup-target`
pub async fn handle_backup_target_state(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    match payload(&state, &headers).await {
        Ok(payload) => ResponseHelper::json_success(payload),
        Err(e) => ResponseHelper::json_error(format!("Failed to read the backup destination: {e}")),
    }
}

/// `PUT /api/backup-target/config`
#[derive(Debug, Deserialize)]
pub struct BackupTargetSettingsRequest {
    pub enabled: bool,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub listen: Option<String>,
}

pub async fn handle_backup_target_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BackupTargetSettingsRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let current = backup_target::load_settings();
    let settings = BackupTargetSettings {
        enabled: body.enabled,
        port: body.port.unwrap_or(current.port),
        listen: body.listen.unwrap_or(current.listen),
    };
    let repo = BackupTenantRepository::new(state.db.pool());
    let tenants = match repo.list().await {
        Ok(tenants) => tenants,
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the tenants: {e}")),
    };
    let settings_for_save = settings.clone();
    match run_op(
        "save backup destination settings",
        BACKUP_OP_TIMEOUT,
        move || backup_target::save_settings(&settings_for_save, &tenants),
    )
    .await
    {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' {} the backup destination on port {}",
                admin,
                if body.enabled { "enabled" } else { "disabled" },
                settings.port
            ));
            match payload(&state, &headers).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(format!("Saved, but: {e}")),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the settings: {e}")),
    }
}

/// `POST /api/backup-target/tenants`
#[derive(Debug, Deserialize)]
pub struct CreateTenantRequest {
    pub name: String,
    /// `0` or absent = unlimited.
    #[serde(default)]
    pub quota_bytes: i64,
}

pub async fn handle_backup_tenant_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateTenantRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    // The typed name is validated as typed: silently lowercasing it would mean
    // the admin's screen no longer matches the repository the source must be
    // pointed at. A mixed-case name is refused with that reason instead.
    let name = body.name.trim().to_string();
    if let Err(e) = backup_target::validate_tenant_name(&name) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = backup_target::validate_quota(body.quota_bytes) {
        return ResponseHelper::json_error(e);
    }
    let repo = BackupTenantRepository::new(state.db.pool());
    match repo.name_taken(&name, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "A backup tenant named '{name}' already exists"
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }

    let token = backup_target::generate_token();
    let repo_password = backup_target::generate_repo_password();
    let auth_hash = backup_target::htpasswd_entry(&name, &token);
    let token_hash = backup_target::hash_token(&token);
    let sealed = match secret_box::seal(&format!("backup-tenant:{name}"), &repo_password) {
        Ok(sealed) => sealed,
        Err(e) => return ResponseHelper::json_error(format!("Could not store the password: {e}")),
    };
    let repo_path = match backup_target::repo_path_for(&backup_target::repos_root(), &name) {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let repo_path_text = repo_path.to_string_lossy().to_string();

    let created = repo
        .create(
            &name,
            &auth_hash,
            &token_hash,
            &sealed,
            &repo_path_text,
            body.quota_bytes,
            &admin,
        )
        .await;
    let tenant = match created {
        Ok(tenant) => tenant,
        Err(e) => return ResponseHelper::json_error(format!("Failed to save the tenant: {e}")),
    };

    // Create the repository directory with the daemon's expectation, then
    // regenerate the auth file/unit so the new tenant can connect at once.
    let dir = repo_path.clone();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::events::log_event(format!(
            "Backup tenant '{name}' saved but its directory could not be created: {e}"
        ));
    }
    let settings = backup_target::load_settings();
    let tenants = repo.list().await.unwrap_or_default();
    if let Err(e) = run_op(
        "apply the backup destination",
        BACKUP_OP_TIMEOUT,
        move || backup_target::apply(&settings, &tenants),
    )
    .await
    {
        return ResponseHelper::json_error(format!(
            "The tenant was saved but the daemon could not be updated: {e}"
        ));
    }
    crate::events::log_event(format!(
        "Admin '{}' added backup tenant '{}' (quota {})",
        admin,
        tenant.name,
        if tenant.quota_bytes > 0 {
            format!("{} bytes", tenant.quota_bytes)
        } else {
            "unlimited".to_string()
        }
    ));

    // The only time the token and the repository password are ever returned.
    let settings = backup_target::load_settings();
    let host = advertised_host(&headers);
    ResponseHelper::json_success(serde_json::json!({
        "tenant": backup_target::tenant_json(
            &tenant,
            0,
            &settings,
            &host,
        ),
        "token": token,
        "repository_password": repo_password,
        "repository_url": backup_target::repository_url(&settings, &host, &tenant.name),
        "shown_once": "Copy these values now: the token and the repository password are stored only as hashes and are never shown again.",
    }))
}

/// `PUT /api/backup-target/tenants/{id}`
#[derive(Debug, Deserialize)]
pub struct UpdateTenantRequest {
    #[serde(default)]
    pub quota_bytes: Option<i64>,
    #[serde(default)]
    pub verify_enabled: Option<bool>,
}

pub async fn handle_backup_tenant_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTenantRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = BackupTenantRepository::new(state.db.pool());
    let existing = match repo.find(id).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return ResponseHelper::json_error("That tenant no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the tenant: {e}")),
    };
    let quota = body.quota_bytes.unwrap_or(existing.quota_bytes);
    if let Err(e) = backup_target::validate_quota(quota) {
        return ResponseHelper::json_error(e);
    }
    let verify = body.verify_enabled.unwrap_or(existing.verify_enabled != 0);
    if let Err(e) = repo.set_quota_and_verify(id, quota, verify).await {
        return ResponseHelper::json_error(format!("Failed to save the tenant: {e}"));
    }
    // Raising the quota may unblock a revoked tenant: re-evaluate from the
    // current size rather than waiting for the next watchdog tick.
    let tenants = repo.list().await.unwrap_or_default();
    let usage = backup_target::tenant_usage(&tenants);
    let used = usage.get(&id).copied().unwrap_or(0);
    let blocked = backup_target::quota_verdict(used, quota).blocks();
    if blocked != existing.is_over_quota() {
        let _ = repo.set_over_quota(id, blocked).await;
    }
    let settings = backup_target::load_settings();
    let tenants = repo.list().await.unwrap_or_default();
    if let Err(e) = backup_target::apply(&settings, &tenants) {
        return ResponseHelper::json_error(format!(
            "Saved, but the daemon could not be updated: {e}"
        ));
    }
    crate::events::log_event(format!(
        "Admin '{}' updated backup tenant '{}' (quota {} bytes, verification {})",
        admin,
        existing.name,
        quota,
        if verify { "on" } else { "off" }
    ));
    match payload(&state, &headers).await {
        Ok(payload) => ResponseHelper::json_success(payload),
        Err(e) => ResponseHelper::json_error(format!("Saved, but: {e}")),
    }
}

/// `POST /api/backup-target/tenants/{id}/rotate`
pub async fn handle_backup_tenant_rotate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = BackupTenantRepository::new(state.db.pool());
    let existing = match repo.find(id).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return ResponseHelper::json_error("That tenant no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the tenant: {e}")),
    };
    let token = backup_target::generate_token();
    let repo_password = backup_target::generate_repo_password();
    let auth_hash = backup_target::htpasswd_entry(&existing.name, &token);
    let token_hash = backup_target::hash_token(&token);
    let sealed = match secret_box::seal(&existing.secret_scope(), &repo_password) {
        Ok(sealed) => sealed,
        Err(e) => return ResponseHelper::json_error(format!("Could not store the password: {e}")),
    };
    let tenant = match repo
        .set_credentials(id, &auth_hash, &token_hash, &sealed)
        .await
    {
        Ok(tenant) => tenant,
        Err(e) => return ResponseHelper::json_error(format!("Failed to save the tenant: {e}")),
    };
    let settings = backup_target::load_settings();
    let tenants = repo.list().await.unwrap_or_default();
    if let Err(e) = backup_target::apply(&settings, &tenants) {
        return ResponseHelper::json_error(format!(
            "Rotated, but the daemon could not be updated: {e}"
        ));
    }
    crate::events::log_event(format!(
        "Admin '{}' rotated the credentials of backup tenant '{}'",
        admin, tenant.name
    ));
    let host = advertised_host(&headers);
    ResponseHelper::json_success(serde_json::json!({
        "token": token,
        "repository_password": repo_password,
        "repository_url": backup_target::repository_url(&settings, &host, &tenant.name),
        "warning": "A new repository password means the existing repository can no longer be read by the source. Re-initialise the repository (or keep the old password) before relying on this tenant.",
    }))
}

/// `DELETE /api/backup-target/tenants/{id}`
#[derive(Debug, Deserialize, Default)]
pub struct DeleteTenantRequest {
    /// Whether the received repository directory is deleted too.
    #[serde(default)]
    pub delete_data: bool,
}

pub async fn handle_backup_tenant_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    body: Option<Json<DeleteTenantRequest>>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = BackupTenantRepository::new(state.db.pool());
    let existing = match repo.find(id).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return ResponseHelper::json_error("That tenant no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the tenant: {e}")),
    };
    if let Err(e) = repo.delete(id).await {
        return ResponseHelper::json_error(format!("Failed to delete the tenant: {e}"));
    }
    let delete_data = body.map(|Json(b)| b.delete_data).unwrap_or(false);
    let mut data_note = String::new();
    if delete_data {
        // Only ever inside the repository root: a stored path that was edited
        // by hand must not turn a delete into `rm -rf /`.
        let root = backup_target::repos_root();
        let path = std::path::Path::new(&existing.repo_path);
        match std::fs::canonicalize(path) {
            Ok(canonical) if canonical.starts_with(&root) => {
                if let Err(e) = std::fs::remove_dir_all(&canonical) {
                    data_note = format!(" (the repository data could not be deleted: {e})");
                }
            }
            _ => {
                data_note =
                    " (the repository data was left in place: its path is outside the destination root)"
                        .to_string();
            }
        }
    }
    let settings = backup_target::load_settings();
    let tenants = repo.list().await.unwrap_or_default();
    if let Err(e) = backup_target::apply(&settings, &tenants) {
        return ResponseHelper::json_error(format!(
            "Deleted, but the daemon could not be updated: {e}"
        ));
    }
    crate::events::log_event(format!(
        "Admin '{}' deleted backup tenant '{}'{}{}",
        admin,
        existing.name,
        if delete_data {
            " and its repository data"
        } else {
            " (repository data kept)"
        },
        data_note
    ));
    match payload(&state, &headers).await {
        Ok(payload) => ResponseHelper::json_success(payload),
        Err(e) => ResponseHelper::json_error(format!("Deleted, but: {e}")),
    }
}

/// `POST /api/backup-target/tenants/{id}/verify`
pub async fn handle_backup_tenant_verify(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = BackupTenantRepository::new(state.db.pool());
    let tenant: BackupTenant = match repo.find(id).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return ResponseHelper::json_error("That tenant no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the tenant: {e}")),
    };
    if !crate::restic::ResticService::is_available() {
        return ResponseHelper::json_error("restic is not installed on this host".to_string());
    }
    let password = match secret_box::open(&tenant.secret_scope(), &tenant.repo_password_sealed) {
        Ok(password) => password,
        Err(e) => {
            return ResponseHelper::json_error(format!(
                "The repository password cannot be read: {e}"
            ))
        }
    };
    let repository = tenant.repo_path.clone();
    let result = run_op(
        "verify the backup repository",
        VERIFY_OP_TIMEOUT,
        move || {
            backup_target::verify_repository(&repository, &password).map(|outcome| outcome.message)
        },
    )
    .await;
    match &result {
        Ok(message) => {
            let _ = repo.record_verification(id, true, message).await;
            crate::events::log_event(format!(
                "Admin '{}' verified backup tenant '{}': {}",
                admin, tenant.name, message
            ));
        }
        Err(e) => {
            let _ = repo.record_verification(id, false, e).await;
            crate::events::log_event(format!(
                "Admin '{}' verified backup tenant '{}': FAILED — {e}",
                admin, tenant.name
            ));
        }
    }
    match result {
        Ok(message) => ResponseHelper::json_success(
            serde_json::json!({ "verified": true, "message": message }),
        ),
        Err(e) => ResponseHelper::json_error(format!("The repository check failed: {e}")),
    }
}
