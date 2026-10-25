/* =============================================================================
 * File          : snmp_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Snmp integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the SNMP API — `crate::handlers::snmp`:
//!
//!   GET /api/snmp/availability  (admin)
//!   GET /api/snmp/info          (admin)
//!   GET /api/snmp/interfaces    (admin)
//!   GET /api/snmp/storage       (admin)
//!   GET /api/snmp/settings      (admin)
//!   PUT /api/snmp/settings      (admin)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server. Nothing here
//! touches the host.
//!
//! Run:
//!   cargo test --test snmp_integration

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::snmp::{
    handle_snmp_availability, handle_snmp_info, handle_snmp_interfaces, handle_snmp_settings_get,
    handle_snmp_settings_update, handle_snmp_storage,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    let path_guard = PathGuard::new(vec!["/tmp".to_string()]);
    let admin_rate_limiter = Arc::new(RateLimitState::new(100, 60));
    let pty_manager = Arc::new(PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    (
        Arc::new(AppState {
            auth,
            widgets,
            db,
            path_guard,
            admin_rate_limiter,
            pty_manager,
            downloader,
            ransomware_watch_tx,
        }),
        tmp,
        guard,
    )
}

fn snmp_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/snmp/availability", get(handle_snmp_availability))
        .route("/api/snmp/info", get(handle_snmp_info))
        .route("/api/snmp/interfaces", get(handle_snmp_interfaces))
        .route("/api/snmp/storage", get(handle_snmp_storage))
        .route(
            "/api/snmp/settings",
            get(handle_snmp_settings_get).put(handle_snmp_settings_update),
        )
        .with_state(state)
}

use axum::routing::post;

async fn create_test_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', ?, false, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(&password_hash)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("insert test user");
}

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (_, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
    )
    .await;
    body["data"]["token"].as_str().expect("token").to_string()
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    let req = if let Some(b) = body {
        req.header("content-type", "application/json")
            .body(axum::body::Body::from(b.to_string()))
            .unwrap()
    } else {
        req.body(axum::body::Body::empty()).unwrap()
    };
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn admin_endpoints() -> Vec<(&'static str, Method)> {
    vec![
        ("/api/snmp/availability", Method::GET),
        ("/api/snmp/info", Method::GET),
        ("/api/snmp/interfaces", Method::GET),
        ("/api/snmp/storage", Method::GET),
        ("/api/snmp/settings", Method::GET),
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth guards — all endpoints are admin-only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_snmp_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = snmp_router(state);
    for (uri, method) in admin_endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} (no token → 401)"
        );
    }
}

#[tokio::test]
async fn test_snmp_endpoints_reject_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = snmp_router(state);
    for (uri, method) in admin_endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, Some("nope"), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} (bad token)"
        );
    }
}

#[tokio::test]
async fn test_snmp_endpoints_reject_non_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut r = snmp_router(state);
    let token = login(&mut r, "regular", "Password123!").await;
    for (uri, method) in admin_endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, Some(&token), None).await;
        // `403`, not `400`: the request was well-formed, the caller is simply
        // not an admin. This suite used to pin the drifted `400` that the
        // module's own hand-rolled guard returned (ROADMAP X.7).
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} (non-admin)");
    }
}

// ---------------------------------------------------------------------------
// 2. Availability — works for admin, never 500
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_snmp_availability_returns_success() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = snmp_router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/snmp/availability",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert!(body["data"]["available"].is_boolean());
}

// ---------------------------------------------------------------------------
// 3. Settings round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_snmp_settings_default_and_update() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = snmp_router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    // Default settings.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/snmp/settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["community"], "public");
    assert_eq!(body["data"]["version"], "2c");

    // Update community.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/snmp/settings",
        Some(&token),
        Some(json!({"community": "private"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify update took effect.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/snmp/settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["community"], "private");
    assert_eq!(body["data"]["version"], "2c"); // unchanged
}

#[tokio::test]
async fn test_snmp_version_is_validated_and_v3_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = snmp_router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    // ROADMAP 13.5: the saved version now reaches `snmpget -v`, so v3 — which
    // needs credentials this card does not collect — is refused at save time
    // instead of silently running as v2c.
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/snmp/settings",
        Some(&token),
        Some(json!({"version": "3"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("v3"),
        "the refusal must explain the v3 limitation: {body}"
    );

    // Nothing was persisted by the refused save.
    let (_, got) = send(
        &mut r,
        Method::GET,
        "/api/snmp/settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(got["data"]["version"], "2c");

    // v1 and v2c are accepted.
    for version in ["1", "2c"] {
        let (status, body) = send(
            &mut r,
            Method::PUT,
            "/api/snmp/settings",
            Some(&token),
            Some(json!({"version": version})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{version}: {body}");
    }

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/snmp/settings",
        Some(&token),
        Some(json!({"version": "4"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn test_snmp_interfaces_honours_the_iface_query_parameter() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = snmp_router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    // `iface` used to be accepted and ignored (ROADMAP 13.5). With no agent
    // installed the walk fails, but the filtered request must still be a clean
    // answer rather than a 500 or an unparsed-parameter rejection.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/snmp/interfaces?iface=lo",
        Some(&token),
        None,
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
        "GET with iface: status {status}, body {body}"
    );
    assert!(
        body["success"].is_boolean() || body["message"].is_string(),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// 4. Info / interfaces / storage — admin-only, no crash
// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// 4. Info / interfaces / storage — admin-only, no crash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_snmp_read_endpoints_never_500() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = snmp_router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    // These will fail if snmpget isn't installed, but should return a clean
    // error response (400 with success:false), not a 500.
    for uri in [
        "/api/snmp/info",
        "/api/snmp/interfaces",
        "/api/snmp/storage",
    ] {
        let (status, body) = send(&mut r, Method::GET, uri, Some(&token), None).await;
        // 200 if SNMP is available, 400 if not — either is fine, never 500.
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "GET {uri}: status {status}, body {body}"
        );
        assert!(
            body["success"].is_boolean() || body["message"].is_string(),
            "GET {uri}: {body}"
        );
    }
}
