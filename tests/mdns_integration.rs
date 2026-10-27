/* =============================================================================
 * File          : mdns_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Mdns integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for `GET /api/mdns/status` (`crate::handlers::mdns`) and
//! for the publish/unpublish chain the General settings save drives.
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt::oneshot`. Avahi is not required — the status handler
//! reports `available: false` on a host without it and still returns the
//! canonical `homevault.local` address.
//!
//! Run: cargo test --test mdns_integration

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
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    mdns::handle_mdns_status,
    settings::{handle_general_settings_get, handle_general_settings_update},
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
    // The enable flag is process-global; every test starts from the shipped
    // default so a mutating test cannot leak into the next one.
    homevault::mdns::set_enabled(true);
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());
    std::env::remove_var("PORT"); // status URL must fall back to the 8080 default

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
        .route("/api/mdns/status", get(handle_mdns_status))
        .route(
            "/api/settings/general",
            get(handle_general_settings_get).put(handle_general_settings_update),
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
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
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

#[tokio::test]
async fn test_mdns_status_requires_a_session() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);
    let (_, body) = send(&mut r, Method::GET, "/api/mdns/status", None, None).await;
    assert_eq!(body["success"], json!(false), "no token must be rejected");
}

#[tokio::test]
async fn test_mdns_status_rejects_bad_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = router(state);
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/mdns/status",
        Some("not-a-real-token"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// ROADMAP 29.1 — switching discovery off really removes the host artefacts
// ---------------------------------------------------------------------------

/// A directory of shims put first on `PATH`, so the publish/unpublish chain can
/// run end to end without touching `/etc` or the real Avahi.
///
/// The shims do exactly what the real tools would, but only for the paths the
/// test passes (both are redirected into a tempdir through
/// `HV_AVAHI_CONF`/`HV_AVAHI_SERVICE_FILE`): `sudo tee`/`sudo rm` act on the
/// file, `which` claims `avahi-daemon` is installed (otherwise `unpublish`
/// returns early and the test would be vacuous), and `systemctl`/`busctl`/the
/// reload binary succeed silently — this host may have no Avahi at all, and
/// the test must not restart one.
///
/// This is the same seam `tests/health_integration.rs` uses for a slow `df`.
fn shim_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("shim dir");
    let write = |name: &str, body: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).expect("write shim");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod shim");
        }
    };

    // `sudo tee <path>` reads the body on stdin and writes it.
    write(
        "sudo",
        "#!/bin/sh\n\
         # `sudo tee <path>` / `sudo rm -f <path>` / anything else succeeds.\n\
         if [ \"$1\" = tee ]; then shift; cat > \"$1\"; exit $?; fi\n\
         if [ \"$1\" = rm ]; then shift; shift; rm -f \"$1\"; exit $?; fi\n\
         exit 0\n",
    );
    write(
        "which",
        "#!/bin/sh\n\
         # Only `avahi-daemon` is \"installed\" for this test.\n\
         [ \"$1\" = avahi-daemon ] && { echo /usr/sbin/avahi-daemon; exit 0; }\n\
         exit 1\n",
    );
    write("systemctl", "#!/bin/sh\nexit 0\n");
    write("busctl", "#!/bin/sh\necho 's \"homevault\"\nexit 0\n");
    write("avahi-daemon", "#!/bin/sh\nexit 0\n");
    dir
}

/// The whole point of item 29.1: `unpublish()` had **no caller**, so turning
/// discovery off left the Avahi service file and the `home-vault` name override
/// on disk while the setting said "off".
///
/// The test drives the real settings endpoint with both Avahi paths redirected
/// into a tempdir, and asserts the two artefacts are gone afterwards.
#[tokio::test]
async fn disabling_discovery_removes_the_service_file_and_the_name_override() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin1", "Password123!", 2).await;

    let avahi = tempfile::tempdir().expect("avahi dir");
    let conf_path = avahi.path().join("avahi-daemon.conf");
    let service_path = avahi.path().join("homevault.service");
    std::fs::write(
        &conf_path,
        "[server]\nuse-ipv4=yes\nhost-name=homevault\n\n[publish]\npublish-hinfo=no\n",
    )
    .expect("seed conf");
    std::fs::write(&service_path, "<service-group></service-group>\n").expect("seed service");

    std::env::set_var(homevault::mdns::AVAHI_CONF_ENV, &conf_path);
    std::env::set_var(homevault::mdns::SERVICE_FILE_ENV, &service_path);
    let shims = shim_dir();
    let previous_path = std::env::var("PATH").ok();
    // Prepend, never replace: the fake `sudo` calls `cat`/`rm` itself, so a PATH
    // holding only the shim directory made it fail with "cat: not found".
    let shim_path = match previous_path.as_deref() {
        Some(path) => format!("{}:{path}", shims.path().display()),
        None => shims.path().display().to_string(),
    };
    std::env::set_var("PATH", shim_path);

    let mut r = router(state);
    let token = login(&mut r, "admin1", "Password123!").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/general",
        Some(&token),
        Some(json!({"mdns_enabled": false})),
    )
    .await;

    // Restore the process environment before asserting, so a failure does not
    // leak the shims into the rest of the suite.
    if let Some(path) = previous_path {
        std::env::set_var("PATH", path);
    }
    std::env::remove_var(homevault::mdns::AVAHI_CONF_ENV);
    std::env::remove_var(homevault::mdns::SERVICE_FILE_ENV);

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], json!(true), "{body}");
    assert!(
        body["data"]["mdns_notice"].is_null(),
        "the host change must have succeeded: {body}"
    );

    assert!(
        !service_path.exists(),
        "the DNS-SD service file must be removed, not left advertising HomeVault"
    );
    let conf = std::fs::read_to_string(&conf_path).expect("conf still there");
    assert!(
        !conf.contains("host-name"),
        "the `host-name` override must be removed too: {conf}"
    );
    // …and the rest of the file is untouched.
    assert!(conf.contains("use-ipv4=yes"), "{conf}");
    assert!(conf.contains("publish-hinfo=no"), "{conf}");

    // The status endpoint now reports the appliance as undiscoverable.
    let (status, body) = send(&mut r, Method::GET, "/api/mdns/status", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], json!(false), "{body}");
}

/// Turning it back on republishes immediately instead of waiting for the
/// reconciler's next 120 s tick — otherwise the switch looks broken for two
/// minutes.
#[tokio::test]
async fn enabling_discovery_writes_the_service_file_and_pins_the_name_again() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "admin2", "Password123!", 2).await;

    let avahi = tempfile::tempdir().expect("avahi dir");
    let conf_path = avahi.path().join("avahi-daemon.conf");
    let service_path = avahi.path().join("homevault.service");
    std::fs::write(&conf_path, "[server]\nuse-ipv4=yes\n").expect("seed conf");

    std::env::set_var(homevault::mdns::AVAHI_CONF_ENV, &conf_path);
    std::env::set_var(homevault::mdns::SERVICE_FILE_ENV, &service_path);
    let shims = shim_dir();
    let previous_path = std::env::var("PATH").ok();
    // Prepend, never replace: the fake `sudo` calls `cat`/`rm` itself, so a PATH
    // holding only the shim directory made it fail with "cat: not found".
    let shim_path = match previous_path.as_deref() {
        Some(path) => format!("{}:{path}", shims.path().display()),
        None => shims.path().display().to_string(),
    };
    std::env::set_var("PATH", shim_path);

    let mut r = router(state);
    let token = login(&mut r, "admin2", "Password123!").await;
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/settings/general",
        Some(&token),
        Some(json!({"mdns_enabled": true})),
    )
    .await;

    if let Some(path) = previous_path {
        std::env::set_var("PATH", path);
    }
    std::env::remove_var(homevault::mdns::AVAHI_CONF_ENV);
    std::env::remove_var(homevault::mdns::SERVICE_FILE_ENV);

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        service_path.exists(),
        "the service file must be written on enable: {body}"
    );
    let service = std::fs::read_to_string(&service_path).expect("service file");
    assert!(service.contains("_http._tcp"), "{service}");
    let conf = std::fs::read_to_string(&conf_path).expect("conf");
    assert!(conf.contains("host-name=homevault"), "{conf}");

    let (status, body) = send(&mut r, Method::GET, "/api/mdns/status", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], json!(true), "{body}");
}

/// The setting is admin-only on both verbs, and the read reports the live flag.
#[tokio::test]
async fn the_mdns_setting_is_admin_only_and_reads_back() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "member", "Password123!", 1).await;
    let mut r = router(state);

    let (status, _) = send(&mut r, Method::GET, "/api/settings/general", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no token → 401");

    let token = login(&mut r, "member", "Password123!").await;
    let (status, _) = send(
        &mut r,
        Method::GET,
        "/api/settings/general",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a member is refused → 403");

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/settings/general",
        Some(&token),
        Some(json!({"mdns_enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_mdns_status_returns_canonical_address_for_any_user() {
    let (state, _tmp, _g) = setup_state().await;
    mk_user(&state, "u", "pw012345", 1).await; // a plain user is enough
    let mut r = router(state);
    let token = login(&mut r, "u", "pw012345").await;

    let (status, body) = send(&mut r, Method::GET, "/api/mdns/status", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], json!(true));
    let d = &body["data"];
    assert_eq!(d["host_name"], json!("homevault"));
    assert_eq!(d["fqdn"], json!("homevault.local"));
    assert_eq!(d["http_url"], json!("http://homevault.local:8080"));
    let services = d["services"].as_array().expect("services array");
    assert!(
        services.iter().any(|s| s == "_http._tcp"),
        "web UI is always advertised"
    );
    // `available`/`running` are host-dependent (Avahi may or may not be
    // installed in CI) — just assert they are booleans and consistent.
    assert!(d["available"].is_boolean());
    assert!(d["running"].is_boolean());
    // The admin's choice is reported (ROADMAP 29.1). Discovery ships on.
    assert_eq!(d["enabled"], json!(true), "discovery ships enabled");
    if !d["available"].as_bool().unwrap() {
        assert_eq!(d["running"], json!(false));
    }
}
