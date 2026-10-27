/* =============================================================================
 * File          : setup_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   First-run setup wizard tests: setup-status gating, account creation, storage options, progress persistence.
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
//! Integration tests for the Setup Wizard API endpoints.
//!
//! Each test creates a fresh in-memory SQLite database, runs all embedded
//! migrations, builds a minimal Axum router with the auth handlers, and
//! uses `tower::ServiceExt` for HTTP-level testing without starting a
//! real server.
//!
//! Run:
//!   cargo test --test setup_integration
//!   cargo test --test setup_integration -- --nocapture

use axum::extract::ConnectInfo;
use axum::{Router, routing::get};
use http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;

/// Serializes all tests that mutate DATA_DIRECTORY (process-global env var).
/// Without this, parallel tests clobber each other's lock/flag files.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    AppState,
    auth::{
        handle_check_username, handle_setup, handle_setup_progress_get, handle_setup_progress_put,
        handle_setup_rebaseline, handle_setup_status, handle_setup_verify_totp,
    },
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::Database;
use homevault::storage::SetupStateRepository;
use homevault::storage::WidgetRepository;
use homevault::web::create_app;

type Req = Request<axum::body::Body>;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Build a fresh in-memory database with all migrations applied.
async fn setup_db() -> Database {
    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations().await.expect("Failed to run migrations");
    db
}

/// Build a minimal AppState with a temp DATA_DIRECTORY for lock/flag files.
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

/// Build a test router with only the auth/setup endpoints.
fn test_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/setup-status", get(handle_setup_status))
        .route("/api/auth/check-username", get(handle_check_username))
        .route("/api/auth/setup", axum::routing::post(handle_setup))
        .route(
            "/api/auth/setup/verify-totp",
            axum::routing::post(handle_setup_verify_totp),
        )
        .route(
            "/api/auth/setup/progress",
            get(handle_setup_progress_get).put(handle_setup_progress_put),
        )
        .with_state(state)
}

/// Helper: send a GET request and return (status, body JSON).
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

/// Helper: send a POST request with JSON body and return (status, body JSON).
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

/// Helper: send a PUT request with JSON body and return (status, body JSON).
async fn put_json(router: &mut Router, path: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::PUT)
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
    let put_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&put_bytes)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&put_bytes)}));
    (status, json)
}

// ========================================================================
// 1. GET /api/auth/setup-status
// ========================================================================

#[tokio::test]
async fn test_setup_status_first_run() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = get_json(&mut app, "/api/auth/setup-status").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    let data = &body["data"];
    assert!(data["is_first_run"].as_bool().unwrap());
    assert!(!data["is_setup_valid"].as_bool().unwrap());
}

#[tokio::test]
async fn test_setup_status_has_sudo_available_field() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(&mut app, "/api/auth/setup-status").await;
    let data = &body["data"];
    assert!(
        data.get("sudo_available").is_some(),
        "sudo_available should be present"
    );
}

/// The wizard no longer asks for a language: the host's POSIX locale is
/// reported so the frontend can pick the initial dictionary itself.
#[tokio::test]
async fn test_setup_status_reports_a_supported_system_language() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(&mut app, "/api/auth/setup-status").await;
    let lang = body["data"]["system_language"]
        .as_str()
        .expect("system_language should be a string");
    assert!(
        ["en", "fr", "de", "es"].contains(&lang),
        "system_language {lang:?} must be one of the shipped languages"
    );
}

#[tokio::test]
async fn test_setup_status_has_machine_id() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(&mut app, "/api/auth/setup-status").await;
    let data = &body["data"];
    let machine_id = data["server_machine_id"].as_str().unwrap_or("");
    assert!(!machine_id.is_empty(), "machine_id should not be empty");
}

// ========================================================================
// 2. GET /api/auth/check-username
// ========================================================================

#[tokio::test]
async fn test_check_username_available() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = get_json(&mut app, "/api/auth/check-username?username=newuser").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    assert!(body["data"]["available"].as_bool().unwrap());
}

#[tokio::test]
async fn test_check_username_reserved_root() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = get_json(&mut app, "/api/auth/check-username?username=root").await;

    assert_eq!(status, StatusCode::OK);
    assert!(!body["data"]["available"].as_bool().unwrap());
    let reason = body["data"]["reason"].as_str().unwrap_or("");
    assert!(
        !reason.is_empty(),
        "Reason should explain why username is unavailable"
    );
}

#[tokio::test]
async fn test_check_username_reserved_admin() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(&mut app, "/api/auth/check-username?username=admin").await;
    assert!(!body["data"]["available"].as_bool().unwrap());
}

#[tokio::test]
async fn test_check_username_too_short() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(&mut app, "/api/auth/check-username?username=ab").await;
    assert!(!body["data"]["available"].as_bool().unwrap());
}

#[tokio::test]
async fn test_check_username_invalid_chars() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(&mut app, "/api/auth/check-username?username=user+name").await;
    assert!(!body["data"]["available"].as_bool().unwrap());
}

#[tokio::test]
async fn test_check_username_is_disabled_once_setup_complete() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    // Complete setup first — the wizard's oracle must close with it.
    let (status1, _) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);

    let (status, body) = get_json(&mut app, "/api/auth/check-username?username=someoneelse").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "post-setup the endpoint must not answer availability at all: {:?}",
        body
    );
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("already"),
        "expected already-complete rejection, got: {}",
        msg
    );
}

// ========================================================================
// 3. POST /api/auth/setup — validation errors
// ========================================================================

#[tokio::test]
async fn test_setup_empty_username() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "", "password": "ValidPassw0rd"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(!msg.is_empty(), "Error message should not be empty");
}

#[tokio::test]
async fn test_setup_reserved_username() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "root", "password": "ValidPassw0rd"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("reserved") || msg.contains("Username"),
        "Expected reserved/username error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_setup_weak_password_no_uppercase() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "testuser", "password": "lowercase1234"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("uppercase"),
        "Expected uppercase error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_setup_weak_password_no_digit() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "testuser", "password": "NoDigitsHere"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(msg.contains("digit"), "Expected digit error, got: {}", msg);
}

#[tokio::test]
async fn test_setup_password_too_short() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "testuser", "password": "Short1"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("12 characters"),
        "Expected length error, got: {}",
        msg
    );
}

/// An 11-character password that satisfies every composition rule is still
/// rejected: the floor is 12 (the admin credential owns the whole appliance).
#[tokio::test]
async fn test_setup_password_eleven_chars_rejected() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "testuser", "password": "Abcdefgh123"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(msg.contains("12 characters"), "got: {msg}");
}

/// A compromised / obvious password is refused even when it clears the length
/// and composition rules — and no admin row is created.
#[tokio::test]
async fn test_setup_password_too_common_rejected() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state.clone());

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "testuser", "password": "Password1234"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(msg.contains("too common"), "got: {msg}");

    let admin_exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role_id = 2")
        .fetch_one(&*state.db.pool())
        .await
        .expect("count admins");
    assert_eq!(admin_exists, 0, "no admin may be created");
}

// ========================================================================
// 4. POST /api/auth/setup — success flow
// ========================================================================

#[tokio::test]
async fn test_setup_creates_admin_and_returns_session() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "fullname": "Test Admin"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "Setup should succeed: {:?}", body);
    assert!(body["success"].as_bool().unwrap_or(false));

    let data = &body["data"];
    assert!(
        !data["token"].as_str().unwrap_or("").is_empty(),
        "Should return a session token"
    );
    assert_eq!(data["username"].as_str().unwrap_or(""), "firstadmin");
    assert!(data["is_admin"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_setup_marks_first_run_false() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(Arc::clone(&state));

    // Before setup
    let (_, before) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(before["data"]["is_first_run"].as_bool().unwrap());

    // Do setup
    post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;

    // After setup — same state, same in-memory DB
    let mut app2 = test_router(state);
    let (_, after) = get_json(&mut app2, "/api/auth/setup-status").await;
    assert!(!after["data"]["is_first_run"].as_bool().unwrap());
}

#[tokio::test]
async fn test_setup_second_call_fails_already_complete() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    // First setup succeeds
    let (status1, _) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);

    // Second setup fails
    let (status2, body2) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "admin2", "password": "SecurePass456"}),
    )
    .await;
    assert_ne!(status2, StatusCode::OK, "Second setup should fail");
    let msg = body2["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("already") || msg.contains("Setup"),
        "Expected already-complete error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_setup_with_fullname() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "fullname": "John Doe"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_setup_without_fullname() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_setup_with_avatar_seed() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "avatar_seed": "my_custom_seed_123"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
}

// ========================================================================
// 5. POST /api/auth/setup — check-username after setup
// ========================================================================

// ========================================================================
// 6. Full flow: status → setup → check → login ready
// ========================================================================

#[tokio::test]
async fn test_full_setup_flow() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    // Step 1: Check status — should be first run
    let (_, status_body) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(status_body["data"]["is_first_run"].as_bool().unwrap());

    // Step 2: Check username availability
    let (_, check_body) = get_json(&mut app, "/api/auth/check-username?username=firstadmin").await;
    assert!(check_body["data"]["available"].as_bool().unwrap());

    // Step 3: Create admin
    let (setup_status, setup_body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "fullname": "Administrator"
        }),
    )
    .await;
    assert_eq!(setup_status, StatusCode::OK);
    let token = setup_body["data"]["token"].as_str().unwrap();
    assert!(!token.is_empty());

    // Step 4: the username oracle is closed once setup completed —
    // check-username answers 403 instead of reporting availability.
    let (taken_status, taken_body) =
        get_json(&mut app, "/api/auth/check-username?username=firstadmin").await;
    assert_eq!(
        taken_status,
        StatusCode::FORBIDDEN,
        "post-setup check-username must refuse: {:?}",
        taken_body
    );

    // Step 5: Verify setup status is no longer first run
    let (_, final_status) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(!final_status["data"]["is_first_run"].as_bool().unwrap());
}

// ========================================================================
// 7. Edge cases
// ========================================================================

#[tokio::test]
async fn test_setup_empty_body() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/setup")
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();

    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_setup_malformed_json() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/setup")
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
async fn test_check_username_missing_param() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/auth/check-username")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "Missing username param should return 4xx, got: {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_setup_long_username() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let long_username = "a".repeat(65);
    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": long_username, "password": "ValidPassw0rd"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("64") || msg.contains("exceed"),
        "Expected length error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_setup_invalid_chars_username() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "user name!", "password": "ValidPassw0rd"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("letters") || msg.contains("characters"),
        "Expected char validation error, got: {}",
        msg
    );
}

// ========================================================================
// 8. POST /api/auth/setup/verify-totp — pre-creation TOTP validation
// ========================================================================

// Deterministic 32-char RFC 4648 secret — decodes to exactly 20 bytes.
const TEST_TOTP_SECRET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32 decode of the same alphabet the wizard's client and
/// `AuthService::decode_base32` use (A-Z, 2-7; invalid chars skipped).
fn b32_decode(input: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u64;
    let mut bit_count = 0u32;
    let mut out = Vec::new();
    for c in input.trim_end_matches('=').to_uppercase().bytes() {
        let Some(idx) = ALPHABET.iter().position(|&a| a == c) else {
            continue;
        };
        bits = (bits << 5) | idx as u64;
        bit_count += 5;
        while bit_count >= 8 {
            bit_count -= 8;
            out.push(((bits >> bit_count) & 0xFF) as u8);
        }
    }
    out
}

/// 6-digit TOTP for this secret at the given UTC second — mirrors
/// `AuthService::generate_totp` exactly (RFC 6238, HMAC-SHA1, dynamic
/// truncation), so tests can produce codes the server actually accepts.
fn totp_at(secret: &str, at_secs: u64) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    use sha1::Sha1;
    type HmacSha1 = Hmac<Sha1>;

    let key = b32_decode(secret);
    let counter = (at_secs / 30).to_be_bytes();
    let mut mac = HmacSha1::new_from_slice(&key).expect("any key length is valid");
    mac.update(&counter);
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0F) as usize;
    let truncated = ((hash[offset] & 0x7F) as u32) << 24
        | (hash[offset + 1] as u32) << 16
        | (hash[offset + 2] as u32) << 8
        | (hash[offset + 3] as u32);
    format!("{:06}", truncated % 1_000_000)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[tokio::test]
async fn test_verify_totp_accepts_the_correct_code() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup/verify-totp",
        json!({
            "secret": TEST_TOTP_SECRET,
            "code": totp_at(TEST_TOTP_SECRET, now_secs()),
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]["valid"].as_bool().unwrap_or(false),
        "the current TOTP code must verify: {:?}",
        body
    );
}

#[tokio::test]
async fn test_verify_totp_rejects_a_wrong_code() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup/verify-totp",
        json!({ "secret": TEST_TOTP_SECRET, "code": "000000" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(!body["data"]["valid"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_verify_totp_is_disabled_once_setup_complete() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    // Complete setup first.
    let (status1, _) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;
    assert_eq!(status1, StatusCode::OK);

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup/verify-totp",
        json!({ "secret": TEST_TOTP_SECRET, "code": "000000" }),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("already"),
        "expected already-complete rejection, got: {}",
        msg
    );
}

// ========================================================================
// 9. POST /api/auth/setup — TOTP enrollment at creation
// ========================================================================

#[tokio::test]
async fn test_setup_with_invalid_totp_code_creates_no_user() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state.clone());

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "totp_secret": TEST_TOTP_SECRET,
            "totp_code": "000000",
        }),
    )
    .await;

    assert_ne!(status, StatusCode::OK, "invalid TOTP must reject setup");
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("two-factor") || msg.contains("verification"),
        "expected a TOTP rejection, got: {}",
        msg
    );

    // Nothing was persisted: the rejection happens before any row is created.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&*state.db.pool())
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

#[tokio::test]
async fn test_setup_with_valid_totp_enables_2fa_and_returns_backup_codes() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state.clone());

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "totp_secret": TEST_TOTP_SECRET,
            "totp_code": totp_at(TEST_TOTP_SECRET, now_secs()),
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "setup with a valid TOTP must succeed: {:?}",
        body
    );
    assert!(body["data"]["totp_enabled"].as_bool().unwrap_or(false));
    let backup_count = body["data"]["backup_codes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        backup_count >= 8,
        "expected ≥8 backup codes, got {}",
        backup_count
    );

    // The exact secret the admin scanned is stored **encrypted at rest**
    // (AES-256-GCM `hv1:` blob — never the raw base32 in the column),
    // enabled, with 8 codes; the plaintext only ever exists in memory, and
    // reads back identically through the repository.
    let (stored_secret, enabled): (String, bool) =
        sqlx::query_as("SELECT totp_secret_encrypted, enabled FROM user_2fa WHERE username = ?")
            .bind("firstadmin")
            .fetch_one(&*state.db.pool())
            .await
            .unwrap();
    assert_ne!(
        stored_secret,
        TEST_TOTP_SECRET.to_string(),
        "the TOTP secret must never hit the database in plaintext"
    );
    assert!(stored_secret.starts_with("hv1:"), "got: {stored_secret}");
    assert!(enabled);

    let repo = homevault::storage::User2faRepository::new(state.db.pool());
    let row = repo
        .get_by_username("firstadmin")
        .await
        .expect("2FA row")
        .expect("2FA row present");
    assert_eq!(row.totp_secret, TEST_TOTP_SECRET);
    assert!(row.enabled);
}

#[tokio::test]
async fn test_setup_ignores_lone_totp_secret_without_code() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state.clone());

    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({
            "username": "firstadmin",
            "password": "SecurePass123",
            "totp_secret": TEST_TOTP_SECRET,
        }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "a secret without a code must be ignored, not rejected: {:?}",
        body
    );
    assert!(!body["data"]["totp_enabled"].as_bool().unwrap_or(true));

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_2fa")
        .fetch_one(&*state.db.pool())
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}

// ========================================================================
// Concurrency — two setup requests racing must not both create an admin
// ========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_setup_only_one_succeeds() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = test_router(Arc::clone(&state));

    let fire = |username: &'static str| {
        let app = app.clone();
        tokio::spawn(async move {
            let req = Request::builder()
                .method(Method::POST)
                .uri("/api/auth/setup")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&json!({
                        "username": username,
                        "password": "ValidPassw0rd",
                    }))
                    .unwrap(),
                ))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
            (status, json)
        })
    };

    let (a, b) = tokio::join!(fire("adminone"), fire("admintwo"));
    let results = [a.unwrap(), b.unwrap()];

    let successes = results
        .iter()
        .filter(|(_, body)| body["success"].as_bool().unwrap_or(false))
        .count();
    assert_eq!(
        successes, 1,
        "exactly one concurrent setup call must succeed"
    );

    // The loser is told setup is already running / done, not a generic 500.
    let loser = results
        .iter()
        .find(|(_, body)| !body["success"].as_bool().unwrap_or(false))
        .expect("one call must fail");
    let msg = loser.1["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("in progress") || msg.contains("already complete"),
        "loser should get an in-progress/already-complete message, got: {}",
        msg
    );

    // Exactly one admin row exists.
    let admins: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&*state.db.pool())
        .await
        .unwrap();
    assert_eq!(admins.0, 1);
}

// ========================================================================
// 10. First-run state is derived from the database, not marker files
// ========================================================================

/// Insert an administrator row directly (role_id 2), with no marker files.
async fn insert_admin(state: &AppState, username: &str) {
    let hash = state.auth.hash_password("SecurePass123").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at)
         VALUES (?, ?, 'Admin', '', 2, false, '2026-01-01T00:00:00Z')",
    )
    .bind(username)
    .bind(&hash)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert admin");
}

#[tokio::test]
async fn test_setup_status_not_first_run_when_admin_exists_in_db_only() {
    // The old flag-file design answered "first run?" from `/etc/homevault`.
    // Losing that directory while the DB survived re-opened the Setup Wizard
    // even though an admin still existed. The DB is now authoritative.
    let (state, _tmp, _lock) = setup_state().await;
    insert_admin(&state, "dbboss").await;

    let mut app = test_router(state);
    let (_, body) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(
        !body["data"]["is_first_run"].as_bool().unwrap_or(true),
        "an admin row means the appliance is configured"
    );
}

#[tokio::test]
async fn test_setup_rejected_when_admin_exists_in_db_only() {
    let (state, _tmp, _lock) = setup_state().await;
    insert_admin(&state, "dbboss").await;

    let mut app = test_router(Arc::clone(&state));
    let (status, body) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "intruder", "password": "SecurePass123"}),
    )
    .await;

    assert_ne!(status, StatusCode::OK, "setup must refuse a second admin");
    assert!(
        body["message"].as_str().unwrap_or("").contains("already"),
        "expected an already-complete rejection, got: {:?}",
        body
    );

    // The second attempt persisted nothing.
    let users: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&*state.db.pool())
        .await
        .unwrap();
    assert_eq!(users.0, 1);
}

#[tokio::test]
async fn test_verify_totp_disabled_when_admin_exists_in_db_only() {
    let (state, _tmp, _lock) = setup_state().await;
    insert_admin(&state, "dbboss").await;

    let mut app = test_router(state);
    let (status, _) = post_json(
        &mut app,
        "/api/auth/setup/verify-totp",
        json!({ "secret": TEST_TOTP_SECRET, "code": "000000" }),
    )
    .await;

    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_setup_records_the_fingerprint_in_the_database() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(Arc::clone(&state));

    let (status, _) = post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let recorded = SetupStateRepository::new(state.db.pool())
        .get()
        .await
        .unwrap()
        .expect("setup state must be recorded");
    assert_eq!(
        recorded.system_hash,
        homevault::setup::SetupService::generate_system_hash(),
        "the recorded fingerprint must match this host"
    );
    assert!(!recorded.machine_id.is_empty());
}

// ========================================================================
// 11. POST /api/auth/setup/rebaseline — hardware-change recovery
// ========================================================================

fn rebaseline_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/setup-status", get(handle_setup_status))
        .route(
            "/api/auth/setup/rebaseline",
            axum::routing::post(handle_setup_rebaseline),
        )
        .with_state(state)
}

/// POST with an explicit `x-session-token` header (or none when empty).
async fn post_with_session(
    router: &mut Router,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("x-session-token", token);
    }
    let req = builder
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

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
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json)
}

#[tokio::test]
async fn test_rebaseline_requires_a_session() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = rebaseline_router(state);

    let (status, _) =
        post_with_session(&mut app, "/api/auth/setup/rebaseline", None, json!({})).await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_rebaseline_requires_admin_role() {
    let (state, _tmp, _lock) = setup_state().await;
    insert_user(&state, "plainuser").await;
    let session = state
        .auth
        .create_session("plainuser".into(), "127.0.0.1".into(), false)
        .await
        .unwrap();

    let mut app = rebaseline_router(state);
    let (status, _) = post_with_session(
        &mut app,
        "/api/auth/setup/rebaseline",
        Some(&session.token),
        json!({}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "a non-admin must not re-baseline");
}

#[tokio::test]
async fn test_rebaseline_clears_needs_recovery_for_admin() {
    let (state, _tmp, _lock) = setup_state().await;
    insert_admin(&state, "recoverboss").await;
    SetupStateRepository::new(state.db.pool())
        .mark_complete("stale-fingerprint", "old-machine")
        .await
        .unwrap();
    let session = state
        .auth
        .create_session("recoverboss".into(), "127.0.0.1".into(), false)
        .await
        .unwrap();

    let mut app = rebaseline_router(Arc::clone(&state));

    let (_, before) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(
        before["data"]["needs_recovery"].as_bool().unwrap_or(false),
        "a stale fingerprint on a configured appliance needs recovery"
    );

    let (status, body) = post_with_session(
        &mut app,
        "/api/auth/setup/rebaseline",
        Some(&session.token),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rebaseline failed: {:?}", body);

    let (_, after) = get_json(&mut app, "/api/auth/setup-status").await;
    assert!(!after["data"]["needs_recovery"].as_bool().unwrap_or(true));
    assert!(!after["data"]["hardware_changed"].as_bool().unwrap_or(true));

    assert_eq!(
        SetupStateRepository::new(state.db.pool())
            .get()
            .await
            .unwrap()
            .unwrap()
            .system_hash,
        homevault::setup::SetupService::generate_system_hash()
    );
}

/// Insert a regular (non-admin) user row.
async fn insert_user(state: &AppState, username: &str) {
    let hash = state.auth.hash_password("SecurePass123").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at)
         VALUES (?, ?, 'User', '', 1, false, '2026-01-01T00:00:00Z')",
    )
    .bind(username)
    .bind(&hash)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert user");
}

// ========================================================================
// 12. Local-network guard on the public bootstrap endpoints
// ========================================================================

fn peer(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse().expect("valid IP"), 54321)
}

/// Drive the real production router (`create_app`) as if the request arrived
/// from `ip`. `oneshot` does not go through a socket, so the connection
/// metadata is injected the same way `into_make_service_with_connect_info`
/// would.
async fn post_as_peer(app: &Router, ip: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(peer(ip)));

    let mut app = app.clone();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json)
}

async fn get_as_peer(app: &Router, ip: &str, path: &str) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(peer(ip)));
    let mut app = app.clone();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json)
}

#[tokio::test]
async fn test_setup_guard_blocks_setup_from_a_public_peer() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer(
        &app,
        "8.8.8.8",
        "/api/auth/setup",
        json!({"username": "attacker", "password": "SecurePass123"}),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a public peer must not reach first-run setup: {:?}",
        body
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("local network")
    );
}

// ------------------------------------------------------------------------
// Same-origin guard: the CORS layer is permissive, so a cross-site page in the
// operator's own browser must not be able to claim the first admin.
// ------------------------------------------------------------------------

/// Drive the real router with arbitrary extra headers (same-origin probes).
async fn post_as_peer_with_headers(
    app: &Router,
    ip: &str,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut req = builder
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(peer(ip)));

    let mut app = app.clone();
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json)
}

/// A LAN peer whose browser was lured into a cross-site POST is refused, even
/// though the peer address itself is allowed.
#[tokio::test]
async fn test_setup_guard_rejects_a_cross_site_origin() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer_with_headers(
        &app,
        "192.168.1.30",
        "/api/auth/setup",
        json!({"username": "attacker", "password": "SecurePass123"}),
        &[
            ("host", "homevault.local:8080"),
            ("origin", "https://evil.example"),
            ("sec-fetch-site", "cross-site"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("own interface")
    );
}

/// `Sec-Fetch-Site: cross-site` alone (no Origin) is enough to refuse.
#[tokio::test]
async fn test_setup_guard_rejects_cross_site_fetch_metadata() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer_with_headers(
        &app,
        "192.168.1.30",
        "/api/auth/setup",
        json!({"username": "attacker", "password": "SecurePass123"}),
        &[("sec-fetch-site", "cross-site")],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// `Origin: null` (sandboxed iframe / file://) can never be the appliance.
#[tokio::test]
async fn test_setup_guard_rejects_a_null_origin() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer_with_headers(
        &app,
        "192.168.1.30",
        "/api/auth/setup",
        json!({"username": "attacker", "password": "SecurePass123"}),
        &[("origin", "null")],
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// The appliance's own UI (same Origin/Host, or a plain non-browser client
/// sending neither header) still reaches the handler.
#[tokio::test]
async fn test_setup_guard_allows_a_same_origin_request() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer_with_headers(
        &app,
        "192.168.1.30",
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
        &[
            ("host", "homevault.local:8080"),
            ("origin", "http://homevault.local:8080"),
            ("sec-fetch-site", "same-origin"),
        ],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["username"], json!("firstadmin"));
}

/// Over HTTP/2 (the default when built-in TLS is enabled on `:8443`), browsers
/// send the `:authority` pseudo-header (`request.uri().authority()`) rather
/// than an HTTP/1.1 `Host` header. Same-origin verification must accept a
/// matching `:authority` and still reject a mismatched `Origin`.
#[tokio::test]
async fn test_setup_guard_allows_http2_authority_without_host_header() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let mut bad_req = Request::builder()
        .method(Method::POST)
        .uri("https://homevault.local:8443/api/auth/setup")
        .header("content-type", "application/json")
        .header("origin", "https://evil.example")
        .header("sec-fetch-site", "same-origin")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "username": "attacker",
                "password": "SecurePass123"
            }))
            .unwrap(),
        ))
        .unwrap();
    bad_req
        .extensions_mut()
        .insert(ConnectInfo(peer("192.168.1.30")));
    let mut app_bad = app.clone();
    let bad_resp = ServiceExt::<Req>::ready(&mut app_bad)
        .await
        .unwrap()
        .call(bad_req)
        .await
        .unwrap();
    assert_eq!(bad_resp.status(), StatusCode::FORBIDDEN);

    let mut ok_req = Request::builder()
        .method(Method::POST)
        .uri("https://homevault.local:8443/api/auth/setup")
        .header("content-type", "application/json")
        .header("origin", "https://homevault.local:8443")
        .header("sec-fetch-site", "same-origin")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "username": "h2admin",
                "password": "SecurePass123"
            }))
            .unwrap(),
        ))
        .unwrap();
    ok_req
        .extensions_mut()
        .insert(ConnectInfo(peer("192.168.1.30")));
    let mut app_ok = app.clone();
    let ok_resp = ServiceExt::<Req>::ready(&mut app_ok)
        .await
        .unwrap()
        .call(ok_req)
        .await
        .unwrap();
    assert_eq!(ok_resp.status(), StatusCode::OK);
}

/// A bare non-browser client (no Origin, no Sec-Fetch-Site) is still allowed.
#[tokio::test]
async fn test_setup_guard_allows_a_header_less_client() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer(
        &app,
        "192.168.1.30",
        "/api/auth/setup",
        json!({"username": "scriptadmin", "password": "SecurePass123"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The bootstrap routes are rate-limited per peer: a flood gets 429 rather
/// than hammering the handlers.
#[tokio::test]
async fn test_setup_bootstrap_endpoints_are_rate_limited() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    // `check-username` is the cheapest bootstrap route and hits no DB write.
    let mut saw_429 = false;
    for _ in 0..40 {
        let (status, _) = get_as_peer(
            &app,
            "203.0.113.9",
            "/api/auth/check-username?username=probe",
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
    }
    assert!(saw_429, "the bootstrap routes must rate-limit per peer");
}

#[tokio::test]
async fn test_setup_progress_does_not_consume_the_bootstrap_budget() {
    // Regression: the wizard PUTs `/auth/setup/progress` on every step
    // navigation. While that route shared the 20/min bootstrap limiter, a
    // handful of step round-trips exhausted it and the next `check-username`
    // came back 429 — which the UI then read as "username already taken" and
    // refused to leave the account step (the E2E suite hit this reliably).
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);
    // A LAN peer: the bootstrap routes must be reachable from a private
    // address for the budget question to even arise.
    let peer_ip = "192.168.1.50";

    // Well past the 20/min budget, all through the exempt route.
    for step in 0..30 {
        let body = json!({ "step": step % 3, "data": {} });
        let mut req = Request::builder()
            .method(Method::PUT)
            .uri("/api/auth/setup/progress")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(peer(peer_ip)));
        let mut app_clone = app.clone();
        let resp = ServiceExt::<Req>::ready(&mut app_clone)
            .await
            .unwrap()
            .call(req)
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "setup progress must never be rate-limited by the bootstrap budget"
        );
    }

    // The metered route still answers normally right after.
    let (status, body) = get_as_peer(
        &app,
        peer_ip,
        "/api/auth/check-username?username=freshadmin",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "check-username must still work after the wizard's progress writes: {:?}",
        body
    );
    assert!(body["data"]["available"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_setup_guard_blocks_check_username_from_a_public_peer() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, _) = get_as_peer(
        &app,
        "203.0.113.7",
        "/api/auth/check-username?username=boss",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_setup_guard_blocks_verify_totp_from_a_public_peer() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, _) = post_as_peer(
        &app,
        "198.51.100.4",
        "/api/auth/setup/verify-totp",
        json!({ "secret": TEST_TOTP_SECRET, "code": "000000" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_setup_guard_allows_loopback_and_reaches_the_handler() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, body) = post_as_peer(
        &app,
        "127.0.0.1",
        "/api/auth/setup",
        json!({"username": "localboss", "password": "SecurePass123"}),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::OK,
        "loopback setup must succeed: {:?}",
        body
    );
    assert!(body["success"].as_bool().unwrap_or(false));
}

#[tokio::test]
async fn test_setup_guard_allows_a_private_lan_peer() {
    let (state, _tmp, _lock) = setup_state().await;
    let app = create_app(state);

    let (status, _) = get_as_peer(
        &app,
        "192.168.1.42",
        "/api/auth/check-username?username=boss",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_setup_guard_env_override_allows_a_public_peer() {
    let (state, _tmp, _lock) = setup_state().await;
    std::env::set_var("HOMEVAULT_SETUP_ALLOW_REMOTE", "1");
    let app = create_app(state);

    let (status, _) = get_as_peer(&app, "8.8.8.8", "/api/auth/check-username?username=boss").await;

    std::env::remove_var("HOMEVAULT_SETUP_ALLOW_REMOTE");
    assert_eq!(
        status,
        StatusCode::OK,
        "the documented opt-out must lift the restriction"
    );
}

// ========================================================================
// Setup progress (reboot-safe onboarding): GET/PUT /api/auth/setup/progress
// ========================================================================

/// Fresh first run reports step 0 with empty choices.
#[tokio::test]
async fn test_setup_progress_defaults_to_step_zero() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = get_json(&mut app, "/api/auth/setup/progress").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["step"], json!(0));
    assert_eq!(body["data"]["data"], json!({}));
}

/// PUT persists step + choices; a later GET resumes them (the reboot path).
#[tokio::test]
async fn test_setup_progress_save_and_resume() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = put_json(
        &mut app,
        "/api/auth/setup/progress",
        json!({"step": 3, "data": {"language": "fr"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["step"], json!(3));

    let (status, body) = get_json(&mut app, "/api/auth/setup/progress").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["step"], json!(3));
    assert_eq!(body["data"]["data"]["language"], json!("fr"));
}

/// Out-of-range steps and non-object payloads are rejected, never stored.
#[tokio::test]
async fn test_setup_progress_rejects_bad_step_and_shape() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    for bad in [
        json!({"step": 99, "data": {}}),
        json!({"step": -1, "data": {}}),
        json!({"step": 2, "data": [1, 2]}),
        json!({"step": 2, "data": "fr"}),
    ] {
        let (status, body) = put_json(&mut app, "/api/auth/setup/progress", bad).await;
        assert_ne!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], json!(false));
    }

    // Nothing from the rejected writes may have stuck.
    let (_, body) = get_json(&mut app, "/api/auth/setup/progress").await;
    assert_eq!(body["data"]["step"], json!(0));
}

/// Once an admin exists the progress endpoints close (setup is over).
#[tokio::test]
async fn test_setup_progress_forbidden_after_setup() {
    let (state, _tmp, _lock) = setup_state().await;
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES ('boss', 'x', 'Boss', 'b@example.com', 2, false, datetime('now'), 's')",
    )
    .execute(&*state.db.pool())
    .await
    .expect("insert admin");
    let mut app = test_router(state);

    let (status, _) = get_json(&mut app, "/api/auth/setup/progress").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = put_json(
        &mut app,
        "/api/auth/setup/progress",
        json!({"step": 2, "data": {}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}
