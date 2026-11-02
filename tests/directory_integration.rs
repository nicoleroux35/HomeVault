/* =============================================================================
 * File          : directory_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   LDAP/AD directory integration tests (ROADMAP P3.1, stage 1): the auth
 *   matrix, the payload shape, validation refusals, the sealed bind password,
 *   the group→role resync, principal revocation and — the property the whole
 *   design exists for — a directory outage that neither signs anyone in nor
 *   locks the local administrator out.
 *
 *   Nothing here needs an LDAP server. The fake directory is unit-tested in
 *   src/directory.rs; these tests use an address that refuses connections
 *   (127.0.0.1:1) to prove the fail-safe path, and the repository to create
 *   principals without a network round trip.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot)
 *
 * Usage:
 *   cargo test --test directory_integration
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
use homevault::handlers::directory::{
    handle_directory_get, handle_directory_settings_put, handle_directory_test,
    handle_identity_delete, handle_identity_disabled,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::users::handle_user_update_password;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::directory_repo::DIRECTORY_PROVISIONED_HASH;
use homevault::storage::{Database, DirectoryRepository, IdentityUpdate, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// A service-account password long enough to be worth asserting on.
const BIND_PASSWORD: &str = "DirectoryBind!2026";

/// The group a test principal belongs to, when it wants to be an admin.
const ADMIN_GROUP: &str = "cn=admins,ou=groups,dc=example,dc=com";

struct Harness {
    state: Arc<AppState>,
    root: PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Harness {
    fn settings_path(&self) -> PathBuf {
        self.root.join("data").join("directory.json")
    }

    fn repo(&self) -> DirectoryRepository {
        DirectoryRepository::new(self.state.db.pool())
    }
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
        .route("/api/directory", get(handle_directory_get))
        .route(
            "/api/directory/settings",
            put(handle_directory_settings_put),
        )
        .route("/api/directory/test", post(handle_directory_test))
        .route(
            "/api/directory/identities/{id}/disabled",
            post(handle_identity_disabled),
        )
        .route(
            "/api/directory/identities/{id}",
            axum::routing::delete(handle_identity_delete),
        )
        .route(
            "/api/users/{username}/password",
            put(handle_user_update_password),
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

async fn login_with(router: &mut Router, username: &str, password: &str) -> (StatusCode, Value) {
    send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
    )
    .await
}

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (status, body) = login_with(router, username, password).await;
    assert_eq!(status, StatusCode::OK, "{body}");
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

/// A complete, enabled configuration. Tests override the field they exercise.
fn settings_body() -> Value {
    json!({
        "enabled": true,
        "provider": "ldap",
        "server_url": "ldap://127.0.0.1:389",
        "start_tls": false,
        "verify_tls": true,
        "bind_dn": "",
        "bind_password": Value::Null,
        "base_dn": "dc=example,dc=com",
        "user_filter": "(&(objectClass=person)(uid={username}))",
        "group_filter": "",
        "attr_username": "uid",
        "attr_display_name": "cn",
        "attr_email": "mail",
        "attr_member_of": "memberOf",
        "default_role": "user",
        "group_roles": [],
        "cache_ttl_seconds": 300,
    })
}

fn with(body: &Value, key: &str, value: Value) -> Value {
    let mut body = body.clone();
    body[key] = value;
    body
}

async fn put_settings(router: &mut Router, token: &str, body: Value) -> (StatusCode, Value) {
    send(
        router,
        Method::PUT,
        "/api/directory/settings",
        Some(token),
        Some(body),
    )
    .await
}

/// An enabled configuration pointing at a port that refuses connections.
fn unreachable_settings() -> Value {
    with(&settings_body(), "server_url", json!("ldap://127.0.0.1:1"))
}

/// Create a principal the way a first directory sign-in would.
async fn seed_identity(
    h: &Harness,
    username: &str,
    groups: Vec<String>,
    role_id: i64,
) -> homevault::storage::RemoteIdentity {
    h.repo()
        .upsert_login(&IdentityUpdate {
            provider: "ldap".to_string(),
            subject: format!("uid={username},ou=people,dc=example,dc=com"),
            username: username.to_string(),
            display_name: username.to_string(),
            email: format!("{username}@example.com"),
            groups,
            role_id,
        })
        .await
        .expect("seed identity")
}

// ---------------------------------------------------------------------------
// Access control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_directory_screen_is_admin_only() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    let (status, _) = send(&mut router, Method::GET, "/api/directory", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/directory",
        Some(&member),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/directory/settings".to_string(),
            Some(settings_body()),
        ),
        (
            Method::POST,
            "/api/directory/test".to_string(),
            Some(json!({})),
        ),
        (
            Method::POST,
            "/api/directory/identities/1/disabled".to_string(),
            Some(json!({"disabled": true})),
        ),
        (
            Method::DELETE,
            "/api/directory/identities/1".to_string(),
            None,
        ),
    ] {
        let (status, resp) = send(&mut router, method, &uri, Some(&member), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {resp}");
    }
}

// ---------------------------------------------------------------------------
// Payload shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_payload_reports_health_counts_and_a_local_way_in() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::GET,
        "/api/directory",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let data = &body["data"];
    assert_eq!(data["settings"]["enabled"], json!(false));
    assert_eq!(data["settings"]["provider"], json!("ldap"));
    // No password has ever been stored, so nothing to leak.
    assert_eq!(data["settings"]["has_bind_password"], json!(false));
    assert!(data["settings"].get("bind_password_sealed").is_none());
    assert!(data["settings"].get("bind_password").is_none());

    assert_eq!(data["status"]["enabled"], json!(false));
    assert_eq!(data["status"]["usable"], json!(false));
    // Disabled settings are allowed to be incomplete, so there is no problem.
    assert!(data["status"]["problem"].is_null());
    assert_eq!(data["status"]["cache"]["entries"], json!(0));

    assert_eq!(data["identities"], json!([]));
    assert_eq!(data["counts"]["total"], json!(0));
    assert_eq!(data["counts"]["disabled"], json!(0));

    // Break-glass: the admin created by the harness is a local account, and
    // the screen says so before anyone saves anything.
    assert_eq!(data["break_glass"]["local_admins"], json!(1));
    assert!(data["break_glass"]["policy"]
        .as_str()
        .unwrap()
        .contains("local account"));
}

#[tokio::test]
async fn test_an_incomplete_enabled_configuration_is_reported_but_usable_stays_false() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // Enabled with an empty server: validation must complain, and the screen
    // must still render (the admin is preparing the form).
    let (status, body) = put_settings(
        &mut router,
        &admin,
        with(&settings_body(), "server_url", json!("")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("ldap://"));
}

// ---------------------------------------------------------------------------
// Settings validation and persistence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_bind_password_is_sealed_and_never_returned() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let body = with(
        &with(
            &settings_body(),
            "bind_dn",
            json!("cn=homevault,dc=example,dc=com"),
        ),
        "bind_password",
        json!(BIND_PASSWORD),
    );
    let (status, body) = put_settings(&mut router, &admin, body).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["has_bind_password"], json!(true));
    assert_eq!(
        body["data"]["settings"]["bind_dn"],
        json!("cn=homevault,dc=example,dc=com")
    );
    // The response carries the fact, never the secret.
    let rendered = body.to_string();
    assert!(
        !rendered.contains(BIND_PASSWORD),
        "the response leaked the bind password: {rendered}"
    );

    // ...and neither does the file on disk.
    let stored = std::fs::read_to_string(h.settings_path()).expect("settings file");
    assert!(
        !stored.contains(BIND_PASSWORD),
        "the settings file stored the bind password in clear: {stored}"
    );
    assert!(stored.contains("bind_password_sealed"));
    assert_eq!(body["data"]["settings"]["updated_by"], json!("admin"));

    // A second save without a password keeps the stored one: the screen never
    // has to re-type it to change an unrelated field.
    let (status, body) = put_settings(
        &mut router,
        &admin,
        with(
            &with(
                &settings_body(),
                "bind_dn",
                json!("cn=homevault,dc=example,dc=com"),
            ),
            "cache_ttl_seconds",
            json!(60),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["has_bind_password"], json!(true));
    assert_eq!(body["data"]["settings"]["cache_ttl_seconds"], json!(60));

    // An explicit empty string clears it (and the account with it, since a
    // bind password without a bind DN is refused).
    let (status, body) = put_settings(
        &mut router,
        &admin,
        json!({
            "enabled": true,
            "provider": "ldap",
            "server_url": "ldap://127.0.0.1:389",
            "start_tls": false,
            "verify_tls": true,
            "bind_dn": "",
            "bind_password": "",
            "base_dn": "dc=example,dc=com",
            "user_filter": "(&(objectClass=person)(uid={username}))",
            "group_filter": "",
            "attr_username": "uid",
            "attr_display_name": "cn",
            "attr_email": "mail",
            "attr_member_of": "memberOf",
            "default_role": "user",
            "group_roles": [],
            "cache_ttl_seconds": 300,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["has_bind_password"], json!(false));
}

#[tokio::test]
async fn test_every_contradiction_is_refused_with_a_usable_sentence() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "plain-http server",
            with(
                &settings_body(),
                "server_url",
                json!("http://dc.example.com"),
            ),
            "ldap://",
        ),
        (
            "starttls on ldaps",
            with(
                &with(
                    &settings_body(),
                    "server_url",
                    json!("ldaps://dc.example.com"),
                ),
                "start_tls",
                json!(true),
            ),
            "StartTLS",
        ),
        (
            "search base is not a DN",
            with(&settings_body(), "base_dn", json!("example.com")),
            "search base",
        ),
        (
            "user filter without its token",
            with(&settings_body(), "user_filter", json!("(uid=bob)")),
            "{username}",
        ),
        (
            "group filter without its token",
            with(&settings_body(), "group_filter", json!("(member=bob)")),
            "{dn}",
        ),
        (
            "attribute carrying filter syntax",
            with(&settings_body(), "attr_username", json!("uid)(|(uid=*")),
            "login name",
        ),
        (
            "unknown default role",
            with(&settings_body(), "default_role", json!("root")),
            "not a role",
        ),
        (
            "unknown group role",
            with(
                &settings_body(),
                "group_roles",
                json!([{"group": ADMIN_GROUP, "role": "root"}]),
            ),
            "not a role",
        ),
        (
            "the same group mapped twice",
            with(
                &settings_body(),
                "group_roles",
                json!([
                    {"group": ADMIN_GROUP, "role": "admin"},
                    {"group": "CN=Admins,OU=Groups,DC=example,DC=com", "role": "user"}
                ]),
            ),
            "mapped twice",
        ),
        (
            "an unbounded cache",
            with(&settings_body(), "cache_ttl_seconds", json!(86_400)),
            "at most",
        ),
        (
            "a bind account without a password",
            with(
                &settings_body(),
                "bind_dn",
                json!("cn=homevault,dc=example,dc=com"),
            ),
            "no password",
        ),
    ];

    for (label, body, expected) in cases {
        let (status, resp) = put_settings(&mut router, &admin, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {resp}");
        let message = resp["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(expected),
            "{label}: expected '{expected}' in '{message}'"
        );
    }

    // Nothing was written: a refused save must not leave a half-configuration.
    assert!(!h.settings_path().exists());
}

// ---------------------------------------------------------------------------
// Group mapping and the resync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_changing_the_mapping_demotes_existing_principals_immediately() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let identity = seed_identity(
        &h,
        "alice",
        vec![ADMIN_GROUP.to_string()],
        homevault::storage::models::ADMIN_ROLE_ID,
    )
    .await;
    assert_eq!(identity.role_id, 2);

    // Map the group to "user": the principal stored as admin must lose it on
    // save, not on its next sign-in.
    let mut body = settings_body();
    body["group_roles"] = json!([{"group": ADMIN_GROUP, "role": "user"}]);
    let (status, resp) = put_settings(&mut router, &admin, body).await;
    assert_eq!(status, StatusCode::OK, "{resp}");

    let data = &resp["data"];
    assert_eq!(data["identities"][0]["username"], json!("alice"));
    assert_eq!(data["identities"][0]["role_id"], json!(1));
    assert_eq!(data["counts"]["admins"], json!(0));
    assert_eq!(data["counts"]["users"], json!(1));

    // The mirrored `users` row follows, so the session's admin claim is right
    // the next time it is read.
    let (role_id,): (i64,) = sqlx::query_as("SELECT role_id FROM users WHERE username = 'alice'")
        .fetch_one(&*h.state.db.pool())
        .await
        .expect("shadow user");
    assert_eq!(role_id, 1);

    // And the other way round: promoting back is immediate too.
    let mut body = settings_body();
    body["group_roles"] = json!([{"group": ADMIN_GROUP, "role": "admin"}]);
    let (status, resp) = put_settings(&mut router, &admin, body).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    assert_eq!(resp["data"]["identities"][0]["role_id"], json!(2));
    assert_eq!(resp["data"]["counts"]["admins"], json!(1));
}

#[tokio::test]
async fn test_a_directory_principal_cannot_claim_a_local_accounts_name() {
    let h = setup().await;
    let (mut router, _admin, _member) = signed_in(&h).await;

    // `admin` exists as a local account; a directory that returns the same
    // login name must be refused rather than silently taking the account over.
    let result = h
        .repo()
        .upsert_login(&IdentityUpdate {
            provider: "ldap".to_string(),
            subject: "uid=admin,ou=people,dc=example,dc=com".to_string(),
            username: "admin".to_string(),
            display_name: "Not Our Admin".to_string(),
            email: String::new(),
            groups: vec![ADMIN_GROUP.to_string()],
            role_id: homevault::storage::models::ADMIN_ROLE_ID,
        })
        .await;
    assert!(result.is_err(), "a local account must not be claimed");

    // The local account still has its own password and its own hash.
    let (status, body) = login_with(&mut router, "admin", "pass").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (hash, source): (String, String) =
        sqlx::query_as("SELECT password_hash, auth_source FROM users WHERE username = 'admin'")
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert_ne!(hash, DIRECTORY_PROVISIONED_HASH);
    assert_eq!(source, "local");
}

// ---------------------------------------------------------------------------
// Principals: disable and forget
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_disabling_a_principal_revokes_its_sessions() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let identity = seed_identity(&h, "alice", vec![], 1).await;
    let session = h
        .state
        .auth
        .create_session("alice".to_string(), "127.0.0.1".to_string(), false)
        .await
        .expect("session for the directory principal");

    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/directory/identities/{}/disabled", identity.id),
        Some(&admin),
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["identities"][0]["disabled"], json!(true));
    assert_eq!(body["data"]["counts"]["disabled"], json!(1));

    // The token is gone, not merely flagged.
    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions WHERE token = ?")
        .bind(&session.token)
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(remaining, 0);

    // Re-enabling restores the principal but not the revoked session.
    let (status, body) = send(
        &mut router,
        Method::POST,
        &format!("/api/directory/identities/{}/disabled", identity.id),
        Some(&admin),
        Some(json!({"disabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["identities"][0]["disabled"], json!(false));
    assert_eq!(body["data"]["counts"]["disabled"], json!(0));
}

#[tokio::test]
async fn test_forgetting_a_principal_removes_its_mirror_and_its_sessions() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let identity = seed_identity(&h, "bob", vec![], 1).await;
    h.state
        .auth
        .create_session("bob".to_string(), "127.0.0.1".to_string(), false)
        .await
        .expect("session");

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        &format!("/api/directory/identities/{}", identity.id),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["identities"], json!([]));

    let (identities,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM remote_identities WHERE username = 'bob'")
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(identities, 0);

    // The mirror goes with it: a session cannot outlive the principal.
    let (users, sessions): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM users WHERE username = 'bob'), \
                (SELECT COUNT(*) FROM sessions WHERE username = 'bob')",
    )
    .fetch_one(&*h.state.db.pool())
    .await
    .unwrap();
    assert_eq!(users, 0);
    assert_eq!(sessions, 0);

    // A local account of the same name would survive (it can never be the
    // mirror, but the delete is scoped so the guarantee is visible here).
    let (status, _) = login_with(&mut router, "admin", "pass").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_an_unknown_principal_is_a_404_not_a_500() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = send(
        &mut router,
        Method::DELETE,
        "/api/directory/identities/999",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/directory/identities/999/disabled",
        Some(&admin),
        Some(json!({"disabled": true})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ---------------------------------------------------------------------------
// Sign-in behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_directory_account_cannot_sign_in_while_the_integration_is_off() {
    let h = setup().await;
    let (mut router, _admin, _member) = signed_in(&h).await;

    seed_identity(&h, "alice", vec![], 1).await;

    let (status, body) = login_with(&mut router, "alice", "whatever").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("turned off"),
        "the message must name the cause: {message}"
    );

    // A local account is untouched by any of this.
    let (status, _) = login_with(&mut router, "admin", "pass").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_an_unreachable_directory_is_503_and_never_locks_anyone_out() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = put_settings(&mut router, &admin, unreachable_settings()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"]["usable"], json!(true));

    // A name the directory should own: the search cannot run.
    for attempt in 0..3 {
        let (status, body) = login_with(&mut router, "alice", "whatever").await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "attempt {attempt}: {body}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("local account"),
            "the outage must name the way in: {message}"
        );
    }

    // An outage is not a wrong password: no IP ban was recorded.
    let (bans,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ip_bans")
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(bans, 0, "a directory outage must not ban the address");

    // And the break-glass account still signs in, with its own password.
    let (status, body) = login_with(&mut router, "admin", "pass").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (locked,): (bool,) = sqlx::query_as("SELECT locked FROM users WHERE username = 'admin'")
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert!(!locked);
}

#[tokio::test]
async fn test_the_test_button_reports_the_failure_without_saving_anything() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    // Unsaved settings are tried, and nothing is persisted.
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/directory/test",
        Some(&admin),
        Some(json!({"settings": unreachable_settings()})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!h.settings_path().exists(), "a test must not save settings");

    // The same configuration saved, then tested through the stored path.
    let (status, _) = put_settings(&mut router, &admin, unreachable_settings()).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/directory/test",
        Some(&admin),
        Some(json!({"username": "alice", "password": "whatever"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("local account"), "{message}");
}

#[tokio::test]
async fn test_an_invalid_inline_test_configuration_is_refused_before_any_connection() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let body = with(
        &settings_body(),
        "server_url",
        json!("https://dc.example.com"),
    );
    let (status, resp) = send(
        &mut router,
        Method::POST,
        "/api/directory/test",
        Some(&admin),
        Some(json!({"settings": body})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp["message"].as_str().unwrap().contains("ldap://"));
}

// ---------------------------------------------------------------------------
// Password ownership
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_directory_password_cannot_be_changed_here() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    seed_identity(&h, "alice", vec![], 1).await;

    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/users/alice/password",
        Some(&admin),
        Some(json!({"new_password": "N3wPassword!x"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("directory"),
        "the message must say where the password lives: {message}"
    );

    // The sentinel is untouched, so no local password became valid.
    let (hash,): (String,) =
        sqlx::query_as("SELECT password_hash FROM users WHERE username = 'alice'")
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(hash, DIRECTORY_PROVISIONED_HASH);

    // A local account is still changeable.
    let (status, body) = send(
        &mut router,
        Method::PUT,
        "/api/users/admin/password",
        Some(&admin),
        Some(json!({"new_password": "N3wPassword!x", "current_password": "pass"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
