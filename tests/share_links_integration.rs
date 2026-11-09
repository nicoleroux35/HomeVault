/* =============================================================================
 * File          : share_links_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Share links integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for Explorer "share by link"
//! (`crate::handlers::share_links`):
//!
//!   GET    /api/explorer/shares            list the caller's links
//!   POST   /api/explorer/shares            create a link for a file
//!   DELETE /api/explorer/shares/{token}    revoke a link
//!   GET    /share/{token}                  public download
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server. Files are created inside a per-test
//! `tempfile::tempdir()`; `PathGuard` is scoped to it so a non-admin session
//! is confined to it.
//!
//! Run:
//!   cargo test --test share_links_integration
//!   cargo test --test share_links_integration -- --nocapture

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
use homevault::handlers::share_links::{
    handle_share_link_create, handle_share_link_delete, handle_share_link_download,
    handle_share_link_unlock, handle_share_links_list,
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
        .route("/api/explorer/shares", get(handle_share_links_list))
        .route("/api/explorer/shares", post(handle_share_link_create))
        .route(
            "/api/explorer/shares/{token}",
            delete(handle_share_link_delete),
        )
        .route("/share/{token}", get(handle_share_link_download))
        .route("/share/{token}/unlock", post(handle_share_link_unlock))
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

/// One `folder` grant for a non-admin. Share-link creation now requires the
/// owner to still hold a `read` grant on the path (a `PathGuard` root alone is
/// not an authorization).
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

/// Creates a real file in the temp root and returns its absolute path.
fn make_file(root: &Path, name: &str, content: &str) -> String {
    let p = root.join(name);
    std::fs::write(&p, content).expect("write test file");
    p.to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// Auth matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_share_links_require_auth() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;

    // No token at all.
    let (s, _) = send(&mut router, Method::GET, "/api/explorer/shares", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        None,
        Some(json!({"path": "/x"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        "/api/explorer/shares/sometoken",
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Garbage token.
    let (s, _) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some("garbage"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Valid session works.
    let (s, body) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["data"], json!([]));
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_share_link_create_and_list() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "hello.txt", "hello world");

    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["success"], true);
    let token = body["data"]["token"].as_str().expect("token").to_string();
    assert_eq!(token.len(), 48);
    let share_path = body["data"]["share_path"].as_str().expect("share_path");
    assert_eq!(share_path, format!("/share/{}", token));
    // Default mode + 30-day expiry.
    assert_eq!(body["data"]["mode"], "anyone");
    assert!(body["data"]["expires_at"].is_string());
    assert_eq!(body["data"]["expired"], false);

    // The link shows up in the list.
    let (s, body) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let links = body["data"].as_array().expect("array");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["token"], token);
}

#[tokio::test]
async fn test_share_link_password_limit_and_redaction() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "protected.txt", "secret");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({
            "path": file, "password": "correct horse", "max_downloads": 1
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["protected"], true);
    assert!(body["data"].get("password_hash").is_none());
    let token = body["data"]["token"].as_str().unwrap().to_string();

    let (status, _) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/share/{token}"))
        .header("x-share-password", "correct horse")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"secret");

    let (status, _) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_share_link_accepts_folders_and_rejects_missing_files() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;

    // Folder → 200, served as a tar.gz snapshot.
    let folder = h.root.join("shared-folder");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("inside.txt"), b"inside").unwrap();
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": folder.to_string_lossy().to_string()})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["is_directory"], json!(true), "{body}");
    let token = body["data"]["token"].as_str().expect("token").to_string();

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/share/{token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "application/gzip"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(!bytes.is_empty(), "the folder archive must not be empty");

    // Nonexistent path → 400.
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": "/nonexistent/file.txt"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Invalid mode → 400.
    let file = make_file(&h.root, "m.txt", "x");
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "mode": "everyone"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Out-of-range expiry → 400.
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "expires_days": 99999})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_share_link_non_admin_path_guard() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "bob", "pass", 1).await;
    grant_folder(&h.state, "bob", &h.root.to_string_lossy(), "read").await;
    let bob = login(&mut router, "bob", "pass").await;

    // Inside the allowed root: allowed for a regular user.
    let file = make_file(&h.root, "bob.txt", "bob data");
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&bob),
        Some(json!({"path": file})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Outside allowed roots → 403.
    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&bob),
        Some(json!({"path": "/etc/hostname"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(body["success"], false);
}

/// A `PathGuard` root is not an authorization: a member without a `read` grant
/// on the path may not mint a link to it (this was the share-links hole the P0
/// pass closed).
#[tokio::test]
async fn test_share_link_creation_requires_a_grant() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "bob", "pass", 1).await;
    let bob = login(&mut router, "bob", "pass").await;
    let file = make_file(&h.root, "ungranted.txt", "secret");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&bob),
        Some(json!({"path": file})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["success"], json!(false), "{body}");
}

/// A `signed_in` link can name the accounts/groups it was minted for; an
/// unnamed account is refused even though it holds a valid session.
#[tokio::test]
async fn test_targeted_signed_in_link_only_opens_for_named_accounts() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "bob", "pass", 1).await;
    create_test_user(&h.state, "alice", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let bob = login(&mut router, "bob", "pass").await;
    let alice = login(&mut router, "alice", "pass").await;
    let file = make_file(&h.root, "targeted.txt", "targeted");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({
            "path": file,
            "mode": "signed_in",
            "allowed_users": "bob",
            "allowed_groups": ""
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["data"]["token"].as_str().expect("token").to_string();

    let (status, _) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the named account may download");

    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "an unnamed account is refused");
    assert_eq!(body["success"], json!(false), "{body}");

    // A target list on a public link is a 400, not a silently ignored field.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "mode": "anyone", "allowed_users": "bob"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Revoking the owner's grant kills the links they already minted: a link is a
/// standing grant, not a permanent capability.
#[tokio::test]
async fn test_revoking_the_owner_grant_kills_an_existing_link() {    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "bob", "pass", 1).await;
    grant_folder(&h.state, "bob", &h.root.to_string_lossy(), "read").await;
    let bob = login(&mut router, "bob", "pass").await;
    let file = make_file(&h.root, "shared.txt", "shared data");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&bob),
        Some(json!({"path": file})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["data"]["token"].as_str().expect("token").to_string();

    // Still downloadable while the grant holds.
    let (status, _) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Revoke the grant, then the same token is gone.
    sqlx::query("DELETE FROM resource_shares WHERE member = 'bob'")
        .execute(&*h.state.db.pool())
        .await
        .expect("revoke grant");
    let (status, body) = send(
        &mut router,
        Method::GET,
        &format!("/share/{token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a revoked grant kills the link");
    assert_eq!(body["success"], json!(false), "{body}");
}

// ---------------------------------------------------------------------------
// Public download
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_share_link_public_download() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "payload.bin", "SECRET-CONTENT-42");

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file})),
    )
    .await;
    let share_path = body["data"]["share_path"]
        .as_str()
        .expect("share_path")
        .to_string();

    // No session at all — anyone mode allows it.
    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(&share_path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"SECRET-CONTENT-42");
}

#[tokio::test]
async fn test_share_link_supports_range_requests() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let file = h.root.join("range.bin");
    std::fs::write(&file, &payload).unwrap();

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file.to_string_lossy().to_string()})),
    )
    .await;
    let share_path = body["data"]["share_path"].as_str().unwrap().to_string();

    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(&share_path)
                .header("range", "bytes=100-199")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], &payload[100..200]);
}

#[tokio::test]
async fn test_share_link_unknown_and_deleted_token_404() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;

    // Unknown token → 400 (json_error), deliberately not a 200.
    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri("/share/deadbeefdeadbeefdeadbeefdeadbeef")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Delete a real link, then it can no longer be downloaded.
    let file = make_file(&h.root, "gone.txt", "gone");
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap();
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/explorer/shares/{}", token),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/share/{}", token))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_share_link_expired_not_downloadable() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "old.txt", "old");

    // Force a link that already expired by backdating the DB row.
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "expires_days": 1})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap().to_string();
    sqlx::query(
        "UPDATE explorer_share_links SET expires_at = '2000-01-01T00:00:00Z' WHERE token = ?",
    )
    .bind(&token)
    .execute(&*h.state.db.pool())
    .await
    .unwrap();

    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/share/{}", token))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // It also no longer shows as downloadable from the list (expired flag).
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"][0]["expired"], true);
}

#[tokio::test]
async fn test_share_link_signed_in_requires_session() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "bob", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let bob = login(&mut router, "bob", "pass").await;
    let file = make_file(&h.root, "private.txt", "private data");

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "mode": "signed_in"})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap();
    let share_path = format!("/share/{}", token);

    // No session → 401.
    let resp = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(&share_path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Any valid session (not even the owner) → 200.
    let (status, _) = send(&mut router, Method::GET, &share_path, Some(&bob), None).await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Revoke
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_share_link_revoke_only_owner_or_admin() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "alice", "pass", 1).await;
    create_test_user(&h.state, "bob", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let alice = login(&mut router, "alice", "pass").await;
    let bob = login(&mut router, "bob", "pass").await;

    // A member's Explorer roots come from an explicit grant; alice needs the
    // fixture folder to create her link (the admin's path_guard is not enough).
    sqlx::query(
        "INSERT INTO resource_shares (owner, member, kind, resource, permission) \
         VALUES ('alice', 'alice', 'folder', ?, 'full')",
    )
    .bind(h.root.to_string_lossy().to_string())
    .execute(&*h.state.db.pool())
    .await
    .expect("grant alice's folder");

    let file = make_file(&h.root, "alice.txt", "alice's file");
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&alice),
        Some(json!({"path": file})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap();

    // A third user cannot revoke someone else's link → 403.
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/explorer/shares/{}", token),
        Some(&bob),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // The owner can.
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/explorer/shares/{}", token),
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Deleting again → 400 (not found).
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/explorer/shares/{}", token),
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // An admin can revoke a link owned by someone else.
    let file2 = make_file(&h.root, "alice2.txt", "alice 2");
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&alice),
        Some(json!({"path": file2})),
    )
    .await;
    let token2 = body["data"]["token"].as_str().unwrap();
    let (s, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/explorer/shares/{}", token2),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn test_share_link_expiry_0_is_never() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "forever.txt", "forever");

    let (s, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "expires_days": 0})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["data"]["expires_at"].is_null());
}

/// The public `/share/{token}` route must be reachable through the *real*
/// production router (`web::create_app`), registered before the SPA fallback
/// and outside the CSRF layer — not just in the minimal test router above.
#[tokio::test]
async fn test_share_download_reachable_through_production_router() {
    let h = setup().await;
    create_test_user(&h.state, "admin", "pass", 2).await;
    let file = make_file(&h.root, "prod.txt", "prod payload");

    // Mint a link through the repository directly, then hit the full app.
    let repo = homevault::storage::ShareLinkRepository::new(h.state.db.pool());
    let link = repo
        .create(
            "admin",
            &"A".repeat(48),
            &file,
            "anyone",
            None,
            None,
            None,
            false,
            None,
            "",
            "",
        )
        .await
        .expect("create link");

    let mut app = homevault::web::create_app(h.state.clone());
    let resp = ServiceExt::<Req>::ready(&mut app)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/share/{}", link.token))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"prod payload");
}

// ---------------------------------------------------------------------------
// Password unlock, download cap and one-time links (ROADMAP-DSM QW-04)
// ---------------------------------------------------------------------------

/// Sends a fully built request and reads the whole response — status, headers
/// and body — which is what `Set-Cookie`, `HEAD` and cookie-carrying `GET`s
/// need (the `send` helper above only returns status + JSON).
async fn raw(router: &mut Router, req: Req) -> (StatusCode, http::HeaderMap, Vec<u8>) {
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

fn public_get(uri: &str) -> http::request::Builder {
    Request::builder().method(Method::GET).uri(uri)
}

/// The slot is released by a spawned task (or by the stream's `Drop`), so a
/// follow-up request must not race it.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

/// The nonce out of a `Set-Cookie` header, for replaying the unlock.
fn unlock_nonce(headers: &http::HeaderMap) -> String {
    let cookie = headers
        .get(http::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("unlock must set a cookie");
    cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, value)| value.to_string())
        .expect("cookie must carry a value")
}

async fn password_link(
    router: &mut Router,
    state: &AppState,
    root: &Path,
    name: &str,
    password: &str,
    max_downloads: Option<i64>,
) -> (String, String) {
    create_test_user(state, "admin", "pass", 2).await;
    let admin = login(router, "admin", "pass").await;
    let file = make_file(root, name, "protected payload");
    let (s, body) = send(
        router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({
            "path": file,
            "password": password,
            "max_downloads": max_downloads,
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "link created: {body}");
    let token = body["data"]["token"].as_str().expect("token").to_string();
    (admin, token)
}

#[tokio::test]
async fn test_share_link_download_cap_counts_completed_transfers_only() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "capped.txt", "capped payload");
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "max_downloads": 2})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["max_downloads"], 2);
    assert_eq!(body["data"]["download_count"], 0);
    assert_eq!(body["data"]["exhausted"], false);

    for attempt in 1..=2 {
        let (status, _, bytes) = raw(
            &mut router,
            public_get(&format!("/share/{token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "download {attempt} must be served");
        assert_eq!(&bytes[..], b"capped payload");
        settle().await;
    }

    // The third download is the one refused: the N-th was the last.
    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The owner sees the truth, and the hash/active count never leave the server.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"][0]["download_count"], 2);
    assert_eq!(body["data"][0]["exhausted"], true);
    assert!(body["data"][0].get("password_hash").is_none());
    assert!(body["data"][0].get("active_downloads").is_none());
}

/// The regression this pins is invisible to `to_bytes`: hyper stops polling a
/// body once it has written the full `Content-Length` and **drops** the stream
/// without ever observing `Poll::Ready(None)`. Reading exactly that many bytes
/// and dropping the body is what a real browser download looks like, and it
/// used to record *every* completed download as interrupted — so a one-time
/// link was never spent and a cap never advanced.
#[tokio::test]
async fn test_share_link_completed_download_counts_when_hyper_frames_the_body() {
    use futures_util::StreamExt;

    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let payload = "hyper-framed payload";
    let file = make_file(&h.root, "framed.txt", payload);
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "max_downloads": 1})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap().to_string();

    let response = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            public_get(&format!("/share/{token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let expected = payload.len();

    // Read exactly what the browser would write, then drop the body the way
    // hyper does for a sized response.
    let mut stream = response.into_body().into_data_stream();
    let mut received = 0usize;
    while received < expected {
        let chunk = stream.next().await.expect("a chunk").expect("chunk bytes");
        received += chunk.len();
    }
    assert_eq!(received, expected);
    drop(stream);
    settle().await;

    // One completed download: the one-time link is spent.
    let (_, list) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(list["data"][0]["download_count"], 1);
    assert_eq!(list["data"][0]["exhausted"], true);

    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_share_link_one_time_link_is_dead_after_one_completed_download() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "once.txt", "one shot");
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "max_downloads": 1})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap().to_string();

    let (status, _, bytes) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], b"one shot");
    settle().await;

    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_share_link_aborted_download_does_not_spend_a_slot() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;
    let file = make_file(&h.root, "aborted.txt", "aborted payload");
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": file, "max_downloads": 1})),
    )
    .await;
    let token = body["data"]["token"].as_str().unwrap().to_string();

    // Reserve a slot, then walk away before a single byte was read — the shape
    // of a browser tab closed mid-transfer.
    let response = ServiceExt::<Req>::ready(&mut router)
        .await
        .unwrap()
        .call(
            public_get(&format!("/share/{token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    settle().await;

    // The abandoned transfer must not have spent the one-time link.
    let (status, _, bytes) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an interrupted download must not consume the cap"
    );
    assert_eq!(&bytes[..], b"aborted payload");
    settle().await;

    // …and this one did.
    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_share_link_head_probe_is_honest_and_never_consumes_the_cap() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    let (admin, token) = password_link(
        &mut router,
        &h.state,
        &h.root,
        "probe.txt",
        "open sesame",
        Some(1),
    )
    .await;

    let head = |password: Option<&str>| {
        let mut req = Request::builder()
            .method(Method::HEAD)
            .uri(format!("/share/{token}"));
        if let Some(password) = password {
            req = req.header("x-share-password", password);
        }
        req.body(axum::body::Body::empty()).unwrap()
    };

    // No password: refused, and the reason is machine-readable so the public
    // page knows to ask for one instead of showing "unavailable".
    let (status, headers, body) = raw(&mut router, head(None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        headers.get("x-hv-share-auth").and_then(|v| v.to_str().ok()),
        Some("password")
    );
    assert!(body.is_empty());

    // Wrong password: refused, still no quota spent.
    let (status, _, _) = raw(&mut router, head(Some("wrong password"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Right password: the probe says "downloadable" and spends nothing.
    let (status, _, body) = raw(&mut router, head(Some("open sesame"))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty(), "a HEAD must not carry the file");

    let (_, list) = send(
        &mut router,
        Method::GET,
        "/api/explorer/shares",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(list["data"][0]["download_count"], 0);
    assert_eq!(list["data"][0]["exhausted"], false);

    // The real download works, then the probe tells the truth about the spent
    // link instead of promising a download that would be refused.
    let (status, _, bytes) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .header("x-share-password", "open sesame")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], b"protected payload");
    settle().await;

    let (status, _, _) = raw(&mut router, head(Some("open sesame"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_share_link_unlock_cookie_flow() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    let (_admin, token) = password_link(
        &mut router,
        &h.state,
        &h.root,
        "unlocked.txt",
        "correct horse",
        None,
    )
    .await;
    let unlock_uri = format!("/share/{token}/unlock");

    // A wrong password is refused, and says which factor failed.
    let (status, headers, _) = raw(
        &mut router,
        Request::builder()
            .method(Method::POST)
            .uri(&unlock_uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"password": "wrong"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        headers.get("x-hv-share-auth").and_then(|v| v.to_str().ok()),
        Some("password")
    );

    // The right one mints a short-lived, path-scoped, HttpOnly cookie.
    let (status, headers, _) = raw(
        &mut router,
        Request::builder()
            .method(Method::POST)
            .uri(&unlock_uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"password": "correct horse"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = headers
        .get(http::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie");
    assert!(cookie.starts_with("hv_share_unlock="));
    assert!(cookie.contains(&format!("Path=/share/{token}")));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    let nonce = unlock_nonce(&headers);

    // The cookie — not the password — authorises the download.
    let (status, headers, bytes) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .header(http::header::COOKIE, format!("hv_share_unlock={nonce}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], b"protected payload");
    // A grant that can expire, be revoked or run out must not be replayed from
    // a cache — that would keep a spent one-time link alive in that browser.
    assert_eq!(
        headers
            .get(http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );

    // A forged nonce is not an unlock, and neither is the password in the URL —
    // it would land in the access log and the browser history.
    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .header(http::header::COOKIE, "hv_share_unlock=forged")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}?password=correct%20horse"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a password in the query string must never authenticate"
    );
}

#[tokio::test]
async fn test_share_link_wrong_passwords_are_throttled() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    let (_admin, token) = password_link(
        &mut router,
        &h.state,
        &h.root,
        "throttled.txt",
        "correct horse",
        None,
    )
    .await;

    // Empty probes (the public page asking "is a password needed?") are free.
    for _ in 0..20 {
        let (status, _, _) = raw(
            &mut router,
            Request::builder()
                .method(Method::HEAD)
                .uri(format!("/share/{token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // Eight real guesses are checked with Argon2, the ninth is refused before
    // the hash is ever touched.
    for attempt in 1..=8 {
        let (status, _, _) = raw(
            &mut router,
            public_get(&format!("/share/{token}"))
                .header("x-share-password", "guess")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {attempt}");
    }
    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .header("x-share-password", "guess")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // Even the correct password waits out the window.
    let (status, _, _) = raw(
        &mut router,
        public_get(&format!("/share/{token}"))
            .header("x-share-password", "correct horse")
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

/// The unlock endpoint must be reachable through the *real* production router,
/// next to the download — not only in the minimal test router above.
#[tokio::test]
async fn test_share_unlock_reachable_through_production_router() {
    let h = setup().await;
    create_test_user(&h.state, "admin", "pass", 2).await;

    let repo = homevault::storage::ShareLinkRepository::new(h.state.db.pool());
    let hash = h.state.auth.hash_password("hunter2").unwrap();
    let link = repo
        .create(
            "admin",
            &"B".repeat(48),
            &make_file(&h.root, "prod-unlock.txt", "unlocked over the app"),
            "anyone",
            None,
            Some(&hash),
            Some(1),
            false,
            None,
            "",
            "",
        )
        .await
        .expect("create link");

    let mut app = homevault::web::create_app(h.state.clone());
    let (status, headers, _) = raw(
        &mut app,
        Request::builder()
            .method(Method::POST)
            .uri(format!("/share/{}/unlock", link.token))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"password": "hunter2"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let nonce = unlock_nonce(&headers);

    let (status, _, bytes) = raw(
        &mut app,
        public_get(&format!("/share/{}", link.token))
            .header(http::header::COOKIE, format!("hv_share_unlock={nonce}"))
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&bytes[..], b"unlocked over the app");
}
