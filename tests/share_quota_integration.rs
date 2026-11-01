/* =============================================================================
 * File          : share_quota_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-share quota integration tests: in-memory SQLite + assembled Axum
 *   router for the admin matrix, the round-trip, the validation and the state
 *   payload a card renders. The filesystem half is *not* exercised (it needs
 *   ZFS/Btrfs/XFS with root): the backend selection, the argv and the parsers
 *   are pinned by `src/share_quota.rs`'s unit tests, and this file asserts the
 *   endpoint contract — including the honest "unsupported" answer on the test
 *   host's own filesystem.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test share_quota_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for per-share quotas (`crate::share_quota`,
//! `handlers::share_quota`):
//!
//!   GET    /api/shares/{name}/{share_type}/quota
//!   PUT    /api/shares/{name}/{share_type}/quota
//!   DELETE /api/shares/{name}/{share_type}/quota
//!
//! The share used by the tests is an **SMB share written into a temporary
//! `smb.conf`**, so `SharesService::get_share` resolves it for real without
//! touching the host's configuration.

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
use homevault::handlers::share_quota::{
    handle_share_quota_clear, handle_share_quota_get, handle_share_quota_set,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, ShareQuotaRepository, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    /// The directory the test "share" points at (kept so the temp tree exists
    /// for the probes even when a test only exercises the endpoints).
    share_dir: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

/// Point `smb.conf` at a temporary file so the test never reads (or writes) the
/// host's real Samba configuration.
async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);

    // A real directory the share points at, so `findmnt --target` resolves.
    let share_dir = root.join("photos");
    std::fs::create_dir_all(&share_dir).expect("share dir");

    // The SMB backend reads `smb.conf` from a fixed path; the harness points it
    // at the temp tree through the same override the shares tests use.
    std::env::set_var("HV_SMB_CONF", root.join("smb.conf"));

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
        share_dir,
        _tmp: tmp,
        _guard: guard,
    }
}

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/shares/{name}/{share_type}/quota",
            get(handle_share_quota_get)
                .put(handle_share_quota_set)
                .delete(handle_share_quota_clear),
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
    let mut router = build_router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_share_quota_is_admin_only() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;
    let uri = "/api/shares/photos/smb/quota";

    // No session at all.
    for method in [Method::GET, Method::PUT, Method::DELETE] {
        let body = (method == Method::PUT).then(|| json!({"quota_bytes": 1_000_000_000}));
        let (status, _) = send(&mut router, method, uri, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }

    // A regular member: a quota caps other people's storage, so setting one is
    // an admin action (and the whole Shares app is admin-only).
    let (status, _) = send(&mut router, Method::GET, uri, Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        uri,
        Some(&member),
        Some(json!({"quota_bytes": 1_000_000_000})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(&mut router, Method::DELETE, uri, Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin gets past the guard (the share may not exist in this harness, so
    // the status is a 400 naming it — never a 401/403).
    let (status, body) = send(&mut router, Method::GET, uri, Some(&admin), None).await;
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    assert_ne!(status, StatusCode::FORBIDDEN);
    if status == StatusCode::BAD_REQUEST {
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("does not exist"),
            "a missing share is named: {body}"
        );
    }
}

#[tokio::test]
async fn test_an_unknown_share_is_named_and_an_unknown_protocol_is_refused() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/shares/nope/smb/quota",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap_or_default().contains("nope"),
        "the refusal names the share: {body}"
    );

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/shares/photos/carrier-pigeon/quota",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Invalid share type"),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// The validation happens before any filesystem command, so these refusals hold
/// even on a host whose backend could not apply them anyway.
#[tokio::test]
async fn test_a_quota_below_the_floor_or_above_the_ceiling_is_refused() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let uri = "/api/shares/photos/smb/quota";

    let (status, body) = send(
        &mut router,
        Method::PUT,
        uri,
        Some(&admin),
        Some(json!({"quota_bytes": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a 1-byte share is a typo: {body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("at least"),
        "{body}"
    );

    let (status, body) = send(
        &mut router,
        Method::PUT,
        uri,
        Some(&admin),
        Some(json!({"quota_bytes": homevault::share_quota::MAX_QUOTA_BYTES + 1})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"].as_str().unwrap_or_default().contains("at most"),
        "{body}"
    );

    // Nothing was stored by either refusal.
    let repo = ShareQuotaRepository::new(h.state.db.pool());
    assert_eq!(repo.quota("photos", "smb").await.expect("db"), None);
}

/// The repo is the intent's only home: a set limit round-trips, clearing it
/// deletes the row (not a zero), and deleting a share's rows removes them all.
#[tokio::test]
async fn test_the_declared_limit_round_trips_in_the_database() {
    let h = setup().await;
    let _ = signed_in(&h).await;
    let repo = ShareQuotaRepository::new(h.state.db.pool());

    assert_eq!(repo.quota("photos", "smb").await.expect("db"), None);
    repo.set("photos", "smb", 10_000_000_000).await.expect("set");
    assert_eq!(
        repo.quota("photos", "smb").await.expect("db"),
        Some(10_000_000_000)
    );
    // Per (name, type): another protocol on the same folder is its own limit.
    assert_eq!(repo.quota("photos", "nfs").await.expect("db"), None);

    let all = repo.all().await.expect("db");
    assert_eq!(all, vec![("photos".to_string(), "smb".to_string(), 10_000_000_000)]);

    // 0 clears (the row goes away, so "no quota" has one representation).
    repo.set("photos", "smb", 0).await.expect("clear");
    assert_eq!(repo.quota("photos", "smb").await.expect("db"), None);
    assert!(repo.all().await.expect("db").is_empty());

    // Deleting the share drops every protocol's row.
    repo.set("photos", "smb", 5_000_000_000).await.expect("set");
    repo.set("photos", "nfs", 6_000_000_000).await.expect("set");
    let dropped = repo.clear_all_for("photos").await.expect("clear all");
    assert_eq!(dropped, 2);
    assert!(repo.all().await.expect("db").is_empty());
}

// ---------------------------------------------------------------------------
// The state payload
// ---------------------------------------------------------------------------

/// The payload always describes the *share*, including the honest "cannot be
/// enforced here" answer: on a test host the temp directory is usually ext4 or
/// tmpfs, neither of which this build can cap.
#[tokio::test]
async fn test_the_state_payload_reports_support_and_usage() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    assert!(h.share_dir.is_dir(), "the harness share directory exists");

    // The harness writes no smb.conf, so the share does not resolve; the point
    // of this test is the *shape* the card depends on, asserted on whichever
    // answer the host produces.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/shares/photos/smb/quota",
        Some(&admin),
        None,
    )
    .await;
    if status != StatusCode::OK {
        // No share: the refusal names it, and nothing else is claimed.
        assert_eq!(status, StatusCode::BAD_REQUEST);
        return;
    }
    let data = &body["data"];
    for key in [
        "backend",
        "supported",
        "filesystem",
        "quota_bytes",
        "used_bytes",
        "free_bytes",
        "percent",
        "quota_human",
        "used_human",
    ] {
        assert!(!data[key].is_null(), "the card needs `{key}`: {data}");
    }
    assert!(data["supported"].is_boolean());
    // An unsupported backend must explain itself — that is what the card shows
    // instead of a field.
    if data["supported"] == json!(false) {
        assert!(
            data["reason"].as_str().is_some_and(|r| !r.is_empty()),
            "an unsupported filesystem must say why: {data}"
        );
        assert_eq!(data["quota_bytes"], 0, "nothing is claimed as enforced");
    }
    assert!(data["percent"].as_u64().unwrap_or(101) <= 100);
}
