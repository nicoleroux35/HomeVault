/* =============================================================================
 * File          : notifications_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Notifications integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Notifications API — `crate::handlers::notifications`:
//!
//!   GET    /api/notifications              (paginated list, per-user read state)
//!   GET    /api/notifications/unread-count  (caller's badge count)
//!   POST   /api/notifications/read-all     (mark all read — caller only)
//!   POST   /api/notifications/{id}/read    (mark one read — caller only)
//!   DELETE /api/notifications/{id}         (delete one — **admin only**)
//!   DELETE /api/notifications              (clear all — **admin only**, ROADMAP 21.3)
//!   POST   /api/notifications/events/ticket (SSE ticket — any session)
//!   GET    /api/events                     (SSE stream — ticket only)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server. Nothing here
//! touches the host.
//!
//! Run:
//!   cargo test --test notifications_integration

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
use homevault::handlers::notifications::{
    handle_notification_delete, handle_notification_events_config_get,
    handle_notification_events_config_update, handle_notification_mark_read,
    handle_notifications_clear_all, handle_notifications_events, handle_notifications_events_ticket,
    handle_notifications_list, handle_notifications_mark_all_read, handle_notifications_unread_count,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
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

fn notifications_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/notifications",
            get(handle_notifications_list).delete(handle_notifications_clear_all),
        )
        .route(
            "/api/notifications/unread-count",
            get(handle_notifications_unread_count),
        )
        .route(
            "/api/notifications/read-all",
            post(handle_notifications_mark_all_read),
        )
        .route(
            "/api/notifications/{id}/read",
            post(handle_notification_mark_read),
        )
        .route(
            "/api/notifications/{id}",
            delete(handle_notification_delete),
        )
        .route(
            "/api/notifications/events/ticket",
            post(handle_notifications_events_ticket),
        )
        .route("/api/events", get(handle_notifications_events))
        .route(
            "/api/notifications/events/config",
            get(handle_notification_events_config_get),
        )
        .route(
            "/api/notifications/events/config",
            put(handle_notification_events_config_update),
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

/// Insert a notification row directly into the DB. `read_for` (an optional
/// username) inserts the caller's per-user read row (migration 058) — the
/// old global `read_at` column is gone.
async fn insert_notification(
    state: &AppState,
    title: &str,
    severity: &str,
    read_for: Option<&str>,
) -> String {
    insert_notification_at(state, title, severity, "datetime('now')", read_for).await
}

/// Insert a notification with an explicit `created_at` SQL expression so
/// ordering tests are deterministic (newest first).
async fn insert_notification_at(
    state: &AppState,
    title: &str,
    severity: &str,
    created_at: &str,
    read_for: Option<&str>,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO notifications (id, title, message, severity, category, source, created_at)
         VALUES (?, ?, 'msg', ?, 'cat', 'test', ?)",
    )
    .bind(&id)
    .bind(title)
    .bind(severity)
    .bind(created_at)
    .execute(&*state.db.pool())
    .await
    .expect("insert notification");
    if let Some(username) = read_for {
        sqlx::query(
            "INSERT INTO notification_reads (notification_id, username, read_at)
             VALUES (?, ?, '2026-01-01T00:00:00Z')",
        )
        .bind(&id)
        .bind(username)
        .execute(&*state.db.pool())
        .await
        .expect("insert read row");
    }
    id
}

// ---------------------------------------------------------------------------
// 1. Auth guards — all endpoints require a valid session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_notifications_list_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(&mut r, Method::GET, "/api/notifications", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/notifications",
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_unread_count_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_mark_all_read_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/read-all",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_mark_read_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/fake-id/read",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_delete_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/notifications/fake-id",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_clear_all_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(&mut r, Method::DELETE, "/api/notifications", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_events_ticket_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/events/ticket",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. List / unread-count with real data (per-user read state, ROADMAP 21.2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_returns_empty_when_no_notifications() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(
        body["data"]["notifications"].as_array().map(|a| a.len()),
        Some(0)
    );
    assert_eq!(body["data"]["total"], 0);
}

#[tokio::test]
async fn test_list_returns_notifications_and_unread_count_tracks_reads() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    // Insert 2 unread + 1 notification already read by "u".
    let id1 = insert_notification(&state, "unread1", "info", None).await;
    let _id2 = insert_notification(&state, "unread2", "warning", None).await;
    let _id3 = insert_notification(&state, "already-read", "info", Some("u")).await;

    // Unread count = 2.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 2);

    // List returns all 3, with per-user read_at resolved.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications",
        Some(&token),
        None,
    )
    .await;
    let items = body["data"]["notifications"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(body["data"]["total"], 3);

    // Mark one as read.
    let (status, _) = send(
        &mut r,
        Method::POST,
        &format!("/api/notifications/{}/read", id1),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Unread count drops to 1.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 1);

    // Mark all as read.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/read-all",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Unread count is now 0.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 0);
}

#[tokio::test]
async fn test_read_state_is_per_user() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let admin = login(&mut r, "admin", "AdminPass123!").await;
    let user = login(&mut r, "user", "UserPass123!").await;

    let id = insert_notification(&state, "shared", "warning", None).await;

    // Both accounts see it unread.
    for token in [&admin, &user] {
        let (_, body) = send(
            &mut r,
            Method::GET,
            "/api/notifications/unread-count",
            Some(token),
            None,
        )
        .await;
        assert_eq!(body["data"]["count"], 1, "unread for {token}");
    }

    // The admin reads it; only the admin's count changes.
    let (status, _) = send(
        &mut r,
        Method::POST,
        &format!("/api/notifications/{}/read", id),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 0, "admin read it");

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 1, "user still unread");

    // The user's list still resolves read_at = null; the admin's is set.
    let (_, admin_list) = send(
        &mut r,
        Method::GET,
        "/api/notifications",
        Some(&admin),
        None,
    )
    .await;
    assert!(admin_list["data"]["notifications"][0]["read_at"].is_string());
    let (_, user_list) = send(&mut r, Method::GET, "/api/notifications", Some(&user), None).await;
    assert!(user_list["data"]["notifications"][0]["read_at"].is_null());
}

#[tokio::test]
async fn test_mark_all_read_is_caller_scoped() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let admin = login(&mut r, "admin", "AdminPass123!").await;
    let user = login(&mut r, "user", "UserPass123!").await;

    let _id1 = insert_notification(&state, "a", "info", None).await;
    let _id2 = insert_notification(&state, "b", "info", None).await;

    // Admin marks all read — the user's badge must be untouched.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/read-all",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 0);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 2, "other account unaffected");

    // The user marks all read in turn.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/read-all",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 0);
}

#[tokio::test]
async fn test_mark_read_idempotent() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let id = insert_notification(&state, "mark-twice", "info", None).await;

    let url = format!("/api/notifications/{}/read", id);
    let (status, _) = send(&mut r, Method::POST, &url, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);

    // Second mark is still 200 (idempotent — already read by this caller).
    let (status, _) = send(&mut r, Method::POST, &url, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_mark_read_nonexistent_is_404() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/notifications/nonexistent-id/read",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// 3. Delete / clear-all — admin-only (ROADMAP 21.2/21.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_requires_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let admin = login(&mut r, "admin", "AdminPass123!").await;
    let user = login(&mut r, "user", "UserPass123!").await;

    let id = insert_notification(&state, "to-delete", "error", None).await;

    // A regular account cannot delete from the broadcast feed.
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/notifications/{}", id),
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin can.
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/notifications/{}", id),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Gone.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 0);
}

#[tokio::test]
async fn test_delete_nonexistent_is_404() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut r = notifications_router(state);
    let admin = login(&mut r, "admin", "AdminPass123!").await;

    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/notifications/nonexistent-id",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_clear_all_requires_admin_and_empties_feed() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let admin = login(&mut r, "admin", "AdminPass123!").await;
    let user = login(&mut r, "user", "UserPass123!").await;

    let _id1 = insert_notification(&state, "one", "info", None).await;
    let _id2 = insert_notification(&state, "two", "warning", None).await;
    let _id3 = insert_notification(&state, "three", "error", None).await;

    // A regular account cannot clear the feed.
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/notifications",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // One awaited admin call clears everything (ROADMAP 21.3).
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/notifications",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["deleted"], 3);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["notifications"].as_array().map(|a| a.len()), Some(0));
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/unread-count",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(body["data"]["count"], 0);
}

// ---------------------------------------------------------------------------
// 4. Pagination (ROADMAP 21.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_paginates_with_limit_offset_and_total() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    // Explicit, distinct timestamps so ordering is deterministic.
    let _newest = insert_notification_at(&state, "newest", "info", "'2026-01-03T00:00:00Z'", None).await;
    let _middle = insert_notification_at(&state, "middle", "info", "'2026-01-02T00:00:00Z'", None).await;
    let _oldest = insert_notification_at(&state, "oldest", "info", "'2026-01-01T00:00:00Z'", None).await;

    // First page: limit 2, newest first.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications?limit=2&offset=0",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["notifications"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["title"], "newest");
    assert_eq!(items[1]["title"], "middle");
    assert_eq!(body["data"]["total"], 3);
    assert_eq!(body["data"]["offset"], 0);
    // offset + page < total → a client can show "load older".
    assert_eq!(0 + 2 < 3, true);

    // Second page picks up the remainder.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications?limit=2&offset=2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["notifications"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "oldest");
}

#[tokio::test]
async fn test_list_clamps_oversized_limit() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications?limit=99999",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["limit"], 200, "limit clamped server-side");
}

// ---------------------------------------------------------------------------
// 5. SSE events ticket
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_events_ticket_returns_ticket_string() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/notifications/events/ticket",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert!(body["data"]["ticket"].is_string());
    assert!(!body["data"]["ticket"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_events_endpoint_rejects_missing_ticket() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(&mut r, Method::GET, "/api/events", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_events_endpoint_rejects_invalid_ticket() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = notifications_router(state);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/events?ticket=invalid",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Per-event notification config (2026-10-13)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_config_requires_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut r = notifications_router(state);
    let admin = login(&mut r, "admin", "AdminPass123!").await;
    let user = login(&mut r, "user", "UserPass123!").await;

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        None,
        None,
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "anonymous must not read the event config"
    );
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/notifications/events/config",
        Some(&user),
        Some(json!({"events": {}})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn event_config_lists_the_catalog_with_enabled_defaults() {
    let (state, _tmp, _guard) = setup_state().await;

    // NOTIFICATION_SETTINGS is a process-wide static — reset it so a
    // previous test's toggles don't leak into this one's expectations.
    *homevault::handlers::settings::NOTIFICATION_SETTINGS.write() =
        homevault::handlers::settings::NotificationSettings::default();

    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut r = notifications_router(state);
    let admin = login(&mut r, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = body["data"]["events"].as_array().expect("events array");
    assert!(!events.is_empty());
    // Default state: every catalog event enabled (absent toggle = enabled).
    assert!(events.iter().all(|e| e["enabled"] == json!(true)), "{body}");
    // The catalog includes the backup + app-update ids and the health checks.
    let ids: Vec<&str> = events.iter().filter_map(|e| e["id"].as_str()).collect();
    assert!(ids.contains(&"backup.jobs"));
    assert!(ids.contains(&"app.updates"));
    assert!(ids.contains(&"health.disk_smart"));
    // webhook_format defaults to null (generic).
    assert!(body["data"]["webhook_format"].is_null());
}

#[tokio::test]
async fn event_config_update_mutes_events_and_persists() {
    let (state, _tmp, _guard) = setup_state().await;

    // NOTIFICATION_SETTINGS is a process-wide static — reset it so a
    // previous test's toggles don't leak into this one's expectations.
    *homevault::handlers::settings::NOTIFICATION_SETTINGS.write() =
        homevault::handlers::settings::NotificationSettings::default();

    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut r = notifications_router(state);
    let admin = login(&mut r, "admin", "AdminPass123!").await;

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/notifications/events/config",
        Some(&admin),
        Some(json!({"events": {"backup.jobs": false, "health.cpu": true}})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown event id must be rejected"
    );

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/notifications/events/config",
        Some(&admin),
        Some(json!({"events": {"backup.jobs": false}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        Some(&admin),
        None,
    )
    .await;
    let events = body["data"]["events"].as_array().unwrap();
    let backup = events.iter().find(|e| e["id"] == "backup.jobs").unwrap();
    assert_eq!(backup["enabled"], json!(false), "muted event persists");
    let other = events.iter().find(|e| e["id"] == "app.updates").unwrap();
    assert_eq!(
        other["enabled"],
        json!(true),
        "untouched events stay enabled"
    );
}

#[tokio::test]
async fn webhook_format_updates_and_validates() {
    let (state, _tmp, _guard) = setup_state().await;

    // NOTIFICATION_SETTINGS is a process-wide static — reset it so a
    // previous test's toggles don't leak into this one's expectations.
    *homevault::handlers::settings::NOTIFICATION_SETTINGS.write() =
        homevault::handlers::settings::NotificationSettings::default();

    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut r = notifications_router(state);
    let admin = login(&mut r, "admin", "AdminPass123!").await;

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/notifications/events/config",
        Some(&admin),
        Some(json!({"webhook_format": "discord"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["webhook_format"], "discord");

    // Unknown format rejected, previous value untouched.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/notifications/events/config",
        Some(&admin),
        Some(json!({"webhook_format": "teams"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/notifications/events/config",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["webhook_format"], "discord");
}
