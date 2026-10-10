// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the AI Assistant (Chat) API — `crate::handlers::chat`.
//!
//! 7 endpoints:
//!   POST /api/chat/send            — one-shot chat (session)
//!   POST /api/chat/send/stream     — streamed chat, SSE (session)
//!   GET  /api/chat/models          — list Ollama models (session)
//!   POST /api/chat/models/pull     — pull a model (admin)
//!   GET  /api/chat/models/recommend— model recommendations for this host
//!   GET  /api/chat/model           — get the active model (session)
//!   PUT  /api/chat/model           — set the active model (admin)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server, no Ollama.
//!
//! The 2026-07-13 audit's headline finding was that `handle_chat_send`,
//! `handle_chat_models_list` and `handle_chat_model_get` had **no session
//! check at all** — anyone reachable could chat or enumerate models. This
//! suite locks that in: `send` / `send/stream` / `models` / `model` GET
//! need a valid session (any role); `models/pull` and `model` PUT need
//! **admin**. A valid session that reaches Ollama (not running here) gets a
//! clean "cannot reach Ollama" error, never a 500 — so the auth boundary
//! is testable without a model. The real chat round-trip (a running Ollama
//! + a pulled model) is `tests/e2e/chat/chat.spec.ts` (opt-in).
//!
//! Response conventions differ per handler (documented inline): `send` /
//! `models` always answer HTTP 200 with `{"success":false}`; `model` GET/PUT
//! and `models/pull` use real 400/401/403 via `ResponseHelper`.
//!
//! (`GET /api/chat/models/recommend` was the only chat endpoint with no
//! session check — it leaked the host's RAM + GPU to any caller. Fixed
//! 2026-09-06 to require a session like its siblings;
//! `test_recommend_requires_a_session` covers it.)
//!
//! Run:
//!   cargo test --test chat_integration
//!   cargo test --test chat_integration -- --nocapture

use axum::{
    routing::{get, post, put},
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
    chat::{
        handle_chat_model_get, handle_chat_model_set, handle_chat_models_list,
        handle_chat_models_pull, handle_chat_models_recommend, handle_chat_send,
        handle_chat_send_stream,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

fn chat_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/chat/send", post(handle_chat_send))
        .route("/api/chat/send/stream", post(handle_chat_send_stream))
        .route("/api/chat/models", get(handle_chat_models_list))
        .route("/api/chat/models/pull", post(handle_chat_models_pull))
        .route(
            "/api/chat/models/recommend",
            get(handle_chat_models_recommend),
        )
        .route("/api/chat/model", get(handle_chat_model_get))
        .route("/api/chat/model", put(handle_chat_model_set))
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

fn err_text(b: &Value) -> String {
    b["error"]
        .as_str()
        .or_else(|| b["message"].as_str())
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// 1. /send + /models + /model GET — a valid session (any role) is required
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_send_and_models_and_model_get_reject_a_missing_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = chat_router(state);

    // /send answers HTTP 200 with {success:false}
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/chat/send",
        None,
        Some(json!({"message": "hi"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false);
    assert!(err_text(&body).to_lowercase().contains("session"));

    // /models same
    let (_, body) = send(&mut router, Method::GET, "/api/chat/models", None, None).await;
    assert_eq!(body["success"], false);

    // /model GET uses a real status code
    let (status, _) = send(&mut router, Method::GET, "/api/chat/model", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // /send/stream uses 401
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/chat/send/stream",
        None,
        Some(json!({"message": "hi"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_send_and_models_reject_a_bogus_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = chat_router(state);

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/chat/send",
        Some("not-a-real-token"),
        Some(json!({"message": "hi"})),
    )
    .await;
    assert_eq!(body["success"], false);

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/chat/model",
        Some("not-a-real-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_a_regular_user_may_chat_and_read_the_model() {
    // `send` requires *a session*, not admin (chatting is not privileged) —
    // the fix was to require any session, not to lock it to admins.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = chat_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    // /model GET works for a non-admin
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/chat/model",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["model"].as_str().is_some());

    // /send passes the auth gate — any failure is a reachability/error from
    // Ollama (running or not), never a session error.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/chat/send",
        Some(&token),
        Some(json!({"message": "hello", "language": "en"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    if body["success"] == false {
        let e = err_text(&body).to_lowercase();
        assert!(
            e.contains("ai communication error") || e.contains("ollama") || e.contains("connect"),
            "expected an Ollama-reach error, got {e:?}"
        );
        assert!(!e.contains("session"), "must be past the session check");
    }
}

#[tokio::test]
async fn test_models_list_for_a_valid_session_reports_ollama_state() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = chat_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/chat/models",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Well-formed body with the current model regardless of Ollama state;
    // `success` mirrors `ollama_running` when Ollama answers cleanly, and the
    // response must stay parseable either way.
    assert!(body["data"]["current"].as_str().is_some());
    assert!(body["data"]["ollama_running"].is_boolean());
}

// ---------------------------------------------------------------------------
// 2. /models/pull + /model PUT — admin only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pull_and_model_set_require_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = chat_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/chat/models/pull",
        Some(&token),
        Some(json!({"name": "gemma3:1b"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err_text(&body).to_lowercase().contains("admin"));

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/chat/model",
        Some(&token),
        Some(json!({"model": "gemma3:1b"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err_text(&body).to_lowercase().contains("admin"));
}

#[tokio::test]
async fn test_pull_and_model_set_reject_missing_or_empty_name() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = chat_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // empty name (admin) — rejected before any Ollama call
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/chat/models/pull",
        Some(&token),
        Some(json!({"name": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        err_text(&body).to_lowercase().contains("modèle")
            || err_text(&body).to_lowercase().contains("model")
    );

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/chat/model",
        Some(&token),
        Some(json!({"model": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // missing field entirely → 422 from the Json extractor
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/chat/model",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn test_model_set_for_admin_updates_the_active_model_then_restore() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = chat_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // capture the current model so we can put it back (process-global)
    let (_, before) = send(
        &mut router,
        Method::GET,
        "/api/chat/model",
        Some(&token),
        None,
    )
    .await;
    let original = before["data"]["model"].as_str().unwrap().to_string();

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/chat/model",
        Some(&token),
        Some(json!({"model": "llama3.2:3b"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["success"] == true || body["message"].as_str().is_some());

    let (_, after) = send(
        &mut router,
        Method::GET,
        "/api/chat/model",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(after["data"]["model"], "llama3.2:3b");

    // restore
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/chat/model",
        Some(&token),
        Some(json!({ "model": original })),
    )
    .await;
}

// ---------------------------------------------------------------------------
// 3. /models/recommend — now requires a session (fixed 2026-09-06)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_recommend_requires_a_session() {
    // This used to be the only chat endpoint with no auth — it exposed the
    // host's RAM + GPU to any caller. Fixed to match its siblings.
    let (state, _tmp, _g) = setup_state().await;
    let mut router = chat_router(state);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/chat/models/recommend",
        None, // no token
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false);
    // no host details leaked to an unauthenticated caller
    assert_eq!(body["data"]["system"]["total_ram_gb"].as_u64(), Some(0));
    assert!(body["data"]["recommendations"]
        .as_array()
        .unwrap()
        .is_empty());

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/chat/models/recommend",
        Some("bogus-token"),
        None,
    )
    .await;
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn test_recommend_also_works_with_a_valid_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = chat_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/chat/models/recommend",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
}
