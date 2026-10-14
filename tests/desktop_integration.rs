/* =============================================================================
 * File          : desktop_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop shell backend tests: wallpaper, monitor layout, weather location and per-account preference endpoints.
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
//! Integration tests for the Desktop shell's own backend surface.
//!
//! The desktop shell (`frontend/src/routes/Desktop.svelte`) is almost
//! entirely frontend; the only endpoints it owns are the power menu's:
//!
//!   POST /api/server/restart   (admin — reboots the host)
//!   POST /api/server/shutdown  (admin — powers the host off)
//!
//! **Both actually run `shutdown -r now` / `shutdown -h now` from a spawned
//! task the moment an admin token passes the check**, so this suite only
//! ever drives the *rejection* paths — no token, a bogus token, a valid
//! non-admin session. A valid admin token is never sent. The confirm-then-
//! reboot flow is a `tests/e2e/desktop/desktop.spec.ts` concern and even
//! there it stops at the confirmation modal.
//!
//! Everything else the shell renders (dock, topbar, launcher, windows,
//! global search) is covered by the E2E suite; the monitor panels are the
//! Widgets suite's job.
//!
//! Run:
//!   cargo test --test desktop_integration

use axum::{routing::post, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::system::{handle_server_restart, handle_server_shutdown};
use homevault::handlers::terminal_pty::PtyManager;
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

fn desktop_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/server/restart", post(handle_server_restart))
        .route("/api/server/shutdown", post(handle_server_shutdown))
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

const POWER_ENDPOINTS: [&str; 2] = ["/api/server/restart", "/api/server/shutdown"];

#[tokio::test]
async fn test_power_endpoints_reject_a_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = desktop_router(state);
    for uri in POWER_ENDPOINTS {
        let (status, _) = send(&mut r, Method::POST, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} (no token)");
    }
}

#[tokio::test]
async fn test_power_endpoints_reject_a_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = desktop_router(state);
    for uri in POWER_ENDPOINTS {
        let (status, _) = send(&mut r, Method::POST, uri, Some("not-a-real-token"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} (bad token)");
    }
}

#[tokio::test]
async fn test_power_endpoints_reject_a_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut r = desktop_router(state);
    let token = login(&mut r, "regular", "Password123!").await;
    for uri in POWER_ENDPOINTS {
        let (status, body) = send(&mut r, Method::POST, uri, Some(&token), None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} must be admin-only: {body}"
        );
    }
    // Never sent with an admin token — that would spawn `shutdown` for real.
}
