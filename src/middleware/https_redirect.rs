//! Optional "redirect plain HTTP to HTTPS" middleware.
//!
//! Wired onto the **HTTP** listener only (see [`crate::web::run`]) when the
//! admin has enabled both HTTPS and the redirect toggle. Loopback clients are
//! exempt so the installer health check (`curl http://127.0.0.1:PORT/...`) and
//! the internal app-proxy keep working over plain HTTP.

use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// State for [`https_redirect_middleware`]: the port to redirect to.
#[derive(Clone, Copy)]
pub struct HttpsRedirect {
    pub https_port: u16,
}

pub async fn https_redirect_middleware(
    State(cfg): State<HttpsRedirect>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    // Loopback (installer health check, app-proxy) is always served as-is.
    if peer.ip().is_loopback() {
        return next.run(request).await;
    }

    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).trim().to_string())
        .filter(|h| !h.is_empty());

    let Some(host) = host else {
        // No usable Host header — serve normally rather than redirect to a
        // broken URL.
        return next.run(request).await;
    };

    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let location = if cfg.https_port == 443 {
        format!("https://{host}{path_and_query}")
    } else {
        format!("https://{host}:{}{path_and_query}", cfg.https_port)
    };

    match header::HeaderValue::from_str(&location) {
        Ok(value) => (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, value)]).into_response(),
        Err(_) => next.run(request).await,
    }
}
