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

    let (community, version) = effective_connection(&params);

    // Every `snmpget`/`snmpwalk` is a blocking child process with a 10 s
    // deadline, so it runs on the blocking pool (ROADMAP X.1).
    match tokio::task::spawn_blocking(move || SnmpService::get_system_info(&community, &version))
        .await
    {
        Ok(Ok(info)) => ResponseHelper::json_success(info),
        Ok(Err(e)) => ResponseHelper::json_error(format!("SNMP error: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("SNMP task failed: {}", e)),
    }
}

/// Returns SNMP interface stats (bytes in/out, errors, etc.).
///
/// `iface` narrows the result to one interface (ROADMAP 13.5 — it used to be
/// accepted and ignored).
pub async fn handle_snmp_interfaces(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<SnmpQueryParams>,
) -> Response {
    if let Some(resp) = require_admin(&headers, &state).await {
        return resp;
    }

    let iface = params.iface.clone().unwrap_or_default();
    let (community, version) = effective_connection(&params);

    match tokio::task::spawn_blocking(move || {
        SnmpService::get_interface_stats(&iface, &community, &version)
    })
    .await
    {
        Ok(Ok(interfaces)) => ResponseHelper::json_success(interfaces),
        Ok(Err(e)) => ResponseHelper::json_error(format!("SNMP error: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("SNMP task failed: {}", e)),
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

    let (community, version) = effective_connection(&params);

    match tokio::task::spawn_blocking(move || SnmpService::get_storage_stats(&community, &version))
        .await
    {
        Ok(Ok(stats)) => ResponseHelper::json_success(stats),
        Ok(Err(e)) => ResponseHelper::json_error(format!("SNMP error: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("SNMP task failed: {}", e)),
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

    let available = tokio::task::spawn_blocking(SnmpService::is_available)
        .await
        .unwrap_or(false);
    ResponseHelper::json_success(serde_json::json!({ "available": available }))
}

/// The community string and protocol version a query actually uses: the
/// per-request override when present, otherwise the saved settings.
///
/// ROADMAP 13.5: the saved `version` was write-only — `run_walk` always passed
/// `2c`. Reading it here is what makes the field mean something.
fn effective_connection(params: &SnmpQueryParams) -> (String, String) {
    let saved = SNMP_SETTINGS.read().clone();
    let community = params
        .community
        .clone()
        .filter(|c| !c.trim().is_empty())
        .or(saved.community)
        .unwrap_or_else(|| "public".to_string());
    let version = saved
        .version
        .unwrap_or_else(|| "2c".to_string())
        .trim()
        .to_string();
    (community, version)
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

    // ROADMAP 13.5: the version is now actually used by every query, so an
    // unsupported value is refused at save time instead of producing a broken
    // query later (v3 needs credentials this card does not collect).
    let version = match req.version.as_deref() {
        Some(raw) => match crate::snmp::validate_snmp_version(raw) {
            Ok(version) => Some(version),
            Err(e) => return ResponseHelper::json_error(e),
        },
        None => None,
    };

    let mut next = SNMP_SETTINGS.read().clone();
    if let Some(community) = &req.community {
        next.community = Some(community.trim().to_string());
    }
    if let Some(version) = version {
        next.version = Some(version);
    }
    *SNMP_SETTINGS.write() = next.clone();
    persist_snmp_settings(&next);

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
