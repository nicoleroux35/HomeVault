// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Downloader (aria2) API — `crate::handlers::downloader`:
//!
//!   - GET  /api/downloader/status
//!   - POST /api/downloader/start
//!   - POST /api/downloader/stop
//!   - GET  /api/downloader/global-stat
//!   - GET  /api/downloader/tasks
//!   - POST /api/downloader/add
//!   - POST /api/downloader/pause
//!   - POST /api/downloader/unpause
//!   - POST /api/downloader/remove
//!   - GET  /api/downloader/task/{gid}
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! **This suite never spawns a real `aria2c` process and never issues an
//! aria2 JSON-RPC call.** Every endpoint except `status` calls
//! `ensure_aria2_running()`, which really launches `aria2c` on port 6800 and
//! would then talk to whatever daemon is listening there (on a dev box, the
//! one the live HomeVault server already started). Per CLAUDE.md's "What NOT
//! to test in Rust", that behaviour — a real download, pause/resume, remove
//! against a live daemon — is `tests/e2e/downloader/downloader.spec.ts`'s
//! job, exercised there against a genuine aria2c with a throwaway file
//! served by the app itself.
//!
//! What this suite locks down instead:
//!   1. the admin-only auth guard (`require_admin_request`) on **all 10**
//!      endpoints — the Downloader app's original audit (2026-07-16) found
//!      the feature had never worked and this handler family is the kind
//!      that elsewhere in the project shipped with *no* auth check at all;
//!   2. request-body validation (malformed / missing-field JSON → 422,
//!      before any handler logic or aria2 spawn);
//!   3. the one genuinely side-effect-free success path: `GET
//!      /api/downloader/status` with an admin session, which only runs
//!      `which aria2c` and reads the (fresh, empty) process handle.
//!
//! Note on status codes: these handlers always return HTTP 200 and signal
//! auth failure in the JSON body (`{"success":false,"error":...}`) — the
//! frontend checks `res.success`, not the HTTP status. The assertions below
//! match that contract rather than expecting 401/403.
//!
//! Run:
//!   cargo test --test downloader_integration
//!   cargo test --test downloader_integration -- --nocapture

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
    downloader::{
        handle_downloader_add, handle_downloader_global_stat, handle_downloader_pause,
        handle_downloader_remove, handle_downloader_start, handle_downloader_status,
        handle_downloader_stop, handle_downloader_task, handle_downloader_tasks,
        handle_downloader_unpause,
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

fn downloader_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/downloader/status", get(handle_downloader_status))
        .route("/api/downloader/start", post(handle_downloader_start))
        .route("/api/downloader/stop", post(handle_downloader_stop))
        .route(
            "/api/downloader/global-stat",
            get(handle_downloader_global_stat),
        )
        .route("/api/downloader/tasks", get(handle_downloader_tasks))
        .route("/api/downloader/add", post(handle_downloader_add))
        .route("/api/downloader/pause", post(handle_downloader_pause))
        .route("/api/downloader/unpause", post(handle_downloader_unpause))
        .route("/api/downloader/remove", post(handle_downloader_remove))
        .route("/api/downloader/task/{gid}", get(handle_downloader_task))
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

/// Every endpoint, paired with a body that is *valid* for its extractor so the
/// request reaches `require_admin_request` rather than failing JSON parsing
/// first. `task/{gid}` has no body.
fn all_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/downloader/status", None),
        (Method::POST, "/api/downloader/start", None),
        (Method::POST, "/api/downloader/stop", None),
        (Method::GET, "/api/downloader/global-stat", None),
        (Method::GET, "/api/downloader/tasks", None),
        (
            Method::POST,
            "/api/downloader/add",
            Some(json!({"uri": "http://127.0.0.1:1/x"})),
        ),
        (
            Method::POST,
            "/api/downloader/pause",
            Some(json!({"gid": "0000000000000000"})),
        ),
        (
            Method::POST,
            "/api/downloader/unpause",
            Some(json!({"gid": "0000000000000000"})),
        ),
        (
            Method::POST,
            "/api/downloader/remove",
            Some(json!({"gid": "0000000000000000"})),
        ),
        (Method::GET, "/api/downloader/task/0000000000000000", None),
    ]
}

fn assert_denied(body: &Value, expect_fragment: &str) {
    assert_eq!(
        body["success"], false,
        "expected an auth-denied envelope, got: {body}"
    );
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains(expect_fragment),
        "error {err:?} should contain {expect_fragment:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. Auth guard — no session token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = downloader_router(state);

    for (method, uri, body) in all_endpoints() {
        let (status, json) = send(&mut router, method.clone(), uri, None, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{method} {uri} should answer 200 with a JSON error body"
        );
        assert_denied(&json, "No session token provided");
    }
}

// ---------------------------------------------------------------------------
// 2. Auth guard — invalid / expired session token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_endpoints_reject_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = downloader_router(state);

    for (method, uri, body) in all_endpoints() {
        let (_, json) = send(
            &mut router,
            method.clone(),
            uri,
            Some("totally-bogus-token"),
            body,
        )
        .await;
        assert_denied(&json, "Invalid or expired session");
    }
}

// ---------------------------------------------------------------------------
// 3. Auth guard — a valid *non-admin* session is not enough
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_endpoints_reject_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regularuser", "Password123!", 1).await;
    let mut router = downloader_router(state);
    let token = login(&mut router, "regularuser", "Password123!").await;

    for (method, uri, body) in all_endpoints() {
        let (_, json) = send(&mut router, method.clone(), uri, Some(&token), body).await;
        assert_denied(&json, "Admin privileges required");
    }
}

// ---------------------------------------------------------------------------
// 4. Request-body validation — malformed JSON never reaches aria2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_add_rejects_missing_uri_field_with_422() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = downloader_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // `uri` is required on `AddUriRequest`; the `Json` extractor fails before
    // the handler body (and before `ensure_aria2_running`) — 422, not 200.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/downloader/add",
        Some(&token),
        Some(json!({"dir": "/tmp/x"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_add_rejects_non_json_body_with_400_or_415() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = downloader_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/downloader/add")
        .header("x-session-token", &token)
        .header("content-type", "application/json")
        .body(axum::body::Body::from("not json at all"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "a non-JSON body must be rejected as a client error, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_pause_rejects_missing_gid_field_with_422() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = downloader_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for uri in [
        "/api/downloader/pause",
        "/api/downloader/unpause",
        "/api/downloader/remove",
    ] {
        let (status, _) = send(
            &mut router,
            Method::POST,
            uri,
            Some(&token),
            Some(json!({"not_gid": "x"})),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{uri} with no `gid` must be 422"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. The one safe success path — GET /status with an admin session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_status_success_path_for_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = downloader_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/downloader/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert!(
        body["data"]["available"].is_boolean(),
        "`available` must be a bool, got {}",
        body["data"]["available"]
    );
    // This isolated AppState started aria2 nowhere, so `running` is deterministically false.
    assert_eq!(body["data"]["running"], false);

    // And the guarantee this whole file rests on: reading status spawned nothing.
    assert!(
        state.downloader.lock().await.child.is_none(),
        "GET /status must never start an aria2c process"
    );
}

#[tokio::test]
async fn test_status_reports_aria2_available_when_binary_is_installed() {
    // CLAUDE.md's installer ships `aria2` as an optional package and the
    // Downloader app is non-functional without it, so on any host where the
    // suite is expected to pass, `which aria2c` resolves. Assert the real
    // wiring (`find_aria2()` → `available`) rather than just the type.
    let aria2c_on_path = std::process::Command::new("which")
        .arg("aria2c")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !aria2c_on_path {
        eprintln!("skipping: aria2c not installed on this host");
        return;
    }

    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = downloader_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/downloader/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["available"], true);
}
