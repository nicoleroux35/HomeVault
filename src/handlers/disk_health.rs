/* =============================================================================
 * File          : disk_health.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface for the disk-health history: active findings, per-disk
 *   samples, and an admin-only "sample now" action.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::disk_health, crate::storage::disk_health_repo
 *
 * Usage:
 *   Routes under /api/health/disk-health in src/web/mod.rs; see
 *   docs/guides/health.md.
 * =============================================================================
 */

//! Disk-health endpoints.

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::admin_guard::{require_admin, require_auth};
use super::AppState;
use crate::storage::disk_health_repo::DiskHealthRepository;
use crate::utils::ResponseHelper;

#[derive(Debug, Deserialize)]
pub struct SamplesQuery {
    pub serial: String,
    pub limit: Option<u32>,
}

/// GET /api/health/disk-health — active findings (auth).
pub async fn handle_disk_findings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    match crate::disk_health::active_findings(&state).await {
        Ok(findings) => ResponseHelper::json_success(findings),
        Err(e) => ResponseHelper::json_internal_error(e),
    }
}

/// GET /api/health/disk-health/samples?serial=…&limit=… — history (auth).
pub async fn handle_disk_samples(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SamplesQuery>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let serial = query.serial.trim();
    if serial.is_empty() || serial.len() > 128 {
        return ResponseHelper::json_error("serial must be 1-128 characters".to_string());
    }
    let repo = DiskHealthRepository::new(state.db.pool());
    match repo.list_samples(serial, query.limit.unwrap_or(500)).await {
        Ok(samples) => ResponseHelper::json_success(samples),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// One disk's trusted trends, for the Health screen.
#[derive(Debug, Serialize)]
pub struct DiskPrediction {
    pub serial: String,
    pub trends: Vec<crate::disk_prediction::Trend>,
}

/// GET /api/health/disk-health/predictions — every sampled disk (auth).
///
/// A disk with fewer than `MIN_POINTS` samples simply has an empty `trends`
/// list: the endpoint never invents a trend from noise.
pub async fn handle_disk_predictions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let repo = DiskHealthRepository::new(state.db.pool());
    let serials = match repo.list_serials().await {
        Ok(serials) => serials,
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };
    let mut out = Vec::new();
    for serial in serials {
        match crate::disk_prediction::predictions_for(&repo, &serial).await {
            Ok(trends) => out.push(DiskPrediction { serial, trends }),
            Err(e) => return ResponseHelper::json_internal_error(e),
        }
    }
    ResponseHelper::json_success(out)
}

/// GET /api/health/disk-health/prediction?serial=… — regression trend (auth).
pub async fn handle_disk_prediction(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SamplesQuery>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let serial = query.serial.trim();
    if serial.is_empty() || serial.len() > 128 {
        return ResponseHelper::json_error("serial must be 1-128 characters".to_string());
    }
    let repo = DiskHealthRepository::new(state.db.pool());
    match crate::disk_prediction::predictions_for(&repo, serial).await {
        Ok(trends) => ResponseHelper::json_success(trends),
        Err(e) => ResponseHelper::json_internal_error(e),
    }
}

/// POST /api/health/disk-health/sample — sample now (admin).
pub async fn handle_disk_sample(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    match crate::disk_health::sample_once(&state).await {
        Ok(report) => {
            crate::events::log_event(format!(
                "Admin '{}' ran a disk-health sample ({} disks, {} findings)",
                admin, report.sampled_disks, report.findings
            ));
            ResponseHelper::json_success(report)
        }
        Err(e) => ResponseHelper::json_internal_error(e),
    }
}
