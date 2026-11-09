/* =============================================================================
 * File          : update_apply.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   "Apply update" endpoints (P2-8, UI half): see whether the update engine
 *   can run, queue a signed release manifest, start the fixed systemd unit,
 *   and follow the outcome across the restart it causes.
 *
 *   The handler never performs the swap: it writes a request and asks systemd
 *   to start `homevault-update.service`. The installer still verifies the
 *   manifest signature before touching a byte, and a failed start is rolled
 *   back automatically.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::update_engine, crate::handlers::AppState
 *
 * Usage:
 *   Routes /api/settings/update in src/web/mod.rs; see docs/guides/settings.md.
 * =============================================================================
 */

//! Signed-update application endpoints, see docs/guides/settings.md.
//!
//! * `GET  /api/settings/update/status` — can we apply, is it running, what
//!   happened last time (log tail included)
//! * `POST /api/settings/update/apply`  — queue a manifest and start the unit

use std::sync::Arc;

use axum::{body::Bytes, extract::State, http::HeaderMap, response::Response};
use serde::Deserialize;

use super::AppState;
use crate::handlers::admin_guard::require_admin;
use crate::update_engine::{self, UpdateRequest};
use crate::utils::ResponseHelper;

/// The status the Settings screen shows next to the button.
pub async fn handle_update_apply_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let (available, running, request, log_tail, exit, configured) =
        tokio::task::spawn_blocking(|| {
            let configured = crate::handlers::settings::UPDATE_SETTINGS
                .read()
                .release_manifest
                .as_deref()
                .map(|m| !m.trim().is_empty())
                .unwrap_or(false);
            (
                update_engine::available(),
                update_engine::engine_running(),
                update_engine::read_request(),
                update_engine::log_tail(),
                update_engine::read_exit(),
                configured,
            )
        })
        .await
        .unwrap_or((false, false, None, String::new(), None, false));

    ResponseHelper::json_success(serde_json::json!({
        "available": available,
        "configured": configured,
        "running": running,
        "current_version": env!("CARGO_PKG_VERSION"),
        "request": request,
        "exit": exit,
        "log_tail": log_tail,
        // The scope is a URL and a signature, never a command: the UI states
        // what will happen rather than implying a silent background swap.
        "restarts_service": true,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ApplyUpdateRequest {
    /// Override the saved channel for this one update.
    #[serde(default)]
    pub manifest: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    /// Trust anchor path; absent = the installer's default.
    #[serde(default)]
    pub pubkey: Option<String>,
    /// Must repeat `apply`: this restarts the server, so the click has to be
    /// deliberate (the dialog also names the consequence).
    #[serde(default)]
    pub confirm: String,
}

/// `POST /api/settings/update/apply` — queue and start the update engine.
pub async fn handle_update_apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let req: ApplyUpdateRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };
    if req.confirm.trim().eq_ignore_ascii_case("apply") == false {
        return ResponseHelper::json_error(
            "Confirm the update: it restarts HomeVault and can take a few minutes.".to_string(),
        );
    }

    // The request is validated **before** asking whether the host can run it:
    // a malformed channel is a 400 on any server, not a 503 that hides it.
    // The channel: the request's manifest, else the saved one.
    let saved = crate::handlers::settings::UPDATE_SETTINGS
        .read()
        .release_manifest
        .clone();
    let manifest = match req.manifest.as_deref().filter(|m| !m.trim().is_empty()) {
        Some(raw) => match update_engine::validate_manifest(raw) {
            Ok(value) => value,
            Err(e) => return ResponseHelper::json_error(e),
        },
        None => match saved.filter(|m| !m.trim().is_empty()) {
            Some(value) => value,
            None => {
                return ResponseHelper::json_error(
                    "No signed release channel is configured: set the release manifest in \
                     Update settings first"
                        .to_string(),
                )
            }
        },
    };
    let signature = match req.signature.as_deref() {
        Some(raw) => match update_engine::validate_signature(raw) {
            Ok(value) => value,
            Err(e) => return ResponseHelper::json_error(e),
        },
        None => None,
    };
    let pubkey = match req.pubkey.as_deref() {
        Some(raw) => match update_engine::validate_pubkey(raw) {
            Ok(value) => value,
            Err(e) => return ResponseHelper::json_error(e),
        },
        None => None,
    };

    if !update_engine::available() {
        return ResponseHelper::json_unavailable(
            "The update engine is not installed on this server (installer script or \
             homevault-update.service missing)",
        );
    }
    if update_engine::engine_running() {
        return ResponseHelper::json_error("An update is already running".to_string());
    }

    let request = UpdateRequest {
        manifest,
        signature,
        pubkey,
        actor: Some(admin.clone()),
        requested_at: Some(chrono::Utc::now().to_rfc3339()),
    };
    let written = tokio::task::spawn_blocking({
        let request = request.clone();
        move || update_engine::write_request(&request)
    })
    .await;
    match written {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return ResponseHelper::json_error(format!("Could not queue the update: {e}"))
        }
        Err(e) => return ResponseHelper::json_internal_error(format!("Queue task failed: {e}")),
    }

    let started = tokio::task::spawn_blocking(update_engine::start_engine).await;
    match started {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{admin}' started a signed update from {}",
                request.manifest
            ));
            ResponseHelper::json_success(serde_json::json!({
                "queued": true,
                "restarting": true,
                // The service restarts under us: the caller must poll the
                // status endpoint rather than expect this request to return a
                // final outcome.
                "poll": "/api/settings/update/status",
            }))
        }
        Ok(Err(e)) => {
            // Nothing ran: drop the request so a later attempt is clean.
            let _ = tokio::task::spawn_blocking(update_engine::clear_request).await;
            ResponseHelper::json_error(format!("The update engine could not start: {e}"))
        }
        Err(e) => {
            let _ = tokio::task::spawn_blocking(update_engine::clear_request).await;
            ResponseHelper::json_internal_error(format!("Start task failed: {e}"))
        }
    }
}
