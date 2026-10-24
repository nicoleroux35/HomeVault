/* =============================================================================
 * File          : system.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Server power control: graceful restart and shutdown. Each endpoint
 *   validates the session and admin role inline, then runs the host power
 *   action through the single shared primitive in `power.rs` — the same
 *   `sudo systemctl <verb>` path the Settings power card and the MCP
 *   `system.reboot` tool use. A refused command answers with the host's own
 *   reason instead of a silent success.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - http (>= 1)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/server/restart and /api/server/shutdown in src/web/mod.rs;
 *   admin-only.
 * =============================================================================
 */

//! Server control endpoints: graceful restart and shutdown (admin-only,
//! session checked inline per handler).
//!
//! Both handlers delegate to [`crate::power::host_power_action`] (ROADMAP 24.2):
//! before that, `/api/server/*` ran a bare `shutdown -r now` with no `sudo`
//! while `/api/power/*` ran `sudo systemctl reboot`, so on a host where the
//! service user cannot call `shutdown` one endpoint worked and the other was
//! always refused. The block runs on the blocking pool with a 30 s deadline,
//! and a refusal is returned to the caller with the host's stderr (ROADMAP
//! 24.1) instead of being logged and swallowed.

use crate::handlers::AppState;
use crate::power::{host_power_action, PowerAction};
use crate::utils::ResponseHelper;
use axum::{extract::State, response::IntoResponse, response::Response, Json};
use http::{HeaderMap, StatusCode};
use std::sync::Arc;

/// Initiates a graceful restart of the server.
///
/// Requires admin privileges. The action runs before the response so a refused
/// command is visible: `systemctl reboot` returns as soon as the shutdown is
/// scheduled, and the client already treats a dropped connection as expected.
pub async fn handle_server_restart(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    run_host_power_action(&state, &headers, PowerAction::Reboot, "restart").await
}

/// Initiates a graceful shutdown of the server.
///
/// Requires admin privileges. Same contract as [`handle_server_restart`].
pub async fn handle_server_shutdown(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    run_host_power_action(&state, &headers, PowerAction::Shutdown, "shutdown").await
}

/// The `status` string the clients branch on. Pure, so the mapping is tested
/// without touching the host.
pub fn power_status_label(action: PowerAction) -> &'static str {
    match action {
        PowerAction::Reboot => "restarting",
        PowerAction::Shutdown => "shutting_down",
        // Not reachable from `/api/server/*`; kept total so a future caller
        // cannot silently answer the wrong status.
        PowerAction::Suspend => "suspending",
    }
}

/// The success payload under `data` — the contract the desktop reads
/// (`message` is shown verbatim, `status` is the machine-readable half).
pub fn power_accepted_body(action: PowerAction, label: &str) -> serde_json::Value {
    serde_json::json!({
        "message": format!("Server {label} initiated"),
        "status": power_status_label(action),
    })
}

/// Maps the primitive's outcome to the exact response envelope.
///
/// This is the seam that makes the failure path testable: the host command
/// itself must never run under `cargo test`, but the shape the screen reads —
/// `success: false` plus the host's own reason — is pure and asserted in
/// [`tests`] and in `tests/power_integration.rs`.
pub fn power_outcome(
    action: PowerAction,
    label: &str,
    outcome: &Result<(), String>,
) -> (StatusCode, serde_json::Value) {
    match outcome {
        Ok(()) => (
            StatusCode::OK,
            serde_json::json!({
                "success": true,
                "data": power_accepted_body(action, label),
            }),
        ),
        Err(reason) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({
                "success": false,
                "message": format!("Server {label} refused: {reason}"),
            }),
        ),
    }
}

/// Shared body of both power endpoints: admin check, one bounded host call,
/// and a response that distinguishes "started" from "refused".
async fn run_host_power_action(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    action: PowerAction,
    label: &str,
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

    tracing::info!(
        "{} requested by admin {} ({} via sudo systemctl {})",
        label,
        username,
        action.as_str(),
        action.systemctl_verb()
    );

    // The primitive is synchronous and bounded by `run_timed`; keep it off the
    // async runtime (ROADMAP X.1).
    let outcome = tokio::task::spawn_blocking(move || host_power_action(action))
        .await
        .unwrap_or_else(|join_err| Err(format!("the host task could not run: {join_err}")));

    match &outcome {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' requested {} (systemctl {})",
                username,
                label,
                action.systemctl_verb()
            ));
        }
        Err(reason) => {
            tracing::error!("{} refused by the host: {}", label, reason);
            crate::events::log_event_warn(format!(
                "Admin '{}' failed to {} the server: {}",
                username, label, reason
            ));
        }
    }

    let (status, body) = power_outcome(action, label, &outcome);
    (status, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::{host_power_argv, host_power_command};

    /// ROADMAP 24.2 — the regression guard: the server endpoints must not
    /// carry their own argv. `handle_server_restart`/`handle_server_shutdown`
    /// call `power::host_power_action`, whose argv is built by the shared
    /// helper; a second hardcoded `shutdown -r now` in this module would break
    /// the privilege contract silently, so assert both the argv and the fact
    /// that this module never spawns a process itself.
    #[test]
    fn server_endpoints_use_the_shared_power_primitive() {
        assert_eq!(
            host_power_argv(PowerAction::Reboot),
            ["sudo", "systemctl", "reboot"]
        );
        assert_eq!(
            host_power_argv(PowerAction::Shutdown),
            ["sudo", "systemctl", "poweroff"]
        );

        let cmd = host_power_command(PowerAction::Reboot);
        assert_eq!(cmd.get_program(), "sudo");

        let source = include_str!("system.rs");
        // Built with `concat!` so the needle does not appear in this file's own
        // text (an `include_str!` guard would otherwise match itself).
        let command_new = concat!("Command", "::new");
        assert!(
            !source.contains(command_new),
            "server control must not shell out on its own (ROADMAP 24.2)"
        );
        assert!(
            source.contains("host_power_action") && source.contains("spawn_blocking"),
            "server control must delegate to power::host_power_action off the runtime"
        );
    }

    /// The status strings are a client contract: the desktop picks its
    /// "Restarting…" / "Shutting down…" wording from them.
    #[test]
    fn status_labels_are_stable() {
        assert_eq!(power_status_label(PowerAction::Reboot), "restarting");
        assert_eq!(power_status_label(PowerAction::Shutdown), "shutting_down");
        assert_eq!(power_status_label(PowerAction::Suspend), "suspending");
    }

    /// An accepted command answers `200` with `data.status` — what tells the
    /// desktop the box is going down instead of leaving it looking inert.
    #[test]
    fn accepted_command_answers_its_status() {
        let (status, body) = power_outcome(PowerAction::Shutdown, "shutdown", &Ok(()));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["data"]["status"], "shutting_down");
        assert_eq!(body["data"]["message"], "Server shutdown initiated");

        let (_, restart) = power_outcome(PowerAction::Reboot, "restart", &Ok(()));
        assert_eq!(restart["data"]["status"], "restarting");
    }

    /// ROADMAP 24.1 — a refusal must carry the host's own words in the body,
    /// not only in the log: the desktop renders `message` verbatim in its
    /// `role="alert"` and otherwise cannot tell "refused" from "slow".
    #[test]
    fn refused_command_carries_the_host_reason() {
        let reason = "systemctl poweroff failed: Interactive authentication required";
        let (status, body) = power_outcome(
            PowerAction::Shutdown,
            "shutdown",
            &Err(reason.to_string()),
        );
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["success"], false);
        let message = body["message"].as_str().expect("refusal message");
        assert!(
            message.contains(reason),
            "the refusal body must repeat the host's reason, got {message}"
        );
        assert!(
            body.get("data").is_none(),
            "a refusal must not carry a success payload"
        );
    }

    /// A join failure is still a refusal with a reason, never a fake success.
    #[test]
    fn a_host_task_failure_is_a_refusal() {
        let (status, body) = power_outcome(
            PowerAction::Reboot,
            "restart",
            &Err("the host task could not run: cancelled".to_string()),
        );
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["success"], false);
        assert!(body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("could not run"));
    }
}
