/* =============================================================================
 * File          : progress_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SSE progress tests: job creation, event streaming, activity feed visibility rules.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the SSE progress API — `crate::handlers::progress`:
//!
//!   - GET    /api/progress/{job_id}          (SSE stream)
//!   - GET    /api/progress/jobs              (list active jobs)
//!   - GET    /api/progress/{job_id}/status
//!   - DELETE /api/progress/{job_id}
//!
//! Audit finding **G7** (2026-09-07): these four handlers previously only
//! checked that `x-session-token` was **non-empty** — the same
//! "any non-empty token accepted" bypass class fixed elsewhere in the
//! project. `handle_progress_sse` had no check at all. They now all call
//! `require_auth`. This suite locks that in:
//!
//!   1. no token            -> 400 (require_auth's no-token branch)
//!   2. a garbage token     -> 401 (invalid/expired session)
//!   3. a valid session     -> 200 (and the endpoint does its job)
//!
//! It also asserts the response-hardening middleware (finding **G5**):
//! every response carries the CSP / X-Frame-Options / nosniff headers.
//!
//! Fresh in-memory SQLite + migrations per test, minimal Axum router,
//! `tower::ServiceExt` — no real server, no real long-running job.
//!
//! Run: `cargo test --test progress_integration`

use axum::{
    routing::{delete, get},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::{LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    progress::{handle_cancel_job, handle_get_job_status, handle_list_jobs, handle_progress_sse},
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::middleware::security_headers::ArrivedOverTls;
use homevault::middleware::security_headers_middleware;
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

fn progress_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route("/api/progress/{job_id}", get(handle_progress_sse))
        .route("/api/progress/jobs", get(handle_list_jobs))
        .route("/api/progress/{job_id}/status", get(handle_get_job_status))
        .route("/api/progress/{job_id}", delete(handle_cancel_job))
        .with_state(state)
        // Same middleware the real app wires as its outermost layer.
        .layer(axum::middleware::from_fn(security_headers_middleware))
}

async fn create_test_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&password_hash)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("insert user");
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
    let (status, _headers, val) = send_full(router, method, uri, token, body).await;
    (status, val)
}

async fn send_full(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, http::HeaderMap, Value) {
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
    let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, val)
}

/// Like `send_full`, but stamps the request with `ArrivedOverTls` — the same
/// tag the real HTTPS listener in `web::run` puts on TLS-terminated requests —
/// so middleware that keys off it (`strict-transport-security` injection) is
/// exercised on the TLS path.
async fn send_over_tls(
    router: &mut Router,
    method: Method,
    uri: &str,
) -> (StatusCode, http::HeaderMap, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .extension(ArrivedOverTls)
        .body(axum::body::Body::empty())
        .unwrap();
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
    let val: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, val)
}

// ---------------------------------------------------------------------------
// G7 — every progress endpoint requires a valid session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_jobs_rejects_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = progress_router(state);
    let (status, _) = send(&mut r, Method::GET, "/api/progress/jobs", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no token must be refused");
}

#[tokio::test]
async fn list_jobs_rejects_garbage_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = progress_router(state);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/progress/jobs",
        Some("not-a-real-session-token"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unrecognised token must be 401, not accepted"
    );
}

#[tokio::test]
async fn list_jobs_accepts_a_valid_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Sup3rSecret!!", 2).await;
    let mut r = progress_router(state);
    let token = login(&mut r, "alice", "Sup3rSecret!!").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/progress/jobs",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
}

#[tokio::test]
async fn sse_stream_rejects_garbage_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = progress_router(state);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/progress/00000000-0000-0000-0000-000000000000",
        Some("bogus"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn job_status_and_cancel_reject_garbage_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = progress_router(state);
    for (method, uri) in [
        (Method::GET, "/api/progress/deadbeef/status"),
        (Method::DELETE, "/api/progress/deadbeef"),
    ] {
        let (status, _) = send(&mut r, method.clone(), uri, Some("bogus"), None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must be 401"
        );
    }
}

// ---------------------------------------------------------------------------
// The status endpoint reports the job's real last event
// ---------------------------------------------------------------------------

/// `GET /api/progress/{job_id}/status` used to answer a hardcoded
/// `Running` / 0 % / `"Waiting..."` for any id present in the manager,
/// discarding the job's real last event
/// (`ProgressManager::get_progress`). Driven over HTTP because the handler —
/// not just the manager — is what a polling client talks to; the manager-side
/// behaviour is unit-tested in `src/sse/progress.rs`.
#[tokio::test]
async fn job_status_reports_the_real_last_event() {
    use homevault::sse::progress::{JobType, PROGRESS_MANAGER};

    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Sup3rSecret!!", 2).await;
    let mut r = progress_router(state);
    let token = login(&mut r, "alice", "Sup3rSecret!!").await;

    // A real tracked job, unique id (uuid), removed again before the test ends
    // so the process-wide singleton does not leak into the other tests here.
    let job_id = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("explorer_copy".to_string()),
        "Starting copy",
        Some("alice".to_string()),
    );
    PROGRESS_MANAGER.emit_progress(&job_id, 42.0, "42/100 files");

    let uri = format!("/api/progress/{job_id}/status");
    let (status, body) = send(&mut r, Method::GET, &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["job_id"], json!(job_id));
    assert_eq!(body["data"]["state"], json!("running"));
    assert_eq!(body["data"]["progress"], json!(42.0));
    assert_eq!(body["data"]["message"], json!("42/100 files"));

    // A finished job must read back as finished — the whole point of the fix.
    PROGRESS_MANAGER.complete_job(&job_id, "Copy finished");
    let (_, body) = send(&mut r, Method::GET, &uri, Some(&token), None).await;
    assert_eq!(body["data"]["state"], json!("completed"));
    assert_eq!(body["data"]["progress"], json!(100.0));
    assert_eq!(body["data"]["message"], json!("Copy finished"));

    PROGRESS_MANAGER.remove_job(&job_id);

    // An id nobody ever created is a clean error, never a phantom job.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/progress/00000000-0000-0000-0000-000000000000/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
}

// ---------------------------------------------------------------------------
// The SSE stream replays a terminal event that already happened
// ---------------------------------------------------------------------------

/// A job that fails **before** the client subscribes (a refused `sudo`, a
/// missing tool, an invalid device) used to be invisible: the client opened a
/// broadcast receiver after the terminal event had already been sent, so the
/// stream carried only heartbeats and a progress modal stayed on its bar
/// forever. The stream now replays the job's last known event first — the same
/// snapshot `GET /api/progress/{job_id}/status` serves — and closes when that
/// event is terminal.
#[tokio::test]
async fn sse_stream_replays_a_terminal_event_that_already_happened() {
    use homevault::sse::progress::{JobType, PROGRESS_MANAGER};

    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Sup3rSecret!!", 2).await;
    let mut r = progress_router(state);
    let token = login(&mut r, "alice", "Sup3rSecret!!").await;

    let job_id = PROGRESS_MANAGER.create_job_with_owner(
        JobType::Custom("encryption_create".to_string()),
        "Encrypting 'vault'",
        Some("alice".to_string()),
    );
    PROGRESS_MANAGER.fail_job(&job_id, "sudo: a password is required");

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/progress/{job_id}"))
        .header("x-session-token", &token)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // The stream must end on its own: a terminal job has nothing left to send.
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);

    assert!(
        body.contains("\"state\":\"failed\""),
        "the terminal state must be replayed: {body}"
    );
    assert!(
        body.contains("sudo: a password is required"),
        "the failure reason must reach the client: {body}"
    );
    assert!(
        body.contains("event: progress"),
        "a failed job is delivered as a progress event (the UI reads its message): {body}"
    );

    PROGRESS_MANAGER.remove_job(&job_id);
}

// ---------------------------------------------------------------------------
// G5 — response hardening headers on every response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn responses_carry_security_headers() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = progress_router(state);
    // Even an auth-refused response must be hardened.
    let (_status, headers, _) =
        send_full(&mut r, Method::GET, "/api/progress/jobs", None, None).await;

    let csp = headers
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        csp.contains("frame-ancestors 'none'"),
        "CSP must be present: {csp:?}"
    );
    assert!(csp.contains("default-src 'self'"));

    assert_eq!(
        headers.get("x-frame-options").and_then(|v| v.to_str().ok()),
        Some("DENY")
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    // HSTS is deliberate only for HTTPS: `security_headers_middleware`
    // injects `strict-transport-security` solely when the request was stamped
    // `ArrivedOverTls` by the real TLS listener (see `web::run`). Assert both
    // sides of that contract — absent on plain HTTP, present on the TLS path.
    assert!(
        headers.get("strict-transport-security").is_none(),
        "plain-HTTP responses must not advertise HSTS"
    );
    let (_tls_status, tls_headers, _) =
        send_over_tls(&mut r, Method::GET, "/api/progress/jobs").await;
    assert!(
        tls_headers.get("strict-transport-security").is_some(),
        "TLS-tagged responses must carry HSTS"
    );
    assert!(headers.get("referrer-policy").is_some());
    assert!(headers.get("permissions-policy").is_some());
}

// ---------------------------------------------------------------------------
// Cancelling a job runs its producer hook (2026-10-21)
// ---------------------------------------------------------------------------

/// `DELETE /api/progress/{job_id}` used to answer "Job cancelled" after merely
/// dropping the id from the tracking map: a `sudo ddrescue` clone, a PhotoRec
/// carve or a backup kept running with no way to stop it from the UI, while
/// the activity feed showed the job gone. The handler now runs the producer's
/// registered cancellation hook — which is what actually stops the work — and
/// answers honestly about which of the three cases happened.
#[tokio::test]
async fn cancel_job_runs_the_producer_hook_and_reports_what_happened() {
    use homevault::sse::progress::{JobType, PROGRESS_MANAGER};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "alice", "Sup3rSecret!!", 2).await;
    let mut r = progress_router(state);
    let token = login(&mut r, "alice", "Sup3rSecret!!").await;

    // A real job with a hook that records being called.
    let hooked = PROGRESS_MANAGER.create_job(JobType::Backup, "start");
    let fired = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&fired);
    PROGRESS_MANAGER.register_cancel_hook(
        &hooked,
        Box::new(move || flag.store(true, Ordering::SeqCst)),
    );

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/progress/{hooked}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["cancelled"], json!(true));
    assert!(
        fired.load(Ordering::SeqCst),
        "the producer's hook must actually run"
    );
    assert_eq!(
        PROGRESS_MANAGER.get_progress(&hooked).map(|e| e.state),
        Some(homevault::sse::progress::ProgressState::Cancelled),
        "the job must be reported as cancelled, not silently dropped"
    );
    PROGRESS_MANAGER.remove_job(&hooked);

    // A tracked job with no hook: nothing to stop beyond the tracking entry.
    let plain = PROGRESS_MANAGER.create_job(JobType::Backup, "start");
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/progress/{plain}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["cancelled"], json!(true));
    assert!(PROGRESS_MANAGER.get_progress(&plain).is_none());

    // An unknown id is a clean 400 — never a phantom "cancelled".
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/progress/00000000-0000-0000-0000-000000000000",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], json!(false));
}
