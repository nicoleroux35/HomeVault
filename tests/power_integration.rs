/* =============================================================================
 * File          : power_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Power-management integration tests: in-memory SQLite + assembled Axum
 *   router, exercising auth guards and input validation via oneshot
 *   requests. Real systemctl/hdparm behaviour is deliberately NOT exercised
 *   (host actions — see CLAUDE.md "What NOT to test in Rust"); every test
 *   path below stops at validation or reads only.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the power API:
//!
//!   - GET  /api/power/status
//!   - PUT  /api/power/governor
//!   - POST /api/power/{action}
//!   - GET  /api/power/spindown
//!   - PUT  /api/power/spindown
//!   - POST /api/power/spindown/apply
//!   - POST /api/server/restart, /api/server/shutdown (ROADMAP 24.2: same
//!     admin gate and the same `power::host_power_action` primitive as
//!     `/api/power/{action}`, so the two families cannot diverge again)

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::power::{
    handle_power_action, handle_power_governor_set, handle_power_spindown_apply,
    handle_power_spindown_get, handle_power_spindown_set, handle_power_status,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::models::{ADMIN_ROLE_ID, USER_ROLE_ID};
use homevault::storage::Database;
use homevault::storage::WidgetRepository;

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

fn power_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/auth/login",
            axum::routing::post(homevault::handlers::auth::handle_login),
        )
        .route("/api/power/status", get(handle_power_status))
        .route(
            "/api/power/governor",
            axum::routing::put(handle_power_governor_set),
        )
        .route(
            "/api/power/{action}",
            axum::routing::post(handle_power_action),
        )
        .route("/api/power/spindown", get(handle_power_spindown_get))
        .route(
            "/api/power/spindown",
            axum::routing::put(handle_power_spindown_set),
        )
        .route(
            "/api/power/spindown/apply",
            axum::routing::post(handle_power_spindown_apply),
        )
        // ROADMAP 24.2: the server-control family shares the power primitive.
        .route(
            "/api/server/restart",
            axum::routing::post(homevault::handlers::system::handle_server_restart),
        )
        .route(
            "/api/server/shutdown",
            axum::routing::post(homevault::handlers::system::handle_server_shutdown),
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
    .expect("insert user");
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

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/auth/login")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"username": username, "password": password}).to_string(),
        ))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["data"]["token"].as_str().expect("token").to_string()
}

#[tokio::test]
async fn all_endpoints_are_admin_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = power_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;
    let user = login(&mut router, "user", "UserPass123!").await;

    // Anonymous is rejected everywhere.
    let (status, _) = send(&mut router, Method::GET, "/api/power/status", None, None).await;
    assert_ne!(status, StatusCode::OK);
    let (status, _) = send(&mut router, Method::GET, "/api/power/spindown", None, None).await;
    assert_ne!(status, StatusCode::OK);

    // Non-admin is forbidden on every read and every action.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/power/status",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/power/spindown",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/power/governor",
        Some(&user),
        Some(json!({"governor": "performance"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/reboot",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "host reboot must be admin-only"
    );

    // Admin passes on the reads.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/power/status",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["available_governors"].is_array());
    assert!(body["data"]["governor_supported"].is_boolean());
    assert!(body["data"]["spindown_supported"].is_boolean());
}

/// ROADMAP 24.2 — `/api/server/*` and `/api/power/*` must answer the same
/// authorization contract. They used to run different commands through
/// different privilege paths (bare `shutdown` vs `sudo systemctl`), so one
/// worked and the other was refused on a normal install.
///
/// Only the refusal paths are exercised: an admin call on any of these four
/// routes really takes the host down, which is why no test below sends one.
#[tokio::test]
async fn server_control_and_power_share_one_admin_gate() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = power_router(state);
    let _admin = login(&mut router, "admin", "AdminPass123!").await;
    let user = login(&mut router, "user", "UserPass123!").await;

    for uri in [
        "/api/server/restart",
        "/api/server/shutdown",
        "/api/power/reboot",
        "/api/power/shutdown",
    ] {
        // Anonymous: both families now answer `401`. They used to differ —
        // `/api/server/*` checked the token inline (401) while `/api/power/*`
        // went through `require_admin`, which answered `400` for a missing
        // header. A client branching on the status read one as "not signed in"
        // and the other as "malformed request" (ROADMAP X.8).
        let (status, body) = send(&mut router, Method::POST, uri, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{uri} must answer 401 to an anonymous caller: {body}"
        );

        let (status, body) = send(&mut router, Method::POST, uri, Some(&user), None).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{uri} must be admin-only: {body}"
        );
    }
}

/// Both families must reject a bogus session the same way, and neither may
/// fall through to the host command on a failed lookup. (An invalid token is
/// `401` in both: `/api/server/*` checks it inline, `require_admin` answers
/// `401` for an unknown session too — only the *missing* header differs, see
/// ROADMAP X.8.)
#[tokio::test]
async fn an_invalid_session_is_rejected_on_every_power_route() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = power_router(state);

    for uri in [
        "/api/server/restart",
        "/api/server/shutdown",
        "/api/power/reboot",
        "/api/power/shutdown",
        "/api/power/suspend",
    ] {
        let (status, body) = send(&mut router, Method::POST, uri, Some("not-a-session"), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{uri} must reject an unknown session before any host call: {body}"
        );
        assert_eq!(body["success"], false, "{uri}: {body}");
    }
}

/// The server-control routes are POST-only (a `GET` must not be a way in, and
/// it documents them as actions rather than resources).
#[tokio::test]
async fn server_control_routes_only_answer_post() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = power_router(state);

    for uri in ["/api/server/restart", "/api/server/shutdown"] {
        let (status, _) = send(&mut router, Method::GET, uri, None, None).await;
        assert_eq!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{uri} must only accept POST"
        );
    }
}

/// ROADMAP 24.1 — the refusal must reach the client body, not only the log.
///
/// This asserts the handler's response mapper directly instead of POSTing as
/// an admin: an admin request on any of these routes really takes the test
/// host down, and a `PATH` shim would be process-global (the suite runs in
/// parallel). The mapper is the exact seam `handle_server_*` uses, so the
/// wire contract is still the thing under test.
#[test]
fn a_refused_host_command_reaches_the_response_body() {
    use homevault::handlers::system::power_outcome;
    use homevault::power::PowerAction;

    let reason = "systemctl reboot failed: Interactive authentication required";
    let (status, body) = power_outcome(PowerAction::Reboot, "restart", &Err(reason.to_string()));

    // A refusal is a failure the screen renders as a `role="alert"`, carrying
    // the host's own words — the desktop shows `message` verbatim.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["success"], false);
    assert!(
        body["message"].as_str().unwrap_or_default().contains(reason),
        "the refusal reason must reach the body: {body}"
    );
    assert!(body.get("data").is_none());
}

/// …and an accepted command answers the status the desktop announces before
/// the socket drops (`restarting` / `shutting_down`).
#[test]
fn an_accepted_host_command_answers_its_status() {
    use homevault::handlers::system::power_outcome;
    use homevault::power::PowerAction;

    let (status, body) = power_outcome(PowerAction::Shutdown, "shutdown", &Ok(()));
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["status"], "shutting_down");
    assert_eq!(body["data"]["message"], "Server shutdown initiated");
}

#[tokio::test]
async fn unknown_power_action_is_rejected_without_executing() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = power_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/format-everything",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Unknown power action"),
        "{body}"
    );
}

#[tokio::test]
async fn governor_rejects_values_not_offered_by_the_kernel() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = power_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // Metacharacters must never reach `sudo tee`.
    for bad in ["performance; rm -rf /", "$(reboot)", "not_a_governor", ""] {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/power/governor",
            Some(&admin),
            Some(json!({ "governor": bad })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "'{bad}' must be rejected: {body}"
        );
    }

    // Missing field.
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/power/governor",
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn spindown_validation_rejects_bad_devices_and_ranges() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = power_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // Path traversal / option injection in the device name.
    for bad in ["../sda", "/dev/sda", "sda;reboot", ""] {
        let (status, _) = send(
            &mut router,
            Method::PUT,
            "/api/power/spindown",
            Some(&admin),
            Some(json!({"device": bad, "apm_level": 127})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "'{bad}' must be rejected");
    }

    // Out-of-range APM and spindown values.
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/power/spindown",
        Some(&admin),
        Some(json!({"device": "sda", "apm_level": 300})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "apm_level > 255 must be rejected"
    );
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/power/spindown",
        Some(&admin),
        Some(json!({"device": "sda", "spindown_minutes": 9999})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "spindown > 330 min has no hdparm code"
    );

    // A valid configuration for a device the test host may not have still
    // round-trips the setting (apply is best-effort, never fatal).
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/power/spindown",
        Some(&admin),
        Some(json!({"device": "sda", "apm_level": 127, "spindown_minutes": 20})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["setting"]["apm_level"], 127);
    assert!(
        body["data"]["applied"].is_boolean(),
        "apply result reported, never fatal"
    );

    // The saved setting is listed back.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/power/spindown",
        Some(&admin),
        None,
    )
    .await;
    let settings = body["data"]["settings"].as_array().unwrap();
    assert_eq!(settings.len(), 1);
    assert_eq!(settings[0]["device"], "sda");

    // Both fields null clears the row (the handler returns success with an
    // empty list afterwards).
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/power/spindown",
        Some(&admin),
        Some(json!({"device": "sda"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/power/spindown",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["settings"].as_array().unwrap().len(), 0);

    // Apply-now on an empty config is a harmless success.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/spindown/apply",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
