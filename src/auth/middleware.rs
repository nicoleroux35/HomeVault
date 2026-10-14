/* =============================================================================
 * File          : middleware.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Auth middleware: every request passing through it must carry a valid
 *   X-Session-Token header; anything missing or expired is rejected with
 *   401 before reaching the handler.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Re-exported by auth/mod.rs but not currently registered on any router —
 *   handlers authenticate individually through require_auth / require_admin
 *   (src/handlers/admin_guard.rs).
 * =============================================================================
 */

//! Session-token validation middleware (not currently wired — see Usage).

use super::service::AuthService;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Axum middleware that validates the `X-Session-Token` header.
/// Returns 401 Unauthorized if the token is missing or invalid.
pub async fn auth_middleware(
    State(auth_service): State<Arc<AuthService>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = request.headers();

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    if !auth_service.validate_session_token(session_token).await {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(request).await)
}
