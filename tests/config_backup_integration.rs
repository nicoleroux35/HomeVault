/* =============================================================================
 * File          : config_backup_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Full configuration backup integration tests: in-memory SQLite + assembled
 *   Axum router on an isolated `DATA_DIRECTORY`. Covers the admin matrix, a
 *   real export → inspect → restore round-trip through the endpoints, the
 *   passphrase paths, the newer-format refusal, and the acceptance criterion
 *   that an archive made **without** secrets contains no working credential.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test config_backup_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the full configuration backup (`crate::config_backup`,
//! `handlers::config_backup`):
//!
//!   GET  /api/settings/config-backup
//!   POST /api/settings/config-backup/export
//!   GET  /api/settings/config-backup/archives/{id}
//!   POST /api/settings/config-backup/inspect
//!   POST /api/settings/config-backup/restore
//!
//! The restore is **staged**, never applied to the running server: the tests
//! assert the staging tree and the `requires_restart` answer, and
//! `apply_pending_restore` is exercised directly on that tree.
//!
//! Run:
//!   cargo test --test config_backup_integration

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::config_backup::{
    handle_config_backup_download, handle_config_backup_export, handle_config_backup_inspect,
    handle_config_backup_restore, handle_config_backup_state,
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
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);

    // A *file* database, not `sqlite::memory:`: this feature snapshots the
    // database with `VACUUM INTO`, and `Database::vacuum_into` documents that an
    // in-memory database writes no file at all. On a memory DB the export fails
    // with "Cannot finalise snapshot: No such file or directory".
    // The export handler reads the process-global settings singleton, so point
    // it at this test's throwaway directory (the same wiring
    // `settings_core_integration` uses) before anything else touches settings.
    homevault::settings_persistence::init_persistence(root.to_string_lossy().to_string());
    let _ = homevault::settings_persistence::reset_settings();

    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir).expect("data dir");
    let db_file = data_dir.join("homevault.db");
    let db = Database::new(&format!("sqlite://{}?mode=rwc", db_file.display()))
        .await
        .expect("db");
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

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/settings/config-backup", get(handle_config_backup_state))
        .route(
            "/api/settings/config-backup/export",
            post(handle_config_backup_export),
        )
        .route(
            "/api/settings/config-backup/archives/{*id}",
            get(handle_config_backup_download),
        )
        .route(
            "/api/settings/config-backup/inspect",
            post(handle_config_backup_inspect),
        )
        .route(
            "/api/settings/config-backup/restore",
            post(handle_config_backup_restore),
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
    extra: Option<(&str, &str)>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    if let Some((name, value)) = extra {
        req = req.header(name, value);
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

/// POST with a raw binary body (the archive).
async fn send_bytes(
    router: &mut Router,
    uri: &str,
    token: &str,
    passphrase: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("x-session-token", token)
        .header("content-type", "application/octet-stream");
    if let Some(passphrase) = passphrase {
        req = req.header("x-backup-passphrase", passphrase);
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
        .unwrap()
        .to_vec();
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = build_router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

/// Export an archive through the API and return its bytes from the download
/// endpoint, so the test covers the whole path the UI uses.
async fn export_and_fetch(
    router: &mut Router,
    admin: &str,
    include_secrets: bool,
    passphrase: Option<&str>,
) -> Vec<u8> {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/settings/config-backup/export",
        Some(admin),
        None,
        Some(json!({ "include_secrets": include_secrets, "passphrase": passphrase })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "export: {body}");
    let id = body["data"]["id"].as_str().expect("an archive id").to_string();
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/settings/config-backup/archives/{id}"))
        .header("x-session-token", admin)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_config_backup_is_admin_only() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    // No session.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/config-backup",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/settings/config-backup/export",
        None,
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send_bytes(
        &mut router,
        "/api/settings/config-backup/restore",
        "",
        None,
        vec![0u8; 32],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A regular member: the archive holds every account and every share, and a
    // restore replaces them — admin only, on both verbs.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/config-backup",
        Some(&member),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/settings/config-backup/export",
        Some(&member),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send_bytes(
        &mut router,
        "/api/settings/config-backup/restore",
        &member,
        None,
        vec![0u8; 32],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The admin reads the state, which is what the card renders.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/config-backup",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["format_version"].as_u64().unwrap_or(0) >= 1);
    assert!(body["data"]["settings_schema"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(body["data"]["pending_restore"], Value::Null);
    assert!(body["data"]["archives"].is_array());
    assert!(!body["data"]["config_files"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Export → inspect → restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_backup_round_trips_through_the_endpoints() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // A real configuration to carry: the admin account exists, so the archive's
    // database snapshot holds it.
    assert!(h.root.join("data").is_dir(), "the isolated data directory exists");
    let archive = export_and_fetch(&mut router, &admin, false, None).await;
    assert!(homevault::config_backup::looks_like_archive(&archive));
    assert!(archive.len() > 1024, "the snapshot makes a real archive");

    // The export is listed, so the card can offer it again.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/config-backup",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["archives"].as_array().unwrap().len(), 1);

    // Inspect: a dry run that names the manifest without staging anything.
    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/inspect",
        &admin,
        None,
        archive.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "inspect: {body}");
    let manifest = &body["data"]["manifest"];
    assert_eq!(manifest["format_version"], 1);
    assert_eq!(manifest["includes_secrets"], false);
    assert!(manifest["entries"].as_array().unwrap().len() >= 2);
    // Nothing is staged by an inspect.
    assert!(!homevault::config_backup::staging_dir().join("READY").exists());

    // Restore: validated and staged, and the answer says a restart is required
    // instead of claiming the running server changed.
    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/restore",
        &admin,
        None,
        archive,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "restore: {body}");
    assert_eq!(body["data"]["staged"], true);
    assert_eq!(body["data"]["requires_restart"], true);
    let staged_entries = body["data"]["staged_entries"].as_array().unwrap();
    assert!(staged_entries.iter().any(|e| e == "database.db"));
    // The staging tree exists and is marked ready, and the marker is written
    // last (so an interrupted staging is never applied).
    let staging = homevault::config_backup::staging_dir();
    assert!(staging.join("READY").exists());
    assert!(staging.join("homevault.db").exists());
    // …and the state now reports the pending restore.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/config-backup",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert!(!body["data"]["pending_restore"].is_null());

    // Applying it at "startup" swaps the database in and keeps the old one.
    let applied = homevault::config_backup::apply_pending_restore().expect("applies");
    assert!(applied.is_some());
    assert!(!staging.join("READY").exists(), "the marker is consumed");
    let kept = homevault::config::paths::database_path().with_extension("db.pre-restore");
    assert!(kept.exists(), "the previous database is kept for recovery");
}

#[tokio::test]
async fn test_secrets_require_a_passphrase_and_an_encrypted_archive_round_trips() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // Secrets without a passphrase is refused before anything is written.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/config-backup/export",
        Some(&admin),
        None,
        Some(json!({"include_secrets": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "refused: {body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("must be encrypted"));

    // A short passphrase is refused too: it protects every credential.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/settings/config-backup/export",
        Some(&admin),
        None,
        Some(json!({"include_secrets": true, "passphrase": "short"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The real thing.
    let archive = export_and_fetch(&mut router, &admin, true, Some("a long enough passphrase")).await;

    // Without the passphrase the archive refuses to be read…
    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/inspect",
        &admin,
        None,
        archive.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("encrypted"));

    // …with a wrong one it fails cleanly…
    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/inspect",
        &admin,
        Some("the wrong passphrase"),
        archive.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("does not open"));

    // …and with the right one it reads.
    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/inspect",
        &admin,
        Some("a long enough passphrase"),
        archive,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "inspect: {body}");
    assert_eq!(body["data"]["manifest"]["encrypted"], true);
    assert_eq!(body["data"]["manifest"]["includes_secrets"], true);
}

/// The acceptance criterion: an archive exported **without** secrets carries no
/// working credential.
#[tokio::test]
async fn test_a_default_archive_carries_no_plaintext_credential() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // A live session and a token that would be a credential if it travelled.
    create_test_user(&h.state, "alice", "SecretMemberPass1", 1).await;
    let alice_session = login(&mut router, "alice", "SecretMemberPass1").await;
    sqlx::query(
        "INSERT INTO mcp_tokens (id, name, token_hash, token_prefix, created_at) \
         VALUES ('t1', 'test', 'hash', 'prefix', datetime('now'))",
    )
    .execute(&*h.state.db.pool())
    .await
    .ok();

    let archive = export_and_fetch(&mut router, &admin, false, None).await;
    let bytes_text = String::from_utf8_lossy(&archive).to_string();

    // No session token, no MCP token row, no password in the clear.
    assert!(
        !bytes_text.contains(&alice_session),
        "a live session token must never travel in a default archive"
    );
    assert!(!bytes_text.contains("SecretMemberPass1"));
    assert!(!bytes_text.contains("token_prefix"));
    // No secret-bearing config file either.
    for forbidden in ["tls.json", "acme.json", "ddns.json", "syncthing"] {
        assert!(
            !bytes_text.contains(forbidden),
            "{forbidden} is secret-bearing and must not be in the archive"
        );
    }

    // And the archive still describes itself honestly.
    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/inspect",
        &admin,
        None,
        archive,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["manifest"]["includes_secrets"], false);
    assert_eq!(body["data"]["manifest"]["encrypted"], false);
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_newer_archive_is_refused_with_the_versions_named() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let mut archive = export_and_fetch(&mut router, &admin, false, None).await;
    // Nudge the container version forward.
    let magic = homevault::config_backup::MAGIC.len();
    let newer = (homevault::config_backup::FORMAT_VERSION + 1).to_le_bytes();
    archive[magic..magic + 4].copy_from_slice(&newer);

    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/restore",
        &admin,
        None,
        archive,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("newer HomeVault"), "{message}");
    assert!(message.contains("Update HomeVault"), "the refusal says what to do: {message}");
    // Nothing was staged by a refused restore.
    assert!(!homevault::config_backup::staging_dir().join("READY").exists());
}

#[tokio::test]
async fn test_a_file_that_is_not_an_archive_is_refused() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send_bytes(
        &mut router,
        "/api/settings/config-backup/restore",
        &admin,
        None,
        b"this is a shopping list, not a backup".to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not a HomeVault configuration archive"));

    // An unknown archive id is a miss, not a path traversal: the id is checked
    // against the allow-list and confined to the archive directory.
    for id in ["../secrets", "..%2Fsecrets", "x/y"] {
        let (status, _) = send(
            &mut router,
            Method::GET,
            &format!("/api/settings/config-backup/archives/{id}"),
            Some(&admin),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "id {id} must be refused");
    }
}
