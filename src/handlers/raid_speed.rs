/* =============================================================================
 * File          : raid_speed.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Read/write the RAID resync-speed profile (FIAB-10).
 *
 *   `GET  /api/storage/raid/resync-speed` — saved profile + limits + whether
 *   the host currently exposes `/proc/sys/dev/raid`.
 *   `PUT  /api/storage/raid/resync-speed` — validate, persist, apply.
 *
 *   Persisting matters: the sysctls are volatile, and an admin may set the
 *   profile before creating the array. The monitor loop re-applies it at boot;
 *   array creation/rebuild calls the same helper afterwards.
 *
 * Dependencies:
 *   - axum, crate::raid_stability
 *
 * Usage:
 *   Routes in src/web/mod.rs; selector in Storage -> RAID.
 * =============================================================================
 */

//! RAID resync-speed endpoint (admin).

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::AppState;
use crate::raid_stability::{self, SpeedProfile};
use crate::utils::ResponseHelper;

#[derive(Debug, Deserialize)]
pub struct RaidSpeedRequest {
    /// `low` | `balanced` | `fast`.
    pub profile: String,
}

fn payload(profile: SpeedProfile, applied: bool, warning: Option<String>) -> serde_json::Value {
    let (min, max) = profile.limits();
    json!({
        "profile": profile.as_str(),
        "min_kbps": min,
        "max_kbps": max,
        "applied": applied,
        "kernel_available": std::path::Path::new("/proc/sys/dev/raid/speed_limit_min").is_file(),
        "warning": warning,
    })
}

/// `GET /api/storage/raid/resync-speed`
pub async fn handle_raid_speed_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let profile = raid_stability::load_speed_profile();
    ResponseHelper::json_success(payload(profile, true, None))
}

/// `PUT /api/storage/raid/resync-speed`
pub async fn handle_raid_speed_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(body): axum::extract::Json<RaidSpeedRequest>,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let Some(profile) = SpeedProfile::parse(&body.profile) else {
        return ResponseHelper::json_error(
            "profile must be 'low', 'balanced' or 'fast'".to_string(),
        );
    };
    if let Err(e) = raid_stability::save_speed_profile(profile) {
        return ResponseHelper::json_error(e);
    }
    let (applied, warning) = match raid_stability::apply_speed_profile(profile).await {
        Ok(()) => (true, None),
        Err(e) => {
            // Persisted on purpose: the array may not exist yet, or the md
            // module may load later. The answer says the write did not land.
            (false, Some(e))
        }
    };
    crate::events::log_event(format!(
        "Admin '{admin}' set the RAID resync speed to '{}' (applied: {applied})",
        profile.as_str()
    ));
    ResponseHelper::json_success(payload(profile, applied, warning))
}
