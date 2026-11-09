/* =============================================================================
 * File          : docker_update_schedule.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026
 * Last modified : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin endpoints for the opt-in scheduled Docker/catalog updates (P1):
 *   read the schedule, change it, and run one pass immediately. The scheduler
 *   itself lives in src/docker_update_scheduler.rs.
 *
 * Dependencies:
 *   - axum (>= 0.8), serde
 *
 * Usage:
 *   Routed under /api/docker/updates in src/web/mod.rs.
 * =============================================================================
 */

//! Admin endpoints for scheduled Docker/catalog updates.

use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The schedule plus the live Docker capability and the last outcome.
#[derive(Debug, Clone, Serialize)]
pub struct ScheduleView {
    pub enabled: bool,
    pub interval_hours: u32,
    pub last_run_at: Option<String>,
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    pub docker_available: bool,
}

fn view() -> ScheduleView {
    let settings = crate::docker_update_scheduler::load_settings();
    ScheduleView {
        enabled: settings.enabled,
        interval_hours: settings.interval_hours,
        last_run_at: settings.last_run_at,
        last_result: settings.last_result,
        last_error: settings.last_error,
        docker_available: crate::docker::DockerService::is_available(),
    }
}

/// `GET /api/docker/updates/schedule`
pub async fn handle_get_schedule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return response;
    }
    ResponseHelper::json_success(view())
}

/// Body for `PUT /api/docker/updates/schedule`.
#[derive(Debug, Deserialize)]
pub struct ScheduleRequest {
    pub enabled: bool,
    pub interval_hours: u32,
}

/// `PUT /api/docker/updates/schedule`
pub async fn handle_put_schedule(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<ScheduleRequest>,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let interval = match crate::docker_update_scheduler::validate_interval(req.interval_hours) {
        Ok(hours) => hours,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let mut settings = crate::docker_update_scheduler::load_settings();
    settings.enabled = req.enabled;
    settings.interval_hours = interval;
    if let Err(e) = crate::docker_update_scheduler::save_settings(&settings) {
        return ResponseHelper::json_error(format!("Could not save the schedule: {e}"));
    }
    crate::events::log_event(format!(
        "Admin '{}' {} scheduled Docker updates (every {} h)",
        admin,
        if req.enabled { "enabled" } else { "disabled" },
        interval
    ));
    ResponseHelper::json_success(view())
}

/// `POST /api/docker/updates/run-now` — one pass, regardless of the schedule.
pub async fn handle_run_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    match crate::docker_update_scheduler::run_now(&state, &admin).await {
        Ok(report) => ResponseHelper::json_success(serde_json::json!({
            "report": report,
            "schedule": view(),
        })),
        Err(e) => ResponseHelper::json_error(e),
    }
}
