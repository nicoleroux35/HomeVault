/* =============================================================================
 * File          : soak_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 11/11/2026
 * Version       : 1.0.0
 *
 * Description:
 *   Load / soak coverage for the real Axum stack: several concurrent users,
 *   a large file transfer round-trip, a deep directory tree and a bounded
 *   endurance window. Runs in-process (in-memory SQLite + `tower::oneshot`),
 *   so it is safe on CI and needs no privileged host operation.
 *
 * Dependencies:
 *   - tokio, tower, sha2 (already direct dependencies)
 *
 * Usage:
 *   cargo test --test soak_integration -- --nocapture
 *   HV_SOAK_SECONDS=30 HV_SOAK_CONCURRENCY=32 cargo test --test soak_integration -- --nocapture
 * =============================================================================
 */

// The suite serialises `DATA_DIRECTORY` across await points on purpose.
#![allow(clippy::await_holding_lock)]

//! Load / soak tests.
//!
//! Every assertion here answers a production-risk question the functional
//! suites cannot:
//!
//! - **Several users at once** — eight distinct sessions hammer the API in
//!   parallel; a shared lock or a per-session leak shows up as a timeout.
//! - **Large transfers** — an 8 MiB (configurable) upload is streamed back
//!   and byte-compared; the multipart path and the download path must agree
//!   under load.
//! - **Deep trees** — 1 000 files across 40 nested directories; the listing
//!   and the recursive search must stay bounded and finish in a sane time.
//! - **Endurance** — a mixed read workload runs for `HV_SOAK_SECONDS` with a
//!   fixed concurrency, and every request must succeed.
//!
//! Knobs (all optional):
//!   `HV_SOAK_SECONDS`     endurance window, default 2 (max 120)
//!   `HV_SOAK_CONCURRENCY` parallel requests, default 16 (max 128)
//!   `HV_SOAK_FILE_MB`     large-file size, default 8 (max 256)

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::explorer::{
    handle_explorer_disks, handle_explorer_download, handle_explorer_list, handle_explorer_roots,
    handle_explorer_search, handle_explorer_upload,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::{auth::handle_login, AppState};
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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
    let admin_rate_limiter = Arc::new(RateLimitState::new(10_000, 60));
    let pty_manager = Arc::new(PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    let state = Arc::new(AppState {
        auth,
        widgets,
        db,
        path_guard,
        admin_rate_limiter,
        pty_manager,
        downloader,
        ransomware_watch_tx,
    });

    Harness {
        state,
        root,
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/explorer/list", get(handle_explorer_list))
        .route("/api/explorer/search", get(handle_explorer_search))
        .route("/api/explorer/disks", get(handle_explorer_disks))
        .route("/api/explorer/roots", get(handle_explorer_roots))
        .route("/api/explorer/download", get(handle_explorer_download))
        .route("/api/explorer/upload", post(handle_explorer_upload))
        // axum's default multipart ceiling is 2 MiB; the real server raises it
        // for the upload routes. Match that here so the large-transfer case
        // exercises the handler, not the body-limit guard.
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            homevault::middleware::explorer_access_middleware,
        ))
        .with_state(state)
}

async fn create_user(state: &AppState, username: &str, password: &str, role_id: i64) {
    let password_hash = state.auth.hash_password(password).expect("hash");
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, ?, 'Soak User', 'soak@example.com', ?, false, datetime('now'), 'soakseed')",
    )
    .bind(username)
    .bind(&password_hash)
    .bind(role_id)
    .execute(&*state.db.pool())
    .await
    .expect("insert soak user");

    if role_id == 1 {
        for root in state.path_guard.allowed_roots() {
            sqlx::query(
                "INSERT INTO resource_shares (owner, member, kind, resource, permission)
                 VALUES (?, ?, 'folder', ?, 'write')
                 ON CONFLICT(member, kind, resource) DO UPDATE SET permission = 'write'",
            )
            .bind(username)
            .bind(username)
            .bind(root.to_string_lossy().to_string())
            .execute(&*state.db.pool())
            .await
            .expect("grant folder");
        }
    }
}

async fn login(app: &mut Router, username: &str, password: &str) -> String {
    let (status, body) = send(
        app,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "login failed: {body}");
    body["data"]["token"].as_str().expect("token").to_string()
}

async fn send(
    app: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, bytes) = send_raw(app, method, uri, token, body).await;
    let json =
        serde_json::from_slice(&bytes).unwrap_or(json!({ "raw": String::from_utf8_lossy(&bytes) }));
    (status, json)
}

async fn send_raw(
    app: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
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
    let resp = ServiceExt::<Req>::ready(app)
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
    (status, bytes)
}

async fn upload_file(
    app: &mut Router,
    token: &str,
    dir: &Path,
    filename: &str,
    contents: &[u8],
) -> Value {
    let boundary = "----hvsoakboundary42";
    let mut body = Vec::with_capacity(contents.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(contents);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let uri = format!(
        "/api/explorer/upload?path={}",
        urlencode(&dir.to_string_lossy())
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("x-session-token", token)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(axum::body::Body::from(body))
        .unwrap();
    let resp = ServiceExt::<Req>::ready(app)
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
        serde_json::from_slice(&bytes).unwrap_or(json!({ "raw": String::from_utf8_lossy(&bytes) }));
    assert_eq!(status, StatusCode::OK, "upload failed: {json}");
    json
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

fn env_u64(name: &str, default: u64, max: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
        .min(max)
}

/// One parallel request against a cloned router. `Router` is cheap to clone
/// and every clone shares the same `AppState`, exactly like a real server.
async fn one_request(app: Router, token: String, path: String) -> Result<Duration, String> {
    let mut app = app;
    let uri = format!("/api/explorer/list?path={}", urlencode(&path));
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("x-session-token", token)
        .body(axum::body::Body::empty())
        .unwrap();
    let started = Instant::now();
    let resp = tokio::time::timeout(
        Duration::from_secs(15),
        ServiceExt::<Req>::ready(&mut app).await.unwrap().call(req),
    )
    .await
    .map_err(|_| "request timed out after 15 s".to_string())?
    .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|e| e.to_string())?;
    if status != StatusCode::OK {
        return Err(format!("{status}: {}", String::from_utf8_lossy(&body)));
    }
    let json: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if json["success"] != json!(true) {
        return Err(format!("success:false: {json}"));
    }
    Ok(started.elapsed())
}

// ===========================================================================
// 1. Several users at once
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_users_share_the_api_without_errors() {
    let h = setup().await;
    let app = router(h.state.clone());

    // One admin + 7 members, each with its own session.
    create_user(&h.state, "soakadmin", "SoakSecret123", 2).await;
    let mut sessions = Vec::new();
    let mut admin_app = app.clone();
    sessions.push(login(&mut admin_app, "soakadmin", "SoakSecret123").await);

    for i in 0..7 {
        let name = format!("soakuser{i}");
        create_user(&h.state, &name, "SoakSecret123", 1).await;
        let mut member_app = app.clone();
        sessions.push(login(&mut member_app, &name, "SoakSecret123").await);
    }

    let concurrency = env_u64("HV_SOAK_CONCURRENCY", 16, 128) as usize;
    let root = h.root.to_string_lossy().to_string();
    let mut handles = Vec::new();
    for round in 0..concurrency {
        let token = sessions[round % sessions.len()].clone();
        let path = root.clone();
        let app = app.clone();
        handles.push(tokio::spawn(
            async move { one_request(app, token, path).await },
        ));
    }

    let mut durations = Vec::new();
    for handle in handles {
        match handle.await.expect("join") {
            Ok(d) => durations.push(d),
            Err(e) => panic!("a concurrent user request failed: {e}"),
        }
    }

    durations.sort();
    let p95 = durations[(durations.len() * 95 / 100).min(durations.len() - 1)];
    assert!(durations.len() == concurrency);
    assert!(
        p95 < Duration::from_secs(5),
        "p95 latency under 8 concurrent users was {p95:?}"
    );
    println!(
        "concurrent users: {} requests, p95 = {:?}, max = {:?}",
        durations.len(),
        p95,
        durations.last().unwrap()
    );
}

// ===========================================================================
// 2. Large transfer round-trip
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_upload_and_download_round_trip_byte_for_byte() {
    let h = setup().await;
    let mut app = router(h.state.clone());
    create_user(&h.state, "soakadmin", "SoakSecret123", 2).await;
    let token = login(&mut app, "soakadmin", "SoakSecret123").await;

    let megabytes = env_u64("HV_SOAK_FILE_MB", 8, 256) as usize;
    let size = megabytes * 1024 * 1024;
    // Deterministic, non-compressible-enough content; compares byte-for-byte.
    let contents: Vec<u8> = (0..size).map(|i| ((i * 31 + 7) % 251) as u8).collect();
    let upload = upload_file(&mut app, &token, &h.root, "soak-large.bin", &contents).await;
    assert_eq!(upload["success"], json!(true), "upload response: {upload}");

    let file = h.root.join("soak-large.bin");
    assert_eq!(
        std::fs::metadata(&file).expect("uploaded file").len(),
        size as u64
    );

    let uri = format!(
        "/api/explorer/download?path={}",
        urlencode(&file.to_string_lossy())
    );
    let (status, downloaded) = send_raw(&mut app, Method::GET, &uri, Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(downloaded.len(), size, "download length mismatch");

    let sent = Sha256::digest(&contents);
    let received = Sha256::digest(&downloaded);
    assert_eq!(sent, received, "SHA-256 mismatch after the round-trip");
    println!("large transfer: {megabytes} MiB round-trip verified");
}

// ===========================================================================
// 3. Deep directory tree
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_tree_listing_and_search_stay_bounded() {
    let h = setup().await;
    let mut app = router(h.state.clone());
    create_user(&h.state, "soakadmin", "SoakSecret123", 2).await;
    let token = login(&mut app, "soakadmin", "SoakSecret123").await;

    // 40 nested levels x 25 files = 1 000 files. A 40-deep tree is already
    // far past what a home share creates, and catches a recursive walk that
    // is accidentally quadratic.
    let mut dir = h.root.clone();
    for depth in 0..40 {
        dir = dir.join(format!("level-{depth:02}"));
        std::fs::create_dir_all(&dir).expect("create level");
        for file in 0..25 {
            std::fs::write(dir.join(format!("file-{file:02}.txt")), b"soak").expect("write file");
        }
    }
    let deepest = dir.clone();

    let started = Instant::now();
    let (status, listing) = send(
        &mut app,
        Method::GET,
        &format!(
            "/api/explorer/list?path={}",
            urlencode(&deepest.to_string_lossy())
        ),
        Some(&token),
        None,
    )
    .await;
    let list_elapsed = started.elapsed();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["success"], json!(true), "listing: {listing}");
    let entries = listing["data"]["entries"]
        .as_array()
        .or_else(|| listing["data"].as_array())
        .expect("listing entries");
    assert_eq!(entries.len(), 25, "one directory, 25 entries");
    assert!(
        list_elapsed < Duration::from_secs(2),
        "listing took {list_elapsed:?}"
    );

    let started = Instant::now();
    let (status, search) = send(
        &mut app,
        Method::GET,
        &format!(
            "/api/explorer/search?path={}&query=file-",
            urlencode(&h.root.to_string_lossy())
        ),
        Some(&token),
        None,
    )
    .await;
    let search_elapsed = started.elapsed();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(search["success"], json!(true), "search: {search}");
    assert!(
        search_elapsed < Duration::from_secs(15),
        "recursive search over 1 000 files took {search_elapsed:?}"
    );
    println!(
        "deep tree: listing 25 entries in {list_elapsed:?}, search over 1 000 files in {search_elapsed:?}"
    );
}

// ===========================================================================
// 4. Endurance window
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_workload_survives_an_endurance_window() {
    let h = setup().await;
    let app = router(h.state.clone());
    create_user(&h.state, "soakadmin", "SoakSecret123", 2).await;
    let mut admin_app = app.clone();
    let token = login(&mut admin_app, "soakadmin", "SoakSecret123").await;

    let seconds = env_u64("HV_SOAK_SECONDS", 2, 120);
    let concurrency = env_u64("HV_SOAK_CONCURRENCY", 16, 128);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let root = h.root.to_string_lossy().to_string();

    let mut handles = Vec::new();
    for worker in 0..concurrency {
        let app = app.clone();
        let token = token.clone();
        let path = root.clone();
        handles.push(tokio::spawn(async move {
            let mut completed = 0u64;
            let mut failures = Vec::new();
            let mut worst = Duration::ZERO;
            while Instant::now() < deadline {
                let uri = match worker % 3 {
                    0 => format!("/api/explorer/list?path={}", urlencode(&path)),
                    1 => "/api/explorer/disks".to_string(),
                    _ => "/api/explorer/roots".to_string(),
                };
                let req = Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .header("x-session-token", &token)
                    .body(axum::body::Body::empty())
                    .unwrap();
                let started = Instant::now();
                let mut attempt = app.clone();
                let result = tokio::time::timeout(Duration::from_secs(15), async {
                    ServiceExt::<Req>::ready(&mut attempt)
                        .await
                        .unwrap()
                        .call(req)
                        .await
                })
                .await;
                let elapsed = started.elapsed();
                match result {
                    Ok(Ok(resp)) if resp.status() == StatusCode::OK => {
                        let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
                            .await
                            .unwrap_or_default();
                        match serde_json::from_slice::<Value>(&body) {
                            Ok(json) if json["success"] == json!(true) => {
                                completed += 1;
                                worst = worst.max(elapsed);
                            }
                            Ok(json) => failures.push(format!("success:false: {json}")),
                            Err(e) => failures.push(format!("invalid JSON: {e}")),
                        }
                    }
                    Ok(Ok(resp)) => failures.push(format!("status {}", resp.status())),
                    Ok(Err(e)) => failures.push(format!("transport: {e}")),
                    Err(_) => failures.push("timeout after 15 s".to_string()),
                }
            }
            (completed, failures, worst)
        }));
    }

    let mut total = 0u64;
    let mut worst = Duration::ZERO;
    for handle in handles {
        let (completed, failures, worker_worst) = handle.await.expect("join");
        assert!(
            failures.is_empty(),
            "endurance workload recorded {} failure(s): {:?}",
            failures.len(),
            &failures[..failures.len().min(5)]
        );
        total += completed;
        worst = worst.max(worker_worst);
    }

    assert!(total > 0, "the endurance window completed no request");
    println!(
        "endurance: {total} requests in {seconds}s at concurrency {concurrency}, worst latency {worst:?}"
    );
}
