/* =============================================================================
 * File          : iso_mount_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   ISO mounting integration tests: the auth matrix, the capability probe, and
 *   every refusal that can be decided before a syscall.
 *
 *   Nothing here mounts anything. The test host has no passwordless sudo, and
 *   a real `losetup`/`mount` would leave a device attached; every case asserts
 *   a decision the handler makes *before* it shells out. A successful mount is
 *   covered by the pure unit tests in `src/iso_mount.rs`, where the argv and the
 *   mount-point containment are asserted directly.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test iso_mount_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    routing::{delete, get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::iso_mount::{handle_iso_list, handle_iso_mount, handle_iso_unmount};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    root: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);

    // The Explorer's roots come from the path guard; the ISO feature is
    // bounded to exactly those, so the test seeds one real folder to hold
    // images in.
    let images = root.join("images");
    std::fs::create_dir_all(&images).expect("images dir");

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    let path_guard = PathGuard::new(vec![images.to_string_lossy().to_string()]);
    let admin_rate_limiter = Arc::new(RateLimitState::new(1000, 60));
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
        root,
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/explorer/iso",
            get(handle_iso_list).post(handle_iso_mount),
        )
        .route("/api/explorer/iso/{id}", delete(handle_iso_unmount))
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

/// Writes a file with the given name under the seeded images folder.
fn seed_image(h: &Harness, name: &str) -> std::path::PathBuf {
    let path = h.root.join("images").join(name);
    std::fs::write(&path, b"not a real iso").expect("write image");
    path
}

// ---------------------------------------------------------------------------
// Access control (P0.2: reads auth, mutations admin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_needs_a_session_and_mutations_need_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(&mut router, Method::GET, "/api/explorer/iso", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/explorer/iso",
        Some(&member),
        None,
    )
    .await;
    // The capability probe may be false on the test host, but a member must be
    // allowed past the guard to read it.
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        None,
        Some(json!({"path": "/whatever.iso"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&member),
        Some(json!({"path": "/whatever.iso"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::DELETE,
        "/api/explorer/iso/some-id",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// The guard runs **before** the body is parsed, so an unauthenticated caller
/// gets `401` and never a `422` that echoes the schema (P0.2).
#[tokio::test]
async fn test_a_malformed_body_without_a_session_is_still_401() {
    let h = setup().await;
    let (mut router, _admin, _member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        None,
        Some(json!({"not_path": 42})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Payload shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_reports_capability_and_an_empty_mount_table() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/explorer/iso",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["available"].is_boolean(), "{body}");
    assert!(
        data["iso_root"]
            .as_str()
            .unwrap_or_default()
            .ends_with("iso"),
        "the root must be the appliance-managed iso/ directory: {body}"
    );
    assert_eq!(data["mounts"], json!([]), "{body}");
}

/// A malformed body *with* a session is `400` — the one case where `400` is
/// the right answer (P0.2: "your request is wrong").
#[tokio::test]
async fn test_a_malformed_body_with_a_session_is_400() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&admin),
        Some(json!({"not_path": 42})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Malformed"),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals decided before any syscall
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_an_image_outside_the_allowed_roots_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // A real file the caller may not read: the refusal must be about the root,
    // not about existence.
    let outside = h.root.join("outside.iso");
    std::fs::write(&outside, b"x").unwrap();

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&admin),
        Some(json!({"path": outside.to_string_lossy()})),
    )
    .await;
    // A refusal on the request's merits is 400, not 503: installing a package
    // would not make this path readable.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("outside"),
        "the refusal must name the rule: {body}"
    );
}

#[tokio::test]
async fn test_a_type_outside_the_allowlist_is_refused_with_the_allowed_set() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let archive = seed_image(&h, "backup.zip");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&admin),
        Some(json!({"path": archive.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains(".iso"), "{body}");
    assert!(message.contains(".img"), "{body}");
}

#[tokio::test]
async fn test_a_missing_image_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let absent = h.root.join("images").join("absent.iso");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&admin),
        Some(json!({"path": absent.to_string_lossy()})),
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

/// A `..` in the request is refused before `canonicalize`, so a lexical path
/// that would pass the containment test cannot reach outside a root.
#[tokio::test]
async fn test_a_parent_traversal_in_the_path_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let escaped = format!("{}/images/../outside.iso", h.root.to_string_lossy());
    std::fs::write(h.root.join("outside.iso"), b"x").unwrap();

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&admin),
        Some(json!({"path": escaped})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn test_a_relative_path_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/iso",
        Some(&admin),
        Some(json!({"path": "isos/ubuntu.iso"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ---------------------------------------------------------------------------
// Unmount
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unmounting_an_unknown_id_is_refused_with_the_id_named() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/explorer/iso/does-not-exist",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("does-not-exist"),
        "{body}"
    );
}

/// A crafted id must never be able to name a path: the mount point is
/// re-derived from the id and re-checked against the ISO root.
#[tokio::test]
async fn test_a_traversal_in_the_id_cannot_name_a_path() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/explorer/iso/..%2f..%2fetc",
        Some(&admin),
        None,
    )
    .await;
    // No mount carries that id, so it is refused as unknown — never resolved
    // to a directory to unmount.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("No ISO mount"),
        "{body}"
    );
}
