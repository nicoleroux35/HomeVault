/* =============================================================================
 * File          : shares_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shares integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Shares / Protocols API
//! (`crate::handlers::shares` + `crate::handlers::permissions`):
//!
//!   - GET    /api/shares                                    (list every protocol's shares)
//!   - POST   /api/shares                                    (create a share)
//!   - GET    /api/shares/{name}/{type}                      (get one share)
//!   - PUT    /api/shares/{name}/{type}                      (update a share)
//!   - PUT    /api/shares/{name}/{type}/toggle/{enabled}     (enable/disable)
//!   - DELETE /api/shares/{name}/{type}                      (delete a share)
//!   - GET|PUT /api/shares/{smb,nfs,ftp,webdav,sftp}/config  (per-protocol service config)
//!   - GET    /api/shares/sftp/users                         (SFTP grantees)
//!   - PUT    /api/shares/sftp/users/key                     (set an SFTP public key)
//!   - POST   /api/shares/sftp/rebuild                       (reconcile SFTP jails)
//!   - POST   /api/shares/{type}/{start,stop,restart}        (service control)
//!   - GET|POST|DELETE /api/shares/{name}/{type}/permissions (ACL grants)
//!   - POST   /api/shares/{name}/{type}/permissions/sync     (DB → filesystem ACL sync)
//!   - GET    /api/users/{username}/permissions              (a user's effective grants)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! **This suite never creates, deletes, toggles or reconfigures a real
//! share, never starts/stops/restarts smbd/nfs-server/vsftpd/the SFTP sshd
//! instance, and never provisions a real Linux account or bind mount.** Per
//! CLAUDE.md's "What NOT to test in Rust", every one of those paths shells
//! out via `sudo` to `tee`/`systemctl`/`exportfs`/`useradd`/`mount --bind`
//! or writes a root-owned file under `/etc` — they need the real host and
//! are exercised end-to-end (against disposable state) by
//! `tests/e2e/shares/shares.spec.ts`. Here the mutating endpoints are only
//! driven far enough to confirm they:
//!   * reject a missing / non-admin / tampered session (400 / 401), and
//!   * reject invalid input (bad protocol name, bad permission string, bad
//!     grantee type, a share that doesn't exist) *before* any privileged
//!     side effect — a clean error response, never a panic / 500.
//!
//! The read-only endpoints (share list, per-protocol config GET, SFTP
//! grantee list, permission list) are safe to run for real and their
//! success shape is asserted directly.
//!
//! Run:
//!   cargo test --test shares_integration
//!   cargo test --test shares_integration -- --nocapture

use axum::{
    routing::{delete, get, post, put},
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
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    permissions::{
        handle_share_permission_grant, handle_share_permission_revoke,
        handle_share_permissions_list, handle_share_permissions_sync, handle_user_permissions,
    },
    shares::{
        handle_ftp_config_get, handle_ftp_config_update, handle_nfs_config_get,
        handle_nfs_config_update, handle_sftp_config_get, handle_sftp_config_update,
        handle_sftp_grantees_list, handle_sftp_key_set, handle_sftp_rebuild, handle_share_create,
        handle_share_delete, handle_share_get, handle_share_service_restart,
        handle_share_service_start, handle_share_service_stop, handle_share_toggle,
        handle_share_update, handle_shares_list, handle_smb_config_get, handle_smb_config_update,
        handle_webdav_config_get, handle_webdav_config_update,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

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

fn shares_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        // shares CRUD
        .route("/api/shares", get(handle_shares_list))
        .route("/api/shares", post(handle_share_create))
        .route("/api/shares/{name}/{share_type}", get(handle_share_get))
        .route("/api/shares/{name}/{share_type}", put(handle_share_update))
        .route(
            "/api/shares/{name}/{share_type}/toggle/{enabled}",
            put(handle_share_toggle),
        )
        .route(
            "/api/shares/{name}/{share_type}",
            delete(handle_share_delete),
        )
        // per-protocol service config
        .route("/api/shares/smb/config", get(handle_smb_config_get))
        .route("/api/shares/smb/config", put(handle_smb_config_update))
        .route("/api/shares/nfs/config", get(handle_nfs_config_get))
        .route("/api/shares/nfs/config", put(handle_nfs_config_update))
        .route("/api/shares/ftp/config", get(handle_ftp_config_get))
        .route("/api/shares/ftp/config", put(handle_ftp_config_update))
        .route("/api/shares/webdav/config", get(handle_webdav_config_get))
        .route(
            "/api/shares/webdav/config",
            put(handle_webdav_config_update),
        )
        .route("/api/shares/sftp/config", get(handle_sftp_config_get))
        .route("/api/shares/sftp/config", put(handle_sftp_config_update))
        .route("/api/shares/sftp/users", get(handle_sftp_grantees_list))
        .route("/api/shares/sftp/users/key", put(handle_sftp_key_set))
        .route("/api/shares/sftp/rebuild", post(handle_sftp_rebuild))
        // service control
        .route(
            "/api/shares/{share_type}/start",
            post(handle_share_service_start),
        )
        .route(
            "/api/shares/{share_type}/stop",
            post(handle_share_service_stop),
        )
        .route(
            "/api/shares/{share_type}/restart",
            post(handle_share_service_restart),
        )
        // permissions / ACL
        .route(
            "/api/shares/{name}/{share_type}/permissions",
            get(handle_share_permissions_list),
        )
        .route(
            "/api/shares/{name}/{share_type}/permissions",
            post(handle_share_permission_grant),
        )
        .route(
            "/api/shares/{name}/{share_type}/permissions",
            delete(handle_share_permission_revoke),
        )
        .route(
            "/api/shares/{name}/{share_type}/permissions/sync",
            post(handle_share_permissions_sync),
        )
        .route(
            "/api/users/{username}/permissions",
            get(handle_user_permissions),
        )
        .with_state(state)
}

/// Insert a user directly into the DB. `role_id` 2 = admin, 1 = regular.
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

/// A fully-populated SMB config body — used so a `PUT /config` request
/// reaches the auth guard instead of failing body deserialization (422)
/// first, which would test the wrong thing.
fn valid_smb_config() -> Value {
    json!({
        "workgroup": "WORKGROUP",
        "security_mode": "user",
        "protocol_version": "SMB3",
        "allow_guest": false,
        "create_mask": "0755",
        "directory_mask": "0755",
        "additional_params": []
    })
}

fn valid_nfs_config() -> Value {
    json!({
        "networks": ["*"],
        "default_access": "rw",
        "version": "4",
        "root_squash": "root_squash",
        "custom_options": []
    })
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

/// Every mutating share/permission side effect on a real deployment goes
/// through `sudo`. Assert a response never leaked a privileged success and
/// never crashed the handler — it either failed the auth gate or failed
/// cleanly with `success:false`, but the status is a real 4xx, not a 500.
fn assert_clean_rejection(status: StatusCode, body: &Value) {
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "handler panicked / 500'd instead of returning a clean error: {body}"
    );
    assert_eq!(
        body["success"],
        json!(false),
        "expected success:false, got: {body}"
    );
}

// ===========================================================================
// 1. Auth guards — every endpoint rejects a missing token
// ===========================================================================

#[tokio::test]
async fn test_all_endpoints_reject_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = shares_router(state);

    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/shares", None),
        (
            Method::POST,
            "/api/shares",
            Some(json!({"name":"x","share_type":"smb","path":"/tmp/x","comment":""})),
        ),
        (Method::GET, "/api/shares/x/smb", None),
        (
            Method::PUT,
            "/api/shares/x/smb",
            Some(json!({"enabled": false})),
        ),
        (Method::PUT, "/api/shares/x/smb/toggle/false", None),
        (Method::DELETE, "/api/shares/x/smb", None),
        (Method::GET, "/api/shares/smb/config", None),
        (
            Method::PUT,
            "/api/shares/smb/config",
            Some(valid_smb_config()),
        ),
        (Method::GET, "/api/shares/nfs/config", None),
        (Method::GET, "/api/shares/ftp/config", None),
        (Method::GET, "/api/shares/webdav/config", None),
        (Method::GET, "/api/shares/sftp/config", None),
        (Method::GET, "/api/shares/sftp/users", None),
        (
            Method::PUT,
            "/api/shares/sftp/users/key",
            Some(json!({"username":"x","public_key":"ssh-ed25519 AAAA x"})),
        ),
        (Method::POST, "/api/shares/sftp/rebuild", None),
        (Method::POST, "/api/shares/smb/start", None),
        (Method::POST, "/api/shares/smb/stop", None),
        (Method::POST, "/api/shares/smb/restart", None),
        (Method::GET, "/api/shares/x/smb/permissions", None),
        (
            Method::POST,
            "/api/shares/x/smb/permissions",
            Some(json!({"grantee_type":"user","grantee_name":"a","permission":"r"})),
        ),
        (
            Method::DELETE,
            "/api/shares/x/smb/permissions",
            Some(json!({"grantee_type":"user","grantee_name":"a"})),
        ),
        (Method::POST, "/api/shares/x/smb/permissions/sync", None),
        (Method::GET, "/api/users/someone/permissions", None),
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
            "{method} {uri} must not return 200 unauthenticated"
        );
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{method} {uri} 500'd"
        );
    }
}

#[tokio::test]
async fn test_endpoints_reject_a_tampered_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = shares_router(state);
    let bogus = "not-a-real-session-token";

    for uri in [
        "/api/shares",
        "/api/shares/smb/config",
        "/api/shares/sftp/users",
        "/api/shares/demo/smb/permissions",
    ] {
        let (status, body) = send(&mut r, Method::GET, uri, Some(bogus), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} with a bogus token");
        assert_eq!(body["success"], json!(false));
    }
}

// ===========================================================================
// 2. Admin gate — a valid non-admin session is not enough
// ===========================================================================

#[tokio::test]
async fn test_share_endpoints_require_admin_not_just_a_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;

    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/shares", None),
        (
            Method::POST,
            "/api/shares",
            Some(json!({"name":"x","share_type":"smb","path":"/tmp/x","comment":""})),
        ),
        (Method::GET, "/api/shares/x/smb", None),
        (
            Method::PUT,
            "/api/shares/x/smb",
            Some(json!({"enabled": false})),
        ),
        (Method::DELETE, "/api/shares/x/smb", None),
        (Method::GET, "/api/shares/smb/config", None),
        (
            Method::PUT,
            "/api/shares/nfs/config",
            Some(valid_nfs_config()),
        ),
        (Method::GET, "/api/shares/sftp/users", None),
        (Method::POST, "/api/shares/smb/start", None),
        (Method::GET, "/api/shares/x/smb/permissions", None),
        (
            Method::POST,
            "/api/shares/x/smb/permissions",
            Some(json!({"grantee_type":"user","grantee_name":"a","permission":"r"})),
        ),
    ];

    for (method, uri, body) in cases {
        let (status, resp) = send(&mut r, method.clone(), uri, Some(&token), body.clone()).await;
        // `403`, never `400`: the request was well-formed, the session is
        // valid, the account is simply not an admin. This suite used to ignore
        // the status here, which is how the module's own guard drifted to
        // `400` for a non-admin caller (ROADMAP X.7).
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} must answer 403 for a non-admin session: {resp}"
        );
        assert_eq!(
            resp["success"],
            json!(false),
            "{method} {uri} must be refused for a non-admin session"
        );
        assert!(
            resp["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("admin"),
            "{method} {uri} rejection message should mention admin access: {resp}"
        );
    }
}

#[tokio::test]
async fn test_user_permissions_is_self_or_admin_only() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Passw0rd!123", 1).await;
    create_test_user(&state, "bob", "Passw0rd!123", 1).await;
    let mut r = shares_router(state);
    let alice = login(&mut r, "alice", "Passw0rd!123").await;

    // alice may not read bob's permissions
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/users/bob/permissions",
        Some(&alice),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "another account's permissions must answer 403, not 400: {body}"
    );
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Access denied"));
}

// ===========================================================================
// 3. Read endpoints — clean success shapes (no privileged side effects)
// ===========================================================================

#[tokio::test]
async fn test_shares_list_returns_an_array_for_an_admin() {
    // No `/etc/samba/smb.conf` / `/etc/exports` / `/etc/homevault/*.json`
    // exist in this sandbox, so the list is expected to be empty — this
    // only pins the *shape* of a successful response.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/shares", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"].is_array(),
        "shares list must be an array: {body}"
    );
}

#[tokio::test]
async fn test_every_protocol_config_get_returns_defaults_when_unconfigured() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // SMB
    let (s, b) = send(
        &mut r,
        Method::GET,
        "/api/shares/smb/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["workgroup"], json!("WORKGROUP"));
    assert_eq!(b["data"]["allow_guest"], json!(false));

    // NFS
    let (s, b) = send(
        &mut r,
        Method::GET,
        "/api/shares/nfs/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["root_squash"], json!("root_squash"));

    // FTP / WebDAV — shape only
    for proto in ["ftp", "webdav"] {
        let (s, b) = send(
            &mut r,
            Method::GET,
            &format!("/api/shares/{proto}/config"),
            Some(&token),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{proto} config get");
        assert!(
            b["data"].is_object(),
            "{proto} config must be an object: {b}"
        );
    }

    // SFTP — dedicated sshd instance must default to port 2222, never 22
    let (s, b) = send(
        &mut r,
        Method::GET,
        "/api/shares/sftp/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"]["port"], json!(2222));
}

#[tokio::test]
async fn test_sftp_grantees_list_is_empty_and_well_formed_when_nothing_is_provisioned() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/sftp/users",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_permissions_list_is_empty_for_a_share_with_no_grants() {
    // `list_share_permissions` is a pure DB read keyed by (name, type) — it
    // never checks the share exists, so an admin gets a clean empty list.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/never-created/smb/permissions",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"], json!([]));
}

// ===========================================================================
// 4. Invalid protocol name — rejected before any side effect
// ===========================================================================

#[tokio::test]
async fn test_bad_protocol_name_is_rejected_on_every_type_taking_route() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::GET, "/api/shares/demo/bogusproto", None),
        (
            Method::PUT,
            "/api/shares/demo/bogusproto",
            Some(json!({"enabled": true})),
        ),
        (Method::PUT, "/api/shares/demo/bogusproto/toggle/true", None),
        (Method::DELETE, "/api/shares/demo/bogusproto", None),
        (Method::POST, "/api/shares/bogusproto/start", None),
        (Method::POST, "/api/shares/bogusproto/stop", None),
        (Method::POST, "/api/shares/bogusproto/restart", None),
        (Method::GET, "/api/shares/demo/bogusproto/permissions", None),
        (
            Method::POST,
            "/api/shares/demo/bogusproto/permissions",
            Some(json!({"grantee_type":"user","grantee_name":"a","permission":"r"})),
        ),
        (
            Method::POST,
            "/api/shares/demo/bogusproto/permissions/sync",
            None,
        ),
    ];

    for (method, uri, body) in cases {
        let (status, resp) = send(&mut r, method.clone(), uri, Some(&token), body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
        assert_eq!(resp["success"], json!(false));
        assert!(
            resp["message"]
                .as_str()
                .unwrap_or("")
                .contains("Invalid share type"),
            "{method} {uri}: {resp}"
        );
    }
}

#[tokio::test]
async fn test_create_share_rejects_an_unknown_protocol_in_the_body() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares",
        Some(&token),
        Some(json!({"name": "x", "share_type": "gopher", "path": "/tmp/x", "comment": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 5. Permission grant — input validation before touching the filesystem
// ===========================================================================

#[tokio::test]
async fn test_grant_rejects_an_invalid_permission_string() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/demo/smb/permissions",
        Some(&token),
        Some(json!({"grantee_type": "user", "grantee_name": "admin1", "permission": "rwxr"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Invalid permission"));
}

#[tokio::test]
async fn test_grant_rejects_an_invalid_grantee_type() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/demo/smb/permissions",
        Some(&token),
        Some(json!({"grantee_type": "robot", "grantee_name": "admin1", "permission": "rw"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("grantee_type"));
}

#[tokio::test]
async fn test_grant_on_a_nonexistent_share_fails_before_any_acl_write() {
    // Valid permission + valid grantee type, but the share doesn't exist —
    // `SharesService::get_share` fails, so we never reach `setfacl`.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/definitely-not-a-real-share/smb/permissions",
        Some(&token),
        Some(json!({"grantee_type": "user", "grantee_name": "admin1", "permission": "rw"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("Share not found"));
}

#[tokio::test]
async fn test_permissions_sync_on_a_nonexistent_share_fails_cleanly() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/definitely-not-a-real-share/smb/permissions/sync",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ===========================================================================
// 6. SFTP key set — the pasted key is validated before any sudo call
// ===========================================================================

#[tokio::test]
async fn test_sftp_key_set_rejects_a_malformed_key_before_provisioning() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    // The handler rejects a username that is not a HomeVault user *before*
    // looking at the key ("User '...' not found"), so the pasted key must be
    // exercised against a real account for the malformed-key rejection to be
    // the thing under test.
    create_test_user(&state, "someuser", "Passw0rd!123", 1).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    for bad_key in [
        "",
        "not-a-key",
        "ssh-ed25519",
        "ssh-ed25519 AAAAvalid\nssh-rsa AAAAinjected extra-line",
    ] {
        let (status, body) = send(
            &mut r,
            Method::PUT,
            "/api/shares/sftp/users/key",
            Some(&token),
            Some(json!({"username": "someuser", "public_key": bad_key})),
        )
        .await;
        assert_clean_rejection(status, &body);
        assert!(
            body["message"].as_str().unwrap_or("").contains("SFTP key"),
            "key {bad_key:?}: {body}"
        );
    }
}

// ===========================================================================
// 7. Mutating share operations fail cleanly without privilege (never 500)
// ===========================================================================

#[tokio::test]
async fn test_update_or_delete_a_nonexistent_smb_share_is_a_clean_not_found() {
    // The SMB path always validates the share section exists in
    // `/etc/samba/smb.conf` (or that the file itself is missing) before any
    // write — so a ghost share is a deterministic 400, never a 500 and
    // never a fake success.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/shares/ghost/smb",
            Some(json!({"comment": "x"})),
        ),
        (Method::DELETE, "/api/shares/ghost/smb", None),
    ] {
        let (status, resp) = send(&mut r, method.clone(), uri, Some(&token), body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
        assert_eq!(resp["success"], json!(false));
    }
}

#[tokio::test]
async fn test_delete_a_nonexistent_nfs_share_never_panics() {
    // The NFS delete path shells out to `sudo exportfs -u` (best-effort,
    // ignored on failure) then rewrites `/etc/exports` via `sudo tee` only
    // if that file exists. Depending on what's installed it may cleanly
    // fail (`/etc/exports` missing) or no-op — but never a panic / 500, and
    // never an unauthenticated caller getting this far (covered above).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/shares/ghost/nfs",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.is_object());
}

#[tokio::test]
async fn test_sftp_rebuild_admin_path_never_500s() {
    // `rebuild_jails` needs `sudo groupadd`/`useradd`/`mount --bind` — it
    // must degrade to a clean error here, not crash.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/sftp/rebuild",
        Some(&token),
        None,
    )
    .await;
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    // Success (a real `homevault-sftp` group already exists on this host) or
    // a clean failure are both acceptable; a panic / 500 is not.
    assert!(body.is_object());
}

// ===========================================================================
// 8. Config-file injection guards — rejected as 400 before any privileged write
// ===========================================================================
//
// Every value below would otherwise be formatted verbatim into a root-owned
// config file: `name`/`comment`/`path` into `/etc/samba/smb.conf`, `path` and
// `options` into `/etc/exports`, `welcome_message` into `/etc/vsftpd.conf`,
// and the SMB global values into `smb.conf`'s `[global]` section. The
// validators run before the protocol dispatch, so none of these requests ever
// reaches `sudo tee` — which is exactly why they are safe to run here.

#[tokio::test]
async fn test_create_share_rejects_smb_section_forging_in_the_name() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // A newline forges a whole extra Samba section; a bracket breaks out of
    // the section header; `global`/`homes` would rewrite Samba's own sections.
    for name in [
        "media\n[everything]\n  path = /",
        "media]",
        "[global]",
        "global",
        "homes",
        "me dia",
        "../etc",
    ] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/shares",
            Some(&token),
            Some(json!({
                "name": name,
                "share_type": "smb",
                "path": "/mnt/pool/media",
                "comment": ""
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "name {name:?}: {body}");
        assert_eq!(body["success"], json!(false), "name {name:?}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or("")
                .contains("share name"),
            "the rejection must name the offending field: {body}"
        );
    }
}

#[tokio::test]
async fn test_create_share_rejects_exports_column_injection_in_path_and_options() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // Path: whitespace splits /etc/exports columns, `)` closes the client
    // list, `#` comments the rest of the line out, `..` traverses.
    for path in [
        "/mnt/pool/media *(rw,no_root_squash)",
        "/mnt/pool/media)",
        "/mnt/pool/me dia",
        "/mnt/../etc",
        "relative/path",
    ] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/shares",
            Some(&token),
            Some(json!({
                "name": "media",
                "share_type": "nfs",
                "path": path,
                "comment": ""
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "path {path:?}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or("")
                .contains("share path"),
            "the rejection must name the offending field: {body}"
        );
    }

    // Options: `path *(opts)` — a `)` fabricates a second client clause.
    for options in [
        json!(["rw) /etc *(no_root_squash"]),
        json!(["rw,sync"]),
        json!(["rw sync"]),
        json!([""]),
    ] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/shares",
            Some(&token),
            Some(json!({
                "name": "media",
                "share_type": "nfs",
                "path": "/mnt/pool/media",
                "comment": "",
                "options": options
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "options {options}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or("")
                .contains("NFS option"),
            "the rejection must name the offending field: {body}"
        );
    }
}

#[tokio::test]
async fn test_create_share_rejects_a_multiline_comment() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    for comment in [
        "harmless\n  read only = no\n  browseable = no",
        "harmless\rpath = /",
    ] {
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/shares",
            Some(&token),
            Some(json!({
                "name": "media",
                "share_type": "smb",
                "path": "/mnt/pool/media",
                "comment": comment
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{comment:?}: {body}");
        assert!(
            body["message"].as_str().unwrap_or("").contains("comment"),
            "the rejection must name the offending field: {body}"
        );
    }
}

#[tokio::test]
async fn test_update_share_rejects_injected_path_and_comment() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let cases = [
        json!({"path": "/mnt/pool/media *(rw)"}),
        json!({"path": "/mnt/../etc"}),
        json!({"comment": "x\n  read only = no"}),
        json!({"options": ["rw) /etc *(no_root_squash"]}),
    ];
    for body in cases {
        let (status, resp) = send(
            &mut r,
            Method::PUT,
            "/api/shares/media/smb",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {resp}");
        assert_eq!(resp["success"], json!(false), "{body}: {resp}");
    }
}

#[tokio::test]
async fn test_smb_config_update_rejects_multiline_and_bad_values() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let mutations: &[(&str, Value)] = &[
        ("workgroup", json!("WORK\nGROUP")),
        ("workgroup", json!("GRP]")),
        ("security_mode", json!("user\n  guest ok = yes")),
        ("security_mode", json!("bogus")),
        ("protocol_version", json!("SMB3\n;")),
        ("create_mask", json!("9999")),
        ("directory_mask", json!("0o755")),
        ("additional_params", json!(["\n[evil]"])),
    ];

    for (field, value) in mutations {
        let mut cfg = valid_smb_config();
        cfg[field] = value.clone();
        let (status, body) = send(
            &mut r,
            Method::PUT,
            "/api/shares/smb/config",
            Some(&token),
            Some(cfg),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field} = {value}: {body}");
        assert_eq!(body["success"], json!(false), "{field}: {body}");
    }
}

#[tokio::test]
async fn test_ftp_config_update_rejects_banner_injection_and_bad_ports() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let base = json!({
        "port": 21,
        "allowed_users": [],
        "passive_ports": "30000-31000",
        "max_clients": 10,
        "force_tls": false,
        "allow_anonymous": false,
        "chroot": true,
        "welcome_message": "Welcome to HomeVault FTP"
    });

    let mutations: &[(&str, Value)] = &[
        (
            "welcome_message",
            json!("hi\nlocal_enable=YES\nanonymous_enable=YES"),
        ),
        ("welcome_message", json!("hi\rwrite_enable=YES")),
        ("passive_ports", json!("abc")),
        ("passive_ports", json!("31000-30000")),
        ("passive_ports", json!("30000-")),
        ("port", json!(0)),
        ("max_clients", json!(0)),
    ];

    for (field, value) in mutations {
        let mut cfg = base.clone();
        cfg[field] = value.clone();
        let (status, body) = send(
            &mut r,
            Method::PUT,
            "/api/shares/ftp/config",
            Some(&token),
            Some(cfg),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field} = {value}: {body}");
        assert_eq!(body["success"], json!(false), "{field}: {body}");
    }
}

// ===========================================================================
// 9. WebDAV — in-process service lifecycle
// ===========================================================================

#[tokio::test]
async fn test_webdav_service_start_is_admin_only_and_never_binds_without_shares() {
    // The WebDAV server is now in-process. With no WebDAV share defined it has
    // nothing to serve, so `start` reports success without binding a port —
    // this keeps the endpoint testable without opening a real socket.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);

    // No token -> refused.
    let (status, body) = send(&mut r, Method::POST, "/api/shares/webdav/start", None, None).await;
    assert_ne!(status, StatusCode::OK, "{body}");

    // A member session is refused with an admin message.
    let regular = login(&mut r, "regular1", "Passw0rd!123").await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/webdav/start",
        Some(&regular),
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .to_lowercase()
        .contains("admin"));

    // An admin gets a clean success (port 0 = nothing bound, no shares).
    let admin = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/shares/webdav/start",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true), "{body}");

    // Stopping is idempotent and also admin-only.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/shares/webdav/stop",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_webdav_config_default_is_closed_and_bindable() {
    // The old derived `Default` gave port 0 and auth OFF — an unconfigured
    // appliance could neither bind nor be safe. Both are pinned here.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = shares_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/shares/webdav/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["require_auth"], json!(true), "{body}");
    assert_ne!(body["data"]["port"], json!(0), "{body}");
}
