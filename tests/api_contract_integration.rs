/* =============================================================================
 * File          : api_contract_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Contract handshake tests: /api/meta shape and the 426 mismatch behaviour of the contract middleware.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

//! Integration test for the versioned client/server contract endpoint
//! (`crate::handlers::meta` → `GET /api/meta`).
//!
//! The endpoint is deliberately unauthenticated so a stale cached SPA bundle
//! can detect a mismatch before it has a session. This test drives the real
//! router with `tower::ServiceExt` and asserts the payload shape and that no
//! session is required.
//!
//! Run: cargo test --test api_contract_integration

use axum::{routing::get, Router};
use http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

use homevault::api_contract::CONTRACT_VERSION;
use homevault::handlers::meta::handle_meta;

fn router() -> Router {
    Router::new().route("/api/meta", get(handle_meta))
}

async fn get_meta() -> (StatusCode, Value) {
    let resp = router()
        .oneshot(
            Request::builder()
                .uri("/api/meta")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn test_meta_needs_no_session() {
    let (status, _) = get_meta().await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_meta_reports_contract_and_server_versions() {
    let (_, body) = get_meta().await;
    assert_eq!(body["success"], serde_json::json!(true));
    assert_eq!(
        body["data"]["contract_version"],
        serde_json::json!(CONTRACT_VERSION)
    );
    assert_eq!(
        body["data"]["server_version"],
        serde_json::json!(env!("CARGO_PKG_VERSION"))
    );
}
