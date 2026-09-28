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
