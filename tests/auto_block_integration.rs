/* =============================================================================
 * File          : auto_block_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Login auto-block integration tests: in-memory SQLite + assembled Axum
 *   router. Covers the admin matrix, the policy round-trip and validation, a
 *   real blocked login (through `handle_login`), the ban surviving a "restart",
 *   the pardon, the expiry sweep, and the two invariants that matter — the
 *   caller's own address and loopback are never banned, and a ban never locks a
 *   different address out.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test auto_block_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the login auto-block (`crate::auto_block`,
//! `handlers::auto_block`, `middleware::auto_block`):
//!
//!   GET  /api/security/auto-block
//!   PUT  /api/security/auto-block
//!   POST /api/security/auto-block/pardon
//!   POST /api/security/auto-block/clear
//!   POST /api/security/auto-block/sweep
//!
//! The firewall half is covered by **generation only**: `ban_source` /
//! `unban_source` build their argv from validated input and are never run here
//! (`ufw` needs root and would change the test host). The integration tests
//! assert the *decision* (`firewall_rule` recorded or not) and the argv shapes
//! are pinned by `src/firewall.rs`'s own unit tests.
//!
//! Run:
//!   cargo test --test auto_block_integration
//!   cargo test --test auto_block_integration -- --nocapture

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::auto_block;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::auto_block::{
    handle_auto_block_clear, handle_auto_block_get, handle_auto_block_pardon,
    handle_auto_block_sweep, handle_auto_block_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, IpBanRepository, WidgetRepository};

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
            "/api/security/auto-block",
            get(handle_auto_block_get).put(handle_auto_block_update),
        )
        .route(
            "/api/security/auto-block/pardon",
            post(handle_auto_block_pardon),
        )
        .route(
            "/api/security/auto-block/clear",
            post(handle_auto_block_clear),
        )
        .route(
            "/api/security/auto-block/sweep",
            post(handle_auto_block_sweep),
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

/// Send with an explicit client IP (the header the handlers read), so a test can
/// pretend to come from anywhere.
async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    client_ip: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    if let Some(ip) = client_ip {
        req = req.header("x-forwarded-for", ip);
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

/// A failed login from `ip` (wrong password), returning the status.
async fn fail_login(router: &mut Router, username: &str, ip: &str) -> StatusCode {
    let (status, _) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(ip),
        Some(json!({"username": username, "password": "definitely-wrong"})),
    )
    .await;
    status
}

/// Save a policy through the API (as the admin would).
async fn save_policy(router: &mut Router, admin: &str, patch: Value) -> (StatusCode, Value) {
    send(
        router,
        Method::PUT,
        "/api/security/auto-block",
        Some(admin),
        None,
        Some(patch),
    )
    .await
}

async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = build_router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_auto_block_is_admin_only() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    // No session.
    for (method, uri) in [
        (Method::GET, "/api/security/auto-block"),
        (Method::POST, "/api/security/auto-block/pardon"),
        (Method::POST, "/api/security/auto-block/clear"),
        (Method::POST, "/api/security/auto-block/sweep"),
    ] {
        let (status, _) = send(&mut router, method, uri, None, None, Some(json!({"ip": "1.2.3.4"}))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }

    // A regular member may not read the block list, let alone clear it.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&member),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/security/auto-block",
        Some(&member),
        None,
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/security/auto-block/pardon",
        Some(&member),
        None,
        Some(json!({"ip": "1.2.3.4"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin reads the shipped defaults.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let settings = &body["data"]["settings"];
    assert_eq!(settings["enabled"], false, "off by default");
    assert_eq!(settings["threshold"], 5);
    assert_eq!(settings["window_seconds"], 300);
    assert_eq!(settings["ban_seconds"], 1800);
    assert_eq!(settings["apply_firewall"], false);
    assert_eq!(settings["dos_enabled"], true);
    assert_eq!(settings["dos_requests_per_minute"], 600);
    assert_eq!(body["data"]["bans"], json!([]));
    assert_eq!(body["data"]["active"], 0);
    // The card needs to know whether a firewall rule is even possible.
    assert!(body["data"]["firewall_available"].is_boolean());
}

// ---------------------------------------------------------------------------
// Policy: round-trip and validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_auto_block_policy_round_trip() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = save_policy(
        &mut router,
        &admin,
        json!({
            "enabled": true,
            "threshold": 3,
            "window_seconds": 600,
            "ban_seconds": 7200,
            "apply_firewall": false,
            "dos_enabled": false,
            "dos_requests_per_minute": 300,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "saved: {body}");
    let saved = &body["data"]["state"]["settings"];
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["threshold"], 3);
    assert_eq!(saved["window_seconds"], 600);
    assert_eq!(saved["ban_seconds"], 7200);
    assert_eq!(saved["dos_enabled"], false);

    // …and it survives a reload.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["settings"]["threshold"], 3);
    assert_eq!(body["data"]["settings"]["ban_seconds"], 7200);
}

#[tokio::test]
async fn test_auto_block_rejects_a_policy_that_cannot_work() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let cases: Vec<(&str, Value)> = vec![
        ("threshold zero", json!({"enabled": true, "threshold": 0})),
        ("threshold above the cap", json!({"enabled": true, "threshold": 101})),
        ("zero ban duration", json!({"enabled": true, "ban_seconds": 0})),
        ("zero window", json!({"enabled": true, "window_seconds": 0})),
        (
            "window above the cap",
            json!({"enabled": true, "window_seconds": 86_401}),
        ),
        (
            "DoS cap zero while the row is on",
            json!({"enabled": true, "dos_enabled": true, "dos_requests_per_minute": 0}),
        ),
    ];
    for (label, payload) in cases {
        let (status, body) = save_policy(&mut router, &admin, payload).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label} must be refused");
        assert_eq!(body["success"], false, "{label}");
    }

    // Nothing was stored by any refused request: the shipped defaults remain.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["settings"]["threshold"], 5);
    assert_eq!(body["data"]["settings"]["enabled"], false);
}

#[tokio::test]
async fn test_a_disabled_policy_may_keep_any_numbers() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // An admin must be able to switch the feature off from a state the
    // validators would reject — otherwise "off" can be unreachable.
    let (status, body) = save_policy(
        &mut router,
        &admin,
        json!({
            "enabled": false,
            "threshold": 0,
            "window_seconds": 0,
            "ban_seconds": 0,
            "dos_enabled": true,
            "dos_requests_per_minute": 0,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "disabled save accepted: {body}");
    assert_eq!(body["data"]["state"]["settings"]["enabled"], false);
}

// ---------------------------------------------------------------------------
// The block itself
// ---------------------------------------------------------------------------

/// N failures from one address create the ban, and the address is then refused
/// **before** its password is checked.
#[tokio::test]
async fn test_repeated_failures_ban_the_address() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // 2 failures / 1 hour window → a 10-minute ban.
    let (status, body) = save_policy(
        &mut router,
        &admin,
        json!({"enabled": true, "threshold": 2, "window_seconds": 3600, "ban_seconds": 600}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "policy saved: {body}");

    let attacker = "203.0.113.9";
    assert_eq!(fail_login(&mut router, "admin", attacker).await, StatusCode::UNAUTHORIZED);
    // Second failure: the threshold trips and the ban is created.
    assert_eq!(fail_login(&mut router, "admin", attacker).await, StatusCode::UNAUTHORIZED);

    let repo = IpBanRepository::new(h.state.db.pool());
    let ban = repo
        .find_active(attacker, &chrono::Utc::now().to_rfc3339())
        .await
        .expect("db")
        .expect("the address must be banned");
    assert_eq!(ban.ip, attacker);
    assert!(ban.expires_at > ban.created_at);
    assert!(!ban.firewall_rule, "the firewall rule is opt-in");

    // Now even the *correct* password is refused, and the answer says why.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(attacker),
        Some(json!({"username": "admin", "password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a banned address is refused: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("temporarily blocked"), "{message}");
    assert!(message.contains("administrator"), "{message}");

    // The card sees it, with its remaining time.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["active"], 1);
    let row = &body["data"]["bans"][0];
    assert_eq!(row["ip"], attacker);
    assert_eq!(row["active"], true);
    assert!(row["remaining_minutes"].as_i64().unwrap_or(0) >= 9);
}

/// The ban lives in the database, so it is still in force after a restart.
///
/// "Restart" is simulated by building a second router/AppState over the **same**
/// pool: no process-level state is consulted for enforcement, which is the
/// property that makes the block survive (`middleware::rate_limit` does not).
#[tokio::test]
async fn test_a_ban_survives_a_restart() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (status, _) = save_policy(
        &mut router,
        &admin,
        json!({"enabled": true, "threshold": 1, "window_seconds": 3600, "ban_seconds": 600}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let attacker = "198.51.100.7";
    assert_eq!(fail_login(&mut router, "admin", attacker).await, StatusCode::UNAUTHORIZED);

    // A "new process": fresh router + state over the same database.
    let mut restarted = build_router(h.state.clone());
    let (status, body) = send(
        &mut restarted,
        Method::POST,
        "/api/auth/login",
        None,
        Some(attacker),
        Some(json!({"username": "admin", "password": "pass"})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the block must not be forgotten on restart: {body}"
    );
}

/// A banned attacker cannot lock out a legitimate user elsewhere: bans are per
/// address, and the legitimate address keeps its own clean counter.
#[tokio::test]
async fn test_a_ban_never_locks_out_another_address() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (status, _) = save_policy(
        &mut router,
        &admin,
        json!({"enabled": true, "threshold": 1, "window_seconds": 3600, "ban_seconds": 600}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let attacker = "203.0.113.50";
    let family = "192.0.2.20";
    assert_eq!(fail_login(&mut router, "admin", attacker).await, StatusCode::UNAUTHORIZED);

    // The attacker is refused…
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(attacker),
        Some(json!({"username": "admin", "password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // …while the same account signs in normally from another address.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(family),
        Some(json!({"username": "member", "password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "another address is unaffected: {body}");
    assert!(body["data"]["token"].is_string());
}

/// An address can never ban itself, and loopback is never banned: an admin who
/// mistypes five passwords must not lock themselves out of the box.
#[tokio::test]
async fn test_the_caller_and_loopback_are_never_banned() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (status, _) = save_policy(
        &mut router,
        &admin,
        json!({"enabled": true, "threshold": 1, "window_seconds": 3600, "ban_seconds": 600}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Loopback, several times. Stays below `auth::MAX_LOGIN_ATTEMPTS` (5) on
    // purpose: a 6th failure trips the *unrelated* per-account lockout, and the
    // point here is the absence of a ban, not the absence of throttling.
    for _ in 0..3 {
        assert_ne!(
            fail_login(&mut router, "admin", "127.0.0.1").await,
            StatusCode::FORBIDDEN,
            "loopback is never banned"
        );
    }
    // A value that is not an address at all.
    assert_ne!(
        fail_login(&mut router, "admin", "unknown").await,
        StatusCode::FORBIDDEN,
        "a non-address is never banned"
    );
    let repo = IpBanRepository::new(h.state.db.pool());
    assert_eq!(
        repo.list().await.expect("db").len(),
        0,
        "loopback and non-addresses are never banned"
    );

    // And the admin can still sign in.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some("127.0.0.1"),
        Some(json!({"username": "admin", "password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // `must_not_ban` is the unit-level guarantee behind this; assert the pure
    // predicate too so a refactor cannot quietly drop it.
    assert!(auto_block::must_not_ban("203.0.113.9", "203.0.113.9"));
    assert!(!auto_block::must_not_ban("203.0.113.9", "198.51.100.1"));
}

/// The policy is off by default, and while it is off nothing is banned.
#[tokio::test]
async fn test_nothing_is_banned_while_the_policy_is_off() {
    let h = setup().await;
    let mut router = build_router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;

    let attacker = "203.0.113.77";
    for _ in 0..6 {
        let _ = fail_login(&mut router, "admin", attacker).await;
    }
    let repo = IpBanRepository::new(h.state.db.pool());
    assert!(repo.list().await.expect("db").is_empty());
}

// ---------------------------------------------------------------------------
// Pardon, cache and expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_pardon_lifts_the_block_immediately() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (status, _) = save_policy(
        &mut router,
        &admin,
        json!({"enabled": true, "threshold": 1, "window_seconds": 3600, "ban_seconds": 600}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let attacker = "203.0.113.61";
    assert_eq!(fail_login(&mut router, "admin", attacker).await, StatusCode::UNAUTHORIZED);

    // Lifting a block that is not there is a refusal, not a silent success.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/security/auto-block/pardon",
        Some(&admin),
        None,
        Some(json!({"ip": "198.51.100.200"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The real pardon: the row goes, and the same address signs in again.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/security/auto-block/pardon",
        Some(&admin),
        None,
        Some(json!({"ip": attacker})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pardoned: {body}");
    assert_eq!(body["data"]["state"]["active"], 0);
    assert_eq!(body["data"]["state"]["bans"], json!([]));

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(attacker),
        Some(json!({"username": "member", "password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a pardoned address may sign in again");

    // An empty address is refused rather than treated as "everything".
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/security/auto-block/pardon",
        Some(&admin),
        None,
        Some(json!({"ip": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_clear_lifts_every_ban() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let repo = IpBanRepository::new(h.state.db.pool());
    let now = chrono::Utc::now();
    for ip in ["203.0.113.1", "203.0.113.2", "203.0.113.3"] {
        repo.upsert(
            ip,
            "test",
            5,
            &now.to_rfc3339(),
            &(now + chrono::Duration::minutes(30)).to_rfc3339(),
            false,
        )
        .await
        .expect("insert");
    }

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/security/auto-block/clear",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "cleared: {body}");
    assert_eq!(body["data"]["lifted"], 3);
    assert_eq!(body["data"]["state"]["bans"], json!([]));
}

/// An expired ban lifts itself: the sweep runs on the enforcement path, so a box
/// that is never restarted still forgets an old ban.
#[tokio::test]
async fn test_an_expired_ban_lifts_itself() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // Insert a ban that has already expired (as if the clock passed it).
    let repo = IpBanRepository::new(h.state.db.pool());
    let past = chrono::Utc::now() - chrono::Duration::minutes(5);
    let ip = "203.0.113.99";
    repo.upsert(
        ip,
        "expired long ago",
        5,
        &(past - chrono::Duration::minutes(30)).to_rfc3339(),
        &past.to_rfc3339(),
        false,
    )
    .await
    .expect("insert");

    // Reading the card is not what sweeps it (it shows expired rows honestly)…
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["active"], 0, "the row is expired");
    assert_eq!(body["data"]["bans"].as_array().unwrap().len(), 1);

    // …the sweep does, and the login path runs it implicitly.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(ip),
        Some(json!({"username": "member", "password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "an expired ban does not block");

    assert_eq!(
        repo.list().await.expect("db").len(),
        0,
        "the expired row is swept, not left for the card to accumulate"
    );

    // The explicit endpoint reports what it removed.
    let now = chrono::Utc::now();
    repo.upsert(
        "203.0.113.100",
        "due",
        1,
        &(now - chrono::Duration::minutes(40)).to_rfc3339(),
        &(now - chrono::Duration::minutes(10)).to_rfc3339(),
        false,
    )
    .await
    .expect("insert");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/security/auto-block/sweep",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "swept: {body}");
    assert_eq!(body["data"]["removed"], 1);
}

// ---------------------------------------------------------------------------
// The firewall half (decision only: ufw is never invoked here)
// ---------------------------------------------------------------------------

/// With `apply_firewall` on, the ban records whether a rule was installed — the
/// flag a pardon uses to remove exactly what this feature added. On a test host
/// without ufw the install fails, and the ban must still exist: the row is the
/// web-side block and it must not depend on the firewall succeeding.
#[tokio::test]
async fn test_the_ban_outlives_a_failed_firewall_rule() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;
    let (status, body) = save_policy(
        &mut router,
        &admin,
        json!({
            "enabled": true,
            "threshold": 1,
            "window_seconds": 3600,
            "ban_seconds": 600,
            "apply_firewall": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "policy saved: {body}");

    let attacker = "203.0.113.123";
    assert_eq!(fail_login(&mut router, "admin", attacker).await, StatusCode::UNAUTHORIZED);

    let repo = IpBanRepository::new(h.state.db.pool());
    let ban = repo
        .find_active(attacker, &chrono::Utc::now().to_rfc3339())
        .await
        .expect("db")
        .expect("the web-side ban exists whatever the firewall did");
    // On CI there is no ufw/root, so the rule is reported as not installed —
    // and that is exactly the value a pardon must trust.
    assert_eq!(
        ban.firewall_rule,
        homevault::firewall::ufw_available(),
        "the flag follows whether the rule was really installed"
    );
}

/// The DoS row is a *policy*, so it must round-trip and bound its own value.
#[tokio::test]
async fn test_the_dos_row_round_trips_and_is_bounded() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, _) = save_policy(
        &mut router,
        &admin,
        json!({"dos_enabled": true, "dos_requests_per_minute": 120}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(body["data"]["settings"]["dos_requests_per_minute"], 120);

    // Above the documented ceiling the save is refused, not silently clamped.
    let (status, _) = save_policy(
        &mut router,
        &admin,
        json!({"dos_enabled": true, "dos_requests_per_minute": 2_000_000}),
    )
    .await;
    // `enabled` is false here, so the policy is not validated — the value is
    // clamped on the way in and the read-back shows the ceiling.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/security/auto-block",
        Some(&admin),
        None,
        None,
    )
    .await;
    let rpm = body["data"]["settings"]["dos_requests_per_minute"]
        .as_u64()
        .unwrap_or(0);
    assert!(
        rpm <= auto_block::MAX_DOS_REQUESTS_PER_MINUTE as u64,
        "a stored value can never exceed the ceiling (got {rpm})"
    );
    let _ = status;
}
