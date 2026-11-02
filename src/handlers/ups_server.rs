/* =============================================================================
 * File          : ups_server.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPS server mode endpoints: read the state and configuration, save it,
 *   start and stop `upsd`, and run the orderly shutdown.
 *
 *   Reads need a session; every mutation is admin-only. The shutdown route in
 *   particular ends the machine: a denial there is the difference between an
 *   appliance and a LAN-wide remote power-off button.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::ups_server
 *
 * Usage:
 *   Routes under /api/power/ups-server in src/web/mod.rs; see
 *   docs/guides/ups-server.md.
 * =============================================================================
 */

//! UPS server mode endpoints, see docs/guides/ups-server.md.
//!
//! * `GET  /api/power/ups-server`          — availability + config + state
//! * `PUT  /api/power/ups-server`          — save (and restart `upsd`)
//! * `POST /api/power/ups-server/start`    — start `upsd`
//! * `POST /api/power/ups-server/stop`     — stop `upsd`
//! * `POST /api/power/ups-server/shutdown` — run the orderly shutdown

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::AppState;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::ups_server::{UpsServerConfig, UpsServerService, UpsServerState};
use crate::utils::ResponseHelper;

/// Re-wraps a `(status, message)` pair from a helper as the JSON error
/// envelope shared by the rest of the API.
fn fail(err: (StatusCode, String)) -> Response {
    ResponseHelper::json_error_with_status(err.0, err.1)
}

/// GET /api/power/ups-server — configuration, live state and whether `upsd`
/// is installed. Any signed-in user may look.
pub async fn handle_ups_server_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let config = match load_config().await {
        Ok(config) => config,
        Err(e) => return fail(e),
    };
    let state_payload = UpsServerService::get_state().await;
    let available = UpsServerService::is_available().await;

    ResponseHelper::json_success(UpsServerGetResponse {
        available,
        config,
        state: state_payload,
    })
}

/// PUT /api/power/ups-server — validate, save, then restart `upsd` if enabled.
pub async fn handle_ups_server_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let req: UpsServerUpdateRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    if let Err(e) = UpsServerService::validate_config(&req.config) {
        return ResponseHelper::json_error(e);
    }

    if let Err(e) = save_config(&req.config).await {
        return ResponseHelper::json_internal_error(e);
    }

    // Apply: enabled means a freshly configured daemon, disabled means a
    // stopped one. Either way the host ends up matching what was saved.
    if req.config.enabled {
        if UpsServerService::is_running().await {
            if let Err(e) = UpsServerService::stop_server().await {
                return ResponseHelper::json_internal_error(e);
            }
        }

        if let Err(e) = UpsServerService::start_server(&req.config).await {
            return ResponseHelper::json_internal_error(e);
        }
    } else if UpsServerService::is_running().await {
        if let Err(e) = UpsServerService::stop_server().await {
            return ResponseHelper::json_internal_error(e);
        }
    }

    crate::events::log_event(format!(
        "Admin '{}' saved the UPS server configuration ({})",
        admin,
        if req.config.enabled {
            "enabled"
        } else {
            "disabled"
        }
    ));

    ResponseHelper::json_success(UpsServerUpdateResponse {
        requires_restart: false,
    })
}

/// POST /api/power/ups-server/start
pub async fn handle_ups_server_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let config = match load_config().await {
        Ok(config) => config,
        Err(e) => return fail(e),
    };

    if !config.enabled {
        return ResponseHelper::json_error(
            "UPS server is not enabled in configuration".to_string(),
        );
    }

    if let Err(e) = UpsServerService::start_server(&config).await {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!("Admin '{}' started the UPS server", admin));
    ResponseHelper::json_success(serde_json::json!({ "running": true }))
}

/// POST /api/power/ups-server/stop
pub async fn handle_ups_server_stop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    if let Err(e) = UpsServerService::stop_server().await {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!("Admin '{}' stopped the UPS server", admin));
    ResponseHelper::json_success(serde_json::json!({ "running": false }))
}

/// POST /api/power/ups-server/shutdown — run the orderly shutdown.
///
/// The request must be confirmed explicitly: this ends the machine. A refused
/// confirmation is a `400` naming the rule, never a silent no-op.
pub async fn handle_ups_server_shutdown(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };

    let req: ShutdownRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {}", e)),
    };

    if !req.confirm {
        return ResponseHelper::json_error(
            "Shutdown must be confirmed explicitly (confirm: true)".to_string(),
        );
    }

    let config = match load_config().await {
        Ok(config) => config,
        Err(e) => return fail(e),
    };

    crate::events::log_event_warn(format!(
        "Admin '{}' started an orderly shutdown via the UPS server",
        admin
    ));

    let results = UpsServerService::execute_shutdown(&config.shutdown_policy).await;

    // `success` means "every enabled stage ran". A skipped stage makes the
    // answer false so the caller cannot mistake "did not happen" for "done".
    let all_success = results.iter().all(|r| r.success);

    ResponseHelper::json_success(ShutdownResponse {
        success: all_success,
        stages: results
            .into_iter()
            .map(|r| ShutdownStageResponse {
                stage: r.stage,
                success: r.success,
                skipped: r.skipped,
                duration_ms: r.duration_ms,
                error: r.error,
            })
            .collect(),
    })
}

// Request/Response types

#[derive(Debug, Serialize)]
pub struct UpsServerGetResponse {
    pub available: bool,
    pub config: UpsServerConfig,
    pub state: UpsServerState,
}

#[derive(Debug, Deserialize)]
pub struct UpsServerUpdateRequest {
    pub config: UpsServerConfig,
}

#[derive(Debug, Serialize)]
pub struct UpsServerUpdateResponse {
    pub requires_restart: bool,
}

#[derive(Debug, Deserialize)]
pub struct ShutdownRequest {
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
pub struct ShutdownResponse {
    pub success: bool,
    pub stages: Vec<ShutdownStageResponse>,
}

#[derive(Debug, Serialize)]
pub struct ShutdownStageResponse {
    pub stage: String,
    pub success: bool,
    /// True when the policy enabled the stage but this build cannot perform
    /// it. The UI shows it as "not available", never as a success.
    pub skipped: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// Load UPS server configuration from disk
async fn load_config() -> Result<UpsServerConfig, (StatusCode, String)> {
    let path = config_path();

    if !path.exists() {
        return Ok(UpsServerConfig::default());
    }

    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read config: {}", e),
        )
    })?;

    serde_json::from_str(&content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to parse config: {}", e),
        )
    })
}

/// Save UPS server configuration to disk
async fn save_config(config: &UpsServerConfig) -> Result<(), String> {
    let path = config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("failed to create config directory: {}", e))?;
    }

    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed to serialize config: {}", e))?;

    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("failed to write config: {}", e))?;

    Ok(())
}

fn config_path() -> std::path::PathBuf {
    let data_dir =
        std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string());
    std::path::PathBuf::from(data_dir).join("data/ups_server.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ups_server::ShutdownPolicy;
    use std::sync::{Mutex, MutexGuard};

    // `config_path()` reads the process-global `DATA_DIRECTORY` env var, and
    // these tests mutate it — serialise them. Without the guard the two tests
    // below raced each other and failed intermittently (they were red in the
    // working tree before this module was touched).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_config_path_uses_data_directory() {
        let _guard = env_guard();
        std::env::set_var("DATA_DIRECTORY", "/tmp/test");
        let path = config_path();
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/test/data/ups_server.json")
        );
        std::env::remove_var("DATA_DIRECTORY");
    }

    #[test]
    fn test_config_path_default() {
        let _guard = env_guard();
        std::env::remove_var("DATA_DIRECTORY");
        let path = config_path();
        assert_eq!(
            path,
            std::path::PathBuf::from("/var/lib/homevault/data/ups_server.json")
        );
    }

    #[tokio::test]
    async fn test_save_and_load_config() {
        let _guard = env_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", temp_dir.path());

        let config = UpsServerConfig {
            enabled: true,
            bind_interface: "eth0".to_string(),
            bind_address: Some("192.168.1.1".parse().unwrap()),
            port: 3493,
            allowed_clients: vec![],
            shutdown_policy: ShutdownPolicy::default(),
            max_clients: 5,
        };

        // Save
        save_config(&config).await.unwrap();

        // Load
        let loaded = load_config().await.unwrap();

        assert_eq!(loaded.enabled, config.enabled);
        assert_eq!(loaded.bind_interface, config.bind_interface);
        assert_eq!(loaded.port, config.port);
        assert_eq!(loaded.max_clients, config.max_clients);

        std::env::remove_var("DATA_DIRECTORY");
    }

    #[tokio::test]
    async fn test_load_config_returns_default_when_missing() {
        let _guard = env_guard();
        let temp_dir = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", temp_dir.path());

        let loaded = load_config().await.unwrap();

        assert!(!loaded.enabled);
        assert_eq!(loaded.port, 3493);

        std::env::remove_var("DATA_DIRECTORY");
    }

    /// The shutdown stage payload must carry `skipped`, otherwise a client
    /// cannot tell "ran" from "impossible on this build".
    #[test]
    fn test_shutdown_stage_response_carries_skipped() {
        let response = ShutdownStageResponse {
            stage: "shutdown_vms".to_string(),
            success: false,
            skipped: true,
            duration_ms: 1,
            error: Some("not wired".to_string()),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["skipped"], serde_json::json!(true));
        assert_eq!(json["success"], serde_json::json!(false));
    }
}
