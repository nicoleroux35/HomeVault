/* =============================================================================
 * File          : password_change_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   First-sign-in password change integration tests: in-memory SQLite +
 *   assembled Axum router for login + /auth/me + the users password-change
 *   endpoint, layered with the production
 *   `middleware::password_change_guard` so the allowlist/block behaviour is
 *   exercised at HTTP level via tower oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test password_change_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use axum::Router;
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
    auth::{handle_get_current_user, handle_login},
    users::{handle_user_create, handle_user_update_password},
    AppState,
};
use homevault::middleware::password_change_guard_middleware;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

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
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations().await.expect("Failed to run migrations");
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

/// Router mirroring the production layering of the routes involved: all
/// routes registered first, then the guard layered over them.
fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/auth/me", axum::routing::get(handle_get_current_user))
        .route("/api/users", axum::routing::post(handle_user_create))
        .route(
            "/api/users/{username}/password",
            axum::routing::put(handle_user_update_password).delete(handle_user_delete_placeholder),
        )
        .with_state(state.clone())
        .layer(axum::middleware::from_fn_with_state(
            state,
            password_change_guard_middleware,
        ))
}

/// Why the standalone allowlist route matters: with only the three
/// password-flow routes mounted there is nothing the guard could block, so a
/// second "other" route (never reachable by a flagged account) is needed for
/// the negative tests. Registered as an always-failing placeholder on the
/// same password path with a different method.
async fn handle_user_delete_placeholder() -> &'static str {
    "placeholder"
}

/// Insert a user directly into the DB (flag off — as setup/recovery do).
async fn seed_user(state: &AppState, username: &str, password: &str, role_id: i64) {
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

/// Login and return the full data object (token + flags), not just the token.
async fn login_data(router: &mut Router, username: &str, password: &str) -> Value {
    let (_, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
    )
    .await;
    body["data"].clone()
}

// ===========================================================================
// 1. The flag life-cycle
// ===========================================================================

/// A member created through the admin API arrives flagged: login reports it,
/// and the flag is what the change screen keys off.
#[tokio::test]
async fn test_login_flags_a_user_created_through_the_admin_api() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    let (_, create) = send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({
            "username": "member1",
            "password": "MemberPass123",
            "fullname": "Member One",
            "email": "",
            "role_id": 1
        })),
    )
    .await;
    assert_eq!(
        create["success"],
        json!(true),
        "create must succeed: {create}"
    );

    let member = login_data(&mut r, "member1", "MemberPass123").await;
    assert_eq!(
        member["must_change_password"],
        json!(true),
        "an admin-chosen password must be flagged at login"
    );
    assert_eq!(
        member["is_admin"],
        json!(false),
        "the created account is a regular member"
    );
}

/// Directly seeded rows (setup wizard, recovery, SQL seeds) carry no flag —
/// the column defaults to 0 and login must not invent one.
#[tokio::test]
async fn test_login_does_not_flag_admin_chosen_or_seeded_passwords() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    assert_eq!(
        admin["must_change_password"],
        json!(false),
        "a directly seeded password is owner-chosen and must not be flagged"
    );
}

/// /auth/me exposes the flag so a page refresh (no fresh login body) still
/// routes to the change screen instead of the desktop.
#[tokio::test]
async fn test_me_exposes_must_change_password() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({
            "username": "member1",
            "password": "MemberPass123",
            "fullname": "Member One",
            "email": "",
            "role_id": 1
        })),
    )
    .await;
    send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({
            "username": "clean1",
            "password": "CleanPass123",
            "fullname": "Clean One",
            "email": "",
            "role_id": 1
        })),
    )
    .await;

    let member = login_data(&mut r, "member1", "MemberPass123").await;
    let (_, me) = send(
        &mut r,
        Method::GET,
        "/api/auth/me",
        Some(member["token"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(me["data"]["must_change_password"], json!(true));

    // The admin resets clean1's password — that must RE-ARM the flag on an
    // account that was seeded clean (the mirror image of the self change).
    let (_, reset) = send(
        &mut r,
        Method::PUT,
        "/api/users/clean1/password",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({"new_password": "ResetPass123"})),
    )
    .await;
    assert_eq!(reset["success"], json!(true));
    let clean = login_data(&mut r, "clean1", "ResetPass123").await;
    assert_eq!(
        clean["must_change_password"],
        json!(true),
        "an admin reset must flag the account again, even when it was clean"
    );
    let (_, me_reset) = send(
        &mut r,
        Method::GET,
        "/api/auth/me",
        Some(clean["token"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(me_reset["data"]["must_change_password"], json!(true));

    // ... and the admin account itself stays unflagged after /auth/me.
    let (_, me_admin) = send(
        &mut r,
        Method::GET,
        "/api/auth/me",
        Some(admin["token"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(me_admin["data"]["must_change_password"], json!(false));
}

/// A self change (current password verified) clears the flag on the next
/// login; the service-side session invalidation forces the re-login.
#[tokio::test]
async fn test_self_change_clears_the_flag() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({
            "username": "member1",
            "password": "MemberPass123",
            "fullname": "Member One",
            "email": "",
            "role_id": 1
        })),
    )
    .await;

    let member1 = login_data(&mut r, "member1", "MemberPass123").await;
    let token = member1["token"].as_str().unwrap();
    let (_, changed) = send(
        &mut r,
        Method::PUT,
        "/api/users/member1/password",
        Some(token),
        Some(json!({"new_password": "NewOwnerPass1", "current_password": "MemberPass123"})),
    )
    .await;
    assert_eq!(changed["success"], json!(true));

    let again = login_data(&mut r, "member1", "NewOwnerPass1").await;
    assert_eq!(
        again["must_change_password"],
        json!(false),
        "the owner-chosen password must clear the flag"
    );
}

/// An admin changing their own password must not flag themselves: the
/// "someone else chose this secret" rationale does not apply.
#[tokio::test]
async fn test_admin_self_change_does_not_flag() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    let (_, changed) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin1/password",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({"new_password": "AdminPass005"})),
    )
    .await;
    assert_eq!(changed["success"], json!(true));

    let again = login_data(&mut r, "admin1", "AdminPass005").await;
    assert_eq!(
        again["must_change_password"],
        json!(false),
        "an admin's own change is owner-chosen"
    );
}

// ===========================================================================
// 2. The server-side boundary (middleware)
// ===========================================================================

/// The middleware is the boundary the change screen cannot bypass: every
/// route outside the allowlist answers 403 `password_change_required` to a
/// flagged session — even a normally-permitted one.
#[tokio::test]
async fn test_flagged_session_is_limited_to_the_change_flow() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({
            "username": "member1",
            "password": "MemberPass123",
            "fullname": "Member One",
            "email": "",
            "role_id": 1
        })),
    )
    .await;

    let member1 = login_data(&mut r, "member1", "MemberPass123").await;
    let token = member1["token"].as_str().unwrap();

    // Anything else is refused with the dedicated code, not a generic 401.
    let (status, blocked) = send(
        &mut r,
        Method::DELETE,
        "/api/users/member1/password",
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(blocked["code"], json!("password_change_required"));

    // A flagged session must not reset somebody else's password either: the
    // route matches by shape but NOT by target.
    let (status, other) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin1/password",
        Some(token),
        Some(json!({"new_password": "HijackPass1"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(other["code"], json!("password_change_required"));
    assert_eq!(
        other["success"],
        json!(false),
        "the block must come from the guard, not the handler"
    );
}

/// The change screen works end to end within the flagged session: whoami is
/// readable, the SELF password change passes the guard, and the bad-current
/// case is the handler's 4xx (not the guard's 403-code response).
#[tokio::test]
async fn test_change_screen_endpoints_pass_for_a_flagged_session() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    send(
        &mut r,
        Method::POST,
        "/api/users",
        Some(admin["token"].as_str().unwrap()),
        Some(json!({
            "username": "member1",
            "password": "MemberPass123",
            "fullname": "Member One",
            "email": "",
            "role_id": 1
        })),
    )
    .await;

    let member1 = login_data(&mut r, "member1", "MemberPass123").await;
    let token = member1["token"].as_str().unwrap();

    // whoami (allowlisted) answers normally.
    let (status, me) = send(&mut r, Method::GET, "/api/auth/me", Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["success"], json!(true));

    // Wrong current password: the HANDLER must own this failure (400/401-ish
    // validation error message), never the guard's password_change_required
    // code — otherwise a typo read as "you are blocked" would be confusing.
    let (status_bad, bad) = send(
        &mut r,
        Method::PUT,
        "/api/users/member1/password",
        Some(token),
        Some(json!({"new_password": "NewOwnerPass1", "current_password": "WrongCurrent1"})),
    )
    .await;
    assert_ne!(
        status_bad,
        StatusCode::FORBIDDEN,
        "guard must not fire: {bad}"
    );
    assert_eq!(bad["success"], json!(false));
    assert!(
        bad["message"].as_str().unwrap_or("").contains("password"),
        "the handler's password error must surface: {bad}"
    );

    // Correct current password: allowed by the guard and succeeds.
    let (status_ok, ok) = send(
        &mut r,
        Method::PUT,
        "/api/users/member1/password",
        Some(token),
        Some(json!({"new_password": "NewOwnerPass1", "current_password": "MemberPass123"})),
    )
    .await;
    assert_eq!(status_ok, StatusCode::OK);
    assert_eq!(ok["success"], json!(true));
}

/// A cleared (or never-set) session is untouched by the middleware: the same
/// route that was blocked answers from its handler again.
#[tokio::test]
async fn test_unflagged_session_is_not_intercepted() {
    let (state, _tmp, _g) = setup_state().await;
    seed_user(&state, "admin1", "AdminPass123", 2).await;
    let mut r = router(state);

    let admin = login_data(&mut r, "admin1", "AdminPass123").await;
    let token = admin["token"].as_str().unwrap();

    let (status, me) = send(&mut r, Method::GET, "/api/auth/me", Some(token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["data"]["must_change_password"], json!(false));

    // The other-password route blocked above is the handler's business now —
    // here the admin targets itself, so it succeeds.
    let (status_self, self_change) = send(
        &mut r,
        Method::PUT,
        "/api/users/admin1/password",
        Some(token),
        Some(json!({"new_password": "AdminPass005"})),
    )
    .await;
    assert_eq!(status_self, StatusCode::OK);
    assert_eq!(self_change["success"], json!(true));
}
