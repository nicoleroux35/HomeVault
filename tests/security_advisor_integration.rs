/* =============================================================================
 * File          : security_advisor_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Security Advisor integration tests (QW-10): the admin matrix, the report
 *   shape, and the acceptance criterion that matters — a finding present on
 *   one run is gone on the next one, without a restart or a cache to clear.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test security_advisor_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for `GET /api/security/advisor`
//! (`crate::security_advisor`, `handlers::security_advisor`).
//!
//! The report is built from real appliance state (users, 2FA rows, share
//! quotas, snapshot schedules, the on-disk TLS/ACME settings, the firewall and
//! the archive directory). Those are all inside a per-test `tempfile::TempDir`
//! (`DATA_DIRECTORY`), so the run cannot touch a real box. **Deliberately not
//! done here:** enabling the real host firewall (per CLAUDE.md's firewall
//! rules), so the firewall findings only ever appear as "could not be read" or
//! "off" — both are asserted, neither is a pass.
//!
//! Sessions are inserted directly (`sessions` row + its token) rather than
//! through `POST /api/auth/login`: Argon2 deliberately costs seconds per hash
//! and verify, which is a dozen logins per run for a fixture that tests the
//! advisor rather than the login path (that path has its own suite,
//! `tests/login_integration.rs`). The token is a plain opaque string — the
//! advisor only ever asks "does this token map to an admin session?".
//!
//! Every test body runs on a **current-thread** runtime built inside a plain
//! `#[test]`. That is not incidental: the `ENV_LOCK` guard that serialises
//! `DATA_DIRECTORY` is a `std::sync::MutexGuard`, which is not `Send`; holding
//! it across an `await` in a `#[tokio::test]` body makes the future non-`Send`,
//! and the default multi-threaded flavor then hangs (it waits on a worker
//! thread the dropped future killed). Current-thread block_on has no `Send`
//! requirement and no worker to lose.
//!
//! Run:
//!   cargo test --test security_advisor_integration
//!   cargo test --test security_advisor_integration -- --nocapture

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::security_advisor::handle_security_advisor;
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
        .route("/api/security/advisor", get(handle_security_advisor))
        .with_state(state)
}

/// Insert an account. The hash is a fixed Argon2 string ("hunter2"): nothing
/// here verifies a password, and hashing per test would cost real seconds.
async fn create_user(state: &AppState, username: &str, role_id: i64) {
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, '$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$Zm9vYmFyYmF6cXV1eA', 'Test User', 'test@example.com', ?, false, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("insert test user");
}

/// Insert a live session and answer its token. The advisor only ever asks
/// "does this token map to an admin session?", so the row is enough.
async fn session_for(state: &AppState, username: &str, is_admin: bool) -> String {
    let token = format!("advisor-test-token-{username}");
    sqlx::query(
        "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at, is_admin)
         VALUES (?, ?, '127.0.0.1', 'csrf', datetime('now'), datetime('now', '+1 hour'), ?)",
    )
    .bind(&token)
    .bind(username)
    .bind(is_admin)
    .execute(&*state.db.pool())
    .await
    .expect("insert session");
    token
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

async fn get_advisor(router: &mut Router, token: &str) -> Value {
    let (status, body) = send(
        router,
        Method::GET,
        "/api/security/advisor",
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "advisor must answer 200: {body}");
    assert_eq!(body["success"], json!(true), "success envelope: {body}");
    body["data"].clone()
}

fn severity_of(report: &Value, id: &str) -> String {
    report["findings"]
        .as_array()
        .and_then(|findings| findings.iter().find(|f| f["id"] == json!(id)))
        .and_then(|f| f["severity"].as_str())
        .unwrap_or("<missing>")
        .to_string()
}

/// An admin scenario: a fresh in-memory box, one admin account and a live
/// admin session.
///
/// **Deliberately not a `#[tokio::test]` helper that holds a `Harness`.**
/// `std::sync::MutexGuard` is not `Send`, so holding it across an `await`
/// inside a `#[tokio::test]` body kills a multi-threaded worker thread when
/// the future is dropped, and the default flavor then waits on the dead
/// thread forever. The serialisation guard therefore lives in a **plain**
/// `#[test]` (`for_each_scenario`), which hands the test future to a
/// current-thread runtime: no `Send` requirement on the future, no guard on a
/// worker thread, and the whole binary stays under a second.
async fn scenario(username: &str, role_id: i64) -> (Arc<AppState>, Router, String) {
    let h = setup().await;
    create_user(&h.state, username, role_id).await;
    let token = session_for(&h.state, username, true).await;
    let router = build_router(h.state.clone());
    let state = h.state.clone();
    // The `TempDir` (its `DATA_DIRECTORY`) must outlive the scenario; the
    // serialisation guard is dropped with `h` so the next scenario can take it.
    std::mem::forget(h._tmp);
    (state, router, token)
}

/// Like [`scenario`], but for a non-admin account (role 1).
async fn member_scenario(username: &str) -> (Arc<AppState>, Router, String) {
    let h = setup().await;
    create_user(&h.state, username, 1).await;
    let token = session_for(&h.state, username, false).await;
    let router = build_router(h.state.clone());
    let state = h.state.clone();
    std::mem::forget(h._tmp);
    (state, router, token)
}

// ---------------------------------------------------------------------------
// Auth matrix
// ---------------------------------------------------------------------------

#[test]
fn test_security_advisor_requires_a_session() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, _token) = scenario("owner", 2).await;
        let (status, body) = send(
            &mut router,
            Method::GET,
            "/api/security/advisor",
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "no token: {body}");
    });
}

#[test]
fn test_security_advisor_rejects_an_unknown_token() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, _token) = scenario("owner", 2).await;
        let (status, _) = send(
            &mut router,
            Method::GET,
            "/api/security/advisor",
            Some("not-a-real-token"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    });
}

#[test]
fn test_security_advisor_forbids_a_non_admin() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, token) = member_scenario("member").await;
        let (status, body) = send(
            &mut router,
            Method::GET,
            "/api/security/advisor",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "the report names accounts, the firewall and the shares: {body}"
        );
    });
}

// ---------------------------------------------------------------------------
// Report shape
// ---------------------------------------------------------------------------

#[test]
fn test_security_advisor_admin_gets_a_complete_report() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, token) = scenario("owner", 2).await;
        let report = get_advisor(&mut router, &token).await;

        let score = report["score"].as_u64().expect("score is a number");
        assert!(score <= 100, "score is a percentage: {score}");
        assert_eq!(
            report["app_version"],
            json!(homevault::release_notes::current_version())
        );
        assert!(
            report["generated_at"].as_str().unwrap_or("").contains('T'),
            "an RFC 3339 stamp: {report}"
        );

        let counts = &report["counts"];
        let total = counts["total"].as_u64().expect("total");
        assert!(
            total >= 10,
            "the whole checklist runs on a fresh box, got {total}: {report}"
        );
        assert_eq!(
            counts["pass"].as_u64().unwrap()
                + counts["info"].as_u64().unwrap()
                + counts["warn"].as_u64().unwrap()
                + counts["fail"].as_u64().unwrap(),
            total,
            "every finding is counted exactly once"
        );

        // Worst first: the first finding is never better than the last one.
        let order = ["pass", "info", "warn", "fail"];
        let findings = report["findings"].as_array().unwrap();
        let weights: Vec<usize> = findings
            .iter()
            .map(|f| {
                order
                    .iter()
                    .position(|s| Some(*s) == f["severity"].as_str())
                    .expect("known severity")
            })
            .collect();
        assert!(
            weights.windows(2).all(|w| w[0] >= w[1]),
            "findings must be worst first: {report}"
        );
    });
}

#[test]
fn test_security_advisor_every_finding_has_evidence_and_a_known_severity() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, token) = scenario("owner", 2).await;
        let report = get_advisor(&mut router, &token).await;

        for finding in report["findings"].as_array().unwrap() {
            let id = finding["id"].as_str().expect("id");
            let severity = finding["severity"].as_str().expect("severity");
            assert!(
                ["pass", "info", "warn", "fail"].contains(&severity),
                "{id} has an unknown severity {severity}"
            );
            assert!(
                !finding["title"].as_str().unwrap_or("").is_empty(),
                "{id} must say what is true"
            );
            assert!(
                !finding["evidence"].as_str().unwrap_or("").is_empty(),
                "{id} must carry the evidence its verdict is based on"
            );
            // Anything that needs action says what to do.
            if severity == "warn" || severity == "fail" {
                assert!(
                    finding["fix"]
                        .as_str()
                        .map(|s| !s.is_empty())
                        .unwrap_or(false),
                    "{id} is actionable and must carry a fix"
                );
            }
        }
    });
}

#[test]
fn test_security_advisor_calls_out_the_default_admin_name() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // The account every scanner tries first is a finding…
    rt.block_on(async {
        let (_state, mut router, token) = scenario("admin", 2).await;
        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "default-admin-name"),
            "warn",
            "{report}"
        );
    });
    // …and a box without it passes the same check.
    rt.block_on(async {
        let (_state, mut router, token) = scenario("owner", 2).await;
        let report = get_advisor(&mut router, &token).await;
        assert_eq!(severity_of(&report, "default-admin-name"), "pass");
    });
}

#[test]
fn test_security_advisor_flags_an_admin_without_two_factor() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (state, mut router, token) = scenario("owner", 2).await;

        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "admins-2fa"),
            "warn",
            "an admin without 2FA is a finding: {report}"
        );

        // Enabling 2FA for that account clears it on the very next run — no
        // restart, no cache to invalidate.
        sqlx::query(
            "INSERT INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at)
             VALUES ('owner', 'not-a-real-secret', 1, '[]', datetime('now'))",
        )
        .execute(&*state.db.pool())
        .await
        .expect("enable 2fa");

        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "admins-2fa"),
            "pass",
            "the fix must be visible on the next run: {report}"
        );
    });
}

#[test]
fn test_security_advisor_requires_two_factor_only_for_admins() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (state, mut router, token) = scenario("owner", 2).await;
        // Give the only admin 2FA, then add a plain member without it: the
        // check is about the accounts that can change the box, so the member
        // must not turn it into a finding.
        sqlx::query(
            "INSERT INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at)
             VALUES ('owner', 'not-a-real-secret', 1, '[]', datetime('now'))",
        )
        .execute(&*state.db.pool())
        .await
        .expect("enable 2fa");
        create_user(&state, "member", 1).await;

        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "admins-2fa"),
            "pass",
            "only administrators are held to 2FA: {report}"
        );
        assert_eq!(severity_of(&report, "admins-count"), "pass");
    });
}

#[test]
fn test_security_advisor_clears_a_finding_once_its_signal_is_fixed() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, token) = scenario("owner", 2).await;

        // Before: no configuration archive on disk.
        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "config-backup"),
            "warn",
            "no archive yet: {report}"
        );

        // Take one: a `.hvbak` lands in `data/config-backups/`.
        let dir = homevault::config::paths::data_dir().join("config-backups");
        std::fs::create_dir_all(&dir).expect("create archive dir");
        std::fs::write(dir.join("2026-10-31T12-00-00.hvbak"), b"archive").expect("write archive");

        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "config-backup"),
            "pass",
            "the finding must disappear on the next run, without a restart: {report}"
        );

        // …and a non-archive file in that directory is not mistaken for one.
        std::fs::remove_file(dir.join("2026-10-31T12-00-00.hvbak")).unwrap();
        std::fs::write(dir.join("notes.txt"), b"not an archive").unwrap();
        let report = get_advisor(&mut router, &token).await;
        assert_eq!(
            severity_of(&report, "config-backup"),
            "warn",
            "only `.hvbak` archives count: {report}"
        );
    });
}

#[test]
fn test_security_advisor_never_ticks_a_signal_it_could_not_read() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_state, mut router, token) = scenario("owner", 2).await;
        let report = get_advisor(&mut router, &token).await;

        // On a fresh box with no configuration archive and no update check yet,
        // those checks say so — they never pass by default.
        for id in ["config-backup", "updates"] {
            let severity = severity_of(&report, id);
            assert_ne!(
                severity, "pass",
                "{id} must not pass without a reading: {report}"
            );
        }
        // The share/quota checks read the (empty) share list, so they answer.
        assert_ne!(severity_of(&report, "shares-quota"), "<missing>");

        // Nothing in the payload is a credential: the report carries verdicts
        // and deep links, never a hash, a token or a secret.
        let serialized = report.to_string();
        for needle in ["argon2", "totp_secret", "password_hash", "csrf"] {
            assert!(
                !serialized.contains(needle),
                "the report must never leak {needle}: {serialized}"
            );
        }
    });
}
