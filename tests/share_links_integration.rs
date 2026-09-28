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

use axum::{routing::{delete, get, post}, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::share_links::{
    handle_share_link_create, handle_share_link_delete, handle_share_link_download,
    handle_share_links_list,
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
        homevault::handlers::downloader::Aria2Process { child: None },
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
        .route("/api/explorer/shares/{token}", delete(handle_share_link_delete))
        .route("/share/{token}", get(handle_share_link_download))
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
    let json = serde_json::from_slice(&bytes)
        .unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

/// Creates a real file in the temp root and returns its absolute path.
fn make_file(root: &PathBuf, name: &str, content: &str) -> String {
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
    let (s, _) = send(&mut router, Method::DELETE, "/api/explorer/shares/sometoken", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Garbage token.
    let (s, _) = send(&mut router, Method::GET, "/api/explorer/shares", Some("garbage"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Valid session works.
    let (s, body) = send(&mut router, Method::GET, "/api/explorer/shares", Some(&admin), None).await;
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
    let (s, body) = send(&mut router, Method::GET, "/api/explorer/shares", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    let links = body["data"].as_array().expect("array");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["token"], token);
}

#[tokio::test]
async fn test_share_link_rejects_directories_and_missing_files() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;

    // Directory → 400.
    let (s, _) = send(
        &mut router,
        Method::POST,
        "/api/explorer/shares",
        Some(&admin),
        Some(json!({"path": h.root.to_string_lossy().to_string()})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

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
    let share_path = body["data"]["share_path"].as_str().expect("share_path").to_string();

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
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
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
                .uri(&format!("/share/{}", token))
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
    sqlx::query("UPDATE explorer_share_links SET expires_at = '2000-01-01T00:00:00Z' WHERE token = ?")
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
                .uri(&format!("/share/{}", token))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // It also no longer shows as downloadable from the list (expired flag).
    let (_, body) = send(&mut router, Method::GET, "/api/explorer/shares", Some(&admin), None).await;
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
        .create("admin", &"A".repeat(48), &file, "anyone", None)
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
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"prod payload");
}