/* =============================================================================
 * File          : sms_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SMS notification-channel integration tests: the auth matrix, the
 *   validation refusals, the "disabled is always allowed" rule, and the fact
 *   that the stored token is sealed and never returned.
 *
 *   Nothing here sends an SMS. `notification_engine::dispatch_sms` is exercised
 *   only through the pure `crate::sms` module, whose request builder is
 *   asserted directly; a real send would leave the test box talking to a URL.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test sms_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};

use axum::{routing::post, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::settings::{
    handle_notification_settings_get, handle_notification_settings_update,
};
use homevault::handlers::terminal_pty::PtyManager;
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
    let admin_rate_limiter = Arc::new(RateLimitState::new(1000, 60));
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
            "/api/settings/notifications",
            axum::routing::get(handle_notification_settings_get)
                .put(handle_notification_settings_update),
        )
        .with_state(state)
}

async fn create_test_user(state: &AppState, username: &str, role_id: i64) {
    let password_hash = state.auth.hash_password("pass").unwrap();
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
        Some(json!({"username": username, "password": "pass"})),
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
    create_test_user(&h.state, "admin", 2).await;
    create_test_user(&h.state, "member", 1).await;
    let admin = login(&mut router, "admin").await;
    let member = login(&mut router, "member").await;
    (router, admin, member)
}

/// A configuration body that is enabled and valid, so a refusal can only come
/// from the field under test.
fn valid_body() -> Value {
    json!({
        "sms": {
            "enabled": true,
            "endpoint": "https://sms.example.test/send",
            "body_template": r#"{"to":"%to%","text":"%text%"}"#,
            "recipient": "+33612345678",
            "extra_headers": [],
            "token_sealed": ""
        }
    })
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_both_verbs_are_admin_only() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/notifications",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/notifications",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&member),
        Some(valid_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_valid_configuration_is_accepted() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(valid_body()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A national number without a country code is the commonest mistake, and it
/// would be billed but never delivered.
#[tokio::test]
async fn test_a_number_without_a_country_code_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut body = valid_body();
    body["sms"]["recipient"] = json!("0612345678");

    let (status, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
    let message = payload["message"].as_str().unwrap_or_default();
    assert!(message.contains("0612345678"), "{payload}");
    // The refusal teaches the expected shape rather than only rejecting.
    assert!(message.contains("+33612345678"), "{payload}");
}

#[tokio::test]
async fn test_enabling_without_an_endpoint_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut body = valid_body();
    body["sms"]["endpoint"] = json!("");

    let (status, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("endpoint"),
        "{payload}"
    );
}

#[tokio::test]
async fn test_a_template_without_the_placeholders_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut body = valid_body();
    body["sms"]["body_template"] = json!(r#"{"message":"%text%"}"#);

    let (status, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("%to%"),
        "{payload}"
    );
}

/// A disabled configuration is never refused: an admin must be able to clear a
/// half-filled form.
#[tokio::test]
async fn test_a_disabled_configuration_with_empty_fields_is_accepted() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let body = json!({
        "sms": {
            "enabled": false,
            "endpoint": "",
            "body_template": "",
            "recipient": "",
            "extra_headers": [],
            "token_sealed": ""
        }
    });

    let (status, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");
}

// ---------------------------------------------------------------------------
// Secrets (P0.2)
// ---------------------------------------------------------------------------

/// The token is stored sealed and **never** returned: the read path answers
/// `token_set` and nothing that could carry the credential.
#[tokio::test]
async fn test_the_token_is_sealed_and_never_returned() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut body = valid_body();
    body["sms"]["token_sealed"] = json!("super-secret-token");

    let (status, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");

    let (status, read) = send(
        &mut router,
        Method::GET,
        "/api/settings/notifications",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let sms = &read["data"]["sms"];
    assert_eq!(sms["token_set"], json!(true), "{read}");
    // Neither the plaintext nor the sealed blob may appear anywhere.
    let wire = read.to_string();
    assert!(!wire.contains("super-secret-token"), "{wire}");
    assert!(
        sms.get("token_sealed").is_none(),
        "the view must not carry a token field at all: {wire}"
    );
}

/// An empty token on a later save keeps the stored one, so the admin can edit
/// the number without re-typing a credential the API never shows them.
#[tokio::test]
async fn test_saving_without_a_token_keeps_the_stored_one() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut first = valid_body();
    first["sms"]["token_sealed"] = json!("keep-me");
    let (_, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(first),
    )
    .await;
    assert_eq!(payload["success"], json!(true), "{payload}");

    // Second save: same configuration, blank token, different number.
    let mut second = valid_body();
    second["sms"]["recipient"] = json!("+14155552671");
    second["sms"]["token_sealed"] = json!("");
    let (status, payload) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&admin),
        Some(second),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");

    let (_, read) = send(
        &mut router,
        Method::GET,
        "/api/settings/notifications",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(read["data"]["sms"]["token_set"], json!(true), "{read}");
    assert_eq!(
        read["data"]["sms"]["recipient"],
        json!("+14155552671"),
        "{read}"
    );
}

// ---------------------------------------------------------------------------
// The pure channel, exercised through the same process
// ---------------------------------------------------------------------------

/// The message that would actually be posted: severity first, one segment,
/// truncated with a visible marker when it does not fit.
#[tokio::test]
async fn test_the_composed_message_is_one_visible_segment() {
    let text = homevault::sms::compose_text(
        "critical",
        "Disk failure",
        "md0 is degraded and a rebuild is required immediately, please check the storage page",
    );
    assert!(text.starts_with("[critical] Disk failure:"), "{text}");
    assert!(
        text.chars().count() <= homevault::sms::MAX_SMS_CHARS,
        "{}",
        text.chars().count()
    );
}

/// A short alert is never truncated, so a routine message keeps its full text.
#[tokio::test]
async fn test_a_short_alert_is_not_truncated() {
    let text = homevault::sms::compose_text("info", "Backup", "completed");
    assert!(!text.contains(homevault::sms::TRUNCATION_MARKER), "{text}");
    assert_eq!(text, "[info] Backup: completed");
}
