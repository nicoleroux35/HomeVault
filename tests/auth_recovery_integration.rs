// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the numeric recovery-code password-reset flow.
//!
//! Fresh in-memory SQLite + embedded migrations per test, minimal Axum router,
//! `tower::ServiceExt::oneshot` for HTTP-level testing.
//!
//! Run:
//!   cargo test --test auth_recovery_integration

use axum::Router;
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use tower::ServiceExt;

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::{
        handle_generate_recovery_code, handle_reset_password_recovery, handle_verify_recovery_code,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

async fn setup_db() -> Database {
    let db = Database::new("sqlite::memory:")
        .await
        .expect("in-memory db");
    db.run_migrations().await.expect("migrations");
    db
}

async fn setup_state() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = setup_db().await;
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
        .route(
            "/api/auth/recovery-code",
            axum::routing::post(handle_generate_recovery_code),
        )
        .route(
            "/api/auth/verify-recovery-code",
            axum::routing::post(handle_verify_recovery_code),
        )
        .route(
            "/api/auth/reset-password-recovery",
            axum::routing::post(handle_reset_password_recovery),
        )
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, password: &str) {
    let hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, '', '', 2, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&hash)
    .execute(&*state.db.pool())
    .await
    .expect("insert user");
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn post(uri: &str, body: Value) -> Req {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

fn post_auth(uri: &str, token: &str, body: Value) -> Req {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-session-token", token)
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn generate_requires_a_session() {
    let (state, _tmp, _g) = setup_state().await;
    let app = router(state);

    let resp = app
        .oneshot(post("/api/auth/recovery-code", json!({})))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn generate_returns_eight_groups_and_rejects_duplicate() {
    let (state, _tmp, _g) = setup_state().await;
    create_user(&state, "alice", "OldPassw0rd").await;
    let session = state
        .auth
        .create_session("alice".into(), "127.0.0.1".into(), false)
        .await
        .unwrap();
    let app = router(state);

    let resp = app
        .clone()
        .oneshot(post_auth(
            "/api/auth/recovery-code",
            &session.token,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let code = v["data"]["recovery_code"].as_str().unwrap().to_string();
    let groups: Vec<&str> = code.split(' ').collect();
    assert_eq!(groups.len(), 8);
    assert!(groups
        .iter()
        .all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_digit())));

    // Second call without ?regenerate must be refused.
    let resp = app
        .clone()
        .oneshot(post_auth(
            "/api/auth/recovery-code",
            &session.token,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // ?regenerate=true replaces it with a different code.
    let resp = app
        .oneshot(post_auth(
            "/api/auth/recovery-code?regenerate=true",
            &session.token,
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_ne!(v["data"]["recovery_code"].as_str().unwrap(), code);
}

#[tokio::test]
async fn verify_accepts_correct_code_and_rejects_wrong_one() {
    let (state, _tmp, _g) = setup_state().await;
    create_user(&state, "bob", "OldPassw0rd").await;
    let code = state.auth.set_recovery_code("bob").await.unwrap();
    let app = router(state);

    // Wrong code -> generic 400.
    let resp = app
        .clone()
        .oneshot(post(
            "/api/auth/verify-recovery-code",
            json!({ "username": "bob", "recovery_code": "0000 0000 0000 0000 0000 0000 0000 0000" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Correct code, spaces stripped -> {valid:true}.
    let resp = app
        .oneshot(post(
            "/api/auth/verify-recovery-code",
            json!({ "username": "bob", "recovery_code": code.replace(' ', "") }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["data"]["valid"], json!(true));
}

#[tokio::test]
async fn reset_with_recovery_code_changes_password_and_kills_sessions() {
    let (state, _tmp, _g) = setup_state().await;
    create_user(&state, "carol", "OldPassw0rd").await;
    let code = state.auth.set_recovery_code("carol").await.unwrap();
    let session = state
        .auth
        .create_session("carol".into(), "127.0.0.1".into(), false)
        .await
        .unwrap();
    let app = router(state.clone());

    let resp = app
        .oneshot(post(
            "/api/auth/reset-password-recovery",
            json!({
                "username": "carol",
                "recovery_code": code,
                "new_password": "BrandNewP4ss",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Old password no longer works, new one does.
    assert!(state
        .auth
        .authenticate("carol", "OldPassw0rd", "127.0.0.1", None)
        .await
        .is_err());
    assert!(state
        .auth
        .authenticate("carol", "BrandNewP4ss", "127.0.0.1", None)
        .await
        .is_ok());

    // Prior session was invalidated.
    assert!(state.auth.get_session(&session.token).await.is_err());
}

#[tokio::test]
async fn reset_rejects_weak_password() {
    let (state, _tmp, _g) = setup_state().await;
    create_user(&state, "dave", "OldPassw0rd").await;
    let code = state.auth.set_recovery_code("dave").await.unwrap();
    let app = router(state);

    let resp = app
        .oneshot(post(
            "/api/auth/reset-password-recovery",
            json!({ "username": "dave", "recovery_code": code, "new_password": "short" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("security requirements"),
        "unexpected message: {}",
        v["message"]
    );
}

#[tokio::test]
async fn reset_rejects_a_recently_used_password() {
    let (state, _tmp, _g) = setup_state().await;
    create_user(&state, "frank", "FirstPass1").await;
    let code = state.auth.set_recovery_code("frank").await.unwrap();
    let app = router(state.clone());

    // First reset moves FirstPass1 into history.
    let resp = app
        .clone()
        .oneshot(post(
            "/api/auth/reset-password-recovery",
            json!({ "username": "frank", "recovery_code": code, "new_password": "SecondPass2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Trying to go back to the original password is refused.
    let resp = app
        .oneshot(post(
            "/api/auth/reset-password-recovery",
            json!({ "username": "frank", "recovery_code": code, "new_password": "FirstPass1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let v = body_json(resp).await;
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("recently"),
        "unexpected message: {}",
        v["message"]
    );
}

#[tokio::test]
async fn reset_rejects_wrong_code_generically() {
    let (state, _tmp, _g) = setup_state().await;
    create_user(&state, "erin", "OldPassw0rd").await;
    state.auth.set_recovery_code("erin").await.unwrap();
    let app = router(state.clone());

    let resp = app
        .oneshot(post(
            "/api/auth/reset-password-recovery",
            json!({
                "username": "erin",
                "recovery_code": "1111 2222 3333 4444 5555 6666 7777 8888",
                "new_password": "BrandNewP4ss",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // Password must be unchanged.
    assert!(state
        .auth
        .authenticate("erin", "OldPassw0rd", "127.0.0.1", None)
        .await
        .is_ok());
}
