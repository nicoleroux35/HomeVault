/* =============================================================================
 * File          : power.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Host power-management endpoints: status (governor + capabilities),
 *   governor set, host reboot/shutdown/suspend, and per-disk spindown
 *   settings (list/upsert/apply). Every endpoint is admin-only — these
 *   actions take the whole box (or its disks) down.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/power in src/web/mod.rs; primitives in src/power.rs.
 * =============================================================================
 */

//! Host power endpoints (governor, reboot/shutdown/suspend, disk spindown).

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::power::{
    apply_all_spindown, apply_spindown, available_governors, current_governor, host_power_action,
    is_valid_block_device_name, is_valid_governor, set_governor, DiskSpindownSetting, PowerAction,
};
use crate::storage::disk_power_repo::DiskPowerRepository;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json};
use std::sync::Arc;

/// GET /power/status — what this host supports + the current governor.
/// Read-only, no side effects, so the Settings card can render even on
/// hosts where cpufreq/systemd suspend are unavailable.
pub async fn handle_power_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let governors = available_governors();
    // `hdparm --version` forks; keep it off the runtime (ROADMAP X.1) and
    // bounded (X.2).
    let spindown_supported = tokio::task::spawn_blocking(spindown_tool_available)
        .await
        .unwrap_or(false);
    ResponseHelper::json_success(serde_json::json!({
        "governor": current_governor(),
        "available_governors": governors,
        "governor_supported": !governors.is_empty(),
        "spindown_supported": spindown_supported,
    }))
}

/// Whether `hdparm` exists (sudo-wrapped calls fail harmlessly otherwise —
/// the UI disables the section instead of showing buttons that always fail).
fn spindown_tool_available() -> bool {
    use crate::utils::process::{run_timed, HOST_PROBE_TIMEOUT};
    run_timed(
        std::process::Command::new("hdparm").arg("--version"),
        HOST_PROBE_TIMEOUT,
    )
    .map(|s| s.status.success())
    .unwrap_or(false)
}

/// PUT /power/governor — { "governor": "performance" }.
pub async fn handle_power_governor_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let Some(governor) = req.get("governor").and_then(|v| v.as_str()) else {
        return ResponseHelper::json_error("Missing 'governor' field".to_string());
    };
    if !is_valid_governor(governor) {
        return ResponseHelper::json_error(format!(
            "Unknown or invalid CPU governor '{}'",
            governor
        ));
    }
    let governor = governor.to_string();
    match tokio::task::spawn_blocking(move || set_governor(&governor)).await {
        Ok(Ok(())) => {
            crate::events::log_event("Admin changed the CPU governor".to_string());
            ResponseHelper::json_success(serde_json::json!({ "governor": current_governor() }))
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Governor task failed: {e}")),
    }
}

/// POST /power/{action} — host reboot / shutdown / suspend. The UI confirms
/// before calling; this is the point of no return.
pub async fn handle_power_action(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(action): axum::extract::Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let Some(action) = PowerAction::parse(&action) else {
        return ResponseHelper::json_error(
            "Unknown power action: expected reboot, shutdown or suspend".to_string(),
        );
    };
    crate::events::log_event(format!("Admin issued host {}", action.as_str()));
    // Suspend returns when the host wakes; reboot/shutdown kill the process
    // mid-response, so the JSON answer is best-effort by nature.
    match tokio::task::spawn_blocking(move || host_power_action(action)).await {
        Ok(Ok(())) => {
            ResponseHelper::json_success(serde_json::json!({ "action": action.as_str() }))
        }
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Power task failed: {e}")),
    }
}

/// GET /power/spindown — per-disk spindown settings.
pub async fn handle_power_spindown_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = DiskPowerRepository::new(state.db.pool());
    match repo.list().await {
        Ok(settings) => ResponseHelper::json_success(serde_json::json!({ "settings": settings })),
        Err(e) => ResponseHelper::json_error(format!("Failed to read spindown settings: {e}")),
    }
}

/// PUT /power/spindown — upsert one disk's settings. Both fields `None`
/// clears the disk's configuration. Applied immediately so the admin sees
/// the effect without waiting for the next boot.
pub async fn handle_power_spindown_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let Some(device) = req.get("device").and_then(|v| v.as_str()) else {
        return ResponseHelper::json_error("Missing 'device' field".to_string());
    };
    if !is_valid_block_device_name(device) {
        return ResponseHelper::json_error(format!("Invalid device name '{device}'"));
    }
    let apm_level = match req.get("apm_level") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(n @ 1..=255) => Some(n as u8),
            _ => return ResponseHelper::json_error("apm_level must be an integer 1-255".into()),
        },
    };
    let spindown_minutes = match req.get("spindown_minutes") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match v.as_u64() {
            // 0 = never spindown; > 330 (5.5 h) has no hdparm code.
            Some(n @ 0..=330) => Some(n as u32),
            _ => {
                return ResponseHelper::json_error(
                    "spindown_minutes must be an integer 0-330".to_string(),
                )
            }
        },
    };
    let setting = DiskSpindownSetting {
        device: device.to_string(),
        apm_level,
        spindown_minutes,
    };

    let repo = DiskPowerRepository::new(state.db.pool());
    if let Err(e) = repo.upsert(&setting).await {
        return ResponseHelper::json_error(format!("Failed to save spindown setting: {e}"));
    }
    // Apply now (best-effort — SSDs/USB bridges may refuse APM; the saved
    // setting still stands for the next boot).
    let apply_result = tokio::task::spawn_blocking({
        let setting = setting.clone();
        move || apply_spindown(&setting)
    })
    .await
    .ok();
    let applied = matches!(apply_result, Some(Ok(())));

    crate::events::log_event(format!("Admin updated spindown settings for /dev/{device}"));
    ResponseHelper::json_success(serde_json::json!({
        "setting": setting,
        "applied": applied,
    }))
}

/// POST /power/spindown/apply — re-apply every saved setting now (e.g.
/// after adding a disk).
pub async fn handle_power_spindown_apply(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = DiskPowerRepository::new(state.db.pool());
    let settings = match repo.list().await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to read settings: {e}")),
    };
    tokio::task::spawn_blocking(move || apply_all_spindown(&settings))
        .await
        .ok();
    ResponseHelper::json_success(serde_json::json!({ "applied": true }))
}
