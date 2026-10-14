/* =============================================================================
 * File          : snmp.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SNMP query endpoints for monitored network devices: system info,
 *   interface and storage statistics, availability probes, plus the SNMP
 *   settings (persisted as JSON, cached in a process-wide lock). Queries
 *   run through SnmpService; every endpoint is admin-only.
 *
 * Dependencies:
 *   - parking_lot (>= 0.12)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/snmp in src/web/mod.rs; settings persist via JsonStorage.
 * =============================================================================
 */

//! SNMP query endpoints (interfaces, storage, availability, settings) —
//! all admin-only.

use crate::handlers::AppState;
use crate::snmp::SnmpService;
use crate::storage::json::JsonStorage;
use crate::utils::ResponseHelper;
use axum::{extract::Query, extract::State, http::HeaderMap, response::Response};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Query parameters for SNMP info/interface/storage requests.
#[derive(Debug, Deserialize)]
pub struct SnmpQueryParams {
    pub community: Option<String>,
    pub iface: Option<String>,
}

/// SNMP settings (community string, protocol version), persisted to disk as
/// its own `snmp.json` under the data directory (same `JsonStorage` file
/// format the rest of Settings uses) — kept as a standalone key rather than
/// folded into `settings_persistence.rs`'s shared `PersistedSettings` blob,
/// since this module owns its own small, independent piece of config.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SnmpSettings {
    pub community: Option<String>,
    pub version: Option<String>,
}

impl Default for SnmpSettings {
    fn default() -> Self {
        Self {
            community: Some("public".to_string()),
            version: Some("2c".to_string()),
        }
    }
}

const SNMP_STORAGE_KEY: &str = "snmp";

fn snmp_storage() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// Persists the current settings to disk; failures are logged, not fatal —
/// the in-memory value (already updated by the caller) still takes effect
/// for the rest of this process's lifetime even if the write itself fails.
fn persist_snmp_settings(settings: &SnmpSettings) {
    if let Err(e) = snmp_storage().save(SNMP_STORAGE_KEY, settings) {
        tracing::error!("Failed to persist SNMP settings: {}", e);
    }
}

/// Resets the persisted and in-memory SNMP settings to their defaults —
/// used by "Reset to Defaults" on the Settings page.
pub fn reset_snmp_settings() {
    let defaults = SnmpSettings::default();
    *SNMP_SETTINGS.write() = defaults.clone();
    persist_snmp_settings(&defaults);
}

static SNMP_SETTINGS: Lazy<RwLock<SnmpSettings>> = Lazy::new(|| {
    let loaded = snmp_storage()
        .load::<SnmpSettings>(SNMP_STORAGE_KEY)
        .ok()
        .flatten();
    RwLock::new(loaded.unwrap_or_default())
});

/// Body for updating SNMP settings.
#[derive(Debug, Deserialize)]
pub struct SnmpSettingsRequest {
    pub community: Option<String>,
    pub version: Option<String>,
}

/// Returns SNMP system information (hostname, uptime, location, etc.).
pub async fn handle_snmp_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<SnmpQueryParams>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let community = params.community.unwrap_or_else(|| "public".to_string());

    match SnmpService::get_system_info(&community) {
        Ok(info) => ResponseHelper::json_success(info),
        Err(e) => ResponseHelper::json_error(format!("SNMP error: {}", e)),
    }
}

/// Returns SNMP interface stats (bytes in/out, errors, etc.).
pub async fn handle_snmp_interfaces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<SnmpQueryParams>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let community = params.community.unwrap_or_else(|| "public".to_string());
    let iface = params.iface.unwrap_or_default();

    match SnmpService::get_interface_stats(&iface, &community) {
        Ok(interfaces) => ResponseHelper::json_success(interfaces),
        Err(e) => ResponseHelper::json_error(format!("SNMP error: {}", e)),
    }
}

/// Returns SNMP storage/disk statistics.
pub async fn handle_snmp_storage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<SnmpQueryParams>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let community = params.community.unwrap_or_else(|| "public".to_string());

    match SnmpService::get_storage_stats(&community) {
        Ok(stats) => ResponseHelper::json_success(stats),
        Err(e) => ResponseHelper::json_error(format!("SNMP error: {}", e)),
    }
}

/// Returns whether the SNMP service is available on this system.
pub async fn handle_snmp_availability(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    ResponseHelper::json_success(serde_json::json!({
        "available": SnmpService::is_available()
    }))
}

/// Updates the in-memory SNMP settings (community, version).
pub async fn handle_snmp_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<SnmpSettingsRequest>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let mut settings = SNMP_SETTINGS.write();
    if let Some(community) = &req.community {
        settings.community = Some(community.clone());
    }
    if let Some(version) = &req.version {
        settings.version = Some(version.clone());
    }
    persist_snmp_settings(&settings);
    drop(settings);

    tracing::info!("SNMP settings updated");
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "message": "SNMP settings saved successfully"
    }))
}

/// Returns the current in-memory SNMP settings.
pub async fn handle_snmp_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let settings = SNMP_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "community": settings.community,
        "version": settings.version,
    }))
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
                return Some(ResponseHelper::json_error(
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
