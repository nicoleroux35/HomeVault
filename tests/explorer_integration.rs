// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the File Explorer API (`crate::handlers::explorer`):
//!
//!   - GET    /api/explorer/list                     (directory listing)
//!   - GET    /api/explorer/search                   (recursive name search)
//!   - GET    /api/explorer/disks                    (mounted filesystems)
//!   - POST   /api/explorer/mkdir                    (create directory)
//!   - POST   /api/explorer/delete                   (permanent delete)
//!   - POST   /api/explorer/rename                   (rename in place)
//!   - POST   /api/explorer/copy                     (copy file/dir)
//!   - POST   /api/explorer/move                     (move file/dir)
//!   - GET|POST /api/explorer/permissions            (get / chmod)
//!   - GET    /api/explorer/preview                  (text preview)
//!   - POST   /api/explorer/extract                  (unpack an archive)
//!   - POST   /api/explorer/compress                 (create an archive)
//!   - GET    /api/explorer/download                 (stream a file)
//!   - GET    /api/explorer/thumbnail                (image thumbnail)
//!   - POST   /api/explorer/upload                   (multipart upload)
//!   - POST   /api/explorer/trash                    (soft delete)
//!   - GET    /api/explorer/trash                    (list trash)
//!   - DELETE /api/explorer/trash                    (empty trash)
//!   - POST   /api/explorer/trash/restore            (restore one item)
//!   - DELETE /api/explorer/trash/{id}               (purge one item)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! **Unlike the Shares / Storage suites, the File Explorer handlers do all
//! their work with plain filesystem calls (no `sudo`, no system services,
//! no writes under `/etc`)** — `chmod`, `tar`/`unzip` and `image` decoding
//! are the only externals and all are safe. So this suite *does* drive the
//! real success paths end to end, against files it creates inside a
//! per-test `tempfile::tempdir()`. The one thing left to the E2E suite is
//! the browser side (drag-drop upload, previews, the trash UI) and the SSE
//! progress stream for a directory copy/move job.
//!
//! `PathGuard` is scoped to the test's temp dir, so a **non-admin** session
//! is confined to it and traversal attempts (`/etc/passwd`, `..`) can be
//! asserted to fail without touching anything real.
//!
//! Run:
//!   cargo test --test explorer_integration
//!   cargo test --test explorer_integration -- --nocapture

use axum::{
    routing::{delete, get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::explorer::{
    handle_explorer_compress, handle_explorer_copy, handle_explorer_delete, handle_explorer_disks,
    handle_explorer_download, handle_explorer_download_ticket, handle_explorer_eject,
    handle_explorer_extract, handle_explorer_favorites_add, handle_explorer_favorites_list,
    handle_explorer_favorites_remove, handle_explorer_list, handle_explorer_mkdir,
    handle_explorer_move, handle_explorer_permissions, handle_explorer_preview,
    handle_explorer_rename, handle_explorer_roots, handle_explorer_save, handle_explorer_search,
    handle_explorer_snapshots, handle_explorer_thumbnail, handle_explorer_thumbnail_ticket,
    handle_explorer_trash, handle_explorer_trash_empty, handle_explorer_trash_list,
    handle_explorer_trash_purge, handle_explorer_trash_restore, handle_explorer_upload,
    handle_explorer_upload_chunk, handle_explorer_upload_complete,
    handle_explorer_upload_session_abort, handle_explorer_upload_session_start,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct Harness {
    state: Arc<AppState>,
    /// Canonicalised temp dir the PathGuard is scoped to.
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
        homevault::handlers::downloader::Aria2Process { child: None },
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    let state = Arc::new(AppState {
        auth,
        widgets,
        db,
        path_guard,
        admin_rate_limiter,
        pty_manager,
        downloader,
        ransomware_watch_tx,
    });

    Harness {
        state,
        root,
        _tmp: tmp,
        _guard: guard,
    }
}

fn explorer_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/explorer/list", get(handle_explorer_list))
        .route("/api/explorer/search", get(handle_explorer_search))
        .route("/api/explorer/disks", get(handle_explorer_disks))
        .route("/api/explorer/mkdir", post(handle_explorer_mkdir))
        .route("/api/explorer/delete", post(handle_explorer_delete))
        .route("/api/explorer/rename", post(handle_explorer_rename))
        .route("/api/explorer/copy", post(handle_explorer_copy))
        .route("/api/explorer/move", post(handle_explorer_move))
        .route(
            "/api/explorer/permissions",
            get(handle_explorer_permissions),
        )
        .route(
            "/api/explorer/permissions",
            post(handle_explorer_permissions),
        )
        .route("/api/explorer/preview", get(handle_explorer_preview))
        .route("/api/explorer/extract", post(handle_explorer_extract))
        .route("/api/explorer/download", get(handle_explorer_download))
        .route(
            "/api/explorer/download-ticket",
            post(handle_explorer_download_ticket),
        )
        .route("/api/explorer/thumbnail", get(handle_explorer_thumbnail))
        .route(
            "/api/explorer/thumbnail-ticket",
            post(handle_explorer_thumbnail_ticket),
        )
        .route("/api/explorer/upload", post(handle_explorer_upload))
        .route(
            "/api/explorer/upload/session",
            post(handle_explorer_upload_session_start),
        )
        .route(
            "/api/explorer/upload/session",
            delete(handle_explorer_upload_session_abort),
        )
        .route(
            "/api/explorer/upload/chunk",
            post(handle_explorer_upload_chunk),
        )
        .route(
            "/api/explorer/upload/complete",
            post(handle_explorer_upload_complete),
        )
        .route("/api/explorer/compress", post(handle_explorer_compress))
        .route("/api/explorer/trash", post(handle_explorer_trash))
        .route("/api/explorer/trash", get(handle_explorer_trash_list))
        .route("/api/explorer/trash", delete(handle_explorer_trash_empty))
        .route(
            "/api/explorer/trash/restore",
            post(handle_explorer_trash_restore),
        )
        .route(
            "/api/explorer/trash/{id}",
            delete(handle_explorer_trash_purge),
        )
        .route("/api/explorer/roots", get(handle_explorer_roots))
        .route("/api/explorer/save", post(handle_explorer_save))
        .route("/api/explorer/eject", post(handle_explorer_eject))
        .route("/api/explorer/snapshots", get(handle_explorer_snapshots))
        .route(
            "/api/explorer/favorites",
            get(handle_explorer_favorites_list),
        )
        .route(
            "/api/explorer/favorites",
            post(handle_explorer_favorites_add),
        )
        .route(
            "/api/explorer/favorites",
            delete(handle_explorer_favorites_remove),
        )
        .with_state(state)
}

/// `role_id` 2 = admin, 1 = regular.
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
    let (status, bytes) = send_raw(router, method, uri, token, body).await;
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

async fn send_raw(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    send_raw_headers(router, method, uri, token, body, &[]).await
}

/// Like `send_raw` but with additional raw HTTP headers (e.g. `Range`).
async fn send_raw_headers(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
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
    (status, bytes)
}

/// Send a `multipart/form-data` upload with a single file field.
async fn send_upload(
    router: &mut Router,
    uri: &str,
    token: Option<&str>,
    filename: &str,
    contents: &[u8],
) -> (StatusCode, Value) {
    let boundary = "----hvtestboundary8f2a";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(contents);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let mut req = Request::builder().method(Method::POST).uri(uri).header(
        "content-type",
        format!("multipart/form-data; boundary={boundary}"),
    );
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    let req = req.body(axum::body::Body::from(body)).unwrap();
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

fn p(root: &Path, rel: &str) -> String {
    root.join(rel).to_string_lossy().to_string()
}

/// A handler must never 500; either it failed the auth gate or returned a
/// clean `success:false`.
fn assert_clean_rejection(status: StatusCode, body: &Value) {
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "handler 500'd instead of returning a clean error: {body}"
    );
    assert_eq!(
        body["success"],
        json!(false),
        "expected success:false: {body}"
    );
}

// ===========================================================================
// 1. Auth guards
// ===========================================================================

/// Every JSON endpoint rejects a request with no session token.
#[tokio::test]
async fn test_json_endpoints_reject_missing_token() {
    let h = setup().await;
    let mut r = explorer_router(h.state.clone());

    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/explorer/list?path=/tmp", None),
        (Method::GET, "/api/explorer/search?path=/tmp&query=x", None),
        (Method::GET, "/api/explorer/disks", None),
        (Method::POST, "/api/explorer/mkdir?path=/tmp/x", None),
        (Method::POST, "/api/explorer/delete?path=/tmp/x", None),
        (
            Method::POST,
            "/api/explorer/rename?path=/tmp/x&new_name=y",
            None,
        ),
        (
            Method::POST,
            "/api/explorer/copy?source=/tmp/x&destination=/tmp/y",
            None,
        ),
        (
            Method::POST,
            "/api/explorer/move?source=/tmp/x&destination=/tmp/y",
            None,
        ),
        (Method::GET, "/api/explorer/permissions?path=/tmp/x", None),
        (
            Method::POST,
            "/api/explorer/permissions?path=/tmp/x&mode=755",
            None,
        ),
        (Method::GET, "/api/explorer/preview?path=/tmp/x", None),
        (
            Method::POST,
            "/api/explorer/extract?path=/tmp/a.zip&destination=/tmp",
            None,
        ),
        (
            Method::POST,
            "/api/explorer/compress",
            Some(json!({"paths":["/tmp/x"],"destination":"/tmp/a.tar.gz"})),
        ),
        (Method::POST, "/api/explorer/trash?path=/tmp/x", None),
        (Method::GET, "/api/explorer/trash", None),
        (Method::DELETE, "/api/explorer/trash", None),
        (Method::POST, "/api/explorer/trash/restore?id=abc", None),
        (Method::DELETE, "/api/explorer/trash/abc", None),
    ];

    for (method, uri, body) in cases {
        let (status, resp) = send(&mut r, method.clone(), uri, None, body.clone()).await;
        assert_eq!(
            resp["success"],
            json!(false),
            "{method} {uri} must reject a missing token"
        );
        assert_ne!(
            status,
            StatusCode::OK,
            "{method} {uri} returned 200 unauthenticated"
        );
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method} {uri} 500'd"
        );
    }
}

/// The two binary streaming endpoints answer with a 401 status (not JSON).
#[tokio::test]
async fn test_binary_endpoints_reject_missing_token() {
    let h = setup().await;
    let mut r = explorer_router(h.state.clone());

    for uri in [
        "/api/explorer/download?path=/tmp/x",
        "/api/explorer/thumbnail?path=/tmp/x.png",
    ] {
        let (status, _) = send_raw(&mut r, Method::GET, uri, None, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{uri} must be 401 without a token"
        );
    }
}

/// A syntactically-valid but unknown token is rejected as unauthorized.
#[tokio::test]
async fn test_endpoints_reject_bogus_token() {
    let h = setup().await;
    let mut r = explorer_router(h.state.clone());

    let (status, body) = send(
        &mut r,
        Method::GET,
        &format!("/api/explorer/list?path={}", h.root.display()),
        Some("not-a-real-session-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 2. Path access control (non-admin is confined to the allowed root)
// ===========================================================================

#[tokio::test]
async fn test_non_admin_cannot_escape_allowed_root() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    // Absolute path outside the root.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/list?path=/etc",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "listing /etc must be denied: {body}"
    );

    // Traversal out of the root.
    let escape = format!("{}/../../../../etc/passwd", h.root.display());
    let (_, body) = send(
        &mut r,
        Method::GET,
        &format!("/api/explorer/preview?path={escape}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "traversal preview must be denied: {body}"
    );

    // mkdir outside the root.
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/mkdir?path=/etc/hv-should-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(!std::path::Path::new("/etc/hv-should-not-exist").exists());
}

#[tokio::test]
async fn test_non_admin_download_outside_root_is_forbidden() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, _) = send_raw(
        &mut r,
        Method::GET,
        "/api/explorer/download?path=/etc/passwd",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "download of /etc/passwd must be 403"
    );
}

#[tokio::test]
async fn test_admin_can_read_outside_root_but_still_no_crash() {
    let h = setup().await;
    create_test_user(&h.state, "root_admin", "Password123!", 2).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "root_admin", "Password123!").await;

    // Admin bypasses PathGuard — listing a real system dir must succeed
    // (this is the documented admin behaviour), never 500.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/list?path=/etc",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true), "{body}");
}

// ===========================================================================
// 3. Real success paths (files created inside the temp dir)
// ===========================================================================

#[tokio::test]
async fn test_mkdir_list_rename_delete_roundtrip() {
    let h = setup().await;
    // Permanent delete is admin-only now (the UI trashes instead), so this
    // round-trip runs as an admin.
    create_test_user(&h.state, "bob", "Password123!", 2).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    // mkdir
    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/mkdir?path={}", p(&h.root, "project")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(h.root.join("project").is_dir());

    // list shows it
    let (_, body) = send(
        &mut r,
        Method::GET,
        &format!("/api/explorer/list?path={}", h.root.display()),
        Some(&token),
        None,
    )
    .await;
    let names: Vec<&str> = body["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"project"),
        "list missing the new dir: {names:?}"
    );

    // rename
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/rename?path={}&new_name={}",
            p(&h.root, "project"),
            p(&h.root, "renamed")
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert!(h.root.join("renamed").is_dir());
    assert!(!h.root.join("project").exists());

    // permanent delete
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/delete?path={}", p(&h.root, "renamed")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert!(!h.root.join("renamed").exists());
}

#[tokio::test]
async fn test_copy_and_move_a_file() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("a.txt"), b"hello explorer").unwrap();

    // copy
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/copy?source={}&destination={}",
            p(&h.root, "a.txt"),
            p(&h.root, "b.txt")
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert_eq!(
        std::fs::read(h.root.join("b.txt")).unwrap(),
        b"hello explorer"
    );
    assert!(
        h.root.join("a.txt").exists(),
        "copy must not remove the source"
    );

    // move
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/move?source={}&destination={}",
            p(&h.root, "a.txt"),
            p(&h.root, "c.txt")
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert_eq!(
        std::fs::read(h.root.join("c.txt")).unwrap(),
        b"hello explorer"
    );
    assert!(
        !h.root.join("a.txt").exists(),
        "move must remove the source"
    );
}

#[tokio::test]
async fn test_preview_text_file() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("notes.txt"), b"line one\nline two\n").unwrap();

    let (status, body) = send(
        &mut r,
        Method::GET,
        &format!("/api/explorer/preview?path={}", p(&h.root, "notes.txt")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["content"], json!("line one\nline two\n"));
    assert_eq!(body["data"]["previewable"], json!(true));
}

#[tokio::test]
async fn test_search_finds_a_nested_file() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir_all(h.root.join("deep/sub")).unwrap();
    std::fs::write(h.root.join("deep/sub/needle-report.log"), b"x").unwrap();

    let (status, body) = send(
        &mut r,
        Method::GET,
        &format!(
            "/api/explorer/search?path={}&query=needle",
            h.root.display()
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body["data"]["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .any(|e| e["name"].as_str().unwrap().contains("needle-report")),
        "search did not find the nested file: {body}"
    );
}

#[tokio::test]
async fn test_permissions_get_then_chmod() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let file = h.root.join("perm.txt");
    std::fs::write(&file, b"x").unwrap();

    // GET current permissions
    let (status, body) = send(
        &mut r,
        Method::GET,
        &format!("/api/explorer/permissions?path={}", file.display()),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["permissions"].is_string(), "{body}");

    // chmod 600
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/permissions?path={}&mode=600", file.display()),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[tokio::test]
async fn test_permissions_rejects_a_bad_mode() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let file = h.root.join("perm2.txt");
    std::fs::write(&file, b"x").unwrap();

    for bad in ["999", "abc", "12", "77777", "0o644"] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            &format!(
                "/api/explorer/permissions?path={}&mode={bad}",
                file.display()
            ),
            Some(&token),
            None,
        )
        .await;
        assert_clean_rejection(status, &body);
    }
}

#[tokio::test]
async fn test_compress_then_extract_roundtrip() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("doc1.txt"), b"first").unwrap();
    std::fs::write(h.root.join("doc2.txt"), b"second").unwrap();
    let archive = h.root.join("bundle.tar.gz");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/compress",
        Some(&token),
        Some(json!({
            "paths": [p(&h.root, "doc1.txt"), p(&h.root, "doc2.txt")],
            "destination": archive.to_string_lossy(),
            "format": "tar.gz"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(archive.exists(), "archive was not created");

    std::fs::create_dir_all(h.root.join("out")).unwrap();
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/extract?path={}&destination={}",
            archive.to_string_lossy(),
            p(&h.root, "out")
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    // tar stores the paths it was given; at minimum one of the docs lands.
    let extracted: Vec<_> = walkdir(&h.root.join("out"));
    assert!(
        extracted.iter().any(|n| n.ends_with("doc1.txt")),
        "extract produced nothing recognisable: {extracted:?}"
    );
}

fn walkdir(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }
    out
}

// ===========================================================================
// 4. Download / upload
// ===========================================================================

#[tokio::test]
async fn test_download_streams_exact_bytes() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let payload: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(h.root.join("blob.bin"), &payload).unwrap();

    let (status, bytes) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/explorer/download?path={}", p(&h.root, "blob.bin")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, payload, "downloaded bytes differ from the file");
}

#[tokio::test]
async fn test_download_directory_is_bad_request() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir_all(h.root.join("adir")).unwrap();
    let (status, _) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/explorer/download?path={}", p(&h.root, "adir")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_download_missing_file_is_not_found() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, _) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/explorer/download?path={}", p(&h.root, "ghost.bin")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_upload_writes_the_file() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir_all(h.root.join("incoming")).unwrap();
    let (status, body) = send_upload(
        &mut r,
        &format!("/api/explorer/upload?path={}", p(&h.root, "incoming")),
        Some(&token),
        "hello.txt",
        b"uploaded contents",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true), "{body}");
    assert_eq!(
        std::fs::read(h.root.join("incoming/hello.txt")).unwrap(),
        b"uploaded contents"
    );
}

#[tokio::test]
async fn test_upload_rejects_a_traversing_filename() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir_all(h.root.join("incoming")).unwrap();
    let (status, body) = send_upload(
        &mut r,
        &format!("/api/explorer/upload?path={}", p(&h.root, "incoming")),
        Some(&token),
        "../../evil.txt",
        b"pwned",
    )
    .await;
    // The handler sanitises to the basename `evil.txt` and stores it inside
    // the target dir — the important guarantee is nothing escapes.
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        !h.root.join("evil.txt").exists(),
        "file escaped the upload dir"
    );
    assert!(!std::path::Path::new("/tmp/evil.txt").exists());
}

#[tokio::test]
async fn test_upload_into_missing_dir_fails_cleanly() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send_upload(
        &mut r,
        &format!("/api/explorer/upload?path={}", p(&h.root, "nope")),
        Some(&token),
        "x.txt",
        b"x",
    )
    .await;
    assert_clean_rejection(status, &body);
}

// ===========================================================================
// 5. Transfer quota enforcement (Explorer is the only metered channel)
// ===========================================================================

#[tokio::test]
async fn test_download_blocked_when_quota_exceeded() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;

    // A 5-byte download cap for bob.
    homevault::storage::transfer_quota_repo::TransferQuotaRepository::new(h.state.db.pool())
        .set_limits("user", "bob", false, 0, true, 5)
        .await
        .expect("set quota");

    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("big.bin"), vec![0u8; 4096]).unwrap();

    let (status, _) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/explorer/download?path={}", p(&h.root, "big.bin")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "over-quota download must be 429"
    );
}

#[tokio::test]
async fn test_download_allowed_under_quota_and_records_usage() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;

    let repo =
        homevault::storage::transfer_quota_repo::TransferQuotaRepository::new(h.state.db.pool());
    repo.set_limits("user", "bob", false, 0, true, 1_000_000)
        .await
        .expect("set quota");

    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("ok.bin"), vec![7u8; 2048]).unwrap();

    let (status, _) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/explorer/download?path={}", p(&h.root, "ok.bin")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let q = repo.get("user", "bob").await.unwrap();
    assert_eq!(q.download_used_bytes, 2048, "usage was not recorded");
}

#[tokio::test]
async fn test_upload_blocked_when_quota_exceeded() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;

    homevault::storage::transfer_quota_repo::TransferQuotaRepository::new(h.state.db.pool())
        .set_limits("user", "bob", true, 8, false, 0)
        .await
        .expect("set quota");

    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir_all(h.root.join("incoming")).unwrap();
    let (status, body) = send_upload(
        &mut r,
        &format!("/api/explorer/upload?path={}", p(&h.root, "incoming")),
        Some(&token),
        "toobig.txt",
        b"way over eight bytes",
    )
    .await;
    assert_clean_rejection(status, &body);
    assert!(
        !h.root.join("incoming/toobig.txt").exists(),
        "the over-quota upload must not survive"
    );
}

// ===========================================================================
// 6. Trash lifecycle
// ===========================================================================

#[tokio::test]
async fn test_trash_list_restore_then_purge() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let file = h.root.join("throwaway.txt");
    std::fs::write(&file, b"data").unwrap();

    // trash it
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/trash?path={}", file.display()),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert!(
        !file.exists(),
        "trashed file must leave its original location"
    );

    // list — find its id
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/trash",
        Some(&token),
        None,
    )
    .await;
    let entries = body["data"].as_array().expect("trash list is an array");
    let entry = entries
        .iter()
        .find(|e| e["name"] == json!("throwaway.txt"))
        .expect("trashed file not listed");
    let id = entry["id"].as_str().unwrap().to_string();

    // restore
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/trash/restore?id={id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"data",
        "restore did not bring the file back"
    );

    // trash again, then purge by id
    send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/trash?path={}", file.display()),
        Some(&token),
        None,
    )
    .await;
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/trash",
        Some(&token),
        None,
    )
    .await;
    let id = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == json!("throwaway.txt"))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (_, body) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/explorer/trash/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/trash",
        Some(&token),
        None,
    )
    .await;
    assert!(
        !body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == json!("throwaway.txt")),
        "purged item still listed"
    );
}

#[tokio::test]
async fn test_empty_trash_clears_everything() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    for name in ["one.txt", "two.txt", "three.txt"] {
        let f = h.root.join(name);
        std::fs::write(&f, b"x").unwrap();
        send(
            &mut r,
            Method::POST,
            &format!("/api/explorer/trash?path={}", f.display()),
            Some(&token),
            None,
        )
        .await;
    }

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/trash",
        Some(&token),
        None,
    )
    .await;
    assert!(body["data"].as_array().unwrap().len() >= 3);

    let (_, body) = send(
        &mut r,
        Method::DELETE,
        "/api/explorer/trash",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/trash",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["data"].as_array().unwrap().len(),
        0,
        "trash not emptied: {body}"
    );
}

#[tokio::test]
async fn test_restore_unknown_id_fails_cleanly() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/trash/restore?id=00000000-0000-0000-0000-000000000000",
        Some(&token),
        None,
    )
    .await;
    assert_clean_rejection(status, &body);
}

// ===========================================================================
// 7. Validation / edge cases
// ===========================================================================

#[tokio::test]
async fn test_delete_nonexistent_path_fails_cleanly() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/delete?path={}", p(&h.root, "does-not-exist")),
        Some(&token),
        None,
    )
    .await;
    assert_clean_rejection(status, &body);
}

#[tokio::test]
async fn test_thumbnail_on_non_image_is_bad_request() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("plain.txt"), b"not an image").unwrap();
    let (status, _) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/explorer/thumbnail?path={}", p(&h.root, "plain.txt")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_disks_returns_the_six_categories() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/disks",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for key in ["system", "internal", "raid", "external", "cloud", "network"] {
        assert!(
            body["data"][key].is_array(),
            "missing category {key}: {body}"
        );
    }
}

#[tokio::test]
async fn test_list_defaults_to_root_for_admin() {
    let h = setup().await;
    create_test_user(&h.state, "root_admin", "Password123!", 2).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "root_admin", "Password123!").await;

    // No `path` param → handler defaults to "/", admin may read it.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/list",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["path"], json!("/"));
}

// ===========================================================================
// 4. umbrelOS-inspired endpoints — labelled roots, inline save, safe-eject,
//    ZFS snapshot browsing, server-side favorites, and the copy/move/upload
//    collision strategy (`?strategy=`).
// ===========================================================================

/// Every new endpoint rejects a request with no session token (or a bogus
/// one) — same bar as the rest of the Explorer API.
#[tokio::test]
async fn test_new_endpoints_reject_missing_and_bogus_token() {
    let h = setup().await;
    let mut r = explorer_router(h.state.clone());

    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/explorer/roots", None),
        (
            Method::POST,
            "/api/explorer/save",
            Some(json!({"path": "/tmp/x.txt", "content": "hi"})),
        ),
        (Method::POST, "/api/explorer/eject?path=/media/usb", None),
        (Method::GET, "/api/explorer/snapshots?path=/tmp", None),
        (Method::GET, "/api/explorer/favorites", None),
        (
            Method::POST,
            "/api/explorer/favorites",
            Some(json!({"path": "/tmp/x"})),
        ),
        (
            Method::DELETE,
            "/api/explorer/favorites",
            Some(json!({"path": "/tmp/x"})),
        ),
    ];

    for (method, uri, body) in cases {
        let (status, resp) = send(&mut r, method.clone(), uri, None, body.clone()).await;
        assert_ne!(status, StatusCode::OK, "{method} {uri} unauthenticated");
        assert_eq!(
            resp["success"],
            json!(false),
            "{method} {uri} missing token"
        );

        let (status, resp) = send(
            &mut r,
            method.clone(),
            uri,
            Some("not-a-real-token"),
            body.clone(),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{method} {uri} bogus token");
        assert_eq!(resp["success"], json!(false), "{method} {uri} bogus token");
    }
}

/// `GET /roots` lists the sidebar starting points: at minimum the allowed
/// root the PathGuard is scoped to, each carrying a machine `kind` key.
#[tokio::test]
async fn test_roots_lists_the_allowed_root_with_a_kind() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/roots",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let roots = body["data"].as_array().expect("data is an array");
    assert!(
        !roots.is_empty(),
        "at least the scoped root must be listed: {body}"
    );
    let root_str = h.root.to_string_lossy().to_string();
    let scoped = roots
        .iter()
        .find(|r| r["path"] == json!(root_str))
        .unwrap_or_else(|| panic!("scoped root {root_str} not in {body}"));
    assert!(
        scoped["kind"].is_string(),
        "each root carries a kind: {scoped}"
    );
    assert!(scoped["name"].is_string());
}

/// The inline editor writes a text file through an atomic temp + rename, and
/// overwrites an existing one in place.
#[tokio::test]
async fn test_save_writes_and_overwrites_a_text_file() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let target = p(&h.root, "notes.md");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/save",
        Some(&token),
        Some(json!({"path": target, "content": "# first\n"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        std::fs::read_to_string(h.root.join("notes.md")).unwrap(),
        "# first\n"
    );

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/explorer/save",
        Some(&token),
        Some(json!({"path": target, "content": "# second\nmore"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(h.root.join("notes.md")).unwrap(),
        "# second\nmore"
    );

    // No `.hvsave-*` temp debris left behind.
    let debris: Vec<_> = std::fs::read_dir(&h.root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("hvsave"))
        .collect();
    assert!(debris.is_empty(), "temp files left: {debris:?}");
}

#[tokio::test]
async fn test_save_rejects_a_path_outside_the_allowed_root() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/save",
        Some(&token),
        Some(json!({"path": "/etc/hv-should-not-write", "content": "x"})),
    )
    .await;
    assert_clean_rejection(status, &body);
    assert!(!std::path::Path::new("/etc/hv-should-not-write").exists());
}

#[tokio::test]
async fn test_save_rejects_a_directory_and_an_oversized_payload() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir(h.root.join("adir")).unwrap();
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/explorer/save",
        Some(&token),
        Some(json!({"path": p(&h.root, "adir"), "content": "x"})),
    )
    .await;
    assert_clean_rejection(status, &body);

    // A payload larger than the editor's 5 MiB cap must never land on disk.
    // (Axum's own request-body limit already refuses it well before the
    // handler's `MAX_EDIT_BYTES` check — either way, no file is written and
    // nothing 500s.)
    let big = "a".repeat(5 * 1024 * 1024 + 1);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/explorer/save",
        Some(&token),
        Some(json!({"path": p(&h.root, "big.txt"), "content": big})),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "an oversized save must be refused");
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!h.root.join("big.txt").exists());
}

/// Safe-eject is admin-only, and a path that is not a real removable mount
/// point is refused before any `umount` runs.
#[tokio::test]
async fn test_eject_is_admin_only_and_refuses_a_non_mount_path() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    create_test_user(&h.state, "root_admin", "Password123!", 2).await;
    let mut r = explorer_router(h.state.clone());

    let bob = login(&mut r, "bob", "Password123!").await;
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/explorer/eject?path=/media/whatever",
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-admin must be 403");

    let admin = login(&mut r, "root_admin", "Password123!").await;
    let fake_mount = p(&h.root, "not-a-mount");
    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/eject?path={fake_mount}"),
        Some(&admin),
        None,
    )
    .await;
    // Never a 500: the path isn't in /proc/mounts → clean "not a mount point".
    assert_clean_rejection(status, &body);
}

/// `GET /snapshots` on a plain (non-ZFS) path returns an empty list, never
/// an error.
#[tokio::test]
async fn test_snapshots_is_empty_for_a_non_zfs_path() {
    let h = setup().await;
    create_test_user(&h.state, "root_admin", "Password123!", 2).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "root_admin", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        &format!("/api/explorer/snapshots?path={}", p(&h.root, "")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

/// Favorites round-trip through the server-side store, scoped per user.
#[tokio::test]
async fn test_favorites_add_list_remove_lifecycle() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let fav_path = p(&h.root, "project");

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/explorer/favorites",
        Some(&token),
        Some(json!({"path": fav_path, "label": "Project"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/favorites",
        Some(&token),
        None,
    )
    .await;
    let favs = body["data"].as_array().expect("array");
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0]["path"], json!(fav_path));
    assert_eq!(favs[0]["label"], json!("Project"));

    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/explorer/favorites",
        Some(&token),
        Some(json!({"path": fav_path})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/explorer/favorites",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn test_favorites_add_rejects_an_empty_or_root_path() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    for bad in ["", "/"] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/explorer/favorites",
            Some(&token),
            Some(json!({"path": bad})),
        )
        .await;
        assert_clean_rejection(status, &body);
    }
}

/// `?strategy=keep_both` must not clobber an existing destination — it
/// writes a ` (1)` sibling instead.
#[tokio::test]
async fn test_copy_keep_both_strategy_preserves_the_existing_destination() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("src.txt"), b"NEW").unwrap();
    std::fs::write(h.root.join("dst.txt"), b"ORIGINAL").unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/copy?source={}&destination={}&strategy=keep_both",
            p(&h.root, "src.txt"),
            p(&h.root, "dst.txt")
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(true), "{body}");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(std::fs::read(h.root.join("dst.txt")).unwrap(), b"ORIGINAL");
    assert_eq!(std::fs::read(h.root.join("dst (1).txt")).unwrap(), b"NEW");
}

/// `?strategy=skip` leaves an existing destination exactly as it was.
#[tokio::test]
async fn test_copy_skip_strategy_leaves_the_destination_untouched() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("src.txt"), b"NEW").unwrap();
    std::fs::write(h.root.join("dst.txt"), b"ORIGINAL").unwrap();

    let (status, _) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/copy?source={}&destination={}&strategy=skip",
            p(&h.root, "src.txt"),
            p(&h.root, "dst.txt")
        ),
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(std::fs::read(h.root.join("dst.txt")).unwrap(), b"ORIGINAL");
    assert!(!h.root.join("dst (1).txt").exists());
}

// ===========================================================================
// 8. Transfers v2 — HTTP ranges, short-lived tickets, resumable uploads,
//    integrity digests, and the tightened rename/delete guards.
// ===========================================================================

/// `Range: bytes=a-b` must answer 206 with exactly the requested slice, and
/// an unsatisfiable range must be 416 — this is what makes downloads
/// resumable and video seeking possible.
#[tokio::test]
async fn test_download_supports_range_requests() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let payload: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(h.root.join("blob.bin"), &payload).unwrap();
    let uri = format!("/api/explorer/download?path={}", p(&h.root, "blob.bin"));

    let (status, bytes) = send_raw_headers(
        &mut r,
        Method::GET,
        &uri,
        Some(&token),
        None,
        &[("range", "bytes=100-199")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes, payload[100..200], "range body differs");

    // Suffix form: the last 50 bytes.
    let (status, bytes) = send_raw_headers(
        &mut r,
        Method::GET,
        &uri,
        Some(&token),
        None,
        &[("range", "bytes=-50")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(bytes, payload[4950..]);

    // Beyond EOF → 416.
    let (status, _) = send_raw_headers(
        &mut r,
        Method::GET,
        &uri,
        Some(&token),
        None,
        &[("range", "bytes=999999-")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
}

/// A small download carries an `X-Content-Sha256` header matching the file.
#[tokio::test]
async fn test_download_sends_integrity_digest() {
    use sha2::{Digest, Sha256};
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let payload = b"digest me".to_vec();
    std::fs::write(h.root.join("d.bin"), &payload).unwrap();

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!(
            "/api/explorer/download?path={}",
            p(&h.root, "d.bin")
        ))
        .header("x-session-token", &token)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let digest = resp
        .headers()
        .get("x-content-sha256")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .expect("X-Content-Sha256 header missing");
    let expected: String = Sha256::digest(&payload)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    assert_eq!(digest, expected);
}

/// The download-ticket flow: mint a ticket with a session, then download with
/// no session header at all; the ticket is single-use.
#[tokio::test]
async fn test_download_ticket_is_single_use_and_session_free() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let payload: Vec<u8> = (0..1000u32).map(|i| (i % 97) as u8).collect();
    std::fs::write(h.root.join("t.bin"), &payload).unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/download-ticket?path={}", p(&h.root, "t.bin")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["data"]["ticket"].as_str().expect("ticket").to_string();
    let uri = format!(
        "/api/explorer/download?path={}&ticket={}",
        p(&h.root, "t.bin"),
        ticket
    );

    // No session header — the ticket is the authorisation.
    let (status, bytes) = send_raw_headers(&mut r, Method::GET, &uri, None, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, payload);

    // Single-use: the same ticket cannot be replayed.
    let (status, _) = send_raw_headers(&mut r, Method::GET, &uri, None, None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A thumbnail ticket authorises `<img>` requests without the session token in
/// the URL; an invalid ticket is rejected.
#[tokio::test]
async fn test_thumbnail_ticket_authenticates_without_session() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("plain.txt"), b"not an image").unwrap();

    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/thumbnail-ticket?path={}", h.root.display()),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ticket = body["data"]["ticket"].as_str().expect("ticket").to_string();

    // With the ticket and no session, auth passes and we reach the image
    // gate → 400 (not 401).
    let uri = format!(
        "/api/explorer/thumbnail?path={}&ticket={}",
        p(&h.root, "plain.txt"),
        ticket
    );
    let (status, _) = send_raw_headers(&mut r, Method::GET, &uri, None, None, &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A garbage ticket is unauthorised.
    let (status, _) = send_raw_headers(
        &mut r,
        Method::GET,
        &format!(
            "/api/explorer/thumbnail?path={}&ticket=deadbeef",
            p(&h.root, "plain.txt")
        ),
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// Sends one raw binary chunk to the resumable-upload endpoint.
async fn send_chunk(
    router: &mut Router,
    session_id: &str,
    offset: usize,
    bytes: &[u8],
    token: Option<&str>,
) -> (StatusCode, Value) {
    let uri = format!("/api/explorer/upload/chunk?session_id={session_id}&offset={offset}");
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/octet-stream");
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    let req = req.body(axum::body::Body::from(bytes.to_vec())).unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json =
        serde_json::from_slice(&body).unwrap_or(json!({"raw": String::from_utf8_lossy(&body)}));
    (status, json)
}

/// A file uploaded in chunks is byte-identical, reports a checksum, and
/// leaves no temp debris.
#[tokio::test]
async fn test_chunked_upload_roundtrip() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::create_dir_all(h.root.join("incoming")).unwrap();
    let payload: Vec<u8> = (0..20000u32).map(|i| (i % 251) as u8).collect();

    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/upload/session?path={}&filename=chunked.bin&size={}",
            p(&h.root, "incoming"),
            payload.len()
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sid = body["data"]["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    let chunk = 8192usize;
    let mut offset = 0usize;
    while offset < payload.len() {
        let end = (offset + chunk).min(payload.len());
        let (status, body) =
            send_chunk(&mut r, &sid, offset, &payload[offset..end], Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["offset"].as_u64(), Some(end as u64));
        offset = end;
    }

    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/upload/complete?session_id={sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["checksum"].is_string(), "{body}");
    assert_eq!(
        std::fs::read(h.root.join("incoming/chunked.bin")).unwrap(),
        payload
    );

    let debris: Vec<_> = std::fs::read_dir(h.root.join("incoming"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("hvupl"))
        .collect();
    assert!(debris.is_empty(), "temp upload debris left: {debris:?}");
}

/// An out-of-order chunk is rejected with the server's authoritative offset,
/// so the client can resume from there.
#[tokio::test]
async fn test_chunked_upload_offset_mismatch_reports_offset() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;
    std::fs::create_dir_all(h.root.join("incoming")).unwrap();

    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/upload/session?path={}&filename=x.bin&size=10",
            p(&h.root, "incoming")
        ),
        Some(&token),
        None,
    )
    .await;
    let sid = body["data"]["session_id"].as_str().unwrap().to_string();

    let (status, body) = send_chunk(&mut r, &sid, 5, b"hello", Some(&token)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["offset"].as_u64(),
        Some(0),
        "must report the real offset"
    );
}

/// Aborting a resumable upload removes its partial temp file.
#[tokio::test]
async fn test_chunked_upload_abort_removes_temp() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;
    std::fs::create_dir_all(h.root.join("incoming")).unwrap();

    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/upload/session?path={}&filename=abort.bin&size=100",
            p(&h.root, "incoming")
        ),
        Some(&token),
        None,
    )
    .await;
    let sid = body["data"]["session_id"].as_str().unwrap().to_string();
    send_chunk(&mut r, &sid, 0, b"partial", Some(&token)).await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/explorer/upload/session?session_id={sid}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let leftovers: Vec<_> = std::fs::read_dir(h.root.join("incoming"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("hvupl"))
        .collect();
    assert!(leftovers.is_empty(), "abort left temp files: {leftovers:?}");
    assert!(!h.root.join("incoming/abort.bin").exists());
}

/// A non-admin cannot rename a file out of the allowed root (the destination
/// was never validated before).
#[tokio::test]
async fn test_rename_outside_allowed_root_is_rejected() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("move-me.txt"), b"x").unwrap();
    let escape = "/etc/hv-rename-escape.txt";
    let (_, body) = send(
        &mut r,
        Method::POST,
        &format!(
            "/api/explorer/rename?path={}&new_name={}",
            p(&h.root, "move-me.txt"),
            escape
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        body["success"],
        json!(false),
        "rename escape must be denied: {body}"
    );
    assert!(h.root.join("move-me.txt").exists(), "source must stay put");
    assert!(
        !std::path::Path::new(escape).exists(),
        "nothing must escape"
    );
}

/// Permanent delete is admin-only; a regular user is forbidden.
#[tokio::test]
async fn test_permanent_delete_is_admin_only() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    let mut r = explorer_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    std::fs::write(h.root.join("keep.txt"), b"x").unwrap();
    let (status, body) = send(
        &mut r,
        Method::POST,
        &format!("/api/explorer/delete?path={}", p(&h.root, "keep.txt")),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(h.root.join("keep.txt").exists(), "file must not be deleted");
}
