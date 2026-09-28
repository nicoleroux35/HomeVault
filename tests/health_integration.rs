//! Integration tests for the Health page's API — `GET /api/health`
//! (`homevault::web::handle_health_endpoint`).
//!
//! The Health screen (`frontend/src/routes/Health.svelte`) has exactly one
//! backend call: `api.healthCheck()` → `GET /api/health`. The endpoint runs
//! the 8 built-in checks from `src/health.rs` (`df`, `/proc/*`, `lsblk` +
//! `smartctl`, `systemctl`) and returns an aggregate. It reveals real host
//! detail (disk names / SMART verdict, service state, memory, thermals) —
//! same sensitivity class as the Dashboard endpoints — so it requires a
//! valid session (any role; chatting-style "read" access, not admin).
//!
//! This handler always answers **HTTP 200** and signals auth failure in the
//! body (`{"success": false}`), unlike the `require_auth` handlers that
//! return 400/401 — the assertions below check the body, not the status.
//!
//! Everything here is safe to run for real: the checks only inventory the
//! host, nothing is mutated, no `sudo`.
//!
//! Fresh in-memory SQLite + all migrations per test, a one-route Axum
//! router, `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! Run:
//!   cargo test --test health_integration

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};
use homevault::web::handle_health_endpoint;

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// The 8 checks `HealthService::run_all_checks` always returns, in order.
const EXPECTED_CHECKS: [&str; 8] = [
    "authentication",
    "storage",
    "memory",
    "disk_space",
    "disk_smart",
    "network",
    "services",
    "temperatures",
];

const VALID_STATUSES: [&str; 4] = ["healthy", "warning", "unhealthy", "unknown"];

async fn setup_state() -> (Arc<AppState>, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
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

fn health_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/health", get(handle_health_endpoint))
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

/// Re-implements `HealthService::determine_overall_status`'s precedence from
/// the per-check statuses in the response, to assert the aggregate is
/// consistent with what it aggregates.
fn expected_overall(statuses: &[&str]) -> &'static str {
    if statuses.iter().any(|s| *s == "unhealthy") {
        "unhealthy"
    } else if statuses.iter().any(|s| *s == "warning") {
        "warning"
    } else if !statuses.is_empty() && statuses.iter().all(|s| *s == "healthy") {
        "healthy"
    } else {
        "unknown"
    }
}

// ---------------------------------------------------------------------------
// 1. Auth guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_rejects_a_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = health_router(state);
    let (status, body) = send(&mut router, Method::GET, "/api/health", None, None).await;
    // The handler answers 200 and signals the failure in the body.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(false));
    assert!(body.get("data").is_none() || body["data"].is_null());
}

#[tokio::test]
async fn test_health_rejects_a_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = health_router(state);
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/health",
        Some("not-a-real-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_health_allows_any_valid_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await; // role 1 = non-admin
    let mut router = health_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    let (status, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true), "body: {body}");
    assert!(body["data"]["checks"].is_array());
}

// ---------------------------------------------------------------------------
// 2. Payload shape — the fields Health.svelte reads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_returns_the_eight_named_checks() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (_, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    let checks = body["data"]["checks"].as_array().expect("checks array");
    assert_eq!(checks.len(), 8, "expected 8 checks: {body}");

    let names: BTreeSet<&str> = checks.iter().filter_map(|c| c["name"].as_str()).collect();
    let expected: BTreeSet<&str> = EXPECTED_CHECKS.into_iter().collect();
    assert_eq!(names, expected);

    for c in checks {
        assert!(
            VALID_STATUSES.contains(&c["status"].as_str().unwrap_or("")),
            "unexpected check status: {c}"
        );
        assert!(
            !c["message"].as_str().unwrap_or("").trim().is_empty(),
            "empty message: {c}"
        );
        // `details` is always present (null for some checks) — the key exists.
        assert!(c.get("details").is_some());
    }
}

#[tokio::test]
async fn test_health_overall_status_is_a_valid_lowercase_string_mirrored_in_status() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (_, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    let overall = body["data"]["overall_status"].as_str().unwrap();
    assert!(VALID_STATUSES.contains(&overall), "overall: {overall}");
    // Health.svelte reads `overall_status`; the desktop monitor reads
    // `status` — the handler must keep sending both, identical.
    assert_eq!(body["data"]["status"].as_str().unwrap(), overall);
}

#[tokio::test]
async fn test_health_statuses_are_never_pascal_case() {
    // `HealthStatus` serialises as "Healthy"/"Warning"/… — the handler maps
    // every one to lowercase by hand. A regression there would leak the raw
    // enum name and the frontend `CheckStatus` union would silently stop
    // matching.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (_, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    let raw = body.to_string();
    for pascal in ["\"Healthy\"", "\"Warning\"", "\"Unhealthy\"", "\"Unknown\""] {
        assert!(!raw.contains(pascal), "leaked pascal-case status: {pascal}");
    }
}

#[tokio::test]
async fn test_health_aggregate_is_consistent_with_its_checks() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (_, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    let statuses: Vec<&str> = body["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["status"].as_str())
        .collect();
    assert_eq!(
        body["data"]["overall_status"].as_str().unwrap(),
        expected_overall(&statuses),
        "aggregate {} disagrees with checks {statuses:?}",
        body["data"]["overall_status"]
    );
}

#[tokio::test]
async fn test_health_payload_also_carries_cpu_and_memory_usage() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    let (_, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    let cpu = body["data"]["cpu_usage"].as_f64().expect("cpu_usage");
    let mem = body["data"]["memory_usage"].as_f64().expect("memory_usage");
    assert!((0.0..=100.0).contains(&cpu), "cpu_usage {cpu}");
    assert!((0.0..=100.0).contains(&mem), "memory_usage {mem}");
    assert!(body["data"]["timestamp"].as_str().is_some());
}

// ---------------------------------------------------------------------------
// 3. Security / robustness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_never_500s_regardless_of_auth() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "Password123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "Password123!").await;

    for tok in [None, Some("garbage"), Some(token.as_str())] {
        let (status, _) = send(&mut router, Method::GET, "/api/health", tok, None).await;
        assert_eq!(status, StatusCode::OK, "token = {tok:?}");
    }
}

#[tokio::test]
async fn test_health_response_leaks_no_credential() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "u", "SuperSecret123!", 1).await;
    let mut router = health_router(state);
    let token = login(&mut router, "u", "SuperSecret123!").await;

    let (_, body) = send(&mut router, Method::GET, "/api/health", Some(&token), None).await;
    let raw = body.to_string();
    assert!(!raw.contains("SuperSecret123!"));
    assert!(!raw.contains(&token));
    // No password hash / session material echoed back.
    assert!(!raw.contains("$argon2"));
}
