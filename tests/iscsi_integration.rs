/* =============================================================================
 * File          : iscsi_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   iSCSI export integration tests: the auth matrix, the payload shape, every
 *   validation path, the backstore/target CRUD round-trip and the refusal to
 *   forget a backstore a LUN still serves.
 *
 *   Nothing here touches the host. The test box has no `targetcli`, so every
 *   apply reports `no_targetcli` with an empty command list; the assertions
 *   say so explicitly and skip the reason check on a machine that does have
 *   the LIO stack installed.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test iscsi_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    routing::{get, post, put},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::iscsi::{
    handle_iscsi_apply, handle_iscsi_backstore_create, handle_iscsi_backstore_delete,
    handle_iscsi_backstore_update, handle_iscsi_get, handle_iscsi_settings_put,
    handle_iscsi_target_create, handle_iscsi_target_delete, handle_iscsi_target_update,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const CHAP_SECRET: &str = "TwelveChars!";

struct Harness {
    state: Arc<AppState>,
    root: PathBuf,
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
        root,
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/storage/iscsi", get(handle_iscsi_get))
        .route(
            "/api/storage/iscsi/settings",
            put(handle_iscsi_settings_put),
        )
        .route(
            "/api/storage/iscsi/backstores",
            post(handle_iscsi_backstore_create),
        )
        .route(
            "/api/storage/iscsi/backstores/{id}",
            put(handle_iscsi_backstore_update).delete(handle_iscsi_backstore_delete),
        )
        .route(
            "/api/storage/iscsi/targets",
            post(handle_iscsi_target_create),
        )
        .route(
            "/api/storage/iscsi/targets/{id}",
            put(handle_iscsi_target_update).delete(handle_iscsi_target_delete),
        )
        .route("/api/storage/iscsi/apply", post(handle_iscsi_apply))
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
        .unwrap()
        .to_vec();
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    (status, json)
}

async fn signed_in(h: &Harness) -> (Router, String, String) {
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    create_test_user(&h.state, "member", "pass", 1).await;
    let admin = login(&mut router, "admin", "pass").await;
    let member = login(&mut router, "member", "pass").await;
    (router, admin, member)
}

/// A backstore body pointing at a file this test owns, under the data
/// directory (the only place a fileio backstore may live).
fn backstore_body(h: &Harness, name: &str) -> Value {
    let folder = h.root.join("iscsi");
    std::fs::create_dir_all(&folder).expect("backstore folder");
    json!({
        "name": name,
        "kind": "fileio",
        "path": folder.join(format!("{name}.img")).to_string_lossy(),
        "size_bytes": 10_i64 * 1024 * 1024 * 1024,
        "readonly": false,
    })
}

fn target_body(iqn: &str, backstore_id: i64) -> Value {
    json!({
        "iqn": iqn,
        "enabled": true,
        "portals": [{"address": "127.0.0.1", "port": 3260}],
        "luns": [{"lun": 0, "backstore_id": backstore_id}],
        "acls": [{
            "initiator_iqn": "iqn.2026-11.local.client:laptop",
            "userid": "homevault",
            "secret": CHAP_SECRET,
        }],
    })
}

async fn create_backstore(router: &mut Router, admin: &str, body: Value) -> i64 {
    let (status, resp) = send(
        router,
        Method::POST,
        "/api/storage/iscsi/backstores",
        Some(admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    resp["data"]["backstores"][0]["id"]
        .as_i64()
        .expect("the created backstore id")
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_reads_need_a_session_and_mutations_need_an_admin() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(&mut router, Method::GET, "/api/storage/iscsi", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/iscsi",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let target = json!({
        "iqn": "iqn.2026-11.local.homevault:disk0",
        "portals": [{"address": "127.0.0.1", "port": 3260}],
    });
    let store = json!({
        "name": "data",
        "kind": "fileio",
        "path": "/does/not/matter.img",
        "size_bytes": 10_i64 * 1024 * 1024 * 1024,
    });

    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/storage/iscsi/settings".to_string(),
            Some(json!({"enabled": false})),
        ),
        (
            Method::POST,
            "/api/storage/iscsi/backstores".to_string(),
            Some(store.clone()),
        ),
        (
            Method::PUT,
            "/api/storage/iscsi/backstores/1".to_string(),
            Some(store.clone()),
        ),
        (
            Method::DELETE,
            "/api/storage/iscsi/backstores/1".to_string(),
            None,
        ),
        (
            Method::POST,
            "/api/storage/iscsi/targets".to_string(),
            Some(target.clone()),
        ),
        (
            Method::PUT,
            "/api/storage/iscsi/targets/1".to_string(),
            Some(target),
        ),
        (
            Method::DELETE,
            "/api/storage/iscsi/targets/1".to_string(),
            None,
        ),
        (Method::POST, "/api/storage/iscsi/apply".to_string(), None),
    ] {
        let (status, _) = send(&mut router, method.clone(), &uri, None, body.clone()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        let (status, _) = send(&mut router, method, &uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_returns_settings_capabilities_paths_and_empty_state() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/iscsi",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let data = &body["data"];
    assert_eq!(data["settings"]["enabled"], json!(false));
    assert_eq!(data["settings"]["applied_at"], json!(null));
    assert_eq!(data["capabilities"]["usable"], data["state"]["usable"]);
    assert!(data["capabilities"]["modules_loaded"].is_array());
    assert!(data["capabilities"]["modules_missing"].is_array());
    assert_eq!(data["backstores"], json!([]));
    assert_eq!(data["targets"], json!([]));
    assert_eq!(data["defaults"]["portal_address"], json!("0.0.0.0"));
    assert_eq!(data["defaults"]["portal_port"], json!(3260));
    assert!(
        data["defaults"]["iqn"]
            .as_str()
            .is_some_and(|iqn| iqn.starts_with("iqn.")),
        "the suggested IQN must be an iSCSI name: {body}"
    );
    assert!(
        data["paths"]["allowed_roots"]
            .as_array()
            .expect("allowed roots")
            .iter()
            .any(|root| root.as_str() == Some(&h.root.to_string_lossy())),
        "the data directory is the first allowed root: {body}"
    );
    assert_eq!(data["state"]["pending"], json!(false));
    assert_eq!(data["state"]["exported_targets"], json!(0));
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_validation_refuses_unsafe_or_incoherent_input() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    std::fs::create_dir_all(h.root.join("iscsi")).expect("iscsi folder");

    let good_store = backstore_body(&h, "data");
    let mut backstores: Vec<(&str, Value)> = Vec::new();

    let mut name_with_space = good_store.clone();
    name_with_space["name"] = json!("my disk");
    backstores.push(("a space in the name", name_with_space));

    let mut path_outside = good_store.clone();
    path_outside["path"] = json!("/etc/shadow");
    backstores.push(("a path outside the data directory", path_outside));

    let mut block_without_dev = good_store.clone();
    block_without_dev["kind"] = json!("block");
    block_without_dev["path"] = json!("/tmp/not-a-device");
    backstores.push(("a block store outside /dev", block_without_dev));

    let mut tiny = good_store.clone();
    tiny["size_bytes"] = json!(1024);
    backstores.push(("a size below one MiB", tiny));

    let mut bad_kind = good_store.clone();
    bad_kind["kind"] = json!("ramdisk");
    backstores.push(("an unknown kind", bad_kind));

    for (label, payload) in backstores {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/storage/iscsi/backstores",
            Some(&admin),
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert_eq!(body["success"], json!(false), "{label}");
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{label} must explain itself: {body}"
        );
    }

    // A valid store, so the target cases can point at one real LUN.
    let store_id = create_backstore(&mut router, &admin, good_store).await;

    let mut targets: Vec<(&str, Value)> = Vec::new();
    let good = target_body("iqn.2026-11.local.homevault:disk0", store_id);

    let mut no_portal = good.clone();
    no_portal["portals"] = json!([]);
    targets.push(("a target without a portal", no_portal));

    let mut bad_iqn = good.clone();
    bad_iqn["iqn"] = json!("disk0");
    targets.push(("an IQN without a prefix", bad_iqn));

    let mut unknown_store = good.clone();
    unknown_store["luns"] = json!([{"lun": 0, "backstore_id": 99_999}]);
    targets.push(("a LUN on a backstore that does not exist", unknown_store));

    let mut duplicate_lun = good.clone();
    duplicate_lun["luns"] = json!([
        {"lun": 0, "backstore_id": store_id},
        {"lun": 0, "backstore_id": store_id}
    ]);
    targets.push(("the same LUN number twice", duplicate_lun));

    let mut duplicate_acl = good.clone();
    duplicate_acl["acls"] = json!([
        {"initiator_iqn": "iqn.2026-11.local.client:a", "userid": null, "secret": null},
        {"initiator_iqn": "IQN.2026-11.LOCAL.CLIENT:A", "userid": null, "secret": null}
    ]);
    targets.push(("the same initiator twice", duplicate_acl));

    let mut chap_without_secret = good.clone();
    chap_without_secret["acls"] = json!([{
        "initiator_iqn": "iqn.2026-11.local.client:a",
        "userid": "homevault",
        "secret": null
    }]);
    targets.push(("CHAP without a secret", chap_without_secret));

    let mut short_secret = good.clone();
    short_secret["acls"] = json!([{
        "initiator_iqn": "iqn.2026-11.local.client:a",
        "userid": "homevault",
        "secret": "short"
    }]);
    targets.push(("a CHAP secret under 12 characters", short_secret));

    for (label, payload) in targets {
        let (status, body) = send(
            &mut router,
            Method::POST,
            "/api/storage/iscsi/targets",
            Some(&admin),
            Some(payload),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert_eq!(body["success"], json!(false), "{label}");
    }

    // Nothing was saved by any refused request.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/iscsi",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["backstores"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["targets"], json!([]));
}

// ---------------------------------------------------------------------------
// CRUD round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_backstore_write_policy_round_trip_and_validation() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let mut body = backstore_body(&h, "direct-store");
    body["write_policy"] = json!("direct");
    body["emulate_fua"] = json!(false);
    body["unmap"] = json!(false);
    let store_id = create_backstore(&mut router, &admin, body).await;

    let (status, payload) = send(
        &mut router,
        Method::GET,
        "/api/storage/iscsi",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{payload}");
    let store = &payload["data"]["backstores"][0];
    assert_eq!(store["id"], json!(store_id));
    assert_eq!(store["write_policy"], json!("direct"));
    assert_eq!(store["emulate_fua"], json!(false));
    assert_eq!(store["unmap"], json!(false));

    let mut bad = backstore_body(&h, "bad-policy");
    bad["write_policy"] = json!("writeback");
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/backstores",
        Some(&admin),
        Some(bad),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_backstore_and_target_round_trip_keeps_chap_sealed() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let store_id = create_backstore(&mut router, &admin, backstore_body(&h, "data")).await;

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/targets",
        Some(&admin),
        Some(target_body("iqn.2026-11.local.homevault:disk0", store_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let target = &body["data"]["targets"][0];
    let target_id = target["id"].as_i64().expect("target id");
    assert_eq!(target["luns"][0]["backstore_id"], json!(store_id));
    assert_eq!(target["luns"][0]["backstore_name"], json!("data"));
    assert_eq!(target["acls"][0]["userid"], json!("homevault"));
    assert_eq!(target["acls"][0]["has_secret"], json!(true));
    // The secret never travels back, and the payload never echoes it.
    assert!(
        !body.to_string().contains(CHAP_SECRET),
        "the CHAP secret must never be returned: {body}"
    );
    assert_eq!(body["data"]["backstores"][0]["lun_count"], json!(1));

    // Updating with `secret: null` keeps the stored blob instead of failing.
    let kept = json!({
        "iqn": "iqn.2026-11.local.homevault:disk0",
        "enabled": true,
        "portals": [{"address": "127.0.0.1", "port": 3260}],
        "luns": [{"lun": 0, "backstore_id": store_id}],
        "acls": [{
            "initiator_iqn": "iqn.2026-11.local.client:laptop",
            "userid": "homevault",
            "secret": null,
        }],
    });
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/storage/iscsi/targets/{target_id}"),
        Some(&admin),
        Some(kept),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["targets"][0]["acls"][0]["has_secret"],
        json!(true)
    );

    // Renaming the IQN in place is refused: the kernel object is still named
    // after the old IQN, so the export would linger under a name the metadata
    // no longer lists.
    let mut renamed = target_body("iqn.2026-11.local.homevault:disk1", store_id);
    renamed["enabled"] = json!(true);
    let (status, body) = send(
        &mut router,
        Method::PUT,
        &format!("/api/storage/iscsi/targets/{target_id}"),
        Some(&admin),
        Some(renamed),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("cannot be changed in place")),
        "{body}"
    );

    // Deleting the target frees its LUN, and the backstore can then go.
    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/storage/iscsi/targets/{target_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["targets"], json!([]));
    assert_eq!(body["data"]["backstores"][0]["lun_count"], json!(0));

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/storage/iscsi/backstores/{store_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["backstores"], json!([]));
}

#[tokio::test]
async fn test_a_backstore_served_by_a_lun_cannot_be_forgotten() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let store_id = create_backstore(&mut router, &admin, backstore_body(&h, "data")).await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/targets",
        Some(&admin),
        Some(target_body("iqn.2026-11.local.homevault:disk0", store_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let target_id = body["data"]["targets"][0]["id"].as_i64().unwrap();

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/storage/iscsi/backstores/{store_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("still used")),
        "the refusal must name the reason: {body}"
    );

    // The row is still there, so nothing was half-deleted.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/iscsi",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["backstores"].as_array().unwrap().len(), 1);

    send(
        &mut router,
        Method::DELETE,
        &format!("/api/storage/iscsi/targets/{target_id}"),
        Some(&admin),
        None,
    )
    .await;
    let (status, _) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/storage/iscsi/backstores/{store_id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_deleting_a_backing_file_needs_the_typed_confirmation() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let body = backstore_body(&h, "data");
    let file = PathBuf::from(body["path"].as_str().unwrap());
    std::fs::write(&file, b"pretend to be a disk").expect("backing file");
    let store_id = create_backstore(&mut router, &admin, body).await;

    // Without the typed name the request is refused and the file survives.
    let uri = format!("/api/storage/iscsi/backstores/{store_id}?delete_file=true");
    let (status, body) = send(&mut router, Method::DELETE, &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(file.exists(), "a refused delete must leave the file alone");

    let uri = format!("/api/storage/iscsi/backstores/{store_id}?delete_file=true&confirm=data");
    let (status, body) = send(&mut router, Method::DELETE, &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !file.exists(),
        "the confirmed delete removes the backing file"
    );
}

#[tokio::test]
async fn test_a_block_backstore_is_never_deleted_from_disk() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // Not a real block device either, so the create is refused before the
    // delete path is even reachable — the point is that a `/dev` path is what
    // a block store means, not a file HomeVault could remove.
    let mut body = backstore_body(&h, "raw");
    body["kind"] = json!("block");
    body["path"] = json!("/dev/homevault-does-not-exist");
    let (status, resp) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/backstores",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(
        resp["message"]
            .as_str()
            .is_some_and(|m| m.contains("does not exist")),
        "{resp}"
    );
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_apply_reports_the_missing_host_stack_without_running_anything() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // Turning the feature on only saves the switch; it does not apply.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/storage/iscsi/settings",
        Some(&admin),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["enabled"], json!(true));
    assert!(
        h.root.join("iscsi").exists(),
        "turning iSCSI on creates its default folder"
    );

    let store_id = create_backstore(&mut router, &admin, backstore_body(&h, "data")).await;
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/targets",
        Some(&admin),
        Some(target_body("iqn.2026-11.local.homevault:disk0", store_id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["state"]["pending"],
        json!(true),
        "a saved export is pending until it is applied: {body}"
    );

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/apply",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let caps = &body["data"]["capabilities"];
    let apply = &body["data"]["apply"];
    if caps["targetcli_available"] == json!(false) {
        assert_eq!(apply["applied"], json!(false));
        assert_eq!(apply["reason_code"], json!("no_targetcli"));
        assert_eq!(
            apply["commands"],
            json!([]),
            "a host without targetcli must never receive a command"
        );
        assert_eq!(apply["failed"], json!(null));
        // Nothing was applied, so the metadata is still pending.
        assert_eq!(body["data"]["state"]["pending"], json!(true));
    } else {
        // A machine that really has the LIO stack: the report is a real one,
        // and the test only asserts it is coherent.
        assert!(apply["applied"].is_boolean());
        assert!(apply["commands"].is_array());
    }

    // The switch persists across a fresh read.
    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/storage/iscsi",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["settings"]["enabled"], json!(true));
    assert!(
        h.root.join("data").join("iscsi.json").exists(),
        "the switch must be persisted in the data directory"
    );
}
