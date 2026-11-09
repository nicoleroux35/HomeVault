/* =============================================================================
 * File          : remote_mounts_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Network-location (CIFS/NFS remote mount) integration tests: in-memory
 *   SQLite + assembled Axum router, exercised through oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test remote_mounts_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for `/api/remote-mounts` (network locations).
//!
//! **No mount is ever performed here.** The suite only drives the refusal
//! paths (auth, name/server/option validation) and the "save without
//! connecting" path, so it needs no `mount.cifs`/`mount.nfs`, no network and
//! no root: `mount_now` is always false and the mount/unmount endpoints are
//! only exercised with a name that fails validation before any syscall.
//!
//! Run: cargo test --test remote_mounts_integration

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    remote_mounts::{
        handle_remote_mount_create, handle_remote_mount_delete, handle_remote_mount_mount,
        handle_remote_mount_unmount, handle_remote_mount_update, handle_remote_mounts_list,
        handle_remote_mounts_reconcile,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

type Req = Request<axum::body::Body>;

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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route(
            "/api/remote-mounts",
            get(handle_remote_mounts_list).post(handle_remote_mount_create),
        )
        .route(
            "/api/remote-mounts/reconcile",
            axum::routing::post(handle_remote_mounts_reconcile),
        )
        .route(
            "/api/remote-mounts/{name}",
            axum::routing::put(handle_remote_mount_update).delete(handle_remote_mount_delete),
        )
        .route(
            "/api/remote-mounts/{name}/mount",
            axum::routing::post(handle_remote_mount_mount),
        )
        .route(
            "/api/remote-mounts/{name}/unmount",
            axum::routing::post(handle_remote_mount_unmount),
        )
        .with_state(state)
}

async fn mk_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'T', 't@e.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&hash)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("insert user");
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Every network-location route is admin-only: the list reveals which remote
/// hosts the appliance talks to, and the mutations run root-owned `mount`.
#[tokio::test]
async fn remote_mounts_are_admin_only() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    mk_user(&state, "member", "pw012345", 1).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let member = login(&mut r, "member", "pw012345").await;

    let (status, _) = send(&mut r, Method::GET, "/api/remote-mounts", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/remote-mounts",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/remote-mounts",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["available"].is_boolean());
    assert!(data["kinds"].is_array());
    assert_eq!(data["root"], json!("/mnt/homevault-remote"));
    assert_eq!(
        data["mounts"],
        json!([]),
        "a fresh appliance mounts nothing"
    );
}

/// The name becomes a directory: it is a strict slug, and a refilled value
/// gets the rule, not "invalid".
#[tokio::test]
async fn remote_mount_rejects_an_unsafe_name() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    for bad in ["../etc", "Photos", "with space", "a/b"] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/remote-mounts",
            Some(&admin),
            Some(json!({
                "name": bad,
                "kind": "cifs",
                "server": "nas.example.com",
                "source": "photos",
                "mount_now": false,
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("lowercase"),
            "{bad}: {body}"
        );
    }
}

/// The server and the share/export are spliced into `//server/share` and
/// `server:/export`, so anything that could add an option or a path is a 400.
#[tokio::test]
async fn remote_mount_rejects_an_injected_server_or_source() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts",
        Some(&admin),
        Some(json!({
            "name": "photos",
            "kind": "cifs",
            "server": "-oProxyCommand=touch /tmp/x",
            "source": "photos",
            "mount_now": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts",
        Some(&admin),
        Some(json!({
            "name": "photos",
            "kind": "cifs",
            "server": "nas.example.com",
            "source": "photos/2024",
            "mount_now": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("share name only"));
}

/// Options are an allowlist: a credential key can never be smuggled through
/// them, and `ro`/`rw` stay the dedicated field.
#[tokio::test]
async fn remote_mount_rejects_a_credential_option() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts",
        Some(&admin),
        Some(json!({
            "name": "photos",
            "kind": "cifs",
            "server": "nas.example.com",
            "source": "photos",
            "options": ["credentials=/etc/shadow"],
            "mount_now": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("Allowed"));
}

/// Saving without connecting never runs `mount`, so it is safe on any host:
/// either the location is stored, or the protocol's helper is missing and the
/// answer names it (`503` semantics).
#[tokio::test]
async fn remote_mount_saves_without_connecting() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts",
        Some(&admin),
        Some(json!({
            "name": "photos-nas",
            "kind": "cifs",
            "server": "nas.example.com",
            "source": "photos",
            "read_only": true,
            "enabled": false,
            "mount_now": false,
        })),
    )
    .await;

    if status == StatusCode::SERVICE_UNAVAILABLE {
        // This host has no cifs-utils: the request is refused, not silently
        // stored as something that can never mount.
        assert!(body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("mount.cifs"));
        return;
    }

    assert_eq!(status, StatusCode::OK, "{body}");
    let mount = &body["data"];
    assert_eq!(mount["name"], json!("photos-nas"));
    assert_eq!(mount["target"], json!("/mnt/homevault-remote/photos-nas"));
    assert_eq!(mount["read_only"], json!(true));
    assert_eq!(mount["enabled"], json!(false));
    assert_eq!(mount["mounted"], json!(false));
    assert_eq!(mount["has_credentials"], json!(false));

    // It survives as a configured location.
    let (_, listed) = send(
        &mut r,
        Method::GET,
        "/api/remote-mounts",
        Some(&admin),
        None,
    )
    .await;
    let mounts = listed["data"]["mounts"].as_array().expect("mounts array");
    assert_eq!(mounts.len(), 1);
    assert_eq!(mounts[0]["name"], json!("photos-nas"));

    // A duplicate name is refused instead of silently replacing the other.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts",
        Some(&admin),
        Some(json!({
            "name": "photos-nas",
            "kind": "cifs",
            "server": "other.example.com",
            "source": "photos",
            "mount_now": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("already exists"));

    // Removing it never runs `umount` (it is not mounted) and reports what it
    // did.
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/remote-mounts/photos-nas",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["removed"], json!("photos-nas"));
}

/// Unmounting (or mounting) a name that fails validation is refused before
/// any syscall, so this suite never touches the host.
#[tokio::test]
async fn remote_mount_actions_validate_the_name_first() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts/BAD/mount",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts/BAD/unmount",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A well-formed but unknown name is a plain error, not a panic.
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/remote-mounts/missing",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("No network location"));
}

/// "Reconnect all" on an appliance with no configured location mounts nothing
/// and reports an empty failure list.
#[tokio::test]
async fn remote_mount_reconcile_with_nothing_configured() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    mk_user(&state, "member", "pw012345", 1).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let member = login(&mut r, "member", "pw012345").await;

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts/reconcile",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/remote-mounts/reconcile",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["failed"], json!([]));
    assert_eq!(body["data"]["mounts"], json!([]));
}
