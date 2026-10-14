/* =============================================================================
 * File          : system.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Server power control: graceful restart and shutdown. Each endpoint
 *   validates the session and admin role inline, answers first, and runs
 *   the system command from a short-delay background task so the client
 *   receives the confirmation before the machine goes down.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - http (>= 1)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/system/restart and /api/system/shutdown in src/web/mod.rs;
 *   admin-only.
 * =============================================================================
 */

//! Server control endpoints: graceful restart and shutdown (admin-only,
//! session checked inline per handler).

use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, response::Response};
use http::HeaderMap;
use std::sync::Arc;
use tokio::time::Duration;

/// Initiates a graceful restart of the server.
///
/// Requires admin privileges. This function spawns a background task
/// that waits briefly before executing the system shutdown command with reboot flag.
pub async fn handle_server_restart(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // Verify admin privileges
    let session_token = match headers.get("x-session-token").and_then(|h| h.to_str().ok()) {
        Some(token) if !token.is_empty() => token,
        _ => return ResponseHelper::json_unauthorized("No session token provided".to_string()),
    };

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    if !state.auth.is_admin(&username).await {
        return ResponseHelper::json_forbidden("Admin privileges required".to_string());
    }

    tracing::info!("Restart requested by admin: {}", username);

    // Spawn background task to avoid blocking the response
    let username_clone = username.clone();
    tokio::spawn(async move {
        // Wait a moment for the response to be sent
        tokio::time::sleep(Duration::from_secs(2)).await;

        tracing::info!("Executing system reboot (requested by {})", username_clone);
        let output = tokio::process::Command::new("shutdown")
            .args(["-r", "now"])
            .output()
            .await;

        match output {
            Ok(result) => {
                if result.status.success() {
                    tracing::info!("Reboot command executed successfully");
                } else {
                    tracing::error!("Reboot command failed: {:?}", result.stderr);
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute reboot command: {}", e);
            }
        }
    });

    ResponseHelper::json_success(serde_json::json!({
        "message": "Server restart initiated",
        "status": "restarting"
    }))
}

/// Initiates a graceful shutdown of the server.
///
/// Requires admin privileges. This function spawns a background task
/// that waits briefly before executing the system shutdown command.
pub async fn handle_server_shutdown(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // Verify admin privileges
    let session_token = match headers.get("x-session-token").and_then(|h| h.to_str().ok()) {
        Some(token) if !token.is_empty() => token,
        _ => return ResponseHelper::json_unauthorized("No session token provided".to_string()),
    };

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    if !state.auth.is_admin(&username).await {
        return ResponseHelper::json_forbidden("Admin privileges required".to_string());
    }

    tracing::info!("Shutdown requested by admin: {}", username);

    // Spawn background task to avoid blocking the response
    let username_clone = username.clone();
    tokio::spawn(async move {
        // Wait a moment for the response to be sent
        tokio::time::sleep(Duration::from_secs(2)).await;

        tracing::info!(
            "Executing system shutdown (requested by {})",
            username_clone
        );
        let output = tokio::process::Command::new("shutdown")
            .args(["-h", "now"])
            .output()
            .await;

        match output {
            Ok(result) => {
                if result.status.success() {
                    tracing::info!("Shutdown command executed successfully");
                } else {
                    tracing::error!("Shutdown command failed: {:?}", result.stderr);
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute shutdown command: {}", e);
            }
        }
    });

    ResponseHelper::json_success(serde_json::json!({
        "message": "Server shutdown initiated",
        "status": "shutting_down"
    }))
}
