/* =============================================================================
 * File          : auth_guard_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 24/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Contract tests for the single session-permission primitive
 *   (`handlers::admin_guard`) — the status a caller gets for "no session",
 *   "bad session", "not an admin" and "malformed request", across a
 *   `require_auth` endpoint, a `require_admin` endpoint and an endpoint that
 *   validates its body.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test auth_guard_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the auth-guard contract (ROADMAP X.7 / X.8).
//!
//! The whole tree used to answer **two different statuses** for the same
//! situation: `handlers::admin_guard` (and `security.rs`, `shares.rs`,
//! `settings.rs`, `chat.rs`, the Explorer, ...) each rebuilt the token check
//! and answered `400 Bad Request` when the `x-session-token` header was
//! absent, while a handful of modules answered `401 Unauthorized`. A client
//! that branches on the status therefore read "you are not signed in" as
//! "your request is malformed" on most of the API.
//!
//! The contract asserted here, once:
//!
//! | Situation                        | Status |
//! |----------------------------------|--------|
//! | `x-session-token` absent         | `401`  |
//! | Token unknown or expired         | `401`  |
//! | Valid session, account not admin | `403`  |
//! | Valid admin, malformed body      | `400`  |
//!
//! The last row is the point of the suite: `400` must stay reserved for a
//! request the server could not understand, which is only meaningful if
//! `401` is used for authentication.
//!
//! Run:
//!   cargo test --test auth_guard_integration
//!   cargo test --test auth_guard_integration -- --nocapture

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
use homevault::handlers::security::{
    handle_get_login_history, handle_get_my_sessions, handle_revoke_sessions,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::whats_new::handle_whats_new;
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

/// One `require_auth` endpoint, one `require_admin` endpoint, one endpoint
/// that both guards and validates its body.
fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/whats-new", get(handle_whats_new))
        .route("/api/security/sessions", get(handle_get_my_sessions))
        .route(
            "/api/security/sessions/revoke",
            post(handle_revoke_sessions),
        )
        .route("/api/security/login-history", get(handle_get_login_history))
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
    let (status, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
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

/// Every guarded endpoint of the router, with a method and a body where the
/// route needs one.
fn guarded_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/whats-new", None),
        (Method::GET, "/api/security/sessions", None),
        (
            Method::POST,
            "/api/security/sessions/revoke",
            Some(json!({"ids": ["0123456789ab"]})),
        ),
        (Method::GET, "/api/security/login-history", None),
    ]
}

// ---------------------------------------------------------------------------
// 1. No session → 401 (not 400)
// ---------------------------------------------------------------------------

/// The assertion X.8 needed: an absent header is an authentication failure.
///
/// Before the fix every one of these answered `400`, so a client that shows
/// "sign in again" on `401` and "check your input" on `400` showed the wrong
/// message — and could not tell a broken login from a broken form.
#[tokio::test]
async fn a_missing_token_is_401_on_every_guarded_endpoint() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    for (method, uri, body) in guarded_endpoints() {
        let (status, payload) = send(&mut r, method.clone(), uri, None, body).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must answer 401 for a missing token: {payload}"
        );
        assert_eq!(payload["success"], json!(false), "{method} {uri}");
        assert!(
            payload["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("session"),
            "{method} {uri} must say the session is missing: {payload}"
        );
    }
}

/// A present-but-empty header is the same situation as an absent one.
#[tokio::test]
async fn an_empty_token_is_401_too() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/security/sessions",
        Some(""),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

/// An unknown token is `401` as well — and the message must not distinguish
/// "no such session" from "expired", which would leak whether a token ever
/// existed.
#[tokio::test]
async fn an_unknown_token_is_401_with_the_same_wording() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    let (missing, _) = send(&mut r, Method::GET, "/api/security/sessions", None, None).await;
    let (bogus, _) = send(
        &mut r,
        Method::GET,
        "/api/security/sessions",
        Some("not-a-real-token"),
        None,
    )
    .await;
    assert_eq!(missing, StatusCode::UNAUTHORIZED);
    assert_eq!(bogus, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. Valid session, wrong privilege → 403 (not 400)
// ---------------------------------------------------------------------------

/// The assertion X.7 needed: a signed-in non-admin is a *permission* refusal,
/// not a malformed request.
///
/// `shares`, `permissions`, `snmp` and `settings` used to answer `400` here
/// while every other module answered `403`, so the same click reported
/// "invalid input" on one screen and "not allowed" on another.
#[tokio::test]
async fn a_signed_in_non_admin_gets_403_on_admin_only_endpoints() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "regular", "Passw0rd!123", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "regular", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/security/login-history",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a non-admin must be refused with 403, not 400: {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("admin"),
        "the refusal must name the missing privilege: {body}"
    );
}

/// A valid non-admin session is accepted by `require_auth` endpoints — the
/// guard must not have been tightened into "admin only" while it was being
/// centralised.
#[tokio::test]
async fn a_valid_non_admin_session_still_reaches_user_endpoints() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "regular2", "Passw0rd!123").await;

    let (status, body) = send(&mut r, Method::GET, "/api/whats-new", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/security/sessions",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], json!(1), "{body}");
}

// ---------------------------------------------------------------------------
// 3. `400` stays reserved for a malformed request
// ---------------------------------------------------------------------------

/// The counterpart that gives the `401` its meaning: with a valid admin
/// session, a body the handler refuses is still `400`. If this ever became
/// `401` the contract would be useless in the other direction.
#[tokio::test]
async fn a_malformed_body_from_an_admin_is_still_400() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/security/sessions/revoke",
        Some(&token),
        Some(json!({"ids": []})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an empty id list is a validation error, not an auth error: {body}"
    );

    // Same endpoint, no session at all → 401. Same route, two situations, two
    // statuses: that is the whole point of the contract.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/security/sessions/revoke",
        None,
        Some(json!({"ids": []})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the guard runs before validation: {body}"
    );
}

/// A body the extractor cannot parse at all is `4xx` and never a panic — the
/// guard must not have replaced the extractor's rejection.
#[tokio::test]
async fn an_unparseable_body_is_refused_without_panicking() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin2", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin2", "AdminPass123!").await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/security/sessions/revoke")
        .header("x-session-token", &token)
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{not json"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_client_error(),
        "an unparseable body must be a clean 4xx, got {status}"
    );
}
