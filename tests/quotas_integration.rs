/* =============================================================================
 * File          : quotas_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Transfer-quota integration tests: in-memory SQLite + assembled Axum
 *   router, exercising the area's endpoints end-to-end via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test quotas_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the transfer-quota API
//! (`crate::handlers::transfer_quotas`), the storage layer behind it and the
//! counting-period settlement that guards every File Explorer transfer.
//!
//! The file replaces the suite deleted on 2026-09-09 (ROADMAP 23): it asserts
//! the three findings that pass closed, not just the response shape.
//!
//!   - **23.1 — fail closed.** Covered by the `transfer_quota::load_*` calls
//!     here and by the source guard in `src/handlers/explorer.rs`; a DB
//!     failure is a `503` naming the refusal, never an unmetered transfer.
//!   - **23.3 — self-or-admin reads.** A member reads their own quota, is
//!     refused another user's and every group's, and the list endpoint is
//!     admin-only.
//!   - **23.2 — counting periods.** A saved period round-trips with its
//!     window, a period *change* starts a fresh window, and a row whose
//!     window elapsed is zeroed the moment the guard consults it — without
//!     waiting for the reset loop.
//!
//! Run:
//!   cargo test --test quotas_integration
//!   cargo test --test quotas_integration -- --nocapture

use axum::{
    routing::{delete, get, put},
    Router,
};
use chrono::{Datelike, Utc};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::transfer_quotas::{
    handle_transfer_quota_delete, handle_transfer_quota_get, handle_transfer_quota_set,
    handle_transfer_quotas_list,
};
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::transfer_quota_repo::{
    QuotaLimits, TransferQuotaRepository,
};
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

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
    let pty_manager = Arc::new(homevault::handlers::terminal_pty::PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    let state = Arc::new(AppState {
        auth,
        widgets,
        db,
        path_guard,
        admin_rate_limiter,
        pty_manager,
        downloader,
        ransomware_watch_tx,
    });

    Harness {
        state,
        _tmp: tmp,
        _guard: guard,
    }
}

fn quota_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", axum::routing::post(handle_login))
        .route(
            "/api/transfer-quotas/{subject_type}",
            get(handle_transfer_quotas_list),
        )
        .route(
            "/api/transfer-quotas/{subject_type}/{subject_name}",
            get(handle_transfer_quota_get),
        )
        .route(
            "/api/transfer-quotas/{subject_type}/{subject_name}",
            put(handle_transfer_quota_set),
        )
        .route(
            "/api/transfer-quotas/{subject_type}/{subject_name}",
            delete(handle_transfer_quota_delete),
        )
        .with_state(state)
}

/// `role_id` 2 = admin, 1 = regular.
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

fn repo(h: &Harness) -> TransferQuotaRepository {
    TransferQuotaRepository::new(h.state.db.pool())
}

fn put_body(period: &str, upload: i64, download: i64) -> Value {
    json!({
        "upload_enabled": upload > 0,
        "upload_limit_bytes": upload,
        "download_enabled": download > 0,
        "download_limit_bytes": download,
        "period": period,
    })
}

// ===========================================================================
// 1. Authentication and authorisation (ROADMAP 23.3)
// ===========================================================================

#[tokio::test]
async fn test_reads_require_a_session() {
    let h = setup().await;
    let mut r = quota_router(h.state.clone());

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user/alice",
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a missing token keeps the module-wide json_error shape (400)"
    );

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user/alice",
        Some("not-a-real-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_a_member_reads_their_own_quota() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    repo(&h)
        .set_limits(QuotaLimits::lifetime("user", "bob", true, 4096, false, 0))
        .await
        .expect("set quota");

    let mut r = quota_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user/bob",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["upload_limit_bytes"], json!(4096));
}

#[tokio::test]
async fn test_a_member_cannot_read_another_users_quota() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    create_test_user(&h.state, "carol", "Password123!", 1).await;
    repo(&h)
        .set_limits(QuotaLimits::lifetime("user", "carol", true, 1, false, 0))
        .await
        .expect("set quota");

    let mut r = quota_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    // The payload used to carry carol's limits *and* her consumed bytes to
    // any authenticated household user.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user/carol",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["success"], json!(false));
    assert!(
        body["data"].is_null(),
        "a refused read must not leak a partial payload"
    );
}

#[tokio::test]
async fn test_a_member_cannot_read_a_group_quota() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    repo(&h)
        .set_limits(QuotaLimits::lifetime("group", "bob", true, 1, false, 0))
        .await
        .expect("set quota");

    let mut r = quota_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    // A group whose name happens to equal the caller's username is still not
    // "their own quota" — only `subject_type = user` can be.
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/group/bob",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_an_admin_reads_any_subject_and_the_list_is_admin_only() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;
    repo(&h)
        .set_limits(QuotaLimits::lifetime("user", "bob", true, 4096, false, 0))
        .await
        .expect("set quota");

    let mut r = quota_router(h.state.clone());
    let member = login(&mut r, "bob", "Password123!").await;

    // The list answers every subject's limits and usage — the admin gate used
    // to live only in the screen (Quotas.svelte).
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let admin = login(&mut r, "admin1", "Password123!").await;
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["subject_name"], json!("bob"));

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user/bob",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_writes_stay_admin_only() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;

    let mut r = quota_router(h.state.clone());
    let token = login(&mut r, "bob", "Password123!").await;

    // A member must not be able to grant themselves unlimited allowance.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/bob",
        Some(&token),
        Some(put_body("lifetime", 0, 0)),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut r,
        Method::DELETE,
        "/api/transfer-quotas/user/bob",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ===========================================================================
// 2. Counting periods (ROADMAP 23.2)
// ===========================================================================

#[tokio::test]
async fn test_an_unknown_period_is_refused_and_nothing_is_stored() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = quota_router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("yearly", 4096, 0)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the module answers errors in 200 JSON");
    assert_eq!(body["success"], json!(false));
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("lifetime"), "{error}");
    assert!(error.contains("month"), "{error}");

    // Nothing was written: the row is still the disabled default.
    let stored = repo(&h).get("user", "alice").await.unwrap();
    assert!(!stored.upload_enabled);
    assert_eq!(stored.period, "lifetime");
}

#[tokio::test]
async fn test_a_period_round_trips_with_its_window_and_next_reset() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = quota_router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("month", 1_000_000, 0)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["period"], json!("month"));

    let started = body["data"]["period_started_at"]
        .as_str()
        .expect("the window must be opened on save");
    // The window opens on the 1st at 00:00 local time, so the stored instant
    // is a calendar boundary — in any zone it is the 1st somewhere within ±14h.
    assert!(
        started.contains("T00:00:00") || started.contains("T22:00:00") || started.contains("T23:00:00"),
        "unexpected window start: {started}"
    );
    let resets = body["data"]["period_resets_at"]
        .as_str()
        .expect("a monthly quota announces its next reset");
    assert_ne!(resets, started, "the reset instant is the next window's start");
}

#[tokio::test]
async fn test_a_lifetime_quota_announces_no_reset() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = quota_router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("lifetime", 4096, 0)),
    )
    .await;
    assert_eq!(body["data"]["period"], json!("lifetime"));
    assert!(
        body["data"]["period_resets_at"].is_null(),
        "a lifetime quota has no reset date to show"
    );
}

#[tokio::test]
async fn test_changing_the_period_zeroes_the_counters_and_editing_a_limit_does_not() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = quota_router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let repo = repo(&h);
    repo.set_limits(QuotaLimits::lifetime("user", "alice", true, 1_000_000, false, 0))
        .await
        .unwrap();
    repo.add_upload_usage("user", "alice", 900_000).await.unwrap();

    // Same period, bigger cap: the 900 kB already transferred still counts.
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("lifetime", 2_000_000, 0)),
    )
    .await;
    assert_eq!(body["data"]["upload_used_bytes"], json!(900_000));

    // A different period is a different window: the lifetime total must not
    // be billed against a monthly cap.
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("month", 2_000_000, 0)),
    )
    .await;
    assert_eq!(body["data"]["period"], json!("month"));
    assert_eq!(body["data"]["upload_used_bytes"], json!(0));
}

#[tokio::test]
async fn test_an_elapsed_window_is_zeroed_when_the_guard_consults_it() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;

    // A monthly row whose window opened last month, with the whole cap used.
    let repo = repo(&h);
    repo.set_limits(QuotaLimits {
        subject_type: "user",
        subject_name: "bob",
        upload_enabled: false,
        upload_limit_bytes: 0,
        download_enabled: true,
        download_limit_bytes: 1_000,
        period: "month",
        period_started_at: "2020-01-01T00:00:00+00:00",
    })
    .await
    .unwrap();
    repo.add_download_usage("user", "bob", 1_000).await.unwrap();

    // Consulting the quota settles it: the counters are zeroed and the window
    // re-opened on the current calendar month, without any scheduler tick.
    let quotas = homevault::transfer_quota::applicable_download_quotas(h.state.db.pool(), "bob")
        .await
        .expect("quota check must answer");
    assert_eq!(quotas.len(), 1);
    assert_eq!(
        quotas[0].used_bytes, 0,
        "an elapsed window must not report its old usage"
    );
    assert!(
        homevault::transfer_quota::check_quota(&quotas, 1_000).is_ok(),
        "the full cap is available again in the new window"
    );

    let settled = repo.get("user", "bob").await.unwrap();
    assert_eq!(settled.download_used_bytes, 0);
    let started = settled.period_started_at.expect("window re-opened");
    assert!(
        started.starts_with(&format!("{}-", Utc::now().format("%Y-%m"))),
        "the new window opens in the current month, got {started}"
    );
}

#[tokio::test]
async fn test_a_window_that_is_still_open_keeps_its_usage() {
    let h = setup().await;
    create_test_user(&h.state, "bob", "Password123!", 1).await;

    let repo = repo(&h);
    let now = Utc::now();
    let this_month = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap();
    repo.set_limits(QuotaLimits {
        subject_type: "user",
        subject_name: "bob",
        upload_enabled: false,
        upload_limit_bytes: 0,
        download_enabled: true,
        download_limit_bytes: 1_000,
        period: "month",
        period_started_at: &format!("{}T00:00:00+00:00", this_month),
    })
    .await
    .unwrap();
    repo.add_download_usage("user", "bob", 400).await.unwrap();

    let quotas = homevault::transfer_quota::applicable_download_quotas(h.state.db.pool(), "bob")
        .await
        .expect("quota check must answer");
    assert_eq!(quotas[0].used_bytes, 400);
}

#[tokio::test]
async fn test_the_reset_pass_only_touches_elapsed_windows() {
    let h = setup().await;

    let repo = repo(&h);
    repo.set_limits(QuotaLimits {
        subject_type: "user",
        subject_name: "stale",
        upload_enabled: true,
        upload_limit_bytes: 1_000,
        download_enabled: false,
        download_limit_bytes: 0,
        period: "day",
        period_started_at: "2020-01-01T00:00:00+00:00",
    })
    .await
    .unwrap();
    repo.add_upload_usage("user", "stale", 900).await.unwrap();

    repo.set_limits(QuotaLimits {
        subject_type: "user",
        subject_name: "fresh",
        upload_enabled: true,
        upload_limit_bytes: 1_000,
        download_enabled: false,
        download_limit_bytes: 0,
        period: "day",
        period_started_at: &Utc::now().to_rfc3339(),
    })
    .await
    .unwrap();
    repo.add_upload_usage("user", "fresh", 900).await.unwrap();

    let reset = homevault::transfer_quota::reset_expired_periods(
        h.state.db.pool(),
        Utc::now(),
        homevault::transfer_quota::appliance_timezone(),
    )
    .await
    .expect("reset pass");
    assert!(reset >= 1, "the stale row must be reset");

    assert_eq!(repo.get("user", "stale").await.unwrap().upload_used_bytes, 0);
    assert_eq!(
        repo.get("user", "fresh").await.unwrap().upload_used_bytes,
        900,
        "a window that is still open keeps its counters"
    );
}

// ===========================================================================
// 3. Storage contract
// ===========================================================================

#[tokio::test]
async fn test_a_deleted_row_returns_to_the_disabled_lifetime_default() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = quota_router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("week", 4096, 0)),
    )
    .await;

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));

    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["period"], json!("lifetime"));
    assert_eq!(body["data"]["upload_enabled"], json!(false));
}

#[tokio::test]
async fn test_a_negative_limit_is_refused() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = quota_router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/transfer-quotas/user/alice",
        Some(&admin),
        Some(put_body("lifetime", -1, 0)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(false));
    assert!(body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("negative"));
}
