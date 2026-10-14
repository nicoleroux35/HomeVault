/* =============================================================================
 * File          : admin_guard.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shared permission checks for every protected endpoint: require_auth
 *   validates the caller's session token and returns the username,
 *   require_admin additionally refuses non-admin accounts. Both answer with
 *   a ready-made JSON error (401/403) when the check fails, so a handler
 *   guards itself with a single match.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   `require_auth(&state, &headers).await` / `require_admin(...)` at the
 *   top of each protected handler — see docs/API.md for the auth model of
 *   every endpoint.
 * =============================================================================
 */

//! Session permission checks shared by every protected handler.
//!
//! `require_auth` resolves and validates the caller's session token and
//! returns the username; `require_admin` additionally refuses non-admin
//! accounts. Both return a ready-to-send JSON error response on failure, so
//! a handler guards itself with a single `match`.

use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::http::HeaderMap;
use axum::response::Response;
use std::sync::Arc;

/// Ensures the caller is authenticated AND an admin.
/// Returns `Ok(username)` on success, `Err(response)` otherwise.
// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
pub async fn require_admin(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Response> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Err(ResponseHelper::json_error(
            "No session token provided".to_string(),
        ));
    }

    match state.auth.get_username_from_token(session_token).await {
        Some(username) => {
            if state.auth.is_admin(&username).await {
                Ok(username)
            } else {
                Err(ResponseHelper::json_forbidden(
                    "Admin privileges required for this operation".to_string(),
                ))
            }
        }
        None => Err(ResponseHelper::json_unauthorized(
            "Invalid or expired session".to_string(),
        )),
    }
}

/// Ensures the caller is authenticated (admin or regular user).
/// Returns `Ok(username)` on success, `Err(response)` otherwise.
// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
pub async fn require_auth(state: &Arc<AppState>, headers: &HeaderMap) -> Result<String, Response> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Err(ResponseHelper::json_error(
            "No session token provided".to_string(),
        ));
    }

    match state.auth.get_username_from_token(session_token).await {
        Some(username) => Ok(username),
        None => Err(ResponseHelper::json_unauthorized(
            "Invalid or expired session".to_string(),
        )),
    }
}
