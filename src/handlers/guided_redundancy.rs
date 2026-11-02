use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::guided_redundancy::{
    DiskInfo, GuidedRedundancyService, LayoutRecommendation, RedundancyLevel,
};

/// GET /api/storage/guided-redundancy
pub async fn handle_recommend(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<RecommendParams>,
) -> Result<Json<RecommendationsResponse>, (StatusCode, String)> {
    // For now, return empty recommendations
    // TODO: Integrate with disk detection when disk module is available
    let disk_infos: Vec<DiskInfo> = Vec::new();

    // Generate recommendations
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

    Ok(Json(RecommendationsResponse {
        recommendations,
        total_disks: disk_infos.len(),
        available_disks: disk_infos.iter().filter(|d| !d.in_use).count(),
    }))
}

/// POST /api/storage/guided-redundancy/apply/:id
pub async fn handle_apply(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<String>,
    Json(_req): Json<ApplyRequest>,
) -> Result<Json<ApplyResponse>, (StatusCode, String)> {
    // This is a placeholder - actual implementation would:
    // 1. Load the recommendation by ID
    // 2. Validate the layout can still be created
    // 3. Delegate to disk.rs/pools.rs to create the array
    // 4. Return the created pool/array identifier

    // For now, return "not implemented" with guidance
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "Layout application requires integration with existing pool/array creation. \
         Use the recommendation details to create the layout manually via Storage screen."
            .to_string(),
    ))
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
