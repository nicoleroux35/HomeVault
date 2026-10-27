/* =============================================================================
 * File          : rescue_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Rescue integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Rescue API endpoints (`crate::handlers::rescue`):
//!
//!   - GET  /api/rescue/tools
//!   - GET  /api/rescue/devices
//!   - POST /api/rescue/ddrescue/clone
//!   - GET  /api/rescue/ddrescue/status/{logfile}
//!   - POST /api/rescue/testdisk/analyze
//!   - POST /api/rescue/testdisk/rebuild
//!   - POST /api/rescue/photorec/recover
//!   - POST /api/rescue/mdadm/diagnose
//!   - POST /api/rescue/mdadm/repair
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! Per CLAUDE.md's "What NOT to test in Rust", the mutation/analysis
//! endpoints (ddrescue/testdisk/photorec/mdadm) are only exercised far
//! enough to confirm they fail *cleanly* (400, never a panic) against
//! invalid/protected/nonexistent device paths, without ever touching a
//! real, accessible block device or invoking `sudo` — a real clone/repair
//! against disposable hardware is `tests/e2e/rescue/rescue.spec.ts`'s job.
//! `GET /tools` and `GET /devices` are safe to run for real here (no
//! `sudo`, read-only `--version`/`lsblk` calls).
//!
//! Run:
//!   cargo test --test rescue_integration
//!   cargo test --test rescue_integration -- --nocapture

use axum::{routing::get, routing::post, Router};
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
    rescue::{
        handle_ddrescue_clone, handle_ddrescue_mapfile_path, handle_ddrescue_status,
        handle_mdadm_diagnose, handle_mdadm_repair, handle_photorec_recover, handle_rescue_devices,
        handle_rescue_tools_available, handle_testdisk_analyze, handle_testdisk_rebuild,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

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

fn rescue_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/rescue/tools", get(handle_rescue_tools_available))
        .route("/api/rescue/devices", get(handle_rescue_devices))
        .route("/api/rescue/ddrescue/clone", post(handle_ddrescue_clone))
        .route(
            "/api/rescue/ddrescue/status/{logfile}",
            get(handle_ddrescue_status),
        )
        .route(
            "/api/rescue/testdisk/analyze",
            post(handle_testdisk_analyze),
        )
        .route(
            "/api/rescue/testdisk/rebuild",
            post(handle_testdisk_rebuild),
        )
        .route(
            "/api/rescue/photorec/recover",
            post(handle_photorec_recover),
        )
        .route(
            "/api/rescue/ddrescue/mapfile",
            get(handle_ddrescue_mapfile_path),
        )
        .route("/api/rescue/mdadm/diagnose", post(handle_mdadm_diagnose))
        .route("/api/rescue/mdadm/repair", post(handle_mdadm_repair))
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
    .expect("Failed to insert test user");
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

// ---------------------------------------------------------------------------
// 1. Auth guards — every endpoint requires a real admin session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rescue_tools_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(&mut router, Method::GET, "/api/rescue/tools", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_rescue_devices_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(&mut router, Method::GET, "/api/rescue/devices", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ddrescue_clone_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/ddrescue/clone",
        None,
        Some(json!({"source": "/dev/loop0", "destination": "/dev/loop1"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ddrescue_status_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/rescue/ddrescue/status/nonexistent.log",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_testdisk_analyze_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/analyze",
        None,
        Some(json!({"device": "/dev/loop0"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_testdisk_rebuild_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/rebuild",
        None,
        Some(json!({"device": "/dev/loop0"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_photorec_recover_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/photorec/recover",
        None,
        Some(json!({"device": "/dev/loop0", "output_dir": "/tmp/out"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_mdadm_diagnose_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/diagnose",
        None,
        Some(json!({"name": "md0"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_mdadm_repair_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/repair",
        None,
        Some(json!({"name": "md0"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_rescue_endpoints_reject_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regularuser", "Password123!", 1).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "regularuser", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/rescue/tools",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/rescue/devices",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/diagnose",
        Some(&token),
        Some(json!({"name": "md0"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_rescue_endpoints_reject_invalid_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = rescue_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/rescue/tools",
        Some("totally-bogus-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. Read-only success paths (safe against this host — no sudo)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rescue_tools_lists_all_four_tools() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rescue/tools",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let tools = body["data"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 4);
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"ddrescue"));
    assert!(names.contains(&"testdisk"));
    assert!(names.contains(&"photorec"));
    assert!(names.contains(&"mdadm"));
}

#[tokio::test]
async fn test_rescue_devices_returns_an_array() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rescue/devices",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
}

// ---------------------------------------------------------------------------
// 3. Validation — clean 400s without ever touching a real/accessible
//    device or invoking a privileged command
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ddrescue_clone_rejects_path_outside_dev() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/ddrescue/clone",
        Some(&token),
        Some(json!({"source": "/etc/passwd", "destination": "/dev/loop0"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Invalid source device"));
}

#[tokio::test]
async fn test_ddrescue_clone_rejects_nonexistent_device() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/ddrescue/clone",
        Some(&token),
        Some(json!({"source": "/dev/zzz_does_not_exist", "destination": "/dev/loop0"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_ddrescue_clone_rejects_protected_root_disk_as_destination() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // /dev/sda is both in the static PROTECTED_DEVICES list and (on most
    // real hosts, this dev box included) the active root disk — either way
    // this must be rejected before any real clone is attempted.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/ddrescue/clone",
        Some(&token),
        Some(json!({"source": "/dev/loop0", "destination": "/dev/sda"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Invalid destination device"));
}

#[tokio::test]
async fn test_ddrescue_clone_rejects_logfile_path_traversal() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/ddrescue/clone",
        Some(&token),
        Some(json!({
            "source": "/dev/loop0",
            "destination": "/dev/loop1",
            "logfile": "/tmp/../etc/passwd"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Invalid log file path"));
}

#[tokio::test]
async fn test_testdisk_analyze_rejects_protected_device() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/analyze",
        Some(&token),
        Some(json!({"device": "/dev/sda"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_testdisk_rebuild_rejects_nonexistent_device() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/rebuild",
        Some(&token),
        Some(json!({"device": "/dev/zzz_does_not_exist"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_photorec_recover_rejects_invalid_device() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/photorec/recover",
        Some(&token),
        Some(json!({"device": "/etc/passwd", "output_dir": "/tmp/out"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("Invalid device"));
}

#[tokio::test]
async fn test_photorec_recover_rejects_unauthorized_output_dir() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/photorec/recover",
        Some(&token),
        Some(json!({"device": "/dev/loop0", "output_dir": "/root/sensitive"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Invalid output directory"));
}

#[tokio::test]
async fn test_photorec_recover_rejects_output_dir_path_traversal() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/photorec/recover",
        Some(&token),
        Some(json!({"device": "/dev/loop0", "output_dir": "/tmp/../etc"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_mdadm_diagnose_rejects_nonexistent_array_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // "totally-fake-array" -> /dev/md/totally-fake-array, which does not
    // exist on this host -- validate_block_device fails at canonicalize()
    // before mdadm (real or sudo-wrapped) is ever invoked.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/diagnose",
        Some(&token),
        Some(json!({"name": "totally-fake-array"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_mdadm_diagnose_rejects_protected_device_given_directly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // A name starting with "/dev/" is used as-is (not prefixed with
    // /dev/md/) -- this exercises the protected-device / active-root-disk
    // rejection specifically, never a real `mdadm --detail` call.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/diagnose",
        Some(&token),
        Some(json!({"name": "/dev/sda"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_mdadm_repair_rejects_nonexistent_array_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/repair",
        Some(&token),
        Some(json!({"name": "totally-fake-array"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_mdadm_repair_rejects_protected_device_given_directly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/repair",
        Some(&token),
        Some(json!({"name": "/dev/sda"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 4. ddrescue status — real mapfile parsing (2026-09-05 fix)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ddrescue_status_missing_mapfile_reports_not_running_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let missing = "/tmp/hv_rescue_test_missing_mapfile_does_not_exist.log";
    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!("/api/rescue/ddrescue/status/{}", urlencoding_lite(missing)),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["running"], json!(false));
    assert_eq!(body["data"]["progress"], json!(0.0));
}

#[tokio::test]
async fn test_ddrescue_status_parses_a_real_finished_mapfile() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let mapfile_path = std::env::temp_dir().join(format!(
        "hv_rescue_test_finished_{}.log",
        std::process::id()
    ));
    std::fs::write(
        &mapfile_path,
        "# Mapfile. Created by GNU ddrescue version 1.27\n\
         # Command line: ddrescue -f src dst map.log\n\
         # Start time:   2026-09-05 16:56:09\n\
         # Current time: 2026-09-05 16:56:09\n\
         # Finished\n\
         # current_pos  current_status  current_pass\n\
         0x00010000     +               1\n\
         #      pos        size  status\n\
         0x00000000  0x00019000  +\n",
    )
    .unwrap();

    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!(
            "/api/rescue/ddrescue/status/{}",
            urlencoding_lite(mapfile_path.to_str().unwrap())
        ),
        Some(&token),
        None,
    )
    .await;

    std::fs::remove_file(&mapfile_path).ok();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["running"], json!(false));
    assert_eq!(body["data"]["progress"], json!(100.0));
    assert_eq!(body["data"]["message"], json!("Finished"));
}

/// Minimal percent-encoding for building a URI with a full filesystem path
/// as a single path segment — mirrors what the real frontend does with
/// `encodeURIComponent(logfile)` in `api.ts::ddrescueStatus`.
fn urlencoding_lite(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            '/' => "%2F".to_string(),
            other => format!("%{:02X}", other as u32),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 5. The status endpoint is confined to the rescue directories (2026-10-21)
// ---------------------------------------------------------------------------

/// `GET /api/rescue/ddrescue/status/{path}` took the raw URL segment and
/// `read_to_string`'d it — an arbitrary-file read (and a file-existence
/// oracle) for any admin session. It now has to be a regular file under one of
/// the allowed rescue directories, with no symlink anywhere in the way.
#[tokio::test]
async fn ddrescue_status_refuses_paths_outside_the_rescue_directories() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for outside in [
        "/etc/passwd",
        "/etc/shadow",
        "/root/.ssh/id_rsa",
        "/proc/self/environ",
    ] {
        let (status, body) = send(
            &mut router,
            Method::GET,
            &format!("/api/rescue/ddrescue/status/{}", urlencoding_lite(outside)),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{outside} must be refused: {body}"
        );
        assert_eq!(body["success"], json!(false), "{outside}: {body}");
    }
}

#[tokio::test]
async fn ddrescue_status_refuses_a_symlink_to_a_system_file() {
    // The classic bypass: a link inside /tmp pointing at /etc/shadow. A naive
    // canonicalize-then-check would *pass* (the canonical path is in /etc and
    // would be rejected) — but a naive "check the string, then read the path"
    // would happily read it. Assert the refusal, whichever layer catches it.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let dir = std::env::temp_dir().join(format!("hv_rescue_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join("map.log");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink("/etc/shadow", &link).unwrap();

    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!(
            "/api/rescue/ddrescue/status/{}",
            urlencoding_lite(link.to_str().unwrap())
        ),
        Some(&token),
        None,
    )
    .await;
    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("symbolic link"),
        "the rejection must name the reason: {body}"
    );
}

#[tokio::test]
async fn ddrescue_status_refuses_a_directory() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        &format!(
            "/api/rescue/ddrescue/status/{}",
            urlencoding_lite(std::env::temp_dir().to_str().unwrap())
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ddrescue_status_reads_a_mapfile_inside_a_rescue_directory() {
    // The legitimate case still works: a real mapfile in /tmp.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let mapfile = std::env::temp_dir().join(format!("hv_rescue_ok_{}.log", std::process::id()));
    std::fs::write(
        &mapfile,
        "# current_pos  current_status  current_pass\n\
         0x00008000     ?               1\n\
         #      pos        size  status\n\
         0x00000000  0x00008000  +\n\
         0x00008000  0x00011000  ?\n",
    )
    .unwrap();

    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!(
            "/api/rescue/ddrescue/status/{}",
            urlencoding_lite(mapfile.to_str().unwrap())
        ),
        Some(&token),
        None,
    )
    .await;
    std::fs::remove_file(&mapfile).ok();

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["running"], json!(true));
    assert_eq!(body["data"]["bytes_total"], json!(0x8000 + 0x11000));
}

#[tokio::test]
async fn ddrescue_mapfile_directory_is_reported() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rescue/ddrescue/mapfile",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["data"]["directory"]
            .as_str()
            .is_some_and(|d| d.contains("rescue")),
        "the default mapfile directory must be reported: {body}"
    );
}

// ---------------------------------------------------------------------------
// 6. Options that reach a tool's argv are validated (2026-10-21)
// ---------------------------------------------------------------------------

/// Clone options are turned into argv: a block size ddrescue refuses, or an
/// absurd retry count, is a 400 naming the field instead of a tool that fails
/// (or grinds) halfway through a disk clone.
#[tokio::test]
async fn ddrescue_clone_rejects_invalid_options_before_starting() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let cases: &[(Value, &str)] = &[
        (json!(0), "block_size must be between"),
        (json!(1000), "power of two"),
        (json!(2_097_152), "block_size must be between"),
    ];
    for (value, expected) in cases {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/rescue/ddrescue/clone",
            Some(&token),
            Some(json!({
                "source": "/dev/loop0",
                "destination": "/dev/loop1",
                "block_size": value
            })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "block_size {value}: {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or("").contains(expected),
            "block_size {value}: {body}"
        );
    }

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/ddrescue/clone",
        Some(&token),
        Some(json!({
            "source": "/dev/loop0",
            "destination": "/dev/loop1",
            "retry_passes": 999_999
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("retry_passes"),
        "{body}"
    );
}

/// PhotoRec file types are spliced into PhotoRec's **comma-separated** `/cmd`
/// grammar, so a value with a comma or a keyword injects extra commands. Only
/// bare lower-case extension identifiers are accepted.
#[tokio::test]
async fn photorec_rejects_script_injection_in_file_types() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for bad in [
        "jpg,fileopt,everything,enable",
        "jpg,search",
        "JPG",
        "jp g",
        "../../etc/passwd",
    ] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/rescue/photorec/recover",
            Some(&token),
            Some(json!({
                "device": "/dev/loop0",
                "output_dir": "/tmp/hv_rescue_injection",
                "file_types": [bad]
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "file type {bad:?}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or("")
                .contains("invalid file type"),
            "file type {bad:?}: {body}"
        );
    }
}

#[tokio::test]
async fn photorec_rejects_an_unknown_partition_type() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/photorec/recover",
        Some(&token),
        Some(json!({
            "device": "/dev/loop0",
            "output_dir": "/tmp/hv_rescue_parttype",
            "partition_type": "Amiga"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("partition_type"),
        "{body}"
    );
}

/// `mdadm --action=` was passed through verbatim from the request body;
/// anything outside the real sync-action vocabulary is now refused instead of
/// being handed to mdadm.
#[tokio::test]
async fn mdadm_repair_rejects_an_unknown_action() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/mdadm/repair",
        Some(&token),
        Some(json!({"name": "totally-fake-array", "action": "rm -rf"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("unknown mdadm action"),
        "{body}"
    );
}

/// The TestDisk options this integration cannot honour are refused loudly:
/// they used to be accepted, displayed as applied and never reach the tool.
#[tokio::test]
async fn testdisk_rejects_options_it_cannot_honour() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rescue_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/analyze",
        Some(&token),
        Some(json!({"device": "/dev/sda", "deep_search": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("deep_search"),
        "{body}"
    );

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/analyze",
        Some(&token),
        Some(json!({"device": "/dev/sda", "log_path": "/tmp/testdisk.log"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("log_path"),
        "{body}"
    );

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/rebuild",
        Some(&token),
        Some(json!({"device": "/dev/sda", "force": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("force"),
        "{body}"
    );

    // …and the unsupported-option rejection happens *before* the device check,
    // so an invalid option on a protected device still names the option.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rescue/testdisk/rebuild",
        Some(&token),
        Some(json!({"device": "/dev/zzz_nope", "force": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("force"),
        "{body}"
    );
}
