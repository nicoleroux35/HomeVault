/* =============================================================================
 * File          : mcp_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Mcp integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the external AI-agent (MCP) server
//! (`crate::handlers::mcp`, `crate::mcp`, migration 038) and the catalog
//! phase-B endpoints (`install-review`, `update-all`, `logs`).
//!
//! Management API (`/api/mcp/*`):
//!   GET  /api/mcp/status            — admin
//!   PUT  /api/mcp/status            — admin (kill-switch)
//!   PUT  /api/mcp/permissions       — admin (global permission set)
//!   POST /api/mcp/tokens            — admin (mint, full value shown once)
//!   DELETE /api/mcp/tokens/{id}     — admin (revoke)
//!
//! Agent endpoint:
//!   POST /mcp                       — Bearer `hv_mcp_…`, MCP Streamable HTTP
//!                                     (`initialize`, `tools/list`, `tools/call`)
//!
//! Catalog phase B:
//!   GET  /api/docker/catalog/{id}/install-review — session
//!   POST /api/docker/catalog/update-all          — admin
//!   GET  /api/docker/catalog/{id}/logs           — session
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt::oneshot` — no real server, no Docker, no Ollama.
//! Docker-touching MCP tools (apps.*, vms.*) are exercised only through
//! their auth/scope gates here; the failure modes that need a daemon stay
//! in the live E2E layer.
//!
//! Run: cargo test --test mcp_integration

use axum::{
    routing::{delete, get, post, put},
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
    docker_catalog::{
        handle_catalog_install, handle_catalog_install_review, handle_catalog_logs,
        handle_catalog_update_all,
    },
    mcp::{
        handle_mcp_create_token, handle_mcp_revoke_token, handle_mcp_rpc, handle_mcp_set_enabled,
        handle_mcp_set_permissions, handle_mcp_status,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

type Req = Request<axum::body::Body>;

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
        .route("/api/mcp/status", get(handle_mcp_status))
        .route("/api/mcp/status", put(handle_mcp_set_enabled))
        .route("/api/mcp/permissions", put(handle_mcp_set_permissions))
        .route("/api/mcp/tokens", post(handle_mcp_create_token))
        .route("/api/mcp/tokens/{id}", delete(handle_mcp_revoke_token))
        .route("/mcp", post(handle_mcp_rpc))
        .route(
            "/api/docker/catalog/{id}/install-review",
            get(handle_catalog_install_review),
        )
        .route(
            "/api/docker/catalog/update-all",
            post(handle_catalog_update_all),
        )
        .route("/api/docker/catalog/{id}/logs", get(handle_catalog_logs))
        .route(
            "/api/docker/catalog/{id}/install",
            post(handle_catalog_install),
        )
        .with_state(state)
}

async fn mk_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let hash = state.auth.hash_password(password).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'T', 't@e.com', ?, false, datetime('now'), 'seed')",
    )
    .bind(username)
    .bind(&hash)
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
        &[],
        Some(json!({"username": username, "password": password})),
    )
    .await;
    body["data"]["token"].as_str().expect("token").to_string()
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

fn session(token: &str) -> (String, String) {
    ("x-session-token".to_string(), token.to_string())
}

async fn rpc(
    router: &mut Router,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    // The /mcp handler reads the raw body as a string; send it as text.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("content-type", "application/json");
    let req = match bearer {
        Some(b) => req.header("authorization", format!("Bearer {b}")),
        None => req,
    };
    let req = req.body(axum::body::Body::from(body.to_string())).unwrap();
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

// ── Management guards ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_mcp_status_requires_admin() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "plain", "pw012345", 1).await;
    let mut r = router(state);
    let (_, anon) = send(&mut r, Method::GET, "/api/mcp/status", &[], None).await;
    assert_eq!(anon["success"], json!(false));
    let user = login(&mut r, "plain", "pw012345").await;
    let s = session(&user);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/mcp/status",
        &[(&s.0, &s.1)],
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false), "non-admin must be refused");
}

#[tokio::test]
async fn test_mcp_status_defaults_disabled() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let s = session(&admin);
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/mcp/status",
        &[(&s.0, &s.1)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"]["enabled"], json!(false));
    assert_eq!(body["data"]["status"]["endpoint"], json!("/mcp"));
    assert_eq!(body["data"]["tokens"], json!([]));
}

// ── Tokens + kill-switch ──────────────────────────────────────────────────

async fn enable_and_mint(r: &mut Router, admin: &str) -> String {
    let s = session(admin);
    let (_, body) = send(
        r,
        Method::PUT,
        "/api/mcp/status",
        &[(&s.0, &s.1)],
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(body["data"]["enabled"], json!(true));
    let (_, body) = send(
        r,
        Method::POST,
        "/api/mcp/tokens",
        &[(&s.0, &s.1)],
        Some(json!({"name": "agent-1"})),
    )
    .await;
    body["data"]["token"].as_str().expect("token").to_string()
}

#[tokio::test]
async fn test_mcp_token_lifecycle() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;

    // Disabled server: even a minted token gets 401 on /mcp.
    let s = session(&admin);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/mcp/tokens",
        &[(&s.0, &s.1)],
        Some(json!({"name": "early"})),
    )
    .await;
    let early = body["data"]["token"].as_str().expect("token").to_string();
    assert!(early.starts_with("hv_mcp_"));
    let (status, _) = rpc(&mut r, Some(&early), "ping", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = enable_and_mint(&mut r, &admin).await;
    // Token list shows the prefix, never the secret.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/mcp/status",
        &[(&s.0, &s.1)],
        None,
    )
    .await;
    let tokens = body["data"]["tokens"].as_array().expect("tokens");
    assert_eq!(tokens.len(), 2);
    assert!(tokens.iter().all(|t| t.get("secret_hash").is_none()));
    assert!(tokens
        .iter()
        .all(|t| t["prefix"].as_str().unwrap().starts_with("hv_mcp_")));

    // Live token answers initialize.
    let (status, body) = rpc(&mut r, Some(&token), "initialize", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["protocolVersion"], json!("2024-11-05"));

    // Revoke kills it; revoking again is a 400.
    let token_id = token
        .strip_prefix("hv_mcp_")
        .unwrap()
        .split_once('_')
        .unwrap()
        .0
        .to_string();
    let uri = format!("/api/mcp/tokens/{token_id}");
    let (_, body) = send(&mut r, Method::DELETE, &uri, &[(&s.0, &s.1)], None).await;
    assert_eq!(body["success"], json!(true));
    let (status, _) = rpc(&mut r, Some(&token), "ping", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, body) = send(&mut r, Method::DELETE, &uri, &[(&s.0, &s.1)], None).await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_mcp_token_name_validated() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let s = session(&admin);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/mcp/tokens",
        &[(&s.0, &s.1)],
        Some(json!({"name": ""})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

// ── Permissions ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_mcp_permissions_roundtrip_and_rejection() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let s = session(&admin);

    let granted = json!({
        "apps": "all",
        "app_store": true,
        "files": ["/tmp"],
        "manage_system": false,
        "vms": [],
        "rag": true,
    });
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/mcp/permissions",
        &[(&s.0, &s.1)],
        Some(granted),
    )
    .await;
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["data"]["apps"], json!("all"));

    // A bad shape widens nothing — 400.
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/mcp/permissions",
        &[(&s.0, &s.1)],
        Some(json!({"apps": true})),
    )
    .await;
    assert_eq!(body["success"], json!(false));

    // A relative file grant is a 400.
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/mcp/permissions",
        &[(&s.0, &s.1)],
        Some(json!({"files": ["relative/path"]})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

// ── Agent endpoint ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_mcp_rpc_auth_and_protocol_errors() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let token = enable_and_mint(&mut r, &admin).await;

    // No bearer → 401 with WWW-Authenticate.
    let (status, _) = rpc(&mut r, None, "ping", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Garbage bearer → same indistinguishable 401.
    let (status, _) = rpc(
        &mut r,
        Some("hv_mcp_deadbeefdeadbeef_0123456789abcdef0123456789abcdef"),
        "ping",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Default permissions: only the two always-on tools.
    let (_, body) = rpc(&mut r, Some(&token), "tools/list", json!({})).await;
    let tools = body["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["system.get_info", "system.get_resources"]);

    // Calling an unregistered tool is a denial, not a method error.
    let (_, body) = rpc(
        &mut r,
        Some(&token),
        "tools/call",
        json!({"name": "apps.list", "arguments": {}}),
    )
    .await;
    assert!(body.get("error").is_some());
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("permissions"));

    // Unknown method → -32601; garbage body → -32700.
    let (_, body) = rpc(&mut r, Some(&token), "frobnicate", json!({})).await;
    assert_eq!(body["error"]["code"], json!(-32601));
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from("{{not json"))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], json!(-32700));
}

#[tokio::test]
async fn test_mcp_rpc_tools_with_grants() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let token = enable_and_mint(&mut r, &admin).await;
    let s = session(&admin);

    // Grant files=/tmp (the test's temp dir lives under /tmp on Linux).
    let (_, body) = send(
        &mut r,
        Method::PUT,
        "/api/mcp/permissions",
        &[(&s.0, &s.1)],
        Some(json!({"apps": "all", "app_store": false, "files": ["/tmp"], "rag": true})),
    )
    .await;
    assert_eq!(body["success"], json!(true));

    let (_, body) = rpc(&mut r, Some(&token), "tools/list", json!({})).await;
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "system.get_info",
        "apps.list",
        "apps.uninstall",
        "files.list_directory",
        "knowledge.search",
        "catalog.search",
    ] {
        assert!(names.contains(&expected), "{expected} must be listed");
    }
    assert!(!names.contains(&"apps.install"), "app_store is off");
    assert!(!names.contains(&"system.reboot"), "manage_system is off");

    // system.get_info works with no grants at all.
    let (_, body) = rpc(
        &mut r,
        Some(&token),
        "tools/call",
        json!({"name": "system.get_info", "arguments": {}}),
    )
    .await;
    assert!(body.get("result").is_some());

    // files.list_directory inside the grant works; outside is denied.
    let (_, body) = rpc(
        &mut r,
        Some(&token),
        "tools/call",
        json!({"name": "files.list_directory", "arguments": {"path": "/tmp"}}),
    )
    .await;
    assert!(
        body.get("result").is_some(),
        "listing /tmp must work: {body}"
    );
    let (_, body) = rpc(
        &mut r,
        Some(&token),
        "tools/call",
        json!({"name": "files.list_directory", "arguments": {"path": "/etc"}}),
    )
    .await;
    assert!(body.get("error").is_some());

    // knowledge.search without Ollama surfaces a tool error, never a panic.
    let (_, body) = rpc(
        &mut r,
        Some(&token),
        "tools/call",
        json!({"name": "knowledge.search", "arguments": {"query": "backup"}}),
    )
    .await;
    assert!(body.get("error").is_some(), "no Ollama here: {body}");
}

// ── Catalog phase B ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_install_review_shape() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "u", "pw012345", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "u", "pw012345").await;
    let s = session(&token);

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog/homevault:jellyfin/install-review",
        &[(&s.0, &s.1)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let review = &body["data"];
    assert_eq!(review["id"], json!("homevault:jellyfin"));
    assert!(review.get("dependencies").is_some());
    assert!(review.get("environment").is_some());
    assert!(review.get("incompatibility_reason").is_some());
    assert!(review.get("default_port").is_some());

    // Unknown app → error.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog/homevault:nope/install-review",
        &[(&s.0, &s.1)],
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));

    // No session → rejected.
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog/homevault:jellyfin/install-review",
        &[],
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false));
}

#[tokio::test]
async fn test_update_all_requires_admin_and_reports_empty() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "plain", "pw012345", 1).await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let user = login(&mut r, "plain", "pw012345").await;
    let admin = login(&mut r, "admin", "pw012345").await;

    let su = session(&user);
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/catalog/update-all",
        &[(&su.0, &su.1)],
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false), "non-admin must be refused");

    // Nothing installed → empty queue, still success.
    let sa = session(&admin);
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/catalog/update-all",
        &[(&sa.0, &sa.1)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["updated"], json!([]));
    assert_eq!(body["data"]["failed"], json!([]));
}

#[tokio::test]
async fn test_catalog_logs_requires_installed() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "u", "pw012345", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "u", "pw012345").await;
    let s = session(&token);
    let (_, body) = send(
        &mut r,
        Method::GET,
        "/api/docker/catalog/homevault:jellyfin/logs",
        &[(&s.0, &s.1)],
        None,
    )
    .await;
    assert_eq!(body["success"], json!(false), "not installed → error");
}

#[tokio::test]
async fn test_install_rejects_unknown_alternative_without_docker() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin", "pw012345", 2).await;
    let mut r = router(state);
    let admin = login(&mut r, "admin", "pw012345").await;
    let s = session(&admin);
    // jellyfin has no dependencies, so any alternative override is a 400 —
    // reached before any Docker call.
    let (_, body) = send(
        &mut r,
        Method::POST,
        "/api/docker/catalog/homevault:jellyfin/install",
        &[(&s.0, &s.1)],
        Some(json!({"alternatives": {"postgres": "homevault:postgres"}})),
    )
    .await;
    assert_eq!(body["success"], json!(false));
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("does not depend"));
}
