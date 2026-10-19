/* =============================================================================
 * File          : spa_assets_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 12/10/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Embedded SPA serving tests: asset lookup, ETag/Cache-Control policies and pre-compressed variants.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

//! Integration coverage for the embedded static-asset routes
//! (`src/handlers/spa.rs`), over a real Axum router and `tower::oneshot`.
//!
//! The interesting behaviour is not "does it 200" but the HTTP contract the
//! frontend's performance depends on:
//!
//! * every embedded file carries an `ETag`, and a second conditional request
//!   gets a `304` instead of the whole body (before this, a browser could not
//!   revalidate at all, so `/wallpapers/*.webp` — several MB — were
//!   re-downloaded on every visit);
//! * the paths whose content never changes within a release
//!   (`/static/*`, `/wallpapers/*`) are pinned with `immutable`, while the
//!   generated icon sprite gets a long-but-revalidated lifetime;
//! * a pre-compressed `.br` / `.gz` sibling emitted by
//!   `frontend/scripts/precompress-dist.mjs` is what actually gets served, and
//!   the response is `Vary: accept-encoding`.
//!
//! `static/sounds/README.md` is committed, so the route assertions hold even
//! when `static/dist` has never been built (which is the case in the
//! integration CI job — it runs `cargo test` with no frontend build). The
//! sprite/wallpaper assertions therefore state their build prerequisite
//! explicitly and no-op when it is absent.
//!
//! Run:
//!   cargo test --test spa_assets_integration

use axum::{
    body::{to_bytes, Body},
    routing::get,
    Router,
};
use http::{header, HeaderMap, Request, StatusCode};
use tower::ServiceExt;

use homevault::handlers::spa;

/// The SPA router as `create_app` builds it, minus the API/middleware layers
/// this suite does not exercise.
fn router() -> Router {
    Router::new()
        .route("/static/{*path}", get(spa::serve_static_file))
        .route("/sprite-unified.svg", get(spa::serve_sprite_unified))
        .route("/wallpapers/{*path}", get(spa::serve_wallpapers_file))
        .route(
            "/manifest.json",
            get(|headers: HeaderMap| spa::serve_dist_file("manifest.json", headers)),
        )
}

async fn fetch(path: &str, extra: &[(&str, &str)]) -> http::Response<Body> {
    let mut builder = Request::builder().uri(path).method("GET");
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    router()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("router should not fail")
}

/// Header value as `&str`, if present.
fn header_of(res: &http::Response<Body>, name: header::HeaderName) -> Option<&str> {
    res.headers().get(name).and_then(|v| v.to_str().ok())
}

// =========================================================================
// /static/* — a committed file, so this always runs
// =========================================================================

#[tokio::test]
async fn committed_static_file_is_served_with_a_validator_and_immutable_cache() {
    let res = fetch("/static/sounds/README.md", &[]).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Without an ETag a browser can only ever re-download the body.
    let etag = header_of(&res, header::ETAG).expect("ETag header");
    assert!(etag.starts_with('"') && etag.ends_with('"'), "etag: {etag}");

    // Committed, release-scoped content: safe to pin for a year.
    assert_eq!(
        header_of(&res, header::CACHE_CONTROL),
        Some("public, max-age=31536000, immutable")
    );
    assert!(header_of(&res, header::CONTENT_TYPE)
        .expect("content-type")
        .starts_with("text/markdown"));

    let body = to_bytes(res.into_body(), usize::MAX).await.expect("body");
    assert!(!body.is_empty());
}

#[tokio::test]
async fn conditional_request_returns_304_with_no_body() {
    let first = fetch("/static/sounds/README.md", &[]).await;
    assert_eq!(first.status(), StatusCode::OK);
    let etag = header_of(&first, header::ETAG)
        .expect("ETag header")
        .to_string();

    let second = fetch("/static/sounds/README.md", &[("if-none-match", &etag)]).await;
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    // A 304 describes the stored representation: the validator and the
    // freshness lifetime must still come back, the payload must not.
    assert_eq!(header_of(&second, header::ETAG), Some(etag.as_str()));
    assert_eq!(
        header_of(&second, header::CACHE_CONTROL),
        Some("public, max-age=31536000, immutable")
    );
    let body = to_bytes(second.into_body(), usize::MAX)
        .await
        .expect("body");
    assert!(body.is_empty(), "304 must not carry a body");
}

#[tokio::test]
async fn a_stale_validator_still_gets_the_full_body() {
    let res = fetch(
        "/static/sounds/README.md",
        &[("if-none-match", "\"not-the-current-one\"")],
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.expect("body");
    assert!(!body.is_empty());
}

#[tokio::test]
async fn unknown_static_path_is_404_not_the_spa_fallback() {
    // The SPA fallback would answer 200 with index.html for any path; the
    // asset routes must not, or a typo'd bundle URL would "succeed" with HTML
    // and the browser would fail to execute it.
    let res = fetch("/static/sounds/does-not-exist.md", &[]).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// =========================================================================
// /sprite-unified.svg, /wallpapers/*, /manifest.json — need `static/dist`
// =========================================================================

#[tokio::test]
async fn icon_sprite_is_revalidated_rather_than_immutable() {
    let res = fetch("/sprite-unified.svg", &[]).await;
    if res.status() == StatusCode::NOT_FOUND {
        // No frontend build in this checkout (`static/dist` is gitignored).
        return;
    }
    assert_eq!(res.status(), StatusCode::OK);
    assert!(header_of(&res, header::ETAG).is_some());

    // The sprite is regenerated from the sources on every build, so a year of
    // `immutable` would pin a stale copy (and therefore blank new icons) in
    // every browser. It must stay revalidated.
    assert_eq!(
        header_of(&res, header::CACHE_CONTROL),
        Some("public, max-age=86400")
    );
}

#[tokio::test]
async fn precompressed_sibling_is_served_instead_of_compressing_on_the_fly() {
    let identity = fetch("/sprite-unified.svg", &[]).await;
    if identity.status() == StatusCode::NOT_FOUND {
        return; // no frontend build — see the module docs
    }
    // `npm run build` runs the pre-compressor, so the sibling must exist.
    assert_eq!(
        header_of(&identity, header::CONTENT_ENCODING),
        None,
        "a request without Accept-Encoding must get the identity bytes"
    );

    let gzipped = fetch("/sprite-unified.svg", &[("accept-encoding", "gzip")]).await;
    assert_eq!(gzipped.status(), StatusCode::OK);
    assert_eq!(header_of(&gzipped, header::CONTENT_ENCODING), Some("gzip"));
    // `Vary` is what stops a shared cache from handing the compressed bytes to
    // a client that never asked for them.
    assert_eq!(header_of(&gzipped, header::VARY), Some("accept-encoding"));

    // Distinct validators per coding: the stored identity body must not
    // validate against the compressed request.
    assert_ne!(
        header_of(&identity, header::ETAG),
        header_of(&gzipped, header::ETAG)
    );

    let identity_len = to_bytes(identity.into_body(), usize::MAX)
        .await
        .expect("body")
        .len();
    let gzipped_len = to_bytes(gzipped.into_body(), usize::MAX)
        .await
        .expect("body")
        .len();
    assert!(
        gzipped_len < identity_len,
        "gzip variant ({gzipped_len}) should be smaller than identity ({identity_len})"
    );
}

#[tokio::test]
async fn brotli_is_preferred_when_the_client_offers_it() {
    let identity = fetch("/sprite-unified.svg", &[]).await;
    if identity.status() == StatusCode::NOT_FOUND {
        return;
    }

    let brotli = fetch("/sprite-unified.svg", &[("accept-encoding", "gzip, br")]).await;
    assert_eq!(brotli.status(), StatusCode::OK);
    assert_eq!(header_of(&brotli, header::CONTENT_ENCODING), Some("br"));
}

#[tokio::test]
async fn bundled_wallpapers_are_immutable_and_support_conditional_get() {
    let res = fetch("/wallpapers/hv-01.webp", &[]).await;
    if res.status() == StatusCode::NOT_FOUND {
        return; // no frontend build — see the module docs
    }
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        header_of(&res, header::CACHE_CONTROL),
        Some("public, max-age=31536000, immutable")
    );
    let etag = header_of(&res, header::ETAG).expect("ETag").to_string();

    let second = fetch("/wallpapers/hv-01.webp", &[("if-none-match", &etag)]).await;
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn dist_files_get_a_validator_but_no_pinning_cache_control() {
    let res = fetch("/manifest.json", &[]).await;
    if res.status() == StatusCode::NOT_FOUND {
        return; // no frontend build — see the module docs
    }
    assert_eq!(res.status(), StatusCode::OK);
    assert!(header_of(&res, header::ETAG).is_some());
    // `sw.js` and friends must be allowed to update; pinning them for a year
    // would strand clients on a previous release's service worker.
    assert_eq!(header_of(&res, header::CACHE_CONTROL), None);
}

// =========================================================================
// CSP nonce (2026-10-17)
// =========================================================================
//
// The policy used to allow `script-src 'unsafe-inline'`. Combined with the
// session token in `localStorage`, any injected script could have exfiltrated a
// full session. It now lists a per-response nonce instead, and the single
// inline boot script in `index.html` carries that exact value — so the two must
// agree, in the same response, or the shell never boots.

mod csp_nonce {
    use super::*;
    use homevault::middleware::{
        security_headers_middleware, stamp_nonce, CspNonce, NONCE_PLACEHOLDER,
    };

    /// The SPA route behind the real security-headers layer, i.e. the shape
    /// `create_app` builds.
    fn hardened_router() -> Router {
        Router::new()
            .route("/", get(spa::serve_spa))
            .layer(axum::middleware::from_fn(security_headers_middleware))
    }

    async fn get_root() -> (HeaderMap, String) {
        let res = hardened_router()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .expect("router");
        let headers = res.headers().clone();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.expect("body");
        (headers, String::from_utf8_lossy(&bytes).to_string())
    }

    /// `nonce-…` token out of the CSP header.
    fn header_nonce(headers: &HeaderMap) -> Option<String> {
        let csp = headers
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())?;
        let start = csp.find("'nonce-")? + "'nonce-".len();
        let rest = &csp[start..];
        let end = rest.find('\'')?;
        Some(rest[..end].to_string())
    }

    #[tokio::test]
    async fn csp_no_longer_allows_unsafe_inline_scripts() {
        let (headers, _) = get_root().await;
        let csp = headers
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .expect("CSP header");

        // `unsafe-inline` may still appear in style-src (Svelte scoped styles +
        // style attributes); it must not appear in the script directive.
        let script_src = csp
            .split(';')
            .find(|d| d.trim_start().starts_with("script-src"))
            .expect("a script-src directive");
        assert!(
            !script_src.contains("unsafe-inline"),
            "script-src must not allow inline scripts: {script_src:?}"
        );
        assert!(
            script_src.contains("'nonce-"),
            "script-src must carry a nonce: {script_src:?}"
        );
        assert!(
            script_src.contains("'self'"),
            "bundles still load: {script_src:?}"
        );
    }

    #[tokio::test]
    async fn the_header_nonce_matches_the_stamped_html() {
        let (headers, body) = get_root().await;
        if !body.contains("id=\"app\"") {
            return; // no frontend build in this job — see the module docs
        }

        let nonce = header_nonce(&headers).expect("nonce in the CSP header");
        assert!(!nonce.is_empty());
        assert!(
            body.contains(&format!("nonce=\"{nonce}\"")),
            "the inline script must carry the very nonce the policy lists"
        );
        assert!(
            !body.contains(NONCE_PLACEHOLDER),
            "the placeholder must never reach the browser"
        );
    }

    #[tokio::test]
    async fn every_response_gets_a_fresh_nonce() {
        let (a, _) = get_root().await;
        let (b, _) = get_root().await;
        let na = header_nonce(&a).expect("nonce a");
        let nb = header_nonce(&b).expect("nonce b");
        assert_ne!(na, nb, "a reused nonce is as good as unsafe-inline");
    }

    #[test]
    fn stamping_replaces_the_placeholder_and_leaves_other_markup_alone() {
        let html = r#"<div id="app"></div><script nonce="__HV_CSP_NONCE__">boot()</script>"#;
        let out = stamp_nonce(html, "deadbeef");
        assert!(out.contains(r#"nonce="deadbeef""#));
        assert!(out.contains(r#"<div id="app"></div>"#));
        assert!(!out.contains(NONCE_PLACEHOLDER));
    }

    #[test]
    fn a_nonce_is_128_bits_of_hex() {
        let n = CspNonce::generate();
        assert_eq!(n.as_str().len(), 32);
        assert!(n.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }
}
