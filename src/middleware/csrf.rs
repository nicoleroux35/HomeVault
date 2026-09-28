use crate::handlers::AppState;
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

/// Axum middleware that validates CSRF tokens on state-changing requests.
///
/// This validates against the **session-tied** CSRF token
/// (`AuthService::validate_session_csrf_token`, backed by each session row's
/// own `csrf_token` column) rather than a separate, unrelated token pool —
/// this is the token the frontend actually receives (`LoginResponse.csrf_token`
/// = `session.csrf_token`, see `handle_login`/`handle_2fa_verify`/`handle_setup`)
/// and already sends back as `X-CSRF-Token` on every mutating request
/// (`ApiClient` in `frontend/src/lib/services/api.ts`). An earlier, standalone
/// `CsrfState` (its own random token pool, generated but never actually
/// issued to the frontend) existed here and was never wired into the router
/// at all — replaced entirely rather than left as unreachable dead code that
/// would have rejected every real request if it *had* been wired in, since
/// the frontend never had a matching token for that pool.
///
/// Requests with **no** `X-Session-Token` header at all are passed through
/// unchecked: every currently-public, pre-session endpoint (`/auth/login`,
/// `/auth/setup`, `/auth/forgot-password`, `/auth/reset-password`,
/// `/auth/2fa/verify`) is a `POST` with no session yet to tie a CSRF token
/// to, and every mutating endpoint that *does* require a session already
/// enforces that separately via `require_auth`/`require_admin` inside the
/// handler — this middleware only needs to add the CSRF check for the case
/// where a session token is actually present, without needing to hardcode
/// an allowlist of every public route that could drift out of sync with
/// `web/mod.rs`.
pub async fn csrf_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // CSRF is only enforced on state-changing methods.
    let method = request.method().clone();
    if method == axum::http::Method::GET
        || method == axum::http::Method::HEAD
        || method == axum::http::Method::OPTIONS
    {
        return Ok(next.run(request).await);
    }

    let headers = request.headers().clone();
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);

    // No session token at all: this is a pre-session public endpoint (login,
    // setup, forgot/reset password, 2FA verify) or a request that will be
    // rejected on its own merits by the handler's own require_auth/
    // require_admin check. Either way, there is no session to protect here.
    let Some(session_token) = session_token else {
        return Ok(next.run(request).await);
    };

    let csrf_token = headers.get("x-csrf-token").and_then(|h| h.to_str().ok());

    if let Some(token) = csrf_token {
        if state
            .auth
            .validate_session_csrf_token(&session_token, token)
            .await
        {
            return Ok(next.run(request).await);
        }
    }

    // Fall back to a CSRF token embedded in an urlencoded form body
    // (non-AJAX HTML form submissions cannot set custom headers). Nothing in
    // this SPA currently submits a real HTML form for a mutating request —
    // every mutation goes through `ApiClient` with the header above — but
    // this keeps the same defensive fallback the header-only path replaces.
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if content_type.starts_with("application/x-www-form-urlencoded") {
        let (parts, body) = request.into_parts();
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(b) => b,
            Err(_) => return Err(StatusCode::FORBIDDEN),
        };

        if let Ok(form) = std::str::from_utf8(&bytes) {
            for pair in form.split('&') {
                let mut kv = pair.splitn(2, '=');
                if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                    if k == "csrf_token" {
                        let decoded = decode_percent(v);
                        if state
                            .auth
                            .validate_session_csrf_token(&session_token, &decoded)
                            .await
                        {
                            let req = Request::from_parts(parts, axum::body::Body::from(bytes));
                            return Ok(next.run(req).await);
                        }
                    }
                }
            }
        }

        // No (valid) token found in the form body — reject.
        return Err(StatusCode::FORBIDDEN);
    }

    Err(StatusCode::FORBIDDEN)
}

/// Minimal percent-decoding for form field values (handles + and %XX).
fn decode_percent(input: &str) -> String {
    let input = input.replace('+', " ");
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8 as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
