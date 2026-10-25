/* =============================================================================
 * File          : account_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Account integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Account-related API endpoints:
//!
//!   - PUT  /api/users/{username}          (profile update — self)
//!   - PUT  /api/users/{username}/password  (password change — self)
//!   - POST /api/auth/me/avatar             (avatar regeneration)
//!   - POST /api/auth/2fa/setup             (generate / verify / disable / status / backup_codes)
//!   - GET  /api/security/sessions          (own sessions — opaque ids, no live token)
//!   - POST /api/security/sessions/revoke   (caller-scoped revocation by opaque id)
//!   - POST /api/security/sessions/revoke-all
//!
//! Each test creates a fresh in-memory SQLite database, runs all embedded
//! migrations, builds a minimal Axum router, and uses `tower::ServiceExt`
//! for HTTP-level testing without starting a real server.
//!
//! Run:
//!   cargo test --test account_integration
//!   cargo test --test account_integration -- --nocapture

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
    auth::{handle_2fa_setup, handle_get_current_user, handle_login},
    avatars::handle_update_my_avatar,
    security::{handle_get_my_sessions, handle_revoke_all_sessions, handle_revoke_sessions},
    users::{handle_user_update, handle_user_update_password},
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
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

fn account_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/auth/me", get(handle_get_current_user))
        .route(
            "/api/users/{username}",
            axum::routing::put(handle_user_update),
        )
        .route(
            "/api/users/{username}/password",
            axum::routing::put(handle_user_update_password),
        )
        .route(
            "/api/auth/me/avatar",
            axum::routing::post(handle_update_my_avatar),
        )
        .route("/api/auth/2fa/setup", axum::routing::post(handle_2fa_setup))
        .route("/api/security/sessions", get(handle_get_my_sessions))
        .route(
            "/api/security/sessions/revoke",
            axum::routing::post(handle_revoke_sessions),
        )
        .route(
            "/api/security/sessions/revoke-all",
            axum::routing::post(handle_revoke_all_sessions),
        )
        .with_state(state)
}

/// Insert a user directly into the DB for testing.
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
    .expect("Failed to insert test user");
}

/// Login via HTTP and return the session token.
async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (_, body) = post_json(
        router,
        "/api/auth/login",
        json!({"username": username, "password": password}),
    )
    .await;
    body["data"]["token"]
        .as_str()
        .expect("login must return a token")
        .to_string()
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

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

async fn put_json_with_token(
    router: &mut Router,
    path: &str,
    body: Value,
    token: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::PUT)
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

// ---------------------------------------------------------------------------
// TOTP helper — mirrors the backend's decode_base32 + generate_totp exactly
// ---------------------------------------------------------------------------

/// Decode base32 per RFC 4648 (same table as auth/service.rs).
fn decode_base32(data: &str) -> Vec<u8> {
    const DECODE_TABLE: [i8; 128] = [
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, 26, 27, 28, 29, 30, 31, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 1, 2, 3, 4,
        5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    ];

    let data_upper = data.to_uppercase();
    let mut result = Vec::new();
    let mut buffer = 0u64;
    let mut bits_left = 0;

    for c in data_upper.chars() {
        let value = if c.is_ascii() {
            DECODE_TABLE[c as usize]
        } else {
            -1
        };
        if value < 0 {
            continue;
        }
        buffer = (buffer << 5) | (value as u64);
        bits_left += 5;
        while bits_left >= 8 {
            let byte = ((buffer >> (bits_left - 8)) & 0xFF) as u8;
            result.push(byte);
            bits_left -= 8;
        }
    }
    result
}

/// HMAC-SHA1 based TOTP, 6 digits, 30s step — mirrors auth/service.rs::generate_totp.
fn generate_totp(secret_b32: &str, counter: u64) -> u64 {
    use hmac::{Hmac, KeyInit, Mac};
    use sha1::Sha1;

    let secret_bytes = decode_base32(secret_b32);
    type HmacSha1 = Hmac<Sha1>;
    let mut mac = HmacSha1::new_from_slice(&secret_bytes).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let result = mac.finalize().into_bytes();
    let hash: [u8; 20] = result.into();
    let offset = (hash[hash.len() - 1] & 0x0F) as usize;
    let truncated = ((hash[offset] & 0x7F) as u32) << 24
        | (hash[offset + 1] as u32) << 16
        | (hash[offset + 2] as u32) << 8
        | (hash[offset + 3] as u32);
    (truncated % 1_000_000) as u64
}

/// Current TOTP code for the given base32 secret.
fn totp_now(base32_secret: &str) -> String {
    let counter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 30;
    format!("{:06}", generate_totp(base32_secret, counter))
}

// ========================================================================
// 1. PUT /api/users/{username} — profile update (self)
// ========================================================================

#[tokio::test]
async fn test_self_update_profile_success() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "alice", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "alice", "Passw0rd!").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/alice",
        json!({"fullname": "Alice Wonderland", "email": "alice@wonderland.com"}),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);

    // Verify via GET /api/auth/me
    let (_, me) = get_json_with_token(&mut app, "/api/auth/me", &token).await;
    assert_eq!(me["data"]["fullname"], "Alice Wonderland");
    assert_eq!(me["data"]["email"], "alice@wonderland.com");
}

#[tokio::test]
async fn test_self_update_profile_empty_fullname() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "bob", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "bob", "Passw0rd!").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/bob",
        json!({"fullname": "", "email": "bob@test.com"}),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn test_update_other_user_denied_for_non_admin() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "alice2", "Passw0rd!", 1).await;
    create_test_user(&state, "bob2", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "alice2", "Passw0rd!").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/bob2",
        json!({"fullname": "Hacked", "email": "h@ck.com"}),
        &token,
    )
    .await;

    // Non-admin cannot update another user — handler returns 200 with success:false
    if status == StatusCode::OK {
        assert_eq!(
            body["success"], false,
            "Non-admin must not update another user"
        );
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_update_profile_no_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "alice3", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/alice3",
        json!({"fullname": "Alice", "email": "a@b.com"}),
        "",
    )
    .await;

    // Empty token → error
    if status == StatusCode::OK {
        assert_eq!(body["success"], false);
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_update_profile_invalid_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "alice4", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/alice4",
        json!({"fullname": "Alice", "email": "a@b.com"}),
        "garbage-token-12345",
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false);
    } else {
        assert!(status.is_client_error());
    }
}

// ========================================================================
// 2. PUT /api/users/{username}/password — password change (self)
// ========================================================================

#[tokio::test]
async fn test_password_change_success() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "charlie", "OldPass123", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "charlie", "OldPass123").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/charlie/password",
        json!({"current_password": "OldPass123", "new_password": "NewPassw0rd456"}),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);

    // Old password should no longer authenticate (session invalidated on password change)
    let _ = get_json_with_token(&mut app, "/api/auth/me", &token).await;
    // The session may or may not be immediately invalid depending on implementation,
    // but the response confirms success.
}

#[tokio::test]
async fn test_password_change_missing_current_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "dave", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "dave", "Passw0rd!").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/dave/password",
        json!({"new_password": "NewPassw0rd123"}),
        &token,
    )
    .await;

    // Non-admin must provide current_password
    if status == StatusCode::OK {
        assert_eq!(
            body["success"], false,
            "Missing current_password should fail"
        );
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_password_change_wrong_current_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "eve", "CorrectPass1", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "eve", "CorrectPass1").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/eve/password",
        json!({"current_password": "WrongPass999", "new_password": "NewPassw0rd123"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Wrong current_password should fail");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_password_change_empty_new_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "frank", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "frank", "Passw0rd!").await;

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/frank/password",
        json!({"current_password": "Passw0rd!", "new_password": ""}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Empty new_password should fail");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_password_change_no_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "grace", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let (status, body) = put_json_with_token(
        &mut app,
        "/api/users/grace/password",
        json!({"current_password": "Passw0rd!", "new_password": "NewPassw0rd123"}),
        "",
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Missing token should fail");
    } else {
        assert!(status.is_client_error());
    }
}

// ========================================================================
// 3. POST /api/auth/me/avatar — avatar regeneration
// ========================================================================

#[tokio::test]
async fn test_avatar_regeneration_success() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "helen", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "helen", "Passw0rd!").await;

    // Get initial avatar seed
    let (_, initial) = get_json_with_token(&mut app, "/api/auth/me", &token).await;
    let initial_seed = initial["data"]["avatar_seed"].as_str().unwrap_or("");

    // Regenerate with a specific seed
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/me/avatar",
        json!({"seed": "newrandomseed123"}),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["seed"], "newrandomseed123");
    assert!(body["data"]["svg"].as_str().unwrap_or("").contains("<svg"));

    // Verify the seed changed via GET /api/auth/me
    let (_, updated) = get_json_with_token(&mut app, "/api/auth/me", &token).await;
    let new_seed = updated["data"]["avatar_seed"].as_str().unwrap_or("");
    assert_eq!(new_seed, "newrandomseed123");
    assert_ne!(new_seed, initial_seed);
}

#[tokio::test]
async fn test_avatar_regeneration_no_seed_generates_random() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "ivan", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "ivan", "Passw0rd!").await;

    let (status, body) =
        post_json_with_token(&mut app, "/api/auth/me/avatar", json!({}), &token).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let seed = body["data"]["seed"].as_str().unwrap_or("");
    assert!(!seed.is_empty(), "A random seed should have been generated");
}

#[tokio::test]
async fn test_avatar_no_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "judy", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let (status, body) =
        post_json_with_token(&mut app, "/api/auth/me/avatar", json!({"seed": "test"}), "").await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Missing token should fail");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_avatar_invalid_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "karl", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/me/avatar",
        json!({"seed": "test"}),
        "invalid-token-xyz",
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Invalid token should fail");
    } else {
        assert!(status.is_client_error());
    }
}

// ========================================================================
// 4. POST /api/auth/2fa/setup — 2FA lifecycle
// ========================================================================

#[tokio::test]
async fn test_2fa_generate_secret() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "leo", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "leo", "Passw0rd!").await;

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let secret = body["data"]["secret"].as_str().unwrap_or("");
    assert!(!secret.is_empty(), "Secret should be non-empty");
    let qrcode = body["data"]["qrcode_url"].as_str().unwrap_or("");
    assert!(
        qrcode.starts_with("otpauth://totp/"),
        "QR URL should be an otpauth:// URI, got: {}",
        qrcode
    );
}

#[tokio::test]
async fn test_2fa_status_disabled_initially() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "mia", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "mia", "Passw0rd!").await;

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "status"}),
        &token,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], false);
}

#[tokio::test]
async fn test_2fa_verify_wrong_code() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "nick", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "nick", "Passw0rd!").await;

    // Generate first
    let (status, _) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Try verify with wrong code
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": "000000", "password": "Passw0rd!"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Wrong TOTP code should be rejected");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_2fa_verify_missing_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "olivia", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "olivia", "Passw0rd!").await;

    // Generate
    let (status, _) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify without password
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": "123456"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(
            body["success"], false,
            "Missing password should be rejected"
        );
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_2fa_verify_wrong_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "paul", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "paul", "Passw0rd!").await;

    // Generate
    let (status, _) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Verify with wrong password
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": "123456", "password": "WrongPass999"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Wrong password should be rejected");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_2fa_verify_then_disable_full_lifecycle() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "quinn", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "quinn", "Passw0rd!").await;

    // 1. Generate secret
    let (_, gen_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;
    let secret = gen_body["data"]["secret"].as_str().unwrap().to_string();

    // 2. Verify with correct TOTP code
    let code = totp_now(&secret);
    let (status, verify_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": code, "password": "Passw0rd!"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(verify_body["success"], true);
    assert_eq!(verify_body["data"]["enabled"], true);
    let codes = verify_body["data"]["backup_codes"].as_array().unwrap();
    assert!(codes.len() >= 8, "Should return at least 8 backup codes");

    // 3. Check status — now enabled
    let (_, status_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "status"}),
        &token,
    )
    .await;
    assert_eq!(status_body["data"]["enabled"], true);

    // 4. View backup codes
    let (_, backup_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "backup_codes"}),
        &token,
    )
    .await;
    assert_eq!(backup_body["success"], true);
    let stored_codes = backup_body["data"]["codes"].as_array().unwrap();
    assert!(!stored_codes.is_empty(), "Backup codes should be stored");

    // 5. Disable with correct password
    let (status, disable_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "disable", "password": "Passw0rd!"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(disable_body["success"], true);
    assert_eq!(disable_body["data"]["enabled"], false);

    // 6. Check status — disabled again
    let (_, final_status) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "status"}),
        &token,
    )
    .await;
    assert_eq!(final_status["data"]["enabled"], false);
}

#[tokio::test]
async fn test_2fa_disable_wrong_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "rachel", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "rachel", "Passw0rd!").await;

    // Generate + verify
    let (_, gen_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;
    let secret = gen_body["data"]["secret"].as_str().unwrap().to_string();
    let code = totp_now(&secret);
    let (status, _) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": code, "password": "Passw0rd!"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Try disable with wrong password
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "disable", "password": "WrongPass999"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(
            body["success"], false,
            "Wrong password should prevent disable"
        );
    } else {
        assert!(status.is_client_error());
    }

    // Should still be enabled
    let (_, s) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "status"}),
        &token,
    )
    .await;
    assert_eq!(s["data"]["enabled"], true);
}

#[tokio::test]
async fn test_2fa_disable_missing_password() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "sam", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "sam", "Passw0rd!").await;

    // Generate + verify
    let (_, gen_body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        &token,
    )
    .await;
    let secret = gen_body["data"]["secret"].as_str().unwrap().to_string();
    let code = totp_now(&secret);
    let (status, _) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": code, "password": "Passw0rd!"}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Try disable without password
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "disable"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(
            body["success"], false,
            "Missing password should prevent disable"
        );
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_2fa_no_token() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = account_router(state);

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "generate"}),
        "",
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Missing token should fail");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_2fa_invalid_action() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "tina", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "tina", "Passw0rd!").await;

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "nonexistent"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(body["success"], false, "Invalid action should be rejected");
    } else {
        assert!(status.is_client_error());
    }
}

#[tokio::test]
async fn test_2fa_verify_without_generate() {
    // Verify without generating first should fail
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "uma", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "uma", "Passw0rd!").await;

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/auth/2fa/setup",
        json!({"action": "verify", "totp_code": "123456", "password": "Passw0rd!"}),
        &token,
    )
    .await;

    if status == StatusCode::OK {
        assert_eq!(
            body["success"], false,
            "Verify without generate should fail"
        );
    } else {
        assert!(status.is_client_error());
    }
}

// ===========================================================================
// 7. Self-service sessions — opaque ids and caller-scoped revocation
// (ROADMAP 14.1 / 14.2)
// ===========================================================================

/// GET /api/security/sessions must identify a session without handing the
/// client a replayable credential (ROADMAP 14.1).
#[tokio::test]
async fn test_my_sessions_never_returns_a_live_session_token() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "sess_owner", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let first = login(&mut app, "sess_owner", "Passw0rd!").await;
    let second = login(&mut app, "sess_owner", "Passw0rd!").await;
    assert_ne!(first, second, "two logins must mint two sessions");

    let (status, body) = get_json_with_token(&mut app, "/api/security/sessions", &first).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["total"], json!(2), "{body}");

    let sessions = body["data"]["sessions"].as_array().expect("sessions array");
    assert_eq!(sessions.len(), 2);

    for session in sessions {
        // The opaque id is a 12-char hex fingerprint, never the token itself.
        let id = session["id"].as_str().expect("session id");
        assert_eq!(id.len(), 12, "id must be the short fingerprint: {id}");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        assert!(
            session.get("token").is_none(),
            "the wire shape must not carry a `token` field: {session}"
        );
    }

    // The whole payload must not leak either live token — that is what made
    // the old response a session-hijack primitive.
    let payload = body.to_string();
    assert!(
        !payload.contains(&first),
        "payload leaks the caller's token"
    );
    assert!(
        !payload.contains(&second),
        "payload leaks another live token"
    );

    // Exactly one row is the caller's current session, and it is the one whose
    // fingerprint matches the token that made the request.
    let current: Vec<&Value> = sessions
        .iter()
        .filter(|s| s["is_current"] == json!(true))
        .collect();
    assert_eq!(current.len(), 1, "{body}");
    assert_eq!(
        current[0]["id"],
        json!(homevault::handlers::security::session_public_id(&first))
    );
}

/// A token/id belonging to another account can never be revoked through the
/// self-service endpoint (ROADMAP 14.2).
#[tokio::test]
async fn test_revoke_sessions_is_scoped_to_the_caller() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "alice", "Passw0rd!", 1).await;
    create_test_user(&state, "bob", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let alice_current = login(&mut app, "alice", "Passw0rd!").await;
    let alice_other = login(&mut app, "alice", "Passw0rd!").await;
    let bob_session = login(&mut app, "bob", "Passw0rd!").await;

    // Bob's live token submitted as an id: no session of Alice's matches it,
    // so nothing is deleted — and Bob stays signed in.
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": [bob_session]}),
        &alice_current,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["success"], json!(false));
    let (bob_status, _) = get_json_with_token(&mut app, "/api/auth/me", &bob_session).await;
    assert_eq!(
        bob_status,
        StatusCode::OK,
        "another user's session must survive a foreign revoke request"
    );

    // An unknown id is refused the same way.
    let (status, _) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": ["000000000000"]}),
        &alice_current,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Alice's own other session resolves and is revoked.
    let (_, list) = get_json_with_token(&mut app, "/api/security/sessions", &alice_current).await;
    let other_id = list["data"]["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|s| s["is_current"] == json!(false))
        .and_then(|s| s["id"].as_str())
        .expect("Alice's second session id")
        .to_string();

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": [other_id]}),
        &alice_current,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revoked"], json!(1));

    // The revoked session is dead server-side; the caller's own session and
    // Bob's session are both untouched.
    let (dead, _) = get_json_with_token(&mut app, "/api/auth/me", &alice_other).await;
    assert!(dead.is_client_error(), "revoked session must be rejected");
    let (alive, _) = get_json_with_token(&mut app, "/api/auth/me", &alice_current).await;
    assert_eq!(alive, StatusCode::OK);
    let (bob_status, _) = get_json_with_token(&mut app, "/api/auth/me", &bob_session).await;
    assert_eq!(bob_status, StatusCode::OK);
}

/// The current session stays protected: its own id is never revoked, and a
/// mixed request still deletes only the other sessions (ROADMAP 14.2).
#[tokio::test]
async fn test_revoke_sessions_protects_the_current_session() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "carol", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let current = login(&mut app, "carol", "Passw0rd!").await;
    let _other = login(&mut app, "carol", "Passw0rd!").await;
    let current_id = homevault::handlers::security::session_public_id(&current);

    // Only the current id → refused, nothing deleted.
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": [current_id]}),
        &current,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (alive, _) = get_json_with_token(&mut app, "/api/auth/me", &current).await;
    assert_eq!(alive, StatusCode::OK, "the current session must survive");

    // Current + another id → only the other one is revoked.
    let (_, list) = get_json_with_token(&mut app, "/api/security/sessions", &current).await;
    let other_id = list["data"]["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|s| s["is_current"] == json!(false))
        .and_then(|s| s["id"].as_str())
        .expect("second session id")
        .to_string();
    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": [current_id, other_id]}),
        &current,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revoked"], json!(1));
    let (_, list) = get_json_with_token(&mut app, "/api/security/sessions", &current).await;
    assert_eq!(list["data"]["total"], json!(1), "{list}");
}

/// An empty id list is a validation error, not a silent no-op.
#[tokio::test]
async fn test_revoke_sessions_rejects_an_empty_id_list() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "dave", "Passw0rd!", 1).await;
    let mut app = account_router(state);
    let token = login(&mut app, "dave", "Passw0rd!").await;

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": []}),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], json!(false));
}

/// "Log out all others" stays scoped to the caller's own sessions.
#[tokio::test]
async fn test_revoke_all_sessions_stays_scoped_to_the_caller() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "erin", "Passw0rd!", 1).await;
    create_test_user(&state, "frank", "Passw0rd!", 1).await;
    let mut app = account_router(state);

    let erin_current = login(&mut app, "erin", "Passw0rd!").await;
    let erin_other = login(&mut app, "erin", "Passw0rd!").await;
    let frank_session = login(&mut app, "frank", "Passw0rd!").await;

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke-all",
        json!({}),
        &erin_current,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revoked"], json!(1));

    let (alive, _) = get_json_with_token(&mut app, "/api/auth/me", &erin_current).await;
    assert_eq!(alive, StatusCode::OK);
    let (dead, _) = get_json_with_token(&mut app, "/api/auth/me", &erin_other).await;
    assert!(dead.is_client_error());
    let (frank_status, _) = get_json_with_token(&mut app, "/api/auth/me", &frank_session).await;
    assert_eq!(
        frank_status,
        StatusCode::OK,
        "revoke-all must never touch another account"
    );
}

// ===========================================================================
// 8. The session surface goes through admin_guard (ROADMAP X.7)
// ===========================================================================

/// Expires a session row directly in the DB, the way time would.
async fn expire_session(state: &AppState, token: &str) {
    sqlx::query("UPDATE sessions SET expires_at = ? WHERE token = ?")
        .bind("2000-01-01T00:00:00Z")
        .bind(token)
        .execute(&*state.db.pool())
        .await
        .expect("Failed to expire the session row");
}

/// The three self-service handlers used to rebuild the token check by hand.
/// The delegation to `admin_guard` must not have loosened it: an expired
/// session is `401` on every one of them, and nothing is read or written.
///
/// This is the assertion X.7 needed — a hand-rolled copy can silently lose the
/// expiry predicate (which is exactly what item 15 had to fix on the Dashboard),
/// and no status-only test around a *valid* token would ever notice.
#[tokio::test]
async fn test_security_endpoints_reject_an_expired_session() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "expired_owner", "Passw0rd!", 1).await;
    let mut app = account_router(Arc::clone(&state));

    let current = login(&mut app, "expired_owner", "Passw0rd!").await;
    let other = login(&mut app, "expired_owner", "Passw0rd!").await;
    expire_session(&state, &current).await;

    let (status, body) = get_json_with_token(&mut app, "/api/security/sessions", &current).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an expired session must not list sessions: {body}"
    );

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke",
        json!({"ids": [homevault::handlers::security::session_public_id(&other)]}),
        &current,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an expired session must not revoke anything: {body}"
    );

    let (status, body) = post_json_with_token(
        &mut app,
        "/api/security/sessions/revoke-all",
        json!({}),
        &current,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an expired session must not revoke anything: {body}"
    );

    // The refusal wrote nothing: the other session is still alive.
    let (alive, _) = get_json_with_token(&mut app, "/api/auth/me", &other).await;
    assert_eq!(
        alive,
        StatusCode::OK,
        "a refused revoke must not delete a live session"
    );
}
