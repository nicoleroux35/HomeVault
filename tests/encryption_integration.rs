/* =============================================================================
 * File          : encryption_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 23/10/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Encryption integration tests: in-memory SQLite + assembled Axum router,
 *   exercising the LUKS2 endpoints through oneshot requests — the auth split,
 *   the passphrase policy, the duplicate-label gate, and the two behaviours
 *   ROADMAP item 9 is about (erasure is refused while the volume cannot be
 *   released; the metadata row survives every refusal).
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   Rust: cargo test --test encryption_integration
 * =============================================================================
 */

//! Integration tests for `crate::handlers::encryption`.
//!
//!   GET    /api/encryption/volumes                        (read)
//!   GET    /api/encryption/available-devices              (read)
//!   POST   /api/encryption/volumes                        (admin, job)
//!   POST   /api/encryption/volumes/{id}/unlock            (admin)
//!   POST   /api/encryption/volumes/{id}/lock              (admin)
//!   POST   /api/encryption/volumes/{id}/passphrase        (admin)
//!   DELETE /api/encryption/volumes/{id}                   (admin)
//!
//! **No test here formats, unlocks or erases a real LUKS device.** Every
//! mutating path shells out to `sudo cryptsetup`/`mount` against a real block
//! device; per CLAUDE.md's "What NOT to test in Rust" that is the opt-in
//! `tests/e2e/storage/storage.spec.ts` lifecycle's job. What *is* asserted
//! here is the part that used to be wrong and that runs without hardware:
//!
//! * every route keeps the read = session / write = admin split, including the
//!   new passphrase-rotation route;
//! * a new passphrase is refused *before* any command runs (policy), and a
//!   valid one gets past the policy into the device gate;
//! * two volumes cannot share a label (they would share a mount point);
//! * **`DELETE` refuses to erase a volume it could not release** — the row is
//!   still there afterwards, which is the observable form of ROADMAP 9.2
//!   ("key material erased while the volume is still mounted");
//! * a stored volume whose mapper is not active is listed as `unlocked: false`
//!   (the list reads the live state, it does not echo the DB).
//!
//! The rows are inserted directly: creating one through the API would need a
//! free block device.

#![allow(clippy::await_holding_lock)]

use axum::{
    routing::{delete, get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{
    auth::handle_login,
    encryption::{
        handle_encryption_available_devices, handle_encryption_change_passphrase,
        handle_encryption_create, handle_encryption_delete, handle_encryption_list,
        handle_encryption_lock, handle_encryption_unlock,
    },
    AppState,
};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// A passphrase that clears the policy (12+ characters, four distinct ones).
const GOOD_PASSPHRASE: &str = "correct-horse-battery";
/// A second one, used as the "new" value in rotation tests.
const OTHER_PASSPHRASE: &str = "a-longer-replacement-1";

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

fn encryption_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
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

/// Inserts an `encrypted_volumes` row directly — the only way to exercise the
/// lock/unlock/delete/passphrase paths without a free block device.
async fn seed_volume(state: &AppState, label: &str) {
    sqlx::query(
        "INSERT INTO encrypted_volumes
            (id, device_path, mapper_name, luks_uuid, filesystem, label, mount_point, created_at)
         VALUES (?, ?, ?, ?, 'ext4', ?, ?, datetime('now'))",
    )
    .bind(format!("id-{label}"))
    .bind(format!("/dev/homevault-test-missing-{label}"))
    .bind(format!("hv-{label}"))
    .bind(format!("uuid-{label}"))
    .bind(label)
    .bind(format!("/mnt/encrypted/{label}"))
    .execute(&*state.db.pool())
    .await
    .expect("Failed to seed an encrypted volume");
}

async fn volume_row_count(state: &AppState) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM encrypted_volumes")
        .fetch_one(&*state.db.pool())
        .await
        .expect("count")
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

/// A `POST /api/encryption/volumes` body whose device can never be available,
/// so the request stops at the device gate (before any privileged command).
fn create_body(passphrase: &str) -> Value {
    json!({
        "device": "/dev/homevault-test-missing",
        "filesystem": "ext4",
        "label": "vault",
        "passphrase": passphrase,
    })
}

/// Router with a signed-in admin session.
async fn admin_router() -> (
    Router,
    String,
    tempfile::TempDir,
    std::sync::MutexGuard<'static, ()>,
) {
    let (state, tmp, guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    let mut router = encryption_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;
    (router, token, tmp, guard)
}

// ---------------------------------------------------------------------------
// 1. Auth split
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_endpoints_reject_a_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = encryption_router(state);
    for uri in [
        "/api/encryption/volumes",
        "/api/encryption/available-devices",
    ] {
        let (status, _) = send(&mut router, Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn test_every_write_route_rejects_a_missing_token() {
    let (state, _tmp, _guard) = setup_state().await;
    let mut router = encryption_router(state);
    let id = "00000000-0000-0000-0000-000000000000";
    let writes = vec![
        (
            Method::POST,
            "/api/encryption/volumes".to_string(),
            Some(
                json!({"device": "/dev/x", "filesystem": "ext4", "label": "v", "passphrase": GOOD_PASSPHRASE}),
            ),
        ),
        (
            Method::POST,
            format!("/api/encryption/volumes/{id}/unlock"),
            Some(json!({"passphrase": GOOD_PASSPHRASE})),
        ),
        (
            Method::POST,
            format!("/api/encryption/volumes/{id}/lock"),
            None,
        ),
        (
            Method::POST,
            format!("/api/encryption/volumes/{id}/passphrase"),
            Some(json!({"old_passphrase": GOOD_PASSPHRASE, "new_passphrase": OTHER_PASSPHRASE})),
        ),
        (
            Method::DELETE,
            format!("/api/encryption/volumes/{id}"),
            Some(json!({"confirm": true})),
        ),
    ];
    for (method, uri, body) in writes {
        let (status, _) = send(&mut router, method.clone(), &uri, None, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}");
    }
}

#[tokio::test]
async fn test_every_write_route_refuses_a_non_admin() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "member", "Password123!", 1).await;
    let mut router = encryption_router(state);
    let token = login(&mut router, "member", "Password123!").await;
    let id = "00000000-0000-0000-0000-000000000000";
    let writes = vec![
        (
            Method::POST,
            "/api/encryption/volumes".to_string(),
            Some(
                json!({"device": "/dev/x", "filesystem": "ext4", "label": "v", "passphrase": GOOD_PASSPHRASE}),
            ),
        ),
        (
            Method::POST,
            format!("/api/encryption/volumes/{id}/passphrase"),
            Some(json!({"old_passphrase": GOOD_PASSPHRASE, "new_passphrase": OTHER_PASSPHRASE})),
        ),
        (
            Method::DELETE,
            format!("/api/encryption/volumes/{id}"),
            Some(json!({"confirm": true})),
        ),
    ];
    for (method, uri, body) in writes {
        let (status, _) = send(&mut router, method.clone(), &uri, Some(&token), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
}

// ---------------------------------------------------------------------------
// 2. Passphrase policy (ROADMAP 9 — 8 characters with no strength check)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_rejects_a_passphrase_under_the_policy() {
    let (mut router, token, _tmp, _guard) = admin_router().await;

    for short in ["short", "abcdefgh", "abcdefghijk"] {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/encryption/volumes",
            Some(&token),
            Some(create_body(short)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{short:?}");
        assert!(
            msg(&body).contains("at least 12 characters"),
            "{short:?}: {}",
            msg(&body)
        );
    }

    // The 12-character minimum alone would wave this through.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes",
        Some(&token),
        Some(create_body("password1234")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("too common"), "{}", msg(&body));
}

/// A strong passphrase must get *past* the policy: the next refusal is the
/// device gate, which runs before any privileged command.
#[tokio::test]
async fn test_create_accepts_a_strong_passphrase_past_the_policy_check() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes",
        Some(&token),
        Some(json!({
            "device": "/dev/homevault-test-missing",
            "filesystem": "ext4",
            "label": "vault",
            "passphrase": GOOD_PASSPHRASE,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = msg(&body);
    assert!(
        message.contains("not an available") || message.contains("verify device"),
        "expected the device gate, got: {message}"
    );
    assert!(
        !message.to_lowercase().contains("passphrase"),
        "a valid passphrase must not be reported as the problem: {message}"
    );
}

// ---------------------------------------------------------------------------
// 3. Create pre-checks that run before any command
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_rejects_a_duplicate_label() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state);
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes",
        Some(&token),
        Some(json!({
            "device": "/dev/homevault-test-missing",
            "filesystem": "ext4",
            "label": "vault",
            "passphrase": GOOD_PASSPHRASE,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        msg(&body).contains("already exists"),
        "two volumes sharing a label would share a mount point: {}",
        msg(&body)
    );
}

// ---------------------------------------------------------------------------
// 4. Delete is gated on the volume really being released (ROADMAP 9.2)
// ---------------------------------------------------------------------------

/// The old code ran `let _ = unmount(); let _ = close(); erase();`: a volume
/// that could not be released still had its key slots destroyed. Now the
/// refusal is observable — 400 and the metadata row is untouched, so the
/// volume is still listed and can be fixed by hand.
#[tokio::test]
async fn test_delete_refuses_to_erase_a_volume_it_cannot_release() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/encryption/volumes/id-vault",
        Some(&token),
        Some(json!({"confirm": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", msg(&body));
    let message = msg(&body).to_lowercase();
    assert!(
        message.contains("unmount")
            || message.contains("still open")
            || message.contains("erase")
            || message.contains("still mounted"),
        "the refusal must name what went wrong: {message}"
    );

    assert_eq!(
        volume_row_count(&state).await,
        1,
        "a refused erase must leave the metadata row in place"
    );

    // And the volume is still listed, so the admin can retry after fixing it.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/encryption/volumes",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().map(|a| a.len()), Some(1));
}

#[tokio::test]
async fn test_delete_still_requires_confirmation() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/encryption/volumes/id-vault",
        Some(&token),
        Some(json!({"confirm": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("confirm"), "{}", msg(&body));
    assert_eq!(volume_row_count(&state).await, 1);
}

// ---------------------------------------------------------------------------
// 5. Lock / unlock report failures instead of claiming success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lock_reports_a_failure_instead_of_success() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes/id-vault/lock",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", msg(&body));
    assert!(
        msg(&body).to_lowercase().contains("lock"),
        "the message must say the lock failed: {}",
        msg(&body)
    );
}

#[tokio::test]
async fn test_unlock_of_a_missing_device_reports_a_failure() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes/id-vault/unlock",
        Some(&token),
        Some(json!({"passphrase": GOOD_PASSPHRASE})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        msg(&body).to_lowercase().contains("unlock"),
        "{}",
        msg(&body)
    );
}

// ---------------------------------------------------------------------------
// 6. Passphrase rotation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rotation_rejects_a_weak_new_passphrase_before_any_command() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes/id-vault/passphrase",
        Some(&token),
        Some(json!({"old_passphrase": GOOD_PASSPHRASE, "new_passphrase": "short"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        msg(&body).contains("at least 12 characters"),
        "{}",
        msg(&body)
    );
}

#[tokio::test]
async fn test_rotation_refuses_to_reuse_the_current_passphrase() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes/id-vault/passphrase",
        Some(&token),
        Some(json!({
            "old_passphrase": GOOD_PASSPHRASE,
            "new_passphrase": GOOD_PASSPHRASE,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("must be different"), "{}", msg(&body));
}

#[tokio::test]
async fn test_rotation_reports_an_unknown_volume() {
    let (mut router, token, _tmp, _guard) = admin_router().await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes/does-not-exist/passphrase",
        Some(&token),
        Some(json!({"old_passphrase": GOOD_PASSPHRASE, "new_passphrase": OTHER_PASSPHRASE})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("not found"), "{}", msg(&body));
}

/// `luksChangeKey` cannot succeed against a device that is not there, and the
/// answer must say the current passphrase is unchanged rather than implying a
/// half-applied rotation.
#[tokio::test]
async fn test_rotation_fails_cleanly_on_a_missing_device() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/encryption/volumes/id-vault/passphrase",
        Some(&token),
        Some(json!({
            "old_passphrase": GOOD_PASSPHRASE,
            "new_passphrase": OTHER_PASSPHRASE,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(msg(&body).contains("unchanged"), "{}", msg(&body));
}

// ---------------------------------------------------------------------------
// 7. The list reports the live state, not the DB
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_reports_a_stored_volume_as_locked() {
    let (state, _tmp, _guard) = setup_state().await;
    create_test_user(&state, "admin1", "Password123!", 2).await;
    seed_volume(&state, "vault").await;
    let mut router = encryption_router(state.clone());
    let token = login(&mut router, "admin1", "Password123!").await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/encryption/volumes",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let volumes = body["data"].as_array().expect("an array of volumes");
    assert_eq!(volumes.len(), 1);
    assert_eq!(volumes[0]["label"], "vault");
    assert_eq!(volumes[0]["mount_point"], "/mnt/encrypted/vault");
    assert_eq!(
        volumes[0]["unlocked"], false,
        "a mapper that is not active must be reported as locked"
    );
}
