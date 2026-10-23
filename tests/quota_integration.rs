/* =============================================================================
 * File          : quota_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 22/10/2026 by Nicolas Le Roux
 * Last modified : 22/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Home-directory disk-quota integration tests: the `quota` block returned by
 *   GET /api/users/{username}/home must report the state the filesystem
 *   actually enforces, never the value that was merely saved. Runs against a
 *   live in-memory SQLite + assembled Axum router via `oneshot`; the host's
 *   real mount table is inspected (read-only `findmnt`, no `sudo`, no
 *   `setquota`), so the assertions hold on any developer machine or CI runner
 *   regardless of the filesystem in use.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test quota_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the truthful home-quota reporting (ROADMAP 3.1/3.2):
//!
//!   - GET /api/users/{username}/home → `quota.enforced` is only true after a
//!     successful `repquota` / `zfs get` read-back, and every `false` answer
//!     carries a machine-readable `reason_code` plus a diagnostic `reason`
//!   - a stored limit is never echoed back as "enforced"
//!   - no `quota` object when the home folder is disabled or absent
//!   - PUT keeps its auth and input-validation contract
//!
//! Live enforcement (`setquota`, `quotaon`, `zfs set userquota`) cannot be
//! exercised here — it needs a filesystem mounted with `usrquota` and a real
//! system account — and stays covered by `tests/e2e/users/users.spec.ts`
//! against a live host, per CLAUDE.md's "What NOT to test in Rust".

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
    user_homes::{handle_user_home_get, handle_user_home_put},
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations().await.expect("Failed to run migrations");

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

fn quota_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route(
            "/api/users/{username}/home",
            get(handle_user_home_get).put(handle_user_home_put),
        )
        .with_state(state)
}

/// Insert a user directly into the DB (no Linux account is created).
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
    .expect("Failed to insert test user");
}

/// Insert a home-folder row directly, bypassing the real `mkdir`/`chown`/
/// `setquota` path (which needs a Linux account and a quota-capable mount).
async fn insert_home(
    state: &AppState,
    username: &str,
    enabled: bool,
    path: Option<&str>,
    size_limit_bytes: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO user_homes (username, enabled, disk, path, created_at, size_limit_bytes)
         VALUES (?, ?, NULL, ?, datetime('now'), ?)",
    )
    .bind(username)
    .bind(enabled)
    .bind(path)
    .bind(size_limit_bytes)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert home row");
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

/// Every reason code the backend may return. Keeping the list here makes a
/// new variant fail this test until the UI is taught to localise it.
const REASON_CODES: &[&str] = &[
    "not_mounted_with_quota",
    "unsupported_filesystem",
    "accounting_inactive",
    "no_entry",
    "limit_mismatch",
    "command_failed",
    "inspect_failed",
    "invalid_username",
];

// ===========================================================================
// 1. Auth
// ===========================================================================

#[tokio::test]
async fn test_home_get_requires_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = quota_router(state);

    let (_, body) = send(&mut r, Method::GET, "/api/users/alice/home", None, None).await;
    assert_eq!(body["success"], json!(false));
    assert_eq!(body["message"], json!("Invalid session"));
}

#[tokio::test]
async fn test_home_put_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = quota_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;

    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/home",
        Some(&token),
        Some(json!({"enabled": true, "disk": "/home", "size_bytes": 1024})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert_eq!(body["message"], json!("Admin access required"));
}

#[tokio::test]
async fn test_home_put_rejects_negative_size() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = quota_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/victim/home",
        Some(&token),
        Some(json!({"enabled": true, "disk": "/home", "size_bytes": -1})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert_eq!(
        body["message"],
        json!("Home directory size cannot be negative")
    );
}

// ===========================================================================
// 2. No home / disabled home → no quota claim at all
// ===========================================================================

#[tokio::test]
async fn test_home_get_without_row_returns_no_quota() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = quota_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/home",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["enabled"], json!(false));
    assert!(body["data"]["quota"].is_null());
}

#[tokio::test]
async fn test_home_get_disabled_home_returns_no_quota() {
    let (state, tmp, _g) = setup_state().await;
    let home = tmp.path().join("home-bob");
    std::fs::create_dir_all(&home).expect("create home dir");
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    insert_home(
        &state,
        "admin1",
        false,
        Some(home.to_string_lossy().as_ref()),
        Some(1024),
    )
    .await;

    let mut r = quota_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/home",
        Some(&token),
        None,
    )
    .await;

    // A disabled folder keeps its stored size limit but must not claim one.
    assert_eq!(body["data"]["enabled"], json!(false));
    assert_eq!(body["data"]["size_bytes"], json!(1024));
    assert!(body["data"]["quota"].is_null());
}

// ===========================================================================
// 3. The core contract: a stored limit is never reported as enforced
// ===========================================================================

#[tokio::test]
async fn test_stored_limit_is_reported_not_enforced_with_reason_code() {
    let (state, tmp, _g) = setup_state().await;
    let home = tmp.path().join("home-carol");
    std::fs::create_dir_all(&home).expect("create home dir");
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    insert_home(
        &state,
        "admin1",
        true,
        Some(home.to_string_lossy().as_ref()),
        Some(1_048_576),
    )
    .await;

    let mut r = quota_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/home",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let quota = &body["data"]["quota"];
    assert!(
        quota.is_object(),
        "an enabled home folder must carry a quota object, got {quota}"
    );
    // The test host has no `usrquota` mount for this temporary directory and
    // no Linux account named `admin1`, so enforcement is impossible. The
    // answer must say so instead of echoing the saved value.
    assert_eq!(quota["enforced"], json!(false));
    assert_eq!(quota["limit_bytes"], json!(1_048_576));
    assert_eq!(quota["used_bytes"], json!(null));

    let code = quota["reason_code"]
        .as_str()
        .unwrap_or_else(|| panic!("reason_code must be a string, got {quota}"));
    assert!(
        REASON_CODES.contains(&code),
        "unknown reason_code {code:?} — add it to the UI i18n mapping"
    );
    let reason = quota["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("reason must be a string, got {quota}"));
    assert!(!reason.trim().is_empty(), "reason must never be empty");
    let filesystem = quota["filesystem"]
        .as_str()
        .unwrap_or_else(|| panic!("filesystem must be a string, got {quota}"));
    assert!(!filesystem.trim().is_empty());
    let mount_point = quota["mount_point"]
        .as_str()
        .unwrap_or_else(|| panic!("mount_point must be a string, got {quota}"));
    assert!(!mount_point.trim().is_empty());
}

#[tokio::test]
async fn test_unlimited_home_reports_accounting_state() {
    let (state, tmp, _g) = setup_state().await;
    let home = tmp.path().join("home-dave");
    std::fs::create_dir_all(&home).expect("create home dir");
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    insert_home(
        &state,
        "admin1",
        true,
        Some(home.to_string_lossy().as_ref()),
        None,
    )
    .await;

    let mut r = quota_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/home",
        Some(&token),
        None,
    )
    .await;

    let quota = &body["data"]["quota"];
    assert_eq!(quota["limit_bytes"], json!(null));
    // No limit configured: `enforced` describes the accounting state, so it is
    // either true (accounting on) or false with a localisable reason.
    if quota["enforced"] == json!(false) {
        assert!(quota["reason_code"].is_string());
        assert!(quota["reason"].is_string());
    }
}

#[tokio::test]
async fn test_uninspectable_home_path_is_inspect_failed() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    // A path that does not exist: `findmnt --target` fails on every host, so
    // this specific reason code is deterministic.
    insert_home(
        &state,
        "admin1",
        true,
        Some("/nonexistent/homevault-quota-test"),
        Some(2048),
    )
    .await;

    let mut r = quota_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/home",
        Some(&token),
        None,
    )
    .await;

    let quota = &body["data"]["quota"];
    assert_eq!(quota["enforced"], json!(false));
    assert_eq!(quota["reason_code"], json!("inspect_failed"));
    assert_eq!(quota["filesystem"], json!("unknown"));
    assert_eq!(
        quota["mount_point"],
        json!("/nonexistent/homevault-quota-test")
    );
    // The stored limit is still reported so the UI can say *what* is not
    // enforced.
    assert_eq!(quota["limit_bytes"], json!(2048));
}
