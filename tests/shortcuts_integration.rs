/* =============================================================================
 * File          : shortcuts_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shortcuts integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the web-shortcut metadata endpoint —
//! `crate::handlers::shortcuts`:
//!
//!   POST /api/shortcuts/fetch-metadata  (auth, SSRF-guarded)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing. No test performs a real
//! outbound request: the SSRF cases are rejected before any socket is opened,
//! and the auth case never reaches the fetch.
//!
//! Run:
//!   cargo test --test shortcuts_integration

use axum::{routing::post, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::shortcuts::handle_fetch_metadata;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
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
        homevault::handlers::downloader::Aria2Process { child: None },
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

fn shortcuts_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/shortcuts/fetch-metadata", post(handle_fetch_metadata))
        .with_state(state)
}

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

#[tokio::test]
async fn test_fetch_metadata_requires_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = shortcuts_router(state);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shortcuts/fetch-metadata",
        None,
        Some(json!({"url": "https://example.com"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shortcuts/fetch-metadata",
        Some("nope"),
        Some(json!({"url": "https://example.com"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_fetch_metadata_rejects_non_http_scheme() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    let mut r = shortcuts_router(state);
    let token = login(&mut r, "admin", "pw12345678").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shortcuts/fetch-metadata",
        Some(&token),
        Some(json!({"url": "file:///etc/passwd"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
        .contains("http"));
}

#[tokio::test]
async fn test_fetch_metadata_rejects_loopback_and_private_literals() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    let mut r = shortcuts_router(state);
    let token = login(&mut r, "admin", "pw12345678").await;

    for url in [
        "http://127.0.0.1:8080/",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
        "http://[::1]/",
    ] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/shortcuts/fetch-metadata",
            Some(&token),
            Some(json!({ "url": url })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url} must be refused");
        assert!(
            body["message"].as_str().unwrap_or_default().to_lowercase().contains("private")
                || body["message"].as_str().unwrap_or_default().to_lowercase().contains("local"),
            "{url}: unexpected error {:?}",
            body["message"]
        );
    }
}
