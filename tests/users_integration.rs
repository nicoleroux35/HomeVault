/* =============================================================================
 * File          : users_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Users integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the admin Users-management API endpoints:
//!
//!   - GET    /api/users                     (list)
//!   - GET    /api/users/{username}          (get one)
//!   - POST   /api/users                     (create)
//!   - PUT    /api/users/{username}           (self/admin profile update)
//!   - PUT    /api/users/{username}/full      (admin profile + home-folder update)
//!   - PUT    /api/users/{username}/groups    (group membership sync)
//!   - PUT    /api/users/{username}/password  (password change)
//!   - PUT    /api/users/{username}/role      (role change)
//!   - PUT    /api/users/{username}/locked    (lock/unlock)
//!   - POST   /api/users/{username}/2fa/disable (admin-forced 2FA disable)
//!   - DELETE /api/users/{username}           (delete)
//!   - GET    /api/users/count                (count)
//!   - GET    /api/users/{username}/exists    (existence check)
//!
//! Each test creates a fresh in-memory SQLite database, runs all embedded
//! migrations, builds a minimal Axum router, and uses `tower::ServiceExt`
//! for HTTP-level testing without starting a real server — no `sudo`, no
//! real Linux user/group operations (those stay covered by
//! `tests/e2e/users/users.spec.ts` against a live host, per CLAUDE.md's
//! "What NOT to test in Rust").
//!
//! Run:
//!   cargo test --test users_integration
//!   cargo test --test users_integration -- --nocapture

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
    users::{
        handle_admin_disable_2fa, handle_admin_revoke_user_sessions, handle_admin_user_sessions,
        handle_user_count, handle_user_create, handle_user_delete, handle_user_exists,
        handle_user_get, handle_user_set_expiry, handle_user_set_locked, handle_user_update,
        handle_user_update_full, handle_user_update_groups, handle_user_update_password,
        handle_user_update_role, handle_users_list,
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

fn users_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/users", get(handle_users_list))
        .route("/api/users", axum::routing::post(handle_user_create))
        .route("/api/users/count", get(handle_user_count))
        .route("/api/users/{username}", get(handle_user_get))
        .route(
            "/api/users/{username}",
            axum::routing::put(handle_user_update),
        )
        .route(
            "/api/users/{username}",
            axum::routing::delete(handle_user_delete),
        )
        .route(
            "/api/users/{username}/full",
            axum::routing::put(handle_user_update_full),
        )
        .route(
            "/api/users/{username}/groups",
            axum::routing::put(handle_user_update_groups),
        )
        .route(
            "/api/users/{username}/password",
            axum::routing::put(handle_user_update_password),
        )
        .route(
            "/api/users/{username}/sessions",
            axum::routing::get(handle_admin_user_sessions)
                .delete(handle_admin_revoke_user_sessions),
        )
        .route(
            "/api/users/{username}/expiry",
            axum::routing::put(handle_user_set_expiry),
        )
        .route(
            "/api/users/{username}/role",
            axum::routing::put(handle_user_update_role),
        )
        .route(
            "/api/users/{username}/locked",
            axum::routing::put(handle_user_set_locked),
        )
        .route(
            "/api/users/{username}/2fa/disable",
            axum::routing::post(handle_admin_disable_2fa),
        )
        .route("/api/users/{username}/exists", get(handle_user_exists))
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
// 1. List
// ===========================================================================

#[tokio::test]
async fn test_users_list_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = users_router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/users", None, None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_users_list_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(&mut r, Method::GET, "/api/users", Some(&token), None).await;
    assert_eq!(body["success"], json!(false), "non-admin must be rejected");
}

#[tokio::test]
async fn test_users_list_success_includes_all_users() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/users", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let usernames: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert!(usernames.contains(&"admin1"));
    assert!(usernames.contains(&"regular1"));
}

// ===========================================================================
// 2. Get one
// ===========================================================================

#[tokio::test]
async fn test_user_get_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = users_router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/users/nobody", None, None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_get_self_allowed_for_non_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["username"], json!("regular1"));
}

#[tokio::test]
async fn test_user_get_other_denied_for_non_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_get_admin_can_view_anyone() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["username"], json!("regular1"));
}

#[tokio::test]
async fn test_user_get_nonexistent_user() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(&mut r, Method::GET, "/api/users/ghost", Some(&token), None).await;
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 3. Create
// ===========================================================================

fn valid_create_payload(username: &str) -> Value {
    json!({
        "username": username,
        "password": "Passw0rd!123",
        "fullname": "New Person",
        "email": "",
        "role_id": 1,
        "groups": [],
        "home_enabled": false,
        "home_disk": null,
    })
}

#[tokio::test]
async fn test_user_create_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(valid_create_payload("newuser")),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_invalid_username() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("a");
    payload["username"] = json!("a"); // too short / reserved-ish
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_weak_password() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("weakpwuser");
    payload["password"] = json!("short");
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_invalid_email() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("bademailuser");
    payload["email"] = json!("not-an-email");
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_email_too_long() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("longemailuser");
    let long_local = "a".repeat(130);
    payload["email"] = json!(format!("{}@example.com", long_local));
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_invalid_role_id() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("badroleuser");
    payload["role_id"] = json!(99);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_nonexistent_group() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("groupuser");
    payload["groups"] = json!(["homevault_this_group_does_not_exist"]);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "a nonexistent group must be rejected"
    );
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("does not exist"));
}

#[tokio::test]
async fn test_user_create_success() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(valid_create_payload("brandnewuser")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    // Confirm it's really there.
    let (_, get_body) = send(
        &mut r,
        Method::GET,
        "/api/users/brandnewuser",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(get_body["data"]["username"], json!("brandnewuser"));
}

#[tokio::test]
async fn test_user_create_duplicate_username() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "existinguser", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(valid_create_payload("existinguser")),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 4. Update profile (self / admin)
// ===========================================================================

#[tokio::test]
async fn test_user_update_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = users_router(state);
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/nobody",
        None,
        Some(json!({"fullname": "X", "email": ""})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_self_success() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1",
        Some(&token),
        Some(json!({"fullname": "Updated Name", "email": "u@example.com"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_update_other_denied_for_non_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular2",
        Some(&token),
        Some(json!({"fullname": "Hacked", "email": ""})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_admin_can_update_anyone() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1",
        Some(&token),
        Some(json!({"fullname": "Admin Edited", "email": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ===========================================================================
// 5. Update full (admin: profile + home folder)
// ===========================================================================

#[tokio::test]
async fn test_user_update_full_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/full",
        Some(&token),
        Some(json!({"fullname": "X", "email": "", "home_enabled": false, "home_disk": null})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_full_success_home_disabled() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/full",
        Some(&token),
        Some(json!({"fullname": "Full Update", "email": "", "home_enabled": false, "home_disk": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_update_full_home_enabled_fails_without_real_system_user() {
    // No real Linux account backs a DB-only test user, so enabling a home
    // folder must fail cleanly with a real (not swallowed) error — matches
    // `apply_home_folder`'s own `id -u`/`id -g` check. Deliberately not
    // exercising the success path here (that needs a real host account,
    // covered live by users.spec.ts / the original manual audit).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/full",
        Some(&token),
        Some(json!({"fullname": "X", "email": "", "home_enabled": true, "home_disk": "/home"})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 5b. Home-directory size limit (disk quota)
// ===========================================================================

#[tokio::test]
async fn test_user_create_rejects_negative_home_size() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("sizeuser");
    payload["home_size_bytes"] = json!(-1);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
        .contains("negative"));
}

#[tokio::test]
async fn test_user_update_full_rejects_negative_home_size() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/full",
        Some(&token),
        Some(json!({
            "fullname": "X", "email": "",
            "home_enabled": false, "home_disk": null,
            "home_size_bytes": -5000
        })),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_create_ignores_home_size_when_home_disabled() {
    // A size is accepted on the wire but only takes effect when a home
    // folder is actually enabled — with it off, the user is created and
    // no home row (and no limit) is persisted.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let mut payload = valid_create_payload("nolimituser");
    payload["home_size_bytes"] = json!(5_368_709_120i64);
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    let repo = homevault::storage::user_home_repo::UserHomeRepository::new(state.db.pool());
    assert!(repo
        .find_by_username("nolimituser")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_user_full_rejects_system_directory_home_path() {
    // The folder picker lets an admin choose an explicit home directory
    // (`home_path`); the server must reject a path that sits under a system
    // directory (OWASP path-traversal / privilege-escalation hardening).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "victim1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/victim1/full",
        Some(&token),
        Some(json!({
            "fullname": "V", "email": "",
            "home_enabled": true, "home_disk": null,
            "home_path": "/etc/homevault-hijack"
        })),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase()
        .contains("system directory"));
}

#[tokio::test]
async fn test_user_full_rejects_relative_and_traversal_home_path() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "victim2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    for bad in ["home/relative", "/srv/../etc/x", "/tmp/x\ny"] {
        let (_, body) = send(
            &mut r,
            Method::PUT,
            "/api/users/victim2/full",
            Some(&token),
            Some(json!({
                "fullname": "V", "email": "",
                "home_enabled": true, "home_disk": null, "home_path": bad
            })),
        )
        .await;
        assert_eq!(
            body["success"],
            json!(false),
            "path {:?} should be rejected",
            bad
        );
    }
}

#[tokio::test]
async fn test_user_home_repo_round_trips_size_limit() {
    // Migration 032 added user_homes.size_limit_bytes — verify the repo
    // writes and reads it back (and that NULL means "no limit").
    let (state, _tmp, _g) = setup_state().await;
    let repo = homevault::storage::user_home_repo::UserHomeRepository::new(state.db.pool());

    let with_limit = homevault::storage::user_home_repo::UserHome {
        username: "limited".to_string(),
        enabled: true,
        disk: Some("/home".to_string()),
        path: Some("/home/limited".to_string()),
        created_at: chrono::Utc::now(),
        size_limit_bytes: Some(21_474_836_480),
    };
    repo.upsert(&with_limit).await.unwrap();
    let got = repo.find_by_username("limited").await.unwrap().unwrap();
    assert_eq!(got.size_limit_bytes, Some(21_474_836_480));

    // Upsert again with no limit — the column must clear, not stick.
    let cleared = homevault::storage::user_home_repo::UserHome {
        size_limit_bytes: None,
        ..got
    };
    repo.upsert(&cleared).await.unwrap();
    assert_eq!(
        repo.find_by_username("limited")
            .await
            .unwrap()
            .unwrap()
            .size_limit_bytes,
        None
    );
}

// ===========================================================================
// 6. Update groups
// ===========================================================================

#[tokio::test]
async fn test_user_update_groups_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/groups",
        Some(&token),
        Some(json!({"groups": []})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_groups_empty_is_a_safe_noop() {
    // A DB-only test user has no real Linux groups (`id -nG` fails), so the
    // handler's current-groups set is empty; submitting an empty target
    // list too means no add/remove is attempted and nothing shells out.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/groups",
        Some(&token),
        Some(json!({"groups": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

// ===========================================================================
// 7. Update password
// ===========================================================================

#[tokio::test]
async fn test_user_update_password_no_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = users_router(state);
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/nobody/password",
        None,
        Some(json!({"current_password": null, "new_password": "NewPassw0rd!123"})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_password_admin_reset_no_current_required() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/password",
        Some(&token),
        Some(json!({"current_password": null, "new_password": "BrandNewPassw0rd!123"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_update_password_self_requires_current() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/password",
        Some(&token),
        Some(json!({"current_password": null, "new_password": "NewPassw0rd!123"})),
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "self-service change without current_password must be rejected"
    );
}

#[tokio::test]
async fn test_user_update_password_self_wrong_current_rejected() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/password",
        Some(&token),
        Some(json!({"current_password": "WrongPassword!", "new_password": "NewPassw0rd!123"})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_password_self_correct_current_succeeds() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/password",
        Some(&token),
        Some(json!({"current_password": "Passw0rd!123", "new_password": "NewPassw0rd!123"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_update_password_other_denied_for_non_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular2/password",
        Some(&token),
        Some(json!({"current_password": null, "new_password": "NewPassw0rd!123"})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 8. Update role
// ===========================================================================

#[tokio::test]
async fn test_user_update_role_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular2/role",
        Some(&token),
        Some(json!({"role_id": 2})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_role_invalid_role_id() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/role",
        Some(&token),
        Some(json!({"role_id": 7})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_update_role_cannot_demote_self() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "admin2", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin1/role",
        Some(&token),
        Some(json!({"role_id": 1})),
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "an admin must not be able to demote themselves"
    );
}

#[tokio::test]
async fn test_user_update_role_promote_regular_user_to_admin_succeeds() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/role",
        Some(&token),
        Some(json!({"role_id": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_update_role_demote_admin_when_two_admins_exist_succeeds() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "admin2", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin2/role",
        Some(&token),
        Some(json!({"role_id": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_update_role_nonexistent_user() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "admin2", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/ghost/role",
        Some(&token),
        Some(json!({"role_id": 1})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

/// Regression test for a real bug found by static review and fixed
/// 2026-09-05: the "last admin" guard used to fire whenever `role_id !=
/// ADMIN` for a *different* user, without first checking whether that
/// target was even currently an admin. With exactly one admin in the
/// system, setting an already-*regular* user's role to "user" (a no-op)
/// was refused with "Cannot remove last admin" even though no admin was
/// being touched at all. Fixed by checking `target.is_admin()` before
/// running the admin-count check.
#[tokio::test]
async fn test_user_update_role_noop_on_regular_user_is_allowed_with_one_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await; // the only admin
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await; // already role_id=1
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/role",
        Some(&token),
        Some(json!({"role_id": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["success"],
        json!(true),
        "a no-op role-id=1 update on an already-regular user must not be \
         blocked by the last-admin guard just because the system has only \
         one admin — that admin is never the one being touched"
    );
}

// ===========================================================================
// 9. Set locked
// ===========================================================================

#[tokio::test]
async fn test_user_set_locked_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular2/locked",
        Some(&token),
        Some(json!({"locked": true})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_set_locked_cannot_lock_self() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin1/locked",
        Some(&token),
        Some(json!({"locked": true})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_set_locked_locking_the_other_of_two_admins_succeeds() {
    // `handle_user_set_locked`'s guard counts admins other than the
    // *target* who are active — which always includes the acting session's
    // own admin (session != target for a non-self lock), so with exactly
    // two admins this count is 1, not 0, and the lock is allowed. Same
    // structural shape as the delete guard's dead branch (see
    // `test_user_delete_admin_when_another_admin_remains_succeeds`'s doc
    // comment): the guard can only ever really fire together with the
    // separate self-lock check, since the acting admin is never the one
    // being counted against. Confirmed live by an initial version of this
    // test asserting the opposite (refusal), which failed.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "admin2", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin2/locked",
        Some(&token),
        Some(json!({"locked": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn test_user_set_locked_lock_then_unlock_regular_user() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/locked",
        Some(&token),
        Some(json!({"locked": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    let (_, get_body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(get_body["data"]["locked"], json!(true));

    let (status2, body2) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/locked",
        Some(&token),
        Some(json!({"locked": false})),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    assert_eq!(body2["success"], json!(true));
}

// ===========================================================================
// 10. Admin-forced 2FA disable
// ===========================================================================

#[tokio::test]
async fn test_admin_disable_2fa_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users/regular1/2fa/disable",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_admin_disable_2fa_success_even_when_not_enabled() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/users/regular1/2fa/disable",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], json!(false));
}

// ===========================================================================
// 11. Delete
// ===========================================================================

#[tokio::test]
async fn test_user_delete_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/regular2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_delete_cannot_delete_self() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/admin1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

/// `handle_user_delete`'s "cannot delete the last admin" guard only fires
/// when the *target* `is_admin()` and total `count_admins() <= 1`. Deleting
/// yourself is refused earlier by a separate self-delete check, so the only
/// way to reach the count check is a *different* admin session deleting an
/// admin target — but that necessarily means at least two admins exist
/// (the caller plus the target), so `count_admins()` is always >= 2 right
/// there. The branch is structurally unreachable as written (harmless —
/// it just never fires, it doesn't block anything it shouldn't). This test
/// instead confirms the reachable, correct behavior: deleting one of two
/// admins succeeds and leaves the other one in place.
#[tokio::test]
async fn test_user_delete_admin_when_another_admin_remains_succeeds() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "admin2", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/admin2",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "deleting one of two admins must succeed"
    );
    assert_eq!(body["success"], json!(true));

    let (_, get_body) = send(&mut r, Method::GET, "/api/users/admin1", Some(&token), None).await;
    assert_eq!(
        get_body["success"],
        json!(true),
        "the other admin must remain usable"
    );
}

#[tokio::test]
async fn test_user_delete_success() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    let (_, get_body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        get_body["success"],
        json!(false),
        "deleted user must be gone"
    );
}

#[tokio::test]
async fn test_user_delete_nonexistent_user() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/ghost",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

/// Insert a `share_permissions` row directly — the table keys a grantee by
/// name only (no foreign key to `users`), which is exactly why a deleted
/// account's rows used to outlive it.
async fn grant_test_permission(
    state: &AppState,
    share_name: &str,
    share_type: &str,
    grantee_type: &str,
    grantee_name: &str,
    permission: &str,
) {
    sqlx::query(
        "INSERT INTO share_permissions (share_name, share_type, grantee_type, grantee_name, permission, recursive, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 1, datetime('now'), datetime('now'))",
    )
    .bind(share_name)
    .bind(share_type)
    .bind(grantee_type)
    .bind(grantee_name)
    .bind(permission)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert test share permission");
}

/// Count the `share_permissions` rows for one grantee.
async fn count_test_permissions(state: &AppState, grantee_type: &str, grantee_name: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM share_permissions WHERE grantee_type = ? AND grantee_name = ?",
    )
    .bind(grantee_type)
    .bind(grantee_name)
    .fetch_one(&*state.db.pool())
    .await
    .expect("Failed to count test share permissions");
    row.0
}

/// Deleting an account must revoke the share permissions it holds *directly*
/// — both the `share_permissions` rows and (best-effort) the real POSIX ACLs —
/// so that recreating an account with the same username cannot silently
/// inherit the deleted user's access. Previously the handler only logged an
/// intent to revoke: nothing was deleted, on disk or in the database.
///
/// Group grants are deliberately kept: they belong to the group, and the other
/// members must not lose access because one account was deleted. The share
/// names used here do not exist on the host, so the ACL half fails cleanly and
/// best-effort — this test asserts the database half, which is the regression.
#[tokio::test]
async fn test_user_delete_revokes_its_direct_share_permissions_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;

    // Two direct grants for the deleted user — one on a well-known protocol
    // name, one on a type that does not parse — plus a same-named *group*
    // grant and another user's grant, neither of which may be touched.
    grant_test_permission(&state, "hv-test-gone", "smb", "user", "regular1", "rw").await;
    grant_test_permission(
        &state,
        "hv-test-gone",
        "bogusproto",
        "user",
        "regular1",
        "r",
    )
    .await;
    grant_test_permission(&state, "hv-test-keep", "smb", "user", "regular2", "r").await;
    grant_test_permission(&state, "hv-test-group", "smb", "group", "regular1", "rw").await;

    let mut r = users_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/regular1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    assert_eq!(
        count_test_permissions(&state, "user", "regular1").await,
        0,
        "every grant held directly by the deleted user must be gone"
    );
    assert_eq!(
        count_test_permissions(&state, "user", "regular2").await,
        1,
        "another user's grant must be untouched"
    );
    assert_eq!(
        count_test_permissions(&state, "group", "regular1").await,
        1,
        "a same-named group grant belongs to the group, not to the account"
    );
}

// ===========================================================================
// 12. Count / exists
// ===========================================================================

#[tokio::test]
async fn test_user_count_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(&mut r, Method::GET, "/api/users/count", Some(&token), None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_user_count_success() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "regular2", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/users/count", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], json!(3));
}

#[tokio::test]
async fn test_user_exists_true() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/exists",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["exists"], json!(true));
}

#[tokio::test]
async fn test_user_exists_false() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/nosuchuser/exists",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["exists"], json!(false));
}

// ===========================================================================
// Admin per-user session management (2026-10-20)
// ===========================================================================

/// Insert two live sessions for a user directly into the DB.
async fn seed_sessions(state: &AppState, username: &str, tokens: &[&str]) {
    for (i, token) in tokens.iter().enumerate() {
        sqlx::query(
            "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at)
             VALUES (?, ?, '10.0.0.9', ?, datetime('now'), datetime('now', '+1 day'))",
        )
        .bind(token)
        .bind(username)
        .bind(format!("csrf-{}-{}", username, i))
        .execute(&*state.db.pool())
        .await
        .expect("Failed to insert test session");
    }
}

#[tokio::test]
async fn test_admin_sessions_lists_masked_tokens_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    seed_sessions(
        &state,
        "regular1",
        &["tok-aaaaaaaa-live", "tok-bbbbbbbb-live"],
    )
    .await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1/sessions",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], json!(2));
    // MASKED: the full live token must not appear anywhere in the payload.
    assert!(
        !body["data"]["sessions"][0]["id"]
            .as_str()
            .unwrap()
            .contains("tok-aaaaaaaa"),
        "the full token must never be handed to the admin"
    );
    assert_eq!(body["data"]["sessions"][0]["ip_address"], json!("10.0.0.9"));
}

#[tokio::test]
async fn test_admin_sessions_is_admin_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/admin1/sessions",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(body["message"].as_str().unwrap_or("").contains("Admin"));
}

#[tokio::test]
async fn test_admin_revoke_user_sessions_revokes_everything() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    seed_sessions(
        &state,
        "regular1",
        &["tok-cccccccc-live", "tok-dddddddd-live"],
    )
    .await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/regular1/sessions",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["revoked"], json!(2));

    // And the list is now empty.
    let (_, listed) = send(
        &mut r,
        Method::GET,
        "/api/users/regular1/sessions",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(listed["data"]["total"], json!(0));
}

#[tokio::test]
async fn test_admin_revoke_user_sessions_is_admin_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;

    // A member must NOT be able to nuke anyone's (even their own remaining
    // devices') sessions through THIS endpoint — that is what
    // /security/sessions/revoke-all is for.
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/users/admin1/sessions",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_admin_revoke_user_sessions_without_session_is_refused() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = users_router(state);

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/anybody/sessions",
        None,
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    assert!(body["message"].as_str().unwrap_or("").contains("token"));
}

// ===========================================================================
// Admin account expiry (2026-10-20, migration 050)
// ===========================================================================

#[tokio::test]
async fn test_set_expiry_and_read_it_back() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // Set a horizon a year out.
    let when = (chrono::Utc::now() + chrono::Duration::days(365)).to_rfc3339();
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/expiry",
        Some(&token),
        Some(json!({"expires_at": when})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["expires_at"], json!(when));

    // The stored user carries it; sign-in still works (future horizon).
    let user = state.auth.get_user("regular1").await.expect("get_user");
    assert!(user.expires_at.is_some(), "horizon must be stored");
    assert!(!user.is_expired_at(chrono::Utc::now()));

    // Clear it — back to "never".
    let (status, cleared) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/expiry",
        Some(&token),
        Some(json!({"expires_at": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["data"]["expires_at"], json!(null));
    let user = state.auth.get_user("regular1").await.expect("get_user");
    assert!(user.expires_at.is_none(), "cleared horizon must be NULL");
}

#[tokio::test]
async fn test_set_expiry_rejects_past_and_garbage() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // A horizon in the past would silently lock the account out — refused.
    let past = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/expiry",
        Some(&token),
        Some(json!({"expires_at": past})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);

    // Garbage never reaches the column either.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/expiry",
        Some(&token),
        Some(json!({"expires_at": "not-a-date"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_set_expiry_is_admin_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/users/regular1/expiry",
        Some(&token),
        Some(json!({"expires_at": null})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_expired_account_cannot_authenticate() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "expired1", "Passw0rd!123", 1).await;
    // Set the horizon in the past directly (the endpoint refuses that by
    // design) and give the account a live session.
    sqlx::query("UPDATE users SET expires_at = ? WHERE username = 'expired1'")
        .bind((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339())
        .execute(&*state.db.pool())
        .await
        .expect("set past horizon");
    let mut r = users_router(state);

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": "expired1", "password": "Passw0rd!123"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        body["message"].as_str().unwrap_or("").contains("expired"),
        "the message must say the account expired, got: {}",
        body["message"]
    );
}

#[tokio::test]
async fn test_sweep_deletes_sessions_of_expired_accounts_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "gone1", "Passw0rd!123", 1).await;
    create_test_user(&state, "alive1", "Passw0rd!123", 1).await;
    sqlx::query("UPDATE users SET expires_at = ? WHERE username = 'gone1'")
        .bind((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339())
        .execute(&*state.db.pool())
        .await
        .expect("set past horizon");
    seed_sessions(&state, "gone1", &["tok-gone-aaaaaaa"]).await;
    seed_sessions(&state, "alive1", &["tok-alive-aaaaaaa"]).await;

    let deleted = state
        .auth
        .revoke_expired_account_sessions()
        .await
        .expect("sweep");
    assert_eq!(deleted, 1, "only the expired account's session dies");

    let session_repo = homevault::storage::session_repo::SessionRepository::new(state.db.pool());
    assert!(session_repo
        .find_by_token("tok-gone-aaaaaaa")
        .await
        .unwrap()
        .is_none());
    assert!(session_repo
        .find_by_token("tok-alive-aaaaaaa")
        .await
        .unwrap()
        .is_some());
}

// ===========================================================================
// Host-account lifecycle (2026-10-21)
//
// The real `userdel`/`usermod -L`/`chpasswd` half needs root and a disposable
// machine — see `tests/login_pam_integration.rs` for the `#[ignore]`d host
// tests. What is pinned here is every decision the handlers make *around*
// those commands, without touching the host.
// ===========================================================================

/// Insert a user row the way a PAM sign-in does: sentinel hash, no
/// HomeVault-chosen password.
async fn create_host_mirror_row(state: &AppState, username: &str, role_id: i64) {
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, '!pam-provisioned', '', '', ?, 0, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("Failed to insert host mirror row");
}

#[tokio::test]
async fn test_users_list_flags_host_accounts() {
    // The Users page needs to tell a HomeVault account apart from a mirrored
    // Linux one: only the former is HomeVault's to delete.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "plainuser", "Passw0rd!123", 1).await;
    create_host_mirror_row(&state, "hostuser", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (_, body) = send(&mut r, Method::GET, "/api/users", Some(&token), None).await;
    let users = body["data"].as_array().expect("user list");
    let find = |name: &str| {
        users
            .iter()
            .find(|u| u["username"] == json!(name))
            .cloned()
            .unwrap_or_else(|| panic!("'{}' missing from the list", name))
    };

    assert_eq!(find("plainuser")["is_host_account"], json!(false));
    assert_eq!(find("hostuser")["is_host_account"], json!(true));
    assert_eq!(find("admin1")["is_host_account"], json!(false));
}

#[tokio::test]
async fn test_user_get_flags_host_accounts() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_host_mirror_row(&state, "hostuser", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/users/hostuser",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["is_host_account"], json!(true));
}

#[tokio::test]
async fn test_user_delete_removes_a_stale_host_mirror_row() {
    // The Linux account is already gone (removed on the server by hand), so
    // the mirror row is the only thing left. Deleting it must be allowed —
    // otherwise the Users page could never be cleaned up.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_host_mirror_row(&state, "stalehost", 1).await;
    let mut r = users_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/stalehost",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["success"],
        json!(true),
        "stale mirror row must be removable"
    );
    assert!(!state.auth.user_exists("stalehost").await);
}

#[tokio::test]
async fn test_user_delete_drops_the_home_folder_row_even_when_files_are_kept() {
    // A deleted account's home configuration must not survive it: a new
    // account created with the same username would otherwise inherit the old
    // disk/path/quota settings (same reasoning as the transfer-quota and
    // recents cleanups).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "homeduser", "Passw0rd!123", 1).await;
    let repo = homevault::storage::user_home_repo::UserHomeRepository::new(state.db.pool());
    repo.upsert(&homevault::storage::user_home_repo::UserHome {
        username: "homeduser".to_string(),
        enabled: true,
        disk: Some("sda".to_string()),
        path: Some("/tmp/hv-test-homeduser".to_string()),
        created_at: chrono::Utc::now(),
        size_limit_bytes: Some(1024),
    })
    .await
    .expect("home row");

    let mut r = users_router(state.clone());
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/users/homeduser",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true));

    assert!(
        repo.find_by_username("homeduser").await.unwrap().is_none(),
        "the home configuration row must go with the account"
    );
    assert!(
        !std::path::Path::new("/tmp/hv-test-homeduser").exists(),
        "the files are kept when delete_home_folder is not set"
    );
}

#[tokio::test]
async fn test_user_demotion_revokes_the_demoted_sessions() {
    // A session carries the `is_admin` flag it was created with. Demoting an
    // admin must therefore sign them out, or they keep admin API access for
    // the rest of the session lifetime.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "admin2", "Passw0rd!123", 2).await;
    let mut r = users_router(state.clone());
    let token1 = login(&mut r, "admin1", "Passw0rd!123").await;
    let token2 = login(&mut r, "admin2", "Passw0rd!123").await;

    // admin2 can use admin endpoints before the demotion.
    let (_, before) = send(&mut r, Method::GET, "/api/users", Some(&token2), None).await;
    assert_eq!(before["success"], json!(true));

    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin2/role",
        Some(&token1),
        Some(json!({"role_id": 1})),
    )
    .await;
    assert_eq!(body["success"], json!(true), "demotion itself succeeds");

    let user = state.auth.get_user("admin2").await.expect("admin2");
    assert_eq!(user.role_id, 1);

    // The old token is gone, so the demoted account cannot keep acting.
    let (_, after) = send(&mut r, Method::GET, "/api/users", Some(&token2), None).await;
    assert_eq!(
        after["success"],
        json!(false),
        "the demoted admin's session must be revoked"
    );
}

#[tokio::test]
async fn test_user_create_rejects_more_than_fifty_groups() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let mut payload = valid_create_payload("toomanygroups");
    payload["groups"] = json!((0..51).map(|i| format!("group{}", i)).collect::<Vec<_>>());
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Maximum 50 groups"));
}

#[tokio::test]
async fn test_user_update_groups_rejects_an_unknown_group() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    create_test_user(&state, "member1", "Passw0rd!123", 1).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/users/member1/groups",
        Some(&token),
        Some(json!({"groups": ["ghostgroup"]})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("does not exist"));
}

#[tokio::test]
async fn test_user_create_rejects_a_blank_group_name() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = users_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let mut payload = valid_create_payload("blankgroupuser");
    payload["groups"] = json!(["  "]);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(&token),
        Some(payload),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}
