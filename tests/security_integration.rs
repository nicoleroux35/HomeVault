//! Integration tests for the Security suite API — Antivirus (ClamAV),
//! Anti-Ransomware (honeypots + rate detection), and Firewall (ufw):
//!
//!   Antivirus  — GET /api/antivirus/status | settings, PUT settings,
//!                POST scan | scan/cancel, GET scans | threats,
//!                POST threats/{id}/resolve
//!   Ransomware — GET /api/ransomware/status | settings | protected-paths |
//!                events, PUT settings, POST/DELETE protected-paths
//!   Firewall   — GET /api/firewall/status | enable/preview,
//!                POST enable | disable, POST/DELETE rules
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server. `DATA_DIRECTORY`
//! is a per-test `tempfile::TempDir`, and the path guard only allows `/tmp`,
//! so every real filesystem side effect (honeypot files, the quarantine dir,
//! a real `clamscan`) is contained under a throwaway temp directory.
//!
//! **Real conditions where they are safe:** a genuine `clamscan` of a temp
//! directory containing the standard EICAR test string is run for real (the
//! detection + quarantine-move path), and real honeypot decoy files are
//! written and removed on disk. **Never done:** enabling/disabling the real
//! host firewall, or deleting a real ufw rule — per CLAUDE.md's firewall
//! rules, `ufw` mutations only happen behind an explicit signed-off audit.
//! On a host where `sudo ufw` needs a password (the common dev/CI case),
//! `firewall::ufw_available()` is `false` and the firewall endpoints are
//! only exercised far enough to confirm the graceful "not installed"
//! response and the auth guards; the confirmed-gate and mandatory-rule
//! protection are asserted live in `tests/e2e/security/security.spec.ts`
//! only when a real `ufw` is present.
//!
//! Run:
//!   cargo test --test security_integration
//!   cargo test --test security_integration -- --nocapture

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
    antivirus::{
        handle_antivirus_scan_cancel, handle_antivirus_scan_start, handle_antivirus_scans_list,
        handle_antivirus_settings_get, handle_antivirus_settings_update, handle_antivirus_status,
        handle_antivirus_threat_resolve, handle_antivirus_threats_list,
    },
    auth::handle_login,
    firewall::{
        handle_firewall_disable, handle_firewall_enable, handle_firewall_enable_preview,
        handle_firewall_rule_add, handle_firewall_rule_delete, handle_firewall_status,
    },
    ransomware::{
        handle_ransomware_events_list, handle_ransomware_protected_path_add,
        handle_ransomware_protected_path_delete, handle_ransomware_protected_paths_list,
        handle_ransomware_settings_get, handle_ransomware_settings_update, handle_ransomware_status,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// The standard EICAR anti-malware test string — every AV engine flags it,
/// it is completely harmless, and it exists precisely so scanners can be
/// tested. Split so this source file itself is never flagged.
const EICAR: &str = concat!(
    r"X5O!P%@AP[4\PZX54(P^)7CC)7}$",
    "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*"
);

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

async fn setup_state() -> (Arc<AppState>, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    std::env::set_var("DATA_DIRECTORY", tmp.path());

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    // Allow the OS temp root so a `tempfile::tempdir()` (created under /tmp on
    // Linux) passes `path_guard.validate`.
    let path_guard = PathGuard::new(vec!["/tmp".to_string(), std::env::temp_dir().to_string_lossy().to_string()]);
    let admin_rate_limiter = Arc::new(RateLimitState::new(100, 60));
    let pty_manager = Arc::new(PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process { child: None },
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

fn security_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        // antivirus
        .route("/api/antivirus/status", get(handle_antivirus_status))
        .route(
            "/api/antivirus/settings",
            get(handle_antivirus_settings_get).put(handle_antivirus_settings_update),
        )
        .route("/api/antivirus/scan", post(handle_antivirus_scan_start))
        .route("/api/antivirus/scan/cancel", post(handle_antivirus_scan_cancel))
        .route("/api/antivirus/scans", get(handle_antivirus_scans_list))
        .route("/api/antivirus/threats", get(handle_antivirus_threats_list))
        .route(
            "/api/antivirus/threats/{id}/resolve",
            post(handle_antivirus_threat_resolve),
        )
        // firewall
        .route("/api/firewall/status", get(handle_firewall_status))
        .route("/api/firewall/enable/preview", get(handle_firewall_enable_preview))
        .route("/api/firewall/enable", post(handle_firewall_enable))
        .route("/api/firewall/disable", post(handle_firewall_disable))
        .route(
            "/api/firewall/rules",
            post(handle_firewall_rule_add).delete(handle_firewall_rule_delete),
        )
        // ransomware
        .route("/api/ransomware/status", get(handle_ransomware_status))
        .route(
            "/api/ransomware/settings",
            get(handle_ransomware_settings_get).put(handle_ransomware_settings_update),
        )
        .route(
            "/api/ransomware/protected-paths",
            get(handle_ransomware_protected_paths_list).post(handle_ransomware_protected_path_add),
        )
        .route(
            "/api/ransomware/protected-paths/{id}",
            delete(handle_ransomware_protected_path_delete),
        )
        .route("/api/ransomware/events", get(handle_ransomware_events_list))
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
    let resp = ServiceExt::<Req>::ready(router).await.unwrap().call(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

/// A settings body accepted by `PUT /api/antivirus/settings`.
fn av_settings(quarantine_dir: &str, sched_path: Option<&str>) -> Value {
    json!({
        "quarantine_dir": quarantine_dir,
        "scheduled_scan_enabled": false,
        "scheduled_scan_cron": "0 3 * * 0",
        "scheduled_scan_path": sched_path,
        "scan_archives": true,
    })
}

fn rw_settings(auto_response: &str) -> Value {
    json!({
        "enabled": true,
        "honeypots_enabled": true,
        "rate_threshold_events": 50,
        "rate_threshold_seconds": 10,
        "auto_response": auto_response,
    })
}

// ===========================================================================
// 1. Auth guards
// ===========================================================================

#[tokio::test]
async fn test_all_reads_reject_missing_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = security_router(state);
    for uri in [
        "/api/antivirus/status",
        "/api/antivirus/settings",
        "/api/antivirus/scans",
        "/api/antivirus/threats",
        "/api/firewall/status",
        "/api/firewall/enable/preview",
        "/api/ransomware/status",
        "/api/ransomware/settings",
        "/api/ransomware/protected-paths",
        "/api/ransomware/events",
    ] {
        let (status, body) = send(&mut r, Method::GET, uri, None, None).await;
        assert_eq!(body["success"], json!(false), "{uri}");
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} no-token → 400");
    }
}

#[tokio::test]
async fn test_reads_reject_a_tampered_token() {
    let (state, _tmp, _g) = setup_state().await;
    let mut r = security_router(state);
    for uri in ["/api/antivirus/status", "/api/firewall/status", "/api/ransomware/status"] {
        let (status, body) = send(&mut r, Method::GET, uri, Some("not-a-token"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(body["success"], json!(false));
    }
}

#[tokio::test]
async fn test_mutations_require_admin_reads_do_not() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = security_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;

    // Reads: a plain authenticated user is allowed.
    for uri in [
        "/api/antivirus/status",
        "/api/firewall/status",
        "/api/ransomware/status",
        "/api/ransomware/settings",
    ] {
        let (status, _) = send(&mut r, Method::GET, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{uri} should be readable by any session");
    }

    // Mutations: 403 for a non-admin.
    let muts: &[(Method, &str, Option<Value>)] = &[
        (Method::PUT, "/api/antivirus/settings", Some(av_settings("/tmp/q", None))),
        (Method::POST, "/api/antivirus/scan", Some(json!({"path": "/tmp"}))),
        (Method::POST, "/api/antivirus/scan/cancel", None),
        (Method::POST, "/api/antivirus/threats/whatever/resolve", Some(json!({}))),
        (Method::PUT, "/api/ransomware/settings", Some(rw_settings("alert_only"))),
        (Method::POST, "/api/ransomware/protected-paths", Some(json!({"path": "/tmp"}))),
        (Method::DELETE, "/api/ransomware/protected-paths/whatever", None),
        (Method::POST, "/api/firewall/enable", Some(json!({"confirmed": true}))),
        (Method::POST, "/api/firewall/disable", None),
        (Method::POST, "/api/firewall/rules", Some(json!({"port": "443"}))),
        (Method::DELETE, "/api/firewall/rules", Some(json!({"port": "443"}))),
    ];
    for (method, uri, body) in muts {
        let (status, resp) = send(&mut r, method.clone(), uri, Some(&token), body.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri} must be 403 for a non-admin");
        assert_eq!(resp["success"], json!(false));
    }
}

// ===========================================================================
// 2. Antivirus — settings, status, real EICAR scan
// ===========================================================================

#[tokio::test]
async fn test_antivirus_status_reports_binary_and_definitions() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/antivirus/status", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["clamscan_available"].is_boolean());
    assert!(body["data"]["definitions"]["available"].is_boolean());
    assert_eq!(body["data"]["scan_running"], json!(false));
}

#[tokio::test]
async fn test_antivirus_settings_round_trip_and_path_guard() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // A scheduled path outside the allowed roots is rejected.
    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/antivirus/settings",
        Some(&token),
        Some(av_settings("/tmp/hv-q", Some("/etc"))),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap_or("").contains("allowed root"));

    // A valid update persists and reads back.
    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/antivirus/settings",
        Some(&token),
        Some(av_settings("/tmp/hv-quarantine", Some("/tmp"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = send(&mut r, Method::GET, "/api/antivirus/settings", Some(&token), None).await;
    assert_eq!(body["data"]["quarantine_dir"], json!("/tmp/hv-quarantine"));
    assert_eq!(body["data"]["scheduled_scan_path"], json!("/tmp"));
}

#[tokio::test]
async fn test_antivirus_scan_rejects_bad_paths_before_spawning_clamscan() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // Outside the path guard.
    let (status, _) = send(&mut r, Method::POST, "/api/antivirus/scan", Some(&token), Some(json!({"path": "/etc"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Inside the guard but nonexistent.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/antivirus/scan",
        Some(&token),
        Some(json!({"path": "/tmp/hv-does-not-exist-xyz"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap_or("").contains("does not exist"));
}

#[tokio::test]
async fn test_antivirus_scan_cancel_when_idle_returns_false() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::POST, "/api/antivirus/scan/cancel", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["cancelled"], json!(false));
}

#[tokio::test]
async fn test_antivirus_threat_resolve_not_found() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/antivirus/threats/00000000-0000-0000-0000-000000000000/resolve",
        Some(&token),
        Some(json!({"delete": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap_or("").to_lowercase().contains("not found"));
}

#[tokio::test]
async fn test_antivirus_real_eicar_scan_detects_and_quarantines() {
    // A genuine `clamscan` run against a temp directory holding the EICAR
    // test string. Skips only if clamscan or its virus DB is genuinely
    // absent on this host (then the feature can't work at all and there's
    // nothing to assert).
    if !homevault::antivirus::clamscan_available() {
        eprintln!("skipping: clamscan not installed");
        return;
    }
    if !homevault::antivirus::definitions_status().available {
        eprintln!("skipping: no ClamAV virus database on this host");
        return;
    }

    let (state, tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // Scan target + a quarantine dir, both under the test's temp root.
    let scan_dir = tmp.path().join("scan-target");
    std::fs::create_dir_all(&scan_dir).unwrap();
    std::fs::write(scan_dir.join("eicar.com"), EICAR).unwrap();
    let quarantine = tmp.path().join("quarantine");

    let (status, _) = send(
        &mut r,
        Method::PUT,
        "/api/antivirus/settings",
        Some(&token),
        Some(av_settings(&quarantine.to_string_lossy(), None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/antivirus/scan",
        Some(&token),
        Some(json!({"path": scan_dir.to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "scan start: {body}");
    assert!(body["data"]["job_id"].is_string());

    // Poll the scan history until the scan finishes (bounded).
    let mut completed = false;
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let (_, scans) = send(&mut r, Method::GET, "/api/antivirus/scans", Some(&token), None).await;
        if let Some(arr) = scans["data"].as_array() {
            if arr.iter().any(|s| s["status"] == json!("completed") || s["status"] == json!("failed")) {
                completed = true;
                let last = &arr[0];
                assert_eq!(last["status"], json!("completed"), "scan should complete cleanly: {last}");
                assert!(
                    last["threats_found"].as_i64().unwrap_or(0) >= 1,
                    "EICAR must be detected: {last}"
                );
                break;
            }
        }
    }
    assert!(completed, "the scan never reached a terminal state within 30s");

    // The threat is recorded and the file was moved into quarantine.
    let (_, threats) = send(&mut r, Method::GET, "/api/antivirus/threats", Some(&token), None).await;
    let arr = threats["data"].as_array().cloned().unwrap_or_default();
    assert!(!arr.is_empty(), "a threat row must exist after an EICAR detection");
    let eicar = &arr[0];
    assert!(
        eicar["signature_name"].as_str().unwrap_or("").to_lowercase().contains("eicar"),
        "signature should name EICAR: {eicar}"
    );
    assert_eq!(eicar["action"], json!("quarantined"));
    assert!(
        quarantine.join("eicar.com").exists(),
        "clamscan --move should have quarantined the file"
    );
    assert!(
        !scan_dir.join("eicar.com").exists(),
        "the infected file should no longer be at its original path"
    );
}

// ===========================================================================
// 3. Anti-Ransomware — settings, real honeypot placement
// ===========================================================================

#[tokio::test]
async fn test_ransomware_settings_validates_auto_response() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::PUT,
        "/api/ransomware/settings",
        Some(&token),
        Some(rw_settings("nuke_everything")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap_or("").contains("auto_response"));

    for valid in ["alert_only", "lock_account", "disable_share"] {
        let (status, _) = send(
            &mut r,
            Method::PUT,
            "/api/ransomware/settings",
            Some(&token),
            Some(rw_settings(valid)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{valid} should be accepted");
    }
    let (_, body) = send(&mut r, Method::GET, "/api/ransomware/settings", Some(&token), None).await;
    assert_eq!(body["data"]["auto_response"], json!("disable_share"));
}

#[tokio::test]
async fn test_ransomware_protected_path_add_rejects_bad_input() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    // Outside the path guard.
    let (status, _) = send(
        &mut r,
        Method::POST,
        "/api/ransomware/protected-paths",
        Some(&token),
        Some(json!({"path": "/etc"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Inside the guard but not a directory.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/ransomware/protected-paths",
        Some(&token),
        Some(json!({"path": "/tmp/hv-not-a-real-dir-xyz"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap_or("").contains("directory"));
}

#[tokio::test]
async fn test_ransomware_protected_path_real_honeypot_lifecycle() {
    let (state, tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let watched = tmp.path().join("protected-share");
    std::fs::create_dir_all(&watched).unwrap();

    // Add with a honeypot — a real decoy file must appear in the directory.
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/ransomware/protected-paths",
        Some(&token),
        Some(json!({"path": watched.to_string_lossy(), "place_honeypot": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let path_id = body["data"]["id"].as_str().expect("protected path id").to_string();
    let honeypot_name = body["data"]["honeypot_filename"].as_str().expect("honeypot filename").to_string();
    assert!(
        watched.join(&honeypot_name).exists(),
        "a real honeypot decoy file must have been written to disk"
    );

    // It shows up in the list and bumps the status count.
    let (_, list) = send(&mut r, Method::GET, "/api/ransomware/protected-paths", Some(&token), None).await;
    assert_eq!(list["data"].as_array().map(|a| a.len()), Some(1));
    let (_, st) = send(&mut r, Method::GET, "/api/ransomware/status", Some(&token), None).await;
    assert_eq!(st["data"]["protected_path_count"], json!(1));

    // Delete — the row and the decoy file are both gone.
    let (status, _) = send(
        &mut r,
        Method::DELETE,
        &format!("/api/ransomware/protected-paths/{path_id}"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!watched.join(&honeypot_name).exists(), "the honeypot file must be cleaned up on delete");
    let (_, list) = send(&mut r, Method::GET, "/api/ransomware/protected-paths", Some(&token), None).await;
    assert_eq!(list["data"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn test_ransomware_protected_path_delete_not_found() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::DELETE,
        "/api/ransomware/protected-paths/does-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap_or("").contains("not found"));
}

#[tokio::test]
async fn test_ransomware_events_list_is_empty_and_well_formed() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/ransomware/events", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"], json!([]));
}

// ===========================================================================
// 4. Firewall — auth + graceful degradation (never touches a real ufw)
// ===========================================================================

#[tokio::test]
async fn test_firewall_status_is_always_well_formed() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/firewall/status", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["available"].is_boolean());
    assert!(body["data"]["enabled"].is_boolean());
    assert!(body["data"]["rules"].is_array());
    if body["data"]["available"] == json!(false) {
        assert_eq!(body["data"]["enabled"], json!(false));
        assert_eq!(body["data"]["rules"], json!([]));
    }
}

#[tokio::test]
async fn test_firewall_enable_never_succeeds_without_confirmation_or_ufw() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;

    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/firewall/enable",
        Some(&token),
        Some(json!({"confirmed": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));

    let ufw_available = homevault::firewall::ufw_available();
    if ufw_available {
        // With a real ufw present, the rejection reason must be the missing
        // confirmation (this test still never sends `confirmed: true`).
        assert!(
            body["message"].as_str().unwrap_or("").to_lowercase().contains("confirm"),
            "{body}"
        );
    } else {
        assert!(body["message"].as_str().unwrap_or("").contains("not installed"), "{body}");
    }
}

#[tokio::test]
async fn test_firewall_rule_add_requires_a_port() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "admin1", "Passw0rd!123", 2).await;
    let mut r = security_router(state);
    let token = login(&mut r, "admin1", "Passw0rd!123").await;
    let (status, body) = send(
        &mut r,
        Method::POST,
        "/api/firewall/rules",
        Some(&token),
        Some(json!({"port": "   "})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], json!(false));
    // Either "Port is required" (ufw present) or "ufw is not installed".
}

#[tokio::test]
async fn test_firewall_enable_preview_is_readable_by_any_session() {
    let (state, _tmp, _g) = setup_state().await;
    create_test_user(&state, "regular1", "Passw0rd!123", 1).await;
    let mut r = security_router(state);
    let token = login(&mut r, "regular1", "Passw0rd!123").await;
    let (status, body) = send(&mut r, Method::GET, "/api/firewall/enable/preview", Some(&token), None).await;
    if homevault::firewall::ufw_available() {
        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["ssh_port"].is_number());
        assert!(body["data"]["app_port"].is_number());
    } else {
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["message"].as_str().unwrap_or("").contains("not installed"));
    }
}
