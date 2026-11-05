/* =============================================================================
 * File          : openapi_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   OpenAPI integration tests: in-memory SQLite + assembled Axum router.
 *   Covers the admin boundary, the document shape, the public-route override,
 *   the sensitive-transport exclusions and deterministic serialization.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test openapi_integration
 * =============================================================================
 */

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
use homevault::handlers::openapi::handle_openapi_json;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::models::{ADMIN_ROLE_ID, USER_ROLE_ID};
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

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/openapi.json", get(handle_openapi_json))
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
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
    assert_eq!(resp.status(), StatusCode::OK, "login failed");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["data"]["token"].as_str().expect("token").to_string()
}

async fn get_openapi(router: &mut Router, token: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/api/openapi.json");
    if let Some(token) = token {
        req = req.header("x-session-token", token);
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
    (
        status,
        String::from_utf8(bytes.to_vec()).unwrap_or_default(),
    )
}

#[tokio::test]
async fn openapi_is_admin_only() {
    let harness = setup().await;
    create_user(&harness.state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_user(&harness.state, "member", "MemberPass123!", USER_ROLE_ID).await;
    let mut router = build_router(harness.state);

    let (status, _) = get_openapi(&mut router, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let member = login(&mut router, "member", "MemberPass123!").await;
    let (status, _) = get_openapi(&mut router, Some(&member)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = login(&mut router, "admin", "AdminPass123!").await;
    let (status, body) = get_openapi(&mut router, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"openapi\":\"3.0.3\""), "{body}");
}

#[tokio::test]
async fn openapi_describes_the_json_contract_and_hides_transports() {
    let harness = setup().await;
    create_user(&harness.state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = build_router(harness.state);
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, body) = get_openapi(&mut router, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let spec: Value = serde_json::from_str(&body).expect("OpenAPI JSON");

    assert_eq!(spec["openapi"], "3.0.3");
    assert_eq!(
        spec["components"]["securitySchemes"]["sessionToken"]["name"],
        "X-Session-Token"
    );
    assert_eq!(
        spec["components"]["securitySchemes"]["csrfToken"]["name"],
        "X-CSRF-Token"
    );

    let paths = spec["paths"].as_object().expect("paths");
    assert!(paths.contains_key("/api/meta"));
    assert!(paths.contains_key("/api/settings/general"));
    assert!(paths.contains_key("/api/openapi.json"));
    assert!(
        paths.contains_key("/api/network/interfaces"),
        "nested routers must be part of the contract"
    );

    // Authentication flows are public; the rest inherits the session scheme.
    assert_eq!(
        spec["paths"]["/api/auth/login"]["post"]["security"],
        json!([])
    );
    assert_eq!(
        spec["paths"]["/api/settings/general"]["get"]["x-homevault-auth"],
        "session"
    );

    // Non-JSON transports are excluded and named.
    for forbidden in [
        "/api/terminal/ws",
        "/api/vm/console/ws",
        "/api/explorer/upload",
        "/api/explorer/upload/session",
        "/api/explorer/download",
        "/api/notifications/events/ticket",
        "/api/docker/containers/{name}/logs/ticket",
    ] {
        assert!(!paths.contains_key(forbidden), "{forbidden} leaked");
    }
    assert!(spec["x-homevault-excluded"]
        .as_array()
        .expect("excluded list")
        .iter()
        .any(|entry| entry["path"] == "/api/terminal/ws"));

    // Two reads serialize identically: the document is a stable snapshot.
    let (_, again) = get_openapi(&mut router, Some(&admin)).await;
    assert_eq!(body, again);
}
