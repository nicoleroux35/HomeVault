/* =============================================================================
 * File          : security.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Account-security endpoints: the caller's session list with revocation
 *   ("logout from other devices" and "logout everywhere except current" —
 *   the current session is always protected), the global login history
 *   (admin), per-user login history (admin or the account owner), and the
 *   system-wide active-sessions count for the admin monitor widget.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/security in src/web/mod.rs; see docs/guides/security.md.
 * =============================================================================
 */

//! Security endpoints: own-session list and revocation, login history
//! (admin or self), and the system-wide active-sessions count (admin).
//!
//! The self-service session list never returns a live session token: each
//! session is identified by an opaque SHA-256 fingerprint (`session_public_id`)
//! and revocation works exclusively on those ids, resolved against the
//! *caller's own* sessions — so a token belonging to another user can neither
//! be handed out again nor used to revoke it (CWE-639 / OWASP A01).

use crate::handlers::AppState;
use crate::storage::login_attempt_repo::LoginAttemptRepository;
use crate::storage::session_repo::SessionRepository;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Opaque public identifier for a session: a 12-character SHA-256 fingerprint
/// of its token. Stable for the session's lifetime, unlinkable to the token
/// (the id cannot be replayed as a credential) and — at 48 bits — practically
/// collision-free across a single account's handful of concurrent sessions.
/// The same digest scheme is what `src/mcp.rs` stores for its tokens.
pub fn session_public_id(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut id = String::with_capacity(12);
    for byte in digest.iter().take(6) {
        use std::fmt::Write as _;
        let _ = write!(id, "{:02x}", byte);
    }
    id
}

/// A user session with metadata. `id` is the opaque public identifier — never
/// the live token.
#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub ip_address: String,
    pub created_at: String,
    pub expires_at: String,
    pub is_current: bool,
}

/// Response wrapper for listing user sessions.
#[derive(Debug, Serialize)]
pub struct UserSessionsResponse {
    pub sessions: Vec<SessionInfo>,
    pub total: usize,
}

/// Body for revoking one or more sessions. Carries the opaque public ids
/// from `GET /security/sessions`, never live tokens.
#[derive(Debug, Deserialize)]
pub struct RevokeSessionsRequest {
    pub ids: Vec<String>,
}

/// A single login attempt record.
#[derive(Debug, Serialize)]
pub struct LoginAttemptInfo {
    pub username: String,
    pub ip_address: String,
    pub success: bool,
    pub timestamp: String,
    pub user_agent: Option<String>,
}

/// Response wrapper for login history queries.
#[derive(Debug, Serialize)]
pub struct LoginHistoryResponse {
    pub attempts: Vec<LoginAttemptInfo>,
    pub total: usize,
}

/// Get all sessions for the current user
pub async fn handle_get_my_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let session_repo = SessionRepository::new(state.db.pool());
    let sessions = match session_repo.find_all_by_username(&username).await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to fetch sessions: {}", e)),
    };

    let session_infos: Vec<SessionInfo> = sessions
        .into_iter()
        .map(|s| SessionInfo {
            id: session_public_id(&s.token),
            ip_address: s.ip_address,
            created_at: s.created_at.to_rfc3339(),
            expires_at: s.expires_at.to_rfc3339(),
            is_current: s.token == session_token,
        })
        .collect();

    let total = session_infos.len();
    ResponseHelper::json_success(UserSessionsResponse {
        sessions: session_infos,
        total,
    })
}

/// Revoke specific sessions (logout from other devices).
///
/// The request carries the opaque ids returned by `GET /security/sessions`.
/// Each id is resolved **only against the caller's own sessions**, so an id
/// or token that belongs to another account can never be turned into a
/// deletion — the cross-account revocation path from the 2026-10-22 audit is
/// closed at the resolution step, before `delete_many` sees a token.
pub async fn handle_revoke_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<RevokeSessionsRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    if req.ids.is_empty() {
        return ResponseHelper::json_error("No sessions to revoke".to_string());
    }

    let session_repo = SessionRepository::new(state.db.pool());

    // The candidate set is the caller's own sessions — never the request's
    // ids mapped to arbitrary rows. The current session stays protected.
    let own_sessions = match session_repo.find_all_by_username(&username).await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to fetch sessions: {}", e)),
    };

    let tokens_to_revoke: Vec<String> = own_sessions
        .iter()
        .filter(|s| s.token != session_token)
        .filter(|s| {
            let public_id = session_public_id(&s.token);
            req.ids.iter().any(|id| id == &public_id)
        })
        .map(|s| s.token.clone())
        .collect();

    if tokens_to_revoke.is_empty() {
        return ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            "No matching session to revoke for your account".to_string(),
        );
    }

    match session_repo.delete_many(&tokens_to_revoke).await {
        Ok(deleted) => {
            tracing::info!(username = %username, count = deleted, "Sessions revoked");
            ResponseHelper::json_success(serde_json::json!({
                "revoked": deleted
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke sessions: {}", e)),
    }
}

/// Revoke all other sessions (logout everywhere except current)
pub async fn handle_revoke_all_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let session_repo = SessionRepository::new(state.db.pool());

    // Get all sessions except current
    let all_sessions = match session_repo.find_all_by_username(&username).await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to fetch sessions: {}", e)),
    };

    let tokens_to_revoke: Vec<String> = all_sessions
        .into_iter()
        .filter(|s| s.token != session_token)
        .map(|s| s.token)
        .collect();

    if tokens_to_revoke.is_empty() {
        return ResponseHelper::json_success(serde_json::json!({
            "revoked": 0
        }));
    }

    match session_repo.delete_many(&tokens_to_revoke).await {
        Ok(deleted) => {
            tracing::info!(username = %username, count = deleted, "All other sessions revoked");
            ResponseHelper::json_success(serde_json::json!({
                "revoked": deleted
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke sessions: {}", e)),
    }
}

/// Get login history (admin only)
pub async fn handle_get_login_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    // Verify admin
    let _admin_user =
        match crate::handlers::auth::require_admin(State(state.clone()), headers.clone()).await {
            Ok(u) => u,
            Err(resp) => return resp,
        };

    let attempt_repo = LoginAttemptRepository::new(state.db.pool());
    let attempts = match attempt_repo.get_all_recent(100).await {
        Ok(a) => a,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to fetch login history: {}", e))
        }
    };

    let attempt_infos: Vec<LoginAttemptInfo> = attempts
        .into_iter()
        .map(|a| LoginAttemptInfo {
            username: a.username,
            ip_address: a.ip_address,
            success: a.success,
            timestamp: a.attempted_at,
            user_agent: a.user_agent,
        })
        .collect();

    let total = attempt_infos.len();
    ResponseHelper::json_success(LoginHistoryResponse {
        attempts: attempt_infos,
        total,
    })
}

/// Response for the active-sessions-count widget.
#[derive(Debug, Serialize)]
pub struct ActiveSessionsCountResponse {
    pub count: i64,
}

/// Admin-only: number of distinct users with a currently valid session
/// system-wide (not just the caller's own sessions), for the desktop
/// "Active Sessions" monitor widget.
pub async fn handle_get_active_sessions_count(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if let Err(resp) =
        crate::handlers::auth::require_admin(State(state.clone()), headers.clone()).await
    {
        return resp;
    }

    let session_repo = SessionRepository::new(state.db.pool());
    match session_repo.count_distinct_active_users().await {
        Ok(count) => ResponseHelper::json_success(ActiveSessionsCountResponse { count }),
        Err(e) => ResponseHelper::json_error(format!("Failed to count active sessions: {}", e)),
    }
}

/// Get login history for a specific user
pub async fn handle_get_user_login_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(username): axum::extract::Path<String>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    // Verify admin or own user
    let requesting_user = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    let is_admin = state.auth.is_admin(&requesting_user).await;

    // Non-admins can only view their own history
    if !is_admin && requesting_user != username {
        return ResponseHelper::json_forbidden("Access denied".to_string());
    }

    let attempt_repo = LoginAttemptRepository::new(state.db.pool());
    let attempts = match attempt_repo.get_recent_attempts(&username, 50).await {
        Ok(a) => a,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to fetch login history: {}", e))
        }
    };

    let attempt_infos: Vec<LoginAttemptInfo> = attempts
        .into_iter()
        .map(|a| LoginAttemptInfo {
            username: a.username,
            ip_address: a.ip_address,
            success: a.success,
            timestamp: a.attempted_at,
            user_agent: a.user_agent,
        })
        .collect();

    let total = attempt_infos.len();
    ResponseHelper::json_success(LoginHistoryResponse {
        attempts: attempt_infos,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::session_public_id;

    #[test]
    fn test_session_public_id_is_deterministic() {
        let token = "b3f1a0c2d4e5f60718293a4b5c6d7e8f";
        assert_eq!(session_public_id(token), session_public_id(token));
    }

    #[test]
    fn test_session_public_id_is_a_short_hex_fingerprint() {
        let id = session_public_id("some-live-session-token-value");
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // The id must not embed the token in any readable form.
        assert!(!id.contains("some-live-session-token-value"));
    }

    #[test]
    fn test_session_public_id_differs_for_different_tokens() {
        assert_ne!(
            session_public_id("token-aaaaaaaaaaaaaaaa"),
            session_public_id("token-bbbbbbbbbbbbbbbb")
        );
    }

    #[test]
    fn test_session_public_id_differs_on_a_single_character_change() {
        assert_ne!(
            session_public_id("0123456789abcdef0123456789abcdef"),
            session_public_id("0123456789abcdef0123456789abcdee")
        );
    }
}
