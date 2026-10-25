/* =============================================================================
 * File          : dashboard_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Dashboard integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Dashboard API — `crate::handlers::{dashboard,
//! widgets}`:
//!
//!   Reads (require_auth — any valid session):
//!     GET  /api/dashboard/data | /widgets | /widget/{id}
//!     GET  /api/dashboard/network-traffic | /disk-io | /storage
//!     GET  /api/dashboard/cpu-temperature | /storage-volumes
//!     GET  /api/dashboard/health | /server-details
//!     GET  /api/widgets/cpu | /ram | /network-traffic | /cpu-temp | /update-info
//!   Widget CRUD (require_auth, scoped to the caller's own grid):
//!     POST   /api/dashboard/widget
//!     DELETE /api/dashboard/widget/{id}
//!     POST   /api/dashboard/widgets/reorder
//!   Per-user widget preferences (require_auth since ROADMAP 15.1 — an
//!   expired session used to be accepted because the handler hand-rolled a
//!   `SELECT ... FROM sessions WHERE token = ?` with no `expires_at`):
//!     GET|POST /api/dashboard/widget-preferences
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! The read endpoints inventory the host (`df`, `/proc/*`, `sensors`,
//! `smartctl`, `dmidecode`, `nvidia-smi`) best-effort and always answer
//! `200` with whatever they could gather — safe to run for real. Nothing
//! here mutates the host. The widget + preference tables are plain app
//! state, exercised end to end.
//!
//! Since ROADMAP 15.1 a tile belongs to exactly one account (`widgets` is
//! keyed by `(username, id)`), so the CRUD cases check that one account can
//! neither see, delete nor reorder another's tiles, and that an add request
//! carrying junk ids/titles/sizes is refused before it reaches SQL.
//!
//! `/api/health` (the Health page's own endpoint) is covered by the Health
//! section, not here.
//!
//! Run:
//!   cargo test --test dashboard_integration

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
use homevault::handlers::dashboard::{
    handle_add_widget, handle_cpu_temperature, handle_dashboard_data, handle_disk_io_data,
    handle_get_all_widgets, handle_get_widget_preferences, handle_health_data,
    handle_network_traffic_data, handle_remove_widget, handle_reorder_widgets,
    handle_server_details, handle_storage_data, handle_storage_volumes,
    handle_update_widget_preference, handle_widget_data,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::widgets::{
    handle_widget_cpu_temp, handle_widget_cpu_usage, handle_widget_network_traffic,
    handle_widget_ram_usage, handle_widget_update_info,
};
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

fn dashboard_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/dashboard/data", get(handle_dashboard_data))
        .route("/api/dashboard/widgets", get(handle_get_all_widgets))
        .route("/api/dashboard/widget", post(handle_add_widget))
        .route(
            "/api/dashboard/widget/{id}",
            get(handle_widget_data).delete(handle_remove_widget),
        )
        .route(
            "/api/dashboard/widgets/reorder",
            post(handle_reorder_widgets),
        )
        .route(
            "/api/dashboard/network-traffic",
            get(handle_network_traffic_data),
        )
        .route("/api/dashboard/disk-io", get(handle_disk_io_data))
        .route("/api/dashboard/storage", get(handle_storage_data))
        .route(
            "/api/dashboard/cpu-temperature",
            get(handle_cpu_temperature),
        )
        .route(
            "/api/dashboard/storage-volumes",
            get(handle_storage_volumes),
        )
        .route("/api/dashboard/health", get(handle_health_data))
        .route("/api/dashboard/server-details", get(handle_server_details))
        .route(
            "/api/dashboard/widget-preferences",
            get(handle_get_widget_preferences).post(handle_update_widget_preference),
        )
        .route("/api/widgets/cpu", get(handle_widget_cpu_usage))
        .route("/api/widgets/ram", get(handle_widget_ram_usage))
        .route(
            "/api/widgets/network-traffic",
            get(handle_widget_network_traffic),
        )
        .route("/api/widgets/cpu-temp", get(handle_widget_cpu_temp))
        .route("/api/widgets/update-info", get(handle_widget_update_info))
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

/// Every GET endpoint that goes through `require_auth`.
fn read_endpoints() -> Vec<&'static str> {
    vec![
        "/api/dashboard/data",
        "/api/dashboard/widgets",
        "/api/dashboard/widget/cpu",
        "/api/dashboard/network-traffic",
        "/api/dashboard/disk-io",
        "/api/dashboard/storage",
        "/api/dashboard/cpu-temperature",
        "/api/dashboard/storage-volumes",
        "/api/dashboard/health",
        "/api/dashboard/server-details",
        "/api/widgets/cpu",
        "/api/widgets/ram",
        "/api/widgets/network-traffic",
        "/api/widgets/cpu-temp",
        "/api/widgets/update-info",
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = dashboard_router(state);
    for uri in read_endpoints() {
        let (status, _) = send(&mut router, Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "GET {uri}");
    }
}

#[tokio::test]
async fn test_read_endpoints_reject_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = dashboard_router(state);
    for uri in read_endpoints() {
        let (status, _) = send(&mut router, Method::GET, uri, Some("nope"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "GET {uri}");
    }
}

#[tokio::test]
async fn test_widget_crud_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = dashboard_router(state);
    let cases: &[(Method, &str, Option<Value>)] = &[
        (
            Method::POST,
            "/api/dashboard/widget",
            Some(json!({"id": "w", "wtype": "cpu", "title": "t", "size": "s"})),
        ),
        (Method::DELETE, "/api/dashboard/widget/w", None),
        (
            Method::POST,
            "/api/dashboard/widgets/reorder",
            Some(json!({"widgets": ["w"]})),
        ),
    ];
    for (method, uri, body) in cases {
        let (status, _) = send(&mut router, method.clone(), uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        let (status, _) = send(&mut router, method.clone(), uri, Some("nope"), body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_widget_preferences_reject_missing_token() {
    // A missing header is a malformed request (400) per `require_auth`; an
    // unknown token is an authentication failure (401).
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = dashboard_router(state);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));

    // The write side goes through the same guard.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget-preferences",
        Some("nope"),
        Some(json!({"widget_key": "cpu", "position_x": 1, "position_y": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_widget_preferences_reject_an_expired_session() {
    // Regression for ROADMAP 15.1: the handler used to run its own
    // `SELECT username FROM sessions WHERE token = ?`, which never looked at
    // `expires_at`, so an expired token kept reading AND writing preferences
    // while every other endpoint refused it.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(Arc::clone(&state));
    let token = login(&mut router, "u", "Password123!").await;

    sqlx::query("UPDATE sessions SET expires_at = ? WHERE token = ?")
        .bind(chrono::Utc::now() - chrono::Duration::hours(1))
        .bind(&token)
        .execute(&*state.db.pool())
        .await
        .expect("expire the session");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["success"], json!(false));

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget-preferences",
        Some(&token),
        Some(json!({"widget_key": "cpu", "position_x": 1, "position_y": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Nothing was written on the way through.
    let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_widget_preferences")
        .fetch_one(&*state.db.pool())
        .await
        .expect("count");
    assert_eq!(rows.0, 0, "an expired session must not write preferences");
}

#[tokio::test]
async fn test_widget_preferences_reject_invalid_values() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let cases = [
        // Unsafe key: would be reflected in messages and used as a map key.
        json!({"widget_key": "cpu/../etc", "position_x": 1, "position_y": 1}),
        json!({"widget_key": "", "position_x": 1, "position_y": 1}),
        // Off-screen panel coordinates.
        json!({"widget_key": "cpu", "position_x": 100_000, "position_y": 1}),
        json!({"widget_key": "cpu", "position_x": 1, "position_y": -100_000}),
    ];
    for body in cases {
        let (status, response) = send(
            &mut router,
            Method::POST,
            "/api/dashboard/widget-preferences",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(response["success"], json!(false), "{body}");
    }
}

// ---------------------------------------------------------------------------
// 2. Read endpoints — real host inventory, any session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_endpoints_allow_any_valid_session_and_never_500() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for uri in read_endpoints() {
        let (status, body) = send(&mut router, Method::GET, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        assert_eq!(body["success"], json!(true), "GET {uri}: {body}");
    }
}

#[tokio::test]
async fn test_dashboard_data_has_the_expected_shape() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/data",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["app_name"], "HomeVault");
    assert_eq!(body["data"]["status"], "ok");
    assert!(body["data"]["widgets"].is_array());
    assert!(body["data"]["timestamp"].is_string());
}

#[tokio::test]
async fn test_widget_data_known_and_unknown_ids() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    for (id, wtype) in [("cpu", "cpu"), ("memory", "memory"), ("uptime", "uptime")] {
        let (status, body) = send(
            &mut router,
            Method::GET,
            &format!("/api/dashboard/widget/{id}"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["type"], wtype, "{id}");
    }

    // An id no tile uses is a client bug: 404, not a `type: "unknown"` shell
    // that renders as an empty card (ROADMAP 15.3).
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget/does-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_widget_memory_data_reports_real_numbers() {
    // The memory tile used to answer with placeholders; it now reads
    // /proc/meminfo, so the payload must be self-consistent.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget/memory",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"]["data"];
    let total = data["total_bytes"].as_u64().unwrap_or(0);
    let used = data["used_bytes"].as_u64().unwrap_or(0);
    let available = data["available_bytes"].as_u64().unwrap_or(0);
    let percent = data["usage_percent"].as_u64().unwrap_or(0);

    // A host that cannot report memory has total 0; otherwise the numbers
    // must add up and the percentage must match them.
    if total > 0 {
        assert!(used <= total, "used {used} > total {total}");
        assert_eq!(used + available, total, "used + available must be total");
        let expected = (used as f64 / total as f64 * 100.0).round() as u64;
        assert_eq!(percent, expected);
        assert!(percent <= 100, "{percent}");
    }
}

// ---------------------------------------------------------------------------
// 3. Widget CRUD lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_widget_add_list_reorder_remove_lifecycle() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let add = |id: &str, row: u32, col: u32| json!({"id": id, "wtype": "cpu", "title": id, "size": "1x1", "row": row, "col": col});

    for (id, r, c) in [("alpha", 1, 1), ("beta", 1, 2)] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/dashboard/widget",
            Some(&token),
            Some(add(id, r, c)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "add {id}: {body}");
    }

    // Both show up in the widget list and on the dashboard payload.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widgets",
        Some(&token),
        None,
    )
    .await;
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["id"].as_str())
        .collect();
    assert!(ids.contains(&"alpha") && ids.contains(&"beta"), "{ids:?}");

    // Adding the same id again is rejected.
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget",
        Some(&token),
        Some(add("alpha", 2, 1)),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("already exists"));

    // Reorder puts beta first (index 0 → row 1 col 1), alpha second.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widgets/reorder",
        Some(&token),
        Some(json!({"widgets": ["beta", "alpha"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widgets",
        Some(&token),
        None,
    )
    .await;
    let first = body["data"][0]["id"].as_str().unwrap();
    assert_eq!(first, "beta", "reorder should sort beta to row 1 col 1");

    // Remove alpha; only beta remains.
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        "/api/dashboard/widget/alpha",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widgets",
        Some(&token),
        None,
    )
    .await;
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["beta"]);
}

// ---------------------------------------------------------------------------
// 4. Per-user widget preferences — upsert round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_widget_preferences_upsert_round_trip() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    // Starts empty.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));

    // Insert one.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget-preferences",
        Some(&token),
        Some(json!({"widget_key": "cpu", "position_x": 3, "position_y": 4, "is_visible": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["widget_key"], "cpu");
    assert_eq!(body["data"][0]["position_x"], 3);
    assert_eq!(body["data"][0]["is_visible"], false);

    // Update the same key — still one row, new values.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget-preferences",
        Some(&token),
        Some(json!({"widget_key": "cpu", "position_x": 9, "position_y": 9, "is_visible": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        1,
        "upsert must not duplicate"
    );
    assert_eq!(body["data"][0]["position_x"], 9);
    assert_eq!(body["data"][0]["is_visible"], true);
}

#[tokio::test]
async fn test_widget_preferences_are_scoped_per_user() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let alice = login(&mut router, "alice", "Password123!").await;
    let bob = login(&mut router, "bob", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget-preferences",
        Some(&alice),
        Some(json!({"widget_key": "cpu", "position_x": 1, "position_y": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Bob sees none of Alice's preferences.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widget-preferences",
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

// ---------------------------------------------------------------------------
// 5. Widget ownership — one account can never touch another's grid
//    (ROADMAP 15.1: any authenticated user could delete or reorder every
//    widget, because `username` was hardcoded to `None`)
// ---------------------------------------------------------------------------

/// Add one tile through the API and assert it succeeded.
async fn add_widget(router: &mut Router, token: &str, id: &str, row: u32) {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/dashboard/widget",
        Some(token),
        Some(json!({"id": id, "wtype": "cpu", "title": id, "size": "1x1", "row": row, "col": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "add {id}: {body}");
    assert_eq!(body["success"], json!(true), "add {id}: {body}");
}

async fn widget_ids(router: &mut Router, token: &str) -> Vec<String> {
    let (_, body) = send(
        router,
        Method::GET,
        "/api/dashboard/widgets",
        Some(token),
        None,
    )
    .await;
    body["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|w| w["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn test_widgets_are_scoped_to_their_owner() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let alice = login(&mut router, "alice", "Password123!").await;
    let bob = login(&mut router, "bob", "Password123!").await;

    add_widget(&mut router, &alice, "alice-cpu", 1).await;

    // Bob's list and the shared dashboard payload show nothing of Alice's.
    assert_eq!(widget_ids(&mut router, &bob).await, Vec::<String>::new());
    assert_eq!(widget_ids(&mut router, &alice).await, vec!["alice-cpu"]);

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/data",
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(body["data"]["widgets"].as_array().map(|a| a.len()), Some(0));

    // Bob cannot delete it: the row is not his, so the id is indistinguishable
    // from a missing one.
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/dashboard/widget/alice-cpu",
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["success"], json!(false));
    assert_eq!(
        widget_ids(&mut router, &alice).await,
        vec!["alice-cpu"],
        "Bob's delete must not touch Alice's grid"
    );

    // Bob cannot move it either: the reorder matches none of his rows.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widgets/reorder",
        Some(&bob),
        Some(json!({"widgets": ["alice-cpu"]})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // And the id is free for Bob to use for his own tile.
    add_widget(&mut router, &bob, "alice-cpu", 2).await;
    assert_eq!(widget_ids(&mut router, &bob).await, vec!["alice-cpu"]);
    assert_eq!(widget_ids(&mut router, &alice).await, vec!["alice-cpu"]);

    // Alice deleting hers leaves Bob's intact.
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        "/api/dashboard/widget/alice-cpu",
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(widget_ids(&mut router, &alice).await, Vec::<String>::new());
    assert_eq!(widget_ids(&mut router, &bob).await, vec!["alice-cpu"]);
}

#[tokio::test]
async fn test_widget_remove_an_unknown_id_is_404() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/dashboard/widget/nope",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["success"], json!(false));
    assert!(body["message"].as_str().unwrap_or("").contains("not found"));
}

#[tokio::test]
async fn test_widget_rows_are_attributed_to_the_session_owner() {
    // The old handler wrote `username: None`, so no row had an owner. Every
    // row created through the API must now carry the caller's username.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "carol", "Password123!", 1).await;
    let mut router = dashboard_router(Arc::clone(&state));
    let token = login(&mut router, "carol", "Password123!").await;
    add_widget(&mut router, &token, "tile", 1).await;

    let owners: Vec<(String,)> = sqlx::query_as("SELECT username FROM widgets")
        .fetch_all(&*state.db.pool())
        .await
        .expect("read widgets");
    assert_eq!(owners, vec![("carol".to_string(),)]);
}

// ---------------------------------------------------------------------------
// 6. Widget validation (ROADMAP 15.1: the handler used to accept any id,
//    type, title, size and position)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_widget_add_rejects_invalid_fields() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let valid = || json!({"id": "cpu", "wtype": "cpu", "title": "CPU", "size": "1x1", "row": 1, "col": 1});
    let mut cases: Vec<(String, Value)> = Vec::new();

    let mut body = valid();
    body["id"] = json!("has space");
    cases.push(("unsafe id".into(), body));

    let mut body = valid();
    body["id"] = json!("a".repeat(65));
    cases.push(("over-long id".into(), body));

    let mut body = valid();
    body["title"] = json!("   ");
    cases.push(("blank title".into(), body));

    let mut body = valid();
    body["title"] = json!("t".repeat(121));
    cases.push(("over-long title".into(), body));

    let mut body = valid();
    body["size"] = json!("wide");
    cases.push(("bad size".into(), body));

    let mut body = valid();
    body["size"] = json!("9x9");
    cases.push(("oversized tile".into(), body));

    let mut body = valid();
    body["row"] = json!(0);
    cases.push(("row zero".into(), body));

    let mut body = valid();
    body["col"] = json!(9999);
    cases.push(("col off the grid".into(), body));

    let mut body = valid();
    body["row_span"] = json!(99);
    cases.push(("span too large".into(), body));

    for (label, payload) in cases {
        let (status, response) = send(
            &mut router,
            Method::POST,
            "/api/dashboard/widget",
            Some(&token),
            Some(payload.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {response}");
        assert_eq!(response["success"], json!(false), "{label}");
    }

    // Nothing was stored by the rejected attempts.
    assert_eq!(widget_ids(&mut router, &token).await, Vec::<String>::new());

    // A valid request still works, and the title is stored trimmed.
    let mut ok = valid();
    ok["title"] = json!("  CPU  ");
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget",
        Some(&token),
        Some(ok),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/dashboard/widgets",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"][0]["title"], "CPU");
}

#[tokio::test]
async fn test_widget_reorder_rejects_an_invalid_list() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(state);
    let token = login(&mut router, "u", "Password123!").await;
    add_widget(&mut router, &token, "cpu", 1).await;

    // A non-slug id is refused instead of reaching SQL.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widgets/reorder",
        Some(&token),
        Some(json!({"widgets": ["bad id"]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], json!(false));

    // Too many ids at once.
    let many: Vec<String> = (0..65).map(|i| format!("w{i}")).collect();
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widgets/reorder",
        Some(&token),
        Some(json!({"widgets": many})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // An empty list is a no-op success, not a 404.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widgets/reorder",
        Some(&token),
        Some(json!({"widgets": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn test_widget_add_is_capped_per_account() {
    // `MAX_WIDGETS_PER_USER` keeps one account from filling the table.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = dashboard_router(Arc::clone(&state));
    let token = login(&mut router, "u", "Password123!").await;

    for i in 0..64 {
        sqlx::query(
            "INSERT INTO widgets (username, id, wtype, title, size, row_pos, col_pos, row_span, col_span)
             VALUES ('u', ?, 'cpu', 't', '1x1', 1, 1, 1, 1)",
        )
        .bind(format!("tile{i}"))
        .execute(&*state.db.pool())
        .await
        .expect("seed widget");
    }

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/dashboard/widget",
        Some(&token),
        Some(json!({"id": "one-more", "wtype": "cpu", "title": "t", "size": "1x1", "row": 1, "col": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], json!(false));
    assert!(
        body["message"].as_str().unwrap_or("").contains("maximum"),
        "{body}"
    );

    // Another account is not affected by that cap.
    create_test_user(&state, "other", "Password123!", 1).await;
    let other = login(&mut router, "other", "Password123!").await;
    add_widget(&mut router, &other, "one-more", 1).await;
}
