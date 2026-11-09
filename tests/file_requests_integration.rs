/* =============================================================================
 * File          : file_requests_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Public file-request integration tests: in-memory SQLite + assembled Axum
 *   router, exercising create/list/revoke and the public upload page/route.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test file_requests_integration
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]
//! Integration tests for public file requests (`crate::handlers::file_requests`):
//!
//!   GET/POST /api/explorer/file-requests
//!   DELETE   /api/explorer/file-requests/{token}
//!   GET      /request/{token}
//!   POST     /request/{token}/upload
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server. Files live in a per-test tempdir.

use axum::{
    routing::{delete, get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::file_requests::{
    handle_file_request_create, handle_file_request_delete, handle_file_request_page,
    handle_file_request_upload, handle_file_requests_list,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    root: PathBuf,
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
        root,
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/explorer/file-requests",
            get(handle_file_requests_list).post(handle_file_request_create),
        )
        .route(
            "/api/explorer/file-requests/{token}",
            delete(handle_file_request_delete),
        )
        .route("/request/{token}", get(handle_file_request_page))
        .route("/request/{token}/upload", post(handle_file_request_upload))
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

async fn grant_folder(state: &AppState, member: &str, path: &str, permission: &str) {
    sqlx::query(
        "INSERT INTO resource_shares (owner, member, kind, resource, permission)
         VALUES (?, ?, 'folder', ?, ?)
         ON CONFLICT(member, kind, resource) DO UPDATE SET
             permission = excluded.permission,
             owner = excluded.owner",
    )
    .bind(member)
    .bind(member)
    .bind(path)
    .bind(permission)
    .execute(&*state.db.pool())
    .await
    .expect("insert resource grant");
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
    if let Some(token) = token {
        req = req.header("x-session-token", token);
    }
    let req = if let Some(body) = body {
        req.header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
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

/// Sends a multipart upload to the public route. The password field (when
/// present) is written **before** the file, exactly like the page's form.
async fn send_public_upload(
    router: &mut Router,
    token: &str,
    password: Option<&str>,
    filename: &str,
    contents: &[u8],
) -> (StatusCode, String) {
    let boundary = "----hvrequestboundary91c2";
    let mut body = Vec::new();
    if let Some(password) = password {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"password\"\r\n\r\n");
        body.extend_from_slice(password.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(contents);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/request/{token}/upload"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(axum::body::Body::from(body))
        .unwrap();
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
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn folder(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[tokio::test]
async fn creating_a_file_request_requires_a_write_grant() {
    let h = setup().await;
    let mut r = router(h.state.clone());
    create_test_user(&h.state, "reader", "pass", 1).await;
    grant_folder(&h.state, "reader", &h.root.to_string_lossy(), "read").await;
    let token = login(&mut r, "reader", "pass").await;
    let dest = folder(&h.root, "incoming");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/file-requests",
        Some(&token),
        Some(json!({"path": dest.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Write opens it.
    grant_folder(&h.state, "reader", &h.root.to_string_lossy(), "write").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/file-requests",
        Some(&token),
        Some(json!({"path": dest.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["protected"], json!(false));
    assert!(body["data"]["request_path"]
        .as_str()
        .unwrap()
        .starts_with("/request/"));
}

#[tokio::test]
async fn a_file_request_must_target_a_folder() {
    let h = setup().await;
    let mut r = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let token = login(&mut r, "admin", "pass").await;
    let file = h.root.join("not-a-folder.txt");
    std::fs::write(&file, b"x").unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/file-requests",
        Some(&token),
        Some(json!({"path": file.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn a_public_upload_lands_in_the_folder_and_collisions_are_kept() {
    let h = setup().await;
    let mut r = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let token = login(&mut r, "admin", "pass").await;
    let dest = folder(&h.root, "incoming");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/file-requests",
        Some(&token),
        Some(json!({"path": dest.to_string_lossy(), "max_uploads": 2})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request_token = body["data"]["token"].as_str().unwrap().to_string();

    let (status, page) = send(
        &mut r,
        Method::GET,
        &format!("/request/{request_token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page["raw"].as_str().unwrap_or("").contains("type=\"file\""));

    let (status, page) =
        send_public_upload(&mut r, &request_token, None, "hello.txt", b"hello").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(std::fs::read(dest.join("hello.txt")).unwrap(), b"hello");

    // Same name again → a ` (1)` copy, never an overwrite.
    let (status, _) =
        send_public_upload(&mut r, &request_token, None, "hello.txt", b"second").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(std::fs::read(dest.join("hello.txt")).unwrap(), b"hello");
    assert_eq!(
        std::fs::read(dest.join("hello (1).txt")).unwrap(),
        b"second"
    );

    // The cap of 2 is spent.
    let (status, page) =
        send_public_upload(&mut r, &request_token, None, "third.txt", b"third").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{page}");
    assert!(!dest.join("third.txt").exists());
}

#[tokio::test]
async fn a_password_protected_request_needs_the_password_first() {
    let h = setup().await;
    let mut r = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let token = login(&mut r, "admin", "pass").await;
    let dest = folder(&h.root, "incoming");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/file-requests",
        Some(&token),
        Some(json!({"path": dest.to_string_lossy(), "password": "correct-horse"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request_token = body["data"]["token"].as_str().unwrap().to_string();

    let (status, page) = send_public_upload(&mut r, &request_token, None, "x.txt", b"nope").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{page}");
    assert!(!dest.join("x.txt").exists());

    let (status, page) =
        send_public_upload(&mut r, &request_token, Some("wrong"), "x.txt", b"nope").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a wrong password re-renders the page"
    );
    assert!(page.contains("incorrect"), "{page}");
    assert!(!dest.join("x.txt").exists());

    let (status, page) = send_public_upload(
        &mut r,
        &request_token,
        Some("correct-horse"),
        "x.txt",
        b"ok",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(std::fs::read(dest.join("x.txt")).unwrap(), b"ok");
}

#[tokio::test]
async fn revoking_the_owner_grant_stops_public_uploads() {
    let h = setup().await;
    let mut r = router(h.state.clone());
    create_test_user(&h.state, "owner", "pass", 1).await;
    grant_folder(&h.state, "owner", &h.root.to_string_lossy(), "write").await;
    let token = login(&mut r, "owner", "pass").await;
    let dest = folder(&h.root, "incoming");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/file-requests",
        Some(&token),
        Some(json!({"path": dest.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let request_token = body["data"]["token"].as_str().unwrap().to_string();

    sqlx::query("DELETE FROM resource_shares WHERE member = 'owner'")
        .execute(&*h.state.db.pool())
        .await
        .unwrap();

    let (status, _) =
        send_public_upload(&mut r, &request_token, None, "blocked.txt", b"nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!dest.join("blocked.txt").exists());
}

#[tokio::test]
async fn unknown_request_tokens_render_an_unavailable_page() {
    let h = setup().await;
    let mut r = router(h.state.clone());
    let (status, page) = send(&mut r, Method::GET, "/request/does-not-exist", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(page["raw"].as_str().unwrap_or("").contains("unavailable"));
}
