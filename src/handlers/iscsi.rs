/* =============================================================================
 * File          : iscsi.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   iSCSI export endpoints: read what the host can do and what is saved, flip
 *   the feature switch, CRUD backstores and targets (portals, LUNs, initiator
 *   ACLs, optional CHAP) and apply the metadata to the kernel.
 *
 *   Reads need a session; every mutation is admin-only, because apply runs
 *   root-owned `targetcli` commands. Saving metadata never touches the host:
 *   only Apply (or turning the feature off) does.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::iscsi, crate::storage::IscsiRepository
 *
 * Usage:
 *   Routes under /api/storage/iscsi in src/web/mod.rs; see
 *   docs/guides/iscsi.md.
 * =============================================================================
 */

//! iSCSI export endpoints, see docs/guides/iscsi.md.
//!
//! * `GET    /api/storage/iscsi`                  — settings + capabilities + objects
//! * `PUT    /api/storage/iscsi/settings`         — feature switch
//! * `POST   /api/storage/iscsi/backstores`       — add a backstore
//! * `PUT    /api/storage/iscsi/backstores/{id}`  — edit a backstore
//! * `DELETE /api/storage/iscsi/backstores/{id}`  — forget (and optionally delete) one
//! * `POST   /api/storage/iscsi/targets`          — add a target
//! * `PUT    /api/storage/iscsi/targets/{id}`     — replace a target
//! * `DELETE /api/storage/iscsi/targets/{id}`     — forget a target
//! * `POST   /api/storage/iscsi/apply`            — apply now

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::iscsi::{self, IscsiApplyReport, IscsiBackstore, IscsiSettings, IscsiTarget};
use crate::storage::{IscsiAclInput, IscsiRepository, IscsiTargetInput};
use crate::utils::ResponseHelper;

use super::AppState;

/// Smallest fileio backstore HomeVault will create. Below this a LUN is not a
/// usable disk, and a 0-byte file would be recreated by `targetcli` on every
/// apply.
const MIN_FILEIO_SIZE: i64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct BackstoreRequest {
    pub name: String,
    /// `fileio` | `block`
    pub kind: String,
    pub path: String,
    #[serde(default)]
    pub size_bytes: i64,
    #[serde(default)]
    pub readonly: bool,
    /// `buffered` (advertise a write cache) | `direct` (write-through).
    #[serde(default = "default_write_policy")]
    pub write_policy: String,
    /// Honour the SCSI FUA bit.
    #[serde(default = "default_true")]
    pub emulate_fua: bool,
    /// Advertise thin provisioning / UNMAP.
    #[serde(default = "default_true")]
    pub unmap: bool,
}

fn default_write_policy() -> String {
    "buffered".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct DeleteBackstoreQuery {
    /// Delete the backing file too. Only honoured with `confirm`.
    #[serde(default)]
    pub delete_file: bool,
    /// The backstore name typed back, the confirmation for a data deletion.
    #[serde(default)]
    pub confirm: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PortalRequest {
    pub address: String,
    #[serde(default)]
    pub port: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct LunRequest {
    pub lun: i64,
    pub backstore_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct AclRequest {
    pub initiator_iqn: String,
    #[serde(default)]
    pub userid: Option<String>,
    /// A plaintext CHAP secret. `null` keeps the stored one on an update.
    #[serde(default)]
    pub secret: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TargetRequest {
    pub iqn: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub portals: Vec<PortalRequest>,
    #[serde(default)]
    pub luns: Vec<LunRequest>,
    #[serde(default)]
    pub acls: Vec<AclRequest>,
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// Directories a fileio backstore may live in: the data directory (where the
/// default `iscsi` folder sits) and every mounted pool. Computed on the
/// blocking pool because it shells out to `zpool`/`btrfs`.
async fn allowed_roots() -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(data) = std::env::var("DATA_DIRECTORY") {
        roots.push(data);
    }
    let pools = tokio::task::spawn_blocking(crate::pools::PoolService::scan_pools)
        .await
        .unwrap_or_else(|_| Ok(Vec::new()))
        .unwrap_or_default();
    for pool in pools {
        if let Some(mount) = pool.mount_point {
            roots.push(mount);
        }
    }
    roots
}

/// `{DATA_DIRECTORY}/iscsi` — where the UI suggests putting a new fileio
/// backstore. Created on demand (the data directory is HomeVault-owned).
pub fn default_fileio_root() -> PathBuf {
    let data = std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string());
    FsPath::new(&data).join("iscsi")
}

fn ensure_fileio_root() -> Result<(), String> {
    let root = default_fileio_root();
    if root.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("Could not create {}: {e}", root.display()))
}

/// Whether `path` sits inside one of the roots. Symlinks are not resolved
/// here: the point is to refuse `/etc/shadow`, not to police mounts.
fn under_any_root(path: &str, roots: &[String]) -> bool {
    let candidate = FsPath::new(path);
    roots.iter().any(|root| {
        let root = FsPath::new(root);
        candidate.starts_with(root)
    })
}

fn settings_json(settings: &IscsiSettings) -> serde_json::Value {
    serde_json::json!({
        "enabled": settings.enabled,
        "applied_at": settings.applied_at,
        "updated_at": settings.updated_at,
    })
}

/// Metadata is pending when the newest row is younger than the last apply.
fn is_pending(
    settings: &IscsiSettings,
    backstores: &[IscsiBackstore],
    targets: &[IscsiTarget],
) -> bool {
    if !settings.enabled {
        return false;
    }
    let Some(applied_at) = settings.applied_at.as_deref() else {
        // Never applied: anything saved is pending, including nothing.
        return !backstores.is_empty() || targets.iter().any(|t| t.enabled);
    };
    backstores
        .iter()
        .map(|b| b.updated_at.as_str())
        .chain(targets.iter().map(|t| t.updated_at.as_str()))
        .any(|stamp| stamp > applied_at)
}

fn payload(
    settings: &IscsiSettings,
    capabilities: &iscsi::IscsiCapabilities,
    backstores: &[IscsiBackstore],
    targets: &[IscsiTarget],
    roots: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "settings": settings_json(settings),
        "capabilities": capabilities,
        "backstores": backstores,
        "targets": targets,
        "paths": {
            "fileio_root": default_fileio_root().to_string_lossy(),
            "allowed_roots": roots,
        },
        "defaults": {
            "portal_address": iscsi::DEFAULT_PORTAL_ADDRESS,
            "portal_port": iscsi::DEFAULT_PORTAL_PORT,
            "iqn": format!(
                "iqn.{}.local.homevault:disk0",
                Utc::now().format("%Y-%m")
            ),
        },
        "state": {
            "usable": capabilities.usable,
            "enabled": settings.enabled,
            "pending": is_pending(settings, backstores, targets),
            "applied_at": settings.applied_at,
            "exported_targets": targets.iter().filter(|t| t.enabled).count(),
        },
    })
}

/// Read the saved state and render it. Shared by every endpoint that returns
/// the whole screen payload after a mutation.
async fn current_payload(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
    let repo = IscsiRepository::new(state.db.pool());
    let settings = iscsi::load();
    let capabilities = iscsi::capabilities();
    let backstores = repo
        .list_backstores()
        .await
        .map_err(|e| format!("Failed to read the backstores: {e}"))?;
    let targets = repo
        .list_targets()
        .await
        .map_err(|e| format!("Failed to read the targets: {e}"))?;
    let roots = allowed_roots().await;
    Ok(payload(
        &settings,
        &capabilities,
        &backstores,
        &targets,
        &roots,
    ))
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// The canonical stored form of a backstore body.
struct ValidatedBackstore {
    name: String,
    kind: String,
    path: String,
    size_bytes: i64,
    readonly: bool,
    write_policy: String,
    emulate_fua: bool,
    unmap: bool,
}

/// Validate a backstore body and return the canonical stored form.
async fn validated_backstore(
    body: &BackstoreRequest,
    roots: &[String],
) -> Result<ValidatedBackstore, String> {
    let name = body.name.trim().to_string();
    if !iscsi::valid_backstore_name(&name) {
        return Err(format!(
            "'{name}' is not a valid backstore name: use letters, digits, '.', '_' or '-', \
             at most {} characters",
            iscsi::MAX_BACKSTORE_NAME
        ));
    }
    let kind = body.kind.trim().to_ascii_lowercase();
    if kind != "fileio" && kind != "block" {
        return Err("A backstore kind must be 'fileio' or 'block'".to_string());
    }
    let write_policy = body.write_policy.trim().to_ascii_lowercase();
    if !iscsi::valid_write_policy(&write_policy) {
        return Err("write_policy must be 'buffered' or 'direct'".to_string());
    }
    let path = body.path.trim().to_string();
    iscsi::validate_backstore_path(&path)?;

    if kind == "block" {
        if !path.starts_with("/dev/") {
            return Err("A block backstore must point at a device under /dev".to_string());
        }
        let metadata =
            std::fs::metadata(&path).map_err(|_| format!("{path} does not exist on this host"))?;
        use std::os::unix::fs::FileTypeExt;
        if !metadata.file_type().is_block_device() {
            return Err(format!("{path} is not a block device"));
        }
        return Ok(ValidatedBackstore {
            name,
            kind,
            path,
            size_bytes: 0,
            readonly: body.readonly,
            write_policy,
            emulate_fua: body.emulate_fua,
            unmap: body.unmap,
        });
    }

    if !under_any_root(&path, roots) {
        return Err(format!(
            "The backing file must live in {} or on a storage pool",
            default_fileio_root().display()
        ));
    }
    let parent = FsPath::new(&path)
        .parent()
        .ok_or_else(|| format!("{path} has no parent directory"))?;
    if !parent.is_dir() {
        return Err(format!(
            "The folder {} does not exist — create it first",
            parent.display()
        ));
    }
    if !(MIN_FILEIO_SIZE..=iscsi::MAX_BACKSTORE_SIZE).contains(&body.size_bytes) {
        return Err(format!(
            "A disk size must be between {} MiB and {} TiB",
            MIN_FILEIO_SIZE / (1024 * 1024),
            iscsi::MAX_BACKSTORE_SIZE / (1024 * 1024 * 1024 * 1024)
        ));
    }
    Ok(ValidatedBackstore {
        name,
        kind,
        path,
        size_bytes: body.size_bytes,
        readonly: body.readonly,
        write_policy,
        emulate_fua: body.emulate_fua,
        unmap: body.unmap,
    })
}

/// Validate a target body and turn it into the repository input, sealing any
/// new CHAP secret. `existing` is the target being updated (so a secret the
/// client did not resend can be preserved).
async fn validated_target(
    repo: &IscsiRepository,
    body: &TargetRequest,
    existing: Option<&IscsiTarget>,
) -> Result<IscsiTargetInput, String> {
    let iqn = body.iqn.trim().to_string();
    let portals: Vec<(String, i64)> = body
        .portals
        .iter()
        .map(|p| {
            (
                p.address.trim().to_string(),
                p.port.unwrap_or(iscsi::DEFAULT_PORTAL_PORT as i64),
            )
        })
        .collect();
    let luns: Vec<(i64, i64)> = body.luns.iter().map(|l| (l.lun, l.backstore_id)).collect();

    // Resolve each initiator's CHAP first: an update may omit `secret` to keep
    // the one already stored. Validation then runs on the real value, so the
    // "user name without a secret" rule still holds for a brand-new initiator.
    let mut acl_tuples: Vec<(String, Option<String>, Option<String>)> =
        Vec::with_capacity(body.acls.len());
    for acl in &body.acls {
        let initiator = acl.initiator_iqn.trim().to_string();
        let userid = acl.userid.as_ref().map(|u| u.trim().to_string());
        let secret: Option<String> = match (&userid, &acl.secret) {
            (None, _) => None,
            (Some(_), Some(secret)) => Some(secret.clone()),
            (Some(_), None) => {
                let stored = match existing {
                    Some(target) => match target
                        .acls
                        .iter()
                        .find(|a| a.initiator_iqn.eq_ignore_ascii_case(&initiator))
                    {
                        Some(old) if old.has_secret => {
                            let sealed = repo
                                .chap_secret_for(target.id, &old.initiator_iqn)
                                .await
                                .map_err(|e| format!("Failed to read the stored CHAP secret: {e}"))?
                                .ok_or_else(|| {
                                    "The stored CHAP secret is missing — enter it again".to_string()
                                })?;
                            Some(
                                crate::storage::secret_box::open(
                                    &iscsi::chap_scope(&target.iqn, &old.initiator_iqn),
                                    &sealed,
                                )
                                .map_err(|e| {
                                    format!("Could not read the stored CHAP secret: {e}")
                                })?,
                            )
                        }
                        _ => None,
                    },
                    None => None,
                };
                match stored {
                    Some(secret) => Some(secret),
                    None => {
                        return Err(format!(
                            "The initiator '{initiator}' uses CHAP: give a 12–16 character secret"
                        ))
                    }
                }
            }
        };
        acl_tuples.push((initiator, userid, secret));
    }

    iscsi::validate_target(
        &iqn,
        &portals,
        &luns.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        &acl_tuples,
    )?;

    for (_, backstore_id) in &luns {
        match repo.find_backstore(*backstore_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err("A LUN points at a backstore that no longer exists".to_string()),
            Err(e) => return Err(format!("Failed to read the backstore: {e}")),
        }
    }

    let mut acls = Vec::with_capacity(acl_tuples.len());
    for (initiator, userid, secret) in acl_tuples {
        // Re-seal with a fresh nonce on every save. The scope is derived from
        // the target IQN and the initiator, so a blob cannot be opened under
        // another export.
        let sealed = match (&userid, &secret) {
            (Some(_), Some(secret)) => Some(
                crate::storage::secret_box::seal(&iscsi::chap_scope(&iqn, &initiator), secret)
                    .map_err(|e| format!("Could not store the CHAP secret: {e}"))?,
            ),
            _ => None,
        };
        acls.push(IscsiAclInput {
            initiator_iqn: initiator,
            userid,
            chap_secret_sealed: sealed,
        });
    }

    Ok(IscsiTargetInput {
        iqn,
        enabled: body.enabled,
        portals,
        luns,
        acls,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/storage/iscsi`
pub async fn handle_iscsi_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    match current_payload(&state).await {
        Ok(payload) => ResponseHelper::json_success(payload),
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// `PUT /api/storage/iscsi/settings`
///
/// Turning the feature **off** tears the exports down immediately: leaving a
/// LUN reachable after the admin switched it off would be a silent export.
/// Turning it on only saves the switch — activating an export stays an
/// explicit Apply.
pub async fn handle_iscsi_settings_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SettingsRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let mut settings = iscsi::new_settings(body.enabled);
    if body.enabled {
        if let Err(e) = ensure_fileio_root() {
            return ResponseHelper::json_error(e);
        }
    }
    if let Err(e) = iscsi::save(&settings) {
        return ResponseHelper::json_error(e);
    }

    let mut teardown = None;
    if !body.enabled {
        // Reuse the apply path, which walks the saved metadata and deletes
        // exactly the targets it owns.
        teardown = Some(run_apply(state.db.pool()).await);
        if let Some(report) = teardown.as_ref() {
            if report.applied {
                settings.applied_at = Some(Utc::now().to_rfc3339());
                let _ = iscsi::save(&settings);
            }
        }
    }

    crate::events::log_event(format!(
        "Admin '{}' turned iSCSI {}",
        admin,
        if body.enabled { "on" } else { "off" }
    ));
    match current_payload(&state).await {
        Ok(mut payload) => {
            if let Some(report) = teardown {
                payload["apply"] = serde_json::to_value(report).unwrap_or_default();
            }
            ResponseHelper::json_success(payload)
        }
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// `POST /api/storage/iscsi/backstores`
pub async fn handle_iscsi_backstore_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BackstoreRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let roots = allowed_roots().await;
    let store = match validated_backstore(&body, &roots).await {
        Ok(parts) => parts,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let (name, kind, path) = (store.name.clone(), store.kind.clone(), store.path.clone());

    let repo = IscsiRepository::new(state.db.pool());
    match repo.backstore_name_taken(&name, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!("A backstore named '{name}' already exists"))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }
    match repo.backstore_path_taken(&path, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "Another backstore already uses {path} — two objects over one file would corrupt it"
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the path: {e}")),
    }

    match repo
        .create_backstore(
            &name,
            &kind,
            &path,
            store.size_bytes,
            store.readonly,
            &store.write_policy,
            store.emulate_fua,
            store.unmap,
        )
        .await
    {
        Ok(store) => {
            crate::events::log_event(format!(
                "Admin '{}' added iSCSI backstore '{}' ({kind}, {path})",
                admin, store.name
            ));
            match current_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(e),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the backstore: {e}")),
    }
}

/// `PUT /api/storage/iscsi/backstores/{id}`
pub async fn handle_iscsi_backstore_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<BackstoreRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let roots = allowed_roots().await;
    let store = match validated_backstore(&body, &roots).await {
        Ok(parts) => parts,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let (name, kind, path) = (store.name.clone(), store.kind.clone(), store.path.clone());

    let repo = IscsiRepository::new(state.db.pool());
    match repo.find_backstore(id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ResponseHelper::json_error("That backstore no longer exists".to_string())
        }
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the backstore: {e}")),
    }
    match repo.backstore_name_taken(&name, Some(id)).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!("A backstore named '{name}' already exists"))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }
    match repo.backstore_path_taken(&path, Some(id)).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "Another backstore already uses {path} — two objects over one file would corrupt it"
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the path: {e}")),
    }

    match repo
        .update_backstore(
            id,
            &name,
            &kind,
            &path,
            store.size_bytes,
            store.readonly,
            &store.write_policy,
            store.emulate_fua,
            store.unmap,
        )
        .await
    {
        Ok(store) => {
            crate::events::log_event(format!(
                "Admin '{}' changed iSCSI backstore '{}'",
                admin, store.name
            ));
            match current_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(e),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the backstore: {e}")),
    }
}

/// `DELETE /api/storage/iscsi/backstores/{id}`
///
/// Refused while a LUN still exports it. The backing file is only deleted when
/// `delete_file=true` **and** `confirm` repeats the backstore name — and never
/// for a `block` backstore, which is a device, not a file HomeVault made.
pub async fn handle_iscsi_backstore_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<DeleteBackstoreQuery>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let repo = IscsiRepository::new(state.db.pool());
    let store = match repo.find_backstore(id).await {
        Ok(Some(store)) => store,
        Ok(None) => {
            return ResponseHelper::json_error("That backstore no longer exists".to_string())
        }
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the backstore: {e}")),
    };

    match repo.backstore_lun_count(id).await {
        Ok(0) => {}
        Ok(_) => {
            let targets = repo
                .backstore_exported_targets(id)
                .await
                .unwrap_or_default();
            return ResponseHelper::json_error(format!(
                "'{}' is still used by {} — remove that LUN first",
                store.name,
                if targets.is_empty() {
                    "a target".to_string()
                } else {
                    targets.join(", ")
                }
            ));
        }
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the LUNs: {e}")),
    }

    let delete_file = query.delete_file;
    if delete_file {
        if store.kind() == iscsi::BackstoreKind::Block {
            return ResponseHelper::json_error(
                "A block backstore points at a device — HomeVault will not delete it".to_string(),
            );
        }
        if query.confirm.as_deref().map(str::trim) != Some(store.name.as_str()) {
            return ResponseHelper::json_error(format!(
                "Type '{}' to confirm deleting the backing file",
                store.name
            ));
        }
    }

    // Remove the kernel object first: a metadata row whose object lingers is
    // invisible in the UI but still exportable.
    let capabilities = iscsi::capabilities();
    if capabilities.usable {
        let command = iscsi::backstore_delete_command(&store);
        let ok = tokio::task::spawn_blocking(move || {
            crate::iscsi::run_command_blocking(&command, iscsi::TARGETCLI_TIMEOUT)
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(e) = ok {
            return ResponseHelper::json_error(format!(
                "The backstore could not be removed from the host: {e}"
            ));
        }
    }

    if delete_file {
        if let Err(e) = iscsi::delete_backing_file(&store.path) {
            return ResponseHelper::json_error(format!(
                "The metadata was kept because the file could not be deleted: {e}"
            ));
        }
    }

    match repo.delete_backstore(id).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' forgot iSCSI backstore '{}'{}",
                admin,
                store.name,
                if delete_file {
                    " and deleted its backing file"
                } else {
                    ""
                }
            ));
            match current_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(e),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete the backstore: {e}")),
    }
}

/// `POST /api/storage/iscsi/targets`
pub async fn handle_iscsi_target_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TargetRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let repo = IscsiRepository::new(state.db.pool());
    let input = match validated_target(&repo, &body, None).await {
        Ok(input) => input,
        Err(e) => return ResponseHelper::json_error(e),
    };
    match repo.target_iqn_taken(&input.iqn, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "A target with the IQN '{}' already exists",
                input.iqn
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the IQN: {e}")),
    }
    match repo.list_targets().await {
        Ok(targets) if targets.len() >= iscsi::MAX_TARGETS => {
            return ResponseHelper::json_error(format!(
                "The list is full ({} targets) — delete one first",
                iscsi::MAX_TARGETS
            ))
        }
        Ok(_) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the targets: {e}")),
    }

    match repo.create_target(&input).await {
        Ok(target) => {
            crate::events::log_event(format!(
                "Admin '{}' added iSCSI target '{}'",
                admin, target.iqn
            ));
            match current_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(e),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the target: {e}")),
    }
}

/// `PUT /api/storage/iscsi/targets/{id}`
pub async fn handle_iscsi_target_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<TargetRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let repo = IscsiRepository::new(state.db.pool());
    let existing = match repo.find_target(id).await {
        Ok(Some(target)) => target,
        Ok(None) => return ResponseHelper::json_error("That target no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the target: {e}")),
    };
    let input = match validated_target(&repo, &body, Some(&existing)).await {
        Ok(input) => input,
        Err(e) => return ResponseHelper::json_error(e),
    };
    match repo.target_iqn_taken(&input.iqn, Some(id)).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "A target with the IQN '{}' already exists",
                input.iqn
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the IQN: {e}")),
    }

    // The IQN names the kernel object; renaming it would leave the old export
    // in place under a name the metadata no longer lists. Refuse until the old
    // one has been deleted, and say so.
    if !existing.iqn.eq_ignore_ascii_case(&input.iqn) {
        return ResponseHelper::json_error(format!(
            "The IQN cannot be changed in place — delete this target and create '{}' instead",
            input.iqn
        ));
    }

    match repo.update_target(id, &input).await {
        Ok(target) => {
            crate::events::log_event(format!(
                "Admin '{}' changed iSCSI target '{}'",
                admin, target.iqn
            ));
            match current_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(e),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the target: {e}")),
    }
}

/// `DELETE /api/storage/iscsi/targets/{id}`
pub async fn handle_iscsi_target_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let repo = IscsiRepository::new(state.db.pool());
    let target = match repo.find_target(id).await {
        Ok(Some(target)) => target,
        Ok(None) => return ResponseHelper::json_error("That target no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the target: {e}")),
    };

    // Unexport first, so the initiator loses the disk before its metadata is
    // gone. A host without the LIO stack has nothing to unexport.
    let capabilities = iscsi::capabilities();
    if capabilities.usable {
        let iqn = target.iqn.clone();
        let ok = tokio::task::spawn_blocking(move || crate::iscsi::unexport_target_blocking(&iqn))
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(e) = ok {
            return ResponseHelper::json_error(format!(
                "The target could not be unexported from the host: {e}"
            ));
        }
    }

    match repo.delete_target(id).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' forgot iSCSI target '{}'",
                admin, target.iqn
            ));
            match current_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(payload),
                Err(e) => ResponseHelper::json_error(e),
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete the target: {e}")),
    }
}

/// `POST /api/storage/iscsi/apply`
pub async fn handle_iscsi_apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let report = run_apply(state.db.pool()).await;

    let mut settings = iscsi::load();
    if report.applied {
        settings.applied_at = Some(Utc::now().to_rfc3339());
        if let Err(e) = iscsi::save(&settings) {
            return ResponseHelper::json_error(e);
        }
    }
    crate::events::log_event(format!(
        "Admin '{}' applied the iSCSI configuration ({})",
        admin,
        if report.applied {
            format!("{} command(s)", report.commands.len())
        } else {
            report
                .reason
                .clone()
                .unwrap_or_else(|| "nothing to apply".to_string())
        }
    ));

    match current_payload(&state).await {
        Ok(mut payload) => {
            payload["apply"] = serde_json::to_value(&report).unwrap_or_default();
            ResponseHelper::json_success(payload)
        }
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// Read the metadata, open the CHAP secrets and apply on the blocking pool.
pub(crate) async fn run_apply(pool: Arc<sqlx::SqlitePool>) -> IscsiApplyReport {
    let repo = IscsiRepository::new(pool);
    let settings = iscsi::load();
    let backstores = match repo.list_backstores().await {
        Ok(list) => list,
        Err(e) => {
            return IscsiApplyReport {
                applied: false,
                reason: Some(format!("Failed to read the backstores: {e}")),
                reason_code: None,
                commands: Vec::new(),
                failed: Some(e.to_string()),
            }
        }
    };
    let targets = match repo.list_targets().await {
        Ok(list) => list,
        Err(e) => {
            return IscsiApplyReport {
                applied: false,
                reason: Some(format!("Failed to read the targets: {e}")),
                reason_code: None,
                commands: Vec::new(),
                failed: Some(e.to_string()),
            }
        }
    };
    let sealed = repo.chap_secrets_sealed().await.unwrap_or_default();
    let mut secrets = Vec::with_capacity(sealed.len());
    for (iqn, initiator, blob) in sealed {
        match crate::storage::secret_box::open(&iscsi::chap_scope(&iqn, &initiator), &blob) {
            Ok(secret) => secrets.push((iqn, initiator, secret)),
            Err(e) => {
                // A secret that cannot be opened is a broken export: fail
                // loudly instead of exporting an unauthenticated LUN.
                return IscsiApplyReport {
                    applied: false,
                    reason: Some(format!(
                        "The stored CHAP secret for '{initiator}' cannot be read: {e}"
                    )),
                    reason_code: None,
                    commands: Vec::new(),
                    failed: Some(e),
                };
            }
        }
    }

    tokio::task::spawn_blocking(move || {
        iscsi::apply_sync(&settings, &backstores, &targets, &secrets)
    })
    .await
    .unwrap_or_else(|e| IscsiApplyReport {
        applied: false,
        reason: Some("The iSCSI worker failed".to_string()),
        reason_code: None,
        commands: Vec::new(),
        failed: Some(e.to_string()),
    })
}
