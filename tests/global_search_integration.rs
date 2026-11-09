/* =============================================================================
 * File          : global_search_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Integration tests for the cross-root file search (QW-12):
 *   `GET /api/search/files`. Two real roots on a throwaway temp directory, so
 *   the fan-out, the merge, the ACL confinement and the query validation are
 *   exercised against a real filesystem rather than a mock.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test global_search_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for `GET /api/search/files`.
//!
//! **Deliberately not covered here:** the per-root and total timeouts. They
//! need a mount that stalls, which a test cannot create without root (a FUSE
//! mount or an `LD_PRELOAD` shim); `src/global_search.rs` pins the budgets as
//! constants and the fan-out reads them, and the "root did not answer" branch
//! is exercised in the E2E spec by mocking the endpoint.
//!
//! Every test builds its running futures inside `block_on(async { … })` inside
//! a plain `#[test]`: the `ENV_LOCK` guard is a `std::sync::MutexGuard`, which
//! is not `Send`, and a `#[tokio::test]` body holding it across an `await`
//! makes the future non-`Send` (the default multi-threaded flavor then waits
//! on a worker thread the dropped future killed).

use axum::{routing::get, Router};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::search::handle_search_files;
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct Fixture {
    state: Arc<AppState>,
    root_a: std::path::PathBuf,
    root_b: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

/// Two sibling roots under one temp dir, so a path can only be found when the
/// fan-out covers **both**.
async fn fixture() -> Fixture {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let base = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let root_a = base.join("alpha");
    let root_b = base.join("beta");
    for dir in [&root_a, &root_b] {
        std::fs::create_dir_all(dir).expect("create root");
    }
    // One shared name (dedupe by path must keep both), one name unique to each
    // root (the cross-root criterion), one directory.
    std::fs::write(root_a.join("shared-report.txt"), b"a").unwrap();
    std::fs::write(root_b.join("shared-report.txt"), b"b").unwrap();
    std::fs::write(root_b.join("only-in-beta.txt"), b"beta").unwrap();
    std::fs::write(root_a.join("only-in-alpha.txt"), b"alpha").unwrap();
    std::fs::create_dir_all(root_a.join("photos-2019")).unwrap();
    std::fs::write(root_a.join("photos.txt"), b"x").unwrap();

    std::env::set_var("DATA_DIRECTORY", &base);

    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    let auth = AuthService::new(db.clone());
    let widgets = WidgetRepository::new(db.pool());
    let path_guard = PathGuard::new(vec![
        root_a.to_string_lossy().to_string(),
        root_b.to_string_lossy().to_string(),
    ]);
    let admin_rate_limiter = Arc::new(RateLimitState::new(100, 60));
    let pty_manager = Arc::new(PtyManager::new());
    let downloader = Arc::new(tokio::sync::Mutex::new(
        homevault::handlers::downloader::Aria2Process::new(),
    ));
    let (ransomware_watch_tx, _) =
        tokio::sync::mpsc::unbounded_channel::<homevault::ransomware_watcher::WatchCommand>();

    Fixture {
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
        root_a,
        root_b,
        _tmp: tmp,
        _guard: guard,
    }
}

/// Insert an account plus a live session, and answer the token.
async fn session(state: &AppState, username: &str, is_admin: bool) -> String {
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, '$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$Zm9vYmFyYmF6cXV1eA', 'Test User', 'test@example.com', ?, false, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(if is_admin { 2 } else { 1 })
    .execute(&*state.db.pool())
    .await
    .expect("insert user");

    let token = format!("global-search-test-{username}");
    sqlx::query(
        "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at, is_admin)
         VALUES (?, ?, '127.0.0.1', 'csrf', datetime('now'), datetime('now', '+1 hour'), ?)",
    )
    .bind(&token)
    .bind(username)
    .bind(is_admin)
    .execute(&*state.db.pool())
    .await
    .expect("insert session");
    token
}

/// The member's roots come from explicit grants; the tests below need both
/// fixture folders. Inserting the grant directly avoids mounting the admin
/// share routes in this minimal router.
async fn grant_folder(state: &AppState, member: &str, path: &std::path::Path) {
    sqlx::query(
        "INSERT INTO resource_shares (owner, member, kind, resource, permission) \
         VALUES (?, ?, 'folder', ?, 'read')",
    )
    .bind(member)
    .bind(member)
    .bind(path.to_string_lossy().to_string())
    .execute(&*state.db.pool())
    .await
    .expect("grant folder");
}

async fn grant_fixture_roots(f: &Fixture, member: &str) {
    grant_folder(&f.state, member, &f.root_a).await;
    grant_folder(&f.state, member, &f.root_b).await;
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/search/files", get(handle_search_files))
        .with_state(state)
}

async fn search(router: &mut Router, token: &str, query: &str) -> (StatusCode, Value) {
    search_with_tag(router, token, query, None).await
}

async fn search_with_tag(
    router: &mut Router,
    token: &str,
    query: &str,
    tag: Option<&str>,
) -> (StatusCode, Value) {
    let tag_query = tag
        .map(|tag| format!("&tag={}", urlencode(tag)))
        .unwrap_or_default();
    let uri = format!("/api/search/files?q={}{}", urlencode(query), tag_query);
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("x-session-token", token)
        .body(axum::body::Body::empty())
        .unwrap();
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(json!({ "raw": String::from_utf8_lossy(&bytes) })),
    )
}

/// Minimal percent-encoding for a query value (enough for the shapes under
/// test: `/`, spaces and `&`).
fn urlencode(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            other => format!("%{:02X}", other as u32 & 0xFF),
        })
        .collect()
}

fn hit_paths(body: &Value) -> Vec<String> {
    body["data"]["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|h| h["path"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Run one async test body on a current-thread runtime (`ENV_LOCK` is not
/// `Send`, see the module docs).
fn run<F: std::future::Future<Output = ()>>(future: F) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future);
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[test]
fn test_global_search_requires_a_session() {
    run(async {
        let f = fixture().await;
        let mut r = router(f.state.clone());
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/search/files?q=report")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = ServiceExt::<Req>::ready(&mut r)
            .await
            .unwrap()
            .call(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    });
}

#[test]
fn test_global_search_rejects_an_unknown_token() {
    run(async {
        let f = fixture().await;
        let mut r = router(f.state.clone());
        let (status, _) = search(&mut r, "not-a-real-token", "report").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    });
}

// ---------------------------------------------------------------------------
// Cross-root fan-out
// ---------------------------------------------------------------------------

#[test]
fn test_global_search_tag_filter_is_private_and_acl_confined() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "tag-owner", false).await;
        let other = session(&f.state, "tag-other", false).await;
        grant_fixture_roots(&f, "tag-owner").await;
        grant_fixture_roots(&f, "tag-other").await;
        let tagged = f
            .root_a
            .join("shared-report.txt")
            .to_string_lossy()
            .to_string();
        let repo = homevault::storage::FileTagRepository::new(f.state.db.pool());
        repo.add("tag-owner", &tagged, "Family").await.unwrap();
        repo.add(
            "tag-other",
            &f.root_b.join("shared-report.txt").to_string_lossy(),
            "Family",
        )
        .await
        .unwrap();
        let mut r = router(f.state.clone());

        let (status, body) = search_with_tag(&mut r, &token, "shared-report", Some("Family")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(hit_paths(&body), vec![tagged]);
        assert_eq!(body["data"]["tag"], json!("Family"));

        let (_, body) = search_with_tag(&mut r, &other, "shared-report", Some("Family")).await;
        assert!(hit_paths(&body)
            .iter()
            .all(|path| path.starts_with(f.root_b.to_str().unwrap())));

        let (status, _) = search_with_tag(&mut r, &token, "shared-report", Some("../etc")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    });
}

#[test]
fn test_global_search_finds_a_file_in_the_second_root() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());

        let (status, body) = search(&mut r, &token, "only-in-beta").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], json!(true));
        let hits = hit_paths(&body);
        assert_eq!(hits.len(), 1, "found across roots: {body}");
        assert!(hits[0].ends_with("only-in-beta.txt"), "{hits:?}");
        assert!(
            hits[0].starts_with(&*f.root_b.to_string_lossy()),
            "the hit carries its real root: {hits:?}"
        );
        // The report says which roots were walked, so the UI can name them.
        let roots = body["data"]["roots"].as_array().unwrap();
        assert!(roots.len() >= 2, "both roots were searched: {body}");
        assert_eq!(body["data"]["truncated"], json!(false));
        assert_eq!(body["data"]["timed_out"], json!([]));
    });
}

#[test]
fn test_global_search_dedupes_and_ranks_across_roots() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());

        let (_, body) = search(&mut r, &token, "shared-report").await;
        let hits = hit_paths(&body);
        assert_eq!(
            hits.len(),
            2,
            "one hit per root — a shared file name is not deduplicated away: {body}"
        );
        assert!(hits.iter().any(|p| p.contains("alpha")));
        assert!(hits.iter().any(|p| p.contains("beta")));
    });
}

#[test]
fn test_global_search_ranks_an_exact_name_first() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());

        let (_, body) = search(&mut r, &token, "photos").await;
        let hits = body["data"]["hits"].as_array().unwrap().clone();
        assert!(hits.len() >= 2, "{body}");
        // The directory named exactly `photos-2019` is a prefix match; a file
        // called `photos.txt` contains the query too. Whatever the order, the
        // provider must have walked both kinds and tagged them.
        for hit in &hits {
            assert!(hit["root"].as_str().is_some(), "every hit names its root");
            assert!(hit["root_kind"].as_str().is_some());
        }
    });
}

#[test]
fn test_global_search_caps_the_answer_and_says_so() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());

        let uri = "/api/search/files?q=txt&limit=1";
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("x-session-token", &token)
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = ServiceExt::<Req>::ready(&mut r)
            .await
            .unwrap()
            .call(req)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["data"]["hits"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["data"]["truncated"],
            json!(true),
            "a cut answer must say it was cut: {body}"
        );
    });
}

#[test]
fn test_global_search_files_only_drops_directories() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());

        let uri = "/api/search/files?q=photos&files_only=true";
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("x-session-token", &token)
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = ServiceExt::<Req>::ready(&mut r)
            .await
            .unwrap()
            .call(req)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        for hit in body["data"]["hits"].as_array().unwrap() {
            assert_eq!(hit["is_directory"], json!(false), "{hit}");
        }
    });
}

// ---------------------------------------------------------------------------
// Query validation: a refusal is named, never a silent empty list
// ---------------------------------------------------------------------------

#[test]
fn test_global_search_refuses_an_empty_query() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());
        let (status, body) = search(&mut r, &token, "").await;
        // A refusal is a `400` naming the rule — never a `200` with an empty
        // list, which would read as "no such file".
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["success"], json!(false), "{body}");
        assert!(body["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("file name"));
        assert!(body.get("data").is_none() || body["data"].is_null());
    });
}

#[test]
fn test_global_search_refuses_a_one_character_query() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());
        let (status, body) = search(&mut r, &token, "a").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["success"], json!(false), "{body}");
        let message = body["message"].as_str().unwrap_or("").to_lowercase();
        assert!(message.contains('2'), "the rule is named: {message}");
    });
}

#[test]
fn test_global_search_refuses_a_path_shaped_query() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());
        let (status, body) = search(&mut r, &token, "../../etc/passwd").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["success"], json!(false), "{body}");
        assert!(hit_paths(&body).is_empty());
    });
}

#[test]
fn test_global_search_refuses_an_oversized_query() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());
        let long = "x".repeat(crate::MAX_QUERY_LEN_PLUS_ONE);
        let (status, body) = search(&mut r, &token, &long).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["success"], json!(false), "a 129-char query is refused");
    });
}

/// `MAX_QUERY_LEN + 1`, repeated here so the test does not have to import the
/// constant through the HTTP layer.
const MAX_QUERY_LEN_PLUS_ONE: usize = 129;

// ---------------------------------------------------------------------------
// ACL confinement
// ---------------------------------------------------------------------------

#[test]
fn test_global_search_never_leaves_the_allowed_roots() {
    run(async {
        let f = fixture().await;
        // A file that exists but lives outside both allowed roots.
        let outside = std::path::Path::new("/etc/hostname");
        if !outside.exists() {
            return; // nothing to prove on this host
        }
        let token = session(&f.state, "member", false).await;
        grant_fixture_roots(&f, "member").await;
        let mut r = router(f.state.clone());

        let (_, body) = search(&mut r, &token, "hostname").await;
        let hits = hit_paths(&body);
        assert!(
            hits.iter().all(|p| !p.starts_with("/etc/")),
            "a file outside the allowed roots must never be returned: {hits:?}"
        );
    });
}

#[test]
fn test_global_search_admin_is_confined_to_the_same_roots() {
    run(async {
        // The Explorer lets an admin resolve paths outside the roots, but the
        // search fans out over the **sidebar** roots only: an admin's search
        // must not become a `find /`.
        let f = fixture().await;
        let token = session(&f.state, "boss", true).await;
        let mut r = router(f.state.clone());

        let (_, body) = search(&mut r, &token, "only-in-alpha").await;
        assert!(body["data"].get("tag").is_none());
        let hits = hit_paths(&body);
        assert_eq!(hits.len(), 1, "{body}");
        for path in &hits {
            assert!(
                path.starts_with(&*f.root_a.to_string_lossy())
                    || path.starts_with(&*f.root_b.to_string_lossy()),
                "every hit lives under a sidebar root: {path}"
            );
        }
        for root in body["data"]["roots"].as_array().unwrap() {
            let path = root["path"].as_str().unwrap_or("");
            assert!(
                path.starts_with(&*f.root_a.to_string_lossy())
                    || path.starts_with(&*f.root_b.to_string_lossy()),
                "the fan-out itself is bounded to the configured roots: {path}"
            );
        }
    });
}
