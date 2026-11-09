/* =============================================================================
 * File          : zfs_cache_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   ZFS L2ARC/SLOG integration tests: in-memory SQLite + assembled Axum
 *   router, exercised through oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test zfs_cache_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for `/api/zfs-cache` (ZFS read cache / separate log).
//!
//! **No `zpool add`/`zpool remove` is ever executed here.** Only the refusal
//! paths are driven (admin gate, role, device shape), plus the read-only
//! listing, which on a host without ZFS simply reports `pools: []`.

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
    auth::handle_login,
    zfs_cache::{handle_zfs_cache_attach, handle_zfs_cache_detach, handle_zfs_cache_list},
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
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route(
            "/api/zfs-cache",
            get(handle_zfs_cache_list).post(handle_zfs_cache_attach),
        )
        .route(
            "/api/zfs-cache/remove",
            axum::routing::post(handle_zfs_cache_detach),
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
    body["data"]["token"]
        .as_str()
        .expect("login must return a token")
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Attaching a cache vdev runs root-owned `zpool`: admin only, on every route.
#[tokio::test]
async fn zfs_cache_endpoints_are_admin_only() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    mk_user(&state, "member", "pw012345", 1).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let member = login(&mut r, "member", "pw012345").await;

    let (status, _) = send(&mut r, Method::GET, "/api/zfs-cache", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(&mut r, Method::GET, "/api/zfs-cache", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // The listing only reads: on a host without ZFS it reports `available`
    // and empty collections rather than failing.
    let (status, body) = send(&mut r, Method::GET, "/api/zfs-cache", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["available"].is_boolean());
    assert!(data["pools"].is_array());
    assert!(data["candidates"].is_array());

    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/zfs-cache",
        Some(&member),
        Some(json!({"pool": "tank", "kind": "l2arc", "device": "/dev/sdb"})),
    )
    .await;
    assert_eq!(body["success"], json!(false), "member must be refused");
}

/// The role and the device are validated before any `zpool` call, so a bad
/// request can never reach the pool.
#[tokio::test]
async fn zfs_cache_attach_validates_the_role_and_the_device() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/zfs-cache",
        Some(&admin),
        Some(json!({"pool": "tank", "kind": "nvme", "device": "/dev/sdb"})),
    )
    .await;
    assert_eq!(body["success"], json!(false), "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("read cache"));

    // A device is one argv entry of `zpool add`: a path, not a command.
    for bad in ["", "sdb", "/etc/passwd", "/dev/../etc/passwd", "/dev/sd b"] {
        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/zfs-cache",
            Some(&admin),
            Some(json!({"pool": "tank", "kind": "l2arc", "device": bad})),
        )
        .await;
        assert_eq!(body["success"], json!(false), "{bad:?}: {body}");
    }

    // An empty pool name is refused before the pool scan.
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/zfs-cache",
        Some(&admin),
        Some(json!({"pool": "  ", "kind": "l2arc", "device": "/dev/sdb"})),
    )
    .await;
    assert_eq!(body["success"], json!(false), "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Choose a pool"));
}

/// Detaching is refused for anything that is not a `/dev` path — the request
/// must never be able to name a data disk.
#[tokio::test]
async fn zfs_cache_detach_validates_the_device_first() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    for bad in ["", "tank", "/dev/../etc/passwd", "/etc/shadow"] {
        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/zfs-cache/remove",
            Some(&admin),
            Some(json!({"pool": "tank", "device": bad})),
        )
        .await;
        assert_eq!(body["success"], json!(false), "{bad:?}: {body}");
    }
}
