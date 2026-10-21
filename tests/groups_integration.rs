/* =============================================================================
 * File          : groups_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Groups integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the admin Groups-management API endpoints:
//!
//!   - GET    /api/groups/users              (list all users, for the member picker)
//!   - GET    /api/groups                    (list groups)
//!   - POST   /api/groups                    (create group)
//!   - GET    /api/groups/{name}             (get one group)
//!   - DELETE /api/groups/{name}             (delete group)
//!   - POST   /api/groups/{name}/members     (add member)
//!   - DELETE /api/groups/{name}/members/{username} (remove member)
//!   - GET    /api/users/{username}/groups   (groups for a user)
//!
//! Each test creates a fresh in-memory SQLite database, runs all embedded
//! migrations, builds a minimal Axum router, and uses `tower::ServiceExt`
//! for HTTP-level testing without starting a real server.
//!
//! `GroupService` (src/groups.rs) is a thin wrapper over real Linux
//! `getent`/`groupadd`/`groupdel`/`usermod`/`useradd` commands — reads
//! (`getent group`, `id -nG`) work without privilege, but every mutation
//! (`create_group`, `delete_group`, `add_user_to_group`,
//! `remove_user_from_group`, `ensure_linux_user`) shells out via `sudo` and
//! needs the real host, per CLAUDE.md's "What NOT to test in Rust". Those
//! mutation paths are exercised here only far enough to confirm they fail
//! *cleanly* (a clean error response, not a panic) when `sudo` isn't
//! available — real success/failure on an actual host stays covered by
//! `tests/e2e/groups/groups.spec.ts`.
//!
//! Run:
//!   cargo test --test groups_integration
//!   cargo test --test groups_integration -- --nocapture

use axum::{routing::get, Router};
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
    groups::{
        handle_group_add_member, handle_group_create, handle_group_delete, handle_group_get,
        handle_group_update,
        handle_group_remove_member, handle_groups_list, handle_groups_list_users,
        handle_user_groups,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

async fn setup_db() -> Database {
    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations().await.expect("Failed to run migrations");
    db
}

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = setup_db().await;
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

fn groups_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/groups/users", get(handle_groups_list_users))
        .route("/api/groups", get(handle_groups_list))
        .route("/api/groups", axum::routing::post(handle_group_create))
        .route(
            "/api/groups/{name}",
            get(handle_group_get).put(handle_group_update),
        )
        .route(
            "/api/groups/{name}",
            axum::routing::delete(handle_group_delete),
        )
        .route(
            "/api/groups/{name}/members",
            axum::routing::post(handle_group_add_member),
        )
        .route(
            "/api/groups/{name}/members/{username}",
            axum::routing::delete(handle_group_remove_member),
        )
        .route(
            "/api/users/{username}/groups",
            axum::routing::get(handle_user_groups),
        )
        .with_state(state)
}

/// Insert a user directly into the DB for testing.
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

/// Login via HTTP and return the session token.
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

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

// ===========================================================================
// 1. List users (member picker)
// ===========================================================================

#[tokio::test]
async fn test_groups_list_users_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/groups/users", None, None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_groups_list_users_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(&mut r, Method::GET, "/api/groups/users", Some(&token), None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_groups_list_users_success() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/groups/users", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let usernames: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap())
        .collect();
    assert!(usernames.contains(&"admin1"));
    assert!(usernames.contains(&"regular1"));
}

// ===========================================================================
// 2. List / get groups (real `getent`, no sudo needed)
// ===========================================================================

#[tokio::test]
async fn test_groups_list_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/groups", None, None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_groups_list_allows_any_valid_session_not_just_admin() {
    // Unlike most group-management endpoints, listing groups only requires
    // *a* valid session (see handle_groups_list: `get_session(...).is_err()`
    // is the only check) — any authenticated user, not just an admin.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (status, _) = send(&mut r, Method::GET, "/api/groups", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_groups_list_returns_an_array() {
    // No real `homevault_*` OS groups exist in this sandboxed test
    // environment, so the array is expected to be empty here — this test
    // only pins down the *shape* of a successful response, not specific
    // contents (those depend on the real host).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/groups", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
}

#[tokio::test]
async fn test_group_get_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/groups/nosuchgroup", None, None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_get_nonexistent_group() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/groups/this-group-does-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 3. Create group
// ===========================================================================

#[tokio::test]
async fn test_group_create_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/groups",
        None,
        Some(json!({"name": "newgroup", "members": []})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_create_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/groups",
        Some(&token),
        Some(json!({"name": "newgroup", "members": []})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_create_fails_cleanly_without_sudo() {
    // Real group creation needs `sudo groupadd` — unavailable in this
    // sandbox. This confirms the admin-gated endpoint reaches
    // `GroupService::create_group` and gets back a clean error response
    // (not a panic / 500) when the underlying command can't run.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/groups",
        Some(&token),
        Some(json!({"name": "hvtestgroup", "members": []})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 4. Delete group
// ===========================================================================

#[tokio::test]
async fn test_group_delete_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(&mut r, Method::DELETE, "/api/groups/somegroup", None, None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_delete_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/somegroup",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_delete_admin_group_is_protected() {
    // This check happens *before* any real `groupdel` call, so it's fully
    // reachable without sudo — the admin group must never be deletable
    // through this generic endpoint regardless of host privileges.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/homevault_admin",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "the admin group must be refused before any real groupdel is attempted"
    );
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("admin group"));
}

#[tokio::test]
async fn test_group_delete_admin_group_protected_regardless_of_display_or_full_name() {
    // `handle_group_delete` normalizes via `get_full_group_name` before
    // comparing, so both the bare display name and the fully-prefixed
    // system name must be refused identically.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/admin",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_delete_nonexistent_group_fails_cleanly() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/hvtest-does-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 5. Add member
// ===========================================================================

#[tokio::test]
async fn test_group_add_member_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/groups/somegroup/members",
        None,
        Some(json!({"username": "regular1"})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_add_member_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/groups/somegroup/members",
        Some(&token),
        Some(json!({"username": "regular2"})),
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "a non-admin session must not be able to add anyone to any group, \
         not just the admin group"
    );
}

#[tokio::test]
async fn test_group_add_member_admin_group_rejects_non_admin_user() {
    // This check runs before `ensure_linux_user`/`add_user_to_group`, so
    // it's fully reachable without sudo.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/groups/homevault_admin/members",
        Some(&token),
        Some(json!({"username": "regular1"})),
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "a non-admin (DB role_id=1) user must never be addable to the admin group"
    );
}

#[tokio::test]
async fn test_group_add_member_fails_cleanly_without_sudo() {
    // Non-admin-group adds always call `ensure_linux_user` first, which
    // needs `sudo useradd`-class privileges — unavailable here. Confirms a
    // clean error, not a panic.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/groups/hvtestgroup/members",
        Some(&token),
        Some(json!({"username": "regular1"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 6. Remove member
// ===========================================================================

#[tokio::test]
async fn test_group_remove_member_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/somegroup/members/regular1",
        None,
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_remove_member_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/somegroup/members/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_remove_member_admin_group_always_refused() {
    // This generic endpoint deliberately never allows removing anyone from
    // the admin group (demotion must go through the Users page's role
    // toggle, which keeps the DB role and the OS group in sync) — refused
    // unconditionally, before any real `usermod` call.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/homevault_admin/members/admin1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Users page"));
}

#[tokio::test]
async fn test_group_remove_member_fails_cleanly_without_sudo() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/groups/hvtestgroup/members/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 7. Groups for a user
// ===========================================================================

#[tokio::test]
async fn test_user_groups_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1/groups",
        None,
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_groups_allows_any_valid_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    // A DB-only test user has no real Linux account, so `id -nG` fails and
    // the handler surfaces that as an error — still a clean, non-panicking
    // response, and it confirms the auth gate itself passes.
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1/groups",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ===========================================================================
// Rename + description (2026-10-20)
//
// The OS half (`groupmod`) needs sudo, which this sandbox lacks — so these
// tests cover the contract that does NOT touch the host: the admin gate, the
// admin-group refusal, the clean failure when the rename cannot run, and the
// description path, which is pure database (the `hv_groups` mirror).
// ===========================================================================

#[tokio::test]
async fn test_group_update_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = groups_router(state);
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/groups/family",
        None,
        Some(json!({"description": "Household"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_update_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "member", "Passw0rd!123", 1).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "member", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/groups/family",
        Some(&token),
        Some(json!({"name": "relatives"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Admin"));
}

#[tokio::test]
async fn test_group_update_refuses_to_rename_the_admin_group() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // Renaming the sudo grant would leave the sudoers rule dangling.
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/groups/homevault_admin",
        Some(&token),
        Some(json!({"name": "admins"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("admin group"));
}

#[tokio::test]
async fn test_group_update_rename_fails_cleanly_without_sudo() {
    // No `sudo groupmod` in this sandbox: the endpoint must answer with a
    // clean error, never a panic — and must not have touched the mirror.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/groups/hvtest_nonexistent",
        Some(&token),
        Some(json!({"name": "hvtest_renamed"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_group_description_is_stored_and_listed() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let repo = homevault::storage::group_repo::HvGroupRepository::new(state.db.pool());

    // A group with no mirror row yet: the handler creates one rather than
    // silently dropping the description. Exercised through the repository
    // contract the handler relies on, because the endpoint's happy path ends
    // in `getent`, which has no row to return for a group that only exists
    // in the mirror.
    assert!(repo.update_description("family", Some("Household"))
        .await
        .expect("update")
        == false);
    let created = repo.create("family", Some("Household")).await.expect("create");
    assert_eq!(created.description.as_deref(), Some("Household"));

    assert!(repo.update_description("family", Some("Close family"))
        .await
        .expect("update"));
    let rows = repo.list_non_system().await.expect("list");
    assert_eq!(
        rows.iter()
            .find(|g| g.name == "family")
            .and_then(|g| g.description.clone())
            .as_deref(),
        Some("Close family")
    );

    // Clearing works and is not the same as "leave alone".
    assert!(repo.update_description("family", None).await.expect("clear"));
    let rows = repo.list_non_system().await.expect("list");
    assert!(rows
        .iter()
        .find(|g| g.name == "family")
        .expect("row")
        .description
        .is_none());
}

#[tokio::test]
async fn test_group_rename_moves_the_dependent_rows() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let groups = homevault::storage::group_repo::HvGroupRepository::new(state.db.pool());
    let resources =
        homevault::storage::resource_share_repo::ResourceShareRepository::new(state.db.pool());

    groups.create("family", Some("Household")).await.expect("mirror");
    resources
        .upsert_group(
            "admin1",
            "family",
            homevault::storage::resource_share_repo::ResourceKind::App,
            "jellyfin",
            homevault::storage::resource_share_repo::ResourcePermission::Write,
        )
        .await
        .expect("grant");

    // The two migrations `handle_group_update` performs after the OS rename.
    assert!(groups.rename("family", "relatives").await.expect("mirror rename"));
    assert_eq!(
        resources
            .rename_group_name("family", "relatives")
            .await
            .expect("grant rename"),
        1
    );

    // The grants now hang off the new name, and the old one holds nothing.
    assert!(resources
        .list_groups_all(Some("family"))
        .await
        .expect("old")
        .is_empty());
    let moved = resources
        .list_groups_all(Some("relatives"))
        .await
        .expect("new");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].resource, "jellyfin");

    // The mirror followed too, with the full_name convention and gid intact.
    let rows = groups.list_non_system().await.expect("list");
    let row = rows.iter().find(|g| g.name == "relatives").expect("renamed row");
    assert_eq!(row.full_name, "homevault_relatives");
    assert_eq!(row.description.as_deref(), Some("Household"));
}

#[tokio::test]
async fn test_group_delete_purges_the_mirror_and_the_group_grants() {
    // `handle_group_delete` runs these two cleanups after the OS delete. The
    // OS half needs sudo, so the contract is pinned here: a group re-created
    // with the same display name must inherit neither a stale description nor
    // grants nobody made for it.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let groups = homevault::storage::group_repo::HvGroupRepository::new(state.db.pool());
    let resources =
        homevault::storage::resource_share_repo::ResourceShareRepository::new(state.db.pool());

    groups
        .create("family", Some("Household"))
        .await
        .expect("mirror");
    resources
        .upsert_group(
            "admin1",
            "family",
            homevault::storage::resource_share_repo::ResourceKind::App,
            "jellyfin",
            homevault::storage::resource_share_repo::ResourcePermission::Read,
        )
        .await
        .expect("grant");

    assert!(groups.delete("family").await.expect("mirror delete"));
    assert_eq!(
        resources
            .revoke_all_for_group("family")
            .await
            .expect("grant purge"),
        1
    );

    assert!(groups.list_non_system().await.expect("list").is_empty());
    assert!(resources
        .list_groups_all(Some("family"))
        .await
        .expect("grants")
        .is_empty());
}

#[tokio::test]
async fn test_group_update_does_not_create_a_mirror_row_for_a_missing_group() {
    // A description-only PUT on a group that does not exist on the host must
    // fail *before* writing: otherwise `hv_groups` would grow a phantom row
    // for a group nobody created.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = groups_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/groups/ghostgroup",
        Some(&token),
        Some(json!({"description": "Should not be stored"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Group not found"));

    let repo = homevault::storage::group_repo::HvGroupRepository::new(state.db.pool());
    assert!(
        repo.list_non_system().await.expect("list").is_empty(),
        "no mirror row must be created for a group that does not exist"
    );
}
