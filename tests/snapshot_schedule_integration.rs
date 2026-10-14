/* =============================================================================
 * File          : snapshot_schedule_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Snapshot-schedule integration tests: in-memory SQLite + assembled Axum
 *   router, exercising CRUD, auth guards and validation via oneshot
 *   requests. ZFS/Btrfs execution is deliberately NOT exercised (real pool
 *   operations — covered by tests/e2e/storage on a live host); run-now of a
 *   schedule on a pool that doesn't exist fails cleanly.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the snapshot-schedules API:
//!
//!   - GET    /api/snapshot-schedules
//!   - POST   /api/snapshot-schedules
//!   - PUT    /api/snapshot-schedules/{id}
//!   - DELETE /api/snapshot-schedules/{id}
//!   - POST   /api/snapshot-schedules/{id}/run
//!   - GET    /api/snapshot-schedules/snapshots?pool=

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::snapshot_schedules::{
    handle_snapshot_pool_list, handle_snapshot_schedule_create, handle_snapshot_schedule_delete,
    handle_snapshot_schedule_run_now, handle_snapshot_schedule_update,
    handle_snapshot_schedules_list,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::models::{ADMIN_ROLE_ID, USER_ROLE_ID};
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

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

fn schedules_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/auth/login",
            axum::routing::post(homevault::handlers::auth::handle_login),
        )
        .route(
            "/api/snapshot-schedules",
            get(handle_snapshot_schedules_list),
        )
        .route(
            "/api/snapshot-schedules",
            axum::routing::post(handle_snapshot_schedule_create),
        )
        .route(
            "/api/snapshot-schedules/{id}",
            axum::routing::put(handle_snapshot_schedule_update),
        )
        .route(
            "/api/snapshot-schedules/{id}",
            axum::routing::delete(handle_snapshot_schedule_delete),
        )
        .route(
            "/api/snapshot-schedules/{id}/run",
            axum::routing::post(handle_snapshot_schedule_run_now),
        )
        .route(
            "/api/snapshot-schedules/snapshots",
            get(handle_snapshot_pool_list),
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
    .expect("insert user");
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
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"username": username, "password": password}).to_string(),
        ))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["data"]["token"].as_str().expect("token").to_string()
}

fn valid_schedule() -> Value {
    json!({
        "pool": "tank",
        "cron": "0 */4 * * *",
        "prefix": "hv-auto",
        "recursive": true,
        "gfs_hourly": 24,
        "gfs_daily": 7,
        "gfs_weekly": 4,
        "gfs_monthly": 12,
        "gfs_yearly": 1,
        "enabled": true
    })
}

#[tokio::test]
async fn crud_cycle_round_trips_gfs_counts() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = schedules_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(valid_schedule()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    let id = body["data"]["id"].as_str().expect("id").to_string();
    assert_eq!(body["data"]["gfs_hourly"], 24);
    assert_eq!(body["data"]["gfs_yearly"], 1);
    assert!(
        body["data"]["next_run_at"].is_string(),
        "next run precomputed"
    );

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["schedules"].as_array().unwrap().len(), 1);

    // Update: disable + change retention.
    let mut edited = valid_schedule();
    edited["enabled"] = json!(false);
    edited["gfs_daily"] = json!(14);
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/snapshot-schedules/{id}"),
        Some(&admin),
        Some(edited),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update failed: {body}");
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["gfs_daily"], 14);
    assert!(body["data"]["last_status"].is_null(), "edit resets status");

    // Delete.
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/snapshot-schedules/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["schedules"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn schedules_are_admin_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = schedules_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;
    let user = login(&mut router, "user", "UserPass123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules",
        None,
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK, "anonymous must not list schedules");
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // Destroying snapshots must be even more strictly guarded.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules/x/run",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        "/api/snapshot-schedules/x",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn validation_rejects_bad_pool_cron_and_prefix() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = schedules_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // Invalid pool name (would be spliced into zfs/btrfs argv).
    let mut bad = valid_schedule();
    bad["pool"] = json!("a b");
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Invalid cron.
    let mut bad = valid_schedule();
    bad["cron"] = json!("whenever");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("cron"), "{body}");

    // Invalid prefix (would break the prune filter).
    let mut bad = valid_schedule();
    bad["prefix"] = json!("bad prefix!");
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut bad = valid_schedule();
    bad["prefix"] = json!("");
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Negative GFS counts are structurally impossible (u32), but absurd
    // retention counts should be accepted and round-trip.
    let mut big = valid_schedule();
    big["gfs_hourly"] = json!(1024);
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(big),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["gfs_hourly"], 1024);
}

#[tokio::test]
async fn pool_snapshot_list_validates_and_reports_unavailable_pools() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = schedules_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // Invalid pool name is rejected before any zfs/btrfs call.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules/snapshots?pool=a%20b",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A pool that doesn't exist fails cleanly (no zfs/btrfs on CI → clear
    // error; either way never a panic).
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules/snapshots?pool=nope",
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "must be a clean error, not a panic"
    );
}

#[tokio::test]
async fn run_now_on_a_missing_pool_fails_cleanly_and_records_the_error() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = schedules_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules",
        Some(&admin),
        Some(valid_schedule()),
    )
    .await;
    let id = body["data"]["id"].as_str().expect("id").to_string();

    // No zfs/btrfs pool "tank" exists in CI — the run fails with a clear
    // error envelope, and the failure is recorded on the schedule.
    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/snapshot-schedules/{id}/run"),
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "clean failure expected: {body}"
    );
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshot-schedules",
        Some(&admin),
        None,
    )
    .await;
    let schedule = &body["data"]["schedules"][0];
    if !body["data"]["schedules"].as_array().unwrap().is_empty() {
        assert_eq!(
            schedule["last_status"], "failed",
            "failure must be recorded: {schedule}"
        );
        assert!(schedule["last_error"].as_str().is_some());
    }
}

#[tokio::test]
async fn unknown_schedule_ids_fail_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = schedules_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/snapshot-schedules/nope",
        Some(&admin),
        Some(valid_schedule()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/snapshot-schedules/nope/run",
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        "/api/snapshot-schedules/nope",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "delete of an unknown id is idempotent"
    );
}
