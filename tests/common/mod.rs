/* =============================================================================
 * File          : common/mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 2026-11-18
 * Version       : 1.0.0
 *
 * Description:
 *   Common test utilities for integration tests. Provides helper functions
 *   to create test applications with in-memory databases and admin users.
 *
 * Usage:
 *   mod common;
 *   use common::*;
 * =============================================================================
 */

// Shared by several integration binaries (`mod common;`); each one only uses a
// subset of these helpers, so the unused ones must not trip `-D warnings`.
#![allow(dead_code)]

use axum::{routing::get, Router};
use std::sync::Arc;
use tempfile::TempDir;

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

/// Test database wrapper
pub struct TestDb {
    pub pool: sqlx::SqlitePool,
    pub database: Database,
}

/// Create an in-memory SQLite database with migrations
pub async fn setup_test_db() -> Database {
    let db = Database::new("sqlite::memory:")
        .await
        .expect("Failed to create in-memory database");
    db.run_migrations().await.expect("Failed to run migrations");
    db
}

/// Create a test AppState with all required components
pub async fn setup_test_state() -> (Arc<AppState>, TempDir) {
    let tmp = tempfile::tempdir().expect("Failed to create temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = setup_test_db().await;
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

    (state, tmp)
}

/// Create a test user directly in the database
pub async fn create_test_user(
    state: &AppState,
    username: &str,
    password: &str,
    role_id: i64,
) -> String {
    let password_hash = state.auth.hash_password(password).unwrap();
    let pool = state.db.pool();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, ?, ?, ?, false, datetime('now'), ?)",
    )
    .bind(username)
    .bind(&password_hash)
    .bind(format!("Test User {}", username))
    .bind(format!("{}@example.com", username))
    .bind(role_id)
    .bind("testseed")
    .execute(pool.as_ref())
    .await
    .expect("Failed to create test user");

    username.to_string()
}

/// Create an admin user and return a session token
pub async fn create_admin_with_token(state: &AppState) -> String {
    let username = "testadmin";
    let password = "testpass123";

    // Create admin user (role_id = 2 for admin)
    create_test_user(state, username, password, 2).await;

    // Create a session token
    let session = state
        .auth
        .create_session(username.to_string(), "test-device".to_string(), false)
        .await
        .expect("Failed to create session");

    session.token
}

/// Create a basic test app with common routes
pub async fn create_test_app() -> Router {
    use homevault::handlers::auth::{handle_get_current_user, handle_login, handle_setup_status};
    use homevault::handlers::settings::handle_appearance_settings_get;
    use homevault::web::handle_health_endpoint;

    let (state, _tmp) = setup_test_state().await;

    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/auth/me", get(handle_get_current_user))
        .route("/api/auth/setup-status", get(handle_setup_status))
        .route("/api/health", get(handle_health_endpoint))
        .route(
            "/api/settings/appearance",
            get(handle_appearance_settings_get),
        )
        .with_state(state)
}

/// Create a test app with an admin user and return (app, admin_token, db)
pub async fn create_test_app_with_admin() -> (Router, String, TestDb) {
    use homevault::handlers::auth::{handle_get_current_user, handle_login, handle_setup_status};
    use homevault::handlers::dashboard::handle_storage_data;
    use homevault::handlers::settings::{
        handle_appearance_settings_get, handle_appearance_settings_update,
    };
    use homevault::web::handle_health_endpoint;

    let (state, _tmp) = setup_test_state().await;

    // Create admin user and get token
    let admin_token = create_admin_with_token(&state).await;

    let db = TestDb {
        pool: (*state.db.pool()).clone(),
        database: state.db.clone(),
    };

    let app = Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/auth/me", get(handle_get_current_user))
        .route("/api/auth/setup-status", get(handle_setup_status))
        .route("/api/health", get(handle_health_endpoint))
        .route(
            "/api/settings/appearance",
            get(handle_appearance_settings_get).put(handle_appearance_settings_update),
        )
        .route("/api/dashboard/storage", get(handle_storage_data))
        .with_state(state);

    (app, admin_token, db)
}

/// Helper to create a request with authentication
pub fn authenticated_request(
    method: axum::http::Method,
    uri: &str,
    token: &str,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("x-session-token", token)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Helper to create a request with JSON body and authentication
pub fn authenticated_json_request(
    method: axum::http::Method,
    uri: &str,
    token: &str,
    body: serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("x-session-token", token)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

/// Helper to call a router with a request (works with both Router and Arc<Router>)
pub async fn call_app(
    app: &mut Router,
    req: axum::http::Request<axum::body::Body>,
) -> Result<axum::http::Response<axum::body::Body>, tower::BoxError> {
    use tower::Service;
    // `Router::call` is infallible; `Infallible` converts into `BoxError`.
    app.call(req).await.map_err(tower::BoxError::from)
}
