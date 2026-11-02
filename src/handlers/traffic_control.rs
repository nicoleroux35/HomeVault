/* =============================================================================
 * File          : traffic_control.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Bandwidth-control endpoints: read the policy and what this host can do,
 *   save it, pause every transfer, and re-apply the policy now. Reads need a
 *   session; every mutation is admin-only because the policy runs root-owned
 *   `tc` commands.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::traffic_control
 *
 * Usage:
 *   Routes under /api/network/traffic-control in src/web/mod.rs; see
 *   docs/guides/traffic-control.md.
 * =============================================================================
 */

//! Per-service bandwidth-control endpoints, see docs/guides/traffic-control.md.
//!
//! * `GET  /api/network/traffic-control`      — policy + capabilities + state
//! * `PUT  /api/network/traffic-control`      — save and apply
//! * `POST /api/network/traffic-control/pause` — pause-all on/off
//! * `POST /api/network/traffic-control/apply` — re-apply now

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};
use chrono::Utc;
use serde::Deserialize;

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::traffic_control::{
    self, ApplyReport, ScheduleWindow, ServiceRate, TrafficControlSettings,
};
use crate::utils::ResponseHelper;

use super::AppState;

/// The body of a save request. `updated_at` is server-owned and never read
/// from the client.
#[derive(Debug, Deserialize)]
pub struct TrafficControlRequest {
    pub enabled: bool,
    pub interface: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub services: Vec<ServiceRate>,
    #[serde(default)]
    pub schedule: Vec<ScheduleWindow>,
}

impl TrafficControlRequest {
    fn into_settings(self) -> TrafficControlSettings {
        TrafficControlSettings {
            enabled: self.enabled,
            interface: self.interface.trim().to_string(),
            paused: self.paused,
            services: self.services,
            schedule: self.schedule,
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}

/// `{ interface, type, status, ipv4 }` — enough for the picker without
/// shipping counters.
fn interfaces_json() -> Vec<serde_json::Value> {
    crate::network::list_interfaces()
        .into_iter()
        .map(|i| {
            serde_json::json!({
                "name": i.name,
                "type": i.iface_type,
                "status": i.status,
                "ipv4": i.ipv4,
            })
        })
        .collect()
}

fn state_json(settings: &TrafficControlSettings) -> serde_json::Value {
    let now = Utc::now();
    serde_json::json!({
        "shaping": settings.should_shape(now),
        "reason": settings.inactive_reason(now),
        "reason_code": settings.inactive_reason_code(now),
        "commands": traffic_control::build_commands(settings)
            .iter()
            .map(|c| c.describe())
            .collect::<Vec<_>>(),
    })
}

fn report_json(report: &ApplyReport) -> serde_json::Value {
    serde_json::json!({
        "applied": report.applied,
        "reason": report.reason,
        "commands": report.commands,
        "failed": report.failed,
    })
}

fn settings_json(settings: &TrafficControlSettings) -> serde_json::Value {
    serde_json::json!({
        "enabled": settings.enabled,
        "interface": settings.interface,
        "paused": settings.paused,
        "services": settings.services,
        "schedule": settings.schedule,
        "updated_at": settings.updated_at,
    })
}

fn payload(settings: &TrafficControlSettings) -> serde_json::Value {
    serde_json::json!({
        "settings": settings_json(settings),
        "capabilities": traffic_control::capabilities(),
        "interfaces": interfaces_json(),
        "state": state_json(settings),
    })
}

/// Run the apply pass on the blocking pool; `tc` is a subprocess.
async fn apply(settings: TrafficControlSettings) -> ApplyReport {
    let now = Utc::now();
    tokio::task::spawn_blocking(move || traffic_control::apply_sync(&settings, now))
        .await
        .unwrap_or_else(|e| ApplyReport {
            applied: false,
            reason: Some("The traffic-control worker failed".to_string()),
            commands: Vec::new(),
            failed: Some(e.to_string()),
        })
}

/// `GET /api/network/traffic-control`
pub async fn handle_traffic_control_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let settings = traffic_control::load();
    ResponseHelper::json_success(payload(&settings))
}

/// `PUT /api/network/traffic-control`
pub async fn handle_traffic_control_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TrafficControlRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let settings = body.into_settings();
    if let Err(e) = traffic_control::validate(&settings) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = traffic_control::save(&settings) {
        return ResponseHelper::json_error(e);
    }
    traffic_control::invalidate();
    let report = apply(settings.clone()).await;
    crate::events::log_event(format!(
        "Admin '{}' updated traffic control (interface {}, {} limited service(s), shaping {})",
        admin,
        settings.interface,
        settings.limited_services().len(),
        if report.applied { "on" } else { "off" }
    ));
    ResponseHelper::json_success(serde_json::json!({
        "settings": settings_json(&settings),
        "capabilities": traffic_control::capabilities(),
        "interfaces": interfaces_json(),
        "state": state_json(&settings),
        "apply": report_json(&report),
    }))
}

/// `POST /api/network/traffic-control/pause`
///
/// Pause-all keeps the saved policy and removes the queue, so a link that is
/// being hammered can be released with one action instead of editing every
/// rule.
pub async fn handle_traffic_control_pause(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let Some(paused) = body.get("paused").and_then(|v| v.as_bool()) else {
        return ResponseHelper::json_error("A boolean 'paused' is required".to_string());
    };
    let mut settings = traffic_control::load();
    settings.paused = paused;
    settings.updated_at = Utc::now().to_rfc3339();
    if let Err(e) = traffic_control::save(&settings) {
        return ResponseHelper::json_error(e);
    }
    traffic_control::invalidate();
    let report = apply(settings.clone()).await;
    crate::events::log_event(format!(
        "Admin '{}' {} all transfers",
        admin,
        if paused { "paused" } else { "resumed" }
    ));
    ResponseHelper::json_success(serde_json::json!({
        "settings": settings_json(&settings),
        "state": state_json(&settings),
        "apply": report_json(&report),
    }))
}

/// `POST /api/network/traffic-control/apply`
pub async fn handle_traffic_control_apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let settings = traffic_control::load();
    if let Err(e) = traffic_control::validate(&settings) {
        return ResponseHelper::json_error(e);
    }
    traffic_control::invalidate();
    let report = apply(settings.clone()).await;
    crate::events::log_event(format!(
        "Admin '{}' re-applied traffic control ({})",
        admin,
        if report.applied {
            "shaping on"
        } else {
            "shaping off"
        }
    ));
    ResponseHelper::json_success(serde_json::json!({
        "settings": settings_json(&settings),
        "state": state_json(&settings),
        "apply": report_json(&report),
    }))
}
