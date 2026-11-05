/* =============================================================================
 * File          : share_snapshots_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-share snapshot integration tests (P1-12): in-memory SQLite + assembled
 *   Axum router, exercising the auth split and the validation gates that run
 *   before any real ZFS/Btrfs command.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test share_snapshots_integration
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Per-share snapshot endpoints:
//!
//!   GET    /api/shares/{name}/{share_type}/snapshots
//!   POST   /api/shares/{name}/{share_type}/snapshots
//!   DELETE /api/shares/{name}/{share_type}/snapshots/{snapshot}
//!   POST   /api/shares/{name}/{share_type}/snapshots/{snapshot}/restore
//!
//! No real pool is touched: unknown share names, invalid snapshot names and a
//! missing restore confirmation are all refused before the service is reached.

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
use homevault::handlers::share_snapshots::{
    handle_share_snapshot_create, handle_share_snapshot_delete, handle_share_snapshot_restore,
    handle_share_snapshots_list,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::models::{ADMIN_ROLE_ID, USER_ROLE_ID};
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

fn share_snapshots_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/auth/login",
            post(homevault::handlers::auth::handle_login),
        )
        .route(
            "/api/shares/{name}/{share_type}/snapshots",
            get(handle_share_snapshots_list).post(handle_share_snapshot_create),
        )
        .route(
            "/api/shares/{name}/{share_type}/snapshots/{snapshot}",
            delete(handle_share_snapshot_delete),
        )
        .route(
            "/api/shares/{name}/{share_type}/snapshots/{snapshot}/restore",
            post(handle_share_snapshot_restore),
        )
        .with_state(state)
}

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
    .expect("insert test user");
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

fn write_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (
            Method::POST,
            "/api/shares/hvtest/smb/snapshots",
            Some(json!({})),
        ),
        (
            Method::DELETE,
            "/api/shares/hvtest/smb/snapshots/hv-auto-20261105-120000",
            None,
        ),
        (
            Method::POST,
            "/api/shares/hvtest/smb/snapshots/hv-auto-20261105-120000/restore",
            Some(json!({"path": "a.txt", "confirm": true})),
        ),
    ]
}

#[tokio::test]
async fn reads_reject_a_missing_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = share_snapshots_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/shares/hvtest/smb/snapshots",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutations_reject_missing_and_non_admin_sessions() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", USER_ROLE_ID).await;
    let mut router = share_snapshots_router(state);

    for (method, uri, body) in write_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }

    let token = login(&mut router, "regular", "Password123!").await;
    for (method, uri, body) in write_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, Some(&token), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} must be admin-only"
        );
    }
}

#[tokio::test]
async fn invalid_input_is_refused_before_any_snapshot_command() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", ADMIN_ROLE_ID).await;
    let mut router = share_snapshots_router(state);
    let admin = login(&mut router, "admin", "Password123!").await;

    // An unsafe name is a 400 even though the share does not exist.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/shares/hvtest/smb/snapshots",
        Some(&admin),
        Some(json!({"name": "../escape"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Invalid snapshot name"),
        "{body}"
    );

    // A restore without the explicit confirmation never copies anything.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/shares/hvtest/smb/snapshots/hv-auto-20261105-120000/restore",
        Some(&admin),
        Some(json!({"path": "a.txt", "confirm": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("confirm the restore"),
        "{body}"
    );

    // A traversal is refused before the snapshot lookup.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/shares/hvtest/smb/snapshots/hv-auto-20261105-120000/restore",
        Some(&admin),
        Some(json!({"path": "../../etc/passwd", "confirm": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("inside the share"),
        "{body}"
    );

    // Unknown share / snapshot is a clean 400, never a 500.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/shares/hv-no-such-share/smb/snapshots",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
