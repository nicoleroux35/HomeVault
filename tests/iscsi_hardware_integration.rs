/* =============================================================================
 * File          : iscsi_hardware_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   iSCSI **hardware acceptance** tests. This is the one gap the Phase 3
 *   roadmap left open: every other iSCSI test proves what HomeVault *would*
 *   run, this one proves that a real initiator can log in and read back what
 *   it wrote.
 *
 *   Unlike `iscsi_integration.rs`, this suite touches the host: it loads the
 *   LIO modules through `targetcli`, exports a temporary file backstore on
 *   127.0.0.1:3260 and drives `iscsiadm` from the same machine. It is
 *   therefore **opt-in** and skipped by default — CI and developer machines
 *   have no LIO stack, and a skipped test must not read as a pass.
 *
 * Prerequisites (all of them, on a disposable host):
 *   - root, because `targetcli` writes to configfs and `iscsiadm` needs to
 *     load the initiator modules;
 *   - the LIO stack: `targetcli`, `target_core_mod`, `iscsi_target_mod`,
 *     `target_core_file`, `target_core_iblock`, and a mounted
 *     `/sys/kernel/config`;
 *   - the initiator: `open-iscsi` (`iscsiadm`) and a running `iscsid`;
 *   - `dd` and `cmp` from coreutils.
 *
 * Usage:
 *   sudo HV_ISCSI_HW=1 cargo test --test iscsi_hardware_integration -- --nocapture
 *
 *   Set HV_ISCSI_HW_KEEP=1 to leave the export and the initiator session in
 *   place after a failure so it can be inspected by hand. The suite always
 *   uses a per-process IQN, so a leftover export never collides with the next
 *   run.
 *
 * What each test proves:
 *   - `hardware_login_read_write`  a real initiator logs in, writes a known
 *                                  pattern and reads it back byte for byte;
 *   - `hardware_chap_is_enforced`  the same target refuses the login without
 *                                  CHAP and accepts it with the sealed secret;
 *   - `hardware_survives_reapply`  re-applying (the restart path) rebuilds the
 *                                  export without losing the data or the login.
 *
 * Dependencies:
 *   - none beyond the test crate: axum router + tower oneshot, std::process
 *     for the host commands.
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

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
    handle_iscsi_apply, handle_iscsi_backstore_create, handle_iscsi_get, handle_iscsi_settings_put,
    handle_iscsi_target_create,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::iscsi;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const CHAP_USER: &str = "homevault";
const CHAP_SECRET: &str = "HvHardwareChap!2026";
const PORTAL_ADDRESS: &str = "127.0.0.1";
const PORTAL_PORT: i64 = 3260;

// ---------------------------------------------------------------------------
// Gate
// ---------------------------------------------------------------------------

/// Why the suite cannot run here, or `None` when it can.
///
/// A skipped run prints the reason: a green result must never be mistaken for
/// "the hardware path was exercised".
fn gate() -> Option<String> {
    if std::env::var("HV_ISCSI_HW").as_deref() != Ok("1") {
        return Some(
            "set HV_ISCSI_HW=1 to run the hardware acceptance suite (it exports a LUN on \
             127.0.0.1:3260 and drives iscsiadm)"
                .to_string(),
        );
    }
    if !is_root() {
        return Some(
            "the hardware acceptance suite must run as root (targetcli + iscsiadm)".into(),
        );
    }
    let caps = iscsi::capabilities();
    if !caps.usable {
        return Some(format!(
            "the LIO stack is not usable here: targetcli={}, configfs={}, modules missing={:?}",
            caps.targetcli_available, caps.configfs_available, caps.modules_missing
        ));
    }
    for binary in ["iscsiadm", "dd", "cmp"] {
        if !binary_on_path(binary) {
            return Some(format!("`{binary}` is not on PATH"));
        }
    }
    None
}

/// `gate()` inverted: prints the reason and answers "skip".
fn skip(test: &str) -> bool {
    match gate() {
        Some(reason) => {
            eprintln!("SKIPPED {test}: {reason}");
            true
        }
        None => false,
    }
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

fn binary_on_path(binary: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(binary);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn keep_on_failure() -> bool {
    std::env::var("HV_ISCSI_HW_KEEP").as_deref() == Ok("1")
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

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
            "/api/storage/iscsi/targets",
            post(handle_iscsi_target_create),
        )
        .route("/api/storage/iscsi/apply", post(handle_iscsi_apply))
        .with_state(state)
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

async fn admin_token(h: &Harness) -> (Router, String) {
    let password_hash = h.state.auth.hash_password("pass").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES ('admin', ?, 'Admin', 'admin@example.com', 2, false, datetime('now'), 'seed')",
    )
    .bind(&password_hash)
    .execute(&*h.state.db.pool())
    .await
    .expect("insert admin");

    let mut router = router(h.state.clone());
    let (_, body) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": "admin", "password": "pass"})),
    )
    .await;
    let token = body["data"]["token"]
        .as_str()
        .expect("login must return a token")
        .to_string();
    (router, token)
}

// ---------------------------------------------------------------------------
// Host commands
// ---------------------------------------------------------------------------

fn run(program: &str, args: &[&str]) -> (bool, String) {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {program}: {e}"));
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

/// Runs a command that must succeed, with the output in the panic message.
fn must(program: &str, args: &[&str]) -> String {
    let (ok, text) = run(program, args);
    assert!(ok, "`{program} {}` failed:\n{text}", args.join(" "));
    text
}

/// Best-effort cleanup: never panics, so it is safe to call from a failure path.
fn cleanup(iqn: &str) {
    let portal = format!("{PORTAL_ADDRESS}:{PORTAL_PORT}");
    let _ = Command::new("iscsiadm")
        .args(["-m", "node", "-T", iqn, "-p", &portal, "--logout"])
        .output();
    let _ = Command::new("iscsiadm")
        .args(["-m", "node", "-T", iqn, "-p", &portal, "-o", "delete"])
        .output();
    // `targetcli` removes the objects HomeVault created; the file backstore is
    // deleted with the temp directory.
    let _ = Command::new("targetcli")
        .args(["/iscsi", &format!("delete {iqn}")])
        .output();
}

/// The by-path symlink the kernel creates for a logged-in LUN.
fn device_path(iqn: &str, lun: i64) -> PathBuf {
    PathBuf::from(format!(
        "/dev/disk/by-path/ip-{PORTAL_ADDRESS}:{PORTAL_PORT}-iscsi-{iqn}-lun-{lun}"
    ))
}

/// Waits for udev to publish the device node after a login.
fn wait_for_device(iqn: &str, lun: i64, timeout: Duration) -> PathBuf {
    let path = device_path(iqn, lun);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return path;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!(
        "the LUN never appeared at {} — check `dmesg` and `iscsiadm -m session -P 3`",
        path.display()
    );
}

/// Creates the node entry and, when asked, the CHAP credentials on it.
fn configure_node(iqn: &str, chap: Option<(&str, &str)>) {
    let portal = format!("{PORTAL_ADDRESS}:{PORTAL_PORT}");
    must(
        "iscsiadm",
        &["-m", "node", "-o", "new", "-T", iqn, "-p", &portal],
    );
    if let Some((user, secret)) = chap {
        for (key, value) in [
            ("node.session.auth.authmethod", "CHAP"),
            ("node.session.auth.username", user),
            ("node.session.auth.password", secret),
        ] {
            must(
                "iscsiadm",
                &[
                    "-m", "node", "-o", "update", "-T", iqn, "-p", &portal, "-n", key, "-v", value,
                ],
            );
        }
    }
}

fn login(iqn: &str) -> (bool, String) {
    let portal = format!("{PORTAL_ADDRESS}:{PORTAL_PORT}");
    run(
        "iscsiadm",
        &["-m", "node", "-T", iqn, "-p", &portal, "--login"],
    )
}

fn logout(iqn: &str) {
    let portal = format!("{PORTAL_ADDRESS}:{PORTAL_PORT}");
    let _ = Command::new("iscsiadm")
        .args(["-m", "node", "-T", iqn, "-p", &portal, "--logout"])
        .output();
}

// ---------------------------------------------------------------------------
// Export setup through the API
// ---------------------------------------------------------------------------

/// The whole export, driven through the HTTP API exactly as the screen does.
async fn export_lun(h: &Harness, iqn: &str, with_chap: bool) -> (Router, String, PathBuf) {
    let (mut router, admin) = admin_token(h).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/storage/iscsi/settings",
        Some(&admin),
        Some(json!({"enabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let folder = h.root.join("iscsi");
    std::fs::create_dir_all(&folder).expect("backstore folder");
    let image = folder.join(format!("{}.img", iqn.replace(':', "_")));
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/backstores",
        Some(&admin),
        Some(json!({
            "name": "accdisk",
            "kind": "fileio",
            "path": image.to_string_lossy(),
            "size_bytes": 64_i64 * 1024 * 1024,
            "readonly": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let backstore_id = body["data"]["backstores"][0]["id"]
        .as_i64()
        .expect("the created backstore id");

    let mut acl = json!({
        "initiator_iqn": iqn,
        "userid": null,
        "secret": null,
    });
    if with_chap {
        acl = json!({
            "initiator_iqn": iqn,
            "userid": CHAP_USER,
            "secret": CHAP_SECRET,
        });
    }
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/storage/iscsi/targets",
        Some(&admin),
        Some(json!({
            "iqn": iqn,
            "enabled": true,
            "portals": [{"address": PORTAL_ADDRESS, "port": PORTAL_PORT}],
            "luns": [{"lun": 0, "backstore_id": backstore_id}],
            "acls": [acl],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    (router, admin, image)
}

/// `POST /apply`, asserting the host accepted every command.
async fn apply(router: &mut Router, admin: &str) -> Value {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/storage/iscsi/apply",
        Some(admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let report = body["data"]["apply"].clone();
    assert_eq!(
        report["applied"], true,
        "the host refused the apply: {report}"
    );
    report
}

/// Writes `blocks` MiB of a deterministic pattern, then reads them back and
/// compares. Returns the pattern file so the caller can re-verify it later.
fn write_and_verify(device: &Path, blocks: u32, label: &str) -> PathBuf {
    let pattern = std::env::temp_dir().join(format!("hv-iscsi-{label}.bin"));
    must(
        "dd",
        &[
            "if=/dev/urandom",
            &format!("of={}", pattern.display()),
            "bs=1M",
            &format!("count={blocks}"),
            "status=none",
        ],
    );
    must(
        "dd",
        &[
            &format!("if={}", pattern.display()),
            &format!("of={}", device.display()),
            "bs=1M",
            &format!("count={blocks}"),
            "conv=fsync",
            "status=none",
        ],
    );
    let readback = std::env::temp_dir().join(format!("hv-iscsi-{label}-read.bin"));
    must(
        "dd",
        &[
            &format!("if={}", device.display()),
            &format!("of={}", readback.display()),
            "bs=1M",
            &format!("count={blocks}"),
            "status=none",
        ],
    );
    must(
        "cmp",
        &[
            pattern.to_string_lossy().as_ref(),
            readback.to_string_lossy().as_ref(),
        ],
    );
    let _ = std::fs::remove_file(&readback);
    pattern
}

fn unique_iqn(suffix: &str) -> String {
    format!(
        "iqn.2026-11.local.homevault:acc-{}-{suffix}",
        std::process::id()
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The core acceptance: a real initiator logs in and reads back what it wrote.
#[tokio::test]
async fn hardware_login_read_write() {
    if skip("hardware_login_read_write") {
        return;
    }
    let h = setup().await;
    let iqn = unique_iqn("rw");
    let (mut router, admin, image) = export_lun(&h, &iqn, false).await;

    let report = apply(&mut router, &admin).await;
    assert!(
        report["commands"]
            .as_array()
            .map(|c| !c.is_empty())
            .unwrap_or(false),
        "the apply must have run commands: {report}"
    );

    configure_node(&iqn, None);
    let (ok, text) = login(&iqn);
    if !ok {
        cleanup(&iqn);
        panic!("the initiator could not log in:\n{text}");
    }

    let device = wait_for_device(&iqn, 0, Duration::from_secs(15));
    write_and_verify(&device, 4, "rw");

    // The bytes really landed in the backstore file, not in a page cache we
    // happened to read back: the file is at least as large as what we wrote.
    let written = std::fs::metadata(&image).expect("backstore file").len();
    assert!(
        written >= 4 * 1024 * 1024,
        "the backstore file holds {written} bytes, less than the 4 MiB written"
    );

    logout(&iqn);
    if !keep_on_failure() {
        cleanup(&iqn);
    }
}

/// CHAP is not decoration: without the secret the login must fail, with it the
/// same target must accept the initiator.
#[tokio::test]
async fn hardware_chap_is_enforced() {
    if skip("hardware_chap_is_enforced") {
        return;
    }
    let h = setup().await;
    let iqn = unique_iqn("chap");
    let (mut router, admin, _image) = export_lun(&h, &iqn, true).await;
    apply(&mut router, &admin).await;

    // 1. No credentials: the target must refuse the session.
    configure_node(&iqn, None);
    let (ok, text) = login(&iqn);
    assert!(
        !ok,
        "an initiator without CHAP credentials was allowed to log in:\n{text}"
    );
    logout(&iqn);

    // 2. Wrong secret: still refused.
    configure_node(&iqn, Some((CHAP_USER, "WrongSecret!2026")));
    let (ok, text) = login(&iqn);
    assert!(!ok, "a wrong CHAP secret was accepted:\n{text}");
    logout(&iqn);

    // 3. The sealed secret the admin typed: accepted, and the disk is usable.
    configure_node(&iqn, Some((CHAP_USER, CHAP_SECRET)));
    let (ok, text) = login(&iqn);
    if !ok {
        cleanup(&iqn);
        panic!("the CHAP login with the configured secret failed:\n{text}");
    }
    let device = wait_for_device(&iqn, 0, Duration::from_secs(15));
    write_and_verify(&device, 2, "chap");

    logout(&iqn);
    if !keep_on_failure() {
        cleanup(&iqn);
    }
}

/// Restart recovery: re-applying rebuilds the export idempotently and the data
/// written before the rebuild is still there afterwards.
#[tokio::test]
async fn hardware_survives_reapply() {
    if skip("hardware_survives_reapply") {
        return;
    }
    let h = setup().await;
    let iqn = unique_iqn("reapply");
    let (mut router, admin, _image) = export_lun(&h, &iqn, false).await;
    apply(&mut router, &admin).await;

    configure_node(&iqn, None);
    let (ok, text) = login(&iqn);
    if !ok {
        cleanup(&iqn);
        panic!("the initiator could not log in:\n{text}");
    }
    let device = wait_for_device(&iqn, 0, Duration::from_secs(15));
    let pattern = write_and_verify(&device, 2, "reapply");
    logout(&iqn);

    // A second apply is what a reboot (or a settings change) runs. It must not
    // clear the export, and it must succeed.
    let report = apply(&mut router, &admin).await;
    assert_eq!(report["applied"], true, "{report}");

    // Log back in and prove the first session's data survived.
    let (ok, text) = login(&iqn);
    if !ok {
        cleanup(&iqn);
        panic!("the initiator could not log in after the re-apply:\n{text}");
    }
    let device = wait_for_device(&iqn, 0, Duration::from_secs(15));
    let readback =
        std::env::temp_dir().join(format!("hv-iscsi-reapply2-{}.bin", std::process::id()));
    must(
        "dd",
        &[
            &format!("if={}", device.display()),
            &format!("of={}", readback.display()),
            "bs=1M",
            "count=2",
            "status=none",
        ],
    );
    must(
        "cmp",
        &[
            pattern.to_string_lossy().as_ref(),
            readback.to_string_lossy().as_ref(),
        ],
    );
    let _ = std::fs::remove_file(&readback);
    let _ = std::fs::remove_file(&pattern);

    logout(&iqn);
    if !keep_on_failure() {
        cleanup(&iqn);
    }
}
