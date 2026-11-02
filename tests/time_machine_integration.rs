/* =============================================================================
 * File          : time_machine_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Time Machine over SMB integration tests: the admin matrix, the full
 *   round-trip against a real (temporary) smb.conf — the managed block is
 *   written, read back by the list endpoint and removed on disable — plus the
 *   quota validation paths and the recycle/Time-Machine coexistence.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot, tempfile)
 *
 * Usage:
 *   cargo test --test time_machine_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for Time Machine over SMB (`crate::shares`,
//! `crate::handlers::shares`):
//!
//!   GET /api/shares/smb/time-machine
//!   PUT /api/shares/{name}/smb/time-machine
//!
//! `HV_SMB_CONF` points the whole read/write path at a temp `smb.conf`, so the
//! real generated section is asserted on disk. The test host's own Samba
//! configuration is never read or written, and `smbd` is never bounced (the
//! override tells `reload_smbd` to stay out of the host).

use axum::{
    routing::{get, post, put},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::shares::{
    handle_smb_recycle_list, handle_smb_recycle_set, handle_smb_time_machine_list,
    handle_smb_time_machine_set,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const SMB_CONF: &str = "# managed by the time-machine integration test
[global]
   workgroup = WORKGROUP

[media]
  comment = Family media
  path = /mnt/pool/media
  browseable = yes
  read only = no
  create mask = 0755

[tm]
  comment = Mac backups
  path = /mnt/pool/tm
  browseable = yes
  read only = no
  create mask = 0755
";

struct Harness {
    state: Arc<AppState>,
    smb_conf: std::path::PathBuf,
    _dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);
    let smb_conf = root.join("smb.conf");
    std::fs::write(&smb_conf, SMB_CONF).expect("write smb.conf");
    std::env::set_var("HV_SMB_CONF", &smb_conf);

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
        smb_conf,
        _dir: dir,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/shares/smb/time-machine",
            get(handle_smb_time_machine_list),
        )
        .route(
            "/api/shares/{name}/smb/time-machine",
            put(handle_smb_time_machine_set),
        )
        .route("/api/shares/smb/recycle", get(handle_smb_recycle_list))
        .route(
            "/api/shares/{name}/smb/recycle",
            put(handle_smb_recycle_set),
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
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

fn read_conf(h: &Harness) -> String {
    std::fs::read_to_string(&h.smb_conf).expect("read smb.conf")
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_time_machine_endpoints_are_admin_only() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    for (method, uri, body) in [
        (Method::GET, "/api/shares/smb/time-machine", None),
        (
            Method::PUT,
            "/api/shares/media/smb/time-machine",
            Some(json!({"enabled": true, "quota_gb": 100})),
        ),
    ] {
        let (status, _) = send(&mut router, method.clone(), uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }

    // A refused request must not have touched the config.
    assert_eq!(read_conf(&h), SMB_CONF);
}

// ---------------------------------------------------------------------------
// Round-trip against the generated smb.conf
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_enabling_time_machine_writes_the_fruit_profile() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/media/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": true, "quota_gb": 500})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], json!(true));
    assert_eq!(body["data"]["quota_gb"], json!(500));

    let conf = read_conf(&h);
    assert!(conf.contains("[media]"), "{conf}");
    assert!(conf.contains("vfs objects = fruit streams_xattr"), "{conf}");
    assert!(conf.contains("fruit:time machine = yes"), "{conf}");
    assert!(
        conf.contains("fruit:time machine max size = 500G"),
        "{conf}"
    );
    // Only this share was touched.
    assert!(!conf.contains("[tm]\n  vfs objects"), "{conf}");

    // …and the list endpoint reads it back from the file, not from memory.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/shares/smb/time-machine",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["media"]["enabled"], json!(true));
    assert_eq!(body["data"]["media"]["quota_gb"], json!(500));
    assert_eq!(body["data"]["tm"]["enabled"], json!(false));
}

#[tokio::test]
async fn test_disabling_time_machine_removes_the_profile_but_keeps_the_share() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let _ = send(
        &mut router,
        Method::PUT,
        "/api/shares/tm/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": true, "quota_gb": 0})),
    )
    .await;
    assert!(read_conf(&h).contains("fruit:time machine = yes"));

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/tm/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": false, "quota_gb": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let conf = read_conf(&h);
    assert!(!conf.contains("fruit:time machine"), "{conf}");
    assert!(!conf.contains("homevault-smb-vfs:begin"), "{conf}");
    // The share itself is untouched.
    assert!(conf.contains("[tm]"), "{conf}");
    assert!(conf.contains("  path = /mnt/pool/tm"), "{conf}");
}

#[tokio::test]
async fn test_recycle_and_time_machine_coexist_on_one_vfs_line() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/media/smb/recycle",
        Some(&admin),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/media/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": true, "quota_gb": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let conf = read_conf(&h);
    assert_eq!(
        conf.lines()
            .filter(|l| l.trim().starts_with("vfs objects ="))
            .count(),
        1,
        "Samba accepts one `vfs objects` line per share: {conf}"
    );
    assert!(
        conf.contains("vfs objects = fruit streams_xattr recycle"),
        "{conf}"
    );
    assert!(conf.contains("recycle:repository ="), "{conf}");

    // Turning Time Machine back off must keep the recycle module.
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/shares/media/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": false, "quota_gb": 0})),
    )
    .await;
    let conf = read_conf(&h);
    assert!(conf.contains("vfs objects = recycle"), "{conf}");
    assert!(!conf.contains("fruit:time machine"), "{conf}");
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_an_out_of_range_quota_is_refused_without_rewriting_the_config() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/media/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": true, "quota_gb": 2_000_000u32})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("quota"),
        "{body}"
    );
    assert_eq!(read_conf(&h), SMB_CONF);
}

#[tokio::test]
async fn test_an_unknown_share_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/nope/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": true, "quota_gb": 10})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(read_conf(&h), SMB_CONF);
}

#[tokio::test]
async fn test_a_path_fragment_share_name_never_reaches_the_config() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/..%2Fetc/smb/time-machine",
        Some(&admin),
        Some(json!({"enabled": true, "quota_gb": 0})),
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "{status} {body}"
    );
    assert_eq!(read_conf(&h), SMB_CONF);
}

#[tokio::test]
async fn test_the_list_endpoint_returns_an_empty_object_without_a_config() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    std::fs::remove_file(&h.smb_conf).expect("remove smb.conf");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/shares/smb/time-machine",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"], json!({}));
}
