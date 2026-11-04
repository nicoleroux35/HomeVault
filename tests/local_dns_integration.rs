/* =============================================================================
 * File          : local_dns_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Local DNS server integration tests (phase C1): in-memory SQLite + assembled
 *   Axum router, exercising `GET|PUT /api/network/dns-server` end-to-end.
 *
 *   Nothing here applies a configuration to the host: a *valid* PUT would write
 *   `/etc/dnsmasq.d/homevault.conf` and restart `dnsmasq` on the machine running
 *   the suite. The success path is therefore covered by the module's own unit
 *   tests (`src/local_dns.rs`) and by the E2E suite, which intercepts the call;
 *   this file covers the guards, the validation, and the document round-trip
 *   through the module's own persistence helpers.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test local_dns_integration
 * =============================================================================
 */

// The tests serialize on a process-wide `ENV_LOCK` guard held across `await`
// points, because they all point `DATA_DIRECTORY` at their own temp dir.
#![allow(clippy::await_holding_lock)]
//! Integration tests for the local DNS server endpoints.

use axum::routing::{get, post};
use axum::Router;
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::local_dns::{handle_get, handle_update};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::local_dns::{self, DnsRecord, LocalDnsConfig, RecordKind};
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

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/network/dns-server",
            get(handle_get).put(handle_update),
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

/// A configuration the module accepts once `confirmed` is set.
fn enabled_config() -> Value {
    json!({
        "config": {
            "enabled": true,
            "interface": "eth0",
            "serve_dhcp_clients": false,
            "forwarders": ["192.168.1.1"],
            "records": [],
            "overrides": [],
            "confirmed": true,
        }
    })
}

// ---------------------------------------------------------------------------
// 1. Guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_local_dns_requires_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    for (method, body) in [(Method::GET, None), (Method::PUT, Some(enabled_config()))] {
        let (status, _) = send(
            &mut r,
            method.clone(),
            "/api/network/dns-server",
            None,
            body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}");
    }
}

#[tokio::test]
async fn test_local_dns_rejects_a_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/network/dns-server",
        Some("nope"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The guard runs **before** the body is parsed: an unauthenticated caller must
/// get the API's `401` envelope, never a `422` echoing the deserializer.
#[tokio::test]
async fn test_the_guard_runs_before_the_body_is_parsed() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        None,
        Some(json!({"not": "a config"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn test_local_dns_reads_need_a_session_but_not_an_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", "Password123!", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "member", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/network/dns-server",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn test_local_dns_writes_are_admin_only() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", "Password123!", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "member", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(enabled_config()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

// ---------------------------------------------------------------------------
// 2. The GET payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_reports_an_off_server_and_the_screen_inputs() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/network/dns-server",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let data = &body["data"];
    // Off by default: no name is answered until an admin asks for it.
    assert_eq!(data["config"]["enabled"], json!(false));
    assert_eq!(data["config"]["serve_dhcp_clients"], json!(false));
    assert_eq!(data["config"]["records"], json!([]));
    // The capability gate is a boolean, never a failure: the card names the
    // package when dnsmasq is missing.
    assert!(data["available"].is_boolean(), "{body}");
    // What the screen needs to render a real choice.
    assert!(data["interfaces"].is_array(), "{body}");
    assert!(data["system_forwarders"].is_array(), "{body}");
    assert!(data["state"]["running"].is_boolean(), "{body}");
    assert_eq!(data["state"]["serving"], json!(false));
}

// ---------------------------------------------------------------------------
// 3. Validation — every refusal names its reason
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_malformed_body_is_a_400() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(json!({"config": {"enabled": true}})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_enabling_without_confirmation_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let mut config = enabled_config();
    config["config"]["confirmed"] = json!(false);

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(config),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("confirmation"),
        "{body}"
    );
}

#[tokio::test]
async fn test_enabling_without_a_forwarder_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let mut config = enabled_config();
    config["config"]["forwarders"] = json!([]);

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(config),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("upstream forwarder"),
        "{body}"
    );
}

#[tokio::test]
async fn test_a_directive_injection_in_a_name_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let mut config = enabled_config();
    config["config"]["records"] = json!([
        {"name": "nas.lan\nserver=6.6.6.6", "kind": "a", "value": "192.168.1.5"}
    ]);

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(config),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn test_a_loopback_forwarder_is_refused_over_the_api() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let mut config = enabled_config();
    config["config"]["forwarders"] = json!(["127.0.0.1"]);

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(config),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("back to this appliance"),
        "{body}"
    );
}

#[tokio::test]
async fn test_an_alias_to_a_name_this_appliance_does_not_serve_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    let mut config = enabled_config();
    config["config"]["records"] = json!([
        {"name": "media.lan", "kind": "cname", "value": "nas.example.com"}
    ]);

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/network/dns-server",
        Some(&token),
        Some(config),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("does not serve"),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// 4. The document round-trips (persistence only — nothing is applied)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_saved_configuration_is_what_the_screen_reads_back() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin", "Password123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "Password123!").await;

    // The same path a PUT takes after validation, minus the host apply.
    let mut config = LocalDnsConfig {
        enabled: true,
        interface: "eth0".to_string(),
        serve_dhcp_clients: true,
        forwarders: vec!["192.168.1.1".to_string()],
        records: vec![
            DnsRecord {
                name: "nas.lan".to_string(),
                kind: RecordKind::A,
                value: "192.168.1.5".to_string(),
            },
            DnsRecord {
                name: "media.lan".to_string(),
                kind: RecordKind::Cname,
                value: "nas.lan".to_string(),
            },
        ],
        overrides: vec![],
        confirmed: true,
    };
    config = local_dns::validate_config(&config).expect("valid");
    local_dns::save_persisted(&config).await.expect("save");

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/network/dns-server",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let saved = &body["data"]["config"];
    assert_eq!(saved["enabled"], json!(true));
    assert_eq!(saved["interface"], json!("eth0"));
    assert_eq!(saved["records"][0]["name"], json!("nas.lan"));
    assert_eq!(saved["records"][1]["kind"], json!("cname"));
    // Nothing is serving yet: the daemon was never started by this test.
    assert_eq!(body["data"]["state"]["serving"], json!(false));
}

#[tokio::test]
async fn test_the_module_refuses_what_the_api_refuses() {
    // The same gate, without HTTP: a configuration the endpoint would answer
    // `400` for is refused by `validate_config`, so neither path can drift.
    let mut config = LocalDnsConfig::default();
    config.enabled = true;
    config.confirmed = true;
    config.interface = "eth0".to_string();
    assert!(local_dns::validate_config(&config).is_err(), "no forwarder");

    config.forwarders = vec!["192.168.1.1".to_string()];
    assert!(local_dns::validate_config(&config).is_ok());
}
