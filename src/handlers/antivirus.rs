/* =============================================================================
 * File          : antivirus.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Antivirus (ClamAV) endpoints: service status and settings, manual scan
 *   start (runs in the background, live progress over SSE) and cancel,
 *   scan history, quarantined threats and their resolution. Reads are
 *   session-auth; settings updates, scan triggering/cancelling and threat
 *   resolution are admin-only.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/antivirus in src/web/mod.rs; scans execute via
 *   crate::antivirus, results persist in the scan/threat repositories.
 * =============================================================================
 */

//! HTTP handlers for antivirus (ClamAV) scanning: status, settings, manual
//! scans with live SSE progress, scan history, and quarantined threats.

use crate::antivirus::{self, av_scan_is_running};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::AntivirusRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

/// `GET /api/antivirus/status`
pub async fn handle_antivirus_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let available = antivirus::clamscan_available();
    let definitions = antivirus::definitions_status();
    ResponseHelper::json_success(serde_json::json!({
        "clamscan_available": available,
        "definitions": definitions,
        "scan_running": av_scan_is_running(),
    }))
}

/// `GET /api/antivirus/settings`
pub async fn handle_antivirus_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = AntivirusRepository::new(state.db.pool());
    match repo.get_settings().await {
        Ok(settings) => ResponseHelper::json_success(settings),
        Err(e) => ResponseHelper::json_error(format!("Failed to load antivirus settings: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct AvSettingsUpdate {
    pub quarantine_dir: String,
    pub scheduled_scan_enabled: bool,
    pub scheduled_scan_cron: String,
    pub scheduled_scan_path: Option<String>,
    pub scan_archives: bool,
}

/// `PUT /api/antivirus/settings`
pub async fn handle_antivirus_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AvSettingsUpdate>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if let Some(path) = &body.scheduled_scan_path {
        if state.path_guard.validate(path).is_none() {
            return ResponseHelper::json_error(
                "Scheduled scan path is not within an allowed root".to_string(),
            );
        }
    }

    let repo = AntivirusRepository::new(state.db.pool());
    match repo
        .save_settings(
            &body.quarantine_dir,
            body.scheduled_scan_enabled,
            &body.scheduled_scan_cron,
            body.scheduled_scan_path.as_deref(),
            body.scan_archives,
        )
        .await
    {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "ok": true })),
        Err(e) => ResponseHelper::json_error(format!("Failed to save antivirus settings: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ScanRequest {
    pub path: String,
}

/// `POST /api/antivirus/scan` — starts a manual scan and returns a job id
/// for live SSE progress (`/api/progress/{job_id}`, the shared progress
/// stream endpoint used by every other tracked job).
pub async fn handle_antivirus_scan_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ScanRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // Validate the target path first: rejecting bad input must never depend
    // on whether the scanning tool is installed, and costs no side effects.
    let Some(resolved) = state.path_guard.validate(&body.path) else {
        return ResponseHelper::json_error("Scan path is not within an allowed root".to_string());
    };
    if !resolved.exists() {
        return ResponseHelper::json_error("Scan path does not exist".to_string());
    }
    if !antivirus::clamscan_available() {
        return ResponseHelper::json_error("ClamAV (clamscan) is not installed".to_string());
    }
    if av_scan_is_running() {
        return ResponseHelper::json_error("An antivirus scan is already running".to_string());
    }

    let job_id = antivirus::create_scan_job();
    let pool = state.db.pool();
    let scan_path = resolved.to_string_lossy().to_string();
    let job_id_clone = job_id.clone();
    crate::events::log_event(format!(
        "Admin '{}' started an antivirus scan of '{}'",
        admin_username, scan_path
    ));
    tokio::spawn(async move {
        match antivirus::run_scan(pool, scan_path.clone(), "manual", Some(job_id_clone)).await {
            Ok((_scan_id, summary)) => {
                crate::events::log_event(format!(
                    "Antivirus scan of '{}' completed ({} threats found)",
                    scan_path, summary.threats_found
                ));
            }
            Err(e) => {
                crate::events::log_event_warn(format!(
                    "Antivirus scan of '{}' failed: {}",
                    scan_path, e
                ));
            }
        }
    });

    ResponseHelper::json_success(serde_json::json!({ "job_id": job_id }))
}

/// `POST /api/antivirus/scan/cancel`
pub async fn handle_antivirus_scan_cancel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let cancelled = antivirus::cancel_scan().await;
    ResponseHelper::json_success(serde_json::json!({ "cancelled": cancelled }))
}

/// `GET /api/antivirus/scans`
pub async fn handle_antivirus_scans_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = AntivirusRepository::new(state.db.pool());
    match repo.list_scans(50).await {
        Ok(scans) => ResponseHelper::json_success(scans),
        Err(e) => ResponseHelper::json_error(format!("Failed to list scans: {}", e)),
    }
}

/// `GET /api/antivirus/threats`
pub async fn handle_antivirus_threats_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = AntivirusRepository::new(state.db.pool());
    match repo.list_threats(false).await {
        Ok(threats) => ResponseHelper::json_success(threats),
        Err(e) => ResponseHelper::json_error(format!("Failed to list threats: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ResolveThreatRequest {
    #[serde(default)]
    pub delete: bool,
}

/// `POST /api/antivirus/threats/{id}/resolve`
pub async fn handle_antivirus_threat_resolve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(threat_id): Path<String>,
    Json(body): Json<ResolveThreatRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match antivirus::resolve_threat(state.db.pool(), &threat_id, body.delete).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' resolved antivirus threat (id: {}){}",
                admin_username,
                threat_id,
                if body.delete {
                    " and deleted the file"
                } else {
                    ""
                }
            ));
            ResponseHelper::json_success(serde_json::json!({ "ok": true }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to resolve threat: {}", e)),
    }
}
