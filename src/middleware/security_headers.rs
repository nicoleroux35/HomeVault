use axum::{
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};

/// Marker inserted by the HTTPS listener's outermost layer (see
/// `crate::web::run`) so this middleware can tell a request that actually
/// arrived over TLS from one on the plain-HTTP listener.
#[derive(Clone, Copy)]
pub struct ArrivedOverTls;

/// Inject security-related HTTP headers into every response (CSP, HSTS, etc.).
pub async fn security_headers_middleware(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let over_tls = request.extensions().get::<ArrivedOverTls>().is_some();
    let mut response = next.run(request).await;

    let headers = response.headers_mut();

    // CSP: restrict sources to reduce XSS risk.
    // - script/style 'unsafe-inline': the bundled SPA ships inline boot code
    //   and Svelte scoped styles; tightening to nonces/hashes is tracked
    //   separately.
    // - connect-src is an explicit allow-list: 'self' + the app's own
    //   WebSocket (terminal/SSE) + the two third parties the browser talks
    //   to directly and deliberately (Open-Meteo for the Weather widget,
    //   ipwho.is for the best-effort server-location row). Any new outbound
    //   host must be added here on purpose — see CLAUDE.md's compliance rule.
    // - frame-src http/https: the Docker app-proxy "Open" feature embeds
    //   container web UIs.
    // - frame-ancestors 'none': nobody may embed HomeVault (clickjacking).
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; \
             script-src 'self' 'unsafe-inline'; \
             style-src 'self' 'unsafe-inline'; \
             font-src 'self'; \
             img-src 'self' data: blob: https:; \
             connect-src 'self' ws: wss: https://api.open-meteo.com https://geocoding-api.open-meteo.com https://ipwho.is; \
             frame-src 'self' http: https:; \
             base-uri 'self'; \
             form-action 'self'; \
             frame-ancestors 'none';"
        ),
    );

    // HSTS: instruct browsers to always use HTTPS — but only once this instance
    // actually serves HTTPS. A browser ignores HSTS received over plain HTTP,
    // so sending it unconditionally was harmless but pointless; worse, if TLS
    // is later turned off a stale one-year pin would lock users out. `preload`
    // is deliberately omitted (it implies submitting the domain to the browser
    // preload list, which the operator — not HomeVault — must decide). A
    // browser ignores HSTS received over plain HTTP anyway, but there is no
    // reason to send it there.
    if over_tls {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }

    // Disallow embedding in iframes (clickjacking protection).
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );

    // Prevent MIME-type sniffing (reduce drive-by download risks).
    headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );

    // Enable browser XSS filter (legacy; CSP is the primary defence).
    headers.insert(
        header::HeaderName::from_static("x-xss-protection"),
        HeaderValue::from_static("1; mode=block"),
    );

    // Only send the origin as Referer for cross-origin requests.
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    // Deny access to sensitive device APIs (camera, mic, geolocation, etc.).
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(
            "accelerometer=(), camera=(), geolocation=(), gyroscope=(), \
             magnetometer=(), microphone=(), payment=(), usb=()"
        ),
    );

    Ok(response)
}
