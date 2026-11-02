/* =============================================================================
 * File          : guided_redundancy.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Guided redundancy endpoints: recommend a RAID layout for the disks the
 *   host actually has, and report what applying a recommendation would do.
 *
 *   Reads need a session; the apply path is admin-only even though it does not
 *   create anything yet — it is the route a destructive implementation will
 *   hang off, so it must already be behind the admin boundary rather than
 *   gaining one later.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::guided_redundancy
 *
 * Usage:
 *   Routes under /api/storage/guided-redundancy in src/web/mod.rs; see
 *   docs/guides/guided-redundancy.md.
 * =============================================================================
 */

//! Guided redundancy endpoints, see docs/guides/guided-redundancy.md.
//!
//! * `GET  /api/storage/guided-redundancy`            — recommendations
//! * `POST /api/storage/guided-redundancy/apply/{id}` — not implemented (501)

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::guided_redundancy::{
    DiskInfo, GuidedRedundancyService, LayoutRecommendation, RedundancyLevel,
};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::utils::ResponseHelper;

/// GET /api/storage/guided-redundancy — one recommendation per viable layout.
///
/// The disk inventory is empty for now: the wizard is a pure recommendation
/// engine and `docs/guides/guided-redundancy.md` says so. Wiring it to
/// `disk.rs` is a separate change with its own risk review, because the same
/// route would then start creating arrays.
pub async fn handle_recommend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<RecommendParams>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let disk_infos: Vec<DiskInfo> = Vec::new();

    let min_redundancy = params
        .min_redundancy
        .and_then(|s| match s.as_str() {
            "none" => Some(RedundancyLevel::None),
            "single" => Some(RedundancyLevel::Single),
            "double" => Some(RedundancyLevel::Double),
            _ => None,
        })
        .unwrap_or(RedundancyLevel::Single);

    let recommendations = GuidedRedundancyService::recommend_layouts(&disk_infos, min_redundancy);

    ResponseHelper::json_success(RecommendationsResponse {
        recommendations,
        total_disks: disk_infos.len(),
        available_disks: disk_infos.iter().filter(|d| !d.in_use).count(),
    })
}

/// POST /api/storage/guided-redundancy/apply/{id} — deliberately `501`.
///
/// The route exists so the screen can ask "can this be applied?" and get an
/// honest answer instead of a 404 it would have to guess about. It is
/// admin-only from the start: when the creation path lands, the privilege
/// boundary is already the right one.
pub async fn handle_apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    // The body is parsed only after the guard, so an unauthenticated caller
    // learns nothing about the payload shape and is answered `401`.
    if let Err(e) = serde_json::from_slice::<ApplyRequest>(&body) {
        return ResponseHelper::json_error(format!("Malformed request body: {}", e));
    }

    ResponseHelper::json_error_with_status(
        StatusCode::NOT_IMPLEMENTED,
        "Creating an array from a recommendation is not implemented. \
         Use the recommendation details to create the layout through the Storage screen."
            .to_string(),
    )
}

// Request/Response types

#[derive(Debug, Deserialize)]
pub struct RecommendParams {
    pub min_redundancy: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RecommendationsResponse {
    pub recommendations: Vec<LayoutRecommendation>,
    pub total_disks: usize,
    pub available_disks: usize,
}

#[derive(Debug, Deserialize)]
pub struct ApplyRequest {
    pub confirmed: bool,
}

#[derive(Debug, Serialize)]
pub struct ApplyResponse {
    pub pool_id: String,
    pub status: String,
}
