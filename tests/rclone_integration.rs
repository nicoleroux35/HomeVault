// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Cloud Sync (rclone) API —
//! `crate::handlers::rclone`.
//!
//! 12 endpoints: status, remotes list/add/delete, providers, mounts
//! list/start/stop, sync, copy, config get/save.
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server.
//!
//! **This suite never runs a real `rclone` transfer or mount and never
//! writes `/etc/rclone.conf`.** The original audit found all 13 handlers
//! took no session at all, `GET /rclone/config` leaked every remote's
//! credentials, and `sync`/`copy` with a `"local"` path was an unrestricted
//! file primitive. This suite locks down what protects against that:
//!   * **every** endpoint requires a valid session; every mutation AND the
//!     raw-config read require **admin** (`check_admin`);
//!   * the `[section]` config-injection guard on `add_remote`, the
//!     invalid-name guard, and the empty-source/dest guards on sync/copy
//!     all reject with a clean `{"success":false}` — no `rclone` spawned,
//!     no file written.
//!
//! A real `add_remote` → list → `delete_remote` round-trip needs a
//! writable `/etc/rclone.conf` (root on a real deployment) and is left to
//! `tests/e2e/cloudsync/cloudsync.spec.ts` (opt-in there too).
//!
//! Note on status codes: these handlers always return HTTP 200 and signal
//! auth/validation failure in the body (`{"success":false,"error":…}`) —
//! the frontend checks `res.success`. Assertions match that.
//!
//! Run:
//!   cargo test --test rclone_integration
//!   cargo test --test rclone_integration -- --nocapture

use axum::{
    routing::{get, post},
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
    rclone::{
        handle_add_remote, handle_copy, handle_create_sync_job, handle_delete_remote,
        handle_delete_sync_job, handle_get_config, handle_list_mounts, handle_list_providers,
        handle_list_remotes, handle_list_sync_jobs, handle_mount_start, handle_mount_stop,
        handle_rclone_status, handle_run_sync_job, handle_save_config, handle_sync,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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

fn rclone_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/rclone/status", get(handle_rclone_status))
        .route("/api/rclone/remotes", get(handle_list_remotes))
        .route("/api/rclone/remotes", post(handle_add_remote))
        .route(
            "/api/rclone/remotes/{name}",
            axum::routing::delete(handle_delete_remote),
        )
        .route("/api/rclone/providers", get(handle_list_providers))
        .route("/api/rclone/mounts", get(handle_list_mounts))
        .route("/api/rclone/mount/start", post(handle_mount_start))
        .route("/api/rclone/mount/stop", post(handle_mount_stop))
        .route("/api/rclone/sync", post(handle_sync))
        .route("/api/rclone/copy", post(handle_copy))
        .route("/api/rclone/config", get(handle_get_config))
        .route("/api/rclone/config", post(handle_save_config))
        .route("/api/rclone/jobs", get(handle_list_sync_jobs))
        .route("/api/rclone/jobs", post(handle_create_sync_job))
        .route(
            "/api/rclone/jobs/{id}",
            axum::routing::delete(handle_delete_sync_job),
        )
        .route("/api/rclone/jobs/{id}/run", post(handle_run_sync_job))
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
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn denied(body: &Value, fragment: &str) {
    assert_eq!(
        body["success"], false,
        "expected a denied envelope, got {body}"
    );
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains(fragment),
        "error {err:?} should contain {fragment:?}"
    );
}

/// Every endpoint with a body valid enough to reach the guard.
fn all_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/rclone/status", None),
        (Method::GET, "/api/rclone/remotes", None),
        (
            Method::POST,
            "/api/rclone/remotes",
            Some(json!({"name": "r", "provider": "drive"})),
        ),
        (Method::DELETE, "/api/rclone/remotes/r", None),
        (Method::GET, "/api/rclone/providers", None),
        (Method::GET, "/api/rclone/mounts", None),
        (
            Method::POST,
            "/api/rclone/mount/start",
            Some(json!({"remote": "r", "mount_point": "/tmp/hv-x"})),
        ),
        (
            Method::POST,
            "/api/rclone/mount/stop",
            Some(json!({"remote": "r", "mount_point": "/tmp/hv-x"})),
        ),
        (
            Method::POST,
            "/api/rclone/sync",
            Some(
                json!({"source": "a", "destination": "b", "delete": false, "dry_run": true, "update": false}),
            ),
        ),
        (
            Method::POST,
            "/api/rclone/copy",
            Some(
                json!({"source": "a", "destination": "b", "delete": false, "dry_run": true, "interactive": false}),
            ),
        ),
        (Method::GET, "/api/rclone/config", None),
        (
            Method::POST,
            "/api/rclone/config",
            Some(json!({"config": "[x]\ntype = drive\n"})),
        ),
        (Method::GET, "/api/rclone/jobs", None),
        (
            Method::POST,
            "/api/rclone/jobs",
            Some(json!({"name": "j", "source": "a", "destination": "b"})),
        ),
        (Method::DELETE, "/api/rclone/jobs/1", None),
        (Method::POST, "/api/rclone/jobs/1/run", None),
    ]
}

/// The subset that only needs a session (not admin).
const SESSION_ONLY: &[&str] = &[
    "/api/rclone/status",
    "/api/rclone/remotes", // GET
    "/api/rclone/providers",
    "/api/rclone/mounts",
    "/api/rclone/jobs", // GET
];

// ---------------------------------------------------------------------------
// 1. Session guard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_endpoints_require_a_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = rclone_router(state);
    for (m, u, b) in all_endpoints() {
        let (status, body) = send(&mut router, m.clone(), u, None, b).await;
        assert_eq!(status, StatusCode::OK, "{m} {u}");
        denied(&body, "No session token provided");
    }
}

#[tokio::test]
async fn test_all_endpoints_reject_a_bogus_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = rclone_router(state);
    for (m, u, b) in all_endpoints() {
        let (_, body) = send(&mut router, m.clone(), u, Some("nope"), b).await;
        denied(&body, "Invalid or expired session");
    }
}

// ---------------------------------------------------------------------------
// 2. Admin guard — mutations + the raw-config read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mutations_and_config_read_require_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    let mut router = rclone_router(state);
    let token = login(&mut router, "reg", "Password123!").await;

    for (m, u, b) in all_endpoints() {
        let session_only = m == Method::GET && SESSION_ONLY.contains(&u);
        let (_, body) = send(&mut router, m.clone(), u, Some(&token), b).await;
        if session_only {
            // a non-admin session is allowed here (no assertion on the
            // success flag — `rclone` may or may not be installed)
            assert!(
                body.is_object(),
                "{m} {u} should answer for a valid session"
            );
        } else {
            denied(&body, "Admin privileges required");
        }
    }
}

#[tokio::test]
async fn test_get_config_is_the_credential_leak_gate() {
    // The headline finding: `GET /api/rclone/config` returns the raw
    // rclone.conf, i.e. every remote's (trivially-dobfuscated) credentials.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rclone_router(state);

    let reg = login(&mut router, "reg", "Password123!").await;
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/config",
        Some(&reg),
        None,
    )
    .await;
    denied(&body, "Admin privileges required");

    // admin gets past the guard (the actual read may still fail if
    // /etc/rclone.conf is absent — that's fine, it's not a 403)
    let admin = login(&mut router, "admin1", "Password123!").await;
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/config",
        Some(&admin),
        None,
    )
    .await;
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        !err.contains("Admin privileges"),
        "admin must pass the config gate, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. add_remote validation — nothing is written
// ---------------------------------------------------------------------------

async fn admin_router() -> (
    Router,
    String,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rclone_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;
    (router, token, tmp, guard)
}

#[tokio::test]
async fn test_add_remote_rejects_empty_name() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/remotes",
        Some(&token),
        Some(json!({"name": "", "provider": "drive"})),
    )
    .await;
    denied(&body, "Remote name is required");
}

#[tokio::test]
async fn test_add_remote_rejects_names_with_ini_metacharacters() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    for bad in ["a:b", "has[bracket", "has]bracket", "x:"] {
        let (_, body) = send(
            &mut router,
            Method::POST,
            "/api/rclone/remotes",
            Some(&token),
            Some(json!({"name": bad, "provider": "drive"})),
        )
        .await;
        denied(&body, "invalid characters");
    }
}

#[tokio::test]
async fn test_add_remote_rejects_config_section_injection() {
    // A `config` value with a `[section]` line would inject/override an
    // unrelated remote when appended verbatim into the INI file.
    let (mut router, token, _tmp, _g) = admin_router().await;
    for payload in [
        "token = x\n[evil]\ntype = local",
        "  [evil]",
        "[another]\ntype = s3",
    ] {
        let (_, body) = send(
            &mut router,
            Method::POST,
            "/api/rclone/remotes",
            Some(&token),
            Some(json!({"name": "ok", "provider": "drive", "config": payload})),
        )
        .await;
        denied(&body, "would inject another config section");
    }
}

// ---------------------------------------------------------------------------
// 4. sync / copy / mount validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sync_requires_source_and_destination() {
    let (mut router, token, _tmp, _g) = admin_router().await;

    // empty cloud source → "remote:" collapses to ":" which is rejected
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/sync",
        Some(&token),
        Some(json!({
            "source_type": "cloud", "source": "",
            "destination": "b", "delete": false, "dry_run": true, "update": false
        })),
    )
    .await;
    denied(&body, "Source is required");

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/sync",
        Some(&token),
        Some(json!({
            "source": "a", "source_type": "cloud",
            "dest_type": "cloud", "destination": "",
            "delete": false, "dry_run": true, "update": false
        })),
    )
    .await;
    denied(&body, "Destination is required");
}

#[tokio::test]
async fn test_copy_requires_source_and_destination() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/copy",
        Some(&token),
        Some(json!({
            "source_type": "cloud", "source": "",
            "destination": "b", "delete": false, "dry_run": true, "interactive": false
        })),
    )
    .await;
    denied(&body, "Source is required");
}

#[tokio::test]
async fn test_mount_start_requires_a_mount_point() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/mount/start",
        Some(&token),
        Some(json!({"remote": "gd", "mount_point": ""})),
    )
    .await;
    denied(&body, "Mount point is required");
}

#[tokio::test]
async fn test_mount_start_rejects_a_malformed_body_before_the_handler() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    // `remote` and `mount_point` are required (no serde default) → 422 from
    // the Json extractor, before check_admin even runs.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/rclone/mount/start",
        Some(&token),
        Some(json!({"path": "x"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------------------------------------------------------------------------
// 5. Read success paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_providers_list_is_a_non_empty_hardcoded_catalog() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/providers",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let arr = body["data"].as_array().expect("providers array");
    assert!(arr.len() > 20, "expected the full provider catalog");
    assert!(arr.iter().any(|p| p["name"] == "drive"));
    assert!(arr.iter().all(|p| p["tier"].as_str().is_some()));
}

#[tokio::test]
async fn test_status_and_mounts_answer_for_a_plain_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    let mut router = rclone_router(state);
    let token = login(&mut router, "reg", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert!(body["data"]["available"].is_boolean());

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/mounts",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], true);
    assert!(body["data"].is_array());
}

// ---------------------------------------------------------------------------
// Sync jobs CRUD (server-side persistence — the frontend no longer uses
// localStorage for this)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sync_jobs_crud_round_trip() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "adm", "Password123!", 2).await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    let mut router = rclone_router(state);
    let admin = login(&mut router, "adm", "Password123!").await;
    let reg = login(&mut router, "reg", "Password123!").await;

    // A fresh DB starts with no jobs.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/jobs",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["success"], true);
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    // A non-admin session can list (read) but not create (mutation).
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/jobs",
        Some(&reg),
        None,
    )
    .await;
    assert_eq!(body["success"], true);
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/jobs",
        Some(&reg),
        Some(json!({"name": "nope", "source": "a", "destination": "b"})),
    )
    .await;
    denied(&body, "Admin privileges required");

    // Invalid payload (empty name/source/destination) is rejected cleanly.
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/jobs",
        Some(&admin),
        Some(json!({"name": "", "source": "", "destination": ""})),
    )
    .await;
    assert_eq!(body["success"], false);

    // Create a local-to-cloud job.
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/jobs",
        Some(&admin),
        Some(json!({
            "name": "photos",
            "source_type": "local",
            "source": "/home/user/photos",
            "destination": "gdrive:photos",
        })),
    )
    .await;
    assert_eq!(body["success"], true, "create failed: {body}");
    let id = body["data"]["id"].as_i64().expect("job id");
    assert_eq!(body["data"]["direction"], "local-to-cloud");
    assert_eq!(body["data"]["name"], "photos");

    // List reflects the new job; a second job round-trips the other direction.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/jobs",
        Some(&admin),
        None,
    )
    .await;
    let jobs = body["data"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["id"], json!(id));

    // Run now on a bogus remote — fails gracefully (missing rclone binary OR
    // unknown remote), never panics.
    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/rclone/jobs/{id}/run"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false, "expected graceful failure: {body}");

    // Run now on a missing job → clean error.
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rclone/jobs/999999/run",
        Some(&admin),
        None,
    )
    .await;
    denied(&body, "Sync job not found");

    // Delete + verify the list is empty again; deleting twice is a clean error.
    let (_, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/rclone/jobs/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["success"], true);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rclone/jobs",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
    let (_, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/rclone/jobs/{id}"),
        Some(&admin),
        None,
    )
    .await;
    denied(&body, "Sync job not found");
}
