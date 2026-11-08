/* =============================================================================
 * File          : explorer_access.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Application-level gate for the File Explorer. A member whose
 *   `app:explorer` grant is `deny` is refused every `/api/explorer/...` call,
 *   including the ones that do not take a path (disks, transfers, trash
 *   listing, saved favorites). This is DSM's "Application Privileges" layer:
 *   the per-path grants decide *what* a member may reach, this decides
 *   whether the app itself is available.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Layered onto the API router in src/web/mod.rs. Requests without a session
 *   token pass through so the handler keeps answering its own `401`; admins
 *   and members with no `app:explorer` row are allowed (backward compatible).
 * =============================================================================
 */

//! Application-level Explorer gate (DSM's "Application Privileges").

use crate::handlers::admin_guard::SESSION_HEADER;
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

/// Stable machine code returned with the `403`, so the desktop can hide the
/// Explorer tile instead of showing a generic permission error.
pub const EXPLORER_ACCESS_DENIED: &str = "explorer_access_denied";

/// The paths this layer covers. The trailing slash is deliberate: it keeps a
/// future `/api/explorerx` route from being caught by a bare prefix match.
const EXPLORER_PREFIX: &str = "/api/explorer/";

/// Refuses every Explorer request from a member whose app grant is `deny`.
pub async fn explorer_access_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if !req.uri().path().starts_with(EXPLORER_PREFIX) {
        return next.run(req).await;
    }

    let token = req
        .headers()
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    // No token: the handler's own guard owns the `401` contract. An invalid
    // token takes the same path, so this layer never becomes a second source
    // of truth for "who is signed in".
    let Ok(session) = state.auth.get_session(token).await else {
        return next.run(req).await;
    };

    if !crate::handlers::resource_shares::explorer_app_allowed(&state, &session.username).await {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "success": false,
                "error": "The file explorer is not available for this account",
                "code": EXPLORER_ACCESS_DENIED,
            })),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_covers_every_explorer_route() {
        for path in [
            "/api/explorer/list",
            "/api/explorer/trash/purge-now",
            "/api/explorer/upload/complete",
            "/api/explorer/transfers/history",
        ] {
            assert!(path.starts_with(EXPLORER_PREFIX), "{path}");
        }
        // A different app must not be caught by the prefix.
        assert!(!"/api/shares".starts_with(EXPLORER_PREFIX));
        assert!(!"/api/explorerx".starts_with(EXPLORER_PREFIX));
    }
}
