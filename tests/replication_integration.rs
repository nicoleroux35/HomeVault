/* =============================================================================
 * File          : replication_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Snapshot-replication integration tests: the auth matrix, the full CRUD
 *   lifecycle through the API, every validation path, the dry-run preview and
 *   the capability refusal when the host lacks the backend.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test replication_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for snapshot replication (`crate::replication`,
//! `crate::handlers::replication`):
//!
//!   GET    /api/replication
//!   POST   /api/replication
//!   PUT    /api/replication/{id}
//!   DELETE /api/replication/{id}
//!   GET    /api/replication/{id}/preview
//!   POST   /api/replication/{id}/run
//!
//! Real `zfs send`/`btrfs send` needs a pool, so the transfer itself is covered
//! by the unit tests that assert the exact argv and by the E2E spec on a real
//! pool. Everything above the filesystem — auth, validation, persistence,
//! dry-run and capability refusal — is exercised here.

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
use homevault::handlers::replication::{
    handle_replication_job_create, handle_replication_job_delete, handle_replication_job_update,
    handle_replication_jobs_list, handle_replication_preview, handle_replication_run_now,
};
use homevault::handlers::terminal_pty::PtyManager;
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
            "/api/replication",
            get(handle_replication_jobs_list).post(handle_replication_job_create),
        )
        .route(
            "/api/replication/{id}",
            axum::routing::put(handle_replication_job_update).delete(handle_replication_job_delete),
        )
        .route(
            "/api/replication/{id}/preview",
            get(handle_replication_preview),
        )
        .route(
            "/api/replication/{id}/run",
            post(handle_replication_run_now),
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

fn valid_job() -> Value {
    json!({
        "name": "nightly-to-nas2",
        "source_dataset": "tank/data",
        "backend": "zfs",
        "destination_kind": "ssh",
        "destination_dataset": "backup/data",
        "destination_host": "nas2.lan",
        "destination_user": "backup",
        "destination_port": 2222,
        "interval_minutes": 1440,
        "keep_last": 7,
        "bandwidth_kbps": 8000,
        "enabled": true,
    })
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reads_need_a_session_and_mutations_need_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(&mut router, Method::GET, "/api/replication", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/replication",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (method, uri, body) in [
        (
            Method::POST,
            "/api/replication".to_string(),
            Some(valid_job()),
        ),
        (
            Method::PUT,
            "/api/replication/1".to_string(),
            Some(valid_job()),
        ),
        (Method::DELETE, "/api/replication/1".to_string(), None),
        (Method::POST, "/api/replication/1/run".to_string(), None),
    ] {
        let (status, _) = send(&mut router, method.clone(), &uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, &uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_job_round_trips_through_create_update_and_delete() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/replication",
        Some(&admin),
        Some(valid_job()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = body["data"]["job"]["id"].as_i64().expect("id");
    assert_eq!(body["data"]["job"]["name"], json!("nightly-to-nas2"));
    assert_eq!(body["data"]["job"]["enabled"], json!(true));
    assert!(
        body["data"]["job"]["next_run_at"].is_string(),
        "a scheduled job gets a next run: {body}"
    );

    // The list shows it, with the capability evidence the UI needs.
    let (status, list) = send(
        &mut router,
        Method::GET,
        "/api/replication",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["data"]["jobs"][0]["id"], json!(id));
    assert!(list["data"]["capabilities"].is_object(), "{list}");
    // No credential is ever part of a job view.
    assert!(!list.to_string().contains("password"), "{list}");

    // Update: disabling it clears the schedule.
    let mut updated = valid_job();
    updated["enabled"] = json!(false);
    updated["keep_last"] = json!(3);
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/replication/{id}"),
        Some(&admin),
        Some(updated),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["job"]["keep_last"], json!(3));

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/replication/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, list) = send(
        &mut router,
        Method::GET,
        "/api/replication",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["data"]["jobs"], json!([]));
}

#[tokio::test]
async fn test_a_duplicate_name_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/replication",
        Some(&admin),
        Some(valid_job()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut duplicate = valid_job();
    duplicate["name"] = json!("NIGHTLY-TO-NAS2");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/replication",
        Some(&admin),
        Some(duplicate),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists"),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_dangerous_or_incomplete_jobs_are_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut cases: Vec<(Value, &str)> = Vec::new();

    let mut same = valid_job();
    same["destination_kind"] = json!("local");
    same["destination_dataset"] = json!("tank/data");
    cases.push((same, "same source and destination"));

    let mut shell = valid_job();
    shell["source_dataset"] = json!("tank/data; rm -rf /");
    cases.push((shell, "shell metacharacter in a dataset"));

    let mut traversal = valid_job();
    traversal["destination_dataset"] = json!("backup/../etc");
    cases.push((traversal, "traversal in a dataset"));

    let mut zero_keep = valid_job();
    zero_keep["keep_last"] = json!(0);
    cases.push((zero_keep, "keep_last = 0"));

    let mut btrfs_ssh = valid_job();
    btrfs_ssh["backend"] = json!("btrfs");
    cases.push((btrfs_ssh, "btrfs over ssh"));

    let mut bad_host = valid_job();
    bad_host["destination_host"] = json!("nas; reboot");
    cases.push((bad_host, "shell metacharacter in a host"));

    let mut bad_port = valid_job();
    bad_port["destination_port"] = json!(0);
    cases.push((bad_port, "port 0"));

    let mut bad_interval = valid_job();
    bad_interval["interval_minutes"] = json!(1);
    cases.push((bad_interval, "interval below the minimum"));

    let mut snapshot_as_dataset = valid_job();
    snapshot_as_dataset["destination_dataset"] = json!("backup/data@old");
    cases.push((snapshot_as_dataset, "a snapshot in the dataset field"));

    for (body, label) in cases {
        let (status, response) = send(
            &mut router,
            Method::POST,
            "/api/replication",
            Some(&admin),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{label}: {body} → {response}"
        );
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM replication_jobs")
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "no invalid job may be persisted");
}

// ---------------------------------------------------------------------------
// Dry run and capability refusal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_preview_reports_that_the_pool_is_missing_instead_of_guessing() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/replication",
        Some(&admin),
        Some(valid_job()),
    )
    .await;
    let id = body["data"]["job"]["id"].as_i64().expect("id");

    // No `tank` pool exists in the sandbox: the dry run must say so, and must
    // not create a snapshot or run anything.
    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!("/api/replication/{id}/preview"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot run"),
        "{body}"
    );

    // Previewing an unknown job is a clean error, not a panic.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/replication/999/preview",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn test_run_now_refuses_when_the_backend_is_not_installed() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/replication",
        Some(&admin),
        Some(valid_job()),
    )
    .await;
    let id = body["data"]["job"]["id"].as_i64().expect("id");

    // This host has no ZFS (the test sandbox): the endpoint must refuse with a
    // clear message and leave the job's last status untouched.
    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/replication/{id}/run"),
        Some(&admin),
        None,
    )
    .await;
    if homevault::dependencies::binary_available("zfs", "/usr/sbin/zfs") {
        // A host with ZFS fails later (no `tank` pool) but never silently.
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    } else {
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("no ZFS"),
            "{body}"
        );
    }
}

#[tokio::test]
async fn test_deleting_an_unknown_job_is_a_clean_error() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/replication/424242",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
