//! Helper middleware pour vérifier les permissions admin
//! Utilisé pour protéger les endpoints sensibles

use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::http::HeaderMap;
use axum::response::Response;
use std::sync::Arc;

/// Vérifie qu'un utilisateur est authentifié ET admin
/// Retourne Ok(username) si admin, Err(Response) sinon
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

/// Vérifie qu'un utilisateur est authentifié (admin ou user)
/// Retourne Ok(username) si authentifié, Err(Response) sinon
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
