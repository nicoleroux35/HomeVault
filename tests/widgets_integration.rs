/* =============================================================================
 * File          : widgets_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Widgets integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Widgets app's backend surface:
//!
//!   GET|POST /api/monitor-layout              (per-account layout blob — any session)
//!   GET      /api/security/active-sessions-count  (Sessions monitor — admin)
//!
//! Everything else the Widgets app does is frontend (`monitorStore`,
//! `Widgets.svelte`, the desktop monitor panels) and is covered by
//! `tests/e2e/widgets/widgets.spec.ts`.
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server. Nothing here
//! touches the host.
//!
//! Run:
//!   cargo test --test widgets_integration

use axum::{
    routing::{get, post},
    Router,
};
use chrono::{Duration, Utc};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::monitor_layout::{handle_get_monitor_layout, handle_set_monitor_layout};
use homevault::handlers::security::handle_get_active_sessions_count;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::widgets::handle_widget_update_info;
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

fn widgets_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/monitor-layout",
            get(handle_get_monitor_layout).post(handle_set_monitor_layout),
        )
        .route(
            "/api/security/active-sessions-count",
            get(handle_get_active_sessions_count),
        )
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

/// Insert a bare session row directly (bypassing the login flow) — used to
/// build specific active/expired/duplicate situations for the count.
async fn insert_session(
    state: &AppState,
    username: &str,
    token: &str,
    expires_at: chrono::DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at, is_admin)
         VALUES (?, ?, '127.0.0.1', 'csrf', ?, ?, 0)",
    )
    .bind(token)
    .bind(username)
    .bind(Utc::now().to_rfc3339())
    .bind(expires_at.to_rfc3339())
    .execute(&*state.db.pool())
    .await
    .expect("insert session");
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

// ---------------------------------------------------------------------------
// 1. monitor-layout — auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_monitor_layout_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = widgets_router(state);
    for (method, body) in [
        (Method::GET, None),
        (Method::POST, Some(json!({"layout_json": "{}"}))),
    ] {
        let (status, _) = send(
            &mut r,
            method.clone(),
            "/api/monitor-layout",
            None,
            body.clone(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} (no token → 401)"
        );
        let (status, _) = send(
            &mut r,
            method.clone(),
            "/api/monitor-layout",
            Some("nope"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} (bad token)");
    }
}

// ---------------------------------------------------------------------------
// 2. monitor-layout — round trip, upsert, per-user isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_monitor_layout_is_null_until_saved_then_round_trips() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/monitor-layout",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["layout_json"].is_null(), "{body}");

    let blob = r#"{"resourceMonitorVisible":false,"raidMonitor":{"x":10,"y":20}}"#;
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/monitor-layout",
        Some(&token),
        Some(json!({"layout_json": blob})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/monitor-layout",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["layout_json"], blob);
}

/// The blob is opaque to the backend (no `refresh_ms` column, no schema), so a
/// layout saved by an older frontend — before Settings > Widgets gained the
/// "Refresh interval" control — must survive a round trip byte for byte.
#[tokio::test]
async fn test_monitor_layout_round_trips_a_blob_without_refresh_ms() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let legacy = r#"{"resourceMonitorVisible":true,"raidMonitor":{"x":12,"y":34}}"#;
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/monitor-layout",
        Some(&token),
        Some(json!({"layout_json": legacy})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/monitor-layout",
        Some(&token),
        None,
    )
    .await;
    let stored = body["data"]["layout_json"].as_str().expect("layout_json");
    assert_eq!(stored, legacy);
    // No key was invented server-side — the frontend is what defaults it.
    assert!(!stored.contains("refreshMs"), "{stored}");
}

/// `refreshMs` lives in the same blob as the positions, so changing the
/// interval in Settings is a plain overwrite of that one key and the new value
/// comes back on the next read (this is what makes it follow the account across
/// browser origins).
#[tokio::test]
async fn test_monitor_layout_persists_a_changed_refresh_ms() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    for ms in [5000, 30000] {
        let blob = json!({"resourceMonitorVisible": true, "refreshMs": ms}).to_string();
        let (status, _) = send(
            &mut r,
            Method::POST,
            "/api/monitor-layout",
            Some(&token),
            Some(json!({"layout_json": blob})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = send(
            &mut r,
            Method::GET,
            "/api/monitor-layout",
            Some(&token),
            None,
        )
        .await;
        let stored: Value =
            serde_json::from_str(body["data"]["layout_json"].as_str().expect("layout_json"))
                .expect("stored layout_json must be valid JSON");
        assert_eq!(stored["refreshMs"], ms, "{body}");
        // The sibling keys are still there — the write is not a replace-all.
        assert_eq!(stored["resourceMonitorVisible"], true, "{body}");
    }
}

#[tokio::test]
async fn test_monitor_layout_upsert_keeps_one_row_per_user() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    for blob in [r#"{"v":1}"#, r#"{"v":2}"#, r#"{"v":3}"#] {
        let (status, _) = send(
            &mut r,
            Method::POST,
            "/api/monitor-layout",
            Some(&token),
            Some(json!({"layout_json": blob})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/monitor-layout",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["layout_json"], r#"{"v":3}"#);

    // Three POSTs, one row — proves it's an upsert, not an append.
    let rows: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM user_monitor_layout WHERE username = 'u'")
            .fetch_one(state.db.pool().as_ref())
            .await
            .unwrap();
    assert_eq!(rows.0, 1);
}

#[tokio::test]
async fn test_monitor_layout_is_scoped_per_account() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let alice = login(&mut r, "alice", "Password123!").await;
    let bob = login(&mut r, "bob", "Password123!").await;

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/monitor-layout",
        Some(&alice),
        Some(json!({"layout_json": r#"{"owner":"alice"}"#})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(&mut r, Method::GET, "/api/monitor-layout", Some(&bob), None).await;
    assert!(
        body["data"]["layout_json"].is_null(),
        "bob must not see alice's layout: {body}"
    );
}

// ---------------------------------------------------------------------------
// 3. monitor-layout validation — the blob is bounded before it is stored
//    (ROADMAP 15.1: the handler stored whatever string arrived)
// ---------------------------------------------------------------------------

/// POST a layout blob and return the status.
async fn post_layout(router: &mut Router, token: &str, blob: &str) -> (StatusCode, Value) {
    send(
        router,
        Method::POST,
        "/api/monitor-layout",
        Some(token),
        Some(json!({ "layout_json": blob })),
    )
    .await
}

#[tokio::test]
async fn test_monitor_layout_rejects_an_oversized_blob() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let huge = format!(r#"{{"blob":"{}"}}"#, "x".repeat(20 * 1024));
    let (status, body) = post_layout(&mut r, &token, &huge).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], json!(false));

    // Nothing was stored.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/monitor-layout",
        Some(&token),
        None,
    )
    .await;
    assert!(body["data"]["layout_json"].is_null(), "{body}");
}

#[tokio::test]
async fn test_monitor_layout_rejects_invalid_json() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    for blob in ["", "   ", "{", "not json", r#"{"a":}"#] {
        let (status, body) = post_layout(&mut r, &token, blob).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{blob:?}: {body}");
    }
}

#[tokio::test]
async fn test_monitor_layout_rejects_a_non_object_payload() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    for blob in ["[]", "[1,2]", "42", "null", "\"text\"", "true"] {
        let (status, body) = post_layout(&mut r, &token, blob).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{blob:?}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or("")
                .contains("JSON object"),
            "{blob:?}: {body}"
        );
    }
}

#[tokio::test]
async fn test_monitor_layout_rejects_deep_nesting_and_huge_keys() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let deep = r#"{"a":{"b":{"c":{"d":{"e":1}}}}}"#;
    let (status, body) = post_layout(&mut r, &token, deep).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("nested"),
        "{body}"
    );

    let long_key = format!(r#"{{"{}":1}}"#, "k".repeat(65));
    let (status, body) = post_layout(&mut r, &token, &long_key).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("key longer"),
        "{body}"
    );

    let long_value = format!(r#"{{"note":"{}"}}"#, "v".repeat(257));
    let (status, body) = post_layout(&mut r, &token, &long_value).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn test_monitor_layout_accepts_a_16_kib_boundary_blob() {
    // The cap is exclusive: a blob of exactly MAX_LAYOUT_BYTES still fits.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = widgets_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    // The byte cap is exclusive **and** every string value is capped at 256
    // characters, so a boundary blob is many short values: the single 16 KiB
    // string this test used to build was refused for the per-value cap (the
    // test above pins that refusal), not for its size.
    let mut map = std::collections::BTreeMap::new();
    let mut blob = String::from("{}");
    let mut i = 0usize;
    while blob.len() + 128 < 16 * 1024 {
        map.insert(format!("k{i}"), "x".repeat(100));
        blob = serde_json::to_string(&map).expect("serialize the layout");
        i += 1;
    }
    let deficit = 16 * 1024 - blob.len();
    assert!(
        (1..=128).contains(&deficit),
        "unexpected base size {} (deficit {deficit})",
        blob.len()
    );
    // Pad one value; 100 + deficit stays under the 256-character cap.
    blob = blob.replacen(&"x".repeat(100), &"x".repeat(100 + deficit), 1);
    assert_eq!(blob.len(), 16 * 1024);
    let (status, body) = post_layout(&mut r, &token, &blob).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/monitor-layout",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["data"]["layout_json"].as_str().map(str::len),
        Some(16 * 1024),
        "the accepted blob must round-trip byte for byte"
    );
}

// ---------------------------------------------------------------------------
// 4. active-sessions-count — auth + distinct/expiry semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_active_sessions_count_is_admin_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut r = widgets_router(state);

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/security/active-sessions-count",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/security/active-sessions-count",
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = login(&mut r, "regular", "Password123!").await;
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/security/active-sessions-count",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

async fn active_count(r: &mut Router, token: &str) -> i64 {
    let (_, body) = send(
        r,
        Method::GET,
        "/api/security/active-sessions-count",
        Some(token),
        None,
    )
    .await;
    body["data"]["count"].as_i64().unwrap()
}

#[tokio::test]
async fn test_active_sessions_count_dedupes_users_and_ignores_expired() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "alice", "Password123!", 1).await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    create_test_user(&state, "carol", "Password123!", 1).await;
    let mut r = widgets_router(state.clone());
    let token = login(&mut r, "admin1", "Password123!").await;

    // Baseline: just the admin's own login session.
    let base = active_count(&mut r, &token).await;
    assert_eq!(base, 1);

    // Two distinct users with a valid session each → +2.
    insert_session(
        &state,
        "alice",
        "sess-alice-1",
        Utc::now() + Duration::hours(1),
    )
    .await;
    insert_session(&state, "bob", "sess-bob-1", Utc::now() + Duration::hours(1)).await;
    assert_eq!(active_count(&mut r, &token).await, base + 2);

    // A second session for alice (another tab/device) → still counts once.
    insert_session(
        &state,
        "alice",
        "sess-alice-2",
        Utc::now() + Duration::hours(1),
    )
    .await;
    assert_eq!(active_count(&mut r, &token).await, base + 2);

    // An already-expired session for carol → not counted.
    insert_session(
        &state,
        "carol",
        "sess-carol-1",
        Utc::now() - Duration::hours(1),
    )
    .await;
    assert_eq!(active_count(&mut r, &token).await, base + 2);
}

// ---------------------------------------------------------------------------
// 5. widgets/update-info — auth + honest "no update known"
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_update_info_requires_auth_and_reports_no_false_update() {
    let (state, _tmp, _lock) = setup_state().await;
    create_test_user(&state, "widgetuser", "WidgetPass123", 1).await;
    let mut app = widgets_router(state);

    // Unauthenticated callers are rejected.
    let (status, _) = send(
        &mut app,
        Method::GET,
        "/api/widgets/update-info",
        None,
        None,
    )
    .await;
    assert!(status.is_client_error(), "expected 4xx, got {}", status);

    let token = login(&mut app, "widgetuser", "WidgetPass123").await;
    let (status, body) = send(
        &mut app,
        Method::GET,
        "/api/widgets/update-info",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    // With no UPDATE_CHECK_URL configured the widget must not invent an update.
    assert_eq!(data["has_app_update"], json!(false));
    assert!(data["app_latest_version"].is_null());
    assert!(!data["app_current_version"]
        .as_str()
        .unwrap_or("")
        .is_empty());
    // `has_server_update` was removed: it was the constant `false` in both
    // branches, i.e. a measurement that was never taken (ROADMAP 15.3), and
    // no client read it.
    assert!(
        data.get("has_server_update").is_none(),
        "the fabricated field must be gone: {data}"
    );
}
