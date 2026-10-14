/* =============================================================================
 * File          : logging.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Middleware that logs every HTTP request (method, URI and response
 *   status) under a tracing span, so individual requests can be followed
 *   in the server logs.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - tracing (>= 0.1)
 *
 * Usage:
 *   Available via middleware::logging_middleware but not currently
 *   registered on any router — HTTP request tracing is handled by
 *   tower-http's TraceLayer in src/web/mod.rs.
 * =============================================================================
 */

use axum::{extract::Request, middleware::Next, response::Response};
use tracing::{info, span, Level};

/// Log each HTTP request and its response status.
pub async fn logging_middleware(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let uri = request.uri().to_string();

    let span = span!(Level::INFO, "request", method = %method, uri = %uri);
    let _enter = span.enter();

    let response = next.run(request).await;

    let status = response.status().as_u16();
    info!("Response: {} {} - {}", method, uri, status);

    response
}
