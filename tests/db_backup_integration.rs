/* =============================================================================
 * File          : db_backup_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 17/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the application-database snapshot endpoints
 *   (`GET|PUT /api/settings/backup/db`, `POST /api/settings/backup/db/now`)
 *   and the public health probes (`GET /api/health/live`,
 *   `GET /api/health/ready`). In-memory SQLite + an assembled Axum router,
 *   driven through `tower::ServiceExt::oneshot` — no real server.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test db_backup_integration
 * =============================================================================
 */

// Tests hold a process-wide ENV_LOCK guard across await points to serialize
// the ones that mutate DATA_DIRECTORY.
#![allow(clippy::await_holding_lock)]

//! Integration tests for database snapshots and the liveness/readiness probes.
//!
//! Why these exist: the database runs in WAL mode, so copying `homevault.db`
//! alone yields a file with **no tables**. `VACUUM INTO` is the only correct
//! snapshot, and it has to be reachable from the UI and taken automatically.
//! The probes exist because `GET /api/health` answers 200 even when the caller
//! is unauthenticated and even when the database is unusable — a monitor built
//! on it can never detect an outage.

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login, db_backup::*, health_probe::*,
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

    // A FILE-backed database, not `sqlite::memory:`. `VACUUM INTO` from an
    // in-memory source produces no file at all (it answers Ok and writes
    // nothing), so a memory database cannot exercise the snapshot path — and a
    // file is what the appliance actually runs on anyway.
    let db_path = tmp.path().join("homevault.db");
    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
    let db = Database::new(&db_url).await.expect("db");
    db.run_migrations().await.expect("migrations");
    db.record_schema_version().await.expect("schema version");

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
        .route(
            "/api/settings/backup/db",
            get(handle_db_backup_status).put(handle_db_backup_settings_update),
        )
        .route("/api/settings/backup/db/now", post(handle_db_backup_now))
        .route("/api/health/live", get(handle_health_live))
        .route("/api/health/ready", get(handle_health_ready))
        .with_state(state)
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
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, json)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
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

/// An admin session plus a viewer session on the same router.
async fn sessions(router: &mut Router, state: &AppState) -> (String, String) {
    create_user(state, "dbb_admin", "AdminPassw0rd1", 2).await;
    create_user(state, "dbb_viewer", "ViewerPassw0rd1", 1).await;
    let admin = login(router, "dbb_admin", "AdminPassw0rd1").await;
    let viewer = login(router, "dbb_viewer", "ViewerPassw0rd1").await;
    (admin, viewer)
}

// ========================================================================
// 1. GET /api/settings/backup/db
// ========================================================================

#[tokio::test]
async fn test_backup_status_requires_a_session() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(state);

    let (status, body) = send(&mut app, Method::GET, "/api/settings/backup/db", None, None).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST,
        "unauthenticated access must be refused, got {status} {body}"
    );
}

#[tokio::test]
async fn test_backup_status_requires_admin() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (_admin, viewer) = sessions(&mut app, &state).await;

    let (status, body) = send(
        &mut app,
        Method::GET,
        "/api/settings/backup/db",
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn test_backup_status_reports_defaults_and_an_empty_list() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (admin, _viewer) = sessions(&mut app, &state).await;

    let (status, body) = send(
        &mut app,
        Method::GET,
        "/api/settings/backup/db",
        Some(&admin),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["settings"]["interval_hours"], json!(24));
    assert_eq!(data["settings"]["retain"], json!(7));
    assert!(data["backups"].is_array());
    assert_eq!(data["backups"].as_array().unwrap().len(), 0);
    assert_eq!(data["total_size_bytes"], json!(0));
    assert!(data["directory"]
        .as_str()
        .unwrap_or("")
        .ends_with("backups"));
}

// ========================================================================
// 2. POST /api/settings/backup/db/now
// ========================================================================

#[tokio::test]
async fn test_backup_now_writes_a_restorable_snapshot() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (admin, _viewer) = sessions(&mut app, &state).await;

    let (status, body) = send(
        &mut app,
        Method::POST,
        "/api/settings/backup/db/now",
        Some(&admin),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let name = body["data"]["backup"]["name"]
        .as_str()
        .expect("snapshot name");
    assert!(name.starts_with("homevault-") && name.ends_with(".db"), "{name}");

    let path = body["data"]["backup"]["path"].as_str().expect("path");
    assert!(std::path::Path::new(path).exists(), "the file must be on disk");

    // The list now contains it, newest first.
    let (_, list) = send(
        &mut app,
        Method::GET,
        "/api/settings/backup/db",
        Some(&admin),
        None,
    )
    .await;
    let backups = list["data"]["backups"].as_array().expect("array");
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0]["name"], json!(name));
    assert!(list["data"]["total_size_bytes"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn test_backup_now_requires_admin() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (_admin, viewer) = sessions(&mut app, &state).await;

    let (status, body) = send(
        &mut app,
        Method::POST,
        "/api/settings/backup/db/now",
        Some(&viewer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

// ========================================================================
// 3. PUT /api/settings/backup/db
// ========================================================================

#[tokio::test]
async fn test_backup_settings_reject_an_out_of_range_retention() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (admin, _viewer) = sessions(&mut app, &state).await;

    for bad in [json!({"interval_hours": 24, "retain": 0}), json!({"interval_hours": 24, "retain": 101})] {
        let (status, body) = send(
            &mut app,
            Method::PUT,
            "/api/settings/backup/db",
            Some(&admin),
            Some(bad.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {body}");
    }
}

#[tokio::test]
async fn test_backup_settings_persist() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (admin, _viewer) = sessions(&mut app, &state).await;

    let (status, body) = send(
        &mut app,
        Method::PUT,
        "/api/settings/backup/db",
        Some(&admin),
        Some(json!({"interval_hours": 6, "retain": 3})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, after) = send(
        &mut app,
        Method::GET,
        "/api/settings/backup/db",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(after["data"]["settings"]["interval_hours"], json!(6));
    assert_eq!(after["data"]["settings"]["retain"], json!(3));
}

// ========================================================================
// 4. Retention
// ========================================================================

#[tokio::test]
async fn test_retention_prunes_the_oldest_snapshots() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(Arc::clone(&state));
    let (admin, _viewer) = sessions(&mut app, &state).await;

    // Keep only 2.
    let (status, _) = send(
        &mut app,
        Method::PUT,
        "/api/settings/backup/db",
        Some(&admin),
        Some(json!({"interval_hours": 24, "retain": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Snapshots are named to the second, so space them out to keep the names
    // distinct (and therefore the ordering meaningful).
    for _ in 0..4 {
        let (status, body) = send(
            &mut app,
            Method::POST,
            "/api/settings/backup/db/now",
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }

    let (_, list) = send(
        &mut app,
        Method::GET,
        "/api/settings/backup/db",
        Some(&admin),
        None,
    )
    .await;
    let backups = list["data"]["backups"].as_array().expect("array");
    assert_eq!(
        backups.len(),
        2,
        "retention 2 must keep exactly two snapshots, got {backups:?}"
    );
}

// ========================================================================
// 5. Public probes
// ========================================================================

#[tokio::test]
async fn test_liveness_needs_no_session_and_leaks_nothing() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(state);

    let (status, body) = send(&mut app, Method::GET, "/api/health/live", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], json!("ok"));

    // Nothing about the host may leak: this route is public.
    let text = body.to_string();
    for forbidden in ["hostname", "machine", "disk", "version", "cpu", "memory"] {
        assert!(
            !text.to_lowercase().contains(forbidden),
            "liveness must not expose {forbidden}: {text}"
        );
    }
}

#[tokio::test]
async fn test_readiness_reports_ready_on_a_healthy_database() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(state);
    record_integrity_result(&None);

    let (status, body) = send(&mut app, Method::GET, "/api/health/ready", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], json!("ready"));
    assert_eq!(body["checks"]["database"], json!("ok"));
}

#[tokio::test]
async fn test_readiness_reports_503_when_integrity_failed() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(state);
    record_integrity_result(&Some("database disk image is malformed".to_string()));

    let (status, body) = send(&mut app, Method::GET, "/api/health/ready", None, None).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a damaged database must not report ready: {body}"
    );
    assert_eq!(body["status"], json!("degraded"));
    assert!(body["checks"]["integrity"]
        .as_str()
        .unwrap_or("")
        .contains("malformed"));

    // Leave the global state clean for the other tests in this binary.
    record_integrity_result(&None);
}

#[tokio::test]
async fn test_readiness_is_reachable_without_a_session() {
    let (state, _tmp, _lock) = setup_state().await;
    let mut app = router(state);
    record_integrity_result(&None);

    // A monitor has no credentials — this must not 401.
    let (status, _) = send(&mut app, Method::GET, "/api/health/ready", None, None).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
        "readiness must answer without credentials, got {status}"
    );
}
