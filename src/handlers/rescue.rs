/* =============================================================================
 * File          : rescue.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 2.0.0
 *
 * Description:
 *   Data-recovery endpoints: tool availability, device listing, disk cloning
 *   with ddrescue (background job, live SSE progress, mapfile status, working
 *   cancel), partition analysis and scan with TestDisk, file carving with
 *   PhotoRec and mdadm array diagnose/repair. All endpoints are admin-only.
 *   Every device/path goes through the rescue SecurityValidator, and every
 *   option that reaches a tool's argv is validated — an invalid *or simply
 *   unhonourable* option is a 400 naming the field, never a silent fallback.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/rescue in src/web/mod.rs; execution delegated to
 *   crate::rescue::RescueService (see docs/guides/rescue.md).
 * =============================================================================
 */

//! Data-recovery endpoints (ddrescue cloning, TestDisk analyze/rebuild,
//! PhotoRec carving, mdadm repair) — all admin-only, options validated
//! before execution.

use crate::sse::{JobType, PROGRESS_MANAGER};
use axum::{extract::Path, extract::State, http::HeaderMap, response::Response, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppStateArc;
use crate::rescue::{
    DdrescueOptions, MdadmOptions, PhotoRecOptions, RescueError, RescueService, SecurityValidator,
};
use crate::sse::ProgressEmitter;
use crate::utils::ResponseHelper;

#[derive(Debug, Serialize)]
pub struct ToolsResponse {
    pub tools: Vec<crate::rescue::ToolStatus>,
}

#[derive(Debug, Deserialize)]
pub struct DdrescueCloneRequest {
    pub source: String,
    pub destination: String,
    pub logfile: Option<String>,
    pub block_size: Option<u32>,
    pub retry_passes: Option<u32>,
    pub direct_io: Option<bool>,
    pub reverse: Option<bool>,
    pub verbose: Option<bool>,
}

impl From<DdrescueCloneRequest> for DdrescueOptions {
    fn from(r: DdrescueCloneRequest) -> Self {
        DdrescueOptions {
            logfile: r.logfile,
            block_size: r.block_size,
            retry_passes: r.retry_passes,
            direct_io: r.direct_io,
            reverse: r.reverse,
            verbose: r.verbose,
        }
    }
}

/// TestDisk analyse request.
///
/// `deep_search` and `log_path` are **rejected** when sent: the scripted mode
/// this integration drives (`analyze,search`) always performs the deep scan
/// and TestDisk writes its own log, so both settings were accepted by the API,
/// shown as applied in the UI, and never reached the tool.
#[derive(Debug, Deserialize)]
pub struct TestDiskAnalyzeRequest {
    pub device: String,
    #[serde(default)]
    pub deep_search: Option<bool>,
    #[serde(default)]
    pub log_path: Option<String>,
}

impl TestDiskAnalyzeRequest {
    fn reject_unsupported(&self) -> Result<(), RescueError> {
        if self.deep_search.is_some() {
            return Err(RescueError::InvalidInput(
                "deep_search is not supported: this integration always performs the deep scan (analyze,search)"
                    .to_string(),
            ));
        }
        if self.log_path.as_deref().is_some_and(|p| !p.is_empty()) {
            return Err(RescueError::InvalidInput(
                "log_path is not supported: TestDisk writes its own log".to_string(),
            ));
        }
        Ok(())
    }
}

/// TestDisk rebuild request — same rejections as the analyse, plus `force`
/// (the partition table is never written back automatically, so a "force
/// write" switch could only mislead).
#[derive(Debug, Deserialize)]
pub struct TestDiskRebuildRequest {
    pub device: String,
    #[serde(default)]
    pub deep_search: Option<bool>,
    #[serde(default)]
    pub force: Option<bool>,
}

impl TestDiskRebuildRequest {
    fn reject_unsupported(&self) -> Result<(), RescueError> {
        if self.deep_search.is_some() {
            return Err(RescueError::InvalidInput(
                "deep_search is not supported: this integration always performs the deep scan (analyze,search)"
                    .to_string(),
            ));
        }
        if self.force.unwrap_or(false) {
            return Err(RescueError::InvalidInput(
                "force is not supported: the partition table is never written back automatically"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// PhotoRec request. Only the two knobs the packaged PhotoRec can honour:
/// `keep_broken` (no working `keep_corrupted_file` variant exists on 7.x) and
/// `max_size` (absent from the `/cmd` grammar) are gone rather than kept as
/// accepted-and-ignored fields.
#[derive(Debug, Deserialize)]
pub struct PhotoRecRequest {
    pub device: String,
    pub output_dir: String,
    pub file_types: Option<Vec<String>>,
    pub partition_type: Option<String>,
}

impl From<PhotoRecRequest> for PhotoRecOptions {
    fn from(r: PhotoRecRequest) -> Self {
        PhotoRecOptions {
            file_types: r.file_types,
            partition_type: r.partition_type,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MdadmRequest {
    pub name: String,
    pub action: Option<String>,
    pub force: Option<bool>,
    pub verbose: Option<bool>,
}

impl From<MdadmRequest> for MdadmOptions {
    fn from(r: MdadmRequest) -> Self {
        MdadmOptions {
            action: r.action,
            force: r.force,
            verbose: r.verbose,
        }
    }
}

/// Map a service error onto the HTTP response. `InvalidInput` is the client's
/// mistake and keeps its own message (it names the offending field); the rest
/// are runtime/tool failures and get the caller's prefix.
fn service_error(prefix: &str, error: RescueError) -> Response {
    match error {
        RescueError::InvalidInput(msg) => ResponseHelper::json_error(msg),
        other => ResponseHelper::json_error(format!("{prefix}: {other}")),
    }
}

pub async fn handle_rescue_tools_available(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let tools = RescueService::check_tools().await;
    ResponseHelper::json_success(ToolsResponse { tools })
}

pub async fn handle_rescue_devices(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    match RescueService::list_block_devices().await {
        Ok(devices) => ResponseHelper::json_success(devices),
        Err(e) => service_error("Failed to list devices", e),
    }
}

pub async fn handle_ddrescue_clone(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
    Json(payload): Json<DdrescueCloneRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    // Validate synchronously, before ever creating a progress job — otherwise
    // an invalid device fails inside the spawned task before the job is ever
    // registered, and the client's SSE connection sees "Job not found"
    // instead of a clean error (the service function below re-validates too,
    // as defense in depth, but this is what gives the frontend a real error).
    if let Err(e) = SecurityValidator::validate_block_device(&payload.source) {
        return ResponseHelper::json_error(format!("Invalid source device: {}", e));
    }
    if let Err(e) = SecurityValidator::validate_block_device(&payload.destination) {
        return ResponseHelper::json_error(format!("Invalid destination device: {}", e));
    }

    let source = payload.source.clone();
    let destination = payload.destination.clone();
    let mut options: DdrescueOptions = payload.into();
    if let Err(e) = options.validate() {
        return service_error("Invalid clone options", e);
    }
    if let Some(lf) = &options.logfile {
        if let Err(e) = SecurityValidator::validate_log_path(lf) {
            return ResponseHelper::json_error(format!("Invalid log file path: {}", e));
        }
    }

    // Register the job in PROGRESS_MANAGER (not just a bare UUID) — otherwise
    // every `ProgressEmitter::progress/complete/fail` below is a silent no-op
    // and the client's SSE stream gets "Job not found". Also surfaces the
    // clone in the desktop activity taskbar.
    let job_id = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("rescue_ddrescue".to_string()),
        "Starting disk clone...",
        Some(admin_username.clone()),
    );
    let jid = job_id.clone();

    let logfile = match options.logfile.clone() {
        Some(lf) => lf,
        // Without this the service keeps its own mapfile (under the rescue
        // directory) and the client would not know its path — so ask the same
        // default the service uses, as a string, before the job starts.
        None => RescueService::default_mapfile_display(&job_id),
    };
    // The service derives the same default when `logfile` is None; setting it
    // explicitly here keeps the response, the SSE message and the file on disk
    // in agreement (and made the per-job mapfile the real default: a single
    // shared /tmp/ddrescue.log meant a second clone read the first run's map
    // and silently copied nothing while reporting success).
    options.logfile = Some(logfile.clone());

    // Register the cancellation hook *before* the task starts so a cancel that
    // arrives immediately after this response is never a no-op: the hook asks
    // the running job registry to SIGTERM the tool's whole process group.
    let cancel_job_id = job_id.clone();
    ProgressEmitter::register_cancel_hook(
        &job_id,
        Box::new(move || {
            if !crate::rescue::jobs::cancel(&cancel_job_id) {
                tracing::warn!(
                    "Rescue: cancellation requested for clone job '{}', which had not started its process yet",
                    cancel_job_id
                );
            }
        }),
    );

    crate::events::log_event_warn(format!(
        "Admin '{}' started a ddrescue clone from '{}' to '{}'",
        admin_username, source, destination
    ));

    let clone_source = source.clone();
    let clone_dest = destination.clone();
    tokio::spawn(async move {
        match RescueService::ddrescue_clone(&source, &destination, &options, &jid).await {
            Ok(()) => {
                crate::events::log_event(format!(
                    "ddrescue clone from '{}' to '{}' completed",
                    clone_source, clone_dest
                ));
                PROGRESS_MANAGER.remove_job(&jid);
            }
            Err(RescueError::Cancelled) => {
                crate::events::log_event_warn(format!(
                    "ddrescue clone from '{}' to '{}' was cancelled by an admin",
                    clone_source, clone_dest
                ));
                // A cancelled clone keeps its mapfile and can be resumed — say
                // so instead of reporting a plain failure.
                ProgressEmitter::cancelled(
                    &jid,
                    "Clone cancelled — the mapfile is kept, so the clone can be resumed",
                );
                PROGRESS_MANAGER.remove_job(&jid);
            }
            Err(e) => {
                crate::events::log_event_warn(format!(
                    "ddrescue clone from '{}' to '{}' failed: {}",
                    clone_source, clone_dest, e
                ));
                ProgressEmitter::fail(&jid, &e.to_string());
                PROGRESS_MANAGER.remove_job(&jid);
            }
        }
    });

    let sse_url = format!("/api/progress/{}", job_id);
    ResponseHelper::json_success(serde_json::json!({
        "job_id": job_id,
        "sse_url": sse_url,
        "logfile": logfile,
        "message": "ddrescue clone started"
    }))
}

pub async fn handle_ddrescue_status(
    Path(logfile): Path<String>,
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    // The path comes straight from the URL: the validator confines it to the
    // rescue directories and refuses symlinks/non-files/oversized files, so
    // this endpoint can no longer be used to read an arbitrary system file.
    match RescueService::get_ddrescue_log(&logfile).await {
        Ok(status) => ResponseHelper::json_success(status),
        Err(RescueError::Security(msg)) => ResponseHelper::json_error(msg),
        Err(RescueError::JobNotFound(msg)) => {
            ResponseHelper::json_error(format!("No mapfile at '{}': {}", logfile, msg))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to get status: {}", e)),
    }
}

/// `GET /api/rescue/ddrescue/mapfile` — where the default mapfile lives, so
/// the UI can offer "resume/status" without guessing. Returns `{"path": ...}`
/// (the file may not exist yet).
pub async fn handle_ddrescue_mapfile_path(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(serde_json::json!({
        "directory": crate::config::paths::rescue_dir().to_string_lossy(),
    }))
}

pub async fn handle_testdisk_analyze(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
    Json(payload): Json<TestDiskAnalyzeRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if let Err(e) = payload.reject_unsupported() {
        return service_error("Invalid TestDisk options", e);
    }
    if let Err(e) = SecurityValidator::validate_block_device(&payload.device) {
        return ResponseHelper::json_error(format!("Invalid device: {}", e));
    }

    let device = payload.device.clone();
    match RescueService::testdisk_analyze(&device).await {
        Ok(result) => {
            crate::events::log_event(format!(
                "Admin '{}' ran a TestDisk analysis on '{}'",
                admin_username, device
            ));
            ResponseHelper::json_success(result)
        }
        Err(e) => service_error("TestDisk analysis failed", e),
    }
}

pub async fn handle_testdisk_rebuild(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
    Json(payload): Json<TestDiskRebuildRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if let Err(e) = payload.reject_unsupported() {
        return service_error("Invalid TestDisk options", e);
    }
    if let Err(e) = SecurityValidator::validate_block_device(&payload.device) {
        return ResponseHelper::json_error(format!("Invalid device: {}", e));
    }

    let device = payload.device.clone();
    match RescueService::testdisk_rebuild(&device).await {
        Ok(log) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' ran a TestDisk partition scan on '{}'",
                admin_username, device
            ));
            ResponseHelper::json_success(serde_json::json!({
                "message": "TestDisk scan completed",
                "log": log
            }))
        }
        Err(e) => service_error("TestDisk scan failed", e),
    }
}

pub async fn handle_photorec_recover(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
    Json(payload): Json<PhotoRecRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    // Same reasoning as handle_ddrescue_clone above: validate before ever
    // creating a progress job, so an invalid device/path returns a clean
    // error instead of a phantom job the client's SSE connection can't find.
    if let Err(e) = SecurityValidator::validate_block_device(&payload.device) {
        return ResponseHelper::json_error(format!("Invalid device: {}", e));
    }
    if let Err(e) = SecurityValidator::validate_output_path(&payload.output_dir) {
        return ResponseHelper::json_error(format!("Invalid output directory: {}", e));
    }
    let options = PhotoRecOptions {
        file_types: payload.file_types.clone(),
        partition_type: payload.partition_type.clone(),
    };
    if let Err(e) = options.validate() {
        return service_error("Invalid recovery options", e);
    }

    // See handle_ddrescue_clone: register so ProgressEmitter calls take effect
    // and the recovery shows in the desktop activity taskbar.
    let job_id = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("rescue_photorec".to_string()),
        "Starting file recovery...",
        Some(admin_username.clone()),
    );
    let jid = job_id.clone();
    let device = payload.device.clone();
    let out_dir = payload.output_dir.clone();
    let out_dir_display = out_dir.clone();

    let cancel_job_id = job_id.clone();
    ProgressEmitter::register_cancel_hook(
        &job_id,
        Box::new(move || {
            if !crate::rescue::jobs::cancel(&cancel_job_id) {
                tracing::warn!(
                    "Rescue: cancellation requested for recovery job '{}', which had not started its process yet",
                    cancel_job_id
                );
            }
        }),
    );

    crate::events::log_event_warn(format!(
        "Admin '{}' started a PhotoRec recovery of '{}' into '{}'",
        admin_username, device, out_dir_display
    ));

    let clone_device = device.clone();
    let clone_out_dir_display = out_dir_display.clone();
    let owner = admin_username.clone();
    tokio::spawn(async move {
        match RescueService::photorec_recover(&device, &out_dir, &options, &jid, Some(&owner)).await
        {
            Ok(_) => {
                crate::events::log_event(format!(
                    "PhotoRec recovery of '{}' into '{}' completed",
                    clone_device, clone_out_dir_display
                ));
                PROGRESS_MANAGER.remove_job(&jid);
            }
            Err(RescueError::Cancelled) => {
                crate::events::log_event_warn(format!(
                    "PhotoRec recovery of '{}' was cancelled by an admin",
                    clone_device
                ));
                ProgressEmitter::cancelled(
                    &jid,
                    "Recovery cancelled — the files already carved are kept",
                );
                PROGRESS_MANAGER.remove_job(&jid);
            }
            Err(e) => {
                crate::events::log_event_warn(format!(
                    "PhotoRec recovery of '{}' failed: {}",
                    clone_device, e
                ));
                ProgressEmitter::fail(&jid, &e.to_string());
                PROGRESS_MANAGER.remove_job(&jid);
            }
        }
    });

    let sse_url = format!("/api/progress/{}", job_id);
    ResponseHelper::json_success(serde_json::json!({
        "job_id": job_id,
        "sse_url": sse_url,
        "output_dir": out_dir_display,
        "message": "PhotoRec recovery started"
    }))
}

pub async fn handle_mdadm_diagnose(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
    Json(payload): Json<MdadmRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    match RescueService::mdadm_diagnose(&payload.name).await {
        Ok(diag) => ResponseHelper::json_success(diag),
        Err(e) => service_error("mdadm diagnosis failed", e),
    }
}

pub async fn handle_mdadm_repair(
    State(state): State<Arc<crate::handlers::AppState>>,
    headers: HeaderMap,
    Json(payload): Json<MdadmRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let name = payload.name.clone();
    let options: MdadmOptions = payload.into();
    if let Err(e) = options.validated_action() {
        return service_error("Invalid mdadm options", e);
    }

    match RescueService::mdadm_repair(&name, &options).await {
        Ok(message) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' ran an mdadm repair on array '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(serde_json::json!({ "message": message }))
        }
        Err(e) => service_error("mdadm repair failed", e),
    }
}

pub fn create_rescue_router(state: AppStateArc) -> Router {
    Router::new()
        .route("/tools", axum::routing::get(handle_rescue_tools_available))
        .route("/devices", axum::routing::get(handle_rescue_devices))
        .route(
            "/ddrescue/clone",
            axum::routing::post(handle_ddrescue_clone),
        )
        .route(
            "/ddrescue/status/{logfile}",
            axum::routing::get(handle_ddrescue_status),
        )
        .route(
            "/ddrescue/mapfile",
            axum::routing::get(handle_ddrescue_mapfile_path),
        )
        .route(
            "/testdisk/analyze",
            axum::routing::post(handle_testdisk_analyze),
        )
        .route(
            "/testdisk/rebuild",
            axum::routing::post(handle_testdisk_rebuild),
        )
        .route(
            "/photorec/recover",
            axum::routing::post(handle_photorec_recover),
        )
        .route(
            "/mdadm/diagnose",
            axum::routing::post(handle_mdadm_diagnose),
        )
        .route("/mdadm/repair", axum::routing::post(handle_mdadm_repair))
        .with_state(state)
}
