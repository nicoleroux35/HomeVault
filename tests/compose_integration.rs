/* =============================================================================
 * File          : compose_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Compose integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Docker Compose handlers, focused on the
//! `GET /api/docker/compose/projects/{name}/config` endpoint added for the
//! Docker app's "Edit compose" button (a container's compose file is fetched,
//! edited, and written back via `PUT /api/docker/compose/projects/{name}`).
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt::oneshot` for HTTP-level testing (no real server, no
//! Docker daemon — the config read/write is pure filesystem work under a
//! per-test `DATA_DIRECTORY`).
//!
//! Run: cargo test --test compose_integration

use axum::{
    routing::{get, post, put},
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
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    compose::{handle_compose_create, handle_compose_get_config, handle_compose_update},
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

type Req = Request<axum::body::Body>;

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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/docker/compose/projects", post(handle_compose_create))
        .route(
            "/api/docker/compose/projects/{name}",
            put(handle_compose_update),
        )
        .route(
            "/api/docker/compose/projects/{name}/config",
            get(handle_compose_get_config),
        )
        .with_state(state)
}

async fn mk_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'T', 't@e.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&hash)
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

const SAMPLE_YAML: &str =
    "services:\n  web:\n    image: nginx:latest\n    ports:\n      - \"8080:80\"\n";

#[tokio::test]
async fn test_get_config_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "plain", "pw012345", 1).await; // role 1 = user
    let mut r = router(state);
    let token = login(&mut r, "plain", "pw012345").await;

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/docker/compose/projects/whatever/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_get_config_rejects_missing_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/compose/projects/whatever/config",
        None,
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false), "no token must be rejected");
}

#[tokio::test]
async fn test_get_config_roundtrip() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await; // role 2 = admin
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    // Create a project, then read its compose file back verbatim.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/compose/projects",
        Some(&token),
        Some(json!({"name": "editme", "config": SAMPLE_YAML})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/compose/projects/editme/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["config"], json!(SAMPLE_YAML));
}

#[tokio::test]
async fn test_update_then_get_reflects_change() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    send(
        &mut r,
        Method::POST,
        "/api/docker/compose/projects",
        Some(&token),
        Some(json!({"name": "editme", "config": SAMPLE_YAML})),
    )
    .await;

    let edited = "services:\n  web:\n    image: nginx:1.27\n";
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/docker/compose/projects/editme",
        Some(&token),
        Some(json!({"config": edited})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/compose/projects/editme/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["config"], json!(edited));
}

#[tokio::test]
async fn test_get_config_unknown_project_fails_cleanly() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/compose/projects/doesnotexist/config",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "unknown project must not 500"
    );
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_get_config_rejects_traversal_name() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    // `..%2Fetc` — the name segment must be refused by ComposeManager's
    // validate_name before any path join, never escape the compose dir.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/compose/projects/..%2F..%2Fetc/config",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["success"], json!(false));
}
