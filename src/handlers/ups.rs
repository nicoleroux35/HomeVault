/* =============================================================================
 * File          : ups.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPS monitoring endpoints: enumerate configured UPS devices, live status
 *   and variables (battery charge, load, runtime), availability probes,
 *   and the admin-managed device configuration list (which hosts and
 *   communities to poll). Queries run through UpsService; every endpoint
 *   is admin-only.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/ups in src/web/mod.rs; device configuration persists via
 *   UpsConfigEntry / the UPS storage.
 * =============================================================================
 */

//! UPS monitoring endpoints (device list, live status, configuration) —
//! all admin-only.

use crate::handlers::AppState;
use crate::ups::{UpsConfigEntry, UpsError, UpsService};
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

/// Every `upsc`/`systemctl` call is blocking, so it runs on the blocking
/// pool (ROADMAP X.1) — the handlers below never touch a child process on
/// the runtime thread.
async fn blocking<T, F>(f: F) -> Result<T, Response>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(value) => Ok(value),
        Err(e) => Err(ResponseHelper::json_internal_error(format!(
            "UPS task failed: {e}"
        ))),
    }
}

/// A validation refusal is a `400` naming the field; a command failure keeps
/// the existing message shape.
fn ups_error(e: UpsError) -> Response {
    ResponseHelper::json_error(format!("UPS error: {}", e))
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

    match blocking(UpsService::list_all_status).await {
        Ok(Ok(list)) => ResponseHelper::json_success(list),
        Ok(Err(e)) => ups_error(e),
        Err(resp) => resp,
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

    match blocking(move || UpsService::get_ups_status(&name)).await {
        Ok(Ok(status)) => ResponseHelper::json_success(status),
        Ok(Err(e)) => ups_error(e),
        Err(resp) => resp,
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

    match blocking(move || UpsService::get_ups_variables(&name)).await {
        Ok(Ok(vars)) => ResponseHelper::json_success(vars),
        Ok(Err(e)) => ups_error(e),
        Err(resp) => resp,
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

    match blocking(UpsService::is_available).await {
        Ok(available) => ResponseHelper::json_success(serde_json::json!({
            "available": available
        })),
        Err(resp) => resp,
    }
}

/// Lists the UPS devices configured in `/etc/nut/ups.conf`.
pub async fn handle_ups_config_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    match blocking(crate::ups::list_config).await {
        Ok(Ok(entries)) => ResponseHelper::json_success(entries),
        Ok(Err(e)) => ResponseHelper::json_error(format!("UPS config error: {}", e)),
        Err(resp) => resp,
    }
}

/// Adds or updates one UPS section (serial / network / USB) in `ups.conf`,
/// then reloads NUT.
///
/// ROADMAP 13.2: the response distinguishes "written" from "reloaded". The
/// file is written first (so a reload refusal never loses the admin's edit),
/// and every unit that refused to restart is named in `reload_warnings` —
/// the old handler answered a flat success even when no grant existed and
/// NUT never saw the new section.
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

    match blocking(move || crate::ups::save_config(entry)).await {
        Ok(Ok(reload_warnings)) => ResponseHelper::json_success(serde_json::json!({
            "success": true,
            "message": "UPS configuration saved",
            "reload_warnings": reload_warnings,
        })),
        Ok(Err(e)) => ResponseHelper::json_error(format!("UPS config error: {}", e)),
        Err(resp) => resp,
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

    match blocking(move || crate::ups::delete_config(&name)).await {
        Ok(Ok(reload_warnings)) => ResponseHelper::json_success(serde_json::json!({
            "success": true,
            "message": "UPS configuration removed",
            "reload_warnings": reload_warnings,
        })),
        Ok(Err(e)) => ResponseHelper::json_error(format!("UPS config error: {}", e)),
        Err(resp) => resp,
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
