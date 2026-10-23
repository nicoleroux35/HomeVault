/* =============================================================================
 * File          : encryption.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Standalone LUKS2 disk-encryption endpoints: list volumes with their live
 *   locked/unlocked status, create a volume on an unused device (format,
 *   passphrase, filesystem) as a tracked progress job, unlock and lock it,
 *   rotate its passphrase, and permanently erase its data (LUKS key slots +
 *   metadata) with explicit confirmation — refused while the volume is still
 *   mounted or open. Reads are auth-only; mutations are admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - uuid (>= 1.0)
 *
 * Usage:
 *   Routes under /api/encryption in src/web/mod.rs; the crypto work is
 *   delegated to crate::encryption, metadata stored via
 *   EncryptedVolumeRepository.
 * =============================================================================
 */

//! HTTP handlers for standalone LUKS2 disk encryption (manual unlock only).
//!
//! Three flows carry the safety contract of ROADMAP item 9:
//!
//! - **Create is a job** (`9.1`). `luksFormat` + `mkfs` + `mount` take minutes
//!   on a real disk, so the request validates, starts a tracked job and
//!   answers `{ job_id }` immediately; the client follows
//!   `/api/progress/{job_id}`. A failed job rolls its own container back, so
//!   the device returns to the available pool instead of being stuck "in use"
//!   with no metadata row.
//! - **Delete is gated** (`9.2`). Nothing is erased until the kernel itself
//!   reports the volume neither mounted nor mapped; a failure anywhere in the
//!   release is reported and the metadata row survives.
//! - **Unlock rolls back** (`9.3`). If `open()` succeeds and `mount()` fails,
//!   the mapper is closed again — the state that used to leave the volume
//!   "already mapped" with the UI still saying locked.

use crate::disk::{CreateProgress, DiskService};
use crate::encryption::{self, EncryptionError};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::sse::progress::{JobType, ProgressEmitter, PROGRESS_MANAGER};
use crate::storage::{EncryptedVolume, EncryptedVolumeRepository};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct EncryptedVolumeView {
    #[serde(flatten)]
    volume: crate::storage::EncryptedVolume,
    unlocked: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateEncryptedVolumeRequest {
    pub device: String,
    pub filesystem: String,
    pub label: String,
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
pub struct UnlockRequest {
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePassphraseRequest {
    pub old_passphrase: String,
    pub new_passphrase: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteVolumeRequest {
    pub confirm: bool,
}

const VALID_FILESYSTEMS: &[&str] = &["ext4", "xfs", "btrfs"];

/// Serializes every destructive volume operation (create, delete, passphrase
/// rotation, unlock/lock). Two concurrent creates on the same free device both
/// pass the availability gate — the same shape as `disk::CREATE_LOCK`.
static VOLUME_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

// ---------------------------------------------------------------------------
// Request validation (pure — pinned by the unit tests at the bottom)
// ---------------------------------------------------------------------------

/// Labels are used verbatim as the mount point's last component and inside the
/// fstab marker comment, so only `[A-Za-z0-9_-]` is accepted.
fn validate_label(label: &str) -> Result<(), String> {
    if label.trim().is_empty()
        || !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Invalid label. Use letters, numbers, hyphens and underscores only.".to_string(),
        );
    }
    Ok(())
}

/// `zfs` is refused: ZFS has its own native encryption, and the encrypted
/// volume is formatted through the mdadm-style `mkfs` dispatch.
fn validate_filesystem(filesystem: &str) -> Result<(), String> {
    if !VALID_FILESYSTEMS.contains(&filesystem) {
        return Err(format!(
            "Invalid filesystem: {}. Supported: ext4, xfs, btrfs",
            filesystem
        ));
    }
    Ok(())
}

/// Where a volume called `label` is mounted. Pinned by a test because the
/// fstab entry and the delete gate both key on it.
fn mount_point_for(label: &str) -> String {
    format!("{}/{}", crate::config::paths::ENCRYPTED_MOUNT_BASE, label)
}

/// Sentence returned when `open()` succeeded but `mount()` failed (ROADMAP
/// 9.3). The mapper is closed again; when even that fails the message says so,
/// because the volume then really is stuck open.
fn unlock_failure_message(
    mapper_name: &str,
    mount_error: &EncryptionError,
    rollback: Result<(), EncryptionError>,
) -> String {
    match rollback {
        Ok(()) => format!(
            "Unlocked but failed to mount: {mount_error}. The container was locked again — nothing changed, fix the cause and retry."
        ),
        Err(e) => format!(
            "Unlocked but failed to mount: {mount_error}. It could not be locked again either ({e}), so /dev/mapper/{mapper_name} is still open — run `sudo cryptsetup close {mapper_name}` before retrying."
        ),
    }
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// Lists encrypted volumes with their live locked/unlocked status.
///
/// The per-volume `cryptsetup status` probes run **concurrently** and each is
/// bounded by `LUKS_STATUS_TIMEOUT`: they used to run one after another, so a
/// page load paid the full latency of every probe.
pub async fn handle_encryption_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volumes = match repo.list_all().await {
        Ok(v) => v,
        Err(e) => return ResponseHelper::json_error(format!("Failed to list volumes: {}", e)),
    };

    let views = futures_util::future::join_all(volumes.into_iter().map(|volume| async move {
        let unlocked = encryption::is_open(&volume.mapper_name).await;
        EncryptedVolumeView { volume, unlocked }
    }))
    .await;

    ResponseHelper::json_success(views)
}

/// Lists block devices available to be encrypted (unused, not part of any
/// RAID array or existing filesystem) — reuses `DiskService::get_available_devices`.
pub async fn handle_encryption_available_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match DiskService::get_available_devices().await {
        Ok(devices) => ResponseHelper::json_success(
            devices
                .into_iter()
                .filter(|d| !d.in_use)
                .collect::<Vec<_>>(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to list available devices: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Create (tracked job)
// ---------------------------------------------------------------------------

/// Bridges the creation flow's percent/message milestones into the progress
/// job the request itself created.
struct CreateVolumeProgress {
    job_id: String,
}

impl CreateProgress for CreateVolumeProgress {
    fn report(&self, percent: u8, message: &str) {
        ProgressEmitter::progress(&self.job_id, percent as f64, message);
    }
}

/// Formats an available device as a new LUKS2 volume and mounts it.
///
/// Fast pre-checks (policy, filesystem, label, duplicate label, device gate)
/// still answer synchronously with a 400 so the modal fails loud *before* a
/// job starts and before any privileged command runs. The work itself
/// (`luksFormat` → `open` → `mkfs` → `mount` → fstab → metadata) runs in a
/// tracked job; the response carries its `job_id`.
#[axum_macros::debug_handler]
pub async fn handle_encryption_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateEncryptedVolumeRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // A new passphrase must clear the policy (ROADMAP 9: the old minimum was
    // 8 characters with no strength check at all).
    if let Err(problem) = encryption::validate_passphrase(&req.passphrase, &req.label) {
        return ResponseHelper::json_error(problem.message());
    }
    if let Err(message) = validate_filesystem(&req.filesystem) {
        return ResponseHelper::json_error(message);
    }
    if let Err(message) = validate_label(&req.label) {
        return ResponseHelper::json_error(message);
    }

    // The mount point and the fstab marker both key on the label: two volumes
    // sharing one label would fight over the same mount point and leave a
    // single fstab block behind on delete.
    let repo = EncryptedVolumeRepository::new(state.db.pool());
    match repo.list_all().await {
        Ok(volumes) => {
            if volumes.iter().any(|v| v.label == req.label) {
                return ResponseHelper::json_error(format!(
                    "An encrypted volume named '{}' already exists",
                    req.label
                ));
            }
        }
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to check existing volumes: {}", e));
        }
    }

    // The device string comes straight off the wire — without this check a
    // caller could pass "/dev/sda" and `cryptsetup luksFormat` would happily
    // destroy the boot disk. Only devices `DiskService` reports as present
    // and unused (system disk skipped, mounted / RAID-member devices flagged
    // `in_use`) may be encrypted — the same set the picker offers.
    match DiskService::get_available_devices().await {
        Ok(devices) => {
            let ok = devices.iter().any(|d| d.name == req.device && !d.in_use);
            if !ok {
                return ResponseHelper::json_error(format!(
                    "Device '{}' is not an available (present and unused) block device",
                    req.device
                ));
            }
        }
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to verify device: {}", e));
        }
    }

    let mapper_name = format!("hv-{}", Uuid::new_v4());
    let job_id = PROGRESS_MANAGER.create_job(
        JobType::Custom("encryption_create".to_string()),
        &format!("Encrypting '{}'", req.label),
    );
    let reporter = CreateVolumeProgress {
        job_id: job_id.clone(),
    };

    let pool = state.db.pool();
    let job_id_for_task = job_id.clone();
    let username = admin_username;
    let label = req.label.clone();
    let device = req.device.clone();
    tokio::spawn(async move {
        // Single-flight: two concurrent creates settling on the same free
        // device must not both reach `luksFormat`.
        let _guard = VOLUME_LOCK.lock().await;
        match create_volume(&pool, &req, &mapper_name, &reporter).await {
            Ok(volume) => {
                crate::events::log_event(format!(
                    "Admin '{}' created encrypted volume '{}' ({}, on {}) — mounted at {}",
                    username,
                    volume.label,
                    volume.filesystem,
                    volume.device_path,
                    volume.mount_point
                ));
                ProgressEmitter::complete(
                    &job_id_for_task,
                    &format!(
                        "Encrypted volume '{}' created and mounted at {}",
                        volume.label, volume.mount_point
                    ),
                );
            }
            Err(message) => {
                crate::events::log_event_warn(format!(
                    "Admin '{}' failed to create encrypted volume '{}' on {}: {}",
                    username, label, device, message
                ));
                ProgressEmitter::fail(&job_id_for_task, &message);
            }
        }
    });

    ResponseHelper::json_success(json!({
        "message": "Encryption started",
        "job_id": job_id,
    }))
}

/// The whole creation sequence, reporting milestones as it goes.
///
/// Every failure after `luksFormat` runs [`rollback_volume`]: a half-created
/// container must leave the device exactly as the job found it, otherwise the
/// device is no longer "available", has no metadata row, and cannot be retried
/// from the UI at all.
async fn create_volume(
    pool: &Arc<SqlitePool>,
    req: &CreateEncryptedVolumeRequest,
    mapper_name: &str,
    progress: &dyn CreateProgress,
) -> Result<EncryptedVolume, String> {
    let mount_point = mount_point_for(&req.label);
    let mapper_path = format!("/dev/mapper/{}", mapper_name);

    progress.report(5, "Formatting the LUKS2 container");
    if let Err(e) = encryption::format_luks(&req.device, &req.passphrase).await {
        // Nothing was written that this job owns: `luksFormat` failed, so
        // there is no container to erase.
        return Err(format!("Failed to encrypt the device: {e}"));
    }

    progress.report(30, "Opening the container");
    if let Err(e) = encryption::open(&req.device, mapper_name, &req.passphrase).await {
        let note = rollback_volume(&req.device, mapper_name, &mount_point, true).await;
        return Err(format!("Failed to open the new container: {e}{note}"));
    }

    progress.report(45, &format!("Creating the {} filesystem", req.filesystem));
    if let Err(e) = DiskService::format_device(&mapper_path, &req.filesystem).await {
        let note = rollback_volume(&req.device, mapper_name, &mount_point, true).await;
        return Err(format!("Failed to format the encrypted volume: {e}{note}"));
    }

    progress.report(70, "Mounting the volume");
    if let Err(e) = encryption::mount(mapper_name, &mount_point).await {
        let note = rollback_volume(&req.device, mapper_name, &mount_point, true).await;
        return Err(format!("Failed to mount the new volume: {e}{note}"));
    }

    progress.report(80, "Reading the LUKS UUID");
    let uuid = match encryption::luks_uuid(&req.device).await {
        Ok(uuid) => uuid,
        Err(e) => {
            let note = rollback_volume(&req.device, mapper_name, &mount_point, true).await;
            return Err(format!("Failed to read the LUKS UUID: {e}{note}"));
        }
    };

    progress.report(88, "Persisting the boot entry");
    if let Err(e) =
        encryption::append_fstab_entry(mapper_name, &mount_point, &req.filesystem, &req.label).await
    {
        let note = rollback_volume(&req.device, mapper_name, &mount_point, true).await;
        return Err(format!("Failed to persist the fstab entry: {e}{note}"));
    }

    progress.report(95, "Saving the volume metadata");
    let repo = EncryptedVolumeRepository::new(pool.clone());
    match repo
        .create(
            &req.device,
            mapper_name,
            &uuid,
            &req.filesystem,
            &req.label,
            &mount_point,
        )
        .await
    {
        Ok(volume) => Ok(volume),
        Err(e) => {
            let note = rollback_volume(&req.device, mapper_name, &mount_point, true).await;
            Err(format!("Failed to save the volume metadata: {e}{note}"))
        }
    }
}

/// Undoes a half-created volume: unmount, close the mapper and erase the
/// container this job just wrote, so the device returns to the available pool.
///
/// The erasure is skipped when the release did not fully succeed — erasing
/// while a mapper is still open is the one thing ROADMAP 9.2 forbids, and a
/// rollback has no more right to do it than a delete has.
///
/// Returns a sentence appended to the failure message. A step that failed is
/// **named**: the previous code hid these behind `let _ =`, which is how a
/// half-released volume survived with no metadata row.
async fn rollback_volume(
    device: &str,
    mapper_name: &str,
    mount_point: &str,
    erase_container: bool,
) -> String {
    let mut problems: Vec<String> = Vec::new();
    let mut released = true;

    if let Err(e) = encryption::unmount(mount_point).await {
        problems.push(format!("could not unmount {mount_point}: {e}"));
        released = false;
    }
    if encryption::is_open(mapper_name).await {
        match encryption::close(mapper_name).await {
            Ok(()) => {}
            Err(e) => {
                problems.push(format!("could not close {mapper_name}: {e}"));
                released = false;
            }
        }
    }
    if erase_container && released {
        match encryption::erase(device).await {
            Ok(outcome) => {
                if let Some(warning) = outcome.warning {
                    problems.push(warning);
                }
            }
            Err(e) => problems.push(format!("could not erase the new container: {e}")),
        }
    }

    if problems.is_empty() {
        ". The half-created container was rolled back — the device is available again.".to_string()
    } else {
        format!(
            ". The rollback did not fully succeed ({}), so the device may still be in use — check `sudo cryptsetup status` before retrying.",
            problems.join("; ")
        )
    }
}

// ---------------------------------------------------------------------------
// Unlock / lock
// ---------------------------------------------------------------------------

/// Unlocks and mounts a locked volume with the supplied passphrase.
///
/// A successful `open()` followed by a failed `mount()` closes the mapper
/// again (ROADMAP 9.3): leaving it open made the DB say "locked" while the
/// device was mapped, and every retry then failed with "already mapped" and no
/// button to fix it.
#[axum_macros::debug_handler]
pub async fn handle_encryption_unlock(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<UnlockRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    let _guard = VOLUME_LOCK.lock().await;

    if let Err(e) =
        encryption::open(&volume.device_path, &volume.mapper_name, &req.passphrase).await
    {
        return ResponseHelper::json_error(format!("Failed to unlock volume: {}", e));
    }

    if let Err(mount_error) = encryption::mount(&volume.mapper_name, &volume.mount_point).await {
        let rollback = encryption::close(&volume.mapper_name).await;
        let message = unlock_failure_message(&volume.mapper_name, &mount_error, rollback);
        crate::events::log_event_warn(format!(
            "Admin '{}' unlocked encrypted volume '{}' but it failed to mount: {}",
            admin_username, volume.label, mount_error
        ));
        return ResponseHelper::json_error(message);
    }

    crate::events::log_event(format!(
        "Admin '{}' unlocked encrypted volume '{}'",
        admin_username, volume.label
    ));
    ResponseHelper::json_success("Volume unlocked".to_string())
}

/// Unmounts and locks a volume.
///
/// The unmount error is no longer discarded: closing a mapper whose mount
/// point is still busy cannot work, and reporting "locked" while the
/// filesystem was still mounted is precisely the state that made the old
/// delete path destroy key material under a live mount.
pub async fn handle_encryption_lock(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    let _guard = VOLUME_LOCK.lock().await;

    if let Err(e) = encryption::unmount(&volume.mount_point).await {
        return ResponseHelper::json_error(format!(
            "Failed to unmount '{}' before locking it: {}",
            volume.label, e
        ));
    }

    match encryption::close(&volume.mapper_name).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' locked encrypted volume '{}'",
                admin_username, volume.label
            ));
            ResponseHelper::json_success("Volume locked".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to lock volume: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Passphrase rotation
// ---------------------------------------------------------------------------

/// Rotates a volume's passphrase (`cryptsetup luksChangeKey`, then a
/// `--test-passphrase` proof that the new one opens the container).
///
/// This is the operation that did not exist at all: a user who suspected a
/// leak had to reach for a shell. Works on a locked or an unlocked volume —
/// `luksChangeKey` edits a key slot, not the mapper.
#[axum_macros::debug_handler]
pub async fn handle_encryption_change_passphrase(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<ChangePassphraseRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    if req.old_passphrase.is_empty() {
        return ResponseHelper::json_error("The current passphrase is required".to_string());
    }
    if let Err(problem) = encryption::validate_passphrase(&req.new_passphrase, &volume.label) {
        return ResponseHelper::json_error(problem.message());
    }
    if req.old_passphrase == req.new_passphrase {
        return ResponseHelper::json_error(
            "The new passphrase must be different from the current one".to_string(),
        );
    }

    let _guard = VOLUME_LOCK.lock().await;

    match encryption::change_passphrase(
        &volume.device_path,
        &req.old_passphrase,
        &req.new_passphrase,
    )
    .await
    {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' rotated the passphrase of encrypted volume '{}'",
                admin_username, volume.label
            ));
            ResponseHelper::json_success("Passphrase changed".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!(
            "Failed to change the passphrase — the current passphrase is unchanged: {}",
            e
        )),
    }
}

// ---------------------------------------------------------------------------
// Delete (gated erasure)
// ---------------------------------------------------------------------------

/// Permanently deletes an encrypted volume's data (erases its LUKS key slots)
/// and removes its metadata. Requires explicit confirmation.
///
/// Nothing is erased until the volume is really released (ROADMAP 9.2). The
/// old code did `let _ = unmount(); let _ = close(); erase();` — a busy mount
/// made `umount` fail, `erase` succeeded anyway, and the key material was gone
/// while the filesystem was still mounted and being written to.
#[axum_macros::debug_handler]
pub async fn handle_encryption_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<DeleteVolumeRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !req.confirm {
        return ResponseHelper::json_error("Deletion requires confirm: true".to_string());
    }

    let repo = EncryptedVolumeRepository::new(state.db.pool());
    let volume = match repo.get_by_id(&id).await {
        Ok(Some(v)) => v,
        Ok(None) => return ResponseHelper::json_error("Volume not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to look up volume: {}", e)),
    };

    let _guard = VOLUME_LOCK.lock().await;

    if let Err(e) = encryption::unmount(&volume.mount_point).await {
        return ResponseHelper::json_error(format!(
            "Refusing to erase '{}': it could not be unmounted ({}). Nothing was erased.",
            volume.label, e
        ));
    }
    if encryption::is_open(&volume.mapper_name).await {
        if let Err(e) = encryption::close(&volume.mapper_name).await {
            return ResponseHelper::json_error(format!(
                "Refusing to erase '{}': it is still open and could not be closed ({}). Nothing was erased.",
                volume.label, e
            ));
        }
    }

    // Re-verify from the kernel's own view rather than from the exit status of
    // the two commands above: `umount` can report success for a mount point it
    // never found, and the mapper may have been reopened by something else.
    let mounted = if encryption::is_mounted(&volume.mount_point).await {
        Some(volume.mount_point.as_str())
    } else {
        None
    };
    let open = if encryption::is_open(&volume.mapper_name).await {
        Some(volume.mapper_name.as_str())
    } else {
        None
    };
    if let Err(blocker) = encryption::erase_gate(mounted, open) {
        return ResponseHelper::json_error(format!(
            "Refusing to erase '{}': {}. Nothing was erased.",
            volume.label, blocker
        ));
    }

    let outcome = match encryption::erase(&volume.device_path).await {
        Ok(outcome) => outcome,
        Err(e) => return ResponseHelper::json_error(format!("Failed to erase volume: {}", e)),
    };

    // The key slots are gone — the data is unrecoverable — so the remaining
    // cleanup problems are reported, not hidden behind an "all good".
    let mut warnings: Vec<String> = Vec::new();
    if let Some(warning) = outcome.warning {
        warnings.push(warning);
    }
    if let Err(e) = encryption::remove_fstab_entry(&volume.label).await {
        warnings.push(format!(
            "The fstab entry could not be removed ({}); delete the `# homevault-encrypted:{}` block from /etc/fstab by hand.",
            e, volume.label
        ));
    }

    match repo.delete(&id).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' permanently deleted encrypted volume '{}'",
                admin_username, volume.label
            ));
            ResponseHelper::json_success(json!({
                "message": "Volume deleted",
                "warnings": warnings,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!(
            "The key slots of '{}' are already destroyed, but its metadata could not be removed: {}",
            volume.label, e
        )),
    }
}

#[cfg(test)]
mod tests {
    //! Pure decisions of this module: the two request validators, the mount
    //! point the fstab entry and the delete gate both key on, and the two
    //! branches of the unlock rollback message (ROADMAP 9.3). The flows
    //! themselves shell out to `cryptsetup` and are covered by
    //! `tests/encryption_integration.rs` at the guard/refusal level.
    use super::*;

    #[test]
    fn test_validate_label_accepts_letters_digits_and_separators() {
        for ok in ["vault", "archive-2026", "media_1", "A1"] {
            assert_eq!(validate_label(ok), Ok(()), "{ok:?} must be accepted");
        }
    }

    #[test]
    fn test_validate_label_rejects_empty_spaces_slashes_and_traversal() {
        for bad in ["", "   ", "bad label", "bad/label", "../escape", "vault\n"] {
            let error = validate_label(bad).expect_err(bad);
            assert!(error.contains("Invalid label"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn test_validate_filesystem_is_an_allow_list() {
        for ok in ["ext4", "xfs", "btrfs"] {
            assert_eq!(validate_filesystem(ok), Ok(()));
        }
        for bad in ["zfs", "ntfs", "exfat", ""] {
            let error = validate_filesystem(bad).expect_err(bad);
            assert!(error.contains("Invalid filesystem"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn test_mount_point_is_derived_from_the_label() {
        assert_eq!(
            mount_point_for("vault"),
            format!("{}/vault", crate::config::paths::ENCRYPTED_MOUNT_BASE)
        );
    }

    #[test]
    fn test_unlock_failure_message_names_the_rollback() {
        let message = unlock_failure_message(
            "hv-1",
            &EncryptionError::CommandFailed("mount: wrong fs type".into()),
            Ok(()),
        );
        assert!(message.contains("failed to mount"), "{message}");
        assert!(message.contains("locked again"), "{message}");
        assert!(
            !message.contains("cryptsetup close"),
            "a successful rollback must not tell the operator to clean up: {message}"
        );
    }

    #[test]
    fn test_unlock_failure_message_admits_a_failed_rollback() {
        let message = unlock_failure_message(
            "hv-1",
            &EncryptionError::CommandFailed("mount: wrong fs type".into()),
            Err(EncryptionError::CommandFailed("device is busy".into())),
        );
        assert!(message.contains("still open"), "{message}");
        assert!(message.contains("cryptsetup close hv-1"), "{message}");
        assert!(message.contains("device is busy"), "{message}");
    }
}
