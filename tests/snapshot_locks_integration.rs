/* =============================================================================
 * File          : snapshot_locks_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Immutable-snapshot lock integration tests: the auth matrix, the read
 *   shape, every validation path that must be refused *before* a host command,
 *   and the break-glass confirmation rule enforced by the API.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test snapshot_locks_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for immutable snapshots (`crate::snapshot_locks`,
//! `crate::handlers::snapshot_locks`):
//!
//!   GET    /api/snapshots/locks
//!   POST   /api/snapshots/locks
//!   DELETE /api/snapshots/locks/{id}
//!
//! The pool itself is a real ZFS/Btrfs dataset, which neither exists nor can be
//! created in a test sandbox: the filesystem-side `zfs hold`/`btrfs property`
//! calls are covered by the unit tests that build their exact argv, and by the
//! E2E spec against a real pool. This suite covers everything above that line —
//! auth, validation, persistence and the break-glass rule.

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
use homevault::handlers::snapshot_locks::{
    handle_snapshot_lock_create, handle_snapshot_lock_release, handle_snapshot_locks_list,
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
            "/api/snapshots/locks",
            get(handle_snapshot_locks_list).post(handle_snapshot_lock_create),
        )
        .route(
            "/api/snapshots/locks/{id}",
            axum::routing::delete(handle_snapshot_lock_release),
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

/// Insert an active lock row directly: the filesystem hold cannot exist in a
/// sandbox, but the API's release rules operate on the row and are what these
/// tests exercise.
async fn insert_lock(h: &Harness, snapshot: &str, locked_until: &str) -> i64 {
    sqlx::query(
        "INSERT INTO snapshot_locks (pool, snapshot, tag, locked_until, reason, created_by, created_at) \
         VALUES ('tank', ?, 'homevault-lock', ?, 'test', 'admin', datetime('now'))",
    )
    .bind(snapshot)
    .bind(locked_until)
    .execute(&*h.state.db.pool())
    .await
    .expect("insert lock")
    .last_insert_rowid()
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reading_locks_needs_a_session_and_mutating_them_needs_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(&mut router, Method::GET, "/api/snapshots/locks", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A member may read the lock state…
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshots/locks",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // …but not place or release a lock.
    for (method, uri, body) in [
        (
            Method::POST,
            "/api/snapshots/locks",
            Some(json!({"pool": "tank", "snapshot": "snap-1", "days": 30})),
        ),
        (Method::DELETE, "/api/snapshots/locks/1", None),
    ] {
        let (status, _) = send(&mut router, method.clone(), uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Read shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_lock_list_carries_the_break_glass_phrase_and_the_history() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    insert_lock(&h, "tank@snap-1", "2099-01-01T00:00:00+00:00").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshots/locks?pool=tank",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["locks"][0]["snapshot"], json!("tank@snap-1"));
    assert_eq!(body["data"]["locks"][0]["expired"], json!(false));
    assert_eq!(
        body["data"]["locks"][0]["break_glass_phrase"],
        json!("release snap-1"),
        "the phrase is shown so the admin can type it deliberately: {body}"
    );
    assert_eq!(body["data"]["tag"], json!("homevault-lock"));
    assert!(body["data"]["history"].is_array());

    // A pool filter that matches nothing returns no lock, not an error.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/snapshots/locks?pool=other",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["locks"], json!([]));
}

// ---------------------------------------------------------------------------
// Validation happens before any host command
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_invalid_lock_requests_are_refused_before_the_pool_is_touched() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    for body in [
        // A traversal or an option-like snapshot name never reaches argv.
        json!({"pool": "tank", "snapshot": "../../etc", "days": 30}),
        json!({"pool": "tank", "snapshot": "-destroy", "days": 30}),
        json!({"pool": "tank", "snapshot": "a/b", "days": 30}),
        // A zero, negative or absurd retention.
        json!({"pool": "tank", "snapshot": "snap-1", "days": 0}),
        json!({"pool": "tank", "snapshot": "snap-1", "days": -1}),
        json!({"pool": "tank", "snapshot": "snap-1", "days": 3651}),
        // A reason that would break a single journal line.
        json!({"pool": "tank", "snapshot": "snap-1", "days": 30, "reason": "a\nb"}),
    ] {
        let (status, response) = send(
            &mut router,
            Method::POST,
            "/api/snapshots/locks",
            Some(&admin),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {response}");
        // Nothing was persisted.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM snapshot_locks")
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{body} must not insert a row");
    }
}

#[tokio::test]
async fn test_an_unknown_pool_or_snapshot_fails_without_claiming_a_lock() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // No such pool on this host: the hold cannot be placed, so the row must
    // not be written either (a row would claim protection that does not exist).
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/snapshots/locks",
        Some(&admin),
        Some(json!({"pool": "no-such-pool", "snapshot": "snap-1", "days": 30})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM snapshot_locks")
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "no row may be written when the hold failed");
}

// ---------------------------------------------------------------------------
// Release / break-glass
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_releasing_before_the_horizon_requires_the_typed_phrase() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let id = insert_lock(&h, "tank@snap-1", "2099-01-01T00:00:00+00:00").await;

    // No confirmation: refused, with the exact phrase in the message.
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/snapshots/locks/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("release snap-1"), "{body}");

    // A near-miss is refused too.
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/snapshots/locks/{id}"),
        Some(&admin),
        Some(json!({"confirmation": "release tank@snap-1"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // The row is still active after two refused attempts.
    let released: Option<String> =
        sqlx::query_scalar("SELECT released_at FROM snapshot_locks WHERE id = ?")
            .bind(id)
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert!(released.is_none());
}

#[tokio::test]
async fn test_a_released_lock_cannot_be_released_twice() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let id = insert_lock(&h, "tank@snap-1", "2099-01-01T00:00:00+00:00").await;
    sqlx::query("UPDATE snapshot_locks SET released_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(&*h.state.db.pool())
        .await
        .unwrap();

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/snapshots/locks/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("already released"),
        "{body}"
    );
}

#[tokio::test]
async fn test_deleting_an_unknown_lock_is_a_clean_error() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/snapshots/locks/424242",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no longer exists"),
        "{body}"
    );
}
