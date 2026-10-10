// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the **UPS device configuration** endpoints —
//! `crate::handlers::ups`:
//!
//!   GET    /api/ups/config          (admin) — list `/etc/nut/ups.conf` sections
//!   POST   /api/ups/config          (admin) — add/update a section
//!   DELETE /api/ups/config/{name}   (admin) — remove a section
//!
//! The admin *write* path modifies the real `/etc/nut/ups.conf` and reloads
//! systemd, so it is deliberately NOT exercised here — it is covered live
//! (and its pure config-string logic is unit-tested in `src/ups.rs`). This
//! suite pins the auth contract: no token → 401, invalid token → 401,
//! non-admin → 403, admin → 200.
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server.
//!
//! Run:
//!   cargo test --test ups_integration

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use tower::Service;

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::ups::{
    handle_ups_config_delete, handle_ups_config_list, handle_ups_config_save,
};
use homevault::handlers::{auth::handle_login, terminal_pty::PtyManager, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn setup_state() -> (Arc<AppState>, tempfile::TempDir, MutexGuardStatic) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");

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

type MutexGuardStatic = MutexGuard<'static, ()>;

fn ups_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/ups/config",
            get(handle_ups_config_list).post(handle_ups_config_save),
        )
        .route(
            "/api/ups/config/{name}",
            axum::routing::delete(handle_ups_config_delete),
        )
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, role_id: i64) {
    let password_hash = state.auth.hash_password("Password123!").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test', 't@e.com', ?, false, datetime('now'), 'seed')",
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
    body["data"]["token"]
        .as_str()
        .expect("login token")
        .to_string()
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
    let resp = tower::ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

#[tokio::test]
async fn test_ups_config_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = ups_router(state.clone());

    for (method, uri) in [
        (Method::GET, "/api/ups/config"),
        (Method::POST, "/api/ups/config"),
        (Method::DELETE, "/api/ups/config/rack"),
    ] {
        let (status, _) = send(&mut router, method.clone(), uri, None, None).await;
        assert!(
            status.is_client_error(),
            "{method} {uri} without a token must be rejected, got {status}"
        );
    }
}

#[tokio::test]
async fn test_ups_config_rejects_invalid_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = ups_router(state.clone());

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/ups/config",
        Some("bogus"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ups_config_requires_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "regular", 1).await; // role 1 = non-admin
    let mut router = ups_router(state.clone());
    let token = login(&mut router, "regular").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/ups/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/ups/config",
        Some(&token),
        Some(json!({"name":"rack","driver":"blazer_ser","port":"/dev/ttyS0"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::DELETE,
        "/api/ups/config/rack",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_ups_config_admin_can_list() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", 2).await; // role 2 = admin
    let mut router = ups_router(state.clone());
    let token = login(&mut router, "admin").await;

    // Read-only: just verifies the route + guard + JSON shape. The underlying
    // file lives on the real host (`/etc/nut/ups.conf`).
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/ups/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert!(body["data"].is_array());
}
