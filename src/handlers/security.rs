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

use crate::handlers::AppState;
use crate::storage::login_attempt_repo::LoginAttemptRepository;
use crate::storage::session_repo::SessionRepository;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A user session with metadata.
#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub token: String,
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

/// Body for revoking one or more sessions.
#[derive(Debug, Deserialize)]
pub struct RevokeSessionsRequest {
    pub tokens: Vec<String>,
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
        .map(|s| {
            let token = s.token.clone();
            SessionInfo {
                token,
                ip_address: s.ip_address,
                created_at: s.created_at.to_rfc3339(),
                expires_at: s.expires_at.to_rfc3339(),
                is_current: s.token == session_token,
            }
        })
        .collect();

    let total = session_infos.len();
    ResponseHelper::json_success(UserSessionsResponse {
        sessions: session_infos,
        total,
    })
}

/// Revoke specific sessions (logout from other devices)
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

    // Prevent revoking current session
    let tokens_to_revoke: Vec<String> = req
        .tokens
        .iter()
        .filter(|t| *t != session_token)
        .cloned()
        .collect();

    if tokens_to_revoke.is_empty() {
        return ResponseHelper::json_error("No sessions to revoke".to_string());
    }

    let session_repo = SessionRepository::new(state.db.pool());
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
