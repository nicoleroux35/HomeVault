//! Integration tests for the Network API endpoints (`crate::handlers::network`):
//!
//!   - GET  /api/network/interfaces
//!   - GET  /api/network/interfaces/{name}
//!   - GET  /api/network/interfaces/{name}/config
//!   - PUT  /api/network/interfaces/{name}/config
//!   - PUT  /api/network/interfaces/{name}/state
//!   - POST /api/network/wifi/scan
//!   - GET  /api/network/wifi/status
//!   - POST /api/network/wifi/connect
//!   - POST /api/network/wifi/disconnect
//!   - GET  /api/network/diagnostics
//!   - GET  /api/network/summary
//!   - GET  /api/network/services
//!   - GET  /api/network/public-ip
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! **This suite never applies a real config change or state flip to any
//! real network interface.** Doing so for real could disconnect the host
//! running the test (its actual active interface) or — on a real
//! deployment where sudoers grants the necessary privilege — genuinely take
//! the machine off the network. Per CLAUDE.md's "What NOT to test in Rust",
//! the mutating endpoints (`config` PUT, `state` PUT, wifi connect/
//! disconnect) are only exercised far enough to confirm they fail *cleanly*
//! (400) against invalid input or a nonexistent interface name — a genuine
//! reconfiguration against disposable hardware is `tests/e2e/network/
//! network.spec.ts`'s job, and even there it stays validation-only (see
//! that file's own header). The read-only endpoints (interfaces list/get/
//! config-get, wifi status/scan, diagnostics, summary, services, public-ip)
//! are safe to run for real — they never modify anything.
//!
//! Run:
//!   cargo test --test network_integration
//!   cargo test --test network_integration -- --nocapture

use axum::{routing::get, routing::post, routing::put, Router};
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
    network::{
        handle_network_diagnostics, handle_network_interface_config,
        handle_network_interface_config_update, handle_network_interface_get,
        handle_network_interface_state, handle_network_interfaces, handle_network_services,
        handle_network_summary, handle_network_wifi_connect, handle_network_wifi_disconnect,
        handle_network_wifi_scan, handle_network_wifi_status, handle_public_ip,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

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

fn network_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/network/interfaces", get(handle_network_interfaces))
        .route(
            "/api/network/interfaces/{name}",
            get(handle_network_interface_get),
        )
        .route(
            "/api/network/interfaces/{name}/config",
            get(handle_network_interface_config),
        )
        .route(
            "/api/network/interfaces/{name}/config",
            put(handle_network_interface_config_update),
        )
        .route(
            "/api/network/interfaces/{name}/state",
            put(handle_network_interface_state),
        )
        .route("/api/network/wifi/scan", post(handle_network_wifi_scan))
        .route("/api/network/wifi/status", get(handle_network_wifi_status))
        .route(
            "/api/network/wifi/connect",
            post(handle_network_wifi_connect),
        )
        .route(
            "/api/network/wifi/disconnect",
            post(handle_network_wifi_disconnect),
        )
        .route("/api/network/diagnostics", get(handle_network_diagnostics))
        .route("/api/network/summary", get(handle_network_summary))
        .route("/api/network/services", get(handle_network_services))
        .route("/api/network/public-ip", get(handle_public_ip))
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
    .expect("Failed to insert test user");
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

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_interfaces_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/interfaces",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_wifi_status_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/wifi/status",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_interface_config_update_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/eth0/config",
        None,
        Some(json!({"method": "dhcp"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_interface_state_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/eth0/state",
        None,
        Some(json!({"up": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_wifi_connect_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/connect",
        None,
        Some(json!({"ssid": "SomeNetwork", "password": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_wifi_disconnect_no_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/disconnect",
        None,
        Some(json!({"ssid": "SomeNetwork"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_network_endpoints_reject_invalid_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = network_router(state);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/interfaces",
        Some("totally-bogus-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_read_endpoints_allow_any_valid_session_not_just_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regularuser", "Password123!", 1).await;
    let mut router = network_router(state);
    let token = login(&mut router, "regularuser", "Password123!").await;

    for (method, uri) in [
        (Method::GET, "/api/network/interfaces"),
        (Method::GET, "/api/network/wifi/status"),
        (Method::GET, "/api/network/summary"),
        (Method::GET, "/api/network/services"),
    ] {
        let (status, _) = send(&mut router, method, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "GET {} should allow a non-admin session", uri);
    }
}

#[tokio::test]
async fn test_mutating_endpoints_reject_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regularuser", "Password123!", 1).await;
    let mut router = network_router(state);
    let token = login(&mut router, "regularuser", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/eth0/config",
        Some(&token),
        Some(json!({"method": "dhcp"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/eth0/state",
        Some(&token),
        Some(json!({"up": true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/connect",
        Some(&token),
        Some(json!({"ssid": "x", "password": "y"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/disconnect",
        Some(&token),
        Some(json!({"ssid": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 2. Validation — clean 400s, never touching a real interface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_interface_config_update_rejects_invalid_method() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/definitely-not-a-real-interface/config",
        Some(&token),
        Some(json!({"method": "not-a-real-method"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("Invalid method"));
}

#[tokio::test]
async fn test_interface_config_update_rejects_nonexistent_interface_cleanly() {
    // 2026-09-05 hardening: apply_interface_config now validates `name`
    // against the real interface list before doing anything — this is the
    // regression test for that check (previously, a bogus interface name
    // reached the IPv4/IPv6 config logic with no gate at all).
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/definitely-not-a-real-interface/config",
        Some(&token),
        Some(json!({"method": "dhcp"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Unknown network interface"));
}

#[tokio::test]
async fn test_interface_config_update_rejects_path_traversal_interface_name_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/..%2F..%2F..%2Ftmp%2Fpwned/config",
        Some(&token),
        Some(json!({"method": "dhcp", "ipv6_method": "disabled"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_interface_state_rejects_missing_up_field() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/network/interfaces/definitely-not-a-real-interface/state",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("up"));
}

#[tokio::test]
async fn test_wifi_connect_rejects_missing_ssid() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/connect",
        Some(&token),
        Some(json!({"password": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("ssid"));
}

#[tokio::test]
async fn test_wifi_disconnect_rejects_missing_ssid() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/disconnect",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("ssid"));
}

// ---------------------------------------------------------------------------
// 3. Read-only success paths — safe to run for real
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_interfaces_list_returns_an_array() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/network/interfaces",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["interfaces"].is_array());
}

#[tokio::test]
async fn test_loopback_interface_get_and_config_are_readable() {
    // "lo" always exists on any Linux host — safe to GET (read-only), but
    // deliberately never PUT against it anywhere in this suite.
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/network/interfaces/lo",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["interface"]["name"], json!("lo"));

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/interfaces/lo/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_wifi_status_returns_ok_even_with_no_wifi_hardware() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/wifi/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_wifi_scan_returns_ok_even_with_no_wifi_hardware() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/wifi/scan",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["networks"].is_array());
}

#[tokio::test]
async fn test_diagnostics_returns_ok() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/diagnostics",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_summary_returns_ok() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/summary",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_services_returns_ok() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/services",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_public_ip_returns_ok_regardless_of_resolution() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = network_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/public-ip",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
