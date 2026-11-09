/* =============================================================================
 * File          : docker_update_schedule_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026
 * Last modified : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the opt-in scheduled Docker/catalog updates (P1):
 *   admin gate, default off, validation, persistence and the manual-run gate.
 *
 * Run:
 *   cargo test --test docker_update_schedule_integration
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
use homevault::handlers::docker_update_schedule::{
    handle_get_schedule, handle_put_schedule, handle_run_now,
};
use homevault::handlers::terminal_pty::PtyManager;
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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/docker/updates/schedule",
            get(handle_get_schedule).put(handle_put_schedule),
        )
        .route("/api/docker/updates/run-now", post(handle_run_now))
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
    if let Some(token) = token {
        req = req.header("x-session-token", token);
    }
    let req = if let Some(body) = body {
        req.header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
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
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn test_schedule_is_admin_only_and_off_by_default() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "user1", "Password123!", 1).await;
    let mut router = router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;
    let user = login(&mut router, "user1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/docker/updates/schedule",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/docker/updates/schedule",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/docker/updates/schedule",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], json!(false));
    assert_eq!(body["data"]["interval_hours"], json!(24));
    assert!(body["data"]["docker_available"].is_boolean());
}

#[tokio::test]
async fn test_schedule_persists_and_validates_the_interval() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/docker/updates/schedule",
        Some(&admin),
        Some(json!({"enabled": true, "interval_hours": 12})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], json!(true));
    assert_eq!(body["data"]["interval_hours"], json!(12));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/docker/updates/schedule",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["enabled"], json!(true));
    assert_eq!(body["data"]["interval_hours"], json!(12));

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/docker/updates/schedule",
        Some(&admin),
        Some(json!({"enabled": true, "interval_hours": 5})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/docker/updates/schedule",
        Some(&admin),
        Some(json!({"enabled": true, "interval_hours": 169})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_run_now_is_admin_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "user1", "Password123!", 1).await;
    let mut router = router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;
    let user = login(&mut router, "user1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/docker/updates/run-now",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/docker/updates/run-now",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The admin call is not asserted: on a host with Docker and installed
    // catalog apps it would actually pull images and restart containers.
    let _ = admin;
}
