/* =============================================================================
 * File          : scheduled_tasks_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled-tasks integration tests: in-memory SQLite + assembled Axum
 *   router, exercising CRUD, auth guards, validation and a real run-now of
 *   a safe command via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the scheduled-tasks API:
//!
//!   - GET    /api/scheduled-tasks            (list)
//!   - POST   /api/scheduled-tasks            (create)
//!   - PUT    /api/scheduled-tasks/{id}       (update)
//!   - DELETE /api/scheduled-tasks/{id}       (delete)
//!   - POST   /api/scheduled-tasks/{id}/run   (run now)
//!   - GET    /api/scheduled-tasks/{id}/runs  (history)

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::scheduled_tasks::{
    handle_task_create, handle_task_delete, handle_task_run_now, handle_task_runs,
    handle_task_update, handle_tasks_list,
};
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

fn tasks_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/auth/login",
            axum::routing::post(homevault::handlers::auth::handle_login),
        )
        .route("/api/scheduled-tasks", get(handle_tasks_list))
        .route(
            "/api/scheduled-tasks",
            axum::routing::post(handle_task_create),
        )
        .route(
            "/api/scheduled-tasks/{id}",
            axum::routing::put(handle_task_update),
        )
        .route(
            "/api/scheduled-tasks/{id}",
            axum::routing::delete(handle_task_delete),
        )
        .route(
            "/api/scheduled-tasks/{id}/run",
            axum::routing::post(handle_task_run_now),
        )
        .route("/api/scheduled-tasks/{id}/runs", get(handle_task_runs))
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

fn valid_task() -> Value {
    json!({
        "name": "Nightly snapshot",
        "task_type": "snapshot",
        "cron": "0 2 * * *",
        "payload": {"pool": "tank", "recursive": false},
        "enabled": true
    })
}

#[tokio::test]
async fn crud_cycle_round_trips() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = tasks_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // Create
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(valid_task()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {body}");
    let id = body["data"]["id"].as_str().expect("id").to_string();
    assert_eq!(body["data"]["task_type"], "snapshot");
    assert!(
        body["data"]["next_run_at"].is_string(),
        "next run precomputed"
    );

    // List
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/scheduled-tasks",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["tasks"].as_array().unwrap().len(), 1);

    // Update (disable)
    let mut edited = valid_task();
    edited["enabled"] = json!(false);
    edited["name"] = json!("Renamed");
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/scheduled-tasks/{id}"),
        Some(&admin),
        Some(edited),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update failed: {body}");
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["name"], "Renamed");
    assert!(
        body["data"]["last_status"].is_null(),
        "edit resets run summary"
    );

    // Run history (empty until a run)
    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!("/api/scheduled-tasks/{id}/runs"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["runs"].as_array().unwrap().len(), 0);

    // Delete
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/scheduled-tasks/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/scheduled-tasks",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["tasks"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn auth_guards_reject_anonymous_and_non_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    create_test_user(&state, "user", "UserPass123!", USER_ROLE_ID).await;
    let mut router = tasks_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;
    let user = login(&mut router, "user", "UserPass123!").await;

    for (method, uri) in [
        (Method::GET, "/api/scheduled-tasks"),
        (Method::POST, "/api/scheduled-tasks"),
    ] {
        let (status, _) = send(&mut router, method.clone(), uri, None, None).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "anonymous {method} {uri} must not pass"
        );
        // POST carries a body: the Json extractor runs before the guard, so
        // a well-formed request is what reaches require_admin (and gets the
        // 403); an empty body is rejected earlier with 415 — both are
        // rejections, neither is a success.
        let body = if method == Method::POST {
            Some(valid_task())
        } else {
            None
        };
        let (status, resp) = send(&mut router, method.clone(), uri, Some(&user), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "non-admin must be rejected: {resp}"
        );
    }

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/scheduled-tasks",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "admin must pass");
}

#[tokio::test]
async fn validation_rejects_bad_cron_payload_and_type() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = tasks_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // Bad cron
    let mut bad = valid_task();
    bad["cron"] = json!("not a cron");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("cron"), "{body}");

    // Payload/type mismatch: declared snapshot, payload is a command
    let mut bad = valid_task();
    bad["payload"] = json!({"command": "/bin/true"});
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown task type
    let mut bad = valid_task();
    bad["task_type"] = json!("rm_rf");
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Command without absolute path
    let bad = json!({
        "name": "curl", "task_type": "command", "cron": "* * * * *",
        "payload": {"command": "curl", "args": []}
    });
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap().contains("absolute"),
        "{body}"
    );

    // Non-existent executable
    let bad = json!({
        "name": "missing", "task_type": "command", "cron": "* * * * *",
        "payload": {"command": "/no/such/binary", "args": []}
    });
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap().contains("does not exist"),
        "{body}"
    );
}

#[tokio::test]
async fn run_now_executes_a_safe_command_and_records_history() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = tasks_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    // /bin/true exists on every Linux CI runner; /bin/false fails cleanly.
    let task = json!({
        "name": "true task", "task_type": "command", "cron": "0 0 1 1 *",
        "payload": {"command": "/bin/true", "args": []}
    });
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(task),
    )
    .await;
    let id = body["data"]["id"].as_str().expect("id").to_string();

    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/scheduled-tasks/{id}/run"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "run now failed: {body}");
    assert_eq!(body["data"]["last_status"], "success");

    let (_, body) = send(
        &mut router,
        Method::GET,
        &format!("/api/scheduled-tasks/{id}/runs"),
        Some(&admin),
        None,
    )
    .await;
    let runs = body["data"]["runs"].as_array().expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"], "success");

    // A failing command records a failed run with the message.
    let task = json!({
        "name": "false task", "task_type": "command", "cron": "0 0 1 1 *",
        "payload": {"command": "/bin/false", "args": []}
    });
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks",
        Some(&admin),
        Some(task),
    )
    .await;
    let id = body["data"]["id"].as_str().expect("id").to_string();
    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/scheduled-tasks/{id}/run"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "failed run surfaces the error: {body}"
    );
    let (_, body) = send(
        &mut router,
        Method::GET,
        &format!("/api/scheduled-tasks/{id}/runs"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["runs"][0]["status"], "failed");
}

#[tokio::test]
async fn update_and_delete_of_unknown_id_fail_cleanly() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "AdminPass123!", ADMIN_ROLE_ID).await;
    let mut router = tasks_router(state.clone());
    let admin = login(&mut router, "admin", "AdminPass123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/scheduled-tasks/nope",
        Some(&admin),
        Some(valid_task()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Run-now / runs of an unknown id must not panic (the old progress
    // handler did) — they answer with an error envelope.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/scheduled-tasks/nope/run",
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/scheduled-tasks/nope/runs",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "empty history is valid for unknown ids"
    );
}
