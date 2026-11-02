/* =============================================================================
 * File          : traffic_control_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Bandwidth-control integration tests: the auth matrix, the payload shape,
 *   every validation path and the persistence round-trip through the API.
 *
 *   No test names a real interface: `apply` shells out to `sudo tc` once the
 *   interface is set, and deleting a root qdisc on the test host would be an
 *   unacceptable side effect. Keeping the interface empty exercises the whole
 *   HTTP surface while `apply_sync` returns "No interface is selected" before
 *   it can touch the machine.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test traffic_control_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::traffic_control::{
    handle_traffic_control_apply, handle_traffic_control_get, handle_traffic_control_pause,
    handle_traffic_control_put,
};
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    let path_guard = PathGuard::new(vec![root.to_string_lossy().to_string()]);
    let admin_rate_limiter = Arc::new(RateLimitState::new(100, 60));
    let pty_manager = Arc::new(PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    Harness {
        state: Arc::new(AppState {
            auth,
            widgets,
            db,
            path_guard,
            admin_rate_limiter,
            pty_manager,
            downloader,
            ransomware_watch_tx,
        }),
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/network/traffic-control",
            get(handle_traffic_control_get).put(handle_traffic_control_put),
        )
        .route(
            "/api/network/traffic-control/pause",
            post(handle_traffic_control_pause),
        )
        .route(
            "/api/network/traffic-control/apply",
            post(handle_traffic_control_apply),
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
        .unwrap()
        .to_vec();
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

/// A policy that validates but can never touch the host: no interface, so
/// `apply_sync` stops at "No interface is selected".
fn off_policy() -> Value {
    json!({
        "enabled": false,
        "interface": "",
        "paused": false,
        "services": [
            {"service": "smb", "rate_kbps": 0},
            {"service": "nfs", "rate_kbps": 0}
        ],
        "schedule": []
    })
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reads_need_a_session_and_mutations_need_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/network/traffic-control",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/network/traffic-control",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/network/traffic-control".to_string(),
            Some(off_policy()),
        ),
        (
            Method::POST,
            "/api/network/traffic-control/pause".to_string(),
            Some(json!({"paused": true})),
        ),
        (
            Method::POST,
            "/api/network/traffic-control/apply".to_string(),
            None,
        ),
    ] {
        let (status, _) = send(&mut router, method.clone(), &uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, &uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_returns_settings_capabilities_and_state() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/network/traffic-control",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let data = &body["data"];
    assert_eq!(data["settings"]["enabled"], json!(false));
    assert_eq!(data["settings"]["interface"], json!(""));
    assert!(data["interfaces"].is_array());
    assert_eq!(
        data["capabilities"]["services"]
            .as_array()
            .expect("service capabilities")
            .len(),
        5
    );
    for cap in data["capabilities"]["services"].as_array().unwrap() {
        assert!(cap["ports"].is_array());
        // The honest claim: egress only.
        assert_eq!(cap["supports_download_limit"], json!(false));
    }
    assert_eq!(
        data["capabilities"]["management_ports"],
        json!([22, 80, 443, 8443])
    );
    // Traffic control is off by default (nothing is saved yet).
    assert_eq!(data["state"]["shaping"], json!(false));
    assert_eq!(data["state"]["reason_code"], json!("off"));
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_validation_refuses_unsafe_or_incoherent_policies() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let cases = [
        (
            "loopback",
            json!({
                "enabled": true,
                "interface": "lo",
                "services": [{"service": "smb", "rate_kbps": 100}],
            }),
        ),
        (
            "shell injection in an interface name",
            json!({
                "enabled": true,
                "interface": "eth0; reboot",
                "services": [{"service": "smb", "rate_kbps": 100}],
            }),
        ),
        (
            "unknown service",
            json!({
                "enabled": true,
                "interface": "eth0",
                "services": [{"service": "telnet", "rate_kbps": 100}],
            }),
        ),
        (
            "duplicate service",
            json!({
                "enabled": true,
                "interface": "eth0",
                "services": [
                    {"service": "smb", "rate_kbps": 100},
                    {"service": "smb", "rate_kbps": 200}
                ],
            }),
        ),
        (
            "negative rate",
            json!({
                "enabled": false,
                "interface": "",
                "services": [{"service": "smb", "rate_kbps": -5}],
            }),
        ),
        (
            "window without a day",
            json!({
                "enabled": false,
                "interface": "",
                "schedule": [{"days": [], "start_minute": 0, "end_minute": 60}],
            }),
        ),
        (
            "window with a bad time",
            json!({
                "enabled": false,
                "interface": "",
                "schedule": [{"days": [0], "start_minute": 1440, "end_minute": 60}],
            }),
        ),
    ];

    for (label, payload) in cases {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/network/traffic-control",
            Some(&admin),
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert_eq!(body["success"], json!(false), "{label}");
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{label} must explain itself: {body}"
        );
    }
}

#[tokio::test]
async fn test_pause_requires_a_boolean() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/traffic-control/pause",
        Some(&admin),
        Some(json!({"paused": "yes"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_policy_round_trips_through_save_and_reload() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let payload = json!({
        "enabled": false,
        "interface": "",
        "paused": false,
        "services": [
            {"service": "smb", "rate_kbps": 20000},
            {"service": "nfs", "rate_kbps": 0}
        ],
        "schedule": [
            {"days": [0, 1, 2, 3, 4], "start_minute": 540, "end_minute": 1020}
        ]
    });

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/network/traffic-control",
        Some(&admin),
        Some(payload),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["settings"]["services"][0]["rate_kbps"],
        json!(20000)
    );
    // The apply report travelled with the save and stopped before any command.
    assert_eq!(body["data"]["apply"]["applied"], json!(false));
    assert_eq!(
        body["data"]["apply"]["commands"],
        json!([]),
        "an empty interface must never produce a tc command"
    );

    // A fresh GET reads the saved JSON back from disk.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/network/traffic-control",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["settings"]["services"][0]["rate_kbps"],
        json!(20000)
    );
    assert_eq!(
        body["data"]["settings"]["schedule"][0]["start_minute"],
        json!(540)
    );

    let path = h._tmp.path().join("data").join("traffic_control.json");
    assert!(path.exists(), "the policy must be persisted at {path:?}");
}

#[tokio::test]
async fn test_pause_and_resume_toggle_the_saved_policy() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // Save a policy first so pause has something to keep.
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/network/traffic-control",
        Some(&admin),
        Some(off_policy()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/traffic-control/pause",
        Some(&admin),
        Some(json!({"paused": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["paused"], json!(true));

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/traffic-control/pause",
        Some(&admin),
        Some(json!({"paused": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["paused"], json!(false));

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/network/traffic-control/apply",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["apply"]["applied"], json!(false));
}
