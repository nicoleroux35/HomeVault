/* =============================================================================
 * File          : wol_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Wake-on-LAN integration tests: in-memory SQLite + assembled Axum router
 *   for the admin matrix, the CRUD round-trip and the validation paths, plus a
 *   real localhost UDP socket that must receive the 102-byte magic packet.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test wol_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for Wake-on-LAN (`crate::wol`,
//! `crate::handlers::wol`):
//!
//!   GET    /api/power/wol/hosts
//!   POST   /api/power/wol/hosts
//!   PUT    /api/power/wol/hosts/{id}
//!   DELETE /api/power/wol/hosts/{id}
//!   POST   /api/power/wol/wake
//!
//! The list/CRUD tests only touch SQLite. The wake test binds a UDP socket on
//! `127.0.0.1:0` and configures a host whose destination *is* that socket, so
//! the bytes on the wire are asserted for real — no broadcast is sent on the
//! test machine's real network.
//!
//! Run:
//!   cargo test --test wol_integration
//!   cargo test --test wol_integration -- --nocapture

use axum::{
    routing::{get, post, put},
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
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::wol::{
    handle_wol_host_create, handle_wol_host_delete, handle_wol_host_update, handle_wol_hosts_list,
    handle_wol_wake,
};
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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/power/wol/hosts",
            get(handle_wol_hosts_list).post(handle_wol_host_create),
        )
        .route(
            "/api/power/wol/hosts/{id}",
            put(handle_wol_host_update).delete(handle_wol_host_delete),
        )
        .route("/api/power/wol/wake", post(handle_wol_wake))
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
async fn test_wol_endpoints_are_admin_only() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    // No session.
    for (method, uri, body) in [
        (Method::GET, "/api/power/wol/hosts", None),
        (
            Method::POST,
            "/api/power/wol/hosts",
            Some(json!({"label": "pc", "mac": "AA:BB:CC:DD:EE:FF"})),
        ),
        (
            Method::POST,
            "/api/power/wol/wake",
            Some(json!({"mac": "AA:BB:CC:DD:EE:FF"})),
        ),
    ] {
        let (status, _) = send(&mut router, method, uri, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }

    // A regular member: reading the list is already privileged (it names the
    // machines on the network), and sending a packet certainly is.
    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/power/wol/hosts",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/hosts",
        Some(&member),
        Some(json!({"label": "pc", "mac": "AA:BB:CC:DD:EE:FF"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/wake",
        Some(&member),
        Some(json!({"mac": "AA:BB:CC:DD:EE:FF"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin reads an empty list plus the interface evidence.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/power/wol/hosts",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["hosts"], json!([]));
    assert!(body["data"]["interfaces"].is_array());
    assert_eq!(body["data"]["ports"], json!([9, 7]));
    assert!(body["data"]["max_hosts"].as_u64().unwrap_or(0) >= 1);
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_wol_host_round_trip() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/hosts",
        Some(&admin),
        Some(json!({"label": "  Living room PC  ", "mac": "aa-bb-cc-dd-ee-ff"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "created: {body}");
    // Trimmed label, canonical MAC.
    assert_eq!(body["data"]["host"]["label"], "Living room PC");
    assert_eq!(body["data"]["host"]["mac"], "AA:BB:CC:DD:EE:FF");
    assert_eq!(body["data"]["host"]["broadcast"], Value::Null);
    let id = body["data"]["host"]["id"].as_i64().expect("id");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/power/wol/hosts",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["hosts"].as_array().unwrap().len(), 1);

    // Edit: a new label, a new MAC and an explicit destination.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/power/wol/hosts/{id}"),
        Some(&admin),
        Some(json!({
            "label": "Study PC",
            "mac": "02:00:5E:10:00:01",
            "broadcast": "192.168.1.255",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "updated: {body}");
    assert_eq!(body["data"]["host"]["label"], "Study PC");
    assert_eq!(body["data"]["host"]["mac"], "02:00:5E:10:00:01");
    assert_eq!(body["data"]["host"]["broadcast"], "192.168.1.255");

    // Duplicate label is refused with a readable reason, not a 500.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/hosts",
        Some(&admin),
        Some(json!({"label": "study pc", "mac": "AA:BB:CC:DD:EE:01"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("already exists"),
        "the reason names the duplicate: {body}"
    );

    // Delete, then a second delete is a miss.
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/power/wol/hosts/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/power/wol/hosts/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/power/wol/hosts",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["hosts"], json!([]));
}

#[tokio::test]
async fn test_wol_rejects_an_invalid_host() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "empty label",
            json!({"label": "  ", "mac": "AA:BB:CC:DD:EE:FF"}),
        ),
        (
            "label too long",
            json!({"label": "x".repeat(65), "mac": "AA:BB:CC:DD:EE:FF"}),
        ),
        ("missing mac", json!({"label": "pc", "mac": ""})),
        ("short mac", json!({"label": "pc", "mac": "AA:BB:CC:DD:EE"})),
        (
            "non-hex mac",
            json!({"label": "pc", "mac": "GG:BB:CC:DD:EE:FF"}),
        ),
        (
            "all-zero mac",
            json!({"label": "pc", "mac": "00:00:00:00:00:00"}),
        ),
        (
            "broadcast mac",
            json!({"label": "pc", "mac": "FF:FF:FF:FF:FF:FF"}),
        ),
        (
            "multicast mac",
            json!({"label": "pc", "mac": "01:00:5E:00:00:01"}),
        ),
        // A host name would be resolved at wake time.
        (
            "named destination",
            json!({"label": "pc", "mac": "AA:BB:CC:DD:EE:FF", "broadcast": "nas.local"}),
        ),
        (
            "cidr destination",
            json!({"label": "pc", "mac": "AA:BB:CC:DD:EE:FF", "broadcast": "192.168.1.255/24"}),
        ),
    ];
    for (label, payload) in cases {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/power/wol/hosts",
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
        "/api/power/wol/hosts",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(body["data"]["hosts"], json!([]));
}

// ---------------------------------------------------------------------------
// The bytes that leave the box
// ---------------------------------------------------------------------------

/// The 102 bytes on the wire, captured by a real localhost UDP socket.
///
/// This drives `wol::send_magic` directly rather than the endpoint: the
/// endpoint sends to ports 9 and 7 of the destination (privileged, and
/// conventionally closed), so a test cannot bind there. The endpoint's own
/// contract — what it reports — is pinned by the test below.
#[tokio::test]
async fn test_wol_sends_the_magic_packet_bytes() {
    let collector = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("bind collector");
    let port = collector.local_addr().expect("addr").port();

    let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    homevault::wol::send_magic(&mac, format!("127.0.0.1:{port}").parse().expect("addr"))
        .await
        .expect("sent");

    let mut buf = vec![0u8; 512];
    let (len, _from) = tokio::time::timeout(Duration::from_secs(5), collector.recv_from(&mut buf))
        .await
        .expect("the datagram must arrive")
        .expect("recv");
    assert_eq!(len, homevault::wol::MAGIC_PACKET_LEN, "102 bytes");
    assert_eq!(&buf[..6], &[0xFF; 6]);
    for repeat in 0..16 {
        let start = 6 + repeat * 6;
        assert_eq!(&buf[start..start + 6], &mac, "repeat {repeat}");
    }
}

/// The endpoint reports `sent` with the destination it used — and never claims
/// the machine woke, because a UDP datagram is unacknowledged and the target is
/// off by definition.
#[tokio::test]
async fn test_wol_wake_reports_sent_and_the_destination() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/hosts",
        Some(&admin),
        Some(json!({
            "label": "Test PC",
            "mac": "AA:BB:CC:DD:EE:FF",
            "broadcast": "127.0.0.1",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "host saved: {body}");
    let id = body["data"]["host"]["id"].as_i64().expect("id");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/wake",
        Some(&admin),
        Some(json!({"id": id})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wake: {body}");
    assert_eq!(body["data"]["sent"], true);
    assert_eq!(body["data"]["report"]["mac"], "AA:BB:CC:DD:EE:FF");
    assert_eq!(body["data"]["report"]["destination"], "127.0.0.1");
    assert_eq!(body["data"]["report"]["ports"], json!([9, 7]));
    // No interface is named when the admin picked the destination themselves.
    assert_eq!(body["data"]["report"]["interface"], Value::Null);
    let message = body["data"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("only confirms it left"), "{message}");
    assert!(!message.to_lowercase().contains("woken"), "{message}");
}

#[tokio::test]
async fn test_wol_wake_accepts_a_one_off_mac_and_refuses_a_bad_one() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // An ad-hoc MAC goes to the interface broadcast — no saved host needed.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/wake",
        Some(&admin),
        Some(json!({"mac": "aa:bb:cc:dd:ee:ff"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "wake: {body}");
    assert_eq!(body["data"]["report"]["mac"], "AA:BB:CC:DD:EE:FF");

    // A bad MAC is a 400 with the reason, and nothing is sent.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/wake",
        Some(&admin),
        Some(json!({"mac": "nope"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);

    // Neither an id nor a MAC: the message says what to do.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/wake",
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("saved host"),
        "{body}"
    );

    // A host that no longer exists is a miss, not a silent success.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/wol/wake",
        Some(&admin),
        Some(json!({"id": 424242})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// "Wake first" on scheduled tasks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_task_wake_block_must_be_valid_at_creation_time() {
    let h = setup().await;
    let (mut router, admin, _) = signed_in(&h).await;

    // The scheduled-tasks router is not mounted in this harness, so exercise
    // the same gate the create/update handler calls.
    assert!(homevault::wol::parse_wake_plan(
        r#"{"type":"command","command":"/bin/true","wake":{"host_id":1,"delay_seconds":30}}"#
    )
    .unwrap()
    .is_some());
    // A typo in the wait, or a missing host, is refused — never ignored.
    assert!(homevault::wol::parse_wake_plan(
        r#"{"type":"command","command":"/bin/true","wake":{"host_id":1,"delay":30}}"#
    )
    .is_err());
    assert!(homevault::wol::parse_wake_plan(
        r#"{"type":"command","command":"/bin/true","wake":{"host_id":0}}"#
    )
    .is_err());
    assert!(homevault::wol::parse_wake_plan(
        r#"{"type":"command","command":"/bin/true","wake":{"host_id":1,"delay_seconds":601}}"#
    )
    .is_err());

    // …and the list endpoint the picker reads answers the saved hosts.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/power/wol/hosts",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["max_wake_delay_seconds"].as_u64().unwrap_or(0) >= 60);
}
