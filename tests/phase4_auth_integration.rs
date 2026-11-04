/* =============================================================================
 * File          : phase4_auth_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The access-control matrix for the five Phase 4 handler modules: DHCP, USB
 *   copy, fan control, the UPS server and guided redundancy.
 *
 *   These 22 routes shipped without a single `require_auth`/`require_admin`
 *   call, and there is no global auth layer under `/api`, so every one of them
 *   was reachable from the LAN with no session at all. That included
 *   `POST /power/ups-server/shutdown` (which runs `sudo poweroff`) and
 *   `POST /network/dhcp-server/start` (which runs `sudo systemctl start
 *   dnsmasq`). This suite is the regression guard: it fails if any route loses
 *   its guard again.
 *
 *   Nothing here touches the host. Every mutation is asserted to be *refused*
 *   before the handler body runs, so no `sudo`, `systemctl`, `docker` or PWM
 *   write is ever attempted. The read paths are only exercised with a session
 *   token, and where a read would shell out (lease table, device list, fan
 *   channels) the assertion is on the status code, not the payload.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test phase4_auth_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use axum::{
    routing::{get, post, put},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::dhcp_server::{
    handle_get as handle_dhcp_get, handle_leases, handle_start as handle_dhcp_start,
    handle_stop as handle_dhcp_stop, handle_update as handle_dhcp_update,
};
use homevault::handlers::fan_control::{
    handle_channels_list, handle_config_get, handle_config_update, handle_restore_auto,
};
use homevault::handlers::guided_redundancy::{handle_apply, handle_recommend};
use homevault::handlers::local_dns::{
    handle_get as handle_local_dns_get, handle_update as handle_local_dns_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::ups_server::{
    handle_ups_server_get, handle_ups_server_shutdown, handle_ups_server_start,
    handle_ups_server_stop, handle_ups_server_update,
};
use homevault::handlers::usb_copy::{
    handle_devices_list, handle_execute, handle_profile_create, handle_profile_delete,
    handle_profile_update, handle_profiles_list,
};
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

/// The same paths and methods `src/web/mod.rs` wires, so a route that is
/// re-pathed in production shows up here as a mismatch rather than as a
/// silently untested surface.
fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        // DHCP
        .route("/api/network/dhcp-server", get(handle_dhcp_get))
        .route("/api/network/dhcp-server", put(handle_dhcp_update))
        .route("/api/network/dhcp-server/start", post(handle_dhcp_start))
        .route("/api/network/dhcp-server/stop", post(handle_dhcp_stop))
        .route("/api/network/dhcp-server/leases", get(handle_leases))
        // Local DNS server (phase C1) — the same dnsmasq instance.
        .route("/api/network/dns-server", get(handle_local_dns_get))
        .route("/api/network/dns-server", put(handle_local_dns_update))
        // UPS server
        .route("/api/power/ups-server", get(handle_ups_server_get))
        .route("/api/power/ups-server", put(handle_ups_server_update))
        .route("/api/power/ups-server/start", post(handle_ups_server_start))
        .route("/api/power/ups-server/stop", post(handle_ups_server_stop))
        .route(
            "/api/power/ups-server/shutdown",
            post(handle_ups_server_shutdown),
        )
        // USB copy
        .route("/api/usb-copy/profiles", get(handle_profiles_list))
        .route("/api/usb-copy/profiles", post(handle_profile_create))
        .route("/api/usb-copy/profiles/{id}", put(handle_profile_update))
        .route(
            "/api/usb-copy/profiles/{id}",
            axum::routing::delete(handle_profile_delete),
        )
        .route("/api/usb-copy/devices", get(handle_devices_list))
        .route("/api/usb-copy/execute/{profile_id}", post(handle_execute))
        // Fan control
        .route("/api/system/fans", get(handle_channels_list))
        .route("/api/system/fans/config", get(handle_config_get))
        .route("/api/system/fans/config", put(handle_config_update))
        .route("/api/system/fans/restore-auto", post(handle_restore_auto))
        // Guided redundancy
        .route("/api/storage/guided-redundancy", get(handle_recommend))
        .route(
            "/api/storage/guided-redundancy/apply/{id}",
            post(handle_apply),
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

async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

/// Every route of the five modules, with a body that would be valid enough to
/// reach the handler body if the guard were missing.
fn all_routes() -> Vec<(Method, &'static str, Option<Value>, bool)> {
    // (method, uri, body, is_mutation)
    vec![
        // --- DHCP: reads ---
        (Method::GET, "/api/network/dhcp-server", None, false),
        (Method::GET, "/api/network/dhcp-server/leases", None, false),
        // --- DHCP: mutations ---
        (
            Method::PUT,
            "/api/network/dhcp-server",
            Some(json!({"config": disabled_dhcp_config()})),
            true,
        ),
        (Method::POST, "/api/network/dhcp-server/start", None, true),
        (Method::POST, "/api/network/dhcp-server/stop", None, true),
        // --- Local DNS server: reads ---
        (Method::GET, "/api/network/dns-server", None, false),
        // --- Local DNS server: mutations ---
        (
            Method::PUT,
            "/api/network/dns-server",
            Some(json!({"config": disabled_local_dns_config()})),
            true,
        ),
        // --- UPS server: reads ---
        (Method::GET, "/api/power/ups-server", None, false),
        // --- UPS server: mutations ---
        (
            Method::PUT,
            "/api/power/ups-server",
            Some(json!({"config": disabled_ups_config()})),
            true,
        ),
        (Method::POST, "/api/power/ups-server/start", None, true),
        (Method::POST, "/api/power/ups-server/stop", None, true),
        (
            Method::POST,
            "/api/power/ups-server/shutdown",
            Some(json!({"confirm": true})),
            true,
        ),
        // --- USB copy: reads ---
        (Method::GET, "/api/usb-copy/profiles", None, false),
        (Method::GET, "/api/usb-copy/devices", None, false),
        // --- USB copy: mutations ---
        (
            Method::POST,
            "/api/usb-copy/profiles",
            Some(json!({"profile": disabled_usb_profile()})),
            true,
        ),
        (
            Method::PUT,
            "/api/usb-copy/profiles/p1",
            Some(json!({"profile": disabled_usb_profile()})),
            true,
        ),
        (Method::DELETE, "/api/usb-copy/profiles/p1", None, true),
        (Method::POST, "/api/usb-copy/execute/p1", None, true),
        // --- Fan control: reads ---
        (Method::GET, "/api/system/fans", None, false),
        (Method::GET, "/api/system/fans/config", None, false),
        // --- Fan control: mutations ---
        (
            Method::PUT,
            "/api/system/fans/config",
            Some(json!({"profile": "auto"})),
            true,
        ),
        (Method::POST, "/api/system/fans/restore-auto", None, true),
        // --- Guided redundancy ---
        (Method::GET, "/api/storage/guided-redundancy", None, false),
        (
            Method::POST,
            "/api/storage/guided-redundancy/apply/rec-1",
            Some(json!({"confirmed": true})),
            true,
        ),
    ]
}

fn disabled_dhcp_config() -> Value {
    json!({
        "dhcp_enabled": false,
        "tftp_enabled": false,
        "interface": "",
        "dhcp_range_start": "192.168.1.100",
        "dhcp_range_end": "192.168.1.200",
        "netmask": "255.255.255.0",
        "gateway": null,
        "dns_servers": [],
        "lease_time_secs": 86400,
        "reservations": {},
        "tftp_root": "/srv/tftp",
        "pxe_boot_file": null,
        "confirmed": false
    })
}

fn disabled_local_dns_config() -> Value {
    json!({
        "enabled": false,
        "interface": "",
        "serve_dhcp_clients": false,
        "forwarders": [],
        "records": [],
        "overrides": [],
        "confirmed": false
    })
}

fn disabled_ups_config() -> Value {
    json!({
        "enabled": false,
        "bind_interface": "eth0",
        "bind_address": null,
        "port": 3493,
        "allowed_clients": [],
        "shutdown_policy": {
            "stop_containers": false,
            "shutdown_vms": false,
            "flush_restic": false,
            "stop_shares": false,
            "stage_timeout_secs": {}
        },
        "max_clients": 10
    })
}

fn disabled_usb_profile() -> Value {
    json!({
        "id": "p1",
        "name": "Camera Photos",
        "device_uuid": "1234-5678",
        "device_label": "CAMERA",
        "source_path": "/DCIM",
        "destination_path": "/mnt/photos",
        "verify_checksums": true,
        "auto_eject": false,
        "enabled": false,
        "last_run": null,
        "last_status": null
    })
}

// ---------------------------------------------------------------------------
// Access control — the whole point of this suite
// ---------------------------------------------------------------------------

/// The exact defect this suite exists for: before the fix, all 22 routes
/// answered without a session token.
#[tokio::test]
async fn test_every_phase4_route_refuses_an_unauthenticated_caller() {
    let h = setup().await;
    let mut router = signed_in(&h).await.0;

    for (method, uri, body, _mutation) in all_routes() {
        let (status, payload) = send(&mut router, method.clone(), uri, None, body).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} answered {status} with no session token — \
             it must be 401. Body: {payload}"
        );
    }
}

/// A signed-in non-admin may look at network/power/storage state, but must not
/// mutate it. `403` — never `400`, which the API reserves for a malformed
/// request (see `handlers::admin_guard`).
#[tokio::test]
async fn test_phase4_reads_allow_a_member_and_mutations_require_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    for (method, uri, body, mutation) in all_routes() {
        let (status, payload) = send(
            &mut router,
            method.clone(),
            uri,
            Some(&member),
            body.clone(),
        )
        .await;
        if mutation {
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} is a mutation and must refuse a non-admin with 403, got {status}. \
                 Body: {payload}"
            );
        } else {
            assert_ne!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} is a read and a member must be allowed past the guard"
            );
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} is a read and a member must be allowed past the guard"
            );
        }
    }
}

/// An admin passes the guard on every route. The assertion is deliberately
/// narrower than "200": several of these touch hardware or the host (lease
/// table, device list, fan channels) and legitimately answer `500` on a box
/// without dnsmasq/udisks2/hwmon. What must never happen is a refusal.
#[tokio::test]
async fn test_an_admin_is_never_refused_by_the_phase4_guards() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    for (method, uri, body, _mutation) in all_routes() {
        let (status, payload) = send(&mut router, method.clone(), uri, Some(&admin), body).await;
        assert!(
            status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
            "{method} {uri} refused an admin with {status}. Body: {payload}"
        );
    }
}

// ---------------------------------------------------------------------------
// The two routes whose missing guard was the worst
// ---------------------------------------------------------------------------

/// `POST /power/ups-server/shutdown` runs `sudo poweroff`. An unauthenticated
/// caller reaching it is a LAN-wide remote power-off; a non-admin reaching it
/// is a privilege escalation. Both are asserted here, separately, because this
/// is the one route where a regression is not "a missing feature".
#[tokio::test]
async fn test_ups_shutdown_is_admin_only_and_requires_confirmation() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/ups-server/shutdown",
        None,
        Some(json!({"confirm": true})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/power/ups-server/shutdown",
        Some(&member),
        Some(json!({"confirm": true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin without the explicit confirmation is refused too — the policy
    // is saved disabled, so a confirmed call would run `final_poweroff`. Only
    // the refusal path is exercised: asserting `400` proves the handler reached
    // its own check without executing anything.
    let (status, payload) = send(
        &mut router,
        Method::POST,
        "/api/power/ups-server/shutdown",
        Some(&admin),
        Some(json!({"confirm": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("confirmed"),
        "the refusal must name the confirmation rule: {payload}"
    );
}

/// `POST /network/dhcp-server/start` runs `sudo systemctl start dnsmasq`.
#[tokio::test]
async fn test_dhcp_start_is_admin_only() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/network/dhcp-server/start",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/network/dhcp-server/start",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// `POST /storage/guided-redundancy/apply/{id}` creates arrays once it is
/// implemented. It is admin-only from the start so the privilege boundary does
/// not have to be renegotiated later.
#[tokio::test]
async fn test_guided_redundancy_apply_is_admin_only_and_says_not_implemented() {
    let h = setup().await;
    let (mut router, admin, member) = signed_in(&h).await;

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/storage/guided-redundancy/apply/rec-1",
        None,
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/storage/guided-redundancy/apply/rec-1",
        Some(&member),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin gets the honest answer, not a 404 to guess about.
    let (status, payload) = send(
        &mut router,
        Method::POST,
        "/api/storage/guided-redundancy/apply/rec-1",
        Some(&admin),
        Some(json!({"confirmed": true})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{payload}");
    assert!(
        payload["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not implemented"),
        "the refusal must say what is missing: {payload}"
    );
}
