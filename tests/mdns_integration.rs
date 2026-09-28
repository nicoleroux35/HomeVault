//! Integration tests for `GET /api/mdns/status` (`crate::handlers::mdns`).
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt::oneshot`. Avahi is not required — the status handler
//! reports `available: false` on a host without it and still returns the
//! canonical `homevault.local` address.
//!
//! Run: cargo test --test mdns_integration

use axum::{routing::{get, post}, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, mdns::handle_mdns_status, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
type Req = Request<axum::body::Body>;

async fn setup_state() -> (Arc<AppState>, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());
    std::env::remove_var("PORT"); // status URL must fall back to the 8080 default

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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/mdns/status", get(handle_mdns_status))
        .with_state(state)
}

async fn mk_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'T', 't@e.com', ?, false, datetime('now'), 'seed')",
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
    let req = if let Some(b) = body {
        req.header("content-type", "application/json")
            .body(axum::body::Body::from(b.to_string()))
            .unwrap()
    } else {
        req.body(axum::body::Body::empty()).unwrap()
    };
    let resp = ServiceExt::<Req>::ready(router).await.unwrap().call(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
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

#[tokio::test]
async fn test_mdns_status_requires_a_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/mdns/status", None, None).await;
    assert_eq!(body["success"], json!(false), "no token must be rejected");
}

#[tokio::test]
async fn test_mdns_status_rejects_bad_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);
    let (status, _) =
        send(&mut r, Method::GET, "/api/mdns/status", Some("not-a-real-token"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_mdns_status_returns_canonical_address_for_any_user() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "u", "pw012345", 1).await; // a plain user is enough
    let mut r = router(state);
    let token = login(&mut r, "u", "pw012345").await;

    let (status, body) = send(&mut r, Method::GET, "/api/mdns/status", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let d = &body["data"];
    assert_eq!(d["host_name"], json!("homevault"));
    assert_eq!(d["fqdn"], json!("homevault.local"));
    assert_eq!(d["http_url"], json!("http://homevault.local:8080"));
    let services = d["services"].as_array().expect("services array");
    assert!(services.iter().any(|s| s == "_http._tcp"), "web UI is always advertised");
    // `available`/`running` are host-dependent (Avahi may or may not be
    // installed in CI) — just assert they are booleans and consistent.
    assert!(d["available"].is_boolean());
    assert!(d["running"].is_boolean());
    if !d["available"].as_bool().unwrap() {
        assert_eq!(d["running"], json!(false));
    }
}
