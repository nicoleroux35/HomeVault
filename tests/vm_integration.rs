// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Virtual Machines (Incus-backed) API.
//!
//!   GET    /api/vm/available
//!   GET    /api/vm/os-options
//!   GET    /api/vm/list
//!   POST   /api/vm/create
//!   POST   /api/vm/console/ticket
//!   POST   /api/vm/{name}/screen/ticket
//!   GET    /api/vm/{name}
//!   DELETE /api/vm/{name}
//!   PUT    /api/vm/{name}/start|stop|restart
//!   PUT    /api/vm/{name}/config
//!   GET|POST /api/vm/{name}/snapshots
//!   POST   /api/vm/{name}/snapshots/{snap}/restore
//!   DELETE /api/vm/{name}/snapshots/{snap}
//!
//! There is no Incus Unix socket in CI, so any handler that reaches an Incus
//! call answers `{success:false}` at HTTP 400 (`incus_err` → `json_error`).
//! The tests that exercise the Incus boundary detect the daemon's real state
//! via `IncusService::is_available()` and adapt (a live daemon is never asked
//! to create a real VM — that side effect is out of scope for integration
//! tests). We cover:
//!   1. Auth guards — missing (400) / bogus (401) / non-admin (403)
//!   2. Route existence — an admin request reaches the handler (never 404/405)
//!   3. Request validation that runs *before* Incus — bad names, missing
//!      password, unknown OS, unknown kind, out-of-range CPU/memory
//!   4. Endpoints that work without Incus — `available`, `os-options`
//!
//! Run: cargo test --test vm_integration

use axum::{
    routing::{delete, get, post, put},
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
    vm::{
        handle_console_ticket, handle_screen_ticket, handle_vm_available, handle_vm_config,
        handle_vm_create, handle_vm_delete, handle_vm_get, handle_vm_list, handle_vm_os_options,
        handle_vm_restart, handle_vm_snapshot_create, handle_vm_snapshot_delete,
        handle_vm_snapshot_restore, handle_vm_snapshots, handle_vm_start, handle_vm_stop,
    },
    AppState,
};
use homevault::incus::IncusService;
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

fn vm_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/vm/available", get(handle_vm_available))
        .route("/api/vm/os-options", get(handle_vm_os_options))
        .route("/api/vm/list", get(handle_vm_list))
        .route("/api/vm/create", post(handle_vm_create))
        .route("/api/vm/console/ticket", post(handle_console_ticket))
        .route("/api/vm/{name}/screen/ticket", post(handle_screen_ticket))
        .route("/api/vm/{name}", get(handle_vm_get))
        .route("/api/vm/{name}", delete(handle_vm_delete))
        .route("/api/vm/{name}/start", put(handle_vm_start))
        .route("/api/vm/{name}/stop", put(handle_vm_stop))
        .route("/api/vm/{name}/restart", put(handle_vm_restart))
        .route("/api/vm/{name}/config", put(handle_vm_config))
        .route("/api/vm/{name}/snapshots", get(handle_vm_snapshots))
        .route("/api/vm/{name}/snapshots", post(handle_vm_snapshot_create))
        .route(
            "/api/vm/{name}/snapshots/{snap}/restore",
            post(handle_vm_snapshot_restore),
        )
        .route(
            "/api/vm/{name}/snapshots/{snap}",
            delete(handle_vm_snapshot_delete),
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

async fn admin_token(state: &AppState, router: &mut Router) -> String {
    create_test_user(state, "admin", "Admin123!", 2).await;
    login(router, "admin", "Admin123!").await
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
        req.header("content-type", "application/json")
            .body(axum::body::Body::empty())
            .unwrap()
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

// The routes we exercise, as (method, path, optional body).
fn all_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/vm/available", None),
        (Method::GET, "/api/vm/os-options", None),
        (Method::GET, "/api/vm/list", None),
        (
            Method::POST,
            "/api/vm/create",
            Some(json!({"name":"web","os":"ubuntu-24.04","password":"secret12"})),
        ),
        (
            Method::POST,
            "/api/vm/console/ticket",
            Some(json!({"name":"web"})),
        ),
        (Method::POST, "/api/vm/web/screen/ticket", None),
        (Method::GET, "/api/vm/web", None),
        (Method::DELETE, "/api/vm/web", None),
        (Method::PUT, "/api/vm/web/start", None),
        (Method::PUT, "/api/vm/web/stop", None),
        (Method::PUT, "/api/vm/web/restart", None),
        (
            Method::PUT,
            "/api/vm/web/config",
            Some(json!({"cpu_cores":2})),
        ),
        (Method::GET, "/api/vm/web/snapshots", None),
        (
            Method::POST,
            "/api/vm/web/snapshots",
            Some(json!({"name":"snap1"})),
        ),
        (Method::POST, "/api/vm/web/snapshots/snap1/restore", None),
        (Method::DELETE, "/api/vm/web/snapshots/snap1", None),
    ]
}

// ── 1. Auth guards ─────────────────────────────────────────────────────────

#[tokio::test]
async fn every_endpoint_rejects_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state);
    for (m, path, body) in all_endpoints() {
        let (status, _) = send(&mut router, m.clone(), path, None, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{m} {path} missing token");
    }
}

#[tokio::test]
async fn every_endpoint_rejects_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state);
    for (m, path, body) in all_endpoints() {
        let (status, _) = send(&mut router, m.clone(), path, Some("nope"), body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{m} {path} bogus token");
    }
}

#[tokio::test]
async fn every_endpoint_rejects_non_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = vm_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for (m, path, body) in all_endpoints() {
        let (status, _) = send(&mut router, m.clone(), path, Some(&token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{m} {path} non-admin");
    }
}

// ── 2. Route existence (admin reaches the handler) ─────────────────────────

#[tokio::test]
async fn admin_reaches_every_handler_without_404() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    for (m, path, body) in all_endpoints() {
        let (status, _) = send(&mut router, m.clone(), path, Some(&token), body).await;
        assert_ne!(status, StatusCode::NOT_FOUND, "{m} {path}");
        assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{m} {path}");
    }
}

// ── 3. Endpoints that work without Incus ──────────────────────────────────

#[tokio::test]
async fn available_reports_the_real_incus_state() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/vm/available",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    // adapts to the host: Incus may or may not be reachable here
    assert_eq!(
        body["data"]["available"],
        json!(IncusService::is_available().await)
    );
}

#[tokio::test]
async fn os_options_returns_the_catalogue() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/vm/os-options",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let list = body["data"].as_array().expect("array");
    assert!(list.len() >= 4);
    assert!(list.iter().any(|o| o["id"] == "ubuntu-24.04"));
    assert_eq!(list.iter().filter(|o| o["recommended"] == true).count(), 1);
    // every entry carries a plain-language blurb
    assert!(list
        .iter()
        .all(|o| o["blurb"].as_str().map(|s| !s.is_empty()).unwrap_or(false)));
}

#[tokio::test]
async fn list_handles_incus_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let (status, body) = send(&mut router, Method::GET, "/api/vm/list", Some(&token), None).await;
    if IncusService::is_available().await {
        // real daemon → a real list
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
    } else {
        // Incus down → clean 400 with a friendly message
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"], false);
        let msg = body["message"].as_str().unwrap_or("");
        assert!(
            msg.to_lowercase().contains("incus"),
            "friendly message: {msg}"
        );
        // no stack trace / socket path leak
        assert!(!msg.contains("/var/lib/incus"));
    }
}

// ── 4. Create validation (runs before any Incus call) ─────────────────────

async fn create_err(router: &mut Router, token: &str, body: Value) -> String {
    let (status, resp) = send(
        router,
        Method::POST,
        "/api/vm/create",
        Some(token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(resp["success"], false);
    resp["message"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn create_rejects_bad_name() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    // "Web" is fine — the handler lower-cases names for the person.
    for bad in ["-web", "web-", "a--b", "my vm", "web!", &"x".repeat(80)] {
        let msg = create_err(
            &mut router,
            &token,
            json!({"name": bad, "os": "ubuntu-24.04", "password": "secret12"}),
        )
        .await;
        assert!(msg.to_lowercase().contains("name"), "for {bad:?}: {msg}");
    }
}

#[tokio::test]
async fn create_rejects_unknown_os() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "web", "os": "windows-95", "password": "secret12"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("operating system") || msg.to_lowercase().contains("list"));
}

#[tokio::test]
async fn create_rejects_unknown_kind() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "web", "kind": "wormhole", "os": "ubuntu-24.04", "password": "secret12"}),
    )
    .await;
    assert!(!msg.is_empty());
}

#[tokio::test]
async fn create_rejects_short_or_missing_password() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;

    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "web", "os": "ubuntu-24.04", "password": "abc"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("password"), "{msg}");

    // password field entirely absent (it is optional now, for ISO mode) →
    // the catalogue path still requires it → clean 400 body.
    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "web", "os": "ubuntu-24.04"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("password"), "{msg}");
}

#[tokio::test]
async fn create_rejects_bad_username() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "web", "os": "ubuntu-24.04", "username": "1abc", "password": "secret12"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("login name"), "{msg}");
}

#[tokio::test]
async fn create_rejects_alpine_as_vm() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    // alpine is container-only in the catalogue
    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "tiny", "kind": "vm", "os": "alpine-3.21", "password": "secret12"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("container"), "{msg}");
}

#[tokio::test]
async fn create_with_valid_input_then_hits_incus_unavailable() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    // Only meaningful on a host without Incus: with a live daemon this would
    // create a real VM (destructive side effect), which is out of scope here.
    if IncusService::is_available().await {
        return;
    }
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/vm/create",
        Some(&token),
        Some(json!({
            "name": "web", "kind": "vm", "os": "ubuntu-24.04",
            "size": "medium", "username": "admin", "password": "secret12"
        })),
    )
    .await;
    // Passed every validation gate, then Incus is missing.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}

// ── ISO install validation (before any Incus call) ───────────────────────

#[tokio::test]
async fn create_iso_rejects_relative_and_non_iso_paths() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;

    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "winbox", "iso_path": "somewhere/win.iso", "iso_os": "windows"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("absolute"), "{msg}");

    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "winbox", "iso_path": "/srv/media/win.img"}),
    )
    .await;
    assert!(msg.to_lowercase().contains(".iso"), "{msg}");

    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "winbox", "iso_path": "/srv/media/missing.iso"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("not found"), "{msg}");
}

#[tokio::test]
async fn create_rejects_both_os_and_iso() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let msg = create_err(
        &mut router,
        &token,
        json!({"name": "x", "os": "ubuntu-24.04", "password": "secret12", "iso_path": "/srv/x.iso"}),
    )
    .await;
    assert!(msg.to_lowercase().contains("not both"), "{msg}");
}

#[tokio::test]
async fn create_rejects_neither_os_nor_iso() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    let msg = create_err(&mut router, &token, json!({"name": "x"})).await;
    assert!(!msg.is_empty());
}

#[tokio::test]
async fn create_iso_with_real_file_reaches_incus() {
    let (state, tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    // Only meaningful on a host without Incus: with a live daemon this would
    // create a real VM (destructive side effect), which is out of scope here.
    if IncusService::is_available().await {
        return;
    }

    // a 12 MB .iso so it passes the size gate
    let iso = tmp.path().join("mini.iso");
    std::fs::write(&iso, vec![0u8; 12_000_000]).unwrap();

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/vm/create",
        Some(&token),
        Some(json!({
            "name": "linbox", "iso_path": iso.to_str().unwrap(), "iso_os": "linux", "size": "small"
        })),
    )
    .await;
    // Every validation gate passed (path, extension, size, free display),
    // then Incus is missing → clean failure.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn screen_ticket_needs_a_screen_and_a_running_machine() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    // Incus down → get_instance fails → friendly error, never a ticket
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/vm/win/screen/ticket",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
    assert!(body["data"]["ticket"].is_null());
}

// ── Config validation ────────────────────────────────────────────────────

#[tokio::test]
async fn config_rejects_out_of_range_and_empty() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;

    for body in [
        json!({}),
        json!({"cpu_cores": 0}),
        json!({"cpu_cores": 999}),
        json!({"memory_mb": 10}),
        json!({"memory_mb": 9_000_000}),
    ] {
        let (status, resp) = send(
            &mut router,
            Method::PUT,
            "/api/vm/web/config",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(resp["success"], false, "{body}");
    }
}

// ── Snapshot name validation ─────────────────────────────────────────────

#[tokio::test]
async fn snapshot_create_rejects_bad_names() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    for bad in [".hidden", "bad name", "bad/name", ""] {
        let (status, resp) = send(
            &mut router,
            Method::POST,
            "/api/vm/web/snapshots",
            Some(&token),
            Some(json!({"name": bad})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");
        assert_eq!(resp["success"], false, "{bad:?}");
    }
}

// ── Console ticket ───────────────────────────────────────────────────────

#[tokio::test]
async fn console_ticket_needs_a_running_machine() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = vm_router(state.clone());
    let token = admin_token(&state, &mut router).await;
    // Incus down → get_instance fails → friendly error, never a ticket
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/vm/console/ticket",
        Some(&token),
        Some(json!({"name": "web"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
    assert!(body["data"]["ticket"].is_null());
}

/// The OS catalogue exposes the desktop/Windows entries as ISO-only, plus
/// lightweight variants — and Windows ids are refused as one-click creates.
#[tokio::test]
async fn test_os_catalogue_lists_iso_only_windows_entries() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = vm_router(state.clone());
    let token = admin_token(&state, &mut r).await;

    let (status, body) = send(&mut r, Method::GET, "/api/vm/os-options", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let options = body["data"].as_array().expect("options array");
    let ids: Vec<&str> = options.iter().map(|o| o["id"].as_str().unwrap()).collect();
    for want in ["windows-11", "windows-10", "ubuntu-24.04-minimal", "debian-12-minimal"] {
        assert!(ids.contains(&want), "{want} missing from {ids:?}");
    }
    for o in options.iter().filter(|o| o["id"] == "windows-11") {
        assert_eq!(o["iso_only"], json!(true));
    }
}

/// A catalogue create with a Windows id fails fast with an ISO pointer —
/// before any Incus call — so no daemon is needed for this test.
#[tokio::test]
async fn test_create_with_windows_catalogue_id_requires_iso() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = vm_router(state.clone());
    let token = admin_token(&state, &mut r).await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/vm/create",
        Some(&token),
        Some(json!({
            "name": "wintest",
            "kind": "vm",
            "os": "windows-11",
            "size": "medium",
            "username": "admin",
            "password": "Secret123!",
        })),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(false));
    assert!(
        body["message"].as_str().unwrap_or("").contains("ISO"),
        "must point at the ISO install: {body}"
    );
}
