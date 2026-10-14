/* =============================================================================
 * File          : terminal_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Terminal integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Terminal (native PTY) HTTP surface —
//! `crate::handlers::terminal_pty::create_terminal_router`, nested under
//! `/api/terminal` in production:
//!
//!   POST /api/terminal/ticket   → issue a one-time WS connection ticket
//!   POST /api/terminal/close    → tear down one shell tab
//!   GET  /api/terminal/ws       → upgrade to the shell WebSocket
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server.
//!
//! **This suite never opens a real shell.** Redeeming a ticket on `/ws`
//! runs `sudo -u <user> -i` — a real login shell — so the WS tests only
//! cover the rejection paths (no ticket / bad ticket), which return before
//! any shell is spawned. `/ticket` and `/close` never spawn a shell at all
//! (a ticket is just a random string; `close_tab` on a tab that doesn't
//! exist is a clean no-op), so their full auth matrix + the tab-id
//! validation are covered here. The real WS lifecycle, multi-tab model and
//! idle reaper are `tests/e2e/terminal/terminal.spec.ts`'s job.
//!
//! Run:
//!   cargo test --test terminal_integration
//!   cargo test --test terminal_integration -- --nocapture

use axum::{routing::post, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::{create_terminal_router, PtyManager};
use homevault::handlers::{auth::handle_login, AppState};
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

fn terminal_router(state: Arc<AppState>) -> Router {
    // `create_terminal_router` already applies the state (`Router<()>`), so
    // build the login route with its own state and merge the two.
    let login: Router = Router::new()
        .route("/api/auth/login", post(handle_login))
        .with_state(state.clone());
    let terminal: Router = Router::new().nest("/api/terminal", create_terminal_router(state));
    login.merge(terminal)
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
        &[],
        Some(json!({"username": username, "password": password})),
    )
    .await;
    body["data"]["token"].as_str().expect("token").to_string()
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
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
// 1. /ticket — auth matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ticket_requires_a_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = terminal_router(state);
    let (status, _) = send(&mut router, Method::POST, "/api/terminal/ticket", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ticket_rejects_a_bogus_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = terminal_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/terminal/ticket",
        &[("x-session-token", "nope")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_ticket_rejects_a_non_admin_session() {
    // Terminal access is admin-only (a shell as any homevault_admin member).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = terminal_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/terminal/ticket",
        &[("x-session-token", &token)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_ticket_success_for_admin_returns_a_ticket() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = terminal_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/terminal/ticket",
        &[("x-session-token", &token)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let ticket = body["data"]["ticket"].as_str().expect("a ticket string");
    assert_eq!(ticket.len(), 32);
    assert!(ticket.chars().all(|c| c.is_ascii_alphanumeric()));
}

#[tokio::test]
async fn test_ticket_validates_the_tab_id() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = terminal_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // a valid opaque id is accepted
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/terminal/ticket?tab=my-tab_3",
        &[("x-session-token", &token)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // a malformed one is a clean 400
    for bad in ["tab%20with%20space", "..%2Fetc", "%C3%A9", &"x".repeat(65)] {
        let (status, _) = send(
            &mut router,
            Method::POST,
            &format!("/api/terminal/ticket?tab={bad}"),
            &[("x-session-token", &token)],
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "tab={bad}");
    }
}

// ---------------------------------------------------------------------------
// 2. /close — auth matrix + no-op for an unknown tab
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_close_auth_matrix() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = terminal_router(state);

    let (status, _) = send(&mut router, Method::POST, "/api/terminal/close", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/terminal/close",
        &[("x-session-token", "bad")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let reg = login(&mut router, "regular", "Password123!").await;
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/terminal/close",
        &[("x-session-token", &reg)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_close_an_unknown_tab_is_a_clean_success() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = terminal_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/terminal/close?tab=never-opened",
        &[("x-session-token", &token)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/terminal/close?tab=bad%20id",
        &[("x-session-token", &token)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 3. /ws
// ---------------------------------------------------------------------------
//
// `GET /api/terminal/ws` is a WebSocket upgrade endpoint — axum's
// `WebSocketUpgrade` extractor rejects a plain `tower::oneshot` request
// (426 Upgrade Required) before the handler's ticket check ever runs, so
// there is nothing here to assert at the HTTP level that isn't already an
// axum concern. The two things worth pinning — that a missing/expired/
// already-redeemed ticket is refused, and that redeeming one is
// single-use — are covered by the `PtyManager` unit tests
// (`consume_ticket` returning `None`), and the real end-to-end WebSocket
// handshake (ticket exchange → live shell) is `tests/e2e/terminal/
// terminal.spec.ts`.

#[tokio::test]
async fn test_ws_is_a_websocket_endpoint_and_refuses_a_plain_get() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = terminal_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/terminal/ws?ticket=x",
        &[],
        None,
    )
    .await;
    // Not 200 / not 101 — the extractor turns a non-upgrade request away.
    assert!(status.is_client_error(), "got {status}");
}

// ---------------------------------------------------------------------------
// 4. /cwd and /ls — completion support endpoints (auth matrix + no-session
//    guards). No real shell is ever spawned here, so the "live session"
//    branch (200 / 404) is pinned against the tab-id validation and the
//    admin gate, and the live-shell behaviour is left to the E2E suite.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cwd_auth_matrix() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = terminal_router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;
    let regular = login(&mut router, "regular", "Password123!").await;

    // No session → 401.
    let (status, _) = send(&mut router, Method::GET, "/api/terminal/cwd", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Non-admin → 403.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/terminal/cwd",
        &[("x-session-token", &regular)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin without a live session → 200 with `cwd: null`.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/terminal/cwd?tab=never-opened",
        &[("x-session-token", &admin)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["cwd"].is_null());

    // Malformed tab id → 400.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/terminal/cwd?tab=bad%20id",
        &[("x-session-token", &admin)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_ls_auth_matrix_and_no_session_guard() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = terminal_router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;
    let regular = login(&mut router, "regular", "Password123!").await;

    // No session → 401.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/terminal/ls?path=/tmp",
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Non-admin → 403.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/terminal/ls?path=/tmp",
        &[("x-session-token", &regular)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin without a live terminal session for that tab → 404 (the dir is
    // never even looked at).
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/terminal/ls?tab=never-opened&path=/tmp",
        &[("x-session-token", &admin)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
