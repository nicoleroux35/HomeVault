/* =============================================================================
 * File          : email_oauth_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SMTP OAuth2 integration tests (P1-16): in-memory SQLite + assembled Axum
 *   router, exercising the admin gate, configuration validation, the sealed
 *   file, the authorization URL and the callback's state-cookie guard.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test email_oauth_integration
 * =============================================================================
 */

// The OAuth2 store is a process-global file under DATA_DIRECTORY, so tests
// serialise on this lock (same pattern as the other settings suites).
#![allow(clippy::await_holding_lock)]
//! Integration tests for `/api/settings/email/oauth`.
//!
//! **No test reaches a provider.** Every case below stops before the token
//! endpoint: validation, the admin gate, the authorization URL and the
//! callback's state check are all local. The HTTP exchange itself is not
//! exercised by this suite — a real connection needs a provider account.

use axum::{
    routing::{get, post},
    Router,
};
use http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};
use url::Url;

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::email_oauth::{
    handle_email_oauth_callback, handle_email_oauth_disconnect, handle_email_oauth_get,
    handle_email_oauth_start, handle_email_oauth_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Harness {
    state: Arc<AppState>,
    data_dir: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let data_dir = tmp.path().to_path_buf();
    std::env::set_var("DATA_DIRECTORY", &data_dir);
    // The OAuth2 store is a file, not a DB row: clear a previous test's file.
    let _ = homevault::email_oauth::disconnect();

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
        data_dir,
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route(
            "/api/settings/email/oauth",
            get(handle_email_oauth_get)
                .put(handle_email_oauth_update)
                .delete(handle_email_oauth_disconnect),
        )
        .route(
            "/api/settings/email/oauth/start",
            post(handle_email_oauth_start),
        )
        .route(
            "/api/settings/email/oauth/callback",
            get(handle_email_oauth_callback),
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
    .bind(password_hash)
    .bind(role_id)
    .execute(state.db.pool().as_ref())
    .await
    .unwrap();
}

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"]["token"].as_str().unwrap().to_string()
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("x-session-token", token);
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(axum::body::Body::empty()).unwrap(),
    };
    let response = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Like [`send`], but returns the raw body and headers (the callback answers
/// HTML and the start endpoint sets a cookie).
async fn send_raw(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("x-session-token", token);
    }
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let request = builder.body(axum::body::Body::empty()).unwrap();
    let response = ServiceExt::<Req>::ready(router)
        .await
        .unwrap()
        .call(request)
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

fn valid_config() -> Value {
    json!({
        "provider": "google",
        "client_id": "123.apps.googleusercontent.com",
        "client_secret": "s3cr3t-client-value",
        "redirect_url": "https://nas.example.com/api/settings/email/oauth/callback"
    })
}

// ---------------------------------------------------------------------------
// 1. Admin gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn email_oauth_endpoints_are_admin_only() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    create_test_user(&h.state, "user1", "Password123!", 1).await;
    let mut r = router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;
    let user = login(&mut r, "user1", "Password123!").await;

    for (method, uri, body) in [
        (Method::GET, "/api/settings/email/oauth", None),
        (
            Method::PUT,
            "/api/settings/email/oauth",
            Some(valid_config()),
        ),
        (Method::DELETE, "/api/settings/email/oauth", None),
        (
            Method::POST,
            "/api/settings/email/oauth/start",
            Some(json!({})),
        ),
    ] {
        let (status, _) = send(&mut r, method.clone(), uri, None, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut r, method, uri, Some(&user), None, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }

    // The admin is allowed; the callback is public by design (state-cookie
    // guarded), so it is not part of this loop.
    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["oauth"]["configured"], json!(false));
    assert_eq!(body["data"]["oauth"]["connected"], json!(false));
}

// ---------------------------------------------------------------------------
// 2. Configuration validation and secret handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn email_oauth_configuration_is_validated_and_never_returns_a_secret() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    for bad in [
        json!({"provider": "yahoo"}),
        json!({"redirect_url": "ftp://nas.example.com/cb"}),
        json!({"redirect_url": "/relative/callback"}),
        json!({"client_id": "bad id with spaces"}),
    ] {
        let mut config = valid_config();
        for (key, value) in bad.as_object().unwrap() {
            config[key] = value.clone();
        }
        let (status, body) = send(
            &mut r,
            Method::PUT,
            "/api/settings/email/oauth",
            Some(&admin),
            None,
            Some(config),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(valid_config()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["oauth"]["configured"], json!(true));
    assert_eq!(body["data"]["oauth"]["connected"], json!(false));
    assert_eq!(
        body["data"]["oauth"]["redirect_url"],
        json!("https://nas.example.com/api/settings/email/oauth/callback")
    );
    assert_eq!(body["data"]["oauth"]["auth_method"], json!("oauthbearer"));
    // The response must never carry a secret or a token.
    let text = body.to_string();
    assert!(!text.contains("s3cr3t-client-value"), "{text}");
    assert!(!text.contains("client_secret"), "{text}");
    assert!(!text.contains("refresh_token"), "{text}");
    assert!(!text.contains("hv1:"), "{text}");

    // A blank secret keeps the stored one: the card can change the redirect URL
    // without re-typing a credential the API never shows it.
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(json!({
            "provider": "google",
            "client_id": "123.apps.googleusercontent.com",
            "redirect_url": "https://nas.example.com/other-callback"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["oauth"]["configured"], json!(true));

    // Changing the provider invalidates the registration: the card says
    // "ready", never "connected" with the previous provider's token.
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(json!({
            "provider": "microsoft",
            "client_id": "00000000-0000-0000-0000-000000000000",
            "redirect_url": "https://nas.example.com/api/settings/email/oauth/callback"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["oauth"]["provider"], json!("microsoft"));
    assert_eq!(
        body["data"]["oauth"]["smtp_host"],
        json!("smtp.office365.com")
    );
    assert_eq!(body["data"]["oauth"]["auth_method"], json!("xoauth2"));
    assert_eq!(body["data"]["oauth"]["connected"], json!(false));
}

#[tokio::test]
async fn email_oauth_file_is_sealed_on_disk() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(valid_config()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let path = h.data_dir.join("data").join("email_oauth.json");
    let raw = std::fs::read_to_string(&path).expect("the configuration file exists");
    // The secret is sealed, not written in clear. The client id is public.
    assert!(raw.contains("hv1:"), "{raw}");
    assert!(!raw.contains("s3cr3t-client-value"), "{raw}");
    assert!(raw.contains("123.apps.googleusercontent.com"), "{raw}");
}

// ---------------------------------------------------------------------------
// 3. Authorization start and callback state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn email_oauth_start_requires_a_configuration_and_returns_the_provider_url() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/settings/email/oauth/start",
        Some(&admin),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"]
        .as_str()
        .is_some_and(|m| m.contains("redirect URL")));

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(valid_config()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, headers, raw) = send_raw(
        &mut r,
        Method::POST,
        "/api/settings/email/oauth/start",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let body: Value = serde_json::from_str(&raw).unwrap();
    let url = Url::parse(body["data"]["authorization_url"].as_str().unwrap()).unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("accounts.google.com"));
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(
        params.get("access_type").map(String::as_str),
        Some("offline")
    );
    assert!(params.get("state").is_some_and(|s| !s.is_empty()));
    assert_eq!(
        params.get("redirect_uri").map(String::as_str),
        Some("https://nas.example.com/api/settings/email/oauth/callback")
    );

    // The state is also set as a browser-bound, HttpOnly cookie.
    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(cookie.contains("hv_email_oauth_state="), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
}

#[tokio::test]
async fn email_oauth_callback_refuses_an_unbound_or_expired_state() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    // No state cookie at all: a prepared link must not complete a connection.
    let (status, _, body) = send_raw(
        &mut r,
        Method::GET,
        "/api/settings/email/oauth/callback?state=abc&code=xyz",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("did not start"), "{body}");

    // A provider refusal is reported, and the one-time state is consumed.
    let (status, _, body) = send_raw(
        &mut r,
        Method::GET,
        "/api/settings/email/oauth/callback?state=abc&error=access_denied",
        None,
        Some("hv_email_oauth_state=abc"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("did not grant access"), "{body}");

    // A well-formed flow, then a callback with no code: the state is valid, so
    // the answer is about the missing code, and the state cannot be replayed.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(valid_config()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, headers, raw) = send_raw(
        &mut r,
        Method::POST,
        "/api/settings/email/oauth/start",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{raw}");
    let body: Value = serde_json::from_str(&raw).unwrap();
    let url = Url::parse(body["data"]["authorization_url"].as_str().unwrap()).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let state = params.get("state").unwrap().clone();
    let cookie = headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .to_string();
    assert!(cookie.starts_with("hv_email_oauth_state="), "{cookie}");

    let (status, _, body) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/settings/email/oauth/callback?state={state}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("authorization code"), "{body}");

    // Replaying the same state is refused.
    let (status, _, body) = send_raw(
        &mut r,
        Method::GET,
        &format!("/api/settings/email/oauth/callback?state={state}&code=replay"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("expired"), "{body}");
}

// ---------------------------------------------------------------------------
// 4. Disconnect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn email_oauth_disconnect_clears_the_registration_and_the_file() {
    let h = setup().await;
    create_test_user(&h.state, "admin1", "Password123!", 2).await;
    let mut r = router(h.state.clone());
    let admin = login(&mut r, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        Some(valid_config()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["oauth"]["configured"], json!(false));
    assert_eq!(body["data"]["oauth"]["connected"], json!(false));
    assert!(!h.data_dir.join("data").join("email_oauth.json").exists());

    let (status, body) = send(
        &mut r,
        Method::GET,
        "/api/settings/email/oauth",
        Some(&admin),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["oauth"]["client_id"], json!(""));
}
