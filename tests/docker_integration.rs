/* =============================================================================
 * File          : docker_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 20/10/2026
 * Last modified : 20/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Docker integration tests: in-memory SQLite + assembled Axum router,
 *   exercising the endpoints added for the production-ready Docker module
 *   (volumes, prune, exec, inspect, live logs, builds, registries, daemon
 *   config, container updates) end-to-end via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test docker_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Docker handlers.
//!
//! What is covered here is everything that must be true **without a Docker
//! daemon**: authorisation gating on every new endpoint, and input validation
//! that has to reject bad data before any CLI call is attempted (an unknown
//! prune target, an unsafe volume name, a malformed `daemon.json`). The CLI
//! side is covered by the unit tests in `src/docker.rs` (parsers, run-arg
//! reconstruction) and by the Playwright suite against a real daemon.
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt::oneshot` requests.

use axum::{
    routing::{delete, get, post, put},
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
use homevault::handlers::{
    auth::handle_login,
    docker::{
        handle_docker_container_exec, handle_docker_container_inspect,
        handle_docker_container_kill, handle_docker_container_logs_stream,
        handle_docker_container_logs_ticket, handle_docker_container_pause,
        handle_docker_container_recreate, handle_docker_container_rename,
        handle_docker_container_unpause, handle_docker_container_update,
        handle_docker_daemon_config_get, handle_docker_daemon_config_put,
        handle_docker_image_build, handle_docker_prune, handle_docker_registry_list,
        handle_docker_registry_login, handle_docker_registry_logout, handle_docker_system_df,
        handle_docker_volume_create, handle_docker_volume_remove, handle_docker_volumes_list,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

type Req = Request<axum::body::Body>;

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
        // Container extras
        .route(
            "/api/docker/containers/{name}/inspect",
            get(handle_docker_container_inspect),
        )
        .route(
            "/api/docker/containers/{name}/pause",
            put(handle_docker_container_pause),
        )
        .route(
            "/api/docker/containers/{name}/unpause",
            put(handle_docker_container_unpause),
        )
        .route(
            "/api/docker/containers/{name}/kill",
            post(handle_docker_container_kill),
        )
        .route(
            "/api/docker/containers/{name}/rename",
            put(handle_docker_container_rename),
        )
        .route(
            "/api/docker/containers/{name}/update",
            post(handle_docker_container_update),
        )
        .route(
            "/api/docker/containers/{name}/recreate",
            post(handle_docker_container_recreate),
        )
        .route(
            "/api/docker/containers/{name}/exec",
            post(handle_docker_container_exec),
        )
        .route(
            "/api/docker/containers/{name}/logs/ticket",
            post(handle_docker_container_logs_ticket),
        )
        .route(
            "/api/docker/containers/{name}/logs/stream",
            get(handle_docker_container_logs_stream),
        )
        // Volumes, disk usage, prune, builds, registries, daemon config
        .route(
            "/api/docker/volumes",
            get(handle_docker_volumes_list).post(handle_docker_volume_create),
        )
        .route(
            "/api/docker/volumes/{name}",
            delete(handle_docker_volume_remove),
        )
        .route("/api/docker/system/df", get(handle_docker_system_df))
        .route("/api/docker/prune", post(handle_docker_prune))
        .route("/api/docker/images/build", post(handle_docker_image_build))
        .route("/api/docker/registries", get(handle_docker_registry_list))
        .route(
            "/api/docker/registries/login",
            post(handle_docker_registry_login),
        )
        .route(
            "/api/docker/registries/logout",
            post(handle_docker_registry_logout),
        )
        .route(
            "/api/docker/daemon/config",
            get(handle_docker_daemon_config_get).put(handle_docker_daemon_config_put),
        )
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

/// One request per new endpoint, so the auth sweep below covers them all.
fn mutation_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/docker/containers/c/inspect", None),
        (Method::PUT, "/api/docker/containers/c/pause", None),
        (Method::PUT, "/api/docker/containers/c/unpause", None),
        (
            Method::POST,
            "/api/docker/containers/c/kill",
            Some(json!({"signal": "SIGTERM"})),
        ),
        (
            Method::PUT,
            "/api/docker/containers/c/rename",
            Some(json!({"name": "newname"})),
        ),
        (
            Method::POST,
            "/api/docker/containers/c/update",
            Some(json!({"memory_limit": "512m"})),
        ),
        (Method::POST, "/api/docker/containers/c/recreate", None),
        (
            Method::POST,
            "/api/docker/containers/c/exec",
            Some(json!({"command": ["ls"]})),
        ),
        (
            Method::POST,
            "/api/docker/containers/c/logs/ticket",
            Some(json!({"tail": 100})),
        ),
        (Method::GET, "/api/docker/volumes", None),
        (
            Method::POST,
            "/api/docker/volumes",
            Some(json!({"name": "data"})),
        ),
        (Method::DELETE, "/api/docker/volumes/data", None),
        (Method::GET, "/api/docker/system/df", None),
        (
            Method::POST,
            "/api/docker/prune",
            Some(json!({"target": "images"})),
        ),
        (
            Method::POST,
            "/api/docker/images/build",
            Some(json!({"context": "/tmp", "tag": "x:1"})),
        ),
        (Method::GET, "/api/docker/registries", None),
        (
            Method::POST,
            "/api/docker/registries/login",
            Some(json!({"registry": "ghcr.io", "username": "u", "password": "p"})),
        ),
        (
            Method::POST,
            "/api/docker/registries/logout",
            Some(json!({"registry": "ghcr.io"})),
        ),
        (Method::GET, "/api/docker/daemon/config", None),
        (
            Method::PUT,
            "/api/docker/daemon/config",
            Some(json!({"config": "{}"})),
        ),
    ]
}

// ── Authorisation ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_every_docker_endpoint_rejects_missing_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);

    for (method, uri, body) in mutation_endpoints() {
        let (_, json) = send(&mut r, method.clone(), uri, None, body).await;
        assert_eq!(
            json["success"],
            json!(false),
            "{method} {uri} must reject a request with no session token"
        );
    }
}

#[tokio::test]
async fn test_every_docker_endpoint_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "plain", "pw012345", 1).await; // role 1 = user
    let mut r = router(state);
    let token = login(&mut r, "plain", "pw012345").await;

    for (method, uri, body) in mutation_endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, Some(&token), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} must be admin-only"
        );
    }
}

// ── Log stream ticket ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_log_stream_rejects_unknown_ticket() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);

    // No ticket at all.
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/docker/containers/c/logs/stream",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A made-up ticket must not open a stream.
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/docker/containers/c/logs/stream?ticket=not-a-real-ticket",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_log_stream_rejects_ticket_for_another_container() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await; // role 2 = admin
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/containers/alpha/logs/ticket",
        Some(&token),
        Some(json!({"tail": 50, "follow": false})),
    )
    .await;
    assert_eq!(body["success"], json!(true));
    let ticket = body["data"]["ticket"].as_str().expect("ticket").to_string();

    // A ticket minted for `alpha` must not read `beta`'s logs.
    let (status, _) = send(
        &mut r,
        Method::GET,
        &format!("/api/docker/containers/beta/logs/stream?ticket={}", ticket),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── Prune target validation ────────────────────────────────────────────────

#[tokio::test]
async fn test_prune_rejects_unknown_target() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    // An unknown target must never fall back to a destructive default, and it
    // must be rejected before the daemon is even contacted.
    for target in ["everything", "", "image"] {
        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/docker/prune",
            Some(&token),
            Some(json!({ "target": target })),
        )
        .await;
        assert_eq!(
            body["success"],
            json!(false),
            "target '{target}' must be rejected"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("");
        assert!(
            message.contains("Unknown prune target"),
            "unexpected message for '{target}': {message}"
        );
    }
}

// ── Volume name validation ─────────────────────────────────────────────────

#[tokio::test]
async fn test_volume_create_rejects_invalid_names() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    // Path traversal, a leading dash (option injection) and an embedded space
    // are all rejected before any CLI call.
    for name in ["../etc", "-rf", "my volume", "name/with/slash", ""] {
        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/docker/volumes",
            Some(&token),
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(
            body["success"],
            json!(false),
            "volume name '{name}' must be rejected"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("");
        assert!(
            message.contains("Invalid volume name"),
            "unexpected message for '{name}': {message}"
        );
    }
}

// ── Container rename validation ────────────────────────────────────────────

#[tokio::test]
async fn test_rename_rejects_invalid_names() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    for name in ["", "-dash", "with space", "slash/name", "dot.name"] {
        let (_, body) = send(
            &mut r,
            Method::PUT,
            "/api/docker/containers/old/rename",
            Some(&token),
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(
            body["success"],
            json!(false),
            "container name '{name}' must be rejected"
        );
    }
}

// ── daemon.json validation ─────────────────────────────────────────────────

#[tokio::test]
async fn test_daemon_config_rejects_invalid_json() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    // Writing a malformed file would stop dockerd from starting, so it must
    // be refused before the file is touched — and before the sudo call.
    for config in ["{not json", "[1, 2]", "\"string\"", ""] {
        let (_, body) = send(
            &mut r,
            Method::PUT,
            "/api/docker/daemon/config",
            Some(&token),
            Some(json!({ "config": config })),
        )
        .await;
        assert_eq!(
            body["success"],
            json!(false),
            "daemon.json '{config}' must be rejected"
        );
        let message = body["message"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("");
        assert!(
            message.contains("Invalid daemon.json") || message.contains("Invalid JSON"),
            "unexpected message for '{config}': {message}"
        );
    }
}

#[tokio::test]
async fn test_daemon_config_accepts_a_json_object_shape() {
    // The read side never needs root and must answer even when the file is
    // absent, so the settings modal can open on a fresh install.
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/daemon/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["path"], json!("/etc/docker/daemon.json"));
}

// ── Build context validation ───────────────────────────────────────────────

#[tokio::test]
async fn test_build_rejects_context_outside_allowed_roots() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "pw012345").await;

    // `PathGuard` is built with `/tmp` only, so `/etc` must be refused even
    // for an admin: the builder must not be a way to read the whole host.
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/images/build",
        Some(&token),
        Some(json!({"context": "/etc", "tag": "x:1"})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    let message = body["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or("");
    assert!(
        message.contains("outside the allowed directories") || message.contains("not a directory"),
        "unexpected message: {message}"
    );
}
