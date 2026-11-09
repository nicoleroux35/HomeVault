/* =============================================================================
 * File          : restore_drill_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the non-destructive restore drill (P2-9): admin
 *   gate, a valid snapshot + archive reports ready, corruption is detected,
 *   and an encrypted archive is only fully verified when its passphrase is
 *   supplied.
 *
 * Run:
 *   cargo test --test restore_drill_integration
 * =============================================================================
 */

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
use homevault::config_backup::{self, ExportInput};
use homevault::db_backup::DbBackupService;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Fixture {
    state: Arc<AppState>,
    tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn fixture() -> Fixture {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    // A file-backed database, like production: `VACUUM INTO` is a no-op for
    // SQLx's `sqlite::memory:` pool (SQLite refuses the copy between two
    // in-memory databases), and the drill's whole point is a real snapshot.
    let db_path = tmp.path().join("homevault.db");
    let db = Database::new(&format!("sqlite:{}?mode=rwc", db_path.display()))
        .await
        .expect("db");
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
        tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/settings/backup/restore-drill",
            get(homevault::handlers::restore_drill::handle_restore_drill_get)
                .post(homevault::handlers::restore_drill::handle_restore_drill_run),
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
    passphrase: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        req = req.header("x-session-token", token);
    }
    if let Some(passphrase) = passphrase {
        req = req.header("x-backup-passphrase", passphrase);
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
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn backups_dir(tmp: &std::path::Path) -> std::path::PathBuf {
    tmp.join("data").join("backups")
}

fn config_dir(tmp: &std::path::Path) -> std::path::PathBuf {
    tmp.join("data").join("config-backups")
}

/// Writes a real `VACUUM INTO` snapshot of the in-memory database, then a
/// valid (unencrypted) configuration archive built from those bytes.
async fn seed_valid_chain(state: &AppState, tmp: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
    let dir = backups_dir(tmp);
    std::fs::create_dir_all(&dir).unwrap();
    let service = DbBackupService::new(state.db.clone());
    let entry = service.snapshot_into(&dir).await.expect("snapshot");
    let snapshot = std::fs::read(&entry.path).expect("read snapshot");

    let cdir = config_dir(tmp);
    std::fs::create_dir_all(&cdir).unwrap();
    let archive = config_backup::build(
        &ExportInput {
            database: snapshot.clone(),
            settings: "{}".to_string(),
            configs: vec![("configs/smb.conf".to_string(), b"[global]\n".to_vec())],
            skipped: Vec::new(),
            app_version: "test".to_string(),
            settings_schema: 1,
        },
        false,
        None,
        "2026-11-08T00:00:00+00:00",
    )
    .expect("build archive");
    std::fs::write(cdir.join("test.hvbak"), &archive).unwrap();
    (snapshot, archive)
}

#[tokio::test]
async fn test_restore_drill_requires_admin() {
    let fx = fixture().await;
    create_test_user(&fx.state, "admin1", "Password123!", 2).await;
    create_test_user(&fx.state, "user1", "Password123!", 1).await;
    let mut router = router(fx.state.clone());
    let admin = login(&mut router, "admin1", "Password123!").await;
    let user = login(&mut router, "user1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/backup/restore-drill",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/backup/restore-drill",
        Some(&user),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["report"], json!(null));
}

#[tokio::test]
async fn test_restore_drill_is_ready_with_a_valid_snapshot_and_archive() {
    let fx = fixture().await;
    create_test_user(&fx.state, "admin1", "Password123!", 2).await;
    seed_valid_chain(&fx.state, fx.tmp.path()).await;
    // A recent successful data-backup job makes the advisory line green too.
    sqlx::query(
        "INSERT INTO restic_repos (id, name, repository, password, source_paths, created_at, updated_at)
         VALUES ('r1', 'repo', '/tmp/repo', 'secret', '[]', datetime('now'), datetime('now'))",
    )
    .execute(&*fx.state.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO restic_backup_jobs (id, repo_id, source, status, started_at, finished_at, created_at)
         VALUES ('j1', 'r1', 'directory', 'success', datetime('now'), ?, datetime('now'))",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&*fx.state.db.pool())
    .await
    .unwrap();

    let mut router = router(fx.state.clone());
    let admin = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["report"]["ready"], json!(true), "{body}");
    assert_eq!(body["data"]["report"]["database"]["ok"], json!(true));
    assert_eq!(body["data"]["report"]["database"]["verified"], json!(true));
    assert_eq!(body["data"]["report"]["config"]["ok"], json!(true));
    assert_eq!(body["data"]["report"]["data_backup"]["verified"], json!(true));

    // The report is persisted and readable without re-running the drill.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["report"]["ready"], json!(true));
}

#[tokio::test]
async fn test_restore_drill_detects_a_corrupt_database_snapshot() {
    let fx = fixture().await;
    create_test_user(&fx.state, "admin1", "Password123!", 2).await;
    seed_valid_chain(&fx.state, fx.tmp.path()).await;
    let snapshot = std::fs::read_dir(backups_dir(fx.tmp.path()))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|e| e.to_str()) == Some("db"))
        .expect("snapshot file");
    std::fs::write(&snapshot, b"this is not a sqlite database").unwrap();

    let mut router = router(fx.state.clone());
    let admin = login(&mut router, "admin1", "Password123!").await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["report"]["ready"], json!(false), "{body}");
    assert_eq!(body["data"]["report"]["database"]["ok"], json!(false));
    assert!(body["data"]["report"]["database"]["detail"]
        .as_str()
        .unwrap()
        .contains("not a readable SQLite database"));
}

#[tokio::test]
async fn test_restore_drill_detects_a_corrupt_configuration_archive() {
    let fx = fixture().await;
    create_test_user(&fx.state, "admin1", "Password123!", 2).await;
    seed_valid_chain(&fx.state, fx.tmp.path()).await;
    std::fs::write(config_dir(fx.tmp.path()).join("test.hvbak"), b"garbage").unwrap();

    let mut router = router(fx.state.clone());
    let admin = login(&mut router, "admin1", "Password123!").await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["report"]["ready"], json!(false), "{body}");
    assert_eq!(body["data"]["report"]["config"]["ok"], json!(false));
}

#[tokio::test]
async fn test_restore_drill_verifies_an_encrypted_archive_with_its_passphrase() {
    let fx = fixture().await;
    create_test_user(&fx.state, "admin1", "Password123!", 2).await;
    let (snapshot, _) = seed_valid_chain(&fx.state, fx.tmp.path()).await;

    // Replace the plain archive with an encrypted one.
    let encrypted = config_backup::build(
        &ExportInput {
            database: snapshot,
            settings: "{}".to_string(),
            configs: Vec::new(),
            skipped: Vec::new(),
            app_version: "test".to_string(),
            settings_schema: 1,
        },
        true,
        Some("correct horse battery staple"),
        "2026-11-08T00:00:00+00:00",
    )
    .unwrap();
    std::fs::write(config_dir(fx.tmp.path()).join("test.hvbak"), &encrypted).unwrap();

    let mut router = router(fx.state.clone());
    let admin = login(&mut router, "admin1", "Password123!").await;

    // Without the passphrase: structure valid, payload not verified, not ready.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["report"]["config"]["ok"], json!(true));
    assert_eq!(body["data"]["report"]["config"]["verified"], json!(false));
    assert_eq!(body["data"]["report"]["ready"], json!(false));

    // With the passphrase: fully verified and ready.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/backup/restore-drill",
        Some(&admin),
        Some("correct horse battery staple"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["report"]["config"]["verified"], json!(true));
    assert_eq!(body["data"]["report"]["ready"], json!(true));
}
