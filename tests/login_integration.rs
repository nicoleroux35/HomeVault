/* =============================================================================
 * File          : login_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Login integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Login / Auth API endpoints.
//!
//! Each test creates a fresh in-memory SQLite database, runs all embedded
//! migrations, builds a minimal Axum router with the auth handlers, and
//! uses `tower::ServiceExt` for HTTP-level testing without starting a
//! real server.
//!
//! Run:
//!   cargo test --test login_integration
//!   cargo test --test login_integration -- --nocapture

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::{handle_2fa_verify, handle_get_current_user, handle_login, handle_logout},
    security::handle_get_login_history,
    AppState,
};
use homevault::middleware::rate_limit::{rate_limit_middleware, RateLimitState};
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

async fn setup_db() -> Database {
    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations().await.expect("Failed to run migrations");
    db
}

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = setup_db().await;
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

fn login_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/auth/logout", axum::routing::post(handle_logout))
        .route("/api/auth/me", get(handle_get_current_user))
        .route(
            "/api/auth/2fa/verify",
            axum::routing::post(handle_2fa_verify),
        )
        .route("/api/security/login-history", get(handle_get_login_history))
        .with_state(state)
}

/// Same routes as [`login_router`] but wrapped with the real CSRF middleware,
/// exactly as `web/mod.rs` wires it for `/api/*`.
fn login_router_with_csrf(state: Arc<AppState>) -> Router {
    login_router(Arc::clone(&state)).layer(axum::middleware::from_fn_with_state(
        state,
        homevault::middleware::csrf_middleware,
    ))
}

/// Insert a user directly into the DB for login testing.
async fn create_test_user(state: &AppState, username: &str, password: &str) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, '', '', 1, false, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(&password_hash)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert test user");
}

/// Insert an admin user.
async fn create_admin_user(state: &AppState, username: &str, password: &str) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Admin', '', 2, false, datetime('now'), 'adminseed')",
    )
    .bind(username)
    .bind(&password_hash)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert admin user");
}

async fn get_json(router: &mut Router, path: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value =
        serde_json::from_slice(&body).unwrap_or(json!({"raw": String::from_utf8_lossy(&body)}));
    (status, json)
}

async fn post_json(router: &mut Router, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body_bytes)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&body_bytes)}));
    (status, json)
}

/// POST with a `user-agent` header, mirroring a real browser login (used by
/// the audit-history tests, which assert the UA is persisted).
async fn post_json_ua(
    router: &mut Router,
    path: &str,
    body: Value,
    user_agent: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("user-agent", user_agent)
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body_bytes)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&body_bytes)}));
    (status, json)
}

/// Authenticated GET carrying `x-session-token`.
async fn get_json_auth(router: &mut Router, path: &str, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("x-session-token", token)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body_bytes)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&body_bytes)}));
    (status, json)
}

async fn post_json_with_token(
    router: &mut Router,
    path: &str,
    body: Value,
    token: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .header("x-session-token", token)
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body_bytes)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&body_bytes)}));
    (status, json)
}

async fn get_json_with_token(router: &mut Router, path: &str, token: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("x-session-token", token)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value =
        serde_json::from_slice(&body).unwrap_or(json!({"raw": String::from_utf8_lossy(&body)}));
    (status, json)
}

// ========================================================================
// 1. POST /api/auth/login
// ========================================================================

#[tokio::test]
async fn test_login_success() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "testuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "testuser", "password": "TestPassw0rd123"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    let data = &body["data"];
    assert!(!data["token"].as_str().unwrap_or("").is_empty());
    assert_eq!(data["username"].as_str().unwrap_or(""), "testuser");
}

#[tokio::test]
async fn test_login_wrong_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "testuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "testuser", "password": "WrongPass999"}),
    )
    .await;

    assert!(status.is_client_error() || status == StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("Invalid") || msg.contains("invalid"),
        "Expected auth error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_login_user_not_found() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "nonexistent", "password": "TestPassw0rd123"}),
    )
    .await;

    assert!(status.is_client_error() || status == StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_login_empty_credentials() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, _) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "", "password": ""}),
    )
    .await;

    assert!(status.is_client_error());
}

#[tokio::test]
async fn test_login_empty_body() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    assert!(resp.status().is_client_error());
}

#[tokio::test]
async fn test_login_remember_me_sets_longer_session() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "testuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, body_normal) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "testuser", "password": "TestPassw0rd123", "remember_me": false}),
    )
    .await;
    let token_normal = body_normal["data"]["token"].as_str().unwrap_or("");

    let (_, body_remember) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "testuser", "password": "TestPassw0rd123", "remember_me": true}),
    )
    .await;
    let token_remember = body_remember["data"]["token"].as_str().unwrap_or("");

    assert!(!token_normal.is_empty());
    assert!(!token_remember.is_empty());
    assert_ne!(token_normal, token_remember, "Different sessions");
}

#[tokio::test]
async fn test_login_returns_is_admin_for_admin_user() {
    let (state, _tmp, _lock) = setup_state().await;
    create_admin_user(&state, "admincheck", "AdminPassw0rd1").await;
    let mut app = login_router(state);

    let (_, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "admincheck", "password": "AdminPassw0rd1"}),
    )
    .await;

    assert!(body["success"].as_bool().unwrap_or(false));
    assert!(body["data"]["is_admin"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_login_returns_false_is_admin_for_regular_user() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "regularuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "regularuser", "password": "TestPassw0rd123"}),
    )
    .await;

    assert!(body["success"].as_bool().unwrap_or(false));
    assert!(!body["data"]["is_admin"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_login_locks_after_max_attempts() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "lockme", "TestPassw0rd123").await;
    let mut app = login_router(state);

    // 5 wrong attempts
    for _ in 0..5 {
        post_json(
            &mut app,
            "/api/auth/login",
            json!({"username": "lockme", "password": "WrongPass999"}),
        )
        .await;
    }

    // 6th attempt should indicate lockout
    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "lockme", "password": "WrongPass999"}),
    )
    .await;

    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::UNAUTHORIZED
            || msg.contains("locked")
            || msg.contains("too many")
            || msg.contains("Too many"),
        "Expected lockout, got status={}, msg={}",
        status,
        msg
    );
}

// ========================================================================
// 2. GET /api/auth/me
// ========================================================================

#[tokio::test]
async fn test_me_valid_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "meuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, login_body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "meuser", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login_body["data"]["token"].as_str().unwrap();

    let (status, body) = get_json_with_token(&mut app, "/api/auth/me", token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    let data = &body["data"];
    assert_eq!(data["username"].as_str().unwrap_or(""), "meuser");
}

#[tokio::test]
async fn test_me_no_token() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, _) = get_json(&mut app, "/api/auth/me").await;

    assert!(status.is_client_error());
}

#[tokio::test]
async fn test_me_invalid_token() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, _) = get_json_with_token(&mut app, "/api/auth/me", "garbage-token-123").await;

    assert!(status.is_client_error());
}

// ========================================================================
// 3. POST /api/auth/logout
// ========================================================================

#[tokio::test]
async fn test_logout_valid_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "logoutuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, login_body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "logoutuser", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login_body["data"]["token"].as_str().unwrap();

    let (status, body) = post_json_with_token(&mut app, "/api/auth/logout", json!({}), token).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_logout_invalid_token() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, _) =
        post_json_with_token(&mut app, "/api/auth/logout", json!({}), "garbage-token").await;

    // Should still succeed (logout is idempotent) or return error
    // Both are acceptable — the key is it doesn't crash
    assert!(status.is_success() || status.is_client_error());
}

#[tokio::test]
async fn test_me_after_logout_fails() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "logoutuser2", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, login_body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "logoutuser2", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login_body["data"]["token"].as_str().unwrap();

    // Logout
    post_json_with_token(&mut app, "/api/auth/logout", json!({}), token).await;

    // Me should fail
    let (status, _) = get_json_with_token(&mut app, "/api/auth/me", token).await;
    assert!(status.is_client_error());
}

// ========================================================================
// 6. POST /api/auth/2fa/verify
// ========================================================================

#[tokio::test]
async fn test_2fa_verify_no_session() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "testuser", "password": "TestPassw0rd123", "totp_code": "123456"}),
    )
    .await;

    // Should fail — no valid 2FA setup
    assert!(status.is_client_error() || status == StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_2fa_verify_without_2fa_enabled() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofauser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofauser", "password": "TestPassw0rd123", "totp_code": "000000"}),
    )
    .await;

    // Without 2FA enabled, the endpoint authenticates normally (code ignored)
    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    assert_eq!(body["data"]["username"].as_str().unwrap(), "twofauser");
}

#[tokio::test]
async fn test_2fa_verify_wrong_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofauser2", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofauser2", "password": "WrongPass999", "totp_code": "123456"}),
    )
    .await;

    // Wrong password should fail
    let success = body["success"].as_bool().unwrap_or(true);
    assert!(
        status.is_client_error() || status == StatusCode::UNAUTHORIZED || !success,
        "Expected auth failure, got status={}, body={:?}",
        status,
        body
    );
}

#[tokio::test]
async fn test_2fa_verify_empty_body() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/2fa/verify")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    assert!(resp.status().is_client_error());
}

// ========================================================================
// 7. Full flow: setup → login → me → logout → me(fail)
// ========================================================================

#[tokio::test]
async fn test_full_auth_flow() {
    let (state, _tmp, _lock) = setup_state().await;
    create_admin_user(&state, "flowadmin", "FlowPassw0rd123").await;
    let mut app = login_router(Arc::clone(&state));

    // Step 1: Login succeeds
    let (login_status, login_body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "flowadmin", "password": "FlowPassw0rd123"}),
    )
    .await;
    assert_eq!(login_status, StatusCode::OK);
    let token = login_body["data"]["token"].as_str().unwrap();
    assert!(!token.is_empty());

    // Step 4: /me returns user
    let (me_status, me_body) = get_json_with_token(&mut app, "/api/auth/me", token).await;
    assert_eq!(me_status, StatusCode::OK);
    assert_eq!(me_body["data"]["username"].as_str().unwrap(), "flowadmin");

    // Step 5: Logout
    let (logout_status, _) =
        post_json_with_token(&mut app, "/api/auth/logout", json!({}), token).await;
    assert_eq!(logout_status, StatusCode::OK);

    // Step 6: /me fails after logout
    let (me_after, _) = get_json_with_token(&mut app, "/api/auth/me", token).await;
    assert!(me_after.is_client_error());
}

#[tokio::test]
async fn test_login_json_malformed() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("not json"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    assert!(
        resp.status().is_client_error(),
        "Malformed JSON should return 4xx, got: {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_login_returns_csrf_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "csrfuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "csrfuser", "password": "TestPassw0rd123"}),
    )
    .await;

    assert!(body["success"].as_bool().unwrap_or(false));
    let csrf = body["data"]["csrf_token"].as_str().unwrap_or("");
    assert!(!csrf.is_empty(), "Should return a CSRF token");
}

// ========================================================================
// 8. Two-factor login flow (real 2FA, verified via backup codes)
// ========================================================================

/// Insert a user with a real fullname/email for profile assertions.
async fn create_full_user(state: &AppState, username: &str, password: &str) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Full Name', 'user@example.com', 1, false, datetime('now'), 'fullseed')",
    )
    .bind(username)
    .bind(&password_hash)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert full user");
}

/// Reads a session row's `expires_at` (RFC3339 string) straight from the DB.
async fn session_expires_at(state: &AppState, token: &str) -> String {
    let row: (String,) = sqlx::query_as("SELECT expires_at FROM sessions WHERE token = ?")
        .bind(token)
        .fetch_one(&*state.db.pool())
        .await
        .expect("session row not found");
    row.0
}

#[tokio::test]
async fn test_login_with_2fa_enabled_returns_requires_2fa() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofa_login", "TestPassw0rd123").await;
    state
        .auth
        .enable_2fa("twofa_login", "JBSWY3DPEHPK3PXP")
        .await
        .expect("enable 2fa");
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "twofa_login", "password": "TestPassw0rd123"}),
    )
    .await;

    // The plain login endpoint must NOT issue a session for a 2FA account.
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["data"]["requires_2fa"].as_bool().unwrap_or(false));
    assert!(
        body["data"]["token"].as_str().unwrap_or("").is_empty(),
        "no session token before the second factor"
    );
}

#[tokio::test]
async fn test_2fa_verify_with_valid_backup_code_succeeds() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofa_bc", "TestPassw0rd123").await;
    let codes = state
        .auth
        .enable_2fa("twofa_bc", "JBSWY3DPEHPK3PXP")
        .await
        .expect("enable 2fa");
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofa_bc", "password": "TestPassw0rd123", "totp_code": codes[0]}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    assert!(!body["data"]["token"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn test_2fa_verify_with_invalid_code_fails() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofa_bad", "TestPassw0rd123").await;
    state
        .auth
        .enable_2fa("twofa_bad", "JBSWY3DPEHPK3PXP")
        .await
        .expect("enable 2fa");
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofa_bad", "password": "TestPassw0rd123", "totp_code": "999999"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("verification code") || msg.contains("Invalid"),
        "expected 2FA code error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_2fa_verify_backup_code_is_single_use() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofa_reuse", "TestPassw0rd123").await;
    let codes = state
        .auth
        .enable_2fa("twofa_reuse", "JBSWY3DPEHPK3PXP")
        .await
        .expect("enable 2fa");
    let mut app = login_router(state);

    // First use consumes the code.
    let (first, _) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofa_reuse", "password": "TestPassw0rd123", "totp_code": codes[0]}),
    )
    .await;
    assert_eq!(first, StatusCode::OK);

    // Second use of the same code must be rejected.
    let (second, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofa_reuse", "password": "TestPassw0rd123", "totp_code": codes[0]}),
    )
    .await;
    assert_eq!(second, StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_2fa_verify_wrong_password_with_valid_backup_code() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "twofa_wp", "TestPassw0rd123").await;
    let codes = state
        .auth
        .enable_2fa("twofa_wp", "JBSWY3DPEHPK3PXP")
        .await
        .expect("enable 2fa");
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "twofa_wp", "password": "WrongPass999", "totp_code": codes[0]}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_2fa_verify_nonexistent_user() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/2fa/verify",
        json!({"username": "nobody_here", "password": "TestPassw0rd123", "totp_code": "123456"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!body["success"].as_bool().unwrap_or(true));
}

// ========================================================================
// 10. Locked accounts
// ========================================================================

#[tokio::test]
async fn test_login_locked_account_is_rejected() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "lockedacct", "TestPassw0rd123").await;
    sqlx::query("UPDATE users SET locked = 1 WHERE username = ?")
        .bind("lockedacct")
        .execute(&*state.db.pool())
        .await
        .unwrap();
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "lockedacct", "password": "TestPassw0rd123"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("locked"),
        "expected a locked-account message, got: {}",
        msg
    );
}

// ========================================================================
// 11. Session semantics
// ========================================================================

#[tokio::test]
async fn test_logout_only_invalidates_the_calling_session() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "multisess", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, a) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "multisess", "password": "TestPassw0rd123"}),
    )
    .await;
    let token_a = a["data"]["token"].as_str().unwrap().to_string();

    let (_, b) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "multisess", "password": "TestPassw0rd123"}),
    )
    .await;
    let token_b = b["data"]["token"].as_str().unwrap().to_string();
    assert_ne!(token_a, token_b);

    // Log session A out.
    post_json_with_token(&mut app, "/api/auth/logout", json!({}), &token_a).await;

    let (me_a, _) = get_json_with_token(&mut app, "/api/auth/me", &token_a).await;
    assert!(me_a.is_client_error(), "session A should be dead");

    let (me_b, _) = get_json_with_token(&mut app, "/api/auth/me", &token_b).await;
    assert_eq!(me_b, StatusCode::OK, "session B should still be alive");
}

#[tokio::test]
async fn test_remember_me_grants_a_longer_lived_session() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "rmexpiry", "TestPassw0rd123").await;
    let mut app = login_router(Arc::clone(&state));

    let (_, normal) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "rmexpiry", "password": "TestPassw0rd123", "remember_me": false}),
    )
    .await;
    let (_, remembered) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "rmexpiry", "password": "TestPassw0rd123", "remember_me": true}),
    )
    .await;

    let normal_exp = session_expires_at(&state, normal["data"]["token"].as_str().unwrap()).await;
    let remembered_exp =
        session_expires_at(&state, remembered["data"]["token"].as_str().unwrap()).await;

    assert!(
        remembered_exp > normal_exp,
        "remember_me expiry ({}) should be later than the default ({})",
        remembered_exp,
        normal_exp
    );
}

#[tokio::test]
async fn test_concurrent_sessions_are_both_valid() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "concurrent", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let mut tokens = Vec::new();
    for _ in 0..3 {
        let (_, body) = post_json(
            &mut app,
            "/api/auth/login",
            json!({"username": "concurrent", "password": "TestPassw0rd123"}),
        )
        .await;
        tokens.push(body["data"]["token"].as_str().unwrap().to_string());
    }

    for token in &tokens {
        let (status, _) = get_json_with_token(&mut app, "/api/auth/me", token).await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn test_login_csrf_token_differs_from_session_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "csrfdiff", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "csrfdiff", "password": "TestPassw0rd123"}),
    )
    .await;

    let token = body["data"]["token"].as_str().unwrap();
    let csrf = body["data"]["csrf_token"].as_str().unwrap();
    assert_ne!(token, csrf, "CSRF token must not equal the session token");
    assert!(csrf.len() >= 16);
}

// ========================================================================
// 12. /me profile payload
// ========================================================================

#[tokio::test]
async fn test_me_returns_full_profile_fields() {
    let (state, _tmp, _lock) = setup_state().await;
    create_full_user(&state, "profileuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (_, login_body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "profileuser", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login_body["data"]["token"].as_str().unwrap();

    let (status, body) = get_json_with_token(&mut app, "/api/auth/me", token).await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    assert_eq!(data["username"], "profileuser");
    assert_eq!(data["fullname"], "Full Name");
    assert_eq!(data["email"], "user@example.com");
    assert!(!data["two_fa_enabled"].as_bool().unwrap_or(true));
    assert!(!data["role"].as_str().unwrap_or("").is_empty());
    assert!(
        data["avatar_url"]
            .as_str()
            .unwrap_or("")
            .contains("profileuser"),
        "avatar_url should reference the username"
    );
}

// ========================================================================
// 13. Login request-body handling
// ========================================================================

#[tokio::test]
async fn test_login_missing_password_field_rejected() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let (status, _) = post_json(&mut app, "/api/auth/login", json!({"username": "someone"})).await;

    // Missing a required field — serde rejects before the handler runs.
    assert!(status.is_client_error());
}

#[tokio::test]
async fn test_login_ignores_unknown_extra_fields() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "extrafields", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({
            "username": "extrafields",
            "password": "TestPassw0rd123",
            "totally_unknown": "ignored",
            "is_admin": true
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    // The bogus `is_admin: true` in the request must not be honoured.
    assert!(!body["data"]["is_admin"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_login_username_is_case_sensitive() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "casesensitive", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (status, _) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "CaseSensitive", "password": "TestPassw0rd123"}),
    )
    .await;

    // Documents current behaviour: the stored username is matched exactly.
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_login_wrong_password_does_not_leak_user_existence() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "realuser", "TestPassw0rd123").await;
    let mut app = login_router(state);

    let (s1, b1) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "realuser", "password": "WrongPass999"}),
    )
    .await;
    let (s2, b2) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "ghostuser", "password": "WrongPass999"}),
    )
    .await;

    assert_eq!(s1, s2, "same status for wrong-password vs unknown-user");
    assert_eq!(
        b1["message"].as_str().unwrap_or(""),
        b2["message"].as_str().unwrap_or("x"),
        "same message for wrong-password vs unknown-user"
    );
}

// ========================================================================
// 14. CSRF middleware on authenticated mutating requests
// ========================================================================

#[tokio::test]
async fn test_csrf_rejects_authenticated_post_without_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "csrfmw", "TestPassw0rd123").await;
    let mut app = login_router_with_csrf(state);

    let (_, login) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "csrfmw", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login["data"]["token"].as_str().unwrap().to_string();

    // POST /logout with a valid session token but NO x-csrf-token → 403.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/logout")
        .header("content-type", "application/json")
        .header("x-session-token", token.as_str())
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_csrf_rejects_wrong_token_and_accepts_matching_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "csrfmw2", "TestPassw0rd123").await;
    let mut app = login_router_with_csrf(state);

    let (_, login) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "csrfmw2", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login["data"]["token"].as_str().unwrap().to_string();
    let csrf = login["data"]["csrf_token"].as_str().unwrap().to_string();

    let logout = |csrf_header: Option<&str>| {
        let mut b = Request::builder()
            .method(Method::POST)
            .uri("/api/auth/logout")
            .header("content-type", "application/json")
            .header("x-session-token", token.as_str());
        if let Some(c) = csrf_header {
            b = b.header("x-csrf-token", c);
        }
        b.body(axum::body::Body::from("{}")).unwrap()
    };

    // Wrong CSRF token → 403.
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(logout(Some("not-the-real-csrf")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Matching CSRF token → passes through to the handler (200).
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(logout(Some(&csrf)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_csrf_does_not_touch_get_requests() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "csrfget", "TestPassw0rd123").await;
    let mut app = login_router_with_csrf(state);

    let (_, login) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "csrfget", "password": "TestPassw0rd123"}),
    )
    .await;
    let token = login["data"]["token"].as_str().unwrap().to_string();

    // GET /me with a session token but no CSRF header still works.
    let (status, _) = get_json_with_token(&mut app, "/api/auth/me", &token).await;
    assert_eq!(status, StatusCode::OK);
}

// ========================================================================
// 15. Session & token expiry, cleanup jobs
// ========================================================================

/// Insert a session row directly with a caller-chosen `expires_at`.
async fn insert_session(state: &AppState, token: &str, username: &str, expires_at_rfc3339: &str) {
    sqlx::query(
        "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at, is_admin)
         VALUES (?, ?, '127.0.0.1', 'csrf-x', datetime('now'), ?, 0)",
    )
    .bind(token)
    .bind(username)
    .bind(expires_at_rfc3339)
    .execute(&*state.db.pool())
    .await
    .expect("insert session");
}

#[tokio::test]
async fn test_me_rejects_an_expired_session_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "expsess", "TestPassw0rd123").await;
    insert_session(
        &state,
        "expired-token-xyz",
        "expsess",
        "2000-01-01T00:00:00+00:00",
    )
    .await;
    let mut app = login_router(state);

    let (status, _) = get_json_with_token(&mut app, "/api/auth/me", "expired-token-xyz").await;
    assert!(status.is_client_error(), "expired session must be rejected");
}

#[tokio::test]
async fn test_refresh_session_extends_expiry() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "refreshme", "TestPassw0rd123").await;
    insert_session(
        &state,
        "soon-token",
        "refreshme",
        "2999-01-01T00:00:00+00:00",
    )
    .await;

    // A far-future placeholder, then refresh should reset it to now + lifetime,
    // i.e. *earlier* than the year-2999 placeholder.
    let before = session_expires_at(&state, "soon-token").await;
    state
        .auth
        .refresh_session("soon-token")
        .await
        .expect("refresh");
    let after = session_expires_at(&state, "soon-token").await;
    assert!(
        after < before,
        "refresh should rewrite expires_at to now + lifetime"
    );

    // And the refreshed session is still valid right now.
    let mut app = login_router(state);
    let (status, _) = get_json_with_token(&mut app, "/api/auth/me", "soon-token").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_refresh_session_unknown_token_errors() {
    let (state, _tmp, _lock) = setup_state().await;
    let err = state.auth.refresh_session("no-such-token").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn test_cleanup_expired_sessions_removes_only_expired_rows() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "cleanup", "TestPassw0rd123").await;
    insert_session(&state, "dead-1", "cleanup", "2001-01-01T00:00:00+00:00").await;
    insert_session(&state, "dead-2", "cleanup", "2002-01-01T00:00:00+00:00").await;
    insert_session(&state, "alive-1", "cleanup", "2999-01-01T00:00:00+00:00").await;

    state.auth.cleanup_expired_sessions().await;

    let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions WHERE username = ?")
        .bind("cleanup")
        .fetch_one(&*state.db.pool())
        .await
        .unwrap();
    assert_eq!(
        remaining.0, 1,
        "only the non-expired session should survive"
    );
}

// ========================================================================
// 18. Concurrency — the failed-attempt lockout cannot be raced open
// ========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_wrong_logins_still_lock_the_account() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "raceme", "TestPassw0rd123").await;
    let app = login_router(Arc::clone(&state));

    // Fire 12 wrong-password logins concurrently.
    let mut handles = Vec::new();
    for _ in 0..12 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({"username": "raceme", "password": "Wrong999"}))
                        .unwrap(),
                ))
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        }));
    }
    for h in handles {
        let _ = h.await;
    }

    // A subsequent attempt — even with the *correct* password — must be locked
    // out, proving the counter was not lost to a race.
    let mut app2 = login_router(state);
    let (status, body) = post_json(
        &mut app2,
        "/api/auth/login",
        json!({"username": "raceme", "password": "TestPassw0rd123"}),
    )
    .await;
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        status == StatusCode::TOO_MANY_REQUESTS
            || msg.contains("Too many")
            || msg.contains("locked"),
        "account should be locked after a burst of failures, got status={} msg={}",
        status,
        msg
    );
}

// ========================================================================
// 19. HTTP rate-limit middleware (`rate_limit_middleware`, keyed by client IP)
// ========================================================================

/// Minimal router that only carries the IP rate-limit middleware.
fn rate_limited_router(limiter: Arc<RateLimitState>) -> Router {
    async fn ok() -> &'static str {
        "ok"
    }
    Router::new().route("/ping", axum::routing::get(ok)).layer(
        axum::middleware::from_fn_with_state(limiter, rate_limit_middleware),
    )
}

async fn ping_from_ip(router: &mut Router, ip: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/ping")
        .header("x-forwarded-for", ip)
        .body(axum::body::Body::empty())
        .unwrap();
    ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn test_rate_limit_middleware_blocks_after_the_limit() {
    let limiter = Arc::new(RateLimitState::new(3, 60));
    let mut app = rate_limited_router(limiter);

    // First 3 requests from the same IP pass.
    for _ in 0..3 {
        assert_eq!(ping_from_ip(&mut app, "203.0.113.7").await, StatusCode::OK);
    }
    // The 4th is rejected.
    assert_eq!(
        ping_from_ip(&mut app, "203.0.113.7").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn test_rate_limit_middleware_is_per_ip() {
    let limiter = Arc::new(RateLimitState::new(2, 60));
    let mut app = rate_limited_router(limiter);

    assert_eq!(ping_from_ip(&mut app, "198.51.100.1").await, StatusCode::OK);
    assert_eq!(ping_from_ip(&mut app, "198.51.100.1").await, StatusCode::OK);
    assert_eq!(
        ping_from_ip(&mut app, "198.51.100.1").await,
        StatusCode::TOO_MANY_REQUESTS
    );

    // A different IP still has its full budget.
    assert_eq!(ping_from_ip(&mut app, "198.51.100.2").await, StatusCode::OK);
}

#[tokio::test]
async fn test_rate_limit_state_window_resets_after_expiry() {
    // A 1-second window: exhaust it, wait it out, then it refills.
    let limiter = RateLimitState::new(2, 1);
    assert!(limiter.check_rate_limit("k").is_ok());
    assert!(limiter.check_rate_limit("k").is_ok());
    assert!(limiter.check_rate_limit("k").is_err());

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    assert!(
        limiter.check_rate_limit("k").is_ok(),
        "the sliding window should have rolled over"
    );
}

// ========================================================================
// 20. Hardware-change detection surfaced on /setup-status
// ========================================================================

#[tokio::test]
async fn test_setup_status_reports_hardware_changed_when_fingerprint_mismatches() {
    let (state, _tmp, _lock) = setup_state().await;

    // A configured appliance: an admin exists and a fingerprint was recorded
    // that cannot match this host's real system hash.
    create_admin_user(&state, "hwboss", "SecurePass123").await;
    homevault::storage::SetupStateRepository::new(state.db.pool())
        .mark_complete("deadbeef-not-a-real-hash", "machine-x")
        .await
        .unwrap();

    let mut app = Router::new()
        .route(
            "/api/auth/setup-status",
            get(homevault::handlers::auth::handle_setup_status),
        )
        .with_state(state);

    let (status, body) = get_json(&mut app, "/api/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]["hardware_changed"].as_bool().unwrap_or(false),
        "a stale recorded fingerprint must flip hardware_changed"
    );
    assert!(
        body["data"]["needs_recovery"].as_bool().unwrap_or(false),
        "an existing admin + changed fingerprint must ask for the recovery flow"
    );
    assert!(
        !body["data"]["is_first_run"].as_bool().unwrap_or(true),
        "an admin exists, so this is not a first run"
    );
}

#[tokio::test]
async fn test_setup_status_no_hardware_change_when_fingerprint_matches() {
    let (state, _tmp, _lock) = setup_state().await;

    create_admin_user(&state, "hwboss", "SecurePass123").await;
    let real = homevault::setup::SetupService::generate_system_hash();
    homevault::storage::SetupStateRepository::new(state.db.pool())
        .mark_complete(&real, "machine-x")
        .await
        .unwrap();

    let mut app = Router::new()
        .route(
            "/api/auth/setup-status",
            get(homevault::handlers::auth::handle_setup_status),
        )
        .with_state(state);

    let (_, body) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(!body["data"]["hardware_changed"].as_bool().unwrap_or(true));
    assert!(!body["data"]["needs_recovery"].as_bool().unwrap_or(true));
    assert!(body["data"]["is_setup_valid"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_setup_status_not_first_run_when_admin_exists_without_fingerprint() {
    // A database that predates migration 037 (admin row, no `setup_state`
    // row) is configured — not a first run — and not a hardware change.
    let (state, _tmp, _lock) = setup_state().await;
    create_admin_user(&state, "legacy", "SecurePass123").await;

    let mut app = Router::new()
        .route(
            "/api/auth/setup-status",
            get(homevault::handlers::auth::handle_setup_status),
        )
        .with_state(state);

    let (_, body) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(!body["data"]["is_first_run"].as_bool().unwrap_or(true));
    assert!(!body["data"]["hardware_changed"].as_bool().unwrap_or(true));
    assert!(!body["data"]["needs_recovery"].as_bool().unwrap_or(true));
}

// ========================================================================
// 21. Email delivery path — the app delegates to an external MTA
// ========================================================================
//
// `EmailService::send_email` does NOT open an SMTP connection itself; it shells
// out to `mutt` -> `sendmail` -> `msmtp` (see `src/email.rs`). With none of
// those installed in the test environment it must fail cleanly rather than
// panic or hang, and `is_email_configured()` must report the absence.

#[tokio::test]
async fn test_email_delivery_fails_cleanly_without_an_mta() {
    use homevault::email::{EmailConfig, EmailMessage, EmailRecipient, EmailService};

    let configured = EmailService::is_email_configured();
    let msg = EmailMessage {
        to: vec![EmailRecipient {
            email: "someone@example.com".to_string(),
            name: Some("Someone".to_string()),
        }],
        subject: "test".to_string(),
        body: "body".to_string(),
        is_html: false,
    };
    let result = EmailService::send_email(&msg, &EmailConfig::default());

    if configured {
        // An MTA is present on this box — we can't assert on delivery, only
        // that the call returns without panicking.
        let _ = result;
    } else {
        assert!(
            result.is_err(),
            "with no MTA installed, send_email must return Err (not silently 'succeed')"
        );
    }
}

#[tokio::test]
async fn test_login_history_persists_real_success_and_user_agent() {
    let (state, _tmp, _lock) = setup_state().await;
    create_admin_user(&state, "histadmin", "AdminPass123").await;
    create_test_user(&state, "histfail", "Passw0rd123456").await;
    create_test_user(&state, "histok", "Passw0rd123456").await;
    let mut app = login_router(state);

    // Failed login — the audit trail must record success=false + the browser UA.
    let (status, _) = post_json_ua(
        &mut app,
        "/api/auth/login",
        json!({"username": "histfail", "password": "WrongPass999"}),
        "Edge-Agent/13",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Successful login with a different UA.
    let (status, _) = post_json_ua(
        &mut app,
        "/api/auth/login",
        json!({"username": "histok", "password": "Passw0rd123456"}),
        "Chrome-Agent/120",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Admin reads the global login history.
    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "histadmin", "password": "AdminPass123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token = body["data"]["token"].as_str().unwrap_or("").to_string();

    let (status, body) = get_json_auth(&mut app, "/api/security/login-history", &token).await;
    assert_eq!(status, StatusCode::OK);
    let attempts = body["data"]["attempts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(attempts.len() >= 2, "expected at least 2 attempts");

    let fail_row = attempts.iter().find(|a| a["username"] == "histfail");
    let ok_row = attempts.iter().find(|a| a["username"] == "histok");
    assert!(fail_row.is_some(), "failed attempt must appear in history");
    assert!(
        ok_row.is_some(),
        "successful attempt must appear in history"
    );

    let fail = fail_row.unwrap();
    assert_eq!(fail["success"], json!(false));
    assert_eq!(fail["user_agent"], json!("Edge-Agent/13"));

    let ok = ok_row.unwrap();
    assert_eq!(ok["success"], json!(true));
    assert_eq!(ok["user_agent"], json!("Chrome-Agent/120"));
}

// ========================================================================
// 23. Lockout expiry & per-IP throttle (2026-10-17 hardening)
// ========================================================================
//
// Five wrong passwords used to set `users.locked = 1` and never clear it: on a
// single-admin appliance that bricked the only account (there is no support
// desk), with "temporarily locked" in the message and no way back in short of
// the offline recovery code or editing the database. An automatic lock now
// records `locked_until` and lifts itself; a deliberate admin lock (no
// `locked_until`) stays permanent. Separately, the only throttle used to be per
// *account*, so spraying one password across many usernames was unlimited.

/// Five wrong passwords lock the account *with* an expiry recorded.
#[tokio::test]
async fn test_failed_logins_record_a_temporary_lock_expiry() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "expirylock", "TestPassw0rd123").await;
    let mut app = login_router(Arc::clone(&state));

    for _ in 0..5 {
        post_json(
            &mut app,
            "/api/auth/login",
            json!({"username": "expirylock", "password": "WrongPass999"}),
        )
        .await;
    }

    let (locked, locked_until): (bool, Option<String>) =
        sqlx::query_as("SELECT locked, locked_until FROM users WHERE username = ?")
            .bind("expirylock")
            .fetch_one(&*state.db.pool())
            .await
            .expect("read lock state");

    assert!(locked, "the account must be locked after 5 failures");
    assert!(
        locked_until.is_some(),
        "an automatic lock must record when it expires, otherwise it is permanent"
    );
}

/// Once `locked_until` has passed, the correct password works again — the whole
/// point of the fix.
#[tokio::test]
async fn test_expired_temporary_lock_lifts_itself_on_login() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "autounlock", "TestPassw0rd123").await;

    // Simulate the 5-minute window having elapsed.
    sqlx::query(
        "UPDATE users SET locked = 1, locked_until = ? WHERE username = ?",
    )
    .bind((chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339())
    .bind("autounlock")
    .execute(&*state.db.pool())
    .await
    .unwrap();

    let mut app = login_router(Arc::clone(&state));
    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "autounlock", "password": "TestPassw0rd123"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "expired lock must not block: {body}");
    assert_eq!(body["success"], json!(true));

    let (locked, locked_until): (bool, Option<String>) =
        sqlx::query_as("SELECT locked, locked_until FROM users WHERE username = ?")
            .bind("autounlock")
            .fetch_one(&*state.db.pool())
            .await
            .expect("read lock state");
    assert!(!locked, "the lock must be cleared");
    assert!(locked_until.is_none(), "the expiry must be cleared too");
}

/// A lock that has not expired yet still refuses the login.
#[tokio::test]
async fn test_unexpired_temporary_lock_still_blocks() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "stilllocked", "TestPassw0rd123").await;

    sqlx::query("UPDATE users SET locked = 1, locked_until = ? WHERE username = ?")
        .bind((chrono::Utc::now() + chrono::Duration::minutes(4)).to_rfc3339())
        .bind("stilllocked")
        .execute(&*state.db.pool())
        .await
        .unwrap();

    let mut app = login_router(state);
    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "stilllocked", "password": "TestPassw0rd123"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .to_lowercase()
        .contains("locked"));
}

/// A deliberate admin lock carries no expiry and must survive forever, even
/// with the correct password.
#[tokio::test]
async fn test_admin_lock_without_expiry_is_permanent() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "adminlocked", "TestPassw0rd123").await;
    state
        .auth
        .set_user_locked("adminlocked", true)
        .await
        .expect("admin lock");

    let mut app = login_router(state);
    let (status, body) = post_json(
        &mut app,
        "/api/auth/login",
        json!({"username": "adminlocked", "password": "TestPassw0rd123"}),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .to_lowercase()
        .contains("locked"));
}

/// Password spraying — one password tried across many usernames — used to be
/// unlimited because the only counter was per account. It is now throttled per
/// client IP.
#[tokio::test]
async fn test_password_spraying_is_throttled_per_ip() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = login_router(state);

    let mut statuses = Vec::new();
    // Well past the per-minute budget (30), each attempt a different username.
    for i in 0..40 {
        let (status, _) = post_json(
            &mut app,
            "/api/auth/login",
            json!({"username": format!("spray{i}"), "password": "SprayPass123"}),
        )
        .await;
        statuses.push(status);
    }

    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "spraying must eventually be throttled, got: {statuses:?}"
    );
}

/// The per-IP throttle must not fire for a handful of legitimate retries.
#[tokio::test]
async fn test_a_few_failed_logins_are_not_ip_throttled() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "fewfails", "TestPassw0rd123").await;
    let mut app = login_router(state);

    for _ in 0..4 {
        let (status, _) = post_json(
            &mut app,
            "/api/auth/login",
            json!({"username": "fewfails", "password": "WrongPass999"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "must stay a normal 401");
    }
}
