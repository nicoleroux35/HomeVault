/* =============================================================================
 * File          : restic_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Restic integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Backup (restic) API — `crate::handlers::restic`.
//!
//! 27 endpoints: repo CRUD, snapshots/stats/jobs, backup/restore/check/
//! forget/prune/unlock, backup schedules CRUD, retention-policy CRUD.
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server.
//!
//! **This suite never runs a real `restic` command that touches a repo.**
//! `restic init`/`backup`/`restore`/`forget`/`prune`/`check`/`stats`/
//! `snapshots` against a real on-disk repository is
//! `tests/e2e/backup/backup.spec.ts`'s job (restic is genuinely available
//! there and the test uses a throwaway local repo). Here the restic-shelling
//! endpoints are exercised only up to:
//!   * the auth split — reads (`available`, `version`, list/get repos,
//!     snapshots, stats, jobs, check, list schedules/policies) take any
//!     valid session; every mutation and `?include_password=true` takes
//!     admin;
//!   * a clean 400 for a missing/unknown repo id, an invalid cron
//!     expression, an empty policy/schedule name, or missing required
//!     fields — all *before* any `restic` process is spawned.
//!
//! The **schedule** and **retention-policy** lifecycles are pure DB +
//! `compute_next_run` (no restic at all), so those are covered end to end.
//!
//! The single most important guard here: `GET /api/restic/repos?
//! include_password=true` must require admin — it returns the repository
//! passwords in plaintext (release-audit finding).
//!
//! Run:
//!   cargo test --test restic_integration
//!   cargo test --test restic_integration -- --nocapture

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
    restic::{
        handle_restic_available, handle_restic_backup_create, handle_restic_backup_jobs,
        handle_restic_check, handle_restic_forget, handle_restic_prune, handle_restic_reminders,
        handle_restic_repo_create, handle_restic_repo_delete, handle_restic_repo_get,
        handle_restic_repo_update, handle_restic_repos_list, handle_restic_restore,
        handle_restic_restore_plan, handle_restic_schedule_create, handle_restic_schedule_delete,
        handle_restic_schedule_set_enabled, handle_restic_schedule_update,
        handle_restic_schedules_list, handle_restic_snapshot_delete, handle_restic_snapshots_list,
        handle_restic_stats, handle_restic_unlock, handle_restic_version,
        handle_retention_policies_list, handle_retention_policy_create,
        handle_retention_policy_delete, handle_retention_policy_update,
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

fn restic_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/restic/available", get(handle_restic_available))
        .route("/api/restic/version", get(handle_restic_version))
        .route("/api/restic/repos", get(handle_restic_repos_list))
        .route("/api/restic/repos", post(handle_restic_repo_create))
        .route("/api/restic/repos/{id}", get(handle_restic_repo_get))
        .route("/api/restic/repos/{id}", put(handle_restic_repo_update))
        .route("/api/restic/repos/{id}", delete(handle_restic_repo_delete))
        .route(
            "/api/restic/repos/{id}/snapshots",
            get(handle_restic_snapshots_list),
        )
        .route("/api/restic/repos/{id}/stats", get(handle_restic_stats))
        .route(
            "/api/restic/repos/{id}/jobs",
            get(handle_restic_backup_jobs),
        )
        .route("/api/restic/repos/{id}/unlock", post(handle_restic_unlock))
        .route("/api/restic/repos/{id}/prune", post(handle_restic_prune))
        .route("/api/restic/backup", post(handle_restic_backup_create))
        .route("/api/restic/restore", post(handle_restic_restore))
        .route("/api/restic/restore-plan", get(handle_restic_restore_plan))
        .route("/api/restic/reminders", get(handle_restic_reminders))
        .route("/api/restic/check", post(handle_restic_check))
        .route("/api/restic/forget", post(handle_restic_forget))
        .route(
            "/api/restic/snapshots/delete",
            post(handle_restic_snapshot_delete),
        )
        .route("/api/restic/schedules", get(handle_restic_schedules_list))
        .route("/api/restic/schedules", post(handle_restic_schedule_create))
        .route(
            "/api/restic/schedules/{id}",
            put(handle_restic_schedule_update),
        )
        .route(
            "/api/restic/schedules/{id}",
            delete(handle_restic_schedule_delete),
        )
        .route(
            "/api/restic/schedules/{id}/enabled",
            put(handle_restic_schedule_set_enabled),
        )
        .route(
            "/api/restic/retention-policies",
            get(handle_retention_policies_list),
        )
        .route(
            "/api/restic/retention-policies",
            post(handle_retention_policy_create),
        )
        .route(
            "/api/restic/retention-policies/{id}",
            put(handle_retention_policy_update),
        )
        .route(
            "/api/restic/retention-policies/{id}",
            delete(handle_retention_policy_delete),
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

/// Insert a restic_repos row directly (bypasses `restic init`). Its
/// `repository` path points nowhere — fine, nothing here shells out to restic.
async fn insert_repo(state: &AppState, id: &str, name: &str) {
    sqlx::query(
        "INSERT INTO restic_repos (id, name, repository, password, source_paths, snapshot_source, snapshot_create_before_backup, created_at, updated_at)
         VALUES (?, ?, '/nonexistent/repo', 'pw', '[\"/tmp/src\"]', 'directory', 0, datetime('now'), datetime('now'))",
    )
    .bind(id)
    .bind(name)
    .execute(&*state.db.pool())
    .await
    .expect("insert repo row");
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

fn msg(b: &Value) -> String {
    b["message"]
        .as_str()
        .or_else(|| b["error"].as_str())
        .unwrap_or("")
        .to_string()
}

async fn admin_router() -> (
    Router,
    String,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = restic_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;
    (router, token, tmp, guard)
}

fn read_endpoints() -> Vec<(Method, &'static str)> {
    vec![
        (Method::GET, "/api/restic/available"),
        (Method::GET, "/api/restic/version"),
        (Method::GET, "/api/restic/repos"),
        (Method::GET, "/api/restic/schedules"),
        (Method::GET, "/api/restic/retention-policies"),
    ]
}

fn mutation_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (
            Method::POST,
            "/api/restic/repos",
            Some(
                json!({"name":"r","repository":"/tmp/r","password":"p","source_paths":["/tmp/s"]}),
            ),
        ),
        (
            Method::PUT,
            "/api/restic/repos/x",
            Some(json!({"name":"r2"})),
        ),
        (Method::DELETE, "/api/restic/repos/x", None),
        (
            Method::POST,
            "/api/restic/backup",
            Some(json!({"repo_id":"x"})),
        ),
        (
            Method::POST,
            "/api/restic/restore",
            Some(json!({"repo_id":"x","snapshot_id":"s","target":"/tmp/t"})),
        ),
        (
            Method::POST,
            "/api/restic/forget",
            Some(json!({"repo_id":"x"})),
        ),
        (Method::POST, "/api/restic/repos/x/prune", None),
        (Method::POST, "/api/restic/repos/x/unlock", None),
        (
            Method::POST,
            "/api/restic/snapshots/delete",
            Some(json!({"repo_id":"x","snapshot_id":"s"})),
        ),
        (
            Method::POST,
            "/api/restic/schedules",
            Some(json!({"repo_id":"x","name":"n","cron_expression":"0 3 * * *"})),
        ),
        (
            Method::PUT,
            "/api/restic/schedules/x",
            Some(json!({"name":"n2"})),
        ),
        (Method::DELETE, "/api/restic/schedules/x", None),
        (
            Method::PUT,
            "/api/restic/schedules/x/enabled",
            Some(json!({"enabled": false})),
        ),
        (
            Method::POST,
            "/api/restic/retention-policies",
            Some(json!({"name":"p"})),
        ),
        (
            Method::PUT,
            "/api/restic/retention-policies/x",
            Some(json!({"name":"p2"})),
        ),
        (Method::DELETE, "/api/restic/retention-policies/x", None),
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reads_reject_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = restic_router(state);
    for (m, u) in read_endpoints() {
        let (s, _) = send(&mut router, m.clone(), u, None, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{m} {u}");
    }
}

#[tokio::test]
async fn test_reads_reject_bogus_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = restic_router(state);
    for (m, u) in read_endpoints() {
        let (s, _) = send(&mut router, m.clone(), u, Some("nope"), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{m} {u}");
    }
}

#[tokio::test]
async fn test_reads_allow_any_valid_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    let mut router = restic_router(state);
    let token = login(&mut router, "reg", "Password123!").await;
    for (m, u) in read_endpoints() {
        let (s, _) = send(&mut router, m.clone(), u, Some(&token), None).await;
        if u == "/api/restic/version" {
            // `restic version` shells out to the real CLI; when the binary is
            // not installed the endpoint still rejects cleanly (400) — a fresh
            // QA/CI host must not turn green->red over a missing tool, so
            // tolerate either outcome here (auth gate stays asserted both ways).
            assert!(
                s == StatusCode::OK || s == StatusCode::BAD_REQUEST,
                "{m} {u} should allow a non-admin (cleanly) — got {s}"
            );
        } else {
            assert_eq!(s, StatusCode::OK, "{m} {u} should allow a non-admin");
        }
    }
}

#[tokio::test]
async fn test_mutations_reject_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = restic_router(state);
    for (m, u, b) in mutation_endpoints() {
        let (s, _) = send(&mut router, m.clone(), u, None, b).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{m} {u}");
    }
}

#[tokio::test]
async fn test_mutations_reject_non_admin_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    let mut router = restic_router(state);
    let token = login(&mut router, "reg", "Password123!").await;
    for (m, u, b) in mutation_endpoints() {
        let (s, _) = send(&mut router, m.clone(), u, Some(&token), b).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {u} should be admin-only");
    }
}

// ---------------------------------------------------------------------------
// 2. The include_password credential gate (release-audit finding)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_repos_list_include_password_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg", "Password123!", 1).await;
    let mut router = restic_router(state);
    let token = login(&mut router, "reg", "Password123!").await;

    // plain list is fine for a non-admin
    let (s, _) = send(
        &mut router,
        Method::GET,
        "/api/restic/repos",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // ...but asking for the passwords is admin-only
    let (s, _) = send(
        &mut router,
        Method::GET,
        "/api/restic/repos?include_password=true",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_repos_list_password_is_redacted_without_the_flag_and_present_with_it() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    sqlx::query(
        "INSERT INTO restic_repos (id, name, repository, password, source_paths, snapshot_source, snapshot_create_before_backup, created_at, updated_at)
         VALUES ('r1','R1','/nope','s3cr3t-pw','[]','directory',0,datetime('now'),datetime('now'))",
    )
    .execute(&*state.db.pool())
    .await
    .expect("insert repo");
    let mut router = restic_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // plain list (admin) — password must be redacted / not the real value
    let (s, body) = send(
        &mut router,
        Method::GET,
        "/api/restic/repos",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let plain = &body["data"].as_array().unwrap()[0];
    assert_ne!(
        plain["password"].as_str(),
        Some("s3cr3t-pw"),
        "plain list must not leak the repo password"
    );

    // include_password=true (admin) — real value returned
    let (s, body) = send(
        &mut router,
        Method::GET,
        "/api/restic/repos?include_password=true",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        body["data"].as_array().unwrap()[0]["password"].as_str(),
        Some("s3cr3t-pw")
    );
}

// ---------------------------------------------------------------------------
// 3. restic-command endpoints: clean 400 for an unknown repo, no process spawn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_repo_scoped_endpoints_report_unknown_repo() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    let cases: Vec<(Method, &str, Option<Value>)> = vec![
        (Method::GET, "/api/restic/repos/ghost", None),
        (Method::GET, "/api/restic/repos/ghost/snapshots", None),
        (Method::GET, "/api/restic/repos/ghost/stats", None),
        (Method::POST, "/api/restic/repos/ghost/unlock", None),
        (Method::POST, "/api/restic/repos/ghost/prune", None),
        (
            Method::POST,
            "/api/restic/backup",
            Some(json!({"repo_id":"ghost"})),
        ),
        (
            Method::POST,
            "/api/restic/check",
            Some(json!({"repo_id":"ghost"})),
        ),
        (
            Method::POST,
            "/api/restic/forget",
            Some(json!({"repo_id":"ghost"})),
        ),
        (
            Method::POST,
            "/api/restic/restore",
            Some(json!({"repo_id":"ghost","snapshot_id":"s","target":"/tmp/t"})),
        ),
    ];
    for (m, u, b) in cases {
        let (s, body) = send(&mut router, m.clone(), u, Some(&token), b).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{m} {u}");
        assert!(
            msg(&body).to_lowercase().contains("not found"),
            "{m} {u}: {:?}",
            msg(&body)
        );
    }

    // `jobs` deliberately does NOT gate on repo existence — it just returns
    // an empty list for an unknown repo (harmless).
    let (s, body) = send(
        &mut router,
        Method::GET,
        "/api/restic/repos/ghost/jobs",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn test_snapshot_delete_requires_both_ids() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    for b in [
        json!({}),
        json!({"repo_id":"x"}),
        json!({"snapshot_id":"s"}),
    ] {
        let (s, body) = send(
            &mut router,
            Method::POST,
            "/api/restic/snapshots/delete",
            Some(&token),
            Some(b),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(msg(&body).contains("required"));
    }
}

// ---------------------------------------------------------------------------
// 4. Retention policies — full lifecycle (pure DB)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retention_policy_lifecycle() {
    let (mut router, token, _tmp, _g) = admin_router().await;

    // empty name rejected
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/retention-policies",
        Some(&token),
        Some(json!({"name":"  "})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("name is required"));

    // create
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/retention-policies",
        Some(&token),
        Some(json!({"name":"keep-7","max_count":7,"max_age_days":30,"apply_to_all":true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let id = body["data"]["id"].as_str().unwrap().to_string();

    // list contains it
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/restic/retention-policies",
        Some(&token),
        None,
    )
    .await;
    assert!(body["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == id));

    // update
    let (s, _) = send(
        &mut router,
        Method::PUT,
        &format!("/api/restic/retention-policies/{id}"),
        Some(&token),
        Some(json!({"max_count":14})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // update unknown → 400
    let (s, body) = send(
        &mut router,
        Method::PUT,
        "/api/restic/retention-policies/nope",
        Some(&token),
        Some(json!({"max_count":1})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"));

    // delete
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/restic/retention-policies/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // delete again → 400
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/restic/retention-policies/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 5. Schedules — full lifecycle (pure DB + compute_next_run)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_schedule_create_requires_existing_repo() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/schedules",
        Some(&token),
        Some(json!({"repo_id":"ghost","name":"nightly","cron_expression":"0 3 * * *"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("Repository not found"));
}

#[tokio::test]
async fn test_schedule_lifecycle() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    insert_repo(&state, "repo-1", "Repo One").await;
    let mut router = restic_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // invalid cron rejected
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/schedules",
        Some(&token),
        Some(json!({"repo_id":"repo-1","name":"n","cron_expression":"not a cron"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("Invalid cron"));

    // empty name rejected
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/restic/schedules",
        Some(&token),
        Some(json!({"repo_id":"repo-1","name":"  ","cron_expression":"0 3 * * *"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // create with a valid crontab expression
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/schedules",
        Some(&token),
        Some(json!({"repo_id":"repo-1","name":"nightly","cron_expression":"0 3 * * 1"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let id = body["data"]["id"].as_str().unwrap().to_string();
    assert!(
        body["data"]["next_run_at"].as_str().is_some(),
        "next_run_at must be computed on create"
    );

    // list (filtered by repo)
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/restic/schedules?repo_id=repo-1",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    // disable then re-enable (re-enable recomputes next_run_at)
    let (s, _) = send(
        &mut router,
        Method::PUT,
        &format!("/api/restic/schedules/{id}/enabled"),
        Some(&token),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/restic/schedules/{id}/enabled"),
        Some(&token),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], true);

    // update cron (bad cron on update also rejected)
    let (s, _) = send(
        &mut router,
        Method::PUT,
        &format!("/api/restic/schedules/{id}"),
        Some(&token),
        Some(json!({"cron_expression":"*/30 * * * *"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(
        &mut router,
        Method::PUT,
        &format!("/api/restic/schedules/{id}"),
        Some(&token),
        Some(json!({"cron_expression":"99 99 * * *"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // update / delete unknown → 400
    let (s, _) = send(
        &mut router,
        Method::PUT,
        "/api/restic/schedules/nope",
        Some(&token),
        Some(json!({"name":"x"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // delete
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/restic/schedules/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/restic/schedules/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 6. Read success paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_available_and_version_shape() {
    let (mut router, token, _tmp, _g) = admin_router().await;

    let (s, body) = send(
        &mut router,
        Method::GET,
        "/api/restic/available",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["data"]["available"].is_boolean());

    // version only succeeds when restic is installed; tolerate either
    // (a clean 400 from a missing CLI is a valid, non-panicking response).
    let (s, _) = send(
        &mut router,
        Method::GET,
        "/api/restic/version",
        Some(&token),
        None,
    )
    .await;
    assert!(
        s == StatusCode::OK || s == StatusCode::BAD_REQUEST,
        "GET /api/restic/version should be OK or a clean 400 — got {s}"
    );
}

#[tokio::test]
async fn test_lists_start_empty() {
    let (mut router, token, _tmp, _g) = admin_router().await;
    for u in [
        "/api/restic/repos",
        "/api/restic/schedules",
        "/api/restic/retention-policies",
    ] {
        let (s, body) = send(&mut router, Method::GET, u, Some(&token), None).await;
        assert_eq!(s, StatusCode::OK, "{u}");
        assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0), "{u}");
    }
}

// ---------------------------------------------------------------------------
// Reminders + restore pre-flight.
// ---------------------------------------------------------------------------

/// Insert a completed backup job finishing `days_ago` days in the past.
async fn insert_completed_job(state: &AppState, repo_id: &str, days_ago: i64) {
    let finished = chrono::Utc::now() - chrono::Duration::days(days_ago);
    sqlx::query(
        "INSERT INTO restic_backup_jobs (id, repo_id, source, backup_type, status, started_at, finished_at, created_at)
         VALUES (?, ?, '/tmp/src', 'manual', 'completed', ?, ?, datetime('now'))",
    )
    .bind(format!("job-{repo_id}-{days_ago}"))
    .bind(repo_id)
    .bind(finished.to_rfc3339())
    .bind(finished.to_rfc3339())
    .execute(&*state.db.pool())
    .await
    .expect("insert completed job");
}

/// A repo that never backed up is reminded; a freshly-backed-up repo is not.
#[tokio::test]
async fn test_reminders_flag_stale_repos_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut r = restic_router(state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    insert_repo(&state, "repo-stale", "stale").await;
    insert_repo(&state, "repo-fresh", "fresh").await;
    insert_completed_job(&state, "repo-fresh", 1).await;
    insert_completed_job(&state, "repo-stale", 30).await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/restic/reminders",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reminders = body["data"]["reminders"]
        .as_array()
        .expect("reminders array");
    let ids: Vec<&str> = reminders
        .iter()
        .map(|e| e["repo_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"repo-stale"), "stale repo reminded: {body}");
    assert!(!ids.contains(&"repo-fresh"), "fresh repo quiet: {body}");
    let stale = reminders
        .iter()
        .find(|e| e["repo_id"] == json!("repo-stale"))
        .unwrap();
    assert_eq!(stale["days_since_last_success"], json!(30));
    assert_eq!(stale["has_enabled_schedule"], json!(false));
}

/// Reminders require a session; restore-plan requires admin.
#[tokio::test]
async fn test_reminders_and_restore_plan_auth_split() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut r = restic_router(state.clone());
    let user = login(&mut r, "bob", "Password123!").await;

    let (status, _) = send(&mut r, Method::GET, "/api/restic/reminders", None, None).await;
    assert_ne!(status, StatusCode::OK);

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/restic/restore-plan?repo_id=x&snapshot_id=y&target=/tmp",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Restore-plan validates before shelling out: unknown repo and bad target
/// fail cleanly with no restic process spawned.
#[tokio::test]
async fn test_restore_plan_rejects_unknown_repo_and_bad_target() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "root2", "Password123!", 2).await;
    let mut r = restic_router(state.clone());
    let admin = login(&mut r, "root2", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/restic/restore-plan?repo_id=nope&snapshot_id=abc&target=/tmp/restore",
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(false));

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/restic/restore-plan?repo_id=&snapshot_id=&target=",
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(false));
}

// ---------------------------------------------------------------------------
// 8. Hardening — argument validation, run exclusion, stale-job reaper
//
// Every case below is refused *before* the restic CLI is reached: that is the
// point. A value that would become an argv entry (`--keep-last=0` as a
// snapshot id, `relative/path` as a backup source) never gets there.
// ---------------------------------------------------------------------------

/// Insert a restic_repos row with an explicit `source_paths` list.
async fn insert_repo_with_paths(state: &AppState, id: &str, source_paths: &[&str]) {
    let paths = serde_json::to_string(source_paths).unwrap();
    sqlx::query(
        "INSERT INTO restic_repos (id, name, repository, password, source_paths, snapshot_source, snapshot_create_before_backup, created_at, updated_at)
         VALUES (?, ?, '/nonexistent/repo', 'pw', ?, 'directory', 0, datetime('now'), datetime('now'))",
    )
    .bind(id)
    .bind(id)
    .bind(paths)
    .execute(&*state.db.pool())
    .await
    .expect("insert repo row");
}

#[tokio::test]
async fn test_repo_create_validates_before_touching_restic() {
    let (mut router, token, _tmp, _g) = admin_router().await;

    let cases: Vec<(Value, &str)> = vec![
        (
            json!({"name":"","repository":"/tmp/r","password":"p","source_paths":["/tmp/s"]}),
            "name is required",
        ),
        (
            json!({"name":"r","repository":"  ","password":"p","source_paths":["/tmp/s"]}),
            "repository location is required",
        ),
        (
            json!({"name":"r","repository":"/tmp/r","password":"","source_paths":["/tmp/s"]}),
            "password is required",
        ),
        (
            json!({"name":"r","repository":"/tmp/r","password":"p","source_paths":["relative/dir"]}),
            "absolute path",
        ),
        (
            json!({"name":"r","repository":"/tmp/r","password":"p","source_paths":["/tmp/s"],"retention_policy_id":"ghost"}),
            "retention policy not found",
        ),
        (
            json!({"name":"r","repository":"/tmp/r","password":"p","source_paths":["/tmp/s"],"snapshot_create_before_backup":true}),
            "snapshot pool is required",
        ),
        (
            json!({"name":"r","repository":"/tmp/r","password":"p","source_paths":["/tmp/s"],"snapshot_create_before_backup":true,"snapshot_pool":"zpool","snapshot_dataset":"tank/data"}),
            "absolute path",
        ),
    ];

    for (body, expected) in cases {
        let (status, resp) = send(
            &mut router,
            Method::POST,
            "/api/restic/repos",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {resp}");
        assert!(
            msg(&resp).to_lowercase().contains(&expected.to_lowercase()),
            "{body}: got {:?}",
            msg(&resp)
        );
    }
}

#[tokio::test]
async fn test_backup_validates_sources_and_refuses_a_busy_repo() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin8", "Password123!", 2).await;
    let mut router = restic_router(state.clone());
    let token = login(&mut router, "admin8", "Password123!").await;

    // A relative source path never reaches restic's argv.
    insert_repo_with_paths(&state, "rel-repo", &["relative/dir"]).await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/backup",
        Some(&token),
        Some(json!({"repo_id":"rel-repo"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(msg(&body).contains("absolute path"), "{}", msg(&body));

    // A repository already running a backup/restore is refused up front
    // instead of failing later with restic's "repository is already locked".
    insert_repo_with_paths(&state, "busy-repo", &["/tmp/src"]).await;
    let held = homevault::restic::RepoRunLock::acquire("busy-repo").expect("take the run slot");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/backup",
        Some(&token),
        Some(json!({"repo_id":"busy-repo"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(msg(&body).contains("already running"), "{}", msg(&body));
    drop(held);
    assert!(!homevault::restic::RepoRunLock::is_busy("busy-repo"));
}

#[tokio::test]
async fn test_forget_requires_a_rule_and_a_valid_keep_within() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin7", "Password123!", 2).await;
    let mut router = restic_router(state.clone());
    let token = login(&mut router, "admin7", "Password123!").await;
    insert_repo(&state, "repo-forget", "repo-forget").await;

    // No rule at all: restic would answer "Fatal: no policy was specified".
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/forget",
        Some(&token),
        Some(json!({"repo_id":"repo-forget"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(msg(&body).contains("retention rule"), "{}", msg(&body));

    // A malformed duration is refused here, not by restic.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/forget",
        Some(&token),
        Some(json!({"repo_id":"repo-forget","keep_within":"30 days"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        msg(&body).to_lowercase().contains("keep-within"),
        "{}",
        msg(&body)
    );
}

#[tokio::test]
async fn test_snapshot_delete_and_restore_reject_option_injection() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin6", "Password123!", 2).await;
    let mut router = restic_router(state.clone());
    let token = login(&mut router, "admin6", "Password123!").await;
    insert_repo(&state, "repo-inject", "repo-inject").await;

    // `restic forget --keep-last=0` would wipe the whole repository.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/snapshots/delete",
        Some(&token),
        Some(json!({"repo_id":"repo-inject","snapshot_id":"--keep-last=0"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        msg(&body).contains("cannot start with '-'"),
        "{}",
        msg(&body)
    );

    // A relative restore target would be resolved against the server's cwd.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/restore",
        Some(&token),
        Some(json!({"repo_id":"repo-inject","snapshot_id":"abc123","target":"relative/target"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(msg(&body).contains("absolute path"), "{}", msg(&body));

    // A snapshot id that looks like a flag is not a snapshot id.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/restore",
        Some(&token),
        Some(json!({"repo_id":"repo-inject","snapshot_id":"--target","target":"/tmp/restore"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        msg(&body).contains("cannot start with '-'"),
        "{}",
        msg(&body)
    );
}

#[tokio::test]
async fn test_check_read_data_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "reg3", "Password123!", 1).await;
    let mut router = restic_router(state.clone());
    let token = login(&mut router, "reg3", "Password123!").await;
    insert_repo(&state, "repo-check", "repo-check").await;

    // `--read-data` re-reads every pack: hours of I/O an ordinary session
    // must not be able to trigger on demand.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/restic/check",
        Some(&token),
        Some(json!({"repo_id":"repo-check","read_data":true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The metadata-only check stays available to a plain session (it fails
    // here only because restic is not installed on the test host).
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/check",
        Some(&token),
        Some(json!({"repo_id":"repo-check"})),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn test_retention_policy_rejects_zero_and_a_second_default() {
    let (mut router, token, _tmp, _g) = admin_router().await;

    // `max_count = 0` would mean "forget every snapshot".
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/retention-policies",
        Some(&token),
        Some(json!({"name":"zero","max_count":0})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(msg(&body).contains("greater than 0"), "{}", msg(&body));

    // A policy with no limit at all would never prune anything.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/retention-policies",
        Some(&token),
        Some(json!({"name":"none"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        msg(&body).contains("At least one retention limit"),
        "{}",
        msg(&body)
    );

    // Only one policy may be the `apply_to_all` default.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/restic/retention-policies",
        Some(&token),
        Some(json!({"name":"default-a","max_count":7,"apply_to_all":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/restic/retention-policies",
        Some(&token),
        Some(json!({"name":"default-b","max_count":3,"apply_to_all":true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(msg(&body).contains("already the default"), "{}", msg(&body));
}

#[tokio::test]
async fn test_stale_running_jobs_are_closed_and_snapshot_id_is_recorded() {
    use homevault::storage::restic_repo::ResticRepository;

    let (state, _tmp, _g) = setup_state().await;
    insert_repo(&state, "repo-jobs", "repo-jobs").await;
    let repo = ResticRepository::new(state.db.pool());

    // A job left `running` by a crash, and one that completed normally.
    let stale = repo
        .create_backup_job("repo-jobs", None, "/tmp/src", "manual")
        .await
        .expect("create stale job");
    let done = repo
        .create_backup_job("repo-jobs", None, "/tmp/src", "scheduled")
        .await
        .expect("create completed job");
    repo.update_backup_job(
        &done,
        "completed",
        Some("da0f81d6"),
        1,
        0,
        0,
        10,
        1,
        10,
        1,
        None,
    )
    .await
    .expect("close the completed job");

    let closed = repo.fail_stale_running_jobs().await.expect("reap");
    assert_eq!(closed, 1, "only the running job is closed");

    let jobs = repo.list_backup_jobs("repo-jobs", 10).await.unwrap();
    let stale_job = jobs.iter().find(|j| j.id == stale).expect("stale row");
    assert_eq!(stale_job.status, "failed");
    assert!(stale_job
        .error_message
        .as_deref()
        .unwrap_or("")
        .contains("Interrupted"));
    assert!(stale_job.finished_at.is_some());

    let done_job = jobs.iter().find(|j| j.id == done).expect("done row");
    assert_eq!(done_job.status, "completed");
    // The job row is created before the run, so the snapshot id is filled in
    // on completion rather than at creation.
    assert_eq!(done_job.snapshot_id.as_deref(), Some("da0f81d6"));
}
