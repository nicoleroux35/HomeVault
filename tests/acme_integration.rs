/* =============================================================================
 * File          : acme_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for certificate automation: guard behaviour, validation
 *   before a CA is ever contacted, redaction of the provider token, and the
 *   public HTTP-01 challenge route.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test acme_integration
 * =============================================================================
 */

// These tests mutate `DATA_DIRECTORY`, so the whole file serialises on one
// process-wide lock and holds it across await points on purpose.
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
use homevault::handlers::acme::{
    handle_acme_challenge, handle_acme_confirm, handle_acme_issue, handle_acme_settings_get,
    handle_acme_settings_update, handle_acme_test_challenge,
};
use homevault::handlers::auth::handle_login;
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
            "/api/settings/acme",
            get(handle_acme_settings_get).put(handle_acme_settings_update),
        )
        .route("/api/settings/acme/issue", post(handle_acme_issue))
        .route("/api/settings/acme/confirm", post(handle_acme_confirm))
        .route(
            "/api/settings/acme/test-challenge",
            post(handle_acme_test_challenge),
        )
        .route(
            "/.well-known/acme-challenge/{token}",
            get(handle_acme_challenge),
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
    .execute(&*state.db.pool())
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

/// Every automation endpoint, so the guard test cannot miss one.
fn endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (Method::GET, "/api/settings/acme", None),
        (Method::PUT, "/api/settings/acme", Some(full_settings())),
        (Method::POST, "/api/settings/acme/issue", None),
        (Method::POST, "/api/settings/acme/confirm", None),
        (Method::POST, "/api/settings/acme/test-challenge", None),
    ]
}

fn full_settings() -> Value {
    json!({
        "enabled": true,
        "domain": "nas.example.com",
        "directory_url": "",
        "contact_email": "admin@example.com",
        "challenge": "http-01",
        "provider": "manual",
        "dns_api_token": "",
        "tos_agreed": true,
        "dns_propagation_seconds": 60,
        "status": "idle",
        "last_error": null,
        "last_attempt": null,
        "last_success": null,
        "account_url": null,
        "pending_dns_record": null,
        "consecutive_failures": 0
    })
}

#[tokio::test]
async fn every_endpoint_refuses_an_anonymous_caller_with_401() {
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
async fn an_admin_reads_the_defaults_with_a_redacted_token() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/acme",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["enabled"], false);
    assert_eq!(data["status"], "disabled");
    assert_eq!(data["dns_api_token"], "");
    assert_eq!(
        data["default_directory"],
        homevault::acme::DEFAULT_DIRECTORY_URL
    );
    assert_eq!(data["providers"][0], "manual");
    assert_eq!(data["run_in_progress"], false);
}

#[tokio::test]
async fn saving_an_incomplete_configuration_is_refused_before_any_ca_call() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let mut body = full_settings();
    body["domain"] = json!("");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/acme",
        Some(&token),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("domain name is required"));

    let mut no_tos = full_settings();
    no_tos["tos_agreed"] = json!(false);
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/acme",
        Some(&token),
        Some(no_tos),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"].as_str().unwrap().contains("terms"));
}

#[tokio::test]
async fn a_saved_configuration_round_trips_without_ever_returning_the_token() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let mut body = full_settings();
    body["challenge"] = json!("dns-01");
    body["provider"] = json!("cloudflare");
    body["dns_api_token"] = json!("cloudflare-secret");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/acme",
        Some(&token),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"]["settings"]["dns_api_token"], "set");
    assert!(!resp.to_string().contains("cloudflare-secret"));

    // `"set"` means "keep what is stored", not "store the word set".
    let mut again = body.clone();
    again["contact_email"] = json!("ops@example.com");
    again["dns_api_token"] = json!("set");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/acme",
        Some(&token),
        Some(again),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"]["settings"]["dns_api_token"], "set");
    let stored = homevault::acme::load_settings();
    assert_eq!(stored.dns_api_token, "cloudflare-secret");
    assert_eq!(stored.contact_email, "ops@example.com");

    // An empty token clears it — but only once the provider no longer needs
    // one, which is exactly what validation enforces.
    let mut cleared = body.clone();
    cleared["dns_api_token"] = json!("");
    let (status, resp) = send(
        &mut r,
        Method::PUT,
        "/api/settings/acme",
        Some(&token),
        Some(cleared.clone()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a Cloudflare configuration without a token must be refused: {resp}"
    );

    cleared["provider"] = json!("manual");
    cleared["challenge"] = json!("http-01");
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/acme",
        Some(&token),
        Some(cleared),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(homevault::acme::load_settings().dns_api_token.is_empty());
}

#[tokio::test]
async fn issuing_while_disabled_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, resp) = send(
        &mut r,
        Method::POST,
        "/api/settings/acme/issue",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"]
        .as_str()
        .unwrap()
        .contains("enable certificate automation"));
}

#[tokio::test]
async fn confirming_without_a_pending_record_is_refused() {
    let (state, _tmp, _guard) = setup_state().await;
    create_user(&state, "admin", "AdminPass123!", 2).await;
    let mut r = router(state);
    let token = login(&mut r, "admin", "AdminPass123!").await;

    let (status, resp) = send(
        &mut r,
        Method::POST,
        "/api/settings/acme/confirm",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"].as_str().unwrap().contains("no DNS record"));
}

#[tokio::test]
async fn the_challenge_route_answers_only_a_token_that_is_pending() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);

    let (status, _) = send(
        &mut r,
        Method::GET,
        "/.well-known/acme-challenge/unknown-token",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    homevault::acme::register_challenge("test-token", "test-token.thumbprint").unwrap();
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/.well-known/acme-challenge/test-token",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, Value::Null, "the answer is plain text, not JSON");

    homevault::acme::remove_challenge("test-token");
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/.well-known/acme-challenge/test-token",
        None,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the answer is dropped after use"
    );
}

#[tokio::test]
async fn the_challenge_route_rejects_a_traversal_style_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    for uri in [
        "/.well-known/acme-challenge/..%2fsettings",
        "/.well-known/acme-challenge/",
        "/.well-known/acme-challenge/a.b",
    ] {
        let (status, _) = send(&mut r, Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn the_plain_text_answer_carries_no_cache_and_no_store() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut r = router(state);
    homevault::acme::register_challenge("cached-token", "answer").unwrap();
    let resp = ServiceExt::<Req>::ready(&mut r)
        .await
        .unwrap()
        .call(
            Request::builder()
                .method(Method::GET)
                .uri("/.well-known/acme-challenge/cached-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"answer");
    homevault::acme::remove_challenge("cached-token");
}
