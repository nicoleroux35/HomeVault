/* =============================================================================
 * File          : rag_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 22/10/2026 by Nicolas Le Roux
 * Last modified : 22/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   RAG knowledge-base integration tests (ROADMAP 4.1/4.2): the index
 *   reconciles (adds, updates AND removes), an embedding failure fails the
 *   run loudly instead of storing NULL-embedding chunks, and the API
 *   distinguishes an error from an empty result. Runs against a live
 *   in-memory SQLite + assembled Axum router via `oneshot`.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test rag_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the RAG knowledge base.
//!
//! The HTTP surface (`/api/rag/*`) is exercised with the real router; the
//! indexing/search/reconcile pipeline is exercised through the same
//! [`homevault::rag::EmbeddingProvider`] seam the handler uses, with a
//! deterministic offline embedder — a real index needs a live Ollama, which
//! these tests deliberately do not require.
//!
//! Run:
//!   cargo test --test rag_integration

use axum::{
    routing::{get, post},
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
    rag::{handle_rag_documents, handle_rag_index, handle_rag_search, handle_rag_status},
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::rag::{EmbedFuture, EmbeddingProvider, SearchResult};
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Deterministic, offline stand-in for Ollama. `fail` turns every batch into
/// an error, which is how "an embedding failure fails the run" is asserted.
struct FakeEmbedder {
    fail: bool,
}

impl EmbeddingProvider for FakeEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> EmbedFuture<'a> {
        let fail = self.fail;
        let n = texts.len();
        Box::pin(async move {
            if fail {
                return Err("embedding backend unavailable".to_string());
            }
            // A stable 4-dim vector, identical for every chunk: enough for
            // cosine similarity to rank, no dependence on the text.
            Ok(vec![vec![0.5f32, 0.5, 0.5, 0.5]; n])
        })
    }
}

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
        guard,
    )
}

fn rag_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/rag/status", get(handle_rag_status))
        .route("/api/rag/index", post(handle_rag_index))
        .route("/api/rag/search", post(handle_rag_search))
        .route("/api/rag/documents", get(handle_rag_documents))
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

async fn seed_document(pool: &sqlx::SqlitePool, id: &str, source: &str, title: &str) {
    sqlx::query(
        "INSERT INTO rag_documents (id, source, source_id, title, content, language)
         VALUES (?, ?, ?, ?, 'seeded body', 'en')",
    )
    .bind(id)
    .bind(source)
    .bind(id)
    .bind(title)
    .execute(pool)
    .await
    .expect("seed document");
}

// ---------------------------------------------------------------------------
// 1. HTTP auth boundaries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_status_requires_a_session_and_answers_any_role() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = rag_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    // No session → success:false (the handler's envelope, HTTP 200).
    let (status, body) = send(&mut router, Method::GET, "/api/rag/status", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false);

    // A regular session may read the status (it drives the chat hint).
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rag/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["data"]["total_documents"], 0);
    assert_eq!(body["data"]["status"], "idle");
}

#[tokio::test]
async fn test_index_and_documents_and_search_require_admin() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = rag_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rag/index",
        Some(&token),
        Some(json!({})),
    )
    .await;
    assert_eq!(body["success"], false);
    assert!(err_text(&body).to_lowercase().contains("admin"));

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/rag/documents",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["success"], false);
    assert!(err_text(&body).to_lowercase().contains("admin"));

    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/rag/search",
        Some(&token),
        Some(json!({"query": "docker"})),
    )
    .await;
    assert_eq!(body["success"], false);
    assert!(err_text(&body).to_lowercase().contains("admin"));
}

#[tokio::test]
async fn test_documents_lists_the_index_for_an_admin() {
    let (state, _tmp, _g) = setup_state().await;
    seed_document(&state.db.pool(), "catalog:demo", "catalog", "Demo").await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rag_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/rag/documents",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let docs = body["data"].as_array().expect("document array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["id"], "catalog:demo");
}

// ---------------------------------------------------------------------------
// 2. Search distinguishes an error from an empty result
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_rejects_an_empty_query_with_an_error_not_an_empty_result() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rag_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rag/search",
        Some(&token),
        Some(json!({"query": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false);
    assert!(
        body["data"].is_null(),
        "an error must not carry a results array"
    );
    assert!(err_text(&body).to_lowercase().contains("empty"));
}

#[tokio::test]
async fn test_search_without_an_embedding_backend_reports_an_error() {
    // Ollama is not running under `cargo test`: the handler must answer
    // `success:false` with a reason, never `success:true` with zero results
    // (the old behaviour that made a broken index look empty).
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = rag_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/rag/search",
        Some(&token),
        Some(json!({"query": "how do I create a raid"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false);
    assert!(!err_text(&body).is_empty(), "an error must explain itself");
}

// ---------------------------------------------------------------------------
// 3. Data layer: index → search → reconcile → remove
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_index_then_search_finds_a_document() {
    let (state, _tmp, _g) = setup_state().await;
    let pool = state.db.pool();
    let embedder = FakeEmbedder { fail: false };

    let (docs, chunks) = homevault::rag_indexers::index_all(pool.as_ref(), &embedder)
        .await
        .expect("index_all");
    assert!(docs > 0);
    assert!(chunks > 0);

    // Search with a query embedding built the same way the API would.
    let query = embedder
        .embed(&["how do I manage docker containers".to_string()])
        .await
        .expect("query embedding");
    let results: Vec<SearchResult> = homevault::rag::search_similar(pool.as_ref(), &query[0], 5)
        .await
        .expect("search");
    assert!(!results.is_empty(), "an indexed base must return hits");

    // The status reflects the populated index.
    let status = homevault::rag::get_index_status(pool.as_ref())
        .await
        .expect("status");
    assert_eq!(status.status, "idle");
    assert_eq!(status.total_documents, docs);
}

#[tokio::test]
async fn test_reindexing_removes_a_document_the_sources_no_longer_produce() {
    let (state, _tmp, _g) = setup_state().await;
    let pool = state.db.pool();
    let embedder = FakeEmbedder { fail: false };

    homevault::rag_indexers::index_all(pool.as_ref(), &embedder)
        .await
        .expect("first index");

    // Simulate a guide that was deleted (or an app renamed): a document the
    // current sources no longer emit.
    seed_document(pool.as_ref(), "feature:ghost", "feature", "Ghost").await;

    homevault::rag_indexers::index_all(pool.as_ref(), &embedder)
        .await
        .expect("second index");

    let ghosts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM rag_documents WHERE id = 'feature:ghost'")
            .fetch_one(pool.as_ref())
            .await
            .expect("count");
    assert_eq!(ghosts, 0, "reconcile must remove what the sources dropped");
}

#[tokio::test]
async fn test_indexing_fails_loudly_and_stores_no_null_embedding() {
    let (state, _tmp, _g) = setup_state().await;
    let pool = state.db.pool();

    let err = homevault::rag_indexers::index_all(pool.as_ref(), &FakeEmbedder { fail: true })
        .await
        .expect_err("a dead embedding backend must fail the run");
    assert!(err.contains("embedding failed"), "unexpected error: {err}");

    let nulls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rag_chunks WHERE embedding IS NULL")
        .fetch_one(pool.as_ref())
        .await
        .expect("null-embedding count");
    assert_eq!(nulls, 0, "no chunk may be stored without an embedding");

    let status = homevault::rag::get_index_status(pool.as_ref())
        .await
        .expect("status");
    assert_eq!(status.status, "error");
}
