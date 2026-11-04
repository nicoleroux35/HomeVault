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
use crate::snmp_agent::{AgentConfig, BindScope};
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
/// Its own key, so enabling the local agent never touches the client settings
/// above (and vice versa): they are two different features that happen to
/// share the word "SNMP".
const SNMP_AGENT_STORAGE_KEY: &str = "snmp-agent";

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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
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
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let settings = SNMP_SETTINGS.read();
    ResponseHelper::json_success(serde_json::json!({
        "community": settings.community,
        "version": settings.version,
    }))
}

// ---------------------------------------------------------------------------
// The local agent (phase B4) — the mirror image of everything above, which
// queries *remote* devices. See `src/snmp_agent.rs`.
// ---------------------------------------------------------------------------

/// `snmpd` missing is a capability answer (`503`), never a `400` and never a
/// silent success: the appliance can still store the settings, but it cannot
/// serve a single SNMP request, and the screen must say which of the two it
/// is looking at (P0.2 contract).
fn agent_missing() -> Response {
    ResponseHelper::json_unavailable(
        "snmpd is not installed on this appliance — install the 'snmpd' package to serve SNMP",
    )
}

/// GET /api/snmp/agent — the agent's configuration and its live state.
///
/// The community is **never** returned; `has_community` says whether one is
/// set. The state is read from the host each time, so a daemon stopped from a
/// shell shows up here instead of an optimistic "enabled".
pub async fn handle_snmp_agent_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let installed = tokio::task::spawn_blocking(crate::snmp_agent::agent_installed)
        .await
        .unwrap_or(false);
    if !installed {
        return agent_missing();
    }

    let (config, serving, candidates) = tokio::task::spawn_blocking(|| {
        (
            load_agent_config(),
            crate::snmp_agent::is_serving(),
            crate::snmp_agent::lan_bind_candidates(),
        )
    })
    .await
    .unwrap_or_else(|_| (AgentConfig::default(), false, Vec::new()));

    let mut payload = config.redacted();
    payload["installed"] = serde_json::json!(true);
    payload["serving"] = serde_json::json!(serving);
    payload["lan_candidates"] = serde_json::json!(candidates);
    ResponseHelper::json_success(payload)
}

/// Body for `PUT /api/snmp/agent`. Every field is optional so the screen can
/// flip `enabled` alone without resending the whole document.
#[derive(Debug, Deserialize)]
pub struct AgentConfigRequest {
    #[serde(default)]
    pub enabled: Option<bool>,
    /// `"loopback"` or `"lan"`.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub lan_address: Option<String>,
    /// Empty or absent keeps the stored community (or generates one).
    #[serde(default)]
    pub community: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub contact: Option<String>,
}

/// PUT /api/snmp/agent — enable/disable and configure the local agent.
///
/// Contract: the settings are validated **before** anything is written,
/// persisted only once the host accepted them, and the answer carries the
/// resulting live state. A failure names which half failed (persisted config
/// vs daemon) instead of a bare "error".
pub async fn handle_snmp_agent_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<AgentConfigRequest>,
) -> Response {
    let admin_username = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let installed = tokio::task::spawn_blocking(crate::snmp_agent::agent_installed)
        .await
        .unwrap_or(false);
    if !installed {
        return agent_missing();
    }

    let previous = load_agent_config();
    let mut requested = previous.clone();
    if let Some(enabled) = req.enabled {
        requested.enabled = enabled;
    }
    if let Some(scope) = req.scope.as_deref() {
        requested.scope = match scope.trim().to_ascii_lowercase().as_str() {
            "loopback" => BindScope::Loopback,
            "lan" => BindScope::Lan,
            other => {
                return ResponseHelper::json_error(format!(
                    "Unknown scope '{other}' — use 'loopback' or 'lan'"
                ))
            }
        };
    }
    if let Some(address) = &req.lan_address {
        requested.lan_address = Some(address.clone());
    }
    if let Some(community) = &req.community {
        requested.community = community.clone();
    }
    if let Some(location) = &req.location {
        requested.location = location.clone();
    }
    if let Some(contact) = &req.contact {
        requested.contact = contact.clone();
    }

    let next = match validate_agent_config(requested, &previous) {
        Ok(next) => next,
        Err(e) => return ResponseHelper::json_error(e.0),
    };

    let applied = next.clone();
    let result =
        tokio::task::spawn_blocking(move || crate::snmp_agent::apply_to_host(&applied)).await;

    let state_label = match result {
        Ok(Ok(state)) => state,
        Ok(Err(e)) => {
            // The daemon half failed: put the previous configuration back so
            // "saved" and "running" can never disagree silently.
            let restore = previous.clone();
            let _ = tokio::task::spawn_blocking(move || crate::snmp_agent::apply_to_host(&restore))
                .await;
            return ResponseHelper::json_error(format!(
                "The agent configuration was refused by the host: {e}"
            ));
        }
        Err(e) => return ResponseHelper::json_error(format!("Internal error: {e}")),
    };

    persist_agent_config(&next);
    crate::events::log_event(format!(
        "Admin '{}' {} the SNMP agent (scope: {:?}, state: {})",
        admin_username,
        if next.enabled { "enabled" } else { "disabled" },
        next.scope,
        state_label
    ));

    let mut payload = next.redacted();
    payload["installed"] = serde_json::json!(true);
    payload["serving"] = serde_json::json!(state_label == "running");
    payload["message"] = serde_json::json!(if next.enabled {
        "The SNMP agent is configured"
    } else {
        "The SNMP agent is disabled"
    });
    ResponseHelper::json_success(payload)
}

/// Load the agent config from disk; a missing or unreadable file means "never
/// configured", which is the safe default (off).
fn load_agent_config() -> AgentConfig {
    snmp_storage()
        .load::<AgentConfig>(SNMP_AGENT_STORAGE_KEY)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn persist_agent_config(config: &AgentConfig) {
    if let Err(e) = snmp_storage().save(SNMP_AGENT_STORAGE_KEY, config) {
        tracing::error!("Failed to persist the SNMP agent settings: {}", e);
    }
}

/// Validation is shared with the module's unit tests by going through
/// `snmp_agent::validate_config`, but the type is spelled out here so the
/// handler never builds a config the module would reject.
fn validate_agent_config(
    requested: crate::snmp_agent::AgentConfig,
    previous: &crate::snmp_agent::AgentConfig,
) -> Result<crate::snmp_agent::AgentConfig, crate::snmp_agent::ConfigError> {
    crate::snmp_agent::validate_config(requested, previous)
}
