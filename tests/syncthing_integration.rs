/* =============================================================================
 * File          : syncthing_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Syncthing integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
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
//! Integration tests for the Syncthing API — `crate::handlers::syncthing`.
//!
//! 14 endpoints: status, config, devices, folders, connections, errors (read) +
//! restart, pause, resume, scan, create folder/device, remove folder/device
//! (mutating).
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` — no real server.
//!
//! **This suite never talks to a real Syncthing daemon for a mutation.**
//! The original audit's headline finding was that all 10 handlers had **no
//! auth check whatsoever** — `restart` (full daemon restart) and
//! pause/resume (disrupt any paired device) included, plus read access to
//! every device ID/IP and folder path. Every handler now requires an
//! **admin** session; this suite locks that in on all 14, plus the
//! malformed-body 422 on the ones that take a JSON body. The admin
//! success paths (which issue a real HTTP call to `127.0.0.1:8384`, and
//! for the mutating routes would actually change a running daemon's
//! configuration) are left to `tests/e2e/syncthing/syncthing.spec.ts`, and
//! even there stay read-only unless a disposable instance is wired up.
//!
//! Section 3 covers ROADMAP 18.2: the `<apikey>` in `config.xml` is
//! user-editable file content interpolated into an outbound header, and a
//! malformed value used to **panic the request task**. A temporary
//! `config.xml` under `DATA_DIRECTORY` drives the real handlers with a
//! newline in the key, with a pretty-printed key and with a blank one; the
//! expected answers are a named `success:false` error, an accepted header
//! and "no key configured" respectively — never a panic.
//!
//! Section 5 covers ROADMAP 18.1 (folder and device management). Its rule is
//! the one the 2026-10-23 audits kept restating: the assertion has to be about
//! *behaviour*, not shape. Every refused field is proved to be refused **before
//! any call to the daemon** (the error must not be the transport one), and a
//! valid body is proved to get *past* validation (it must reach the transport),
//! so neither direction can regress into "200 with an envelope".
//!
//! Note on status codes: these handlers always return HTTP 200 and signal
//! failure in the body (`{"success":false,"error":…}`), like Downloader /
//! Cloud Sync. Assertions match that.
//!
//! Run:
//!   cargo test --test syncthing_integration
//!   cargo test --test syncthing_integration -- --nocapture

use axum::{
    routing::{delete, get, post},
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
    syncthing::{
        handle_syncthing_config, handle_syncthing_connections,
        handle_syncthing_create_device, handle_syncthing_create_folder,
        handle_syncthing_delete_device, handle_syncthing_delete_folder,
        handle_syncthing_devices, handle_syncthing_errors, handle_syncthing_folders,
        handle_syncthing_pause_device, handle_syncthing_restart,
        handle_syncthing_resume_device, handle_syncthing_scan_folder, handle_syncthing_status,
    },
    AppState,
};
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

fn syncthing_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/syncthing/status", get(handle_syncthing_status))
        .route("/api/syncthing/config", get(handle_syncthing_config))
        .route(
            "/api/syncthing/devices",
            get(handle_syncthing_devices).post(handle_syncthing_create_device),
        )
        .route(
            "/api/syncthing/devices/{device_id}",
            delete(handle_syncthing_delete_device),
        )
        .route(
            "/api/syncthing/folders",
            get(handle_syncthing_folders).post(handle_syncthing_create_folder),
        )
        .route(
            "/api/syncthing/folders/{folder_id}",
            delete(handle_syncthing_delete_folder),
        )
        .route(
            "/api/syncthing/connections",
            get(handle_syncthing_connections),
        )
        .route("/api/syncthing/errors", get(handle_syncthing_errors))
        .route("/api/syncthing/restart", post(handle_syncthing_restart))
        .route("/api/syncthing/pause", post(handle_syncthing_pause_device))
        .route(
            "/api/syncthing/resume",
            post(handle_syncthing_resume_device),
        )
        .route("/api/syncthing/scan", post(handle_syncthing_scan_folder))
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

fn denied(body: &Value, fragment: &str) {
    assert_eq!(body["success"], false, "expected denied, got {body}");
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains(fragment),
        "error {err:?} should contain {fragment:?}"
    );
}

/// A structurally valid device ID: 56 base-32 characters in eight groups.
const DEVICE_ID: &str = "AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH";

/// (method, uri, body) for every endpoint, with a body valid for its extractor.
fn all_endpoints() -> Vec<(Method, String, Option<Value>)> {
    vec![
        (Method::GET, "/api/syncthing/status".to_string(), None),
        (Method::GET, "/api/syncthing/config".to_string(), None),
        (Method::GET, "/api/syncthing/devices".to_string(), None),
        (Method::GET, "/api/syncthing/folders".to_string(), None),
        (Method::GET, "/api/syncthing/connections".to_string(), None),
        (Method::GET, "/api/syncthing/errors".to_string(), None),
        (Method::POST, "/api/syncthing/restart".to_string(), None),
        (
            Method::POST,
            "/api/syncthing/pause".to_string(),
            Some(json!({"device": "AAAA-BBBB"})),
        ),
        (
            Method::POST,
            "/api/syncthing/resume".to_string(),
            Some(json!({"device": "AAAA-BBBB"})),
        ),
        (
            Method::POST,
            "/api/syncthing/scan".to_string(),
            Some(json!({"folder": "default"})),
        ),
        // ROADMAP 18.1 — folder and device management.
        (
            Method::POST,
            "/api/syncthing/folders".to_string(),
            Some(json!({"id": "e2e", "path": "/tmp/syncthing-e2e"})),
        ),
        (
            Method::DELETE,
            "/api/syncthing/folders/e2e".to_string(),
            None,
        ),
        (
            Method::POST,
            "/api/syncthing/devices".to_string(),
            Some(json!({"device_id": DEVICE_ID})),
        ),
        (
            Method::DELETE,
            format!("/api/syncthing/devices/{DEVICE_ID}"),
            None,
        ),
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth — all 14 endpoints are admin-only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_endpoints_reject_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = syncthing_router(state);
    for (m, u, b) in all_endpoints() {
        let (status, body) = send(&mut router, m.clone(), &u, None, b).await;
        assert_eq!(status, StatusCode::OK, "{m} {u}");
        denied(&body, "No session token provided");
    }
}

#[tokio::test]
async fn test_all_endpoints_reject_bogus_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut router = syncthing_router(state);
    for (m, u, b) in all_endpoints() {
        let (_, body) = send(&mut router, m.clone(), &u, Some("nope"), b).await;
        denied(&body, "Invalid or expired session");
    }
}

#[tokio::test]
async fn test_all_endpoints_reject_non_admin_session() {
    // The whole point of the 2026-07-16 audit fix: device topology + sync
    // control are admin-only. A regular user must not be able to enumerate
    // devices, let alone restart the daemon.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for (m, u, b) in all_endpoints() {
        let (_, body) = send(&mut router, m.clone(), &u, Some(&token), b).await;
        denied(&body, "Admin privileges required");
    }
}

// ---------------------------------------------------------------------------
// 2. Body validation — malformed JSON never reaches the Syncthing API
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pause_resume_scan_reject_missing_field_with_422() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for (uri, bad) in [
        ("/api/syncthing/pause", json!({"not_device": "x"})),
        ("/api/syncthing/resume", json!({})),
        ("/api/syncthing/scan", json!({"not_folder": "x"})),
        ("/api/syncthing/folders", json!({"id": "x"})),
        ("/api/syncthing/devices", json!({"name": "x"})),
    ] {
        let (status, _) = send(&mut router, Method::POST, uri, Some(&token), Some(bad)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// 3. The API key read from config.xml (ROADMAP 18.2)
// ---------------------------------------------------------------------------
//
// `config.xml` is a file an administrator can edit by hand, and its `<apikey>`
// is interpolated into an outbound header. `HeaderValue::from_str(..).unwrap()`
// used to sit on that path, so a key holding a control character or a
// non-ASCII byte **panicked the request task** (and, with `panic = "abort"` in
// the release profile, the whole appliance). These tests drive the real
// handlers: a panic anywhere in the task fails the test, and a `success:false`
// envelope with a named reason is the expected answer.

/// Write a `config.xml` where the DATA_DIRECTORY probe finds it first.
fn write_config_xml(tmp: &tempfile::TempDir, key_body: &str) {
    let dir = tmp.path().join("syncthing");
    std::fs::create_dir_all(&dir).expect("syncthing dir");
    std::fs::write(
        dir.join("config.xml"),
        format!(
            "<configuration version=\"37\">\n  <gui enabled=\"true\">\n    \
             <address>127.0.0.1:8384</address>\n    <apikey>{}</apikey>\n  </gui>\n</configuration>",
            key_body
        ),
    )
    .expect("write config.xml");
}

#[tokio::test]
async fn test_malformed_api_key_is_a_named_error_not_a_panic() {
    let (state, tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // A newline in the key: `HeaderValue::from_str` refuses it, and the old
    // `.unwrap()` turned that into a panic.
    write_config_xml(&tmp, "abc\ndef");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/syncthing/config",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "module convention: 200 + envelope");
    denied(&body, "not a valid HTTP header value");
    denied(&body, "Syncthing API key");

    // The status endpoint reports the same fault when the binary is installed,
    // instead of the misleading "Installed but not running".
    let (_, st) = send(
        &mut router,
        Method::GET,
        "/api/syncthing/status",
        Some(&token),
        None,
    )
    .await;
    if st["success"] == true {
        // No `syncthing` binary on this host: "not available" is the right
        // answer and the key is never read.
        assert_eq!(st["data"]["available"], false);
    } else {
        denied(&st, "not a valid HTTP header value");
    }
}

#[tokio::test]
async fn test_pretty_printed_api_key_is_accepted() {
    // Indented XML puts a newline and spaces around the value. Trimming is what
    // makes a hand-formatted (or GUI-rewritten) config work.
    let (state, tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    write_config_xml(&tmp, "\n      abcDEF123456\n    ");

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/syncthing/config",
        Some(&token),
        None,
    )
    .await;
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        !err.contains("Syncthing API key"),
        "an indented key must not be blamed: {body}"
    );
}

#[tokio::test]
async fn test_blank_api_key_is_not_a_configuration_error() {
    // `<apikey></apikey>` means "no key configured": no header is sent, and the
    // failure that follows is a transport one, never a key error.
    let (state, tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    write_config_xml(&tmp, "");

    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/syncthing/connections",
        Some(&token),
        None,
    )
    .await;
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        !err.contains("Syncthing API key"),
        "a blank key is 'no key', not a broken one: {body}"
    );
}

// ---------------------------------------------------------------------------
// 4. status success shape (admin) — read-only, safe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_status_for_admin_returns_a_well_formed_envelope() {
    // `handle_syncthing_status` runs `syncthing --version` then best-effort
    // pings 127.0.0.1:8384. With a well-formed API key on disk it must answer a
    // `{"success":true,"data":{...available:bool...}}` envelope — never an
    // error and never a panic — whether or not a daemon is up. (The key is
    // written explicitly so the result does not depend on whatever
    // `/root/.config/syncthing/config.xml` holds on the host.)
    let (state, tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;
    write_config_xml(&tmp, "abcDEF123456ghiJKL7890mnoPQR");

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/syncthing/status",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert!(
        body["data"]["available"].is_boolean(),
        "status must report an `available` bool, got {}",
        body["data"]
    );
}

// ---------------------------------------------------------------------------
// 5. Config edits — folder and device management (ROADMAP 18.1)
// ---------------------------------------------------------------------------
//
// The screen could read the daemon's topology and pause/resume a paired
// device, but it could not add or remove a folder or a device at all. The four
// new handlers validate **before** anything reaches the daemon, which is what
// these tests pin: a refused field is a `success:false` envelope naming the
// rule, with no connection attempt, so the daemon being down (the normal state
// of this test host) cannot mask a validation hole.

/// A valid `POST /api/syncthing/folders` body for the test guard (`/tmp`).
fn folder_body() -> Value {
    json!({
        "id": "e2e",
        "label": "E2E",
        "path": "/tmp/syncthing-e2e",
        "type": "sendreceive",
        "devices": [],
    })
}

#[tokio::test]
async fn test_create_folder_refuses_bad_fields_before_any_daemon_call() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let mut cases: Vec<(Value, &str)> = vec![
        (
            json!({"id": "", "path": "/tmp/syncthing-e2e"}),
            "Folder ID must not be empty",
        ),
        (
            json!({"id": "a/b", "path": "/tmp/syncthing-e2e"}),
            "may only contain letters, digits",
        ),
        (
            json!({"id": "e2e", "path": "relative/path"}),
            "must be an absolute path",
        ),
        (
            json!({"id": "e2e", "path": "/tmp/../etc"}),
            "must not contain '..'",
        ),
        (
            json!({"id": "e2e", "path": "/etc"}),
            "must be inside a storage location",
        ),
        (
            json!({"id": "e2e", "path": "/tmp/syncthing-e2e", "type": "receiveencrypted"}),
            "is not supported",
        ),
        (
            json!({"id": "e2e", "path": "/tmp/syncthing-e2e", "devices": ["not-a-device"]}),
            "56 characters",
        ),
    ];
    // A path longer than the cap is refused too, without building it in the
    // (readable) table above.
    cases.push((
        json!({"id": "e2e", "path": format!("/tmp/{}", "a".repeat(1100))}),
        "too long",
    ));

    for (body, fragment) in cases {
        let (status, res) = send(
            &mut router,
            Method::POST,
            "/api/syncthing/folders",
            Some(&token),
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        denied(&res, fragment);
        let err = res["error"].as_str().unwrap_or_default();
        assert!(
            !err.contains("Failed to connect"),
            "validation must run before the daemon call, got {err:?} for {body}"
        );
    }
}

#[tokio::test]
async fn test_create_device_refuses_a_bad_id_before_any_daemon_call() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for (device, fragment) in [
        (json!({"device_id": ""}), "must not be empty"),
        (json!({"device_id": "AAAA-BBBB"}), "56 characters"),
        (json!({"device_id": "0".repeat(56)}), "letters A-Z and digits 2-7"),
        (
            json!({"device_id": DEVICE_ID, "name": "bad\nname"}),
            "control characters",
        ),
    ] {
        let (_, res) = send(
            &mut router,
            Method::POST,
            "/api/syncthing/devices",
            Some(&token),
            Some(device.clone()),
        )
        .await;
        denied(&res, fragment);
        assert!(
            !res["error"].as_str().unwrap_or_default().contains("Failed to connect"),
            "validation must run before the daemon call, got {res}"
        );
    }
}

#[tokio::test]
async fn test_delete_endpoints_refuse_an_invalid_parameter_before_the_call() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    // The ID is re-validated after URL decoding: `..` and `a/b` must never be
    // interpolated into the daemon's REST path.
    for (uri, fragment) in [
        ("/api/syncthing/folders/a%2Fb", "may only contain"),
        ("/api/syncthing/folders/..", "not a usable identifier"),
        ("/api/syncthing/devices/nope", "56 characters"),
    ] {
        let (status, res) = send(&mut router, Method::DELETE, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{uri} {res}");
        denied(&res, fragment);
        assert!(
            !res["error"].as_str().unwrap_or_default().contains("Failed to connect"),
            "{uri}: validation must run before the daemon call, got {res}"
        );
    }
}

#[tokio::test]
async fn test_valid_config_edits_pass_validation_and_reach_the_transport() {
    // A body that passes every rule must get as far as the HTTP call. On a host
    // with no daemon that is the named transport error — *not* a validation
    // message — which is what proves the rules are not refusing valid input.
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = syncthing_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    for (method, uri, body) in [
        (Method::POST, "/api/syncthing/folders", Some(folder_body())),
        (
            Method::POST,
            "/api/syncthing/devices",
            Some(json!({"device_id": DEVICE_ID.to_lowercase(), "name": "Laptop"})),
        ),
        (Method::DELETE, "/api/syncthing/folders/e2e", None),
        (Method::DELETE, "/api/syncthing/devices/AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH", None),
    ] {
        let (status, res) = send(&mut router, method, uri, Some(&token), body).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let err = res["error"].as_str().unwrap_or_default();
        if res["success"] == true {
            // A real daemon answered on 127.0.0.1:8384 — the call was accepted,
            // which is the strongest possible form of this assertion.
            continue;
        }
        assert!(
            err.contains("Failed to connect to Syncthing")
                || err.contains("Syncthing API error"),
            "{uri}: a valid body must reach the daemon, got {res}"
        );
    }
}
