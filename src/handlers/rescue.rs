use crate::sse::{JobType, PROGRESS_MANAGER};
use axum::{extract::Path, extract::State, http::HeaderMap, response::Response, Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppStateArc;
use crate::rescue::{
    DdrescueOptions, MdadmOptions, PhotoRecOptions, RescueService, SecurityValidator,
    TestDiskOptions,
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

#[derive(Debug, Deserialize)]
pub struct TestDiskAnalyzeRequest {
    pub device: String,
    pub deep_search: Option<bool>,
    pub log_path: Option<String>,
}

impl From<TestDiskAnalyzeRequest> for TestDiskOptions {
    fn from(r: TestDiskAnalyzeRequest) -> Self {
        TestDiskOptions {
            deep_search: r.deep_search,
            log_path: r.log_path,
            force: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TestDiskRebuildRequest {
    pub device: String,
    pub deep_search: Option<bool>,
    pub force: Option<bool>,
}

impl From<TestDiskRebuildRequest> for TestDiskOptions {
    fn from(r: TestDiskRebuildRequest) -> Self {
        TestDiskOptions {
            deep_search: r.deep_search,
            log_path: None,
            force: r.force,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PhotoRecRequest {
    pub device: String,
    pub output_dir: String,
    pub file_types: Option<Vec<String>>,
    pub keep_broken: Option<bool>,
    pub max_size: Option<u64>,
    pub partition_type: Option<String>,
}

impl From<PhotoRecRequest> for PhotoRecOptions {
    fn from(r: PhotoRecRequest) -> Self {
        PhotoRecOptions {
            file_types: r.file_types,
            keep_broken: r.keep_broken,
            max_size: r.max_size,
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
        Err(e) => ResponseHelper::json_error(format!("Failed to list devices: {}", e)),
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
    if let Some(lf) = &payload.logfile {
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
    let source = payload.source.clone();
    let dest = payload.destination.clone();
    let mut options: DdrescueOptions = payload.into();
    let logfile = options
        .logfile
        .clone()
        .unwrap_or_else(|| format!("/tmp/ddrescue_{}.log", job_id));
    // Without this, the service function falls back to its own generic
    // "/tmp/ddrescue.log" default (never the per-job path computed above),
    // so every run with no explicit logfile shares one mapfile — confirmed
    // live: a second clone reads the first run's "already rescued" map and
    // silently copies nothing at all, while still reporting success.
    options.logfile = Some(logfile.clone());

    crate::events::log_event_warn(format!(
        "Admin '{}' started a ddrescue clone from '{}' to '{}'",
        admin_username, source, dest
    ));

    let clone_source = source.clone();
    let clone_dest = dest.clone();
    tokio::spawn(async move {
        match RescueService::ddrescue_clone(&source, &dest, &options, &jid).await {
            Ok(()) => {
                crate::events::log_event(format!(
                    "ddrescue clone from '{}' to '{}' completed",
                    clone_source, clone_dest
                ));
            }
            Err(e) => {
                crate::events::log_event_warn(format!(
                    "ddrescue clone from '{}' to '{}' failed: {}",
                    clone_source, clone_dest, e
                ));
                ProgressEmitter::fail(&jid, &e.to_string());
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

    match RescueService::get_ddrescue_log(&logfile).await {
        Ok(status) => ResponseHelper::json_success(status),
        Err(e) => ResponseHelper::json_error(format!("Failed to get status: {}", e)),
    }
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

    let device = payload.device.clone();
    let options: TestDiskOptions = payload.into();
    match RescueService::testdisk_analyze(&device, &options).await {
        Ok(result) => {
            crate::events::log_event(format!(
                "Admin '{}' ran a TestDisk analysis on '{}'",
                admin_username, device
            ));
            ResponseHelper::json_success(result)
        }
        Err(e) => ResponseHelper::json_error(format!("TestDisk analysis failed: {}", e)),
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

    let device = payload.device.clone();
    let options: TestDiskOptions = payload.into();
    match RescueService::testdisk_rebuild(&device, &options).await {
        Ok(log) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' ran a TestDisk partition rebuild on '{}'",
                admin_username, device
            ));
            ResponseHelper::json_success(serde_json::json!({
                "message": "TestDisk rebuild completed",
                "log": log
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("TestDisk rebuild failed: {}", e)),
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
    let options: PhotoRecOptions = payload.into();

    crate::events::log_event_warn(format!(
        "Admin '{}' started a PhotoRec recovery of '{}' into '{}'",
        admin_username, device, out_dir_display
    ));

    let clone_device = device.clone();
    let clone_out_dir_display = out_dir_display.clone();
    tokio::spawn(async move {
        match RescueService::photorec_recover(&device, &out_dir, &options, &jid).await {
            Ok(_) => {
                crate::events::log_event(format!(
                    "PhotoRec recovery of '{}' into '{}' completed",
                    clone_device, clone_out_dir_display
                ));
            }
            Err(e) => {
                crate::events::log_event_warn(format!(
                    "PhotoRec recovery of '{}' failed: {}",
                    clone_device, e
                ));
                ProgressEmitter::fail(&jid, &e.to_string());
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
        Err(e) => ResponseHelper::json_error(format!("mdadm diagnosis failed: {}", e)),
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
    match RescueService::mdadm_repair(&name, &options).await {
        Ok(message) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' ran an mdadm repair on array '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success(serde_json::json!({ "message": message }))
        }
        Err(e) => ResponseHelper::json_error(format!("mdadm repair failed: {}", e)),
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
