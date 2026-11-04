/* =============================================================================
 * File          : upnp_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPnP-IGD integration tests (phase C3): the access-control matrix and every
 *   refusal that happens *before* `upnpc` is allowed to run. Nothing here
 *   opens a port on a real router: a valid add/remove would talk to the
 *   gateway hosting the suite, so the success path is covered by the module's
 *   own tests (`src/upnp.rs`) and by the intercepted E2E suite.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test upnp_integration
 * ============================================================================= */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the UPnP-IGD endpoints.

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
use homevault::handlers::upnp::{handle_add, handle_get, handle_remove};
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
        .route("/api/network/upnp", get(handle_get))
        .route("/api/network/upnp/mappings", post(handle_add))
        .route("/api/network/upnp/mappings", delete(handle_remove))
        .with_state(state)
}

async fn create_test_user(state: &AppState, username: &str, role_id: i64) {
    let password_hash = state.auth.hash_password("Password123!").unwrap();
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

async fn admin_router() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
    Router,
    String,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, "admin", 2).await;
    let mut r = router(state.clone());
    let token = login(&mut r, "admin").await;
    (state, tmp, guard, r, token)
}

fn mapping_body() -> Value {
    json!({
        "protocol": "tcp",
        "external_port": 8080,
        "internal_host": "192.168.1.50",
        "internal_port": 80,
        "description": "web",
        "lease_seconds": 3600,
        "confirmed": true,
    })
}

// ---------------------------------------------------------------------------
// 1. Guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_upnp_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    for (method, uri, body) in [
        (Method::GET, "/api/network/upnp", None),
        (
            Method::POST,
            "/api/network/upnp/mappings",
            Some(mapping_body()),
        ),
        (
            Method::DELETE,
            "/api/network/upnp/mappings",
            Some(json!({"protocol": "tcp", "external_port": 8080, "confirmed": true})),
        ),
    ] {
        let (status, response) = send(&mut r, method.clone(), uri, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}: {response}");
    }
}

#[tokio::test]
async fn test_reads_need_a_member_but_mutations_need_an_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "member").await;

    let (status, body) = send(&mut r, Method::GET, "/api/network/upnp", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["available"].is_boolean());
    assert!(body["data"]["mappings"].is_array());

    for (method, uri, body) in [
        (
            Method::POST,
            "/api/network/upnp/mappings",
            Some(mapping_body()),
        ),
        (
            Method::DELETE,
            "/api/network/upnp/mappings",
            Some(json!({"protocol": "tcp", "external_port": 8080, "confirmed": true})),
        ),
    ] {
        let (status, response) = send(&mut r, method.clone(), uri, Some(&token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {response}");
    }
}

#[tokio::test]
async fn test_the_guard_runs_before_the_body_is_parsed() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/network/upnp/mappings",
        None,
        Some(json!({"not": "a mapping"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. Refusals before upnpc runs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_mapping_without_confirmation_is_refused() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let mut body = mapping_body();
    body["confirmed"] = json!(false);
    let (status, response) = send(
        &mut r,
        Method::POST,
        "/api/network/upnp/mappings",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert!(response["message"]
        .as_str()
        .unwrap_or("")
        .contains("confirmed"));
}

#[tokio::test]
async fn test_a_public_or_loopback_internal_client_is_refused() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    for host in ["8.8.8.8", "127.0.0.1", "0.0.0.0", "example.com"] {
        let mut body = mapping_body();
        body["internal_host"] = json!(host);
        let (status, response) = send(
            &mut r,
            Method::POST,
            "/api/network/upnp/mappings",
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{host}: {response}");
    }
}

#[tokio::test]
async fn test_port_and_lease_refusals() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let cases = [
        ("external_port", json!(0)),
        ("internal_port", json!(0)),
        ("lease_seconds", json!(30)),
        ("protocol", json!("sctp")),
    ];
    for (field, value) in cases {
        let mut body = mapping_body();
        body[field] = value;
        let (status, response) = send(
            &mut r,
            Method::POST,
            "/api/network/upnp/mappings",
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {response}");
    }
}

#[tokio::test]
async fn test_a_malformed_body_is_a_400() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/network/upnp/mappings",
        Some(&token),
        Some(json!({"host": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_remove_requires_confirmation_and_a_port() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    for body in [
        json!({"protocol": "tcp", "external_port": 8080, "confirmed": false}),
        json!({"protocol": "tcp", "external_port": 0, "confirmed": true}),
    ] {
        let (status, response) = send(
            &mut r,
            Method::DELETE,
            "/api/network/upnp/mappings",
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    }
}

#[tokio::test]
async fn test_get_never_reports_a_capability_failure_as_an_error() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let (status, body) = send(&mut r, Method::GET, "/api/network/upnp", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["available"].is_boolean());
    if data["available"] == false {
        assert!(data["mappings"].as_array().unwrap().is_empty());
        assert!(data["error"].is_null());
    }
}
