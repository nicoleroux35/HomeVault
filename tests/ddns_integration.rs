/* =============================================================================
 * File          : ddns_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for dynamic DNS and the reachability advisor: the guard
 *   on every route, the validation that happens before any provider is
 *   contacted, the redaction of the provider token, and the shape of the
 *   advisor report.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test ddns_integration
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::ddns::{
    handle_ddns_settings_get, handle_ddns_settings_update, handle_ddns_update_now,
};
use homevault::handlers::reachability::handle_reachability_get;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
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
            "/api/settings/ddns",
            get(handle_ddns_settings_get).put(handle_ddns_settings_update),
        )
        .route("/api/settings/ddns/update", post(handle_ddns_update_now))
        .route("/api/settings/reachability", get(handle_reachability_get))
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

fn endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/settings/ddns", None),
        (
            Method::PUT,
            "/api/settings/ddns",
            Some(json!({ "enabled": false })),
        ),
        (Method::POST, "/api/settings/ddns/update", None),
        (Method::GET, "/api/settings/reachability", None),
    ]
}

fn duckdns_settings() -> Value {
    json!({
        "enabled": true,
        "provider": "duckdns",
        "hostname": "myhome.duckdns.org",
        "token": "duck-token",
        "custom_update_url": "",
        "status": "disabled",
        "last_ip": null,
        "last_update": null,
        "last_error": null
    })
}

#[tokio::test]
async fn every_ddns_endpoint_refuses_an_anonymous_caller_with_401() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    for (method, uri, body) in endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, None, body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn a_non_admin_session_gets_403() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "regular", "Passw0rd!123", 1).await;
    let mut r = router(state);
    let token = login(&mut r, "regular", "Passw0rd!123").await;
    for (method, uri, body) in endpoints() {
        let (status, _) = send(&mut r, method.clone(), uri, Some(&token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
}

#[tokio::test]
async fn an_admin_reads_the_defaults_with_the_token_redacted() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/ddns",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["enabled"], false);
    assert_eq!(data["status"], "disabled");
    assert_eq!(data["token"], "");
    assert_eq!(data["providers"][0], "duckdns");
    assert_eq!(data["providers"][3], "custom");
    // The detected address is reported (null on a host with no route out) so
    // the card can show "unknown" instead of guessing.
    assert!(data.get("current_ip").is_some());
}

#[tokio::test]
async fn an_enabled_configuration_is_validated_before_any_provider_is_contacted() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let mut body = duckdns_settings();
    body["token"] = json!("");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/ddns",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"].as_str().unwrap().contains("API token"));

    let mut body = duckdns_settings();
    body["hostname"] = json!("https://myhome.duckdns.org");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/ddns",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"].as_str().unwrap().contains("bare host name"));

    // A disabled entry may stay incomplete: the admin can save half a form and
    // come back to it.
    let mut body = duckdns_settings();
    body["enabled"] = json!(false);
    body["hostname"] = json!("");
    body["token"] = json!("");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/ddns",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
}

#[tokio::test]
async fn a_saved_configuration_round_trips_without_ever_returning_the_token() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/ddns",
        Some(&token),
        Some(duckdns_settings()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"]["settings"]["token"], "set");
    assert!(
        !resp.to_string().contains("duck-token"),
        "the provider token must never come back: {resp}"
    );

    // `"set"` keeps the stored value; an empty string clears it.
    let mut again = duckdns_settings();
    again["token"] = json!("set");
    again["enabled"] = json!(false);
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/ddns",
        Some(&token),
        Some(again),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(homevault::ddns::load_settings().token, "duck-token");

    // Changing the provider invalidates the recorded result.
    let mut switched = duckdns_settings();
    switched["token"] = json!("set");
    switched["provider"] = json!("dynv6");
    switched["hostname"] = json!("home.dynv6.net");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/ddns",
        Some(&token),
        Some(switched),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"]["settings"]["last_ip"], Value::Null);
}

#[tokio::test]
async fn updating_while_disabled_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, resp) = send(
        &mut r,
        Method::POST,
        "/api/settings/ddns/update",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("enable dynamic DNS"));
}

#[tokio::test]
async fn the_reachability_report_lists_ordered_checks_with_their_fixes() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/reachability",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["checks"][0]["id"], "tls.enabled");
    // HTTPS is off by default, so the report must say so first and say what to
    // do about it.
    assert_eq!(data["checks"][0]["state"], "fail");
    assert!(data["checks"][0]["fix"].as_str().unwrap().contains("HTTPS"));
    assert_eq!(data["state"], "fail");
    assert!(data["counts"]["fail"].as_u64().unwrap() >= 1);
    let total = data["counts"]["ok"].as_u64().unwrap()
        + data["counts"]["warn"].as_u64().unwrap()
        + data["counts"]["fail"].as_u64().unwrap();
    assert_eq!(total as usize, data["checks"].as_array().unwrap().len());
}
