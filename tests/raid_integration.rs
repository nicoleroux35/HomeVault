/* =============================================================================
 * File          : raid_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Raid integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the RAID app's API — the RAID-specific endpoints
//! that live in `crate::handlers::storage`:
//!
//!   GET    /api/storage/raid            (list arrays — read, any session)
//!   GET    /api/storage/raid/devices    (free block devices — read)
//!   POST   /api/storage/raid            (create a virtual disk — admin)
//!   POST   /api/storage/raid/{name}/rebuild  (rebuild after a disk swap — admin)
//!   POST   /api/storage/virtual-disks/{name}/grow  (grow the array — admin)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! **This suite never creates, destroys or rebuilds a real array.** Every
//! mutating path shells out to `sudo mdadm` / `zpool` / `mkfs` against real
//! block devices — per CLAUDE.md that is `tests/e2e/raid/raid.spec.ts`'s
//! job (and even there it stays validation-only unless an operator opts in
//! with `RAID_E2E_DEVICES`). Here the create/delete/rebuild endpoints are
//! exercised only far enough to confirm the auth split: reads need any
//! session, writes need an admin, and `require_admin` rejects *before* the
//! handler body — so `mdadm` is never reached. The list/devices reads are
//! safe to run for real (they only inventory the host and come back empty
//! when `mdadm` isn't installed or needs a password).
//!
//! Run:
//!   cargo test --test raid_integration

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
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    storage::{
        handle_raid_arrays_list, handle_raid_available_devices, handle_raid_create,
        handle_raid_rebuild, handle_virtual_disk_grow,
    },
    AppState,
};
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

fn raid_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/storage/raid", get(handle_raid_arrays_list))
        .route("/api/storage/raid", post(handle_raid_create))
        .route(
            "/api/storage/raid/devices",
            get(handle_raid_available_devices),
        )
        .route(
            "/api/storage/raid/{name}/rebuild",
            post(handle_raid_rebuild),
        )
        // Storage's grow endpoint (mdadm --add + --grow) shares this file's
        // admin-gate test matrix.
        .route(
            "/api/storage/virtual-disks/{name}/grow",
            post(handle_virtual_disk_grow),
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

/// A full, well-formed create body — so the JSON extractor succeeds and the
/// request actually reaches `require_admin` (which then rejects it). The
/// device path is bogus on purpose; it is never used because no test sends
/// this with a valid admin token.
fn create_body() -> Value {
    json!({
        "name": "hvtest",
        "level": "raid1",
        "devices": ["/dev/hv-nonexistent-a", "/dev/hv-nonexistent-b"],
        "filesystem": "ext4"
    })
}

/// (method, uri, body) for every read endpoint (require_auth).
fn read_endpoints() -> Vec<(Method, &'static str)> {
    vec![
        (Method::GET, "/api/storage/raid"),
        (Method::GET, "/api/storage/raid/devices"),
    ]
}

/// (method, uri, body) for every write endpoint (require_admin). Bodies are
/// valid so the request reaches the auth gate rather than failing at 422.
fn write_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::POST, "/api/storage/raid", Some(create_body())),
        (
            Method::POST,
            "/api/storage/raid/hvtest/rebuild",
            Some(json!({})),
        ),
        (
            Method::POST,
            "/api/storage/virtual-disks/hvtest/grow",
            Some(json!({"device": "/dev/hv-nonexistent-a"})),
        ),
    ]
}

#[tokio::test]
async fn test_read_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = raid_router(state);
    for (method, uri) in read_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_read_endpoints_reject_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = raid_router(state);
    for (method, uri) in read_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, Some("nope"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_read_endpoints_allow_any_valid_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = raid_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for (method, uri) in read_endpoints() {
        let (status, body) = send(&mut router, method.clone(), uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}");
        assert!(
            body["data"].is_array(),
            "{uri} should return an array: {body}"
        );
    }
}

#[tokio::test]
async fn test_write_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = raid_router(state);
    for (method, uri, body) in write_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, None, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_write_endpoints_reject_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = raid_router(state);
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
