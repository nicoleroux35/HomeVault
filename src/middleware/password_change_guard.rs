/* =============================================================================
 * File          : password_change_guard.rs
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Middleware that limits a session whose account still holds an
 *   admin-chosen password (users.must_change_password = 1: set on account
 *   creation and on an admin reset) to the password-change flow. The
 *   frontend shows the forced-change screen, but the UI is not the boundary:
 *   every API request carrying such a session is rejected with 403
 *   `password_change_required` unless it is one of the three calls the
 *   change screen needs — whoami, self password change, logout.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Layered onto the API router in src/web/mod.rs (outermost of the
 *   session-aware layers). Login/2FA-verify/setup pass through unchecked —
 *   they carry no session token yet, and this middleware only inspects
 *   requests that do.
 * =============================================================================
 */

//! First-sign-in password change enforcement (see the module-level docs above).

use crate::handlers::AppState;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::Arc;

/// The JSON error code a blocked request carries, so the frontend can
/// distinguish this situation from a plain authorization failure.
pub const PASSWORD_CHANGE_REQUIRED: &str = "password_change_required";

/// Paths a flagged session may still use, besides the self password change
/// (checked with its username below). Everything else is refused. The two
/// GETs are public by design (no authentication needed to call them) — the
/// gate's boot sequence (`appGate.refresh`) and the login card's footer both
/// call them, and a 403 would break the reload path to the change screen.
const ALLOWED_PATHS: [&str; 3] = ["/api/auth/logout", "/api/auth/me", "/api/auth/setup-status"];
const ALLOWED_PATH_PREFIXES: [&str; 1] = ["/api/settings/public"];

/// Pure path decision: `PUT /api/users/{username}/password` scores only when
/// `{username}` is the flagged session's own username.
pub fn is_self_password_change(method: &str, path: &str, username: &str) -> bool {
    method == "PUT"
        && path
            .strip_prefix("/api/users/")
            .and_then(|rest| rest.strip_suffix("/password"))
            .is_some_and(|target| target == username)
}

fn blocked_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "success": false,
            "code": PASSWORD_CHANGE_REQUIRED,
            "message": "Password change required before using this service",
        })),
    )
        .into_response()
}

/// Axum middleware enforcing `users.must_change_password` server-side.
///
/// Requests without a session token pass through (public pre-session
/// endpoints; the login flow itself). A valid session whose account row
/// carries the flag may only reach the allowlist above plus
/// `PUT /api/users/{own username}/password`; anything else gets 403 and is
/// logged — the block is a security event. A PAM/system account (session
/// without a users row) is never flagged.
pub async fn password_change_guard_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().as_str().to_string();

    let session_token = request
        .headers()
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();

    // No token → not our business (public endpoints; also login/2FA verify).
    if session_token.is_empty() {
        return next.run(request).await;
    }

    let session = match state.auth.get_session(&session_token).await {
        Ok(session) => session,
        Err(_) => return next.run(request).await,
    };

    // Always allowed: authenticate-check, session teardown, and the public
    // reads the gate/login shell uses.
    if ALLOWED_PATHS.contains(&path.as_str())
        || ALLOWED_PATH_PREFIXES.iter().any(|p| path.starts_with(p))
    {
        return next.run(request).await;
    }
    // SELF password change only: the flagged session's one mutation. A
    // flagged *admin* session must not be able to reset somebody else's
    // password while skipping its own obligation.
    if is_self_password_change(&method, &path, &session.username) {
        return next.run(request).await;
    }

    match state.auth.get_user(&session.username).await {
        Ok(user) if user.must_change_password => {
            crate::events::log_event_warn(format!(
                "Blocked {} {} for user '{}': password change required",
                method, path, session.username
            ));
            blocked_response()
        }
        // No flag, or no users row (PAM/system account) → normal traffic.
        _ => next.run(request).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_paths_open_logout_and_me() {
        assert!(ALLOWED_PATHS.contains(&"/api/auth/logout"));
        assert!(ALLOWED_PATHS.contains(&"/api/auth/me"));
        // The gate's boot read: a 403 here would kill the reload path to the
        // change screen (see the 2026-10-20 E2E session).
        assert!(ALLOWED_PATHS.contains(&"/api/auth/setup-status"));
        assert!(!ALLOWED_PATHS.contains(&"/api/users"));
    }

    #[test]
    fn test_self_password_change_matches_own_username() {
        assert!(is_self_password_change(
            "PUT",
            "/api/users/jean/password",
            "jean"
        ));
    }

    #[test]
    fn test_password_change_for_another_username_is_not_self() {
        assert!(!is_self_password_change(
            "PUT",
            "/api/users/admin/password",
            "jean"
        ));
    }

    #[test]
    fn test_password_change_path_matches_solidly() {
        // Unicode or slash-bearing usernames must not widen the match.
        assert!(!is_self_password_change(
            "PUT",
            "/api/users/x/password/extra/password",
            "x"
        ));
        assert!(!is_self_password_change(
            "GET",
            "/api/users/jean/password",
            "jean"
        ));
    }

    #[test]
    fn test_arbitrary_paths_are_not_self_change() {
        assert!(!is_self_password_change("GET", "/api/users", "jean"));
        assert!(!is_self_password_change(
            "POST",
            "/api/auth/2fa/setup",
            "jean"
        ));
    }
}
