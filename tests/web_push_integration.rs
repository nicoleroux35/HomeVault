/* =============================================================================
 * File          : web_push_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Web Push integration tests (phase D2): the access-control matrix, the
 *   VAPID key surface, every subscription refusal that happens before a push
 *   service is contacted, and per-user device isolation in the repository.
 *   Nothing here talks to a real push service or opens an internet socket.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test web_push_integration
 * ============================================================================= */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the Web Push endpoints.

use axum::routing::{delete, get, post};
use axum::Router;
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::web_push::{handle_get, handle_subscribe, handle_unsubscribe};
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};
use homevault::web_push::crypto::b64url_encode;
use homevault::web_push::repo::PushSubscriptionRepository;

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
        .route("/api/notifications/push", get(handle_get))
        .route("/api/notifications/push/subscribe", post(handle_subscribe))
        .route(
            "/api/notifications/push/subscribe/{id}",
            delete(handle_unsubscribe),
        )
        .with_state(state)
}

async fn create_test_user(state: &AppState, username: &str) {
    let password_hash = state.auth.hash_password("Password123!").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', 1, false, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(&password_hash)
    .execute(&*state.db.pool())
    .await
    .expect("insert test user");
}

async fn login(router: &mut Router, username: &str) -> String {
    let (_, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": "Password123!"})),
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
    if let Some(token) = token {
        req = req.header("x-session-token", token);
    }
    let req = if let Some(body) = body {
        req.header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
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

/// A subscription whose endpoint is refused before DNS (private literal) or
/// deliberately malformed.
fn subscribe_body(endpoint: &str, p256dh: &str, auth: &str) -> Value {
    json!({
        "endpoint": endpoint,
        "keys": { "p256dh": p256dh, "auth": auth },
    })
}

fn valid_keys() -> (String, String) {
    let mut public = vec![0u8; 65];
    public[0] = 0x04;
    for (index, byte) in public.iter_mut().enumerate().skip(1) {
        *byte = index as u8;
    }
    (b64url_encode(&public), b64url_encode(&[7u8; 16]))
}

async fn user_router(
    name: &str,
) -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
    Router,
    String,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, name).await;
    let mut r = router(state.clone());
    let token = login(&mut r, name).await;
    (state, tmp, guard, r, token)
}

// ---------------------------------------------------------------------------
// 1. Guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_push_routes_require_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let (p256dh, auth) = valid_keys();
    for (method, uri, body) in [
        (Method::GET, "/api/notifications/push", None),
        (
            Method::POST,
            "/api/notifications/push/subscribe",
            Some(subscribe_body(
                "https://fcm.googleapis.com/wp/x",
                &p256dh,
                &auth,
            )),
        ),
        (
            Method::DELETE,
            "/api/notifications/push/subscribe/abc",
            None,
        ),
    ] {
        let (status, response) = send(&mut r, method.clone(), uri, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}: {response}");
    }
}

#[tokio::test]
async fn test_any_signed_in_user_manages_their_own_devices() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member").await;
    let mut r = router(state);
    let token = login(&mut r, "member").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/push",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["available"], true);
    let key = body["data"]["public_key"].as_str().expect("public key");
    assert_eq!(key.len(), 87);
    assert!(body["data"]["subscriptions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_the_guard_runs_before_the_body_is_parsed() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/push/subscribe",
        None,
        Some(json!({"not": "a subscription"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. Subscription refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_an_endpoint_that_is_not_https_on_443_is_refused() {
    let (_state, _tmp, _guard, mut r, token) = user_router("alice").await;
    let (p256dh, auth) = valid_keys();
    for endpoint in [
        "http://fcm.googleapis.com/wp/x",
        "https://fcm.googleapis.com:8443/wp/x",
        "https://user:pass@fcm.googleapis.com/wp/x",
        "not a url",
    ] {
        let (status, response) = send(
            &mut r,
            Method::POST,
            "/api/notifications/push/subscribe",
            Some(&token),
            Some(subscribe_body(endpoint, &p256dh, &auth)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{endpoint}: {response}");
    }
}

#[tokio::test]
async fn test_a_local_endpoint_is_refused_before_any_connection() {
    let (_state, _tmp, _guard, mut r, token) = user_router("alice").await;
    let (p256dh, auth) = valid_keys();
    for endpoint in [
        "https://127.0.0.1/wp/x",
        "https://192.168.1.10/wp/x",
        "https://[::1]/wp/x",
    ] {
        let (status, response) = send(
            &mut r,
            Method::POST,
            "/api/notifications/push/subscribe",
            Some(&token),
            Some(subscribe_body(endpoint, &p256dh, &auth)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{endpoint}: {response}");
        assert!(
            response["message"]
                .as_str()
                .unwrap_or("")
                .contains("public internet address"),
            "{response}"
        );
    }
}

#[tokio::test]
async fn test_subscription_keys_are_shape_checked() {
    let (_state, _tmp, _guard, mut r, token) = user_router("alice").await;
    let (p256dh, auth) = valid_keys();
    let cases = [
        (b64url_encode(&[0u8; 64]), auth.clone()),
        (b64url_encode(&[0u8; 65]), auth.clone()),
        (p256dh.clone(), b64url_encode(&[0u8; 15])),
        ("not base64!".to_string(), auth.clone()),
    ];
    for (public, secret) in cases {
        let (status, response) = send(
            &mut r,
            Method::POST,
            "/api/notifications/push/subscribe",
            Some(&token),
            Some(subscribe_body(
                "https://fcm.googleapis.com/wp/x",
                &public,
                &secret,
            )),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    }
}

#[tokio::test]
async fn test_removing_an_unknown_device_is_a_404() {
    let (_state, _tmp, _guard, mut r, token) = user_router("alice").await;
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/notifications/push/subscribe/does-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 3. Per-user isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_devices_are_scoped_to_their_owner() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice").await;
    create_test_user(&state, "bob").await;
    let repo = PushSubscriptionRepository::new(state.db.pool());
    let (p256dh, auth) = valid_keys();

    let created = repo
        .upsert(
            "alice",
            "https://fcm.googleapis.com/wp/alice-device",
            &p256dh,
            &auth,
            "Alice laptop",
        )
        .await
        .expect("insert");

    assert_eq!(repo.list_for_user("alice").await.unwrap().len(), 1);
    assert!(repo.list_for_user("bob").await.unwrap().is_empty());
    assert!(!repo.delete_for_user("bob", &created.id).await.unwrap());
    assert!(repo.delete_for_user("alice", &created.id).await.unwrap());
    assert!(repo.list_for_user("alice").await.unwrap().is_empty());
}

#[tokio::test]
async fn test_a_re_subscribe_refreshes_the_keys_without_a_second_row() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice").await;
    let repo = PushSubscriptionRepository::new(state.db.pool());
    let (p256dh, auth) = valid_keys();

    let first = repo
        .upsert(
            "alice",
            "https://fcm.googleapis.com/wp/x",
            &p256dh,
            &auth,
            "Phone",
        )
        .await
        .unwrap();
    let second = repo
        .upsert(
            "alice",
            "https://fcm.googleapis.com/wp/x",
            &p256dh,
            &auth,
            "Phone",
        )
        .await
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(repo.count_for_user("alice").await.unwrap(), 1);
    assert!(repo
        .endpoint_registered("alice", "https://fcm.googleapis.com/wp/x")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_deleting_the_account_removes_its_devices() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice").await;
    let repo = PushSubscriptionRepository::new(state.db.pool());
    let (p256dh, auth) = valid_keys();
    repo.upsert(
        "alice",
        "https://fcm.googleapis.com/wp/x",
        &p256dh,
        &auth,
        "Phone",
    )
    .await
    .unwrap();

    sqlx::query("DELETE FROM users WHERE username = 'alice'")
        .execute(&*state.db.pool())
        .await
        .unwrap();
    assert_eq!(repo.count_for_user("alice").await.unwrap(), 0);
}
