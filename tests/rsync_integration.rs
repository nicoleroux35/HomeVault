/* =============================================================================
 * File          : rsync_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Rsync server integration tests: the admin matrix and the full module
 *   round-trip against a temp state directory, plus a real `rsync --daemon`
 *   serving a generated module over loopback — an authenticated listing and a
 *   real file transfer through the same generated module section.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot, tempfile, the host's rsync)
 *
 * Usage:
 *   cargo test --test rsync_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the Rsync server (`crate::rsync_server`,
//! `crate::handlers::rsync`):
//!
//!   GET    /api/shares/rsync
//!   PUT    /api/shares/rsync/config
//!   POST   /api/shares/rsync/modules
//!   PUT    /api/shares/rsync/modules/{name}
//!   DELETE /api/shares/rsync/modules/{name}
//!   PUT    /api/shares/rsync/users/{user}/secret
//!
//! `HV_RSYNC_DIR` and `HV_SMB_CONF` point the generator at temp paths, so the
//! real `rsyncd.conf` / secrets file are asserted on disk without touching the
//! host. The transfer test starts its own daemon on loopback as the current
//! user and uses the same generated `[module]` section — only the root-only
//! `uid`/`gid`/`pid file`/`log file` lines are dropped, because an unprivileged
//! daemon cannot use them.

use axum::{
    routing::{get, post, put},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::rsync::{
    handle_rsync_module_create, handle_rsync_module_delete, handle_rsync_module_update,
    handle_rsync_secret_set, handle_rsync_settings_update, handle_rsync_state,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    root: std::path::PathBuf,
    share: std::path::PathBuf,
    rsync_dir: std::path::PathBuf,
    _dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
    std::env::set_var("DATA_DIRECTORY", &root);

    // A share this host's Samba config reports: the module path validator only
    // accepts an existing configured share.
    let share = root.join("share");
    std::fs::create_dir_all(&share).unwrap();
    std::fs::write(share.join("hello.txt"), b"from the rsync integration test").unwrap();
    let smb_conf = root.join("smb.conf");
    std::fs::write(
        &smb_conf,
        format!(
            "[global]\n   workgroup = WORKGROUP\n\n[media]\n  comment = Media\n  path = {}\n  browseable = yes\n",
            share.display()
        ),
    )
    .unwrap();
    std::env::set_var("HV_SMB_CONF", &smb_conf);

    let rsync_dir = root.join("rsync");
    std::fs::create_dir_all(&rsync_dir).unwrap();
    std::env::set_var("HV_RSYNC_DIR", &rsync_dir);

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
        share,
        rsync_dir,
        _dir: dir,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/shares/rsync", get(handle_rsync_state))
        .route("/api/shares/rsync/config", put(handle_rsync_settings_update))
        .route("/api/shares/rsync/modules", post(handle_rsync_module_create))
        .route(
            "/api/shares/rsync/modules/{name}",
            put(handle_rsync_module_update).delete(handle_rsync_module_delete),
        )
        .route(
            "/api/shares/rsync/users/{user}/secret",
            put(handle_rsync_secret_set),
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

async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

fn read_file(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Everything the API needs to create a working module: a password first, then
/// the module itself.
async fn create_working_module(router: &mut Router, admin: &str, h: &Harness) -> Value {
    let (status, body) = send(
        router,
        Method::PUT,
        "/api/shares/rsync/users/member/secret",
        Some(admin),
        Some(json!({"password": "rsync-secret"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        router,
        Method::POST,
        "/api/shares/rsync/modules",
        Some(admin),
        Some(json!({
            "name": "media",
            "path": h.share.to_string_lossy(),
            "comment": "Family media",
            "read_only": false,
            "users": ["member"],
            "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_every_rsync_endpoint_is_admin_only() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    for (method, uri, body) in [
        (Method::GET, "/api/shares/rsync", None),
        (
            Method::PUT,
            "/api/shares/rsync/config",
            Some(json!({"enabled": true})),
        ),
        (
            Method::POST,
            "/api/shares/rsync/modules",
            Some(json!({"name": "media", "path": "/tmp", "users": []})),
        ),
        (
            Method::PUT,
            "/api/shares/rsync/modules/media",
            Some(json!({"enabled": false})),
        ),
        (Method::DELETE, "/api/shares/rsync/modules/media", None),
        (
            Method::PUT,
            "/api/shares/rsync/users/member/secret",
            Some(json!({"password": "x"})),
        ),
    ] {
        let (status, _) = send(&mut router, method.clone(), uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }

    // Nothing was generated by a refused call.
    assert!(!h.rsync_dir.join("rsyncd.conf").exists());
}

// ---------------------------------------------------------------------------
// Module lifecycle against the generated files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_module_writes_an_authenticated_conf_and_a_0600_secrets_file() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let body = create_working_module(&mut router, &admin, &h).await;

    // The API response never carries the password.
    let serialized = body.to_string();
    assert!(!serialized.contains("rsync-secret"), "{serialized}");
    assert_eq!(body["data"]["users_with_secret"], json!(["member"]));

    let conf = read_file(&h.rsync_dir.join("rsyncd.conf"));
    assert!(conf.contains("[media]"), "{conf}");
    assert!(conf.contains(&format!("  path = {}", h.share.display())), "{conf}");
    assert!(conf.contains("  auth users = member"), "{conf}");
    assert!(conf.contains("  read only = no"), "{conf}");

    let secrets_path = h.rsync_dir.join("rsyncd.secrets");
    let secrets = read_file(&secrets_path);
    assert_eq!(secrets, "member:rsync-secret\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&secrets_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "secrets file mode was {mode:o}");
    }

    // …and the list endpoint reads the state back without leaking it.
    let (status, list) = send(&mut router, Method::GET, "/api/shares/rsync", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["data"]["modules"][0]["name"], json!("media"));
    assert!(!list.to_string().contains("rsync-secret"));
}

#[tokio::test]
async fn test_enabling_a_module_without_a_user_or_password_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // No users at all.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/shares/rsync/modules",
        Some(&admin),
        Some(json!({
            "name": "media", "path": h.share.to_string_lossy(), "users": [], "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!h.rsync_dir.join("rsyncd.conf").exists());

    // A user with no rsync password yet.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/shares/rsync/modules",
        Some(&admin),
        Some(json!({
            "name": "media", "path": h.share.to_string_lossy(), "users": ["member"], "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("password"),
        "{body}"
    );
}

#[tokio::test]
async fn test_a_module_cannot_serve_a_directory_outside_the_shares() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/shares/rsync/users/member/secret",
        Some(&admin),
        Some(json!({"password": "rsync-secret"})),
    )
    .await;

    let outside = h.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/shares/rsync/modules",
        Some(&admin),
        Some(json!({
            "name": "escape", "path": outside.to_string_lossy(), "users": ["member"], "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not one of HomeVault's shares"),
        "{body}"
    );
}

#[tokio::test]
async fn test_disabling_and_deleting_a_module_rewrites_the_config_each_time() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    create_working_module(&mut router, &admin, &h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/rsync/modules/media",
        Some(&admin),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let conf = read_file(&h.rsync_dir.join("rsyncd.conf"));
    assert!(!conf.contains("[media]"), "a disabled module is removed: {conf}");

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/rsync/modules/media",
        Some(&admin),
        Some(json!({"enabled": true, "read_only": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let conf = read_file(&h.rsync_dir.join("rsyncd.conf"));
    assert!(conf.contains("[media]"), "{conf}");
    assert!(conf.contains("  read only = yes"), "{conf}");

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/shares/rsync/modules/media",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let conf = read_file(&h.rsync_dir.join("rsyncd.conf"));
    assert!(!conf.contains("[media]"), "{conf}");
}

#[tokio::test]
async fn test_settings_are_validated_before_anything_is_written() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/rsync/config",
        Some(&admin),
        Some(json!({"enabled": true, "port": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!h.rsync_dir.join("rsyncd.conf").exists());

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/rsync/config",
        Some(&admin),
        Some(json!({"enabled": true, "port": 8873, "max_connections": 600})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!h.rsync_dir.join("rsyncd.conf").exists());
}

// ---------------------------------------------------------------------------
// A real rsync daemon and a real client
// ---------------------------------------------------------------------------

/// A free loopback port (bind, read, drop — a heartbeat race is acceptable in a
/// test that retries the connection).
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().unwrap().port()
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("rsync daemon did not start listening on {port}");
}

/// The generated config, made runnable by an unprivileged test daemon: the
/// root-only lines (`pid file`, `log file`, per-module `uid`/`gid`) are dropped
/// and the port is the test's. Everything else — the module section, `auth
/// users`, `secrets file`, `use chroot = no` — is exactly what HomeVault
/// generated.
fn runnable_config(generated: &str, port: u16) -> String {
    generated
        .lines()
        .filter(|line| {
            let t = line.trim();
            !t.starts_with("pid file =")
                && !t.starts_with("log file =")
                && !t.starts_with("lock file =")
                && !t.starts_with("uid =")
                && !t.starts_with("gid =")
                && !t.starts_with("port =")
        })
        .map(str::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + &format!(
            "\nport = {port}\nlock file = {}/rsyncd.lock\n",
            std::env::temp_dir().display()
        )
}

#[tokio::test]
async fn test_a_real_rsync_client_lists_and_copies_through_a_generated_module() {
    if !homevault::dependencies::binary_available("rsync", "/usr/bin/rsync") {
        eprintln!("rsync is not installed — skipping the real-daemon test");
        return;
    }
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    create_working_module(&mut router, &admin, &h).await;
    // An unprivileged daemon cannot chroot.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/shares/rsync/config",
        Some(&admin),
        Some(json!({"enabled": false, "chroot": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let generated = read_file(&h.rsync_dir.join("rsyncd.conf"));
    let port = free_port();
    let config_path = h.root.join("test-rsyncd.conf");
    std::fs::write(&config_path, runnable_config(&generated, port)).unwrap();

    let mut daemon = Command::new("rsync")
        .arg("--daemon")
        .arg("--no-detach")
        .arg(format!("--config={}", config_path.display()))
        .arg(format!("--port={port}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rsync daemon");
    wait_for_port(port);

    // A password file the client owns (0600, as rsync requires).
    let password_file = h.root.join("rsync-password");
    std::fs::write(&password_file, "rsync-secret\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    let listing = Command::new("rsync")
        .args([
            "--list-only",
            "--port",
            &port.to_string(),
            &format!("rsync://member@127.0.0.1:{port}/media/"),
            &format!("--password-file={}", password_file.display()),
        ])
        .output()
        .expect("run rsync --list-only");
    let stdout = String::from_utf8_lossy(&listing.stdout).to_string();
    let stderr = String::from_utf8_lossy(&listing.stderr).to_string();
    assert!(
        listing.status.success(),
        "listing failed: {stderr}\n{stdout}"
    );
    assert!(stdout.contains("hello.txt"), "{stdout}");

    // A real transfer of the file the module serves.
    let dest = h.root.join("pulled");
    std::fs::create_dir_all(&dest).unwrap();
    let copy = Command::new("rsync")
        .args([
            "--port",
            &port.to_string(),
            &format!("rsync://member@127.0.0.1:{port}/media/hello.txt"),
            &dest.join("hello.txt").display().to_string(),
            &format!("--password-file={}", password_file.display()),
        ])
        .output()
        .expect("run rsync copy");
    assert!(
        copy.status.success(),
        "copy failed: {}",
        String::from_utf8_lossy(&copy.stderr)
    );
    assert_eq!(
        read_file(&dest.join("hello.txt")),
        "from the rsync integration test"
    );

    // A wrong password is refused, not silently accepted.
    let wrong = h.root.join("wrong-password");
    std::fs::write(&wrong, "not-the-password\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrong, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let refused = Command::new("rsync")
        .args([
            "--list-only",
            "--port",
            &port.to_string(),
            &format!("rsync://member@127.0.0.1:{port}/media/"),
            &format!("--password-file={}", wrong.display()),
        ])
        .output()
        .expect("run rsync with a wrong password");
    assert!(!refused.status.success(), "a wrong password must be refused");

    let _ = daemon.kill();
    let _ = daemon.wait();
}
