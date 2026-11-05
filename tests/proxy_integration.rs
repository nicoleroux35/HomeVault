/* =============================================================================
 * File          : proxy_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026
 * Last modified : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the outbound HTTP(S) proxy: the admin guard on both
 *   routes, validation before persistence, the loopback exemption, and a real
 *   mock proxy proving that an outbound request is actually routed through the
 *   configured address.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot + tokio sockets)
 *
 * Usage:
 *   cargo test --test proxy_integration
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::proxy::{handle_proxy_settings_get, handle_proxy_settings_update};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::outbound::{HttpRequest, ReqwestTransport, Transport};
use homevault::proxy::ProxySettings;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<Body>;

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
    let (ransomware_watch_tx, _rx) =
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
            "/api/settings/proxy",
            get(handle_proxy_settings_get).put(handle_proxy_settings_update),
        )
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let password_hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Test User', 'test@example.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&password_hash)
    .bind(role_id)
    .execute(state.db.pool().as_ref())
    .await
    .expect("insert user");
}

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {body}");
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
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        req.body(Body::empty()).unwrap()
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

fn enabled_body(http_proxy: &str) -> Value {
    json!({
        "enabled": true,
        "http_proxy": http_proxy,
        "https_proxy": "",
        "no_proxy": "",
    })
}

#[tokio::test]
async fn every_proxy_endpoint_refuses_an_anonymous_caller_with_401() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    for (method, body) in [
        (Method::GET, None),
        (Method::PUT, Some(enabled_body("http://proxy.lan:3128"))),
    ] {
        let (status, _) = send(&mut r, method, "/api/settings/proxy", None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn a_non_admin_session_gets_403() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "regular", "Passw0rd!123", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "regular", "Passw0rd!123").await;
    for (method, body) in [
        (Method::GET, None),
        (Method::PUT, Some(enabled_body("http://proxy.lan:3128"))),
    ] {
        let (status, _) = send(&mut r, method, "/api/settings/proxy", Some(&token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn an_admin_reads_the_defaults_with_loopback_always_exempt() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/proxy",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["enabled"], false);
    assert_eq!(data["http_proxy"], "");
    assert_eq!(data["https_proxy"], "");
    assert_eq!(data["no_proxy"], "");
    assert_eq!(data["requires_restart"], false);
    let effective = data["effective_no_proxy"].as_str().unwrap();
    for exempt in homevault::proxy::ALWAYS_DIRECT {
        assert!(effective.split(',').any(|e| e == *exempt), "{effective}");
    }
}

#[tokio::test]
async fn a_saved_configuration_round_trips_and_is_trimmed() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/proxy",
        Some(&token),
        Some(json!({
            "enabled": true,
            "http_proxy": "  http://proxy.lan:3128 ",
            "https_proxy": "http://proxy.lan:3128",
            "no_proxy": " example.com,192.168.1.0/24 ",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let saved = &resp["data"]["settings"];
    assert_eq!(saved["enabled"], true);
    assert_eq!(saved["http_proxy"], "http://proxy.lan:3128");
    assert_eq!(saved["https_proxy"], "http://proxy.lan:3128");
    assert_eq!(saved["no_proxy"], "example.com,192.168.1.0/24");

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/proxy",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["http_proxy"], "http://proxy.lan:3128");
    assert_eq!(
        homevault::proxy::load_settings().http_proxy,
        "http://proxy.lan:3128"
    );
}

#[tokio::test]
async fn unsupported_or_unsafe_proxy_values_are_refused_before_persistence() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let cases = [
        (
            enabled_body("socks5://127.0.0.1:1080"),
            "SOCKS proxies are not supported",
        ),
        (
            enabled_body("http://user:secret@proxy.lan:3128"),
            "must not embed credentials",
        ),
        (enabled_body("ftp://proxy.lan:21"), "http:// or https://"),
        (
            json!({
                "enabled": true,
                "http_proxy": "",
                "https_proxy": "",
                "no_proxy": "",
            }),
            "at least one proxy URL",
        ),
        (
            json!({
                "enabled": true,
                "http_proxy": "http://proxy.lan:3128",
                "https_proxy": "",
                "no_proxy": "http://example.com",
            }),
            "bare hosts",
        ),
    ];
    for (body, needle) in cases {
        let (status, resp) = send(
            &mut r,
            Method::PUT,
            "/api/settings/proxy",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} → {resp}");
        assert!(
            resp["message"].as_str().unwrap_or("").contains(needle),
            "{body} must name '{needle}': {resp}"
        );
    }

    // Nothing was persisted by a refused save.
    assert!(!homevault::proxy::load_settings().enabled);
}

#[tokio::test]
async fn a_disabled_entry_may_stay_empty() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/proxy",
        Some(&token),
        Some(json!({
            "enabled": false,
            "http_proxy": "",
            "https_proxy": "",
            "no_proxy": "",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"]["settings"]["enabled"], false);
}

// --------------------------------------------------------------------------
// Routing: the request really goes through the proxy, loopback does not.
// --------------------------------------------------------------------------

/// A one-shot HTTP proxy: records the request line and answers `200 ok`.
async fn start_mock_proxy() -> (u16, tokio::sync::oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = tx.send(request);
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await;
        }
    });
    (port, rx)
}

/// A one-shot plain HTTP server, used as the loopback destination.
async fn start_mock_target() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("target bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nlocal",
                )
                .await;
        }
    });
    port
}

#[tokio::test]
async fn an_outbound_request_is_routed_through_the_configured_proxy() {
    let (proxy_port, request_line) = start_mock_proxy().await;
    let settings = ProxySettings {
        enabled: true,
        http_proxy: format!("http://127.0.0.1:{proxy_port}"),
        https_proxy: String::new(),
        no_proxy: String::new(),
    };
    let transport = ReqwestTransport::new_with_proxy(&settings).expect("transport");

    let response = transport
        .send(HttpRequest::get("http://example.invalid/probe"))
        .await
        .expect("request through the proxy");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "ok");

    let request = tokio::time::timeout(Duration::from_secs(5), request_line)
        .await
        .expect("the proxy must receive the request")
        .expect("request line");
    assert!(
        request.starts_with("GET http://example.invalid/probe HTTP/1.1"),
        "an HTTP proxy receives the absolute URL: {request}"
    );
}

#[tokio::test]
async fn loopback_traffic_never_goes_through_the_proxy() {
    let proxy = TcpListener::bind("127.0.0.1:0").await.expect("proxy bind");
    let proxy_port = proxy.local_addr().unwrap().port();
    let target_port = start_mock_target().await;

    let settings = ProxySettings {
        enabled: true,
        http_proxy: format!("http://127.0.0.1:{proxy_port}"),
        https_proxy: String::new(),
        // Deliberately empty: the server adds loopback on its own.
        no_proxy: String::new(),
    };
    let transport = ReqwestTransport::new_with_proxy(&settings).expect("transport");

    let response = transport
        .send(HttpRequest::get(&format!(
            "http://127.0.0.1:{target_port}/health"
        )))
        .await
        .expect("direct request to loopback");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "local");

    // The proxy listener must stay silent: no connection was made to it.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), proxy.accept())
            .await
            .is_err(),
        "loopback must bypass the outbound proxy"
    );
}
