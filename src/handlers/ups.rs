use crate::handlers::AppState;
use crate::ups::{UpsConfigEntry, UpsService};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

/// Query parameters for UPS requests.
#[derive(Debug, Deserialize)]
pub struct UpsQueryParams {
    pub name: Option<String>,
}

/// Request body for adding/updating a UPS section in `ups.conf`.
#[derive(Debug, Deserialize)]
pub struct UpsConfigRequest {
    pub name: String,
    pub driver: String,
    pub port: String,
    #[serde(default)]
    pub desc: String,
}

/// Lists all connected UPS devices and their status.
pub async fn handle_ups_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(_params): Query<UpsQueryParams>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match UpsService::list_all_status() {
        Ok(list) => ResponseHelper::json_success(list),
        Err(e) => ResponseHelper::json_error(format!("UPS error: {}", e)),
    }
}

/// Returns detailed status for a specific UPS device.
pub async fn handle_ups_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match UpsService::get_ups_status(&name) {
        Ok(status) => ResponseHelper::json_success(status),
        Err(e) => ResponseHelper::json_error(format!("UPS error: {}", e)),
    }
}

/// Returns all NUT variables for a specific UPS device.
pub async fn handle_ups_variables(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match UpsService::get_ups_variables(&name) {
        Ok(vars) => ResponseHelper::json_success(vars),
        Err(e) => ResponseHelper::json_error(format!("UPS error: {}", e)),
    }
}

/// Returns whether NUT UPS monitoring is available on this system.
pub async fn handle_ups_availability(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    ResponseHelper::json_success(serde_json::json!({
        "available": UpsService::is_available()
    }))
}

/// Lists the UPS devices configured in `/etc/nut/ups.conf`.
pub async fn handle_ups_config_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match crate::ups::list_config() {
        Ok(entries) => ResponseHelper::json_success(entries),
        Err(e) => ResponseHelper::json_error(format!("UPS config error: {}", e)),
    }
}

/// Adds or updates one UPS section (serial / network / USB) in `ups.conf`,
/// then reloads NUT.
pub async fn handle_ups_config_save(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<UpsConfigRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let entry = UpsConfigEntry {
        name: req.name,
        driver: req.driver,
        port: req.port,
        desc: req.desc,
    };

    match crate::ups::save_config(entry) {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({
            "success": true,
            "message": "UPS configuration saved"
        })),
        Err(e) => ResponseHelper::json_error(format!("UPS config error: {}", e)),
    }
}

/// Removes one UPS section from `ups.conf`, then reloads NUT.
pub async fn handle_ups_config_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match crate::ups::delete_config(&name) {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({
            "success": true,
            "message": "UPS configuration removed"
        })),
        Err(e) => ResponseHelper::json_error(format!("UPS config error: {}", e)),
    }
}

async fn require_admin(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Some(ResponseHelper::json_error(
            "No session token provided".to_string(),
        ));
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => {
            if !session.is_admin {
                return Some(ResponseHelper::json_forbidden(
                    "Admin access required".to_string(),
                ));
            }
            None
        }
        Err(_) => Some(ResponseHelper::json_unauthorized(
            "Invalid session".to_string(),
        )),
    }
}
