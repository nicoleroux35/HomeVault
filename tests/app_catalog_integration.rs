/* =============================================================================
 * File          : app_catalog_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   App catalog integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the manifest-driven Docker app catalog and the
//! community app-store endpoints (`crate::handlers::docker_catalog`).
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt::oneshot` for HTTP-level testing (no real server, no
//! real git). Docker itself is not required - `ComposeManager::get_project`
//! just reports "not installed" when the daemon is absent.
//!
//! Run: cargo test --test app_catalog_integration

use axum::{
    routing::{delete, get, post},
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
    docker_catalog::{
        handle_catalog_install, handle_catalog_list, handle_catalog_update, handle_catalog_updates,
        handle_store_add, handle_store_delete, handle_stores_list,
    },
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
        .route("/api/docker/catalog", get(handle_catalog_list))
        .route(
            "/api/docker/catalog/{id}/install",
            post(handle_catalog_install),
        )
        .route("/api/docker/catalog/updates", get(handle_catalog_updates))
        .route(
            "/api/docker/catalog/{id}/update",
            post(handle_catalog_update),
        )
        .route("/api/docker/stores", get(handle_stores_list))
        .route("/api/docker/stores", post(handle_store_add))
        .route("/api/docker/stores/{id}", delete(handle_store_delete))
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

#[tokio::test]
async fn test_catalog_list_requires_auth() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/docker/catalog", None, None).await;
    assert_eq!(body["success"], json!(false), "no token must be rejected");
}

#[tokio::test]
async fn test_catalog_list_returns_builtin_apps() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "u", "pw012345", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "u", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let apps = body["data"].as_array().expect("array");
    assert_eq!(apps.len(), 15, "the 15 built-in apps must be listed");
    let jellyfin = apps
        .iter()
        .find(|a| a["app_id"] == "jellyfin")
        .expect("jellyfin present");
    assert_eq!(jellyfin["id"], "homevault:jellyfin");
    assert_eq!(jellyfin["store_id"], "homevault");
    assert!(jellyfin["description"].as_str().unwrap().len() > 5);
    assert_eq!(jellyfin["installed"], json!(false));
}

#[tokio::test]
async fn test_stores_list_starts_empty() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "u", "pw012345", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "u", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/stores",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_store_add_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "plain", "pw012345", 1).await; // role 1 = user
    let mut r = router(state);
    let token = login(&mut r, "plain", "pw012345").await;

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/docker/stores",
        Some(&token),
        Some(json!({"git_url": "https://github.com/acme/apps.git"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_store_add_rejects_non_git_url() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await; // role 2 = admin
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    for bad in ["file:///etc/passwd", "ext::sh -c id", "not a url"] {
        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/docker/stores",
            Some(&token),
            Some(json!({ "git_url": bad })),
        )
        .await;
        assert_eq!(body["success"], json!(false), "{bad} must be rejected");
    }
}

#[tokio::test]
async fn test_store_delete_unknown_is_error() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/docker/stores/ghost",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_updates_endpoint_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "plain", "pw012345", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "plain", "pw012345").await;

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog/updates",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_updates_endpoint_ok_for_admin_with_nothing_installed() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog/updates",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn test_update_unknown_app_is_error() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/catalog/homevault:jellyfin/update",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false), "jellyfin is not installed");
}

#[tokio::test]
async fn test_install_unknown_app_is_error() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/catalog/nope:missing/install",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Unknown"));
}
