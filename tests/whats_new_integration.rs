/* =============================================================================
 * File          : whats_new_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   What's New endpoint tests: payload shape and per-user acknowledgement.
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
//! Integration tests for the What's New API — `crate::handlers::whats_new`:
//!
//!   GET  /api/whats-new        (release highlights + per-user seen flag)
//!   POST /api/whats-new/seen   (per-user acknowledgement)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! Run:
//!   cargo test --test whats_new_integration

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::whats_new::{handle_whats_new, handle_whats_new_seen};
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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/whats-new", get(handle_whats_new))
        .route("/api/whats-new/seen", post(handle_whats_new_seen))
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&password_hash)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("insert user");
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

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_whats_new_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let (status, _) = send(&mut r, Method::GET, "/api/whats-new", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&mut r, Method::GET, "/api/whats-new", Some("nope"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_whats_new_seen_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let (status, _) = send(&mut r, Method::POST, "/api/whats-new/seen", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 2. Payload + acknowledgement lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_whats_new_starts_unseen_with_highlights() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "u", "Password123!", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(&mut r, Method::GET, "/api/whats-new", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["seen"], json!(false));
    assert_eq!(body["data"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(!body["data"]["title"].as_str().unwrap().is_empty());
    assert!(!body["data"]["highlights"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_whats_new_seen_is_per_user_and_idempotent() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Password123!", 1).await;
    create_user(&state, "bob", "Password123!", 1).await;
    let mut r = router(state);
    let alice = login(&mut r, "alice", "Password123!").await;
    let bob = login(&mut r, "bob", "Password123!").await;

    // Alice acknowledges; Bob is unaffected.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/whats-new/seen",
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["seen"], json!(true));

    let (_, body) = send(&mut r, Method::GET, "/api/whats-new", Some(&alice), None).await;
    assert_eq!(body["data"]["seen"], json!(true));

    let (_, body) = send(&mut r, Method::GET, "/api/whats-new", Some(&bob), None).await;
    assert_eq!(body["data"]["seen"], json!(false));

    // A second acknowledgement is idempotent.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/whats-new/seen",
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
