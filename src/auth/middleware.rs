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
