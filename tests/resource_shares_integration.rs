/* =============================================================================
 * File          : resource_shares_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Resource shares integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for per-member resource access —
//! `crate::handlers::resource_shares`:
//!
//!   GET    /api/shares/access       (admin)
//!   GET    /api/shares/access/mine  (any session)
//!   POST   /api/shares/access       (admin)
//!   DELETE /api/shares/access       (admin)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server. Nothing here
//! touches the host.
//!
//! Run:
//!   cargo test --test resource_shares_integration

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::resource_shares::{
    handle_grant_access, handle_grant_group_access, handle_list_access, handle_list_group_access,
    handle_my_access, handle_revoke_access, handle_revoke_group_access,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::resource_share_repo::{
    ResourceKind, ResourcePermission, ResourceShareRepository,
};
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

fn access_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/shares/access",
            get(handle_list_access)
                .post(handle_grant_access)
                .delete(handle_revoke_access),
        )
        .route("/api/shares/access/mine", get(handle_my_access))
        .route(
            "/api/shares/access/groups",
            get(handle_list_group_access)
                .post(handle_grant_group_access)
                .delete(handle_revoke_group_access),
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
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_access_requires_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", "pw12345678", 1).await;
    let mut r = access_router(state);

    let (status, _) = send(&mut r, Method::GET, "/api/shares/access", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = login(&mut r, "member", "pw12345678").await;
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/shares/access",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_my_access_requires_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = access_router(state);

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/shares/access/mine",
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 2. Grant / list / revoke lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grant_then_member_sees_exactly_that_resource() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    create_test_user(&state, "alice", "pw12345678", 1).await;
    let mut r = access_router(state);

    let admin = login(&mut r, "admin", "pw12345678").await;
    let alice = login(&mut r, "alice", "pw12345678").await;

    // Before any grant, alice sees nothing.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/access/mine",
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["all"], json!(false));
    assert_eq!(body["data"]["apps"].as_array().unwrap().len(), 0);

    // Admin grants app access.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/access",
        Some(&admin),
        Some(json!({"member": "alice", "kind": "app", "resource": "jellyfin", "permission": "write"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["resource"], json!("jellyfin"));
    assert_eq!(body["data"]["permission"], json!("write"));

    // Alice now sees exactly that one app.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/access/mine",
        Some(&alice),
        None,
    )
    .await;
    let apps = body["data"]["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["resource"], json!("jellyfin"));
    assert_eq!(apps[0]["permission"], json!("write"));
}

#[tokio::test]
async fn test_admin_sees_all_flag_and_empty_lists() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    let mut r = access_router(state);
    let admin = login(&mut r, "admin", "pw12345678").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/access/mine",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["is_admin"], json!(true));
    assert_eq!(body["data"]["all"], json!(true));
    assert_eq!(body["data"]["apps"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_grant_is_idempotent_and_updates_permission() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    create_test_user(&state, "bob", "pw12345678", 1).await;
    let mut r = access_router(state);
    let admin = login(&mut r, "admin", "pw12345678").await;

    for perm in ["read", "full"] {
        let (status, _) = send(
            &mut r,
            Method::POST,
            "/api/shares/access",
            Some(&admin),
            Some(
                json!({"member": "bob", "kind": "share", "resource": "photos", "permission": perm}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/access",
        Some(&admin),
        None,
    )
    .await;
    let grants = body["data"].as_array().unwrap();
    assert_eq!(grants.len(), 1, "upsert must not duplicate the row");
    assert_eq!(grants[0]["permission"], json!("full"));
}

#[tokio::test]
async fn test_revoke_removes_access() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    create_test_user(&state, "carol", "pw12345678", 1).await;
    let mut r = access_router(state);
    let admin = login(&mut r, "admin", "pw12345678").await;
    let carol = login(&mut r, "carol", "pw12345678").await;

    send(
        &mut r,
        Method::POST,
        "/api/shares/access",
        Some(&admin),
        Some(json!({"member": "carol", "kind": "folder", "resource": "/srv/media"})),
    )
    .await;

    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/shares/access?member=carol&kind=folder&resource=/srv/media",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/access/mine",
        Some(&carol),
        None,
    )
    .await;
    assert_eq!(body["data"]["folders"].as_array().unwrap().len(), 0);

    // Revoking again is a 400 (no such grant), not a silent success.
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/shares/access?member=carol&kind=folder&resource=/srv/media",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 3. Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grant_rejects_unknown_kind_and_permission() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    let mut r = access_router(state);
    let admin = login(&mut r, "admin", "pw12345678").await;

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shares/access",
        Some(&admin),
        Some(json!({"member": "x", "kind": "disk", "resource": "sda"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shares/access",
        Some(&admin),
        Some(json!({"member": "x", "kind": "app", "resource": "y", "permission": "admin"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shares/access",
        Some(&admin),
        Some(json!({"member": "", "kind": "app", "resource": "y"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_grant_defaults_permission_to_read() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    create_test_user(&state, "dave", "pw12345678", 1).await;
    let mut r = access_router(state);
    let admin = login(&mut r, "admin", "pw12345678").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/access",
        Some(&admin),
        Some(json!({"member": "dave", "kind": "app", "resource": "nextcloud"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["permission"], json!("read"));
}

// ---------------------------------------------------------------------------
// 4. Repository-level server-side filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_visible_resources_and_can_access_filter_by_member() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    create_test_user(&state, "erin", "pw12345678", 1).await;
    let repo = ResourceShareRepository::new(state.db.pool());

    repo.upsert(
        "admin",
        "erin",
        ResourceKind::App,
        "jellyfin",
        ResourcePermission::Read,
    )
    .await
    .unwrap();
    repo.upsert(
        "admin",
        "erin",
        ResourceKind::App,
        "nextcloud",
        ResourcePermission::Full,
    )
    .await
    .unwrap();
    repo.upsert(
        "admin",
        "erin",
        ResourceKind::Share,
        "photos",
        ResourcePermission::Write,
    )
    .await
    .unwrap();

    let apps = repo
        .visible_resources("erin", ResourceKind::App)
        .await
        .unwrap();
    assert_eq!(
        apps,
        vec![
            ("jellyfin".to_string(), "read".to_string()),
            ("nextcloud".to_string(), "full".to_string()),
        ]
    );

    // can_access honours the level: read does not imply write.
    assert!(repo
        .can_access(
            "erin",
            ResourceKind::App,
            "jellyfin",
            ResourcePermission::Read
        )
        .await
        .unwrap());
    assert!(!repo
        .can_access(
            "erin",
            ResourceKind::App,
            "jellyfin",
            ResourcePermission::Write
        )
        .await
        .unwrap());
    assert!(repo
        .can_access(
            "erin",
            ResourceKind::App,
            "nextcloud",
            ResourcePermission::Write
        )
        .await
        .unwrap());
    // A resource with no row is invisible.
    assert!(!repo
        .can_access("erin", ResourceKind::App, "plex", ResourcePermission::Read)
        .await
        .unwrap());
    // A different member shares nothing.
    assert!(!repo
        .can_access(
            "frank",
            ResourceKind::App,
            "jellyfin",
            ResourcePermission::Read
        )
        .await
        .unwrap());
}

#[tokio::test]
async fn test_revoke_all_for_member_clears_every_kind() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "pw12345678", 2).await;
    create_test_user(&state, "gina", "pw12345678", 1).await;
    let repo = ResourceShareRepository::new(state.db.pool());

    repo.upsert(
        "admin",
        "gina",
        ResourceKind::App,
        "a",
        ResourcePermission::Read,
    )
    .await
    .unwrap();
    repo.upsert(
        "admin",
        "gina",
        ResourceKind::Folder,
        "/srv/x",
        ResourcePermission::Read,
    )
    .await
    .unwrap();
    repo.upsert(
        "admin",
        "gina",
        ResourceKind::Share,
        "s",
        ResourcePermission::Read,
    )
    .await
    .unwrap();

    let removed = repo.revoke_all_for_member("gina").await.unwrap();
    assert_eq!(removed, 3);
    assert!(repo.list_for_member("gina", None).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Group-level grants (migration 051, 2026-10-20)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_group_grant_crud_and_listing() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "pw12345678", 2).await;
    let mut r = access_router(state);
    let token = login(&mut r, "admin1", "pw12345678").await;

    // Grant to a group.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/access/groups",
        Some(&token),
        Some(json!({"group": "family", "kind": "app", "resource": "jellyfin", "permission": "write"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["group_name"], json!("family"));
    assert_eq!(body["data"]["permission"], json!("write"));

    // Idempotent: a second grant updates the level, no duplicate row.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shares/access/groups",
        Some(&token),
        Some(
            json!({"group": "family", "kind": "app", "resource": "jellyfin", "permission": "full"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/access/groups?group=family",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["data"].as_array().expect("array");
    assert_eq!(rows.len(), 1, "one row per (group, kind, resource)");
    assert_eq!(rows[0]["permission"], json!("full"));

    // Revoke, then revoking again is "no such grant".
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/shares/access/groups?group=family&kind=app&resource=jellyfin",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/shares/access/groups?group=family&kind=app&resource=jellyfin",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_group_grant_endpoints_are_admin_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", "pw12345678", 1).await;
    let mut r = access_router(state);
    let token = login(&mut r, "member", "pw12345678").await;

    for (method, uri, body) in [
        (Method::GET, "/api/shares/access/groups", None),
        (
            Method::POST,
            "/api/shares/access/groups",
            Some(json!({"group": "family", "kind": "app", "resource": "jellyfin"})),
        ),
        (
            Method::DELETE,
            "/api/shares/access/groups?group=family&kind=app&resource=jellyfin",
            None,
        ),
    ] {
        let (status, _) = send(&mut r, method.clone(), uri, Some(&token), body).await;
        assert_ne!(
            status,
            StatusCode::OK,
            "{} {} must be admin-only",
            method,
            uri
        );
    }
}

#[tokio::test]
async fn test_group_grant_rejects_unknown_kind() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "pw12345678", 2).await;
    let mut r = access_router(state);
    let token = login(&mut r, "admin1", "pw12345678").await;

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shares/access/groups",
        Some(&token),
        Some(json!({"group": "family", "kind": "disk", "resource": "x"})),
    )
    .await;
    assert_ne!(status, StatusCode::OK);
}

/// The union rule end-to-end, without needing real OS groups: the repository
/// is the same one the handlers call, and `merge_strongest` is the single
/// place the "strongest wins" decision lives.
#[tokio::test]
async fn test_effective_access_unions_direct_and_group_grants() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "pw12345678", 2).await;
    create_test_user(&state, "member", "pw12345678", 1).await;
    let repo = ResourceShareRepository::new(state.db.pool());

    // Direct: read on the app. Group: full on the same app + write on another.
    repo.upsert(
        "admin1",
        "member",
        ResourceKind::App,
        "jellyfin",
        ResourcePermission::Read,
    )
    .await
    .expect("direct grant");
    repo.upsert_group(
        "admin1",
        "family",
        ResourceKind::App,
        "jellyfin",
        ResourcePermission::Full,
    )
    .await
    .expect("group grant");
    repo.upsert_group(
        "admin1",
        "family",
        ResourceKind::App,
        "photos",
        ResourcePermission::Write,
    )
    .await
    .expect("group grant 2");

    let direct = repo
        .visible_resources("member", ResourceKind::App)
        .await
        .expect("direct rows");
    let inherited = repo
        .visible_resources_for_groups(&["family".to_string()], ResourceKind::App)
        .await
        .expect("group rows");
    let merged = ResourceShareRepository::merge_strongest(direct, inherited);

    assert_eq!(
        merged,
        vec![
            ("jellyfin".to_string(), "full".to_string()),
            ("photos".to_string(), "write".to_string()),
        ],
        "the union keeps the strongest level per resource"
    );

    // A group the member is not in contributes nothing.
    let other = repo
        .visible_resources_for_groups(&["strangers".to_string()], ResourceKind::App)
        .await
        .expect("empty");
    assert!(other.is_empty());

    // Revoking the group grant demotes nobody who holds a direct row, and
    // removes the group-only one.
    repo.revoke_group("family", ResourceKind::App, "photos")
        .await
        .expect("revoke");
    let direct = repo
        .visible_resources("member", ResourceKind::App)
        .await
        .expect("direct rows");
    let inherited = repo
        .visible_resources_for_groups(&["family".to_string()], ResourceKind::App)
        .await
        .expect("group rows");
    let merged = ResourceShareRepository::merge_strongest(direct, inherited);
    assert_eq!(
        merged,
        vec![("jellyfin".to_string(), "full".to_string())],
        "the direct row survives, the group-only one is gone"
    );
}

#[tokio::test]
async fn test_revoke_all_for_group_drops_every_kind() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "pw12345678", 2).await;
    let repo = ResourceShareRepository::new(state.db.pool());
    for (kind, resource) in [
        (ResourceKind::App, "jellyfin"),
        (ResourceKind::Folder, "/srv/media"),
        (ResourceKind::Share, "photos"),
    ] {
        repo.upsert_group("admin1", "family", kind, resource, ResourcePermission::Read)
            .await
            .expect("grant");
    }

    let deleted = repo.revoke_all_for_group("family").await.expect("purge");
    assert_eq!(deleted, 3);
    assert!(repo
        .list_groups_all(Some("family"))
        .await
        .expect("list")
        .is_empty());
}
