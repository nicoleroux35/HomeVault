/* =============================================================================
 * File          : cache_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 22/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Cache-tier integration tests: in-memory SQLite + assembled Axum router,
 *   exercising the settings/share validation that used to be missing, the
 *   `cache_pool` clearing path, the per-share mover history and the stale-run
 *   reconciliation.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the disk cache tier API:
//!
//!   - GET/PUT /api/cache/settings
//!   - GET     /api/cache/pools
//!   - GET/POST /api/cache/shares
//!   - PUT/DELETE /api/cache/shares/{name}
//!   - POST    /api/cache/mover/run
//!   - GET     /api/cache/mover/runs

use axum::routing::{get, post};
use axum::Router;
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::cache::{
    handle_cache_mover_run, handle_cache_mover_runs, handle_cache_pools_get,
    handle_cache_settings_get, handle_cache_settings_update, handle_cache_share_create,
    handle_cache_share_delete, handle_cache_share_update, handle_cache_shares_list,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::cache_repo::{CacheRepository, ShareOutcomeRecord};
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
    let admin_rate_limiter = Arc::new(RateLimitState::new(1000, 60));
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

fn cache_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/auth/login",
            post(homevault::handlers::auth::handle_login),
        )
        .route("/api/cache/settings", get(handle_cache_settings_get))
        .route(
            "/api/cache/settings",
            axum::routing::put(handle_cache_settings_update),
        )
        .route("/api/cache/pools", get(handle_cache_pools_get))
        .route("/api/cache/shares", get(handle_cache_shares_list))
        .route("/api/cache/shares", post(handle_cache_share_create))
        .route(
            "/api/cache/shares/{name}",
            axum::routing::put(handle_cache_share_update),
        )
        .route(
            "/api/cache/shares/{name}",
            axum::routing::delete(handle_cache_share_delete),
        )
        .route("/api/cache/mover/run", post(handle_cache_mover_run))
        .route("/api/cache/mover/runs", get(handle_cache_mover_runs))
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

/// Seeds the singleton settings row with a cache pool designation, bypassing
/// the API (no real pool exists on a test host).
async fn seed_cache_pool(state: &AppState, pool: &str) {
    let repo = CacheRepository::new(state.db.pool());
    let mut settings = repo.get_settings().await.expect("settings");
    settings.cache_pool = Some(pool.to_string());
    repo.save_settings(&settings).await.expect("save settings");
}

async fn seed_share(state: &AppState, name: &str, mode: &str, target: &str) {
    sqlx::query(
        "INSERT INTO cache_shares (id, name, cache_mode, target_pool, enabled, created_at, updated_at)
         VALUES (?, ?, ?, ?, 1, datetime('now'), datetime('now'))",
    )
    .bind(format!("id-{name}"))
    .bind(name)
    .bind(mode)
    .bind(target)
    .execute(&*state.db.pool())
    .await
    .expect("insert cache share");
}

fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or("")
        .to_string()
}

/// Validation failures come back as a 400 with a human message.
fn assert_rejected(status: StatusCode, body: &Value, needle: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        error_message(body).contains(needle),
        "expected {needle:?} in {body}"
    );
}

// ── Auth split ────────────────────────────────────────────────────────

#[tokio::test]
async fn reads_require_auth_and_writes_require_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let user = login(&mut router, "user", "UserPass123!").await;

    // Reads: anonymous → refused, plain user → 200. (The shared guard answers a
    // missing token with a 400 and an invalid one with a 401.)
    let (status, body) = send(&mut router, Method::GET, "/api/cache/settings", None, None).await;
    assert!(!status.is_success(), "{body}");
    assert!(
        error_message(&body).contains("session token"),
        "anonymous reads must be refused: {body}"
    );
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/cache/shares",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/cache/mover/runs",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Writes: a plain user is refused on every mutating route.
    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/cache/settings",
            json!({"mover_enabled": true}),
        ),
        (
            Method::POST,
            "/api/cache/shares",
            json!({"name": "media", "cache_mode": "yes", "target_pool": "array"}),
        ),
        (
            Method::PUT,
            "/api/cache/shares/media",
            json!({"enabled": false}),
        ),
        (Method::DELETE, "/api/cache/shares/media", Value::Null),
        (Method::POST, "/api/cache/mover/run", Value::Null),
    ] {
        let body = if body.is_null() { None } else { Some(body) };
        let (status, _) = send(&mut router, method.clone(), uri, Some(&user), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} must be admin-only"
        );
    }
}

// ── Settings validation ───────────────────────────────────────────────

#[tokio::test]
async fn settings_reject_a_malformed_cron() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&admin),
        Some(json!({"mover_cron": "nope"})),
    )
    .await;
    assert_rejected(status, &body, "cron");
}

#[tokio::test]
async fn settings_reject_a_zero_or_over_95_free_space_reserve() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // 0 would mean "drain the cache to nothing" for `prefer` shares; the UI
    // has always clamped 1-95 and the server used to accept 0 and 100.
    for bad in [0, 96, 100] {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/cache/settings",
            Some(&admin),
            Some(json!({"min_free_percent": bad})),
        )
        .await;
        assert_rejected(status, &body, "min_free_percent");
    }

    for ok in [1, 50, 95] {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/cache/settings",
            Some(&admin),
            Some(json!({"min_free_percent": ok})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or(false), "{body}");
        let (_, settings) = send(
            &mut router,
            Method::GET,
            "/api/cache/settings",
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(settings["data"]["min_free_percent"], ok);
    }
}

#[tokio::test]
async fn settings_reject_an_unknown_cache_pool() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&admin),
        Some(json!({"cache_pool": "definitely-not-a-pool"})),
    )
    .await;
    assert_rejected(status, &body, "does not exist");
}

#[tokio::test]
async fn an_empty_cache_pool_clears_the_designation() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    seed_cache_pool(&state, "cachepool").await;

    // An absent field leaves the designation alone.
    let (_, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&admin),
        Some(json!({"mover_enabled": true})),
    )
    .await;
    assert!(body["success"].as_bool().unwrap_or(false), "{body}");
    let (_, settings) = send(
        &mut router,
        Method::GET,
        "/api/cache/settings",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(settings["data"]["cache_pool"], "cachepool");

    // An empty string clears it (previously only `null` could).
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&admin),
        Some(json!({"cache_pool": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["success"].as_bool().unwrap_or(false), "{body}");
    let (_, settings) = send(
        &mut router,
        Method::GET,
        "/api/cache/settings",
        Some(&admin),
        None,
    )
    .await;
    assert!(
        settings["data"]["cache_pool"].is_null(),
        "an empty string must clear the designation: {settings}"
    );

    // Whitespace behaves the same way.
    seed_cache_pool(&state, "cachepool").await;
    let (_, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&admin),
        Some(json!({"cache_pool": "   "})),
    )
    .await;
    assert!(body["success"].as_bool().unwrap_or(false), "{body}");
    let (_, settings) = send(
        &mut router,
        Method::GET,
        "/api/cache/settings",
        Some(&admin),
        None,
    )
    .await;
    assert!(settings["data"]["cache_pool"].is_null(), "{settings}");
}

// ── Shares ────────────────────────────────────────────────────────────

#[tokio::test]
async fn share_create_validates_the_name_mode_and_target() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/cache/shares",
        Some(&admin),
        Some(json!({"name": "bad/name", "cache_mode": "yes", "target_pool": "array"})),
    )
    .await;
    assert_rejected(status, &body, "name");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/cache/shares",
        Some(&admin),
        Some(json!({"name": "media", "cache_mode": "turbo", "target_pool": "array"})),
    )
    .await;
    assert_rejected(status, &body, "cache_mode");

    // No cache pool configured yet: a share cannot be created.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/cache/shares",
        Some(&admin),
        Some(json!({"name": "media", "cache_mode": "yes", "target_pool": "array"})),
    )
    .await;
    assert_rejected(status, &body, "No cache pool configured");
}
#[tokio::test]
async fn share_update_validates_the_target_pool() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    seed_cache_pool(&state, "cachepool").await;
    seed_share(&state, "media", "yes", "array").await;

    // An unknown pool used to be accepted verbatim, leaving the share
    // permanently unmovable.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/shares/media",
        Some(&admin),
        Some(json!({"target_pool": "ghost"})),
    )
    .await;
    assert_rejected(status, &body, "does not exist");

    // The cache pool itself is not a valid target.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/shares/media",
        Some(&admin),
        Some(json!({"target_pool": "cachepool"})),
    )
    .await;
    assert_rejected(status, &body, "same as the cache pool");

    // A mode-only update still works and does not touch the target.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/shares/media",
        Some(&admin),
        Some(json!({"cache_mode": "prefer"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["cache_mode"], "prefer");
    assert_eq!(body["data"]["target_pool"], "array");

    // An unknown share is a clean 400-with-message, never a 500.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/shares/ghost",
        Some(&admin),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_rejected(status, &body, "not found");
}

// ── Mover run history ─────────────────────────────────────────────────

#[tokio::test]
async fn a_failed_run_is_recorded_and_reported() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // No cache pool designated: the run fails before touching any share.
    let err = homevault::cache::run_mover(state.db.pool(), None)
        .await
        .expect_err("a run without a cache pool must fail");
    assert!(err.contains("No cache pool"), "{err}");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/cache/mover/runs",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let runs = body["data"].as_array().expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"], "failed");
    assert!(runs[0]["error_message"]
        .as_str()
        .unwrap()
        .contains("No cache pool"));
}

#[tokio::test]
async fn a_restart_does_not_leave_a_run_stuck_on_running() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // A row left `running` by the previous process (the single-flight guard is
    // process-local, so nothing proves it is still alive).
    let repo = CacheRepository::new(state.db.pool());
    let stale_id = repo.create_run("running").await.expect("run");

    let _ = homevault::cache::run_mover(state.db.pool(), None).await;

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/cache/mover/runs",
        Some(&admin),
        None,
    )
    .await;
    let runs = body["data"].as_array().expect("runs");
    let stale = runs
        .iter()
        .find(|r| r["id"] == stale_id)
        .expect("the stale run is still listed");
    assert_eq!(stale["status"], "failed");
    assert!(stale["error_message"]
        .as_str()
        .unwrap()
        .contains("Interrupted by a server restart"));
    assert!(stale["finished_at"].is_string(), "it must be closed out");
}

#[tokio::test]
async fn per_share_outcomes_round_trip_through_the_history() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let repo = CacheRepository::new(state.db.pool());
    let run_id = repo.create_run("running").await.expect("run");
    repo.record_run_shares(
        &run_id,
        &[
            ShareOutcomeRecord {
                share_name: "media".into(),
                mode: "yes".into(),
                status: "moved".into(),
                files_moved: 3,
                bytes_moved: 4096,
                message: None,
            },
            ShareOutcomeRecord {
                share_name: "backups".into(),
                mode: "prefer".into(),
                status: "failed".into(),
                files_moved: 0,
                bytes_moved: 0,
                message: Some(
                    "not enough space on /mnt/array: 1.0 GiB to move, 12.0 MiB free".into(),
                ),
            },
        ],
    )
    .await
    .expect("record shares");
    repo.update_run(
        &run_id,
        "partial",
        3,
        4096,
        Some("Shares not moved: backups"),
    )
    .await
    .expect("update run");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/cache/mover/runs",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let run = &body["data"][0];
    assert_eq!(run["status"], "partial");
    let shares = run["shares"].as_array().expect("shares");
    assert_eq!(shares.len(), 2);
    assert_eq!(shares[0]["share_name"], "backups");
    assert_eq!(shares[0]["status"], "failed");
    assert!(shares[0]["message"]
        .as_str()
        .unwrap()
        .contains("not enough space"));
    assert_eq!(shares[1]["share_name"], "media");
    assert_eq!(shares[1]["status"], "moved");
    assert_eq!(shares[1]["files_moved"], 3);
    assert!(shares[1]["message"].is_null());
}

#[tokio::test]
async fn runs_without_share_rows_still_serialize_an_empty_list() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = cache_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // A run recorded before migration 052 has no per-share rows; the client
    // must still see `shares: []` rather than a missing field.
    let repo = CacheRepository::new(state.db.pool());
    let run_id = repo.create_run("running").await.expect("run");
    repo.update_run(&run_id, "completed", 1, 2, None)
        .await
        .expect("update run");

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/cache/mover/runs",
        Some(&admin),
        None,
    )
    .await;
    let shares = body["data"][0]["shares"].as_array().expect("shares");
    assert!(shares.is_empty(), "{body}");
}
