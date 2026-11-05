/* =============================================================================
 * File          : password_policy_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Password-expiry policy integration tests: in-memory SQLite + assembled
 *   Axum router. Covers the admin matrix, the round-trip, the bounds, and the
 *   behaviour that matters — an old password is flagged at sign-in, a fresh
 *   one is not, a change restarts the clock, and `0` means never.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test password_policy_integration
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
use homevault::handlers::auth::handle_login;
use homevault::handlers::password_policy::{
    handle_password_policy_get, handle_password_policy_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize temp dir");
    std::env::set_var("DATA_DIRECTORY", &root);

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
        _tmp: tmp,
        _guard: guard,
    }
}

fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/settings/password-policy",
            get(handle_password_policy_get).put(handle_password_policy_update),
        )
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
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

async fn age_password(state: &AppState, username: &str, days: i64) {
    sqlx::query(
        "UPDATE users SET created_at = datetime('now', ?), password_changed_at = datetime('now', ?)
         WHERE username = ?",
    )
    .bind(format!("-{days} days"))
    .bind(format!("-{days} days"))
    .bind(username)
    .execute(&*state.db.pool())
    .await
    .expect("age password");
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
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

async fn login(router: &mut Router, username: &str, password: &str) -> Value {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {body}");
    body
}

#[tokio::test]
async fn test_password_policy_admin_matrix_and_bounds() {
    let h = setup().await;
    create_user(&h.state, "admin", "AdminPass123", 2).await;
    create_user(&h.state, "member", "MemberPass123", 1).await;
    let mut router = build_router(Arc::clone(&h.state));

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/password-policy",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let member = login(&mut router, "member", "MemberPass123").await;
    let member_token = member["data"]["token"].as_str().unwrap();
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/password-policy",
        Some(member_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = login(&mut router, "admin", "AdminPass123").await;
    let admin_token = admin["data"]["token"].as_str().unwrap();

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/password-policy",
        Some(admin_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["expiry_days"], 0);
    assert_eq!(body["data"]["max_expiry_days"], 3650);

    for (days, expected) in [(30, StatusCode::OK), (3650, StatusCode::OK)] {
        let (status, _) = send(
            &mut router,
            Method::PUT,
            "/api/settings/password-policy",
            Some(admin_token),
            Some(json!({ "expiry_days": days })),
        )
        .await;
        assert_eq!(status, expected, "days={days}");
    }

    for days in [-1, 3651] {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/settings/password-policy",
            Some(admin_token),
            Some(json!({ "expiry_days": days })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "days={days}");
        assert!(body["message"].as_str().unwrap().contains("expiry_days"));
    }

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/password-policy",
        Some(admin_token),
        None,
    )
    .await;
    assert_eq!(
        body["data"]["expiry_days"], 3650,
        "the refused writes must not change the stored value"
    );
}

#[tokio::test]
async fn test_expired_password_is_flagged_at_sign_in_and_a_change_restarts_it() {
    let h = setup().await;
    create_user(&h.state, "admin", "AdminPass123", 2).await;
    create_user(&h.state, "alice", "AlicePass123", 1).await;
    let mut router = build_router(Arc::clone(&h.state));

    let admin = login(&mut router, "admin", "AdminPass123").await;
    let admin_token = admin["data"]["token"].as_str().unwrap();
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/password-policy",
        Some(admin_token),
        Some(json!({ "expiry_days": 30 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    age_password(&h.state, "alice", 40).await;

    let body = login(&mut router, "alice", "AlicePass123").await;
    assert_eq!(
        body["data"]["must_change_password"], true,
        "a 40-day-old password under a 30-day policy must be flagged"
    );

    // The user changes it; the clock restarts.
    h.state
        .auth
        .update_password("alice", Some("AlicePass123"), "NewAlicePass456", false)
        .await
        .expect("password change");
    let body = login(&mut router, "alice", "NewAlicePass456").await;
    assert_eq!(body["data"]["must_change_password"], false);
}

#[tokio::test]
async fn test_zero_days_never_expires_and_fresh_password_is_not_flagged() {
    let h = setup().await;
    create_user(&h.state, "admin", "AdminPass123", 2).await;
    create_user(&h.state, "bob", "BobPass12345", 1).await;
    create_user(&h.state, "carol", "CarolPass123", 1).await;
    let mut router = build_router(Arc::clone(&h.state));

    // carol's password is ancient, but the policy is still 0.
    age_password(&h.state, "carol", 4000).await;
    let body = login(&mut router, "carol", "CarolPass123").await;
    assert_eq!(body["data"]["must_change_password"], false);

    let admin = login(&mut router, "admin", "AdminPass123").await;
    let admin_token = admin["data"]["token"].as_str().unwrap();
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/password-policy",
        Some(admin_token),
        Some(json!({ "expiry_days": 30 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // bob's password is fresh: not flagged.
    let body = login(&mut router, "bob", "BobPass12345").await;
    assert_eq!(body["data"]["must_change_password"], false);
}
