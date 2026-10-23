/* =============================================================================
 * File          : storage_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

//! Integration tests for the Storage page API — the storage-proper endpoints
//! in `crate::handlers::{storage, cache, encryption}`:
//!
//!   Physical inventory (read, any session):
//!     GET  /api/storage/disks | /partitions | /removable
//!   Cache tier:
//!     GET  /api/cache/settings | /pools | /shares | /mover/runs   (read)
//!     PUT  /api/cache/settings                                    (admin)
//!     POST /api/cache/shares                                      (admin)
//!     PUT  /api/cache/shares/{name}                               (admin)
//!     DELETE /api/cache/shares/{name}                             (admin)
//!     POST /api/cache/mover/run                                   (admin)
//!   Encryption (LUKS2):
//!     GET  /api/encryption/volumes | /available-devices          (read)
//!     POST /api/encryption/volumes                               (admin, job)
//!     POST /api/encryption/volumes/{id}/unlock | /lock           (admin)
//!     POST /api/encryption/volumes/{id}/passphrase               (admin)
//!     DELETE /api/encryption/volumes/{id}                        (admin)
//!
//! Fresh in-memory SQLite + all migrations per test, a minimal Axum router,
//! `tower::ServiceExt` for HTTP-level testing — no real server.
//!
//! **This suite never creates, deletes or scrubs a real ZFS/Btrfs pool, and
//! never formats, unlocks or erases a real LUKS device.** Every one of those
//! paths goes through `sudo zpool`/`btrfs`/`cryptsetup`/`mkfs`/`mount` against
//! real block devices — per CLAUDE.md's "What NOT to test in Rust" that is
//! `tests/e2e/storage/storage.spec.ts`'s job (and even there it stays
//! validation-only unless an operator opts in with disposable loop devices).
//! Here the mutating endpoints are exercised only far enough to confirm they:
//!   * enforce the auth split (reads = any session, writes = admin), and
//!   * reject invalid input with a clean 400 *before* any command runs
//!     (bad cron / out-of-range knobs / bad share name / bad cache mode /
//!     short passphrase / bad filesystem / bad label / unknown volume id /
//!     missing confirmation).
//!
//! The read-only endpoints are safe to run for real — they only inventory
//! the host (`lsblk`, and best-effort `zpool`/`mdadm` scans that come back
//! empty when those tools aren't installed or need a password).
//!
//! The RAID endpoints that also live in `handlers/storage.rs`
//! (`/api/storage/raid*`) are covered by the RAID app's own tests, not here
//! (CLAUDE.md: "RAID handlers in this file stay governed by the RAID
//! section").
//!
//! Run:
//!   cargo test --test storage_integration
//!   cargo test --test storage_integration -- --nocapture

// Every test serialises on a process-wide `ENV_LOCK` (they mutate
// `DATA_DIRECTORY`), so its guard is intentionally held across `.await` for
// the whole test body. That is the pattern clippy's `await_holding_lock`
// warns about, but here it is exactly what keeps the suite deterministic.
#![allow(clippy::await_holding_lock)]

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
    cache::{
        handle_cache_mover_run, handle_cache_mover_runs, handle_cache_pools_get,
        handle_cache_settings_get, handle_cache_settings_update, handle_cache_share_create,
        handle_cache_share_delete, handle_cache_share_update, handle_cache_shares_list,
    },
    encryption::{
        handle_encryption_available_devices, handle_encryption_change_passphrase,
        handle_encryption_create, handle_encryption_delete, handle_encryption_list,
        handle_encryption_lock, handle_encryption_unlock,
    },
    storage::{
        handle_disk_smart_capabilities, handle_disk_smart_test_start,
        handle_disk_smart_test_status, handle_disks_list, handle_import_pool,
        handle_importable_pools_list, handle_partitions_list, handle_removable_disks_list,
        handle_virtual_disk_delete, handle_virtual_disk_grow, handle_virtual_disk_scrub,
        handle_virtual_disk_scrub_status, handle_virtual_disks_list,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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
        homevault::handlers::downloader::Aria2Process { child: None },
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

fn storage_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/storage/disks", get(handle_disks_list))
        .route("/api/storage/partitions", get(handle_partitions_list))
        .route("/api/storage/removable", get(handle_removable_disks_list))
        .route("/api/storage/virtual-disks", get(handle_virtual_disks_list))
        .route(
            "/api/storage/virtual-disks/{name}",
            delete(handle_virtual_disk_delete),
        )
        .route(
            "/api/storage/virtual-disks/{name}/scrub",
            post(handle_virtual_disk_scrub),
        )
        .route(
            "/api/storage/virtual-disks/{name}/scrub",
            get(handle_virtual_disk_scrub_status),
        )
        .route(
            "/api/storage/virtual-disks/{name}/grow",
            post(handle_virtual_disk_grow),
        )
        .route(
            "/api/storage/importable-pools",
            get(handle_importable_pools_list),
        )
        .route("/api/storage/import-pool", post(handle_import_pool))
        .route("/api/cache/settings", get(handle_cache_settings_get))
        .route("/api/cache/settings", put(handle_cache_settings_update))
        .route("/api/cache/pools", get(handle_cache_pools_get))
        .route("/api/cache/shares", get(handle_cache_shares_list))
        .route("/api/cache/shares", post(handle_cache_share_create))
        .route("/api/cache/shares/{name}", put(handle_cache_share_update))
        .route(
            "/api/cache/shares/{name}",
            delete(handle_cache_share_delete),
        )
        .route("/api/cache/mover/run", post(handle_cache_mover_run))
        .route("/api/cache/mover/runs", get(handle_cache_mover_runs))
        .route(
            "/api/storage/disks/{name}/smart/capabilities",
            get(handle_disk_smart_capabilities),
        )
        .route(
            "/api/storage/disks/{name}/smart/test",
            get(handle_disk_smart_test_status).post(handle_disk_smart_test_start),
        )
        .route("/api/encryption/volumes", get(handle_encryption_list))
        .route("/api/encryption/volumes", post(handle_encryption_create))
        .route(
            "/api/encryption/available-devices",
            get(handle_encryption_available_devices),
        )
        .route(
            "/api/encryption/volumes/{id}/unlock",
            post(handle_encryption_unlock),
        )
        .route(
            "/api/encryption/volumes/{id}/lock",
            post(handle_encryption_lock),
        )
        .route(
            "/api/encryption/volumes/{id}/passphrase",
            post(handle_encryption_change_passphrase),
        )
        .route(
            "/api/encryption/volumes/{id}",
            delete(handle_encryption_delete),
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
    .expect("Failed to insert test user");
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

fn msg(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .unwrap_or("")
        .to_string()
}

/// (method, uri, body) for every read endpoint (require_auth).
fn read_endpoints() -> Vec<(Method, &'static str)> {
    vec![
        (Method::GET, "/api/storage/disks"),
        (Method::GET, "/api/storage/partitions"),
        (Method::GET, "/api/storage/removable"),
        (Method::GET, "/api/cache/settings"),
        (Method::GET, "/api/cache/pools"),
        (Method::GET, "/api/cache/shares"),
        (Method::GET, "/api/cache/mover/runs"),
        (Method::GET, "/api/encryption/volumes"),
        (Method::GET, "/api/encryption/available-devices"),
        (Method::GET, "/api/storage/virtual-disks"),
        (Method::GET, "/api/storage/importable-pools"),
        (Method::GET, "/api/storage/disks/sda/smart/capabilities"),
        (Method::GET, "/api/storage/disks/sda/smart/test"),
    ]
}

/// (method, uri, body) for every write endpoint (require_admin).
fn write_endpoints() -> Vec<(Method, &'static str, Option<Value>)> {
    vec![
        (
            Method::PUT,
            "/api/cache/settings",
            Some(json!({"mover_enabled": true})),
        ),
        (
            Method::POST,
            "/api/cache/shares",
            Some(json!({"name": "s1", "cache_mode": "yes", "target_pool": "p"})),
        ),
        (
            Method::PUT,
            "/api/cache/shares/s1",
            Some(json!({"enabled": false})),
        ),
        (Method::DELETE, "/api/cache/shares/s1", None),
        (Method::POST, "/api/cache/mover/run", None),
        (
            Method::POST,
            "/api/encryption/volumes",
            Some(
                json!({"device": "/dev/hv-x", "filesystem": "ext4", "label": "l", "passphrase": "correct-horse-battery"}),
            ),
        ),
        (
            Method::POST,
            "/api/encryption/volumes/00000000-0000-0000-0000-000000000000/unlock",
            Some(json!({"passphrase": "correct-horse-battery"})),
        ),
        (
            Method::POST,
            "/api/encryption/volumes/00000000-0000-0000-0000-000000000000/lock",
            None,
        ),
        (
            Method::POST,
            "/api/encryption/volumes/00000000-0000-0000-0000-000000000000/passphrase",
            Some(json!({
                "old_passphrase": "correct-horse-battery",
                "new_passphrase": "a-longer-replacement-1"
            })),
        ),
        (
            Method::DELETE,
            "/api/encryption/volumes/00000000-0000-0000-0000-000000000000",
            Some(json!({"confirm": false})),
        ),
        (Method::DELETE, "/api/storage/virtual-disks/ghost", None),
        (Method::POST, "/api/storage/virtual-disks/ghost/scrub", None),
        (
            Method::POST,
            "/api/storage/virtual-disks/ghost/grow",
            Some(json!({"device": "/dev/hv-x"})),
        ),
        (
            Method::POST,
            "/api/storage/import-pool",
            Some(json!({"name": "tank", "pool_type": "zfs"})),
        ),
        (
            Method::POST,
            "/api/storage/disks/sda/smart/test",
            Some(json!({"test_type": "short"})),
        ),
    ]
}

// ---------------------------------------------------------------------------
// 1. Auth guards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_all_read_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = storage_router(state);
    for (method, uri) in read_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_all_read_endpoints_reject_bogus_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = storage_router(state);
    for (method, uri) in read_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, Some("nope"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_all_write_endpoints_reject_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = storage_router(state);
    for (method, uri, body) in write_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, None, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_all_write_endpoints_reject_non_admin_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = storage_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for (method, uri, body) in write_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, Some(&token), body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} should be admin-only"
        );
    }
}

#[tokio::test]
async fn test_read_endpoints_allow_any_valid_session() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "regular", "Password123!", 1).await;
    let mut router = storage_router(state);
    let token = login(&mut router, "regular", "Password123!").await;
    for (method, uri) in read_endpoints() {
        let (status, _) = send(&mut router, method.clone(), uri, Some(&token), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {uri} should allow a non-admin session"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Cache-settings validation — clean 400s, no command run
// ---------------------------------------------------------------------------

async fn admin_router() -> (
    Router,
    String,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = storage_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;
    (router, token, tmp, guard)
}

#[tokio::test]
async fn test_cache_settings_rejects_bad_cron() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&token),
        Some(json!({"mover_cron": "not a cron"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("5-field cron"));
}

#[tokio::test]
async fn test_cache_settings_rejects_out_of_range_knobs() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    for (field, val, needle) in [
        // 1-95 since ROADMAP item 2: 0 would drain the cache to nothing.
        ("min_free_percent", 0, "between 1 and 95"),
        ("min_free_percent", 200, "between 1 and 95"),
        ("mover_age_days", 99_999, "between 0 and 3650"),
        ("mover_bwlimit", 99_999_999, "10,000,000"),
    ] {
        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/cache/settings",
            Some(&token),
            Some(json!({ field: val })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}");
        assert!(msg(&body).contains(needle), "{field}: got {:?}", msg(&body));
    }
}

#[tokio::test]
async fn test_cache_settings_accepts_a_valid_update() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, _) = send(
        &mut router,
        Method::PUT,
        "/api/cache/settings",
        Some(&token),
        Some(json!({
            "mover_cron": "0 3 * * *",
            "mover_enabled": true,
            "min_free_percent": 25,
            "mover_age_days": 7,
            "mover_bwlimit": 0
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // ...and it round-trips through GET.
    let (_, body) = send(
        &mut router,
        Method::GET,
        "/api/cache/settings",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(body["data"]["mover_cron"], "0 3 * * *");
    assert_eq!(body["data"]["min_free_percent"], 25);
}

// ---------------------------------------------------------------------------
// 3. Cache-share validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_cache_share_create_rejects_bad_name() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    for bad in ["", "has space", "slash/name", "bad;name", &"x".repeat(65)] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/cache/shares",
            Some(&token),
            Some(json!({"name": bad, "cache_mode": "yes", "target_pool": "p"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "name {bad:?}");
        assert!(msg(&body).contains("Cache share name"));
    }
}

#[tokio::test]
async fn test_cache_share_create_rejects_bad_mode() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/cache/shares",
        Some(&token),
        Some(json!({"name": "media", "cache_mode": "sometimes", "target_pool": "p"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("cache_mode must be one of"));
}

#[tokio::test]
async fn test_cache_share_update_rejects_bad_mode() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/shares/whatever",
        Some(&token),
        Some(json!({"cache_mode": "turbo"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("cache_mode must be one of"));
}

#[tokio::test]
async fn test_cache_share_update_and_delete_report_missing_share() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/cache/shares/ghost",
        Some(&token),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"));

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/cache/shares/ghost",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"));
}

// ---------------------------------------------------------------------------
// 4. Encryption validation — nothing reaches cryptsetup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_encryption_create_rejects_short_passphrase() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes",
        Some(&token),
        Some(json!({"device": "/dev/hv-x", "filesystem": "ext4", "label": "vault", "passphrase": "short"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("at least 12 characters"));
}

#[tokio::test]
async fn test_encryption_create_rejects_bad_filesystem() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes",
        Some(&token),
        Some(json!({"device": "/dev/hv-x", "filesystem": "ntfs", "label": "vault", "passphrase": "correct-horse-battery"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("Invalid filesystem"));
}

#[tokio::test]
async fn test_encryption_create_rejects_bad_label() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    for bad in ["", "  ", "bad label", "bad/label", "../escape"] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/encryption/volumes",
            Some(&token),
            Some(json!({"device": "/dev/hv-x", "filesystem": "ext4", "label": bad, "passphrase": "correct-horse-battery"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "label {bad:?}");
        assert!(msg(&body).contains("Invalid label"));
    }
}

/// The device string is taken off the wire — without the availability
/// check a caller could pass "/dev/sda" straight into `cryptsetup
/// luksFormat`. A device that isn't in `DiskService::get_available_devices`
/// (which skips the system disk and flags mounted / RAID-member devices)
/// must be refused *before* any privileged command runs.
#[tokio::test]
async fn test_encryption_create_rejects_a_device_that_is_not_available() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    for dev in ["/dev/sda", "/dev/nonexistent-zzz", "/dev/loop0"] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/encryption/volumes",
            Some(&token),
            Some(json!({"device": dev, "filesystem": "ext4", "label": "vault", "passphrase": "correct-horse-battery"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "device {dev:?}");
        assert!(
            msg(&body).contains("not an available") || msg(&body).contains("verify device"),
            "device {dev:?}: {}",
            msg(&body)
        );
    }
}

#[tokio::test]
async fn test_encryption_unlock_lock_report_unknown_volume() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let id = "11111111-2222-3333-4444-555555555555";

    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/encryption/volumes/{id}/unlock"),
        Some(&token),
        Some(json!({"passphrase": "correct-horse-battery"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"));

    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/encryption/volumes/{id}/lock"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"));
}

#[tokio::test]
async fn test_encryption_delete_requires_confirmation_then_reports_missing() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let id = "11111111-2222-3333-4444-555555555555";

    // confirm:false → rejected before any lookup or command
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/encryption/volumes/{id}"),
        Some(&token),
        Some(json!({"confirm": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("confirm"));

    // confirm:true but unknown id → "not found" (still no cryptsetup)
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/encryption/volumes/{id}"),
        Some(&token),
        Some(json!({"confirm": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"));
}

// ---------------------------------------------------------------------------
// 5. Read success paths (admin) — safe, real host inventory
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_encryption_list_is_empty_array_on_a_fresh_db() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/encryption/volumes",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn test_cache_shares_and_mover_runs_start_empty() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    for uri in ["/api/cache/shares", "/api/cache/mover/runs"] {
        let (status, body) = send(&mut router, Method::GET, uri, Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(body["data"].is_array(), "{uri} data should be an array");
    }
}

#[tokio::test]
async fn test_disks_list_returns_an_array() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/disks",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
}

// ---------------------------------------------------------------------------
// 6. Virtual disks — STORAGE's unified mdadm + ZFS + Btrfs view
// ---------------------------------------------------------------------------
//
// The RAID app only *creates* virtual disks; listing, scrubbing and
// destroying them all live here. As with the pool endpoints, the mutating
// paths go through `sudo mdadm`/`zpool`/`btrfs` against real block devices,
// so this suite only proves the auth split and that an unknown name is
// rejected cleanly *before* any command runs (`PoolService::scan_pools`
// comes back empty on a host with no arrays, so every name is "not found").

#[tokio::test]
async fn test_virtual_disks_list_returns_an_array() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/virtual-disks",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
}

#[tokio::test]
async fn test_virtual_disk_delete_and_scrub_report_an_unknown_name() {
    let (mut router, token, _tmp, _guard) = admin_router().await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/storage/virtual-disks/does-not-exist",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"), "got: {:?}", msg(&body));

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/virtual-disks/does-not-exist/scrub",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"), "got: {:?}", msg(&body));
}

#[tokio::test]
async fn test_virtual_disk_scrub_status_reports_an_unknown_name() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/virtual-disks/does-not-exist/scrub",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"), "got: {:?}", msg(&body));
}

#[tokio::test]
async fn test_importable_pools_list_returns_an_array() {
    // Best-effort `zpool import` / `btrfs filesystem show` scans: absent
    // tools or a password-gated sudo just yield an empty list, never a 500.
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/importable-pools",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
}

#[tokio::test]
async fn test_import_pool_rejects_unsupported_type_before_any_command() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/import-pool",
        Some(&token),
        Some(json!({"name": "tank", "pool_type": "ext4"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        msg(&body).to_lowercase().contains("unsupported"),
        "got: {:?}",
        msg(&body)
    );
}

#[tokio::test]
async fn test_import_pool_rejects_mdadm_before_any_command() {
    // Storage must never import a RAID array — that's the RAID app's job.
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/import-pool",
        Some(&token),
        Some(json!({"name": "md0", "pool_type": "mdadm"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        msg(&body).to_lowercase().contains("unsupported"),
        "got: {:?}",
        msg(&body)
    );
}

#[tokio::test]
async fn test_cache_pools_get_returns_an_array_even_with_no_pools() {
    // Best-effort `zpool`/`mdadm`/`btrfs` scans: absent tools or a
    // password-gated sudo just yield an empty list, never a 500.
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/cache/pools",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].is_array());
}
