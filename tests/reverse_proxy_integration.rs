/* =============================================================================
 * File          : reverse_proxy_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Generic reverse proxy integration tests (phase C2): the CRUD/auth contract
 *   through the assembled Axum router, and a real listener forwarding to a
 *   loopback upstream. Nothing here talks to a public address or touches the
 *   host's networking: the listener binds an ephemeral port and the upstream is
 *   a test server on 127.0.0.1.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot + hyper loopback)
 *
 * Usage:
 *   cargo test --test reverse_proxy_integration
 * ============================================================================= */

#![allow(clippy::await_holding_lock)]
//! Integration tests for the generic reverse proxy.

use axum::routing::{get, post};
use axum::Router;
use http::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::reverse_proxy::{
    handle_create, handle_delete, handle_list, handle_proxy_auth, handle_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::reverse_proxy::ticket::TICKETS;
use homevault::reverse_proxy::{ProxyRule, ReverseProxyManager};
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
        .route("/api/proxy/rules", get(handle_list).post(handle_create))
        .route(
            "/api/proxy/rules/{id}",
            axum::routing::put(handle_update).delete(handle_delete),
        )
        .route("/proxy-auth", get(handle_proxy_auth))
        .with_state(state)
}

async fn create_test_user(state: &AppState, username: &str, role_id: i64) {
    let password_hash = state.auth.hash_password("Password123!").unwrap();
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

async fn login(router: &mut Router, username: &str) -> String {
    let (_, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": "Password123!"})),
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
    if let Some(token) = token {
        req = req.header("x-session-token", token);
    }
    let req = if let Some(body) = body {
        req.header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
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

fn rule_body(host: &str, path: &str, upstream: &str) -> Value {
    json!({
        "host": host,
        "path": path,
        "upstream": upstream,
        "tls": false,
        "auth_required": true,
        "enabled": true,
    })
}

async fn admin_router() -> (
    Arc<AppState>,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
    Router,
    String,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, "admin", 2).await;
    let mut r = router(state.clone());
    let token = login(&mut r, "admin").await;
    (state, tmp, guard, r, token)
}

// ---------------------------------------------------------------------------
// 1. Guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_rules_require_a_session() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    for (method, uri) in [
        (Method::GET, "/api/proxy/rules"),
        (Method::POST, "/api/proxy/rules"),
        (Method::PUT, "/api/proxy/rules/1"),
        (Method::DELETE, "/api/proxy/rules/1"),
    ] {
        let (status, body) = send(
            &mut r,
            method.clone(),
            uri,
            None,
            Some(rule_body("a.lan", "", "127.0.0.1:9000")),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}: {body}");
    }
}

#[tokio::test]
async fn test_reads_need_a_member_but_mutations_need_an_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "member").await;

    let (status, body) = send(&mut r, Method::GET, "/api/proxy/rules", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["rules"].as_array().unwrap().len(), 0);

    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(rule_body("a.lan", "", "127.0.0.1:9000")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// 2. CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_list_update_delete_round_trip() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(rule_body("HA.Home.LAN.", "/api/", "127.0.0.1:9000")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rule = &body["data"]["rule"];
    let id = rule["id"].as_i64().unwrap();
    assert_eq!(rule["host"], "ha.home.lan");
    assert_eq!(rule["path"], "/api");
    assert_eq!(rule["upstream"], "127.0.0.1:9000");
    assert_eq!(rule["auth_required"], true);

    let (status, body) = send(&mut r, Method::GET, "/api/proxy/rules", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["rules"].as_array().unwrap().len(), 1);
    assert!(body["data"]["listener"].is_object());

    let (status, body) = send(
        &mut r,
        Method::PUT,
        &format!("/api/proxy/rules/{id}"),
        Some(&token),
        Some(json!({
            "host": "ha.home.lan",
            "path": "/api",
            "upstream": "127.0.0.1:9001",
            "tls": false,
            "auth_required": false,
            "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["rule"]["upstream"], "127.0.0.1:9001");
    assert_eq!(body["data"]["rule"]["auth_required"], false);

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/proxy/rules/{id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = send(&mut r, Method::GET, "/api/proxy/rules", Some(&token), None).await;
    assert_eq!(body["data"]["rules"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_duplicate_host_and_path_is_refused() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let body = rule_body("ha.lan", "", "127.0.0.1:9000");
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, response) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert!(response["message"]
        .as_str()
        .unwrap()
        .contains("already exists"));
}

#[tokio::test]
async fn test_unknown_id_is_a_404() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    for method in [Method::PUT, Method::DELETE] {
        let (status, _) = send(
            &mut r,
            method.clone(),
            "/api/proxy/rules/9999",
            Some(&token),
            Some(rule_body("ha.lan", "", "127.0.0.1:9000")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
}

// ---------------------------------------------------------------------------
// 3. Validation and address policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_validation_refusals_name_the_rule_they_broke() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let cases = [
        (rule_body("ha.lan:8080", "", "127.0.0.1:9000"), "port"),
        (rule_body("http://ha.lan", "", "127.0.0.1:9000"), "http"),
        (rule_body("*.ha.lan", "", "127.0.0.1:9000"), "Wildcards"),
        (rule_body("ha.lan", "api", "127.0.0.1:9000"), "start with /"),
        (
            rule_body("ha.lan", "", "http://127.0.0.1:9000"),
            "host:port",
        ),
        (rule_body("ha.lan", "", "127.0.0.1:0"), "port"),
    ];
    for (body, expected) in cases {
        let (status, response) = send(
            &mut r,
            Method::POST,
            "/api/proxy/rules",
            Some(&token),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert!(
            response["message"]
                .as_str()
                .unwrap_or("")
                .contains(expected),
            "expected '{expected}' in {response}"
        );
    }
}

#[tokio::test]
async fn test_a_public_upstream_is_refused_at_save_time() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let (status, response) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(rule_body("ha.lan", "", "8.8.8.8:53")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert!(response["message"]
        .as_str()
        .unwrap_or("")
        .contains("not a private or loopback address"));
}

#[tokio::test]
async fn test_the_reserved_callback_path_is_refused() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(rule_body("ha.lan", "/__hv_proxy_auth", "127.0.0.1:9000")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_the_guard_runs_before_the_body_is_parsed() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        None,
        Some(json!({"not": "a rule"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// 4. /proxy-auth (dashboard side)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_proxy_auth_without_a_session_goes_to_login_and_keeps_next() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri("/proxy-auth?rule=3&next=%2Fdeep%2Fpath")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FOUND);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(location.starts_with("/login?next="), "{location}");
    assert!(location.contains("rule%3D3"), "{location}");
}

#[tokio::test]
async fn test_proxy_auth_for_an_unknown_rule_is_a_404() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri("/proxy-auth?rule=999&next=%2F")
                .header("cookie", format!("hv_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_proxy_auth_says_the_listener_is_down_instead_of_redirecting_nowhere() {
    let (_state, _tmp, _guard, mut r, token) = admin_router().await;
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/proxy/rules",
        Some(&token),
        Some(rule_body("ha.lan", "", "127.0.0.1:9000")),
    )
    .await;
    let id = body["data"]["rule"]["id"].as_i64().unwrap();

    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/proxy-auth?rule={id}&next=%2F"))
                .header("cookie", format!("hv_session={token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // The global listener is never started in this test process.
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ---------------------------------------------------------------------------
// 5. The listener, against a loopback upstream
// ---------------------------------------------------------------------------

/// A tiny upstream that echoes the request path and selected headers, so the
/// test can prove what the listener did to them.
async fn spawn_upstream() -> (u16, tokio::task::JoinHandle<()>) {
    use axum::extract::State;
    use axum::response::IntoResponse;

    #[derive(Clone)]
    struct Seen(Arc<Mutex<Vec<String>>>);

    async fn echo(
        State(seen): State<Seen>,
        req: Request<axum::body::Body>,
    ) -> axum::response::Response {
        let path = req.uri().path().to_string();
        let forwarded = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let real = req
            .headers()
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let custom = req
            .headers()
            .get("x-custom")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let cookie = req
            .headers()
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let host = req
            .headers()
            .get("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        seen.0.lock().unwrap().push(path.clone());
        (
            StatusCode::OK,
            format!("path={path};xff={forwarded};real={real};custom={custom};cookie={cookie};host={host}"),
        )
            .into_response()
    }

    let seen = Seen(Arc::new(Mutex::new(Vec::new())));
    let app = Router::new().fallback(echo).with_state(seen);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream bind");
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (port, handle)
}

fn test_rule(id: i64, host: &str, path: &str, upstream: &str, auth_required: bool) -> ProxyRule {
    ProxyRule {
        id,
        host: host.to_string(),
        path: path.to_string(),
        upstream: upstream.to_string(),
        tls: false,
        auth_required,
        enabled: true,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

async fn start_proxy(rules: Vec<ProxyRule>) -> (Arc<ReverseProxyManager>, u16) {
    let manager = Arc::new(ReverseProxyManager::new(0));
    manager.init(8080);
    manager
        .start_with_rules(rules)
        .await
        .expect("proxy binds an ephemeral port");
    let port = manager.status().await.port;
    (manager, port)
}

async fn fetch(
    port: u16,
    host: &str,
    path: &str,
    cookie: Option<&str>,
) -> (StatusCode, http::HeaderMap, String) {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect listener");
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("host", host)
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-real-ip", "203.0.113.9")
        .header("x-custom", "kept");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let request = builder
        .body(http_body_util::Empty::<hyper::body::Bytes>::new())
        .unwrap();
    let response = sender.send_request(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, headers, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn test_listener_forwards_the_path_and_never_the_forged_headers() {
    let (upstream, _handle) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![test_rule(
        1,
        "ha.home.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        false,
    )])
    .await;

    let (status, _, body) = fetch(port, "ha.home.lan", "/lovelace?x=1", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("path=/lovelace"), "{body}");
    // The client's forged identity is replaced by the observed peer.
    assert!(body.contains("xff=127.0.0.1"), "{body}");
    assert!(body.contains("real=127.0.0.1"), "{body}");
    // A header the proxy does not own is preserved.
    assert!(body.contains("custom=kept"), "{body}");
    // Host is rewritten to the upstream authority, not the rule's name.
    assert!(
        body.contains(&format!("host=127.0.0.1:{upstream}")),
        "{body}"
    );
}

#[tokio::test]
async fn test_listener_uses_the_longest_path_prefix() {
    let (one, _h1) = spawn_upstream().await;
    let (two, _h2) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![
        test_rule(1, "ha.lan", "", &format!("127.0.0.1:{one}"), false),
        test_rule(2, "ha.lan", "/api", &format!("127.0.0.1:{two}"), false),
    ])
    .await;

    let (_, _, body) = fetch(port, "ha.lan", "/api/x", None).await;
    assert!(body.contains("path=/api/x"), "{body}");
    assert!(
        body.contains(&format!("host=127.0.0.1:{two}")),
        "the /api rule must win: {body}"
    );
    let (_, _, body) = fetch(port, "ha.lan", "/other", None).await;
    assert!(
        body.contains(&format!("host=127.0.0.1:{one}")),
        "the host-wide rule must serve the rest: {body}"
    );
}

#[tokio::test]
async fn test_unknown_host_is_a_404() {
    let (upstream, _handle) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![test_rule(
        1,
        "ha.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        false,
    )])
    .await;
    let (status, _, body) = fetch(port, "other.lan", "/", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn test_a_gated_rule_redirects_to_the_dashboard_entry() {
    let (upstream, _handle) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![test_rule(
        7,
        "ha.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        true,
    )])
    .await;

    let (status, headers, _) = fetch(port, "ha.lan", "/deep/path?x=1", None).await;
    assert_eq!(status, StatusCode::FOUND);
    let location = headers
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert!(location.contains("/proxy-auth?rule=7"), "{location}");
    assert!(
        location.contains("next=%2Fdeep%2Fpath%3Fx%3D1"),
        "{location}"
    );
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
}

#[tokio::test]
async fn test_ticket_callback_mints_a_rule_scoped_session_then_forwards() {
    let (upstream, _handle) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![test_rule(
        7,
        "ha.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        true,
    )])
    .await;

    let ticket = TICKETS.mint(7, "alice", "/deep");
    let path = format!("/__hv_proxy_auth?rule=7&ticket={ticket}&next=%2Fdeep");
    let (status, headers, _) = fetch(port, "ha.lan", &path, None).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(headers.get("location").unwrap(), "/deep");
    let cookie = headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("session cookie");
    assert!(cookie.contains("hv_app_session="), "{cookie}");
    assert!(!cookie.contains("Secure"), "{cookie}");

    let session = cookie.split(';').next().unwrap();
    let (status, _, body) = fetch(port, "ha.lan", "/deep", Some(session)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("path=/deep"), "{body}");
}

#[tokio::test]
async fn test_a_ticket_is_single_use() {
    let (upstream, _handle) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![test_rule(
        7,
        "ha.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        true,
    )])
    .await;

    let ticket = TICKETS.mint(7, "alice", "/");
    let path = format!("/__hv_proxy_auth?rule=7&ticket={ticket}&next=%2F");
    assert_eq!(
        fetch(port, "ha.lan", &path, None).await.0,
        StatusCode::FOUND
    );
    assert_eq!(
        fetch(port, "ha.lan", &path, None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn test_a_public_upstream_added_behind_the_api_is_refused_at_request_time() {
    let (_manager, port) = start_proxy(vec![test_rule(1, "ha.lan", "", "8.8.8.8:53", false)]).await;
    let (status, _, body) = fetch(port, "ha.lan", "/", None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert!(body.contains("8.8.8.8"), "{body}");
}

#[tokio::test]
async fn test_homevault_cookies_are_stripped_before_the_upstream() {
    let (upstream, _handle) = spawn_upstream().await;
    let (_manager, port) = start_proxy(vec![test_rule(
        1,
        "ha.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        false,
    )])
    .await;
    let (_, _, body) = fetch(
        port,
        "ha.lan",
        "/",
        Some("hv_session=api-secret; theme=dark; hv_app_session=app-secret"),
    )
    .await;
    assert!(body.contains("cookie=theme=dark"), "{body}");
    assert!(!body.contains("api-secret"), "{body}");
    assert!(!body.contains("app-secret"), "{body}");
}

/// The listener exposes the port it actually bound, which is what `GET
/// /proxy-auth` redirects to. A port of 0 in tests must not leak into a
/// user-visible URL.
#[tokio::test]
async fn test_status_reports_the_bound_port() {
    let (upstream, _handle) = spawn_upstream().await;
    let (manager, port) = start_proxy(vec![test_rule(
        1,
        "ha.lan",
        "",
        &format!("127.0.0.1:{upstream}"),
        false,
    )])
    .await;
    assert_ne!(port, 0);
    let status = manager.status().await;
    assert!(status.listening);
    assert_eq!(status.port, port);
    assert!(status.error.is_none());
}

/// Unused in the happy path, but the type keeps `SocketAddr` in scope for the
/// upstream helper above.
#[allow(dead_code)]
fn _socket_addr(addr: SocketAddr) -> SocketAddr {
    addr
}
