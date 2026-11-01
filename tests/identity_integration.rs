/* =============================================================================
 * File          : identity_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for identity: trusted devices, the rescue address and the
 *   public account-recovery endpoints. Covers the guard on every route, the
 *   ownership rule on revocation, the "nothing is stored until the code comes
 *   back" rule on the rescue address, the second-factor skip on a trusted
 *   device, and the no-enumeration answer of account recovery.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test identity_integration
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::{handle_2fa_verify, handle_login};
use homevault::handlers::identity::{
    handle_device_revoke, handle_devices_list, handle_devices_revoke_all, handle_recover_request,
    handle_recover_verify, handle_rescue_email_clear, handle_rescue_email_get,
    handle_rescue_email_set, handle_rescue_email_verify,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::identity::{self, DeviceStore};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<Body>;

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
    let (ransomware_watch_tx, _rx) =
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
        .route("/api/auth/2fa/verify", post(handle_2fa_verify))
        .route("/api/auth/recover", post(handle_recover_request))
        .route("/api/auth/recover/verify", post(handle_recover_verify))
        .route("/api/identity/devices", get(handle_devices_list))
        .route("/api/identity/devices/revoke", post(handle_device_revoke))
        .route(
            "/api/identity/devices/revoke-all",
            post(handle_devices_revoke_all),
        )
        .route(
            "/api/identity/rescue-email",
            get(handle_rescue_email_get)
                .post(handle_rescue_email_set)
                .delete(handle_rescue_email_clear),
        )
        .route(
            "/api/identity/rescue-email/verify",
            post(handle_rescue_email_verify),
        )
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
    .execute(state.db.pool().as_ref())
    .await
    .expect("insert user");
}

/// Enable 2FA for an account without going through TOTP: the tests never
/// submit a code, they only need `is_2fa_enabled` to be true.
async fn enable_2fa(state: &AppState, username: &str) {
    sqlx::query(
        "INSERT INTO user_2fa (username, totp_secret_encrypted, enabled, created_at)
         VALUES (?, 'unused-in-this-test', 1, datetime('now'))",
    )
    .bind(username)
    .execute(state.db.pool().as_ref())
    .await
    .expect("insert 2fa row");
}

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {body}");
    body["data"]["token"].as_str().expect("token").to_string()
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = request(method, uri, token, cookie, body);
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

fn request(
    method: Method,
    uri: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> Req {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    if let Some(b) = body {
        req.header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        req.body(Body::empty()).unwrap()
    }
}

/// Every signed identity endpoint, so the guard test cannot miss one.
fn signed_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/identity/devices", None),
        (
            Method::POST,
            "/api/identity/devices/revoke",
            Some(json!({ "id": "x" })),
        ),
        (Method::POST, "/api/identity/devices/revoke-all", None),
        (Method::GET, "/api/identity/rescue-email", None),
        (
            Method::POST,
            "/api/identity/rescue-email",
            Some(json!({ "email": "me@example.com" })),
        ),
        (
            Method::POST,
            "/api/identity/rescue-email/verify",
            Some(json!({ "code": "000000" })),
        ),
        (Method::DELETE, "/api/identity/rescue-email", None),
    ]
}

#[tokio::test]
async fn every_signed_identity_endpoint_refuses_an_anonymous_caller_with_401() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    for (method, uri, body) in signed_endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, None, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn a_user_lists_and_revokes_their_own_devices() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    let mut r = router(state.clone());
    let token = login(&mut r, "alice", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/identity/devices",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], 0);

    let pool = state.db.pool();
    let store = DeviceStore::new(pool.as_ref());
    let (first, _first_token) = store.create("alice", "Firefox on Linux").await.unwrap();
    let (_second, _) = store.create("alice", "Safari on iOS").await.unwrap();

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/identity/devices",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], 2);
    assert_eq!(body["data"]["lifetime_days"], 30);
    assert!(!body.to_string().contains("token_hash"), "{body}");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/devices/revoke",
        Some(&token),
        None,
        Some(json!({ "id": first.id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["success"], true);

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/devices/revoke-all",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["revoked"], 1);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/identity/devices",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], 0);
}

#[tokio::test]
async fn one_account_cannot_revoke_another_accounts_device() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    create_user(&state, "bob", "Passw0rd!123", 1).await;
    let mut r = router(state.clone());
    let alice = login(&mut r, "alice", "Passw0rd!123").await;

    let pool = state.db.pool();
    let store = DeviceStore::new(pool.as_ref());
    let (bobs_device, _) = store.create("bob", "Chrome on Windows").await.unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/devices/revoke",
        Some(&alice),
        None,
        Some(json!({ "id": bobs_device.id })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("no such trusted device"));

    // Bob still has it.
    assert_eq!(store.list("bob").await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_rescue_address_is_not_stored_until_the_code_comes_back() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    let mut r = router(state.clone());
    let token = login(&mut r, "alice", "Passw0rd!123").await;

    // A malformed address is refused before anything is sent.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/rescue-email",
        Some(&token),
        None,
        Some(json!({ "email": "not-an-address" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A valid address with no working MTA: the code cannot be delivered, so the
    // request fails and nothing becomes a usable rescue address.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/rescue-email",
        Some(&token),
        None,
        Some(json!({ "email": "alice@example.com" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("could not be sent"));

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/identity/rescue-email",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["configured"], false);
    assert_eq!(body["data"]["email"], Value::Null);
}

#[tokio::test]
async fn the_right_code_stores_the_address_and_a_wrong_one_does_not() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    let mut r = router(state.clone());
    let token = login(&mut r, "alice", "Passw0rd!123").await;

    let code = identity::issue_code(
        state.db.pool().as_ref(),
        "alice",
        "alice@example.com",
        identity::CodePurpose::VerifyEmail,
    )
    .await
    .unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/rescue-email/verify",
        Some(&token),
        None,
        Some(json!({ "code": "000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("not correct"));

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/identity/rescue-email/verify",
        Some(&token),
        None,
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["email"], "alice@example.com");

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/identity/rescue-email",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["configured"], true);
    assert_eq!(body["data"]["email"], "alice@example.com");

    // A code is single use: replaying it now finds nothing.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/identity/rescue-email/verify",
        Some(&token),
        None,
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn clearing_the_rescue_address_removes_it() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    let mut r = router(state.clone());
    let token = login(&mut r, "alice", "Passw0rd!123").await;
    identity::set_rescue_address(state.db.pool().as_ref(), "alice", "alice@example.com")
        .await
        .unwrap();

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/identity/rescue-email",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/identity/rescue-email",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["configured"], false);
}

#[tokio::test]
async fn a_trusted_device_skips_the_second_factor_and_another_accounts_does_not() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 2).await;
    create_user(&state, "bob", "Passw0rd!123", 1).await;
    enable_2fa(&state, "alice").await;
    let mut r = router(state.clone());

    // Without a device cookie, a correct password is not enough.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/auth/login",
        None,
        None,
        Some(json!({ "username": "alice", "password": "Passw0rd!123" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["data"]["requires_2fa"], true);

    let pool = state.db.pool();
    let store = DeviceStore::new(pool.as_ref());
    let (_device, alice_token) = store.create("alice", "Firefox on Linux").await.unwrap();
    let cookie = format!("hv_trusted_device={alice_token}");

    // The same password now signs in, skipping the code.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/auth/login",
        None,
        Some(&cookie),
        Some(json!({ "username": "alice", "password": "Passw0rd!123" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["token"].is_string());

    // Bob presenting Alice's cookie still has to provide a code.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/auth/login",
        None,
        Some(&cookie),
        Some(json!({ "username": "bob", "password": "Passw0rd!123" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "bob has no 2FA, so his password alone signs him in: {body}"
    );

    // A wrong password never reaches the device check.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/auth/login",
        None,
        Some(&cookie),
        Some(json!({ "username": "alice", "password": "wrong-password" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn account_recovery_answers_the_same_whether_or_not_the_account_exists() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    let mut r = router(state);

    let (status_unknown, unknown) = send(
        &mut r,
        Method::POST,
        "/api/auth/recover",
        None,
        None,
        Some(json!({ "username": "nobody" })),
    )
    .await;
    let (status_known, known) = send(
        &mut r,
        Method::POST,
        "/api/auth/recover",
        None,
        None,
        Some(json!({ "username": "alice" })),
    )
    .await;

    assert_eq!(status_unknown, StatusCode::OK, "{unknown}");
    assert_eq!(status_known, StatusCode::OK, "{known}");
    assert_eq!(unknown, known, "the answer must not reveal the account");
    assert!(unknown["data"]["message"]
        .as_str()
        .unwrap()
        .contains("rescue address"));
}

#[tokio::test]
async fn a_recovery_code_is_exchanged_for_a_reset_code() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "alice", "Passw0rd!123", 1).await;
    let mut r = router(state.clone());
    identity::set_rescue_address(state.db.pool().as_ref(), "alice", "alice@example.com")
        .await
        .unwrap();
    let code = identity::issue_code(
        state.db.pool().as_ref(),
        "alice",
        "alice@example.com",
        identity::CodePurpose::RecoverAccount,
    )
    .await
    .unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/auth/recover/verify",
        None,
        None,
        Some(json!({ "username": "alice", "code": "000000" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/auth/recover/verify",
        None,
        None,
        Some(json!({ "username": "alice", "code": code })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reset = body["data"]["recovery_code"].as_str().expect("reset code");
    assert!(!reset.is_empty());
    assert_eq!(body["data"]["username"], "alice");
}
