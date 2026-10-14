/* =============================================================================
 * File          : metrics_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Metrics-history integration tests: in-memory SQLite + assembled Axum
 *   router, exercising the auth guards, range validation and bucketed
 *   reads over seeded samples via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the metrics API:
//!
//!   - GET /api/metrics/history?range=1h|24h|7d
//!   - GET /api/metrics/latest

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::metrics::{handle_metrics_history, handle_metrics_latest};
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

fn metrics_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/auth/login",
            axum::routing::post(homevault::handlers::auth::handle_login),
        )
        .route("/api/metrics/history", get(handle_metrics_history))
        .route("/api/metrics/latest", get(handle_metrics_latest))
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
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    let req = req.body(axum::body::Body::empty()).unwrap();
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

/// Seeds samples at fixed unix timestamps (minutes ago from now).
async fn seed_samples(state: &AppState, minutes_ago_list: &[i64]) {
    let now = chrono::Utc::now().timestamp();
    for &m in minutes_ago_list {
        let ts = now - m * 60;
        sqlx::query(
            "INSERT OR REPLACE INTO metric_samples (ts, cpu, mem, load1, net_rx, net_tx, temp) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(ts)
        .bind(10.0)
        .bind(50.0)
        .bind(0.5)
        .bind(1000.0)
        .bind(2000.0)
        .bind(45.0)
        .execute(&*state.db.pool())
        .await
        .expect("seed sample");
    }
}

#[tokio::test]
async fn history_requires_auth_and_rejects_bad_ranges() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = metrics_router(state);

    let (status, _) = send(&mut router, Method::GET, "/api/metrics/history", None).await;
    assert_ne!(status, StatusCode::OK, "anonymous history must not pass");
    let (status, _) = send(&mut router, Method::GET, "/api/metrics/latest", None).await;
    assert_ne!(status, StatusCode::OK, "anonymous latest must not pass");

    let admin = login(&mut router, "admin", "AdminPass123!").await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/metrics/history?range=90d",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap().contains("Invalid range"),
        "{body}"
    );
}

#[tokio::test]
async fn non_admin_user_can_read_metrics() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = metrics_router(state);
    let user = login(&mut router, "user", "UserPass123!").await;
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/metrics/history?range=1h",
        Some(&user),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "metrics are auth-level, not admin-level"
    );
}

#[tokio::test]
async fn empty_database_yields_all_null_points() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = metrics_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    for range in ["1h", "24h", "7d"] {
        let (status, body) = send(
            &mut router,
            Method::GET,
            &format!("/api/metrics/history?range={range}"),
            Some(&admin),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let points = body["data"]["points"].as_array().expect("points array");
        assert!(!points.is_empty());
        assert!(
            points.iter().all(|p| p["cpu"].is_null()),
            "empty DB → gap points ({range})"
        );
    }

    // latest on an empty DB is a clean error, not a panic.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/metrics/latest",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn seeded_samples_come_back_bucketed() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    // Two samples in the same 1-minute... no — 60 and 120 minutes ago are
    // distinct 1h buckets; seed 3 samples within the last 30 minutes plus
    // one 5 hours old (outside the 1h window).
    seed_samples(&state, &[5, 10, 20, 300]).await;
    let mut router = metrics_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/metrics/history?range=1h",
        Some(&admin),
    )
    .await;
    let points = body["data"]["points"].as_array().expect("points");
    assert_eq!(body["data"]["range"], "1h");
    assert_eq!(body["data"]["bucket_seconds"], 60);
    // Samples are ≥ 5 min apart, so each lands in its own 1-minute
    // bucket; the 5-hour-old sample and the in-progress bucket stay empty.
    let non_null = points.iter().filter(|p| !p["cpu"].is_null()).count();
    assert_eq!(non_null, 3, "expected 3 in-window samples, got {non_null}");
    assert_eq!(
        points.last().unwrap()["cpu"],
        json!(null),
        "in-progress bucket is empty"
    );
    let first = points.iter().find(|p| !p["cpu"].is_null()).unwrap();
    assert_eq!(first["cpu"], json!(10.0));
    assert_eq!(first["temp"], json!(45.0));

    // 7d range: all 4 samples are in-window. How many 2-hour buckets they
    // occupy depends on where the wall clock sits (a bucket boundary may
    // fall between the three recent samples), so assert the merge-safe
    // invariants instead of a brittle count: equal inputs average to
    // themselves, and at most 3 buckets can be non-empty (the three recent
    // samples span 15 min < one bucket; the old one is alone).
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/metrics/history?range=7d",
        Some(&admin),
    )
    .await;
    let points = body["data"]["points"].as_array().unwrap();
    assert_eq!(body["data"]["bucket_seconds"], 7200);
    let non_null: Vec<&Value> = points.iter().filter(|p| !p["cpu"].is_null()).collect();
    assert!(
        (2..=3).contains(&non_null.len()),
        "2-4 samples occupy 2-3 two-hour buckets, got {}",
        non_null.len()
    );
    assert!(
        non_null.iter().all(|p| p["cpu"] == json!(10.0)),
        "averages preserved"
    );
}

#[tokio::test]
async fn latest_returns_the_newest_sample() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    seed_samples(&state, &[10, 2]).await;
    let mut router = metrics_router(state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/metrics/latest",
        Some(&admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["cpu"], json!(10.0));
    assert_eq!(body["data"]["mem"], json!(50.0));
}
