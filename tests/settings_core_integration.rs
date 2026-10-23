/* =============================================================================
 * File          : settings_core_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings core integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the **Settings core** — `crate::handlers::settings`
//! (everything except the AI Assistant card and the 3 security cards, which
//! have their own suites):
//!
//!   Reads (admin only):
//!     GET  /api/settings                      (aggregate)
//!     GET  /api/settings/appearance | /email | /notifications | /update
//!     GET  /api/settings/export
//!   Writes (admin only):
//!     PUT  /api/settings/appearance | /email | /notifications | /update
//!     POST /api/settings/import | /reset
//!     POST /api/settings/language
//!   Any session:
//!     GET  /api/settings/language
//!   Public (no auth):
//!     GET  /api/settings/public
//!
//! Fresh in-memory SQLite + all migrations per test, a fresh
//! `tempfile::tempdir()` wired into `settings_persistence::init_persistence`,
//! a minimal Axum router, `tower::ServiceExt` for HTTP-level testing — no
//! real server.
//!
//! The `*_SETTINGS` statics and the persistence singleton are process-global,
//! so every test serialises behind `ENV_LOCK` and calls `reset_settings()`
//! first. Nothing here shells out: the one handler that would
//! (`/api/settings/general` → `hostnamectl`) is deliberately not covered
//! here — its display-name sanitiser is unit-tested in
//! `handlers::settings::tests` instead.
//!
//! Run:
//!   cargo test --test settings_core_integration

use axum::{
    routing::{get, post},
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
use homevault::handlers::settings::{
    handle_all_settings_get, handle_appearance_settings_get, handle_appearance_settings_update,
    handle_check_version_now, handle_email_settings_get, handle_email_settings_update,
    handle_get_language, handle_notification_settings_get, handle_notification_settings_update,
    handle_public_settings_get, handle_set_language, handle_settings_export,
    handle_settings_import, handle_settings_reset, handle_update_settings_get,
    handle_update_settings_update, handle_version_info, update_check_attempts, UpdateFrequency,
    CACHED_UPDATE_RESULT, LAST_UPDATE_CHECK, UPDATE_SETTINGS,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
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

    // Point the persistence singleton at this test's throwaway dir, then wipe
    // any in-memory settings a previous test in this binary may have left.
    homevault::settings_persistence::init_persistence(tmp.path().to_string_lossy().to_string());
    let _ = homevault::settings_persistence::reset_settings();

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

fn settings_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/settings", get(handle_all_settings_get))
        .route("/api/settings/public", get(handle_public_settings_get))
        .route(
            "/api/settings/appearance",
            get(handle_appearance_settings_get).put(handle_appearance_settings_update),
        )
        .route(
            "/api/settings/email",
            get(handle_email_settings_get).put(handle_email_settings_update),
        )
        .route(
            "/api/settings/notifications",
            get(handle_notification_settings_get).put(handle_notification_settings_update),
        )
        .route(
            "/api/settings/update",
            get(handle_update_settings_get).put(handle_update_settings_update),
        )
        .route(
            "/api/settings/language",
            get(handle_get_language).post(handle_set_language),
        )
        .route("/api/settings/export", get(handle_settings_export))
        .route("/api/settings/import", post(handle_settings_import))
        .route("/api/settings/reset", post(handle_settings_reset))
        .route("/api/settings/version", get(handle_version_info))
        .route(
            "/api/settings/version/check",
            post(handle_check_version_now),
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
        .unwrap();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

/// Every read endpoint that requires an **admin** session.
fn admin_read_endpoints() -> Vec<&'static str> {
    vec![
        "/api/settings",
        "/api/settings/appearance",
        "/api/settings/email",
        "/api/settings/notifications",
        "/api/settings/update",
        "/api/settings/export",
    ]
}

/// (method, uri, body) for every write endpoint — all admin-only. Bodies are
/// the minimal valid shape so the request reaches the handler's auth check.
fn admin_write_endpoints() -> Vec<(Method, &'static str, Value)> {
    vec![
        (Method::PUT, "/api/settings/appearance", json!({})),
        (Method::PUT, "/api/settings/email", json!({})),
        (Method::PUT, "/api/settings/notifications", json!({})),
        (Method::PUT, "/api/settings/update", json!({})),
        (
            Method::POST,
            "/api/settings/language",
            json!({"language": "en"}),
        ),
        (Method::POST, "/api/settings/import", json!({"data": {}})),
        (Method::POST, "/api/settings/reset", json!({})),
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reads_reject_missing_and_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = settings_router(state);
    for uri in admin_read_endpoints() {
        let (status, body) = send(&mut router, Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "GET {uri} no token");
        assert_eq!(body["success"], json!(false), "GET {uri} no token");

        let (status, _) = send(&mut router, Method::GET, uri, Some("nope"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "GET {uri} bogus token");
    }
}

#[tokio::test]
async fn test_writes_reject_missing_and_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = settings_router(state);
    for (method, uri, body) in admin_write_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, None, Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri} no token");

        let (status, _) = send(
            &mut router,
            method.clone(),
            uri,
            Some("nope"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} bogus token"
        );
    }
}

#[tokio::test]
async fn test_reads_and_writes_reject_a_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "regular", "Password123!").await;

    for uri in admin_read_endpoints() {
        let (status, body) = send(&mut router, Method::GET, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "GET {uri} non-admin");
        assert_eq!(body["success"], json!(false));
        assert!(
            body["message"].as_str().unwrap_or("").contains("Admin"),
            "GET {uri}: {body}"
        );
    }
    for (method, uri, b) in admin_write_endpoints() {
        let (status, body) = send(&mut router, method.clone(), uri, Some(&token), Some(b)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri} non-admin");
        assert!(body["message"].as_str().unwrap_or("").contains("Admin"));
    }
}

#[tokio::test]
async fn test_get_language_allows_any_session_but_still_needs_one() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = settings_router(state);

    let (status, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/language",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let token = login(&mut router, "regular", "Password123!").await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/language",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["language"].is_string());
}

#[tokio::test]
async fn test_public_settings_need_no_auth_and_leak_nothing_sensitive() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = settings_router(state);

    let (status, body) = send(&mut router, Method::GET, "/api/settings/public", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["server_name"], "HomeVault");
    assert!(body["data"]["version"].is_string());
    // No email / smtp / password / accent surface here.
    let s = body.to_string().to_lowercase();
    assert!(!s.contains("smtp") && !s.contains("password"), "{body}");
}

// ---------------------------------------------------------------------------
// 2. Appearance round-trip (accent / density / wallpaper — the untested gap)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_appearance_put_then_get_round_trips_all_four_fields() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let payload = json!({
        "theme": "dark",
        "wallpaper": "/wallpapers/hv-09.webp",
        "accent_color": "#e2725b",
        "density": "compact"
    });
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/settings/appearance",
        Some(&token),
        Some(payload.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));

    let (_, got) = send(
        &mut router,
        Method::GET,
        "/api/settings/appearance",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(got["data"]["theme"], "dark");
    assert_eq!(got["data"]["wallpaper"], "/wallpapers/hv-09.webp");
    assert_eq!(got["data"]["accent_color"], "#e2725b");
    assert_eq!(got["data"]["density"], "compact");

    // …and the aggregate endpoint reflects the same values.
    let (_, all) = send(
        &mut router,
        Method::GET,
        "/api/settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(all["data"]["appearance"]["accent_color"], "#e2725b");
    assert_eq!(all["data"]["appearance"]["density"], "compact");
}

#[tokio::test]
async fn test_appearance_partial_update_leaves_other_fields_untouched() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // Set everything, then PATCH just the accent.
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/appearance",
        Some(&token),
        Some(json!({"theme": "light", "density": "spacious", "accent_color": "#111111", "wallpaper": "/wallpapers/hv-01.webp"})),
    )
    .await;
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/appearance",
        Some(&token),
        Some(json!({"accent_color": "#222222"})),
    )
    .await;

    let (_, got) = send(
        &mut router,
        Method::GET,
        "/api/settings/appearance",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(got["data"]["accent_color"], "#222222");
    assert_eq!(got["data"]["theme"], "light", "unspecified field kept");
    assert_eq!(got["data"]["density"], "spacious", "unspecified field kept");
}

// ---------------------------------------------------------------------------
// 3. Export never leaks the SMTP password
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_settings_export_redacts_the_smtp_password() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let secret = "s3cr3t-smtp-do-not-leak";
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/email",
        Some(&token),
        Some(json!({
            "smtp_server": "mail.example.com",
            "smtp_port": 587,
            "smtp_username": "bot@example.com",
            "smtp_password": secret,
            "from_address": "bot@example.com",
            "use_tls": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET /api/settings/email already omits the password entirely.
    let (_, email) = send(
        &mut router,
        Method::GET,
        "/api/settings/email",
        Some(&token),
        None,
    )
    .await;
    assert!(email["data"].get("smtp_password").is_none(), "{email}");
    assert_eq!(email["data"]["smtp_server"], "mail.example.com");

    // The downloadable export redacts it to null and never carries the value.
    // `handle_settings_export` nests the settings blob one level under `data`
    // (`{success, data:{success, data:<settings>}}`), so the email block is at
    // `["data"]["data"]["email"]`.
    let (status, export) = send(
        &mut router,
        Method::GET,
        "/api/settings/export",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let blob = &export["data"]["data"];
    assert_eq!(
        blob["email"]["smtp_server"], "mail.example.com",
        "sanity: {export}"
    );
    assert_eq!(blob["email"]["smtp_password"], Value::Null);
    assert!(
        !export.to_string().contains(secret),
        "the export body must not contain the SMTP password anywhere: {export}"
    );
}

// ---------------------------------------------------------------------------
// 4. Export → import round-trip, and reset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_export_then_import_round_trips_and_keeps_the_password() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // Configure email (with a password) + a custom appearance.
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/email",
        Some(&token),
        Some(json!({"smtp_server": "mail.example.com", "smtp_username": "u", "smtp_password": "keepme-please"})),
    )
    .await;
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/appearance",
        Some(&token),
        Some(json!({"accent_color": "#abcdef", "density": "compact"})),
    )
    .await;

    // Export (password redacted), then import that same blob back. The import
    // handler wants the bare settings object, which the export nests at
    // `["data"]["data"]`.
    let (_, export) = send(
        &mut router,
        Method::GET,
        "/api/settings/export",
        Some(&token),
        None,
    )
    .await;
    let blob = export["data"]["data"].clone();
    assert!(
        blob["appearance"].is_object(),
        "sanity: exported blob shape: {export}"
    );

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/import",
        Some(&token),
        Some(json!({"data": blob})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true));

    // Appearance survived the round-trip…
    let (_, got) = send(
        &mut router,
        Method::GET,
        "/api/settings/appearance",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(got["data"]["accent_color"], "#abcdef");
    assert_eq!(got["data"]["density"], "compact");

    // …and re-exporting still shows the email server (importing a redacted
    // `null` password did not wipe the block).
    let (_, export2) = send(
        &mut router,
        Method::GET,
        "/api/settings/export",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        export2["data"]["data"]["email"]["smtp_server"],
        "mail.example.com"
    );
}

#[tokio::test]
async fn test_settings_reset_restores_the_shipped_defaults() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/appearance",
        Some(&token),
        Some(json!({"accent_color": "#ff0000", "density": "compact"})),
    )
    .await;
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/update",
        Some(&token),
        Some(json!({"check_frequency": "weekly"})),
    )
    .await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/settings/reset",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, appearance) = send(
        &mut router,
        Method::GET,
        "/api/settings/appearance",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(appearance["data"]["accent_color"], "#1E62AE");
    assert_eq!(appearance["data"]["density"], "comfortable");

    // G2: the update-check default is back to Manual (no auto GitHub call).
    let (_, update) = send(
        &mut router,
        Method::GET,
        "/api/settings/update",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(update["data"]["check_frequency"], "manual");
}

#[tokio::test]
async fn test_appearance_put_rewrites_the_legacy_default_accent() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // The legacy default (#4a90d9) fails WCAG 1.4.3 both behind white
    // button text (3.34:1) and as text on the light surfaces — every path
    // that can introduce it rewrites it to the compliant HomeVault Blue.
    let _ = send(
        &mut router,
        Method::PUT,
        "/api/settings/appearance",
        Some(&token),
        Some(json!({"accent_color": "#4a90d9"})),
    )
    .await;

    let (_, appearance) = send(
        &mut router,
        Method::GET,
        "/api/settings/appearance",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(appearance["data"]["accent_color"], "#1E62AE");

    // The aggregate every Settings window seeds from agrees.
    let (_, all) = send(
        &mut router,
        Method::GET,
        "/api/settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(all["data"]["appearance"]["accent_color"], "#1E62AE");
}

// ---------------------------------------------------------------------------
// 5. Language + notification/update validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_set_language_accepts_supported_and_rejects_unknown() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for lang in ["en", "fr", "de", "es"] {
        let (status, _) = send(
            &mut router,
            Method::POST,
            "/api/settings/language",
            Some(&token),
            Some(json!({"language": lang})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "set language {lang}");
    }

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/language",
        Some(&token),
        Some(json!({"language": "pt"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));

    // Put it back so we don't leave the process on a non-English locale for
    // other tests in this binary.
    let _ = send(
        &mut router,
        Method::POST,
        "/api/settings/language",
        Some(&token),
        Some(json!({"language": "en"})),
    )
    .await;
}

#[tokio::test]
async fn test_notification_threshold_is_clamped_and_round_trips() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/notifications",
        Some(&token),
        Some(json!({"email": "alerts@example.com", "alert_threshold": 250, "enable_alerts": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, got) = send(
        &mut router,
        Method::GET,
        "/api/settings/notifications",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(got["data"]["email"], "alerts@example.com");
    assert_eq!(
        got["data"]["alert_threshold"], 100,
        "a u8 threshold above 100 is clamped"
    );
}

// ===========================================================================
// ROADMAP 13.1 — the update check is opt-in and enforced server-side
//
// The privacy baseline (CLAUDE.md → Compliance, point 1) says HomeVault makes
// no outbound third-party call the admin did not ask for. These tests pin the
// three ways it could happen: opening Settings (GET), a saved frequency, and
// the explicit "Check now" (POST). The update endpoint is redirected to a
// local listener through `HOMEVAULT_VERSION_CHECK_URL`, so "zero outbound
// attempts" is measured as zero received requests rather than inferred.
// ===========================================================================

/// Reset the update-check globals a previous test in this binary may have left
/// behind (`settings_persistence::reset` restores the frequency but not the
/// last-check timestamp or the cached answer).
fn reset_update_check_state() {
    *UPDATE_SETTINGS.write() = Default::default();
    *LAST_UPDATE_CHECK.write() = None;
    *CACHED_UPDATE_RESULT.write() = None;
}

/// A stand-in for `api.github.com/.../releases/latest` that counts how many
/// times it is actually called.
async fn spawn_fake_update_endpoint() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/latest",
        get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({
                    "tag_name": "v99.0.0",
                    "published_at": "2026-10-01T00:00:00Z",
                    "body": "Release notes line"
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake update endpoint");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{}/latest", addr), hits)
}

/// Point the update check at `url` for the duration of the test.
fn use_update_endpoint(url: &str) {
    std::env::set_var("HOMEVAULT_VERSION_CHECK_URL", url);
}

fn clear_update_endpoint() {
    std::env::remove_var("HOMEVAULT_VERSION_CHECK_URL");
}

/// Opening Settings must not contact the update endpoint: the default
/// frequency is `Manual`, so the GET serves the current version only.
#[tokio::test]
async fn test_version_info_never_contacts_the_update_endpoint_by_default() {
    let (state, _tmp, _guard) = setup_state().await;
    reset_update_check_state();
    let (url, hits) = spawn_fake_update_endpoint().await;
    use_update_endpoint(&url);

    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let before = update_check_attempts();
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/version",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"]["current_version"],
        json!(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        body["data"]["latest_version"],
        json!(null),
        "no check has run, so there is no latest version to report"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the default (Manual) frequency must never reach the update endpoint"
    );
    assert_eq!(
        update_check_attempts(),
        before,
        "the attempt counter is the invariant's observable form"
    );
    assert_eq!(*LAST_UPDATE_CHECK.read(), None);

    clear_update_endpoint();
}

/// `Disabled` is an even harder no: still zero attempts.
#[tokio::test]
async fn test_version_info_never_contacts_the_update_endpoint_when_disabled() {
    let (state, _tmp, _guard) = setup_state().await;
    reset_update_check_state();
    let (url, hits) = spawn_fake_update_endpoint().await;
    use_update_endpoint(&url);

    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/update",
        Some(&token),
        Some(json!({"check_frequency": "disabled"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        UPDATE_SETTINGS.read().check_frequency,
        Some(UpdateFrequency::Disabled)
    );

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/version",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["latest_version"], json!(null));
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a Disabled frequency must never reach the update endpoint"
    );
    assert_eq!(*LAST_UPDATE_CHECK.read(), None);

    clear_update_endpoint();
}

/// "Check now" is the explicit path: it forces the call, caches the answer,
/// and the next plain GET serves that cache without a second call.
#[tokio::test]
async fn test_check_now_is_the_only_forced_outbound_path() {
    let (state, _tmp, _guard) = setup_state().await;
    reset_update_check_state();
    let (url, hits) = spawn_fake_update_endpoint().await;
    use_update_endpoint(&url);

    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let before = update_check_attempts();
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/version/check",
        Some(&token),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["latest_version"], json!("99.0.0"));
    assert_eq!(body["data"]["up_to_date"], json!(false));
    assert_eq!(body["data"]["release_date"], json!("2026-10-01T00:00:00Z"));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(update_check_attempts(), before + 1);
    assert!(
        LAST_UPDATE_CHECK.read().is_some(),
        "the interval starts from the last attempt"
    );

    // The scheduled gate now has a fresh timestamp, so a plain GET (Manual)
    // serves the cached answer and makes no second call.
    let (status, cached) = send(
        &mut router,
        Method::GET,
        "/api/settings/version",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cached["data"]["latest_version"], json!("99.0.0"));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(update_check_attempts(), before + 1);

    clear_update_endpoint();
}

/// Forcing a third-party call is an admin decision; a plain user cannot do it
/// (and the request never leaves the process).
#[tokio::test]
async fn test_check_now_requires_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    reset_update_check_state();
    let (url, hits) = spawn_fake_update_endpoint().await;
    use_update_endpoint(&url);

    create_test_user(&state, "bob", "Password123!", 1).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "bob", "Password123!").await;

    let before = update_check_attempts();
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/settings/version/check",
        Some(&token),
        None,
    )
    .await;

    assert!(
        status.is_client_error() || body["success"] == json!(false),
        "a non-admin must not be able to force the check: {status} {body}"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the refusal happens before any outbound call"
    );
    assert_eq!(update_check_attempts(), before);

    clear_update_endpoint();
}

/// An opted-in schedule fires once its interval has elapsed — and only then.
#[tokio::test]
async fn test_a_due_schedule_checks_and_a_fresh_one_does_not() {
    let (state, _tmp, _guard) = setup_state().await;
    reset_update_check_state();
    let (url, hits) = spawn_fake_update_endpoint().await;
    use_update_endpoint(&url);

    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = settings_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/settings/update",
        Some(&token),
        Some(json!({"check_frequency": "daily"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Never checked → due on the next tick.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/version",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["latest_version"], json!("99.0.0"));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Just checked → not due, served from cache with no second call.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/settings/version",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["latest_version"], json!("99.0.0"));
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a fresh check must not be repeated on the next GET"
    );

    // Two days later → due again.
    let two_days_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 2 * 86_400;
    *LAST_UPDATE_CHECK.write() = Some(two_days_ago);
    let (_, _) = send(
        &mut router,
        Method::GET,
        "/api/settings/version",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the daily interval elapsed, so the schedule must check again"
    );

    clear_update_endpoint();
}
