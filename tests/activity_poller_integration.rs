/* =============================================================================
 * File          : activity_poller_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 24/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the live-hardware activity poller (ROADMAP 28.3):
 *   a real `/proc/mdstat` written to a temp file is parsed and reconciled into
 *   the process-global `PROGRESS_MANAGER`, and the resulting rows are read back
 *   through the real `GET /api/activity` endpoint, so the contract under test is
 *   the one the desktop taskbar consumes.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test activity_poller_integration
 * =============================================================================
 */

// Integration tests hold a process-wide `ENV_LOCK` guard across `await` points
// to serialize tests that mutate process environment variables and the
// process-global `PROGRESS_MANAGER`.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the activity poller (ROADMAP 28.3, phase 2).
//!
//! The poller is the half of the activity feed that turns *live host state*
//! into progress rows: an mdadm resync, a ZFS/btrfs scrub, a Syncthing folder
//! still catching up. The synthetic job is created and advanced by a
//! background loop, but every rule that matters is observable through
//! `GET /api/activity` — which is what these tests exercise, with a real
//! `/proc/mdstat` fixture on disk instead of a mocked parser.
//!
//! Rules under test:
//!
//!   1. A resync on a **continuation line** becomes one row whose `kind` is
//!      `raid_resync` and whose `progress` is the percentage the kernel
//!      printed — the defect that would make a "live" taskbar empty.
//!   2. The row is **one** job across polls: a second tick with a higher
//!      percentage advances the same id instead of creating a new row (and
//!      never completes the old one).
//!   3. An operation that ends (the array leaves `/proc/mdstat`) leaves the
//!      feed, and the poller never completes a job it did not create.
//!   4. A source that cannot be read contributes nothing, and the endpoint
//!      stays `200`.
//!
//! Run: `cargo test --test activity_poller_integration`

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::activity_poller::{
    plan_reconcile, tick, ScrubKind, KIND_RAID, SYNTHETIC_PREFIX,
};
use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{activity::handle_activity, auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::middleware::security_headers_middleware;
use homevault::sse::{JobType, PROGRESS_MANAGER};
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// A `/proc/mdstat` with one array resyncing at `pct`.
fn mdstat_resync(pct: &str) -> String {
    format!(
        "Personalities : [raid1] [raid6] [raid5] [raid4]\n\
         md0 : active raid1 sdb1[1] sda1[0]\n\
         976630464 blocks super 1.2 [2/2] [UU]\n\
         [==>..................]  resync = {pct}% (123119104/976630464) finish=118.4min speed=135000K/sec\n\
         \n\
         unused devices: <none>\n"
    )
}

/// Wipe the process-global job registry so a test starts from a known state.
fn clear_jobs() {
    for (id, _, _) in PROGRESS_MANAGER.list_active_jobs() {
        PROGRESS_MANAGER.remove_job(&id);
    }
}

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    clear_jobs();
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

fn activity_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/activity", get(handle_activity))
        .with_state(state)
        .layer(axum::middleware::from_fn(security_headers_middleware))
}

async fn create_test_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', ?, false, datetime('now'), 'seed')",
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
    let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, val)
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

fn rows(body: &Value) -> &Vec<Value> {
    body["data"].as_array().expect("data array")
}

fn row_by_id<'a>(body: &'a Value, id: &str) -> Option<&'a Value> {
    rows(body).iter().find(|r| r["id"] == json!(id))
}

/// Run one poller tick against a fixture file, with no pools and no Syncthing.
async fn run_tick(mdstat: std::path::PathBuf, known: &HashSet<String>) -> HashSet<String> {
    let plan = tick(known, &mdstat, &[], false).await;
    plan.live.iter().map(|o| o.id.clone()).collect()
}

// ---------------------------------------------------------------------------
// A resync becomes a row the taskbar can render
// ---------------------------------------------------------------------------

/// ROADMAP 28.3's assertion. `/proc/mdstat` prints the percentage on a
/// continuation line, not on the `md0 : active raid1 …` header. A poller that
/// read only header lines reported every resync as idle — the exact failure of
/// a "live" taskbar that stays empty while the disks grind.
#[tokio::test]
async fn a_resync_becomes_a_live_row_with_its_percentage() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");
    std::fs::write(&mdstat, mdstat_resync("12.6")).unwrap();

    let known = run_tick(mdstat, &HashSet::new()).await;
    assert_eq!(known.len(), 1, "one array, one operation, one row");

    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let row = row_by_id(&body, "sys:raid:md0:resync").expect("the resync row");
    assert_eq!(row["kind"], json!(KIND_RAID));
    assert_eq!(row["state"], json!("running"));
    assert_eq!(row["progress"], json!(12.6), "the kernel's percentage");
    assert!(
        row["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("md0"),
        "the row names the array: {row}"
    );
}

/// The same operation across two polls is **one** row that advances, not a new
/// row per poll. A poller that minted a fresh id each tick would fill the panel
/// with a ghost row per five seconds and no bar would ever move.
#[tokio::test]
async fn a_second_tick_advances_the_same_row() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");

    std::fs::write(&mdstat, mdstat_resync("12.6")).unwrap();
    let known = run_tick(mdstat.clone(), &HashSet::new()).await;

    std::fs::write(&mdstat, mdstat_resync("48.0")).unwrap();
    let known_after = run_tick(mdstat, &known).await;
    assert_eq!(known, known_after, "the id is stable across polls");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    let resync_rows: Vec<&Value> = rows(&body)
        .iter()
        .filter(|r| r["kind"] == json!(KIND_RAID))
        .collect();
    assert_eq!(
        resync_rows.len(),
        1,
        "one operation is one row, whatever the poll count: {body}"
    );
    assert_eq!(resync_rows[0]["progress"], json!(48.0));
}

/// When the array finishes, the row leaves the feed — an activity row that
/// never ends is worse than no row at all.
#[tokio::test]
async fn a_finished_resync_leaves_the_feed() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");

    std::fs::write(&mdstat, mdstat_resync("99.0")).unwrap();
    let known = run_tick(mdstat.clone(), &HashSet::new()).await;

    // The resync finished: the array is active but reports no operation.
    std::fs::write(
        &mdstat,
        "Personalities : [raid1]\n\
         md0 : active raid1 sdb1[1] sda1[0]\n\
         976630464 blocks super 1.2 [2/2] [UU]\n\
         \n\
         unused devices: <none>\n",
    )
    .unwrap();
    let known_after = run_tick(mdstat, &known).await;
    assert!(known_after.is_empty(), "nothing is running any more");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert!(
        row_by_id(&body, "sys:raid:md0:resync").is_none(),
        "a finished resync must not linger: {body}"
    );
}

/// A delayed resync has not started; a row for it would sit at 0 % forever.
#[tokio::test]
async fn a_delayed_resync_is_not_a_row() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");
    std::fs::write(
        &mdstat,
        "md0 : active raid1 sdb1[1] sda1[0]\n\
         976630464 blocks super 1.2 [2/2] [UU]\n\
         [>....................]  resync = DELAYED\n",
    )
    .unwrap();

    let known = run_tick(mdstat, &HashSet::new()).await;
    assert!(known.is_empty(), "a delayed resync is not work in progress");

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert!(!body.to_string().contains("raid_resync"), "{body}");
}

// ---------------------------------------------------------------------------
// Safety rules
// ---------------------------------------------------------------------------

/// An unreadable source is a gap, never a fabricated value — and never a 500
/// on the endpoint the taskbar polls.
#[tokio::test]
async fn a_missing_mdstat_is_an_empty_feed_not_an_error() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let missing = dir.path().join("no-such-mdstat");

    let known = run_tick(missing, &HashSet::new()).await;
    assert!(known.is_empty());

    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows(&body).is_empty(), "{body}");
}

/// The poller must never complete a job it did not create: a real backup (or
/// any other handler's job) is not a scrub, even if it is "known".
#[tokio::test]
async fn a_foreign_job_is_never_swept() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let backup = PROGRESS_MANAGER.create_job(JobType::Backup, "Starting backup");
    PROGRESS_MANAGER.emit_progress(&backup, 30.0, "30/100 files");
    let mut known: HashSet<String> = known_ids();
    known.insert(backup.clone());

    let dir = tempfile::tempdir().expect("fixture dir");
    let missing = dir.path().join("no-such-mdstat");
    run_tick(missing, &known).await;

    let (_, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    let row = row_by_id(&body, &backup).expect("the backup is untouched by the poller");
    assert_eq!(row["progress"], json!(30.0));
    assert!(
        !backup.starts_with(SYNTHETIC_PREFIX),
        "the poller's namespace must not collide with a handler job id"
    );
}

/// Every synthetic id carries the namespace prefix, so the sweep can tell its
/// own rows from the rest of the registry.
#[tokio::test]
async fn synthetic_ids_are_namespaced() {
    let (_state, _tmp, _g) = setup_state().await;
    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");
    std::fs::write(&mdstat, mdstat_resync("5.0")).unwrap();

    let known = run_tick(mdstat, &HashSet::new()).await;
    assert!(!known.is_empty());
    for id in known {
        assert!(
            id.starts_with(SYNTHETIC_PREFIX),
            "{id} must be namespaced so a sweep cannot touch a handler's job"
        );
    }
}

// ---------------------------------------------------------------------------
// Reconciliation is pure and testable without hardware
// ---------------------------------------------------------------------------

/// The plan for an operation that ended contains exactly the id the previous
/// tick created — one create, one complete, never a per-poll row.
#[tokio::test]
async fn an_ended_operation_is_planned_for_removal() {
    let (_state, _tmp, _g) = setup_state().await;
    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");
    std::fs::write(&mdstat, mdstat_resync("40.0")).unwrap();

    let known = run_tick(mdstat, &HashSet::new()).await;
    assert!(!known.is_empty());

    // Nothing live any more: the plan must name the id for removal.
    let plan = plan_reconcile(&known, vec![]);
    assert!(plan.live.is_empty());
    assert_eq!(plan.finished.len(), known.len());
}

/// A pool list with nothing scrubbing produces no rows and stays 200 — the
/// ZFS/btrfs branch is exercised end-to-end without a ZFS host by handing it an
/// empty list (the probe returns nothing without spawning anything).
#[tokio::test]
async fn no_scrubbing_pool_produces_no_row() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut r = activity_router(state);
    let token = login(&mut r, "admin1", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");
    std::fs::write(&mdstat, "md0 : active raid1 sda1[0]\n").unwrap();

    let pools: Vec<(String, ScrubKind)> = vec![];
    let plan = tick(&HashSet::new(), &mdstat, &pools, false).await;
    assert!(plan.live.is_empty());

    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rows(&body).is_empty(), "{body}");
}

/// The synthetic row is only visible to an admin-owned feed, exactly like every
/// other ownerless job: a non-admin must not see host-wide hardware rows.
#[tokio::test]
async fn a_synthetic_row_is_hidden_from_a_non_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Password123!", 1).await; // non-admin
    let mut r = activity_router(state);
    let token = login(&mut r, "alice", "Password123!").await;

    let dir = tempfile::tempdir().expect("fixture dir");
    let mdstat = dir.path().join("mdstat");
    std::fs::write(&mdstat, mdstat_resync("70.0")).unwrap();
    let known = run_tick(mdstat, &HashSet::new()).await;
    assert!(!known.is_empty(), "the row exists");

    let (status, body) = send(&mut r, Method::GET, "/api/activity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        rows(&body).is_empty(),
        "an ownerless hardware row is admin-only: {body}"
    );
}

/// The synthetic ids the manager currently tracks, for seeding the *previous*
/// tick by hand.
fn known_ids() -> HashSet<String> {
    PROGRESS_MANAGER
        .list_active_jobs()
        .into_iter()
        .map(|(id, _, _)| id)
        .filter(|id| id.starts_with(SYNTHETIC_PREFIX))
        .collect()
}
