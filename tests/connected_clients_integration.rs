/* =============================================================================
 * File          : connected_clients_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Connected-clients integration tests: in-memory SQLite + assembled Axum
 *   router. Covers the admin matrix, the "always 200 with per-source status"
 *   contract on a box that has none of the collector tools, the empty-list
 *   shape, and the kick refusals (unknown id, NFS, empty id) — no process and
 *   no `ufw`/`smbcontrol` is ever invoked.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test connected_clients_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for "who is connected" (`crate::connected_clients`):
//!
//!   GET  /api/security/connected-clients
//!   POST /api/security/connected-clients/kick
//!
//! The parsers are unit-tested with real `smbstatus -j` / `showmount -a` /
//! `ss -tnp` samples; these tests own the endpoint contract. They deliberately
//! run on a host where the tools may be missing: that is the case the contract
//! is about — the endpoint must answer `200` with one status per source instead
//! of failing the whole list.
//!
//! Run:
//!   cargo test --test connected_clients_integration

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
use homevault::handlers::connected_clients::{
    handle_connected_client_kick, handle_connected_clients_list,
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

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/security/connected-clients",
            get(handle_connected_clients_list),
        )
        .route(
            "/api/security/connected-clients/kick",
            post(handle_connected_client_kick),
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
async fn test_connected_clients_is_admin_only() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/security/connected-clients",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/security/connected-clients/kick",
        None,
        Some(json!({"id": "smb:1:10.0.0.1"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The list names every peer and every share on the appliance: a regular
    // member must not read it, let alone disconnect anyone.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/security/connected-clients",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/security/connected-clients/kick",
        Some(&member),
        Some(json!({"id": "smb:1:10.0.0.1"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // And the admin gets the list.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/security/connected-clients",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "an admin reads the list: {body}");
}

// ---------------------------------------------------------------------------
// The contract: always 200, with a status per source
// ---------------------------------------------------------------------------

/// A box with none of the tools installed still answers: three sources, each
/// saying whether it was available. An empty list is a fact, not a failure.
#[tokio::test]
async fn test_the_list_always_answers_with_a_status_per_source() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/security/connected-clients",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let sources = body["data"]["sources"].as_array().expect("a source list");
    assert_eq!(sources.len(), 3, "smb, nfs and the stream collector");
    let names: Vec<&str> = sources
        .iter()
        .filter_map(|s| s["source"].as_str())
        .collect();
    assert_eq!(names, vec!["smb", "nfs", "sessions"]);

    for source in sources {
        // Every source says whether it answered…
        assert!(
            source["available"].is_boolean(),
            "each source carries its availability: {source}"
        );
        // …and when it did not, it carries the reason, so the card can explain
        // an empty section instead of showing nothing.
        if source["available"] == json!(false) {
            assert!(
                source["error"].as_str().is_some_and(|e| !e.is_empty()),
                "an unavailable source must say why: {source}"
            );
        }
        assert!(source["count"].is_number());
    }

    // The rest of the envelope is always present.
    assert!(body["data"]["clients"].is_array());
    assert!(body["data"]["kickable"].is_number());
    assert!(body["data"]["checked_at"]
        .as_str()
        .is_some_and(|s| s.contains('T')));
}

/// Nothing in the payload may be a path from `smbstatus`'s `open_files`: the
/// card answers "who is connected", not "what are they reading".
#[tokio::test]
async fn test_the_list_never_leaks_open_file_paths() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/security/connected-clients",
        Some(&admin),
        None,
    )
    .await;
    let rendered = body.to_string();
    // The test host's own CWD must never appear as an "open file".
    if let Ok(cwd) = std::env::current_dir() {
        let needle = cwd.to_string_lossy().to_string();
        assert!(!rendered.contains(&needle), "leaked a real path: {needle}");
    }
    assert!(!rendered.contains("/srv/photos"));
}

// ---------------------------------------------------------------------------
// Kick refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_kick_refuses_an_unknown_client() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // An id that is not in the current collection is a 400 with a sentence that
    // tells the admin to refresh — never a silent success, and never a signal to
    // a pid the request supplied.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/security/connected-clients/kick",
        Some(&admin),
        Some(json!({"id": "smb:999999:203.0.113.9"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("is connected right now"),
        "the refusal says the client is not there: {message}"
    );
    assert!(
        message.contains("refresh"),
        "…and what to do about it: {message}"
    );
}

#[tokio::test]
async fn test_kick_refuses_an_empty_id() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/security/connected-clients/kick",
        Some(&admin),
        Some(json!({"id": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}

/// The pure refusal the endpoint maps to a 400: a NFS export cannot be dropped
/// from here, and the message says what to do instead.
#[test]
fn test_an_nfs_client_cannot_be_kicked_and_says_so() {
    let error = homevault::connected_clients::KickError::NotKickable("nfs".to_string());
    let message = error.message();
    assert!(message.contains("cannot be disconnected"), "{message}");
    assert!(message.contains("kernel"), "{message}");
}
