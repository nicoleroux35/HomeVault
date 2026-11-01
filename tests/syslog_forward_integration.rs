/* =============================================================================
 * File          : syslog_forward_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026
 * Last modified : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Remote syslog forwarding integration tests: in-memory SQLite + assembled
 *   Axum router for the admin matrix and the validation paths, and a real
 *   localhost UDP/TCP collector for the bytes that actually leave the box.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test syslog_forward_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for remote syslog forwarding
//! (`crate::syslog_forward`, `crate::handlers::syslog`):
//!
//!   GET  /api/settings/syslog       read the configuration + queue counters
//!   PUT  /api/settings/syslog       save (validated, CA required for TLS)
//!   POST /api/settings/syslog/test  send one message now
//!
//! Every test runs against a fresh in-memory database and a fresh
//! `DATA_DIRECTORY`, because the settings live in `data/syslog_forward.json`.
//! The queue and the `enabled` cache are process-wide, so the whole file is
//! serialized on `ENV_LOCK`: an assertion about "how many events are queued"
//! would otherwise see another test's audit lines.
//!
//! Run:
//!   cargo test --test syslog_forward_integration
//!   cargo test --test syslog_forward_integration -- --nocapture

use axum::{
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::syslog::{
    handle_syslog_settings_get, handle_syslog_settings_update, handle_syslog_test,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};
use homevault::syslog_forward;

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

    // Every test starts from "forwarding off", with an empty queue: both are
    // process-wide and would otherwise leak between tests.
    syslog_forward::clear_settings();
    syslog_forward::clear_queue();

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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/settings/syslog",
            get(handle_syslog_settings_get).put(handle_syslog_settings_update),
        )
        .route("/api/settings/syslog/test", post(handle_syslog_test))
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
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    let req = if let Some(b) = body {
        // The API router applies the CSRF middleware only when a session token
        // is present; these routes are mounted bare in this harness, so no
        // CSRF header is needed here.
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

/// Admin + member, both signed in.
async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = router(h.state.clone());
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
async fn test_syslog_settings_is_admin_only() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    // No session at all.
    let (status, _) = send(&mut router, Method::GET, "/api/settings/syslog", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        None,
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/settings/syslog/test",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A regular member may not even read where the audit trail goes.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/syslog",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&member),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/settings/syslog/test",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin reads the disabled default.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/syslog",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["status"], "disabled");
    assert_eq!(body["data"]["protocol"], "udp");
    assert_eq!(body["data"]["format"], "rfc5424");
    assert_eq!(body["data"]["port"], 514);
    assert_eq!(body["data"]["facility"], 1);
    assert_eq!(body["data"]["queue"]["pending"], 0);
}

// ---------------------------------------------------------------------------
// Validation and round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_syslog_settings_round_trip() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "protocol": "tcp",
            "host": " 192.168.1.20 ",
            "port": 1514,
            "format": "rfc3164",
            "facility": 16,
            "include_app_events": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "save accepted: {body}");
    assert_eq!(body["data"]["settings"]["enabled"], true);
    assert_eq!(body["data"]["settings"]["protocol"], "tcp");
    assert_eq!(body["data"]["settings"]["format"], "rfc3164");
    assert_eq!(body["data"]["settings"]["port"], 1514);
    assert_eq!(body["data"]["settings"]["facility"], 16);
    // Nothing has been sent yet: enabled and quiet is `idle`, never
    // `forwarding`.
    assert_eq!(body["data"]["settings"]["status"], "idle");
    assert_eq!(body["data"]["settings"]["last_error"], Value::Null);

    // …and it survives a reload from disk.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/syslog",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["host"], "192.168.1.20");
    assert_eq!(body["data"]["protocol"], "tcp");
}

#[tokio::test]
async fn test_syslog_rejects_an_invalid_configuration() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let cases: Vec<(&str, Value)> = vec![
        ("unknown protocol", json!({"enabled": true, "host": "10.0.0.9", "protocol": "quic"})),
        ("unknown format", json!({"enabled": true, "host": "10.0.0.9", "format": "rfc9999"})),
        ("missing host", json!({"enabled": true, "host": "   "})),
        ("host with a scheme", json!({"enabled": true, "host": "https://collector.lan"})),
        ("host with a path", json!({"enabled": true, "host": "collector.lan/syslog"})),
        ("port zero", json!({"enabled": true, "host": "10.0.0.9", "port": 0})),
        ("facility too high", json!({"enabled": true, "host": "10.0.0.9", "facility": 24})),
        ("tls without a CA", json!({"enabled": true, "host": "10.0.0.9", "protocol": "tls"})),
        (
            "tls with a non-PEM CA",
            json!({"enabled": true, "host": "10.0.0.9", "protocol": "tls", "ca_pem": "hunter2"}),
        ),
    ];
    for (label, payload) in cases {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/settings/syslog",
            Some(&admin),
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label} must be refused");
        assert_eq!(body["success"], false, "{label}");
    }

    // Nothing was stored by any refused request.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/syslog",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["enabled"], false);
}

#[tokio::test]
async fn test_syslog_a_disabled_entry_may_stay_half_filled() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // An admin must be able to switch the feature off without first deleting
    // the destination — otherwise "off" is impossible to reach from a broken
    // configuration.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({"enabled": false, "host": "", "port": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "disabled save accepted: {body}");
    assert_eq!(body["data"]["settings"]["status"], "disabled");
}

#[tokio::test]
async fn test_syslog_server_owned_fields_cannot_be_written() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // The request shape has no `status` / `last_error` / `last_sent_at`: an
    // attempt to set them is ignored, not honoured, so a client can never
    // fabricate "forwarding" or hide a failure.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "host": "10.0.0.9",
            "status": "forwarding",
            "last_error": "all good",
            "last_sent_at": "2026-10-31T12:00:00Z",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["settings"]["status"], "idle");
    assert_eq!(body["data"]["settings"]["last_error"], Value::Null);
    assert_eq!(body["data"]["settings"]["last_sent_at"], Value::Null);
}

// ---------------------------------------------------------------------------
// The bytes that leave the box
// ---------------------------------------------------------------------------

/// A real UDP collector on localhost: the test message must arrive, framed.
#[tokio::test]
async fn test_syslog_test_message_reaches_a_udp_collector() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let collector = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind collector");
    let port = collector.local_addr().expect("addr").port();

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "protocol": "udp",
            "host": "127.0.0.1",
            "port": port,
            "format": "rfc5424",
            "facility": 1,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "saved: {body}");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/syslog/test",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "test sent: {body}");

    let mut buf = vec![0u8; 4096];
    let (len, _) = tokio::time::timeout(Duration::from_secs(5), collector.recv_from(&mut buf))
        .await
        .expect("the datagram must arrive")
        .expect("recv");
    let line = String::from_utf8_lossy(&buf[..len]).to_string();
    // `user.info` (1×8+6) in RFC 5424, from this app, tagged as a test.
    assert!(line.starts_with("<14>1 "), "framed RFC 5424, got: {line}");
    assert!(line.contains(" homevault "), "app name in the header: {line}");
    assert!(line.contains(" test "), "the test message is identifiable: {line}");
    assert!(
        line.contains("HomeVault test message"),
        "the human-readable body travels: {line}"
    );
}

/// A real TCP collector: the batch is newline-framed, one message per line.
#[tokio::test]
async fn test_syslog_test_message_reaches_a_tcp_collector() {
    use tokio::io::AsyncReadExt;

    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind collector");
    let port = listener.local_addr().expect("addr").port();

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "protocol": "tcp",
            "host": "127.0.0.1",
            "port": port,
            "format": "rfc3164",
            "facility": 1,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "saved: {body}");

    // Accept in the background, so the handler's connect() is not waiting on a
    // listener nobody is accepting.
    let acceptor = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut received = Vec::new();
        // The sender closes after flushing, so read-to-end terminates.
        let _ = socket.read_to_end(&mut received).await;
        String::from_utf8_lossy(&received).to_string()
    });

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/syslog/test",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "test sent: {body}");

    let text = tokio::time::timeout(Duration::from_secs(5), acceptor)
        .await
        .expect("the collector must receive the batch")
        .expect("accept task");
    let line = text.lines().next().unwrap_or_default();
    // `user.info` in RFC 3164: `<14>Mon dd hh:mm:ss host homevault[pid]: …`.
    assert!(line.starts_with("<14>"), "framed RFC 3164, got: {line}");
    assert!(line.contains("homevault["), "tag with pid: {line}");
    assert!(line.contains("HomeVault test message"), "body: {line}");
}

/// An unreachable collector must be an explicit refusal on the *test* button
/// and must never fail the save that configured it.
#[tokio::test]
async fn test_syslog_unreachable_collector_is_reported_not_hidden() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // Port 0 is refused by validation, so bind-and-drop a port to guarantee
    // nothing is listening on it.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let port = probe.local_addr().expect("addr").port();
    drop(probe);

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "protocol": "tcp",
            "host": "127.0.0.1",
            "port": port,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a dead collector is not a save error");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/syslog/test",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body["message"].as_str().unwrap_or_default().to_string();
    assert!(
        message.contains("could not be sent"),
        "the admin is told the message did not leave: {message}"
    );
}

#[tokio::test]
async fn test_syslog_test_button_is_refused_while_disabled() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/syslog/test",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("disabled"),
        "the reason names the actual state: {body}"
    );
}

// ---------------------------------------------------------------------------
// Forwarding path: an audit event enters the queue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_an_audit_event_is_queued_for_the_collector() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // Enabled but pointing nowhere reachable: the queue is what matters here.
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({"enabled": true, "protocol": "tcp", "host": "127.0.0.1", "port": 9})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    syslog_forward::clear_queue();

    // The same call every audited mutation makes.
    homevault::events::log_event("User 'e2e' created a group named 'family'");

    let stats = syslog_forward::queue_stats();
    assert!(
        stats.pending >= 1,
        "the audit event must be waiting for the worker, got {stats:?}"
    );
    assert!(stats.enqueued >= 1);

    // While forwarding is off, nothing new is queued: a disabled feature must
    // cost a log call nothing at all.
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    syslog_forward::clear_queue();
    homevault::events::log_event("this must not be queued");
    assert_eq!(syslog_forward::queue_stats().pending, 0);
}

/// The worker records an outcome without clobbering a configuration the admin
/// saved meanwhile. `update_observation` re-reads under the write lock and
/// changes only the three observation fields; a whole-snapshot write from a
/// batch that started a second earlier would silently reverse a destination
/// change (a home box must never ship its journal to the previous collector).
#[tokio::test]
async fn test_a_recorded_outcome_never_reverts_the_configuration() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // The admin saves a destination, then the worker — which captured the
    // *previous* settings before sending — records the result of that send.
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/syslog",
        Some(&admin),
        Some(json!({
            "enabled": true,
            "protocol": "tcp",
            "host": "192.0.2.77",
            "port": 6514,
            "format": "rfc5424",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    syslog_forward::update_observation(|saved| {
        saved.status = syslog_forward::SyslogStatus::Degraded;
        saved.last_error = Some("cannot reach 10.0.0.1:514".to_string());
        saved.last_sent_at = None;
    })
    .expect("observation saved");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/syslog",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // The destination the admin chose is intact…
    assert_eq!(body["data"]["host"], "192.0.2.77");
    assert_eq!(body["data"]["port"], 6514);
    assert_eq!(body["data"]["protocol"], "tcp");
    assert_eq!(body["data"]["enabled"], true);
    // …and the observation landed on top of it.
    assert_eq!(body["data"]["status"], "degraded");
    assert_eq!(body["data"]["last_error"], "cannot reach 10.0.0.1:514");
}
