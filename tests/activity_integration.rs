/* =============================================================================
 * File          : activity_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Activity integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the unified activity feed — `GET /api/activity`
//! (`crate::handlers::activity::handle_activity`).
//!
//! The desktop taskbar polls this endpoint. It merges the active
//! `PROGRESS_MANAGER` jobs (restic, cache mover, antivirus, Explorer copy/move,
//! Docker install/update, ddrescue/photorec, RAG index, Ollama pull, cloud
//! sync) with aria2's active downloads. Rules under test:
//!
//!   1. `require_auth` — no token -> 400, garbage token -> 401.
//!   2. Live jobs appear; a job that failed/was cancelled stays for one minute
//!      (with its `state` slug), and a completed one never appears.
//!   3. `progress` is `null` until the job emits a percentage (indeterminate).
//!   4. Owner scoping — a non-admin sees only jobs they own; an admin sees all.
//!   5. Polling never spawns aria2.
//!
//! Fresh in-memory SQLite + migrations per test, minimal Axum router,
//! `tower::ServiceExt` — no real server. `PROGRESS_MANAGER` is a process-global
//! singleton, so every test clears it first (the file's `ENV_LOCK` already
//! serialises the tests).
//!
//! Run: `cargo test --test activity_integration`

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{activity::handle_activity, auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::middleware::security_headers_middleware;
use homevault::sse::{JobType, PROGRESS_MANAGER};
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Wipe the process-global job registry so a test starts from a known state.
fn clear_jobs() {
    for (id, _, _) in PROGRESS_MANAGER.list_active_jobs() {
        PROGRESS_MANAGER.remove_job(&id);
    }
}

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_jobs();
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

fn activity_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/activity", get(handle_activity))
        .with_state(state)
        .layer(axum::middleware::from_fn(security_headers_middleware))
}

async fn create_test_user(state: &AppState, username: &str, password: &str, role_id: i64) {
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
    let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
}

/// The rows in a successful `/api/activity` response body.
fn rows(body: &Value) -> &Vec<Value> {
    body["data"].as_array().expect("data array")
}

fn row_by_id<'a>(body: &'a Value, id: &str) -> Option<&'a Value> {
    rows(body).iter().find(|r| r["id"] == json!(id))
}

// ---------------------------------------------------------------------------
// require_auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_rejects_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = activity_router(state);
    let (status, _) = send(&mut r, Method::GET, "/api/activity", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn activity_rejects_garbage_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = activity_router(state);
    let (status, _) = send(&mut r, Method::GET, "/api/activity", Some("nope"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// job lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_empty_list_when_no_jobs() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;
    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(rows(&body).len(), 0);
}

#[tokio::test]
async fn activity_lists_running_job_with_progress() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let id = PROGRESS_MANAGER.create_job(JobType::Backup, "Starting backup");
    PROGRESS_MANAGER.emit_progress(&id, 42.0, "42/100 files");

    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let row = row_by_id(&body, &id).expect("the backup row");
    assert_eq!(row["kind"], json!("backup"));
    assert_eq!(row["label"], json!(""));
    assert_eq!(row["progress"], json!(42.0));
    assert_eq!(row["detail"], json!("42/100 files"));
}

#[tokio::test]
async fn activity_excludes_completed_job() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let id = PROGRESS_MANAGER.create_job(JobType::Verify, "start");
    PROGRESS_MANAGER.complete_job(&id, "done");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert!(
        row_by_id(&body, &id).is_none(),
        "completed job must not appear"
    );
}

/// ROADMAP 28.2. A job that failed **one second ago** used to disappear on the
/// very next poll — the row the user was watching simply vanished, and the only
/// trace left was a notification they were not looking at. It now stays for one
/// minute, carrying `state: "failed"` so the client renders the outcome instead
/// of a stalled bar.
#[tokio::test]
async fn activity_shows_a_job_that_just_failed() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let id = PROGRESS_MANAGER.create_job(JobType::Restore, "start");
    PROGRESS_MANAGER.fail_job(&id, "disk full");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    let row = row_by_id(&body, &id).expect("a fresh failure must stay visible");
    assert_eq!(
        row["state"],
        json!("failed"),
        "the row must say it failed: {row}"
    );
    assert_eq!(row["kind"], json!("restore"));
    // The reason travels in `detail` (the failure message), never in `label`.
    assert!(
        row["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("disk full"),
        "{row}"
    );
}

/// The same for a cancellation: the user pressed Cancel, so the row must
/// confirm the job stopped instead of blinking out.
#[tokio::test]
async fn activity_shows_a_job_that_was_just_cancelled() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let id = PROGRESS_MANAGER.create_job(JobType::Backup, "start");
    // `mark_cancelled` is the state a job reaches while its producer is still
    // winding down (a cancel hook ran; the task has not dropped the id yet) —
    // `cancel_job` without a hook removes the row outright, which is the
    // "nothing to report" case.
    PROGRESS_MANAGER.mark_cancelled(&id, "Cancelled by admin");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    let row = row_by_id(&body, &id).expect("a fresh cancellation must stay visible");
    assert_eq!(row["state"], json!("cancelled"), "{row}");
}

/// Every row carries its lifecycle slug — the field the client branches on to
/// decide between a progress bar and a status word.
#[tokio::test]
async fn every_row_carries_its_state() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let running = PROGRESS_MANAGER.create_job(JobType::Backup, "start");
    PROGRESS_MANAGER.emit_progress(&running, 10.0, "10%");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    let rows = body["data"].as_array().expect("data array");
    assert!(!rows.is_empty());
    for row in rows {
        let state = row["state"].as_str().unwrap_or("<missing>");
        assert!(
            ["pending", "running", "failed", "cancelled"].contains(&state),
            "unexpected state slug {state:?} in {row}"
        );
    }
    assert_eq!(
        row_by_id(&body, &running).expect("the live row")["state"],
        json!("running")
    );
}

#[tokio::test]
async fn activity_docker_install_job_is_indeterminate() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let id = PROGRESS_MANAGER.create_job(
        JobType::Custom("docker_install".to_string()),
        "Installing Jellyfin",
    );

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    let row = row_by_id(&body, &id).expect("the docker_install row");
    assert_eq!(row["kind"], json!("docker_install"));
    assert_eq!(row["progress"], Value::Null, "no percentage reported yet");
}

#[tokio::test]
async fn activity_does_not_spawn_aria2() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let downloader = state.downloader.clone();
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    for _ in 0..3 {
        let (status, _) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
    }
    assert!(
        downloader.lock().await.child.is_none(),
        "polling /api/activity must never start the aria2 daemon"
    );
}

// ---------------------------------------------------------------------------
// owner scoping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_non_admin_sees_only_owned_jobs() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await; // non-admin
    let mut r = activity_router(state);
    let token = login(&mut r, "alice", "Password123!").await;

    let mine = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("explorer_copy".to_string()),
        "Starting copy",
        Some("alice".to_string()),
    );
    let system = PROGRESS_MANAGER.create_job(JobType::Backup, "system backup");

    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(row_by_id(&body, &mine).is_some(), "own job is visible");
    assert!(
        row_by_id(&body, &system).is_none(),
        "an ownerless (admin) job must be hidden from a non-admin"
    );
}

#[tokio::test]
async fn activity_non_admin_no_foreign_path_leak() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "alice", "Password123!").await;

    let bobs = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("explorer_move".to_string()),
        "Starting move",
        Some("bob".to_string()),
    );
    PROGRESS_MANAGER.emit_progress(&bobs, 10.0, "/home/bob/private/secret-folder");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert!(
        row_by_id(&body, &bobs).is_none(),
        "another user's job is invisible"
    );
    assert!(
        !body.to_string().contains("secret-folder"),
        "another user's path must never appear in the response"
    );
}

#[tokio::test]
async fn activity_admin_sees_every_job() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let system = PROGRESS_MANAGER.create_job(JobType::Backup, "system backup");
    let alices = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("explorer_copy".to_string()),
        "Starting copy",
        Some("alice".to_string()),
    );

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert!(row_by_id(&body, &system).is_some());
    assert!(row_by_id(&body, &alices).is_some());
}
