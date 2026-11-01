/* =============================================================================
 * File          : auto_block.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The login-ban gate: a request that carries no session and comes from an
 *   address with a live ban is refused before any handler runs, with the
 *   "temporarily blocked" answer and a Retry-After.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::auto_block
 *
 * Usage:
 *   Registered on the /api router in src/web/mod.rs; see
 *   docs/guides/security.md.
 * =============================================================================
 */

//! Login-ban enforcement for unauthenticated API requests (QW-05).
//!
//! Scope, deliberately narrow: **only requests without a session token are
//! checked.** Two reasons, both correctness rather than performance:
//!
//! * A session that already exists was granted before the ban; refusing it here
//!   would silently turn "this address may not *sign in*" into "this address is
//!   logged out everywhere", which is a different, destructive action with its
//!   own endpoint ("revoke sessions").
//! * An authenticated admin is never the threat a login ban addresses, and a
//!   middleware that could lock the admin out of the box they are configuring is
//!   the exact foot-gun `auto_block::must_not_ban` exists to prevent.
//!
//! The public document routes (`/share/*`, the ACME challenge) are not behind
//! this router, so a banned address can still fetch a share link it was given —
//! a link is a separate credential, and the ban is about *signing in*.

use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::auto_block::{self, BanCheck};
use crate::handlers::AppState;
use crate::middleware::PeerAddr;

/// Refuse an unauthenticated request from a banned address.
pub async fn auto_block_middleware(
    State(state): State<Arc<AppState>>,
    peer: PeerAddr,
    request: Request,
    next: Next,
) -> Response {
    // A request that already carries a session is left to the auth layer.
    let has_session = request
        .headers()
        .get("x-session-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.is_empty());
    if has_session {
        return next.run(request).await;
    }

    let ip = crate::handlers::auth::client_ip(request.headers(), peer.0);
    match auto_block::check(&state, &ip).await {
        BanCheck::Allowed => {
            // The DoS row: a per-address request budget on unauthenticated
            // calls, so a flood costs a refusal instead of a database read per
            // request. Off by policy turns it into a no-op.
            if !auto_block::dos_allows(&state, &ip).await {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(serde_json::json!({
                        "success": false,
                        "error": "Too many requests from this address, please wait a minute",
                        "message": "Too many requests from this address, please wait a minute",
                    })),
                )
                    .into_response();
            }
            next.run(request).await
        }
        BanCheck::Blocked {
            expires_at,
            reason: _,
        } => {
            let message = auto_block::blocked_message(&expires_at);
            let mut response = (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "success": false,
                    "error": message,
                    "message": message,
                    "blocked_until": expires_at,
                })),
            )
                .into_response();
            // How long the caller should wait, in whole seconds, so a client can
            // back off instead of hammering a refusal.
            if let Some(seconds) = auto_block::remaining_seconds(&expires_at) {
                if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                    response.headers_mut().insert(header::RETRY_AFTER, value);
                }
            }
            response
        }
    }
}
