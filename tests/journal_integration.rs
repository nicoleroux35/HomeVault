// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the System Journal API (`crate::handlers::journal`):
//!
//!   GET    /api/journal/sources           (list log sources — any session)
//!   GET    /api/journal/entries           (read entries — any session)
//!   DELETE /api/journal/clear             (wipe our own app logs — admin)
//!   GET    /api/journal/cleanup-settings  (auto-cleanup config — any session)
//!   PUT    /api/journal/cleanup-settings  (auto-cleanup config — admin)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! The `app` source reads HomeVault's own rotating log files from
//! `DATA_DIRECTORY/logs` — a plain filesystem read the process owns, so
//! this suite drives it for real against files written into a per-test
//! `tempfile::tempdir()`. The `system` / `/var/log` sources shell out to
//! `sudo journalctl` / `sudo tail`; those are only exercised far enough to
//! confirm a clean `{"success":false}` (never a 500) when `sudo` is not
//! available — the real host read is `tests/e2e/journal/journal.spec.ts`'s
//! job. `DELETE /api/journal/clear` is safe here: it only ever deletes
//! `homevault.log*` inside the test's temp `logs/` dir.
//!
//! Run:
//!   cargo test --test journal_integration

use axum::{
    routing::{delete, get},
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
use homevault::handlers::journal::{
    handle_journal_cleanup_settings_get, handle_journal_cleanup_settings_update,
    handle_journal_clear, handle_journal_entries, handle_journal_sources,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    /// The temp `DATA_DIRECTORY`; `logs/` lives under here.
    data_dir: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = tmp.path().to_path_buf();
    std::env::set_var("DATA_DIRECTORY", &data_dir);

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    let path_guard = PathGuard::new(vec!["/tmp".to_string()]);
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
        data_dir,
        _tmp: tmp,
        _guard: guard,
    }
}

impl Harness {
    /// Write `contents` to `DATA_DIRECTORY/logs/<name>`.
    fn write_log(&self, name: &str, contents: &str) {
        let logs = self.data_dir.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join(name), contents).unwrap();
    }

    fn log_files(&self) -> Vec<String> {
        let logs = self.data_dir.join("logs");
        std::fs::read_dir(&logs)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn journal_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/journal/sources", get(handle_journal_sources))
        .route("/api/journal/entries", get(handle_journal_entries))
        .route("/api/journal/clear", delete(handle_journal_clear))
        .route(
            "/api/journal/cleanup-settings",
            get(handle_journal_cleanup_settings_get).put(handle_journal_cleanup_settings_update),
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
async fn test_read_endpoints_require_a_session() {
    let h = setup().await;
    let mut r = journal_router(h.state.clone());
    for uri in [
        "/api/journal/sources",
        "/api/journal/entries",
        "/api/journal/cleanup-settings",
    ] {
        let (status, _) = send(&mut r, Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} (no token)");
        let (status, _) = send(&mut r, Method::GET, uri, Some("nope"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri} (bad token)");
    }
}

#[tokio::test]
async fn test_write_endpoints_require_an_admin() {
    let h = setup().await;
    create_test_user(&h.state, "regular", "Password123!", 1).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "regular", "Password123!").await;

    let cases: &[(Method, &str, Option<Value>)] = &[
        (Method::DELETE, "/api/journal/clear", None),
        (
            Method::PUT,
            "/api/journal/cleanup-settings",
            Some(json!({"auto_cleanup_enabled": false, "interval_days": 30})),
        ),
    ];
    for (method, uri, body) in cases {
        let (status, _) = send(&mut r, method.clone(), uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri} (no token)");
        let (status, _) = send(&mut r, method.clone(), uri, Some(&token), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} (non-admin)");
    }
}

// ---------------------------------------------------------------------------
// 2. Sources
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_sources_always_lists_app_and_system() {
    let h = setup().await;
    create_test_user(&h.state, "u", "Password123!", 1).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/sources",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let ids: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert!(ids.contains(&"app"), "{ids:?}");
    assert!(ids.contains(&"system"), "{ids:?}");
    // A /var/log text source is only advertised if the file exists — never asserted here.
}

// ---------------------------------------------------------------------------
// 3. Entries — the `app` source, read for real
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_entries_app_source_parses_our_own_log_lines() {
    let h = setup().await;
    create_test_user(&h.state, "u", "Password123!", 1).await;
    h.write_log(
        "homevault.log",
        "2026-07-17T10:00:00.000000Z  INFO homevault::auth: user 'alice' logged in\n\
         2026-07-17T10:01:00.000000Z  WARN homevault::disk: SMART warning on /dev/sdb\n\
         2026-07-17T10:02:00.000000Z ERROR homevault::event: Admin 'bob' deleted a share\n",
    );
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/entries?source=app",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true), "{body}");
    let entries = body["data"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["unit"], "homevault::auth");
    assert_eq!(entries[0]["priority_text"], "info");
    assert_eq!(entries[0]["message"], "user 'alice' logged in");
    assert_eq!(entries[2]["priority_text"], "err");
}

#[tokio::test]
async fn test_entries_app_source_honours_the_search_filter() {
    let h = setup().await;
    create_test_user(&h.state, "u", "Password123!", 1).await;
    h.write_log(
        "homevault.log",
        "2026-07-17T10:00:00Z  INFO homevault::auth: user 'alice' logged in\n\
         2026-07-17T10:01:00Z  INFO homevault::auth: user 'bob' logged out\n",
    );
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/entries?source=app&search=ALICE",
        Some(&token),
        None,
    )
    .await;
    let entries = body["data"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["message"], "user 'alice' logged in");
}

#[tokio::test]
async fn test_entries_app_source_is_empty_when_nothing_logged() {
    let h = setup().await;
    create_test_user(&h.state, "u", "Password123!", 1).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/entries?source=app",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn test_entries_unknown_source_fails_cleanly() {
    let h = setup().await;
    create_test_user(&h.state, "u", "Password123!", 1).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/entries?source=notreal",
        Some(&token),
        None,
    )
    .await;
    // Never a 500 — a clean `{"success":false}`.
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["success"], json!(false));
}

// ---------------------------------------------------------------------------
// 4. Clear — only our own log files, admin-gated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_clear_removes_only_our_own_log_files() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let active_name = format!("homevault.log.{}", chrono::Utc::now().format("%Y-%m-%d"));
    h.write_log("homevault.log", "line\n");
    h.write_log(&active_name, "today's line\n");
    h.write_log("homevault.log.2026-07-16", "old line\n");
    h.write_log("something-else.log", "keep me\n");
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/journal/clear",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));

    // The appender keeps today's file (`homevault.log`, `homevault.log.<UTC
    // date>`) open for the whole process lifetime: clear truncates them in
    // place (UI shows them empty, the open inode survives) instead of
    // unlinking them — see `clear_app_logs`. Only the older rotated archives
    // are really deleted, and files not owned by HomeVault must stay.
    let left = h.log_files();
    assert!(left.contains(&"homevault.log".to_string()), "{left:?}");
    assert!(left.contains(&active_name), "{left:?}");
    assert!(
        !left.contains(&"homevault.log.2026-07-16".to_string()),
        "{left:?}"
    );
    assert!(left.contains(&"something-else.log".to_string()), "{left:?}");

    for name in ["homevault.log", active_name.as_str()] {
        let contents = std::fs::read_to_string(h.data_dir.join("logs").join(name))
            .unwrap_or_else(|e| panic!("read {}: {}", name, e));
        assert_eq!(contents, "", "{name} should be truncated to empty");
    }
    let foreign =
        std::fs::read_to_string(h.data_dir.join("logs").join("something-else.log")).unwrap();
    assert_eq!(foreign, "keep me\n");
}

#[tokio::test]
async fn test_clear_is_a_clean_success_when_there_is_nothing_to_clear() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/journal/clear",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

// ---------------------------------------------------------------------------
// 5. Cleanup settings — the auto-retention preference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cleanup_settings_default_is_on_with_a_90_day_interval() {
    let h = setup().await;
    create_test_user(&h.state, "u", "Password123!", 1).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "u", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/cleanup-settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["auto_cleanup_enabled"], json!(true));
    assert_eq!(body["data"]["interval_days"], json!(90));
}

#[tokio::test]
async fn test_cleanup_settings_round_trip_and_clamp() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = journal_router(h.state.clone());
    let token = login(&mut r, "admin1", "Password123!").await;

    // Save a real value.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/journal/cleanup-settings",
        Some(&token),
        Some(json!({"auto_cleanup_enabled": false, "interval_days": 14})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/cleanup-settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["auto_cleanup_enabled"], json!(false));
    assert_eq!(body["data"]["interval_days"], json!(14));

    // interval_days: 0 is clamped up to 1 ("never" is not what a day count means).
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/journal/cleanup-settings",
        Some(&token),
        Some(json!({"auto_cleanup_enabled": true, "interval_days": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/journal/cleanup-settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["interval_days"], json!(1));
}
