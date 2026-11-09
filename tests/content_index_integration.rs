/* =============================================================================
 * File          : content_index_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Content-index integration tests (phase D1): FTS5 schema and triggers
 *   (insert, update, delete), repository retention, the admin configuration
 *   guard, "delete my index", and the ACL filter on the content search
 *   endpoint. The share used by the tests is an empty temp folder, so a run
 *   never walks the machine's real `/tmp`.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test content_index_integration
 * ============================================================================= */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the content index.

use axum::routing::{delete, get, post, put};
use axum::Router;
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::content_index::{self, repo, ContentIndexConfig};
use homevault::handlers::auth::handle_login;
use homevault::handlers::content_index::{
    handle_delete, handle_get, handle_run, handle_search, handle_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Fixture {
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
    share: PathBuf,
}

async fn setup_state() -> Fixture {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());
    let share = tmp.path().join("share");
    std::fs::create_dir_all(&share).expect("share");

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    let path_guard = PathGuard::new(vec![share.to_string_lossy().to_string()]);
    let admin_rate_limiter = Arc::new(RateLimitState::new(100, 60));
    let pty_manager = Arc::new(PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    Fixture {
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
        share,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/search/content", get(handle_search))
        .route("/api/search/index", get(handle_get))
        .route("/api/search/index", put(handle_update))
        .route("/api/search/index/run", post(handle_run))
        .route("/api/search/index", delete(handle_delete))
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, role_id: i64) {
    let password_hash = state.auth.hash_password("Password123!").unwrap();
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

async fn login(router: &mut Router, username: &str) -> String {
    let (_, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": "Password123!"})),
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
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn doc(path: &str, root: &str, body: &str) -> repo::ContentDoc {
    repo::ContentDoc {
        path: path.to_string(),
        root: root.to_string(),
        name: path.rsplit('/').next().unwrap_or(path).to_string(),
        size_bytes: body.len() as i64,
        modified: 1,
    }
}

// ---------------------------------------------------------------------------
// 1. FTS5 schema and triggers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_insert_update_delete_keep_the_fts_index_in_sync() {
    let f = setup_state().await;
    let pool = f.state.db.pool();

    repo::upsert(
        pool.as_ref(),
        &doc("/srv/a.txt", "/srv", "watermelon"),
        "watermelon",
        "now",
    )
    .await
    .unwrap();
    let hits = repo::search(pool.as_ref(), "\"watermelon\"", 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("watermelon"));

    // An update re-indexes the body: the old term must be gone.
    repo::upsert(
        pool.as_ref(),
        &doc("/srv/a.txt", "/srv", "pineapple"),
        "pineapple",
        "later",
    )
    .await
    .unwrap();
    assert!(repo::search(pool.as_ref(), "\"watermelon\"", 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        repo::search(pool.as_ref(), "\"pineapple\"", 10)
            .await
            .unwrap()
            .len(),
        1
    );

    repo::delete_all(pool.as_ref()).await.unwrap();
    assert!(repo::search(pool.as_ref(), "\"pineapple\"", 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn test_prune_and_retention_remove_documents() {
    let f = setup_state().await;
    let pool = f.state.db.pool();
    repo::upsert(
        pool.as_ref(),
        &doc("/srv/a.txt", "/srv", "one"),
        "one",
        "2020-01-01T00:00:00Z",
    )
    .await
    .unwrap();
    repo::upsert(
        pool.as_ref(),
        &doc("/srv/b.txt", "/srv", "two"),
        "two",
        "2026-01-01T00:00:00Z",
    )
    .await
    .unwrap();

    let deleted = repo::prune_older_than(pool.as_ref(), "2021-01-01T00:00:00Z")
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(repo::stats(pool.as_ref()).await.unwrap().0, 1);

    let deleted = repo::prune_root(pool.as_ref(), "/srv", "2026-06-01T00:00:00Z")
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(repo::stats(pool.as_ref()).await.unwrap().0, 0);
}

// ---------------------------------------------------------------------------
// 2. Guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_index_routes_require_an_admin() {
    let f = setup_state().await;
    create_user(&f.state, "member", 1).await;
    let mut r = router(f.state.clone());
    let member = login(&mut r, "member").await;

    for (method, uri, body) in [
        (Method::GET, "/api/search/index", None),
        (Method::POST, "/api/search/index/run", None),
        (
            Method::PUT,
            "/api/search/index",
            Some(json!({"config": ContentIndexConfig::default()})),
        ),
        (Method::DELETE, "/api/search/index?confirm=true", None),
    ] {
        let (status, _) = send(&mut r, method.clone(), uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }

    // And without a session at all.
    for (method, uri) in [
        (Method::GET, "/api/search/index"),
        (Method::GET, "/api/search/content?q=hello"),
    ] {
        let (status, _) = send(&mut r, method.clone(), uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

// ---------------------------------------------------------------------------
// 3. Configuration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_admin_reads_the_default_configuration_and_candidates() {
    let f = setup_state().await;
    create_user(&f.state, "admin", 2).await;
    let mut r = router(f.state.clone());
    let token = login(&mut r, "admin").await;

    let (status, body) = send(&mut r, Method::GET, "/api/search/index", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["config"]["enabled"], false);
    let candidates = body["data"]["candidates"].as_array().unwrap();
    assert!(candidates
        .iter()
        .any(|c| c.as_str().unwrap() == f.share.to_string_lossy()));
    assert_eq!(body["data"]["docs"], 0);
}

#[tokio::test]
async fn test_enabling_requires_confirmation_and_an_allowed_root() {
    let f = setup_state().await;
    create_user(&f.state, "admin", 2).await;
    let mut r = router(f.state.clone());
    let token = login(&mut r, "admin").await;
    let share = f.share.to_string_lossy().to_string();

    let mut config = ContentIndexConfig {
        enabled: true,
        roots: vec![share.clone()],
        ..ContentIndexConfig::default()
    };
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/search/index",
        Some(&token),
        Some(json!({"config": config})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("confirmed"));

    config.confirmed = true;
    config.roots = vec!["/etc".to_string()];
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/search/index",
        Some(&token),
        Some(json!({"config": config})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("not one of this appliance's Explorer roots"));

    config.roots = vec![share];
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/search/index",
        Some(&token),
        Some(json!({"config": config})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["config"]["enabled"], true);
}

#[tokio::test]
async fn test_delete_my_index_requires_confirmation_and_disables_it() {
    let f = setup_state().await;
    create_user(&f.state, "admin", 2).await;
    let mut r = router(f.state.clone());
    let token = login(&mut r, "admin").await;
    repo::upsert(
        f.state.db.pool().as_ref(),
        &doc("/srv/a.txt", "/srv", "secret"),
        "secret",
        "now",
    )
    .await
    .unwrap();

    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/search/index",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/search/index?confirm=true",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["deleted"], 1);
    assert_eq!(body["data"]["config"]["enabled"], false);
    assert_eq!(repo::stats(f.state.db.pool().as_ref()).await.unwrap().0, 0);
}

// ---------------------------------------------------------------------------
// 4. Search + ACL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_is_off_until_enabled() {
    let f = setup_state().await;
    create_user(&f.state, "admin", 2).await;
    let mut r = router(f.state.clone());
    let token = login(&mut r, "admin").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/search/content?q=hello",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["available"], false);
    assert!(body["data"]["hits"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_content_search_filters_by_the_callers_roots() {
    let f = setup_state().await;
    create_user(&f.state, "reader", 1).await;
    let mut r = router(f.state.clone());
    let token = login(&mut r, "reader").await;
    let share = f.share.to_string_lossy().to_string();

    // The member's Explorer roots come from explicit grants; give them the
    // test folder so the search has a visible root (the `/etc/shadow` row
    // stays outside it).
    sqlx::query(
        "INSERT INTO resource_shares (owner, member, kind, resource, permission) \
         VALUES ('reader', 'reader', 'folder', ?, 'read')",
    )
    .bind(&share)
    .execute(&*f.state.db.pool())
    .await
    .expect("grant the test folder");

    // Turn the index on with the test share as its only root.
    let config = ContentIndexConfig {
        enabled: true,
        confirmed: true,
        roots: vec![share.clone()],
        ..ContentIndexConfig::default()
    };
    content_index::save_persisted(&config).await.unwrap();

    let visible_path = format!("{share}/notes.txt");
    repo::upsert(
        f.state.db.pool().as_ref(),
        &doc(&visible_path, &share, "the watermelon is ripe"),
        "the watermelon is ripe",
        "now",
    )
    .await
    .unwrap();
    repo::upsert(
        f.state.db.pool().as_ref(),
        &doc("/etc/shadow", "/etc", "watermelon"),
        "watermelon",
        "now",
    )
    .await
    .unwrap();

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/search/content?q=watermelon",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let hits = body["data"]["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{body}");
    assert_eq!(hits[0]["path"], visible_path);
    assert_eq!(hits[0]["root_kind"], "storage");
    assert!(hits[0]["snippet"].as_str().unwrap().contains("watermelon"));
}

#[tokio::test]
async fn test_a_query_with_only_operators_returns_an_empty_answer() {
    let f = setup_state().await;
    create_user(&f.state, "reader", 1).await;
    let mut r = router(f.state.clone());
    let token = login(&mut r, "reader").await;
    let share = f.share.to_string_lossy().to_string();
    let config = ContentIndexConfig {
        enabled: true,
        confirmed: true,
        roots: vec![share],
        ..ContentIndexConfig::default()
    };
    content_index::save_persisted(&config).await.unwrap();

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/search/content?q=--",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["hits"].as_array().unwrap().is_empty());
}
