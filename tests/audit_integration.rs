/* =============================================================================
 * File          : audit_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the queryable audit trail (P1-18): admin gate,
 *   filters/ordering/pagination, CSV + JSON export, retention prune and the
 *   Journal `audit` source.
 *
 * Run:
 *   cargo test --test audit_integration
 * =============================================================================
 */

use axum::{
    routing::{get, post},
    Router,
};
use http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::audit::{handle_audit_export, handle_audit_list};
use homevault::handlers::journal::{handle_journal_entries, handle_journal_sources};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

async fn setup_state() -> (Arc<AppState>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("temp dir");
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
    )
}

fn audit_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/audit", get(handle_audit_list))
        .route("/api/audit/export", get(handle_audit_export))
        .route("/api/journal/sources", get(handle_journal_sources))
        .route("/api/journal/entries", get(handle_journal_entries))
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
    let (status, _, text) = send_raw(router, method, uri, token, body).await;
    let json = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    (status, json)
}

async fn send_raw(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, String) {
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
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

#[allow(clippy::too_many_arguments)]
async fn seed(
    pool: &sqlx::SqlitePool,
    created_at: &str,
    severity: &str,
    actor: Option<&str>,
    action: Option<&str>,
    message: &str,
) -> i64 {
    sqlx::query(
        "INSERT INTO audit_log (created_at, severity, actor, action, target, client_ip, message) \
         VALUES (?, ?, ?, ?, NULL, '192.0.2.10', ?)",
    )
    .bind(created_at)
    .bind(severity)
    .bind(actor)
    .bind(action)
    .bind(message)
    .execute(pool)
    .await
    .expect("seed audit row")
    .last_insert_rowid()
}

#[tokio::test]
async fn test_audit_endpoints_require_admin() {
    let (state, _tmp) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "user1", "Password123!", 1).await;

    let mut router = audit_router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;
    let user = login(&mut router, "user1", "Password123!").await;

    let (status, _) = send(&mut router, Method::GET, "/api/audit", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(&mut router, Method::GET, "/api/audit", Some(&user), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(&mut router, Method::GET, "/api/audit", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], json!(0));
    assert_eq!(body["data"]["retention_days"], json!(90));

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/audit/export?format=csv",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_audit_filters_ordering_and_pagination() {
    let (state, _tmp) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let pool = state.db.pool();

    seed(
        pool.as_ref(),
        "2026-11-07T09:00:00+00:00",
        "info",
        Some("alice"),
        Some("explorer.write"),
        "User 'alice' deleted a file",
    )
    .await;
    seed(
        pool.as_ref(),
        "2026-11-08T10:00:00+00:00",
        "info",
        Some("alice"),
        Some("auth.login"),
        "User 'alice' logged in successfully",
    )
    .await;
    seed(
        pool.as_ref(),
        "2026-11-08T11:00:00+00:00",
        "warning",
        Some("bob"),
        Some("auth.login.failed"),
        "Failed login attempt for user 'bob'",
    )
    .await;

    let mut router = audit_router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(&mut router, Method::GET, "/api/audit", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], json!(3));
    assert_eq!(body["data"]["entries"][0]["actor"], json!("bob"));
    assert_eq!(body["data"]["entries"][1]["action"], json!("auth.login"));
    assert_eq!(
        body["data"]["entries"][2]["action"],
        json!("explorer.write")
    );

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/audit?severity=warning",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], json!(1));
    assert_eq!(body["data"]["entries"][0]["actor"], json!("bob"));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/audit?actor=alice",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], json!(2));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/audit?action=auth.login",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], json!(1));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/audit?q=logged",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], json!(1));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/audit?from=2026-11-08&to=2026-11-08",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], json!(2));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/audit?limit=1&offset=1",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], json!(3));
    assert_eq!(body["data"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["entries"][0]["action"], json!("auth.login"));

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/audit?severity=fatal",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_audit_export_csv_and_json() {
    let (state, _tmp) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed(
        state.db.pool().as_ref(),
        "2026-11-08T10:00:00+00:00",
        "info",
        Some("alice"),
        Some("auth.login"),
        "user, \"quoted\" message",
    )
    .await;

    let mut router = audit_router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;

    let (status, headers, body) = send_raw(
        &mut router,
        Method::GET,
        "/api/audit/export?format=csv",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let disposition = headers
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(content_type.starts_with("text/csv"));
    assert!(disposition.contains("homevault-audit-"));
    assert!(body.starts_with("timestamp,severity,actor,action,target,client_ip,message\n"));
    assert!(body.contains("\"user, \"\"quoted\"\" message\""));

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/audit/export?format=json",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], json!(1));
    assert_eq!(body["entries"][0]["actor"], json!("alice"));

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/audit/export?format=xml",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_audit_prune_removes_only_old_rows() {
    let (state, _tmp) = setup_state().await;
    let pool = state.db.pool();
    seed(
        pool.as_ref(),
        "2020-01-01T00:00:00+00:00",
        "info",
        Some("alice"),
        Some("auth.login"),
        "old event",
    )
    .await;
    seed(
        pool.as_ref(),
        &chrono::Utc::now().to_rfc3339(),
        "info",
        Some("alice"),
        Some("auth.login"),
        "recent event",
    )
    .await;

    let removed = homevault::audit::prune(pool.as_ref(), 90)
        .await
        .expect("prune");
    assert_eq!(removed, 1);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
        .fetch_one(pool.as_ref())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_journal_audit_source_is_admin_only_and_queryable() {
    let (state, _tmp) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    create_test_user(&state, "user1", "Password123!", 1).await;
    seed(
        state.db.pool().as_ref(),
        "2026-11-08T10:00:00+00:00",
        "info",
        Some("alice"),
        Some("auth.login"),
        "User 'alice' logged in successfully",
    )
    .await;

    let mut router = audit_router(state);
    let admin = login(&mut router, "admin1", "Password123!").await;
    let user = login(&mut router, "user1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/journal/sources",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["id"] == "audit"),
        "the audit source must be advertised to an admin"
    );

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/journal/entries?source=audit",
        Some(&user),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/journal/entries?source=audit&search=alice",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let entries = body["data"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["priority_text"], json!("info"));
    assert!(entries[0]["message"]
        .as_str()
        .unwrap()
        .starts_with("[auth.login]"));
    assert_eq!(body["meta"]["scanned"], json!(1));
}
