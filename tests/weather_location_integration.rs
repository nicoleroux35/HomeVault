// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Weather Location API — `crate::handlers::weather_location`:
//!
//!   GET  /api/weather-location  (any session)
//!   POST /api/weather-location  (any session)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server. Nothing here
//! touches the host.
//!
//! Run:
//!   cargo test --test weather_location_integration

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
use homevault::handlers::weather_location::{
    handle_get_weather_location, handle_set_weather_location,
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

fn weather_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/weather-location",
            get(handle_get_weather_location).post(handle_set_weather_location),
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

// ---------------------------------------------------------------------------
// 1. Auth guards — both endpoints require a session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_weather_location_requires_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = weather_router(state);

    let (status, _) = send(&mut r, Method::GET, "/api/weather-location", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/weather-location",
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_set_weather_location_requires_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = weather_router(state);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/weather-location",
        None,
        Some(json!({"location_json": "{}"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/weather-location",
        Some("nope"),
        Some(json!({"location_json": "{}"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. Round-trip: get returns null, set stores, get retrieves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_weather_location_null_until_saved() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = weather_router(state);
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/weather-location",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert!(
        body["data"]["location_json"].is_null(),
        "should be null before first save: {body}"
    );
}

#[tokio::test]
async fn test_weather_location_set_and_get_round_trip() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = weather_router(state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let location = r#"{"city":"Brussels","lat":50.85,"lon":4.35}"#;
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/weather-location",
        Some(&token),
        Some(json!({"location_json": location})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/weather-location",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["location_json"], location);
}

// ---------------------------------------------------------------------------
// 3. Upsert: second save overwrites the first
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_weather_location_upsert_overwrites() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut r = weather_router(state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    for city in ["Paris", "London", "Berlin"] {
        let blob = format!(r#"{{"city":"{}"}}"#, city);
        let (status, _) = send(
            &mut r,
            Method::POST,
            "/api/weather-location",
            Some(&token),
            Some(json!({"location_json": blob})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/weather-location",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["location_json"], r#"{"city":"Berlin"}"#);

    // Still only one row per user (upsert, not append).
    let rows: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM user_weather_location WHERE username = 'u'")
            .fetch_one(state.db.pool().as_ref())
            .await
            .unwrap();
    assert_eq!(rows.0, 1);
}

// ---------------------------------------------------------------------------
// 4. Per-user isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_weather_location_isolation_between_users() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut r = weather_router(state);
    let alice = login(&mut r, "alice", "Password123!").await;
    let bob = login(&mut r, "bob", "Password123!").await;

    send(
        &mut r,
        Method::POST,
        "/api/weather-location",
        Some(&alice),
        Some(json!({"location_json": r#"{"city":"Antwerp"}"#})),
    )
    .await;

    // Bob has no location yet.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/weather-location",
        Some(&bob),
        None,
    )
    .await;
    assert!(
        body["data"]["location_json"].is_null(),
        "bob must not see alice's location: {body}"
    );

    // Alice's location is hers.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/weather-location",
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(body["data"]["location_json"], r#"{"city":"Antwerp"}"#);
}
