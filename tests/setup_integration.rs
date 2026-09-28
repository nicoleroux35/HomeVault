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

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
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
    auth::{
        handle_check_username, handle_setup, handle_setup_status, handle_setup_verify_totp,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

type Req = Request<axum::body::Body>;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Build a fresh in-memory database with all migrations applied.
async fn setup_db() -> Database {
    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations()
        .await
        .expect("Failed to run migrations");
    db
}

/// Build a minimal AppState with a temp DATA_DIRECTORY for lock/flag files.
async fn setup_state() -> (Arc<AppState>, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
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

/// Build a test router with only the auth/setup endpoints.
fn test_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/setup-status", get(handle_setup_status))
        .route("/api/auth/check-username", get(handle_check_username))
        .route(
            "/api/auth/setup",
            axum::routing::post(handle_setup),
        )
        .route(
            "/api/auth/setup/verify-totp",
            axum::routing::post(handle_setup_verify_totp),
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
    let json: Value = serde_json::from_slice(&body)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&body)}));
    (status, json)
}

/// Helper: send a POST request with JSON body and return (status, body JSON).
async fn post_json(
    router: &mut Router,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&body).unwrap(),
        ))
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

// ========================================================================
// 1. GET /api/auth/setup-status
// ========================================================================

#[tokio::test]
async fn test_setup_status_first_run() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) =
        get_json(&mut app, "/api/auth/setup-status").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    let data = &body["data"];
    assert_eq!(data["is_first_run"].as_bool().unwrap(), true);
    assert_eq!(data["is_setup_valid"].as_bool().unwrap(), false);
}

#[tokio::test]
async fn test_setup_status_has_sudo_available_field() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) =
        get_json(&mut app, "/api/auth/setup-status").await;
    let data = &body["data"];
    assert!(
        data.get("sudo_available").is_some(),
        "sudo_available should be present"
    );
}

#[tokio::test]
async fn test_setup_status_has_machine_id() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) =
        get_json(&mut app, "/api/auth/setup-status").await;
    let data = &body["data"];
    let machine_id = data["server_machine_id"].as_str().unwrap_or("");
    assert!(
        !machine_id.is_empty(),
        "machine_id should not be empty"
    );
}

// ========================================================================
// 2. GET /api/auth/check-username
// ========================================================================

#[tokio::test]
async fn test_check_username_available() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=newuser",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false));
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        true
    );
}

#[tokio::test]
async fn test_check_username_reserved_root() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (status, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=root",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        false
    );
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

    let (_, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=admin",
    )
    .await;
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        false
    );
}

#[tokio::test]
async fn test_check_username_too_short() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=ab",
    )
    .await;
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        false
    );
}

#[tokio::test]
async fn test_check_username_invalid_chars() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    let (_, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=user+name",
    )
    .await;
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        false
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
        json!({"username": "", "password": "ValidPass1"}),
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
        json!({"username": "root", "password": "ValidPass1"}),
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
        json!({"username": "testuser", "password": "lowercase1"}),
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
    assert!(
        msg.contains("digit"),
        "Expected digit error, got: {}",
        msg
    );
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
        msg.contains("8 characters"),
        "Expected length error, got: {}",
        msg
    );
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

    assert_eq!(
        status,
        StatusCode::OK,
        "Setup should succeed: {:?}",
        body
    );
    assert!(body["success"].as_bool().unwrap_or(false));

    let data = &body["data"];
    assert!(
        data["token"].as_str().unwrap_or("").len() > 0,
        "Should return a session token"
    );
    assert_eq!(
        data["username"].as_str().unwrap_or(""),
        "firstadmin"
    );
    assert_eq!(
        data["is_admin"].as_bool().unwrap_or(false),
        true
    );
}

#[tokio::test]
async fn test_setup_marks_first_run_false() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(Arc::clone(&state));

    // Before setup
    let (_, before) =
        get_json(&mut app, "/api/auth/setup-status").await;
    assert_eq!(
        before["data"]["is_first_run"].as_bool().unwrap(),
        true
    );

    // Do setup
    post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;

    // After setup — same state, same in-memory DB
    let mut app2 = test_router(state);
    let (_, after) =
        get_json(&mut app2, "/api/auth/setup-status").await;
    assert_eq!(
        after["data"]["is_first_run"].as_bool().unwrap(),
        false
    );
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

#[tokio::test]
async fn test_check_username_taken_after_setup() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    // Setup creates "firstadmin"
    post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;

    // Check if "firstadmin" is now taken
    let (_, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=firstadmin",
    )
    .await;
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        false
    );
    let reason = body["data"]["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("already") || reason.contains("exists"),
        "Expected taken/exists reason, got: {}",
        reason
    );
}

#[tokio::test]
async fn test_check_username_different_one_still_available() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    post_json(
        &mut app,
        "/api/auth/setup",
        json!({"username": "firstadmin", "password": "SecurePass123"}),
    )
    .await;

    let (_, body) = get_json(
        &mut app,
        "/api/auth/check-username?username=newuser",
    )
    .await;
    assert_eq!(
        body["data"]["available"].as_bool().unwrap(),
        true
    );
}

// ========================================================================
// 6. Full flow: status → setup → check → login ready
// ========================================================================

#[tokio::test]
async fn test_full_setup_flow() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = test_router(state);

    // Step 1: Check status — should be first run
    let (_, status_body) =
        get_json(&mut app, "/api/auth/setup-status").await;
    assert_eq!(
        status_body["data"]["is_first_run"].as_bool().unwrap(),
        true
    );

    // Step 2: Check username availability
    let (_, check_body) = get_json(
        &mut app,
        "/api/auth/check-username?username=firstadmin",
    )
    .await;
    assert_eq!(
        check_body["data"]["available"].as_bool().unwrap(),
        true
    );

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

    // Step 4: Verify username is now taken
    let (_, taken_body) = get_json(
        &mut app,
        "/api/auth/check-username?username=firstadmin",
    )
    .await;
    assert_eq!(
        taken_body["data"]["available"].as_bool().unwrap(),
        false
    );

    // Step 5: Verify setup status is no longer first run
    let (_, final_status) =
        get_json(&mut app, "/api/auth/setup-status").await;
    assert_eq!(
        final_status["data"]["is_first_run"].as_bool().unwrap(),
        false
    );
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
        json!({"username": long_username, "password": "ValidPass1"}),
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
        json!({"username": "user name!", "password": "ValidPass1"}),
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
    use hmac::{Hmac, Mac, KeyInit};
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
    assert_eq!(
        body["data"]["valid"].as_bool().unwrap_or(false),
        true,
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
    assert_eq!(body["data"]["valid"].as_bool().unwrap_or(true), false);
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
    assert_eq!(body["data"]["totp_enabled"].as_bool().unwrap_or(false), true);
    let backup_count = body["data"]["backup_codes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(backup_count >= 8, "expected ≥8 backup codes, got {}", backup_count);

    // The exact secret the admin scanned is stored, enabled, with 8 codes.
    let (stored_secret, enabled): (String, bool) =
        sqlx::query_as("SELECT totp_secret_encrypted, enabled FROM user_2fa WHERE username = ?")
            .bind("firstadmin")
            .fetch_one(&*state.db.pool())
            .await
            .unwrap();
    assert_eq!(stored_secret, TEST_TOTP_SECRET.to_string());
    assert!(enabled);
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
    assert_eq!(body["data"]["totp_enabled"].as_bool().unwrap_or(true), false);

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
                        "password": "ValidPass1",
                    }))
                    .unwrap(),
                ))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
    assert_eq!(successes, 1, "exactly one concurrent setup call must succeed");

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
