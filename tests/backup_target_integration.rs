/* =============================================================================
 * File          : backup_target_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup-destination integration tests: the auth matrix, the tenant
 *   lifecycle where the token and repository password are returned exactly
 *   once, quota updates that revoke/restore access, the generated htpasswd
 *   file, and a real end-to-end verification — a restic repository created,
 *   backed up and integrity-checked through the endpoint.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot; the host's restic for the
 *     verification test, skipped when it is absent)
 *
 * Usage:
 *   cargo test --test backup_target_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the backup destination (`crate::backup_target`,
//! `crate::handlers::backup_target`):
//!
//!   GET    /api/backup-target
//!   PUT    /api/backup-target/config
//!   POST   /api/backup-target/tenants
//!   PUT    /api/backup-target/tenants/{id}
//!   POST   /api/backup-target/tenants/{id}/rotate
//!   DELETE /api/backup-target/tenants/{id}
//!   POST   /api/backup-target/tenants/{id}/verify
//!
//! `HV_BACKUP_TARGET_DIR` and `DATA_DIRECTORY` point every generated file and
//! repository at a temp directory, so the host's `/etc` and real backup root
//! are never touched.

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::backup_target::{
    handle_backup_target_settings_update, handle_backup_target_state,
    handle_backup_tenant_create, handle_backup_tenant_delete, handle_backup_tenant_rotate,
    handle_backup_tenant_update, handle_backup_tenant_verify,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    target_dir: std::path::PathBuf,
    repos_root: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);
    let target_dir = root.join("backup-target-config");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::env::set_var("HV_BACKUP_TARGET_DIR", &target_dir);
    let repos_root = root.join("backup-target");
    std::fs::create_dir_all(&repos_root).unwrap();

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
        target_dir,
        repos_root,
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/backup-target", get(handle_backup_target_state))
        .route(
            "/api/backup-target/config",
            axum::routing::put(handle_backup_target_settings_update),
        )
        .route(
            "/api/backup-target/tenants",
            post(handle_backup_tenant_create),
        )
        .route(
            "/api/backup-target/tenants/{id}",
            axum::routing::put(handle_backup_tenant_update)
                .delete(handle_backup_tenant_delete),
        )
        .route(
            "/api/backup-target/tenants/{id}/rotate",
            post(handle_backup_tenant_rotate),
        )
        .route(
            "/api/backup-target/tenants/{id}/verify",
            post(handle_backup_tenant_verify),
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

fn auth_path(h: &Harness) -> std::path::PathBuf {
    h.target_dir.join("htpasswd")
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reads_need_a_session_and_mutations_need_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(&mut router, Method::GET, "/api/backup-target", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/backup-target",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/backup-target/config".to_string(),
            Some(json!({"enabled": true})),
        ),
        (
            Method::POST,
            "/api/backup-target/tenants".to_string(),
            Some(json!({"name": "family"})),
        ),
        (
            Method::PUT,
            "/api/backup-target/tenants/1".to_string(),
            Some(json!({"quota_bytes": 10})),
        ),
        (
            Method::POST,
            "/api/backup-target/tenants/1/rotate".to_string(),
            None,
        ),
        (Method::DELETE, "/api/backup-target/tenants/1".to_string(), None),
        (
            Method::POST,
            "/api/backup-target/tenants/1/verify".to_string(),
            None,
        ),
    ] {
        let (status, _) = send(&mut router, method.clone(), &uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, &uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_settings_are_off_by_default_and_validated_before_anything_is_written() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/backup-target",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["enabled"], json!(false));
    assert_eq!(body["data"]["tenants"], json!([]));
    assert!(!h.target_dir.join("settings.json").exists());

    for body in [
        json!({"enabled": true, "port": 0}),
        json!({"enabled": true, "listen": "nas.local"}),
        json!({"enabled": true, "listen": "0.0.0.0; id"}),
    ] {
        let (status, response) = send(
            &mut router,
            Method::PUT,
            "/api/backup-target/config",
            Some(&admin),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {response}");
    }
    assert!(!h.target_dir.join("settings.json").exists());

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/backup-target/config",
        Some(&admin),
        Some(json!({"enabled": true, "port": 8123, "listen": "127.0.0.1"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["port"], json!(8123));
    let written = std::fs::read_to_string(h.target_dir.join("settings.json")).unwrap();
    assert!(written.contains("8123"), "{written}");
}

// ---------------------------------------------------------------------------
// Tenant lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_creating_a_tenant_shows_its_secrets_once_and_never_again() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "family", "quota_bytes": 1024})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["data"]["token"].as_str().expect("a token").to_string();
    let password = body["data"]["repository_password"]
        .as_str()
        .expect("a repository password")
        .to_string();
    assert_eq!(token.len(), 64);
    assert_eq!(password.len(), 64);
    assert!(
        body["data"]["repository_url"]
            .as_str()
            .unwrap_or_default()
            .contains("/family/"),
        "{body}"
    );
    assert_eq!(body["data"]["tenant"]["has_token"], json!(true));
    assert_eq!(body["data"]["tenant"]["quota_bytes"], json!(1024));

    // The htpasswd file rest-server reads carries the token's `{SHA}`, never
    // the token itself.
    let htpasswd = std::fs::read_to_string(auth_path(&h)).unwrap();
    assert!(htpasswd.starts_with("family:{SHA}"), "{htpasswd}");
    assert!(!htpasswd.contains(&token), "the token must not be on disk in clear text");

    // The list never repeats either secret.
    let (status, list) = send(
        &mut router,
        Method::GET,
        "/api/backup-target",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let serialized = list.to_string();
    assert!(!serialized.contains(&token), "{serialized}");
    assert!(!serialized.contains(&password), "{serialized}");
    assert_eq!(list["data"]["tenants"][0]["name"], json!("family"));

    // A duplicate name (case-insensitively) is refused.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "family"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("already exists"),
        "{body}"
    );
}

#[tokio::test]
async fn test_an_invalid_tenant_name_is_refused_before_anything_is_created() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    for name in ["../etc", "a/b", "-nas", "FaMily", ""] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/backup-target/tenants",
            Some(&admin),
            Some(json!({"name": name})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name} → {body}");
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_tenants")
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(!auth_path(&h).exists());
}

#[tokio::test]
async fn test_deleting_a_tenant_removes_its_htpasswd_entry_and_keeps_data_by_default() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "family"})),
    )
    .await;
    let id = body["data"]["tenant"]["id"].as_i64().expect("id");
    let repo_path = body["data"]["tenant"]["repo_path"]
        .as_str()
        .expect("repo path")
        .to_string();
    assert!(std::path::Path::new(&repo_path).exists(), "{repo_path}");
    assert!(std::fs::read_to_string(auth_path(&h)).unwrap().contains("family"));

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/backup-target/tenants/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The received data is kept unless the admin explicitly asked to delete it.
    assert!(std::path::Path::new(&repo_path).exists(), "data must be kept");
    let htpasswd = std::fs::read_to_string(auth_path(&h)).unwrap_or_default();
    assert!(!htpasswd.contains("family"), "access must be revoked: {htpasswd}");
    assert_eq!(body["data"]["tenants"], json!([]));
}

#[tokio::test]
async fn test_rotation_returns_new_secrets_and_invalidates_the_old_token() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (_, created) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "family"})),
    )
    .await;
    let id = created["data"]["tenant"]["id"].as_i64().expect("id");
    let old_token = created["data"]["token"].as_str().unwrap().to_string();

    let (status, rotated) = send(
        &mut router,
        Method::POST,
        &format!("/api/backup-target/tenants/{id}/rotate"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let new_token = rotated["data"]["token"].as_str().expect("a token");
    assert_ne!(new_token, old_token);
    assert!(
        rotated["data"]["warning"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
            .contains("re-initialise"),
        "the password change must be explained: {rotated}"
    );
    // The old token no longer matches the generated entry.
    let htpasswd = std::fs::read_to_string(auth_path(&h)).unwrap();
    assert!(!htpasswd.contains(&old_token));
    assert!(htpasswd.starts_with("family:{SHA}"));
}

// ---------------------------------------------------------------------------
// Quota enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_an_over_quota_tenant_is_revoked_from_the_htpasswd_file() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (_, created) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "family", "quota_bytes": 10})),
    )
    .await;
    let id = created["data"]["tenant"]["id"].as_i64().expect("id");
    let repo_path = created["data"]["tenant"]["repo_path"]
        .as_str()
        .expect("repo path")
        .to_string();
    // Exceed the quota with real bytes in the repository directory.
    std::fs::create_dir_all(&repo_path).unwrap();
    std::fs::write(std::path::Path::new(&repo_path).join("data"), vec![0u8; 64]).unwrap();

    let state = h.state.clone();
    homevault::backup_target_scheduler::enforce_quotas(&state)
        .await
        .expect("quota pass");

    let over: i64 = sqlx::query_scalar("SELECT over_quota FROM backup_tenants WHERE id = ?")
        .bind(id)
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(over, 1);
    let htpasswd = std::fs::read_to_string(auth_path(&h)).unwrap_or_default();
    assert!(
        !htpasswd.contains("family"),
        "a revoked tenant must disappear from the auth file: {htpasswd}"
    );

    // Raising the quota restores access without re-issuing a token.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/backup-target/tenants/{id}"),
        Some(&admin),
        Some(json!({"quota_bytes": 1024})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let htpasswd = std::fs::read_to_string(auth_path(&h)).unwrap_or_default();
    assert!(htpasswd.contains("family"), "{htpasswd}");
}

// ---------------------------------------------------------------------------
// Real verification (a restic repository on disk)
// ---------------------------------------------------------------------------

fn restic_available() -> bool {
    homevault::restic::ResticService::is_available()
}

fn run_restic(repository: &str, password: &str, args: &[&str]) -> bool {
    let mut cmd = Command::new("restic");
    cmd.args(["-r", repository]).args(args);
    cmd.env("RESTIC_PASSWORD", password);
    cmd.env("RESTIC_CACHE_DIR", std::env::temp_dir().join("hv-restic-test-cache"));
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

#[tokio::test]
async fn test_a_real_repository_is_verified_and_a_missing_one_fails_cleanly() {
    if !restic_available() {
        eprintln!("restic is not installed — skipping the real verification test");
        return;
    }
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (_, created) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "family"})),
    )
    .await;
    let id = created["data"]["tenant"]["id"].as_i64().expect("id");
    let password = created["data"]["repository_password"]
        .as_str()
        .expect("password")
        .to_string();
    let repo_path = created["data"]["tenant"]["repo_path"]
        .as_str()
        .expect("repo path")
        .to_string();

    // A repository the "source" would have pushed, using the password the
    // destination handed out.
    assert!(
        run_restic(&repo_path, &password, &["init"]),
        "restic init must succeed"
    );
    let source = h.repos_root.join("source-data");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("hello.txt"), b"received backup").unwrap();
    assert!(
        run_restic(
            &repo_path,
            &password,
            &["backup", "--host", "nas1", &source.to_string_lossy()]
        ),
        "restic backup must succeed"
    );

    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/backup-target/tenants/{id}/verify"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["verified"], json!(true));
    assert!(
        body["data"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("repository readable"),
        "{body}"
    );

    let recorded: Option<String> =
        sqlx::query_scalar("SELECT last_verify_status FROM backup_tenants WHERE id = ?")
            .bind(id)
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(recorded.as_deref(), Some("success"));

    // A tenant whose repository was never initialised fails with a message,
    // not a 500.
    let (_, other) = send(
        &mut router,
        Method::POST,
        "/api/backup-target/tenants",
        Some(&admin),
        Some(json!({"name": "laptop"})),
    )
    .await;
    let other_id = other["data"]["tenant"]["id"].as_i64().expect("id");
    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/backup-target/tenants/{other_id}/verify"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap_or_default().contains("failed"),
        "{body}"
    );
}
