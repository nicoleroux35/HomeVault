//! Integration tests for the built-in HTTPS/TLS settings endpoints —
//! `crate::handlers::tls`:
//!
//!   GET  /api/settings/tls            (admin only)
//!   PUT  /api/settings/tls            (admin only)
//!   POST /api/settings/tls/regenerate (admin only)
//!
//! Fresh in-memory SQLite + all migrations per test, a throwaway
//! `tempfile::tempdir()` wired in as `DATA_DIRECTORY`, a minimal Axum router
//! driven with `tower::ServiceExt::oneshot` — no real server, no real TLS
//! handshake (that is covered by the live check in the plan's verification
//! section).
//!
//! `DATA_DIRECTORY` is process-global, so every test serialises behind
//! `ENV_LOCK`.
//!
//! Run:
//!   cargo test --test tls_integration

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
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::tls::{
    handle_tls_regenerate, handle_tls_settings_get, handle_tls_settings_update,
};
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn setup() -> (Router, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());
    std::env::remove_var("PORT"); // http_port() -> default 8080

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

    let state = Arc::new(AppState {
        auth,
        widgets,
        db,
        path_guard,
        admin_rate_limiter,
        pty_manager,
        downloader,
        ransomware_watch_tx,
    });

    create_user(&state, "admin1", "Password123!", 2).await;
    create_user(&state, "regular", "Password123!", 1).await;

    let router = Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/settings/tls",
            get(handle_tls_settings_get).put(handle_tls_settings_update),
        )
        .route("/api/settings/tls/regenerate", post(handle_tls_regenerate))
        .with_state(state);

    (router, tmp, guard)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&hash)
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
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(axum::body::Body::from(b.to_string()))
            .unwrap(),
        None => req.body(axum::body::Body::empty()).unwrap(),
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
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn login(router: &mut Router, username: &str) -> String {
    let (_, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": "Password123!"})),
    )
    .await;
    body["data"]["token"].as_str().expect("token").to_string()
}

// ---------------------------------------------------------------------------
// Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_requires_admin() {
    let (mut r, _tmp, _g) = setup().await;

    let (s, _) = send(&mut r, Method::GET, "/api/settings/tls", None, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST); // no token -> json_error

    let (s, _) = send(
        &mut r,
        Method::GET,
        "/api/settings/tls",
        Some("bogus-token"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let regular = login(&mut r, "regular").await;
    let (s, _) = send(
        &mut r,
        Method::GET,
        "/api/settings/tls",
        Some(&regular),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let admin = login(&mut r, "admin1").await;
    let (s, body) = send(&mut r, Method::GET, "/api/settings/tls", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["mode"], "self-signed");
    assert_eq!(body["data"]["effective_https_port"], 8443);
    assert!(body["data"]["cert_info"].is_object());
}

#[tokio::test]
async fn test_put_and_regenerate_require_admin() {
    let (mut r, _tmp, _g) = setup().await;
    let regular = login(&mut r, "regular").await;

    let (s, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/tls",
        Some(&regular),
        Some(json!({"enabled": false, "mode": "self-signed", "redirect": false})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = send(
        &mut r,
        Method::POST,
        "/api/settings/tls/regenerate",
        Some(&regular),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_put_rejects_https_port_equal_to_http_port() {
    let (mut r, _tmp, _g) = setup().await;
    let admin = login(&mut r, "admin1").await;

    let (s, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/tls",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "https_port": 8080,
            "mode": "self-signed",
            "redirect": false
        })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn test_put_manual_mode_requires_readable_files() {
    let (mut r, _tmp, _g) = setup().await;
    let admin = login(&mut r, "admin1").await;

    // Missing key path.
    let (s, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/tls",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "https_port": 8443,
            "mode": "manual",
            "cert_path": "/etc/ssl/whatever.pem",
            "redirect": false
        })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Traversal in the path.
    let (s, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/tls",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "https_port": 8443,
            "mode": "manual",
            "cert_path": "/etc/ssl/../shadow",
            "key_path": "/etc/ssl/../shadow",
            "redirect": false
        })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_put_self_signed_persists_and_reports_restart() {
    let (mut r, _tmp, _g) = setup().await;
    let admin = login(&mut r, "admin1").await;

    let (s, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/tls",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "https_port": 8443,
            "mode": "self-signed",
            "redirect": true
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"]["requires_restart"], true);

    // GET reflects the saved state.
    let (_, body) = send(&mut r, Method::GET, "/api/settings/tls", Some(&admin), None).await;
    assert_eq!(body["data"]["enabled"], true);
    assert_eq!(body["data"]["effective_https_port"], 8443);
    assert_eq!(body["data"]["redirect"], true);
}

#[tokio::test]
async fn test_regenerate_creates_a_self_signed_cert_on_disk() {
    let (mut r, tmp, _g) = setup().await;
    let admin = login(&mut r, "admin1").await;

    let (s, body) = send(
        &mut r,
        Method::POST,
        "/api/settings/tls/regenerate",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["data"]["cert_info"].is_object());
    assert_eq!(body["data"]["cert_info"]["self_signed"], true);

    assert!(tmp.path().join("tls/self-signed.crt").exists());
    assert!(tmp.path().join("tls/self-signed.key").exists());
}

#[tokio::test]
async fn test_manual_mode_accepts_a_real_pem_pair() {
    let (mut r, tmp, _g) = setup().await;
    let admin = login(&mut r, "admin1").await;

    // Produce a real cert/key pair to point "manual" mode at.
    homevault::tls::generate_self_signed().expect("generate pem pair");
    let cert = tmp.path().join("tls/self-signed.crt");
    let key = tmp.path().join("tls/self-signed.key");

    let (s, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/tls",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "https_port": 9443,
            "mode": "manual",
            "cert_path": cert.to_string_lossy(),
            "key_path": key.to_string_lossy(),
            "redirect": false
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {body}");
    assert_eq!(body["data"]["requires_restart"], true);
}

// ---------------------------------------------------------------------------
// No key material ever leaves the process
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_responses_never_contain_private_key_material() {
    let (mut r, _tmp, _g) = setup().await;
    let admin = login(&mut r, "admin1").await;

    let (_, regen) = send(
        &mut r,
        Method::POST,
        "/api/settings/tls/regenerate",
        Some(&admin),
        None,
    )
    .await;
    let (_, get) = send(&mut r, Method::GET, "/api/settings/tls", Some(&admin), None).await;

    for body in [regen, get] {
        let s = body.to_string();
        assert!(!s.contains("PRIVATE KEY"), "response leaked key material: {s}");
        assert!(!s.contains("BEGIN EC"), "response leaked key material: {s}");
    }
}
