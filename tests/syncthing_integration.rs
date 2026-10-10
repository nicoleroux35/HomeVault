// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Syncthing API — `crate::handlers::syncthing`.
//!
//! 10 endpoints: status, config, devices, folders, connections, errors
//! (read) + restart, pause, resume, scan (mutating).
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server.
//!
//! **This suite never talks to a real Syncthing daemon for a mutation.**
//! The original audit's headline finding was that all 10 handlers had **no
//! auth check whatsoever** — `restart` (full daemon restart) and
//! pause/resume (disrupt any paired device) included, plus read access to
//! every device ID/IP and folder path. Every handler now requires an
//! **admin** session; this suite locks that in on all 10, plus the
//! malformed-body 422 on the three that take a JSON body. The admin
//! success paths (which issue a real HTTP call to `127.0.0.1:8384`, and
//! for `restart`/`pause`/`resume`/`scan` would actually mutate a running
//! daemon) are left to `tests/e2e/syncthing/syncthing.spec.ts`, and even
//! there stay read-only unless a disposable instance is wired up.
//!
//! Note on status codes: these handlers always return HTTP 200 and signal
//! failure in the body (`{"success":false,"error":…}`), like Downloader /
//! Cloud Sync. Assertions match that.
//!
//! Run:
//!   cargo test --test syncthing_integration
//!   cargo test --test syncthing_integration -- --nocapture

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
    syncthing::{
        handle_syncthing_config, handle_syncthing_connections, handle_syncthing_devices,
        handle_syncthing_errors, handle_syncthing_folders, handle_syncthing_pause_device,
        handle_syncthing_restart, handle_syncthing_resume_device, handle_syncthing_scan_folder,
        handle_syncthing_status,
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

fn syncthing_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/syncthing/status", get(handle_syncthing_status))
        .route("/api/syncthing/config", get(handle_syncthing_config))
        .route("/api/syncthing/devices", get(handle_syncthing_devices))
        .route("/api/syncthing/folders", get(handle_syncthing_folders))
        .route(
            "/api/syncthing/connections",
            get(handle_syncthing_connections),
        )
        .route("/api/syncthing/errors", get(handle_syncthing_errors))
        .route("/api/syncthing/restart", post(handle_syncthing_restart))
        .route("/api/syncthing/pause", post(handle_syncthing_pause_device))
        .route(
            "/api/syncthing/resume",
            post(handle_syncthing_resume_device),
        )
        .route("/api/syncthing/scan", post(handle_syncthing_scan_folder))
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
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn denied(body: &Value, fragment: &str) {
    assert_eq!(body["success"], false, "expected denied, got {body}");
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains(fragment),
        "error {err:?} should contain {fragment:?}"
    );
}

/// (method, uri, body) for every endpoint, with a body valid for its extractor.
fn all_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/syncthing/status", None),
        (Method::GET, "/api/syncthing/config", None),
        (Method::GET, "/api/syncthing/devices", None),
        (Method::GET, "/api/syncthing/folders", None),
        (Method::GET, "/api/syncthing/connections", None),
        (Method::GET, "/api/syncthing/errors", None),
        (Method::POST, "/api/syncthing/restart", None),
        (
            Method::POST,
            "/api/syncthing/pause",
            Some(json!({"device": "AAAA-BBBB"})),
        ),
        (
            Method::POST,
            "/api/syncthing/resume",
            Some(json!({"device": "AAAA-BBBB"})),
        ),
        (
            Method::POST,
            "/api/syncthing/scan",
            Some(json!({"folder": "default"})),
        ),
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth — all 10 endpoints are admin-only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_endpoints_reject_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = syncthing_router(state);
    for (m, u, b) in all_endpoints() {
        let (status, body) = send(&mut router, m.clone(), u, None, b).await;
        assert_eq!(status, StatusCode::OK, "{m} {u}");
        denied(&body, "No session token provided");
    }
}

#[tokio::test]
async fn test_all_endpoints_reject_bogus_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = syncthing_router(state);
    for (m, u, b) in all_endpoints() {
        let (_, body) = send(&mut router, m.clone(), u, Some("nope"), b).await;
        denied(&body, "Invalid or expired session");
    }
}

#[tokio::test]
async fn test_all_endpoints_reject_non_admin_session() {
    // The whole point of the 2026-07-16 audit fix: device topology + sync
    // control are admin-only. A regular user must not be able to enumerate
    // devices, let alone restart the daemon.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for (m, u, b) in all_endpoints() {
        let (_, body) = send(&mut router, m.clone(), u, Some(&token), b).await;
        denied(&body, "Admin privileges required");
    }
}

// ---------------------------------------------------------------------------
// 2. Body validation — malformed JSON never reaches the Syncthing API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pause_resume_scan_reject_missing_field_with_422() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for (uri, bad) in [
        ("/api/syncthing/pause", json!({"not_device": "x"})),
        ("/api/syncthing/resume", json!({})),
        ("/api/syncthing/scan", json!({"not_folder": "x"})),
    ] {
        let (status, _) = send(&mut router, Method::POST, uri, Some(&token), Some(bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// 3. status success shape (admin) — read-only, safe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_status_for_admin_returns_a_well_formed_envelope() {
    // `handle_syncthing_status` runs `syncthing --version` then best-effort
    // pings 127.0.0.1:8384. Whether or not a daemon is up, it must answer a
    // `{"success":true,"data":{...available:bool...}}` envelope — never an
    // error and never a panic.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/syncthing/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert!(
        body["data"]["available"].is_boolean(),
        "status must report an `available` bool, got {}",
        body["data"]
    );
}
