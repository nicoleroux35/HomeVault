/* =============================================================================
 * File          : oidc_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   OpenID Connect sign-in tests (ROADMAP P3.1, stage 2): the auth matrix, the
 *   sealed client secret, the group→role resync, and a complete sign-in driven
 *   against a real in-process identity provider — a live HTTP server publishing
 *   a discovery document, a key set and a token endpoint, signing a real ES256
 *   ID token that the appliance verifies with `ring`.
 *
 *   The refusals get as much attention as the happy path: a callback without the
 *   binding cookie, a replayed state, a wrong nonce, a wrong audience, an
 *   expired token, a disabled integration and a one-shot ticket used twice all
 *   have to fail closed.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot + ring for the fake signer)
 *
 * Usage:
 *   cargo test --test oidc_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    extract::{Query, State},
    http::{HeaderMap, Method, Request, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::oidc::{
    handle_auth_methods, handle_oidc_callback, handle_oidc_complete, handle_oidc_get,
    handle_oidc_settings_put, handle_oidc_start, handle_oidc_test,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::oidc;
use homevault::storage::directory_repo::IdentityUpdate;
use homevault::storage::{Database, DirectoryRepository, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const CLIENT_ID: &str = "homevault-test";
const CLIENT_SECRET: &str = "SuperSecret!2026";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    state: Arc<AppState>,
    root: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Harness {
    fn settings_path(&self) -> std::path::PathBuf {
        self.root.join("data").join("oidc.json")
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

    // A stale settings document from another test would be read by `oidc::load`.
    oidc::clear_caches();

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
        .route("/api/auth/methods", get(handle_auth_methods))
        .route("/api/oidc", get(handle_oidc_get))
        .route("/api/oidc/settings", put(handle_oidc_settings_put))
        .route("/api/oidc/test", post(handle_oidc_test))
        .route("/api/auth/oidc/start", post(handle_oidc_start))
        .route("/api/auth/oidc/callback", get(handle_oidc_callback))
        .route("/api/auth/oidc/complete", post(handle_oidc_complete))
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

/// A response as a test needs to see it: status, the headers that matter and
/// the decoded body.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Answer {
    fn location(&self) -> String {
        self.headers
            .get("location")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_string)
            .collect()
    }

    /// The value of one `Set-Cookie`, without its attributes.
    fn cookie(&self, name: &str) -> Option<String> {
        self.set_cookies()
            .iter()
            .find(|cookie| cookie.starts_with(&format!("{name}=")))
            .map(|cookie| {
                cookie
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches(&format!("{name}="))
                    .to_string()
            })
    }

    fn raw(&self) -> String {
        self.body.to_string()
    }
}

async fn send_full(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("x-session-token", t);
    }
    if let Some(c) = cookie {
        req = req.header("cookie", c);
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
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    let body =
        serde_json::from_slice(&bytes).unwrap_or(json!({"raw": String::from_utf8_lossy(&bytes)}));
    Answer {
        status,
        headers,
        body,
    }
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let answer = send_full(router, method, uri, token, None, body).await;
    (answer.status, answer.body)
}

async fn login(router: &mut Router, username: &str, password: &str) -> String {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
    )
    .await;
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

// ---------------------------------------------------------------------------
// A test identity provider
// ---------------------------------------------------------------------------

/// What the fake provider learns from the appliance while a sign-in runs.
#[derive(Debug, Default)]
struct Idp {
    /// The ID token `/token` hands back.
    token: Mutex<Option<String>>,
    /// The token request body, so the test can prove PKCE and the code were
    /// actually sent.
    token_form: Mutex<Option<String>>,
    /// The `Authorization` header of the token request.
    token_auth: Mutex<Option<String>>,
    /// Every request path, in order — proves a callback really happened.
    seen: Mutex<Vec<String>>,
}

struct FakeIdp {
    issuer: String,
    state: Arc<Idp>,
}

async fn start_idp(jwks: Value) -> FakeIdp {
    let state = Arc::new(Idp::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind idp");
    let address = listener.local_addr().expect("idp address");
    let issuer = format!("http://{address}");

    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get({
                let issuer = issuer.clone();
                move || async move {
                    Json(json!({
                        "issuer": issuer,
                        "authorization_endpoint": format!("{issuer}/authorize"),
                        "token_endpoint": format!("{issuer}/token"),
                        "jwks_uri": format!("{issuer}/jwks"),
                        "id_token_signing_alg_values_supported": ["ES256"],
                    }))
                }
            }),
        )
        .route(
            "/jwks",
            get(move || {
                let jwks = jwks.clone();
                async move { Json(jwks) }
            }),
        )
        .route(
            "/token",
            post(
                |State(state): State<Arc<Idp>>, headers: HeaderMap, body: String| async move {
                    if let Ok(mut seen) = state.seen.lock() {
                        seen.push("/token".to_string());
                    }
                    *state.token_form.lock().unwrap() = Some(body);
                    *state.token_auth.lock().unwrap() = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string);
                    match state.token.lock().unwrap().clone() {
                        Some(token) => Json(json!({ "id_token": token })).into_response(),
                        None => (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": "no token configured" })),
                        )
                            .into_response(),
                    }
                },
            ),
        )
        .route("/authorize", get(|Query(_): Query<Value>| async { "ok" }))
        .with_state(state.clone());

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    FakeIdp { issuer, state }
}

impl FakeIdp {
    fn set_token(&self, token: &str) {
        *self.state.token.lock().unwrap() = Some(token.to_string());
    }

    fn token_form(&self) -> String {
        self.state
            .token_form
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }

    fn token_auth(&self) -> String {
        self.state
            .token_auth
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default()
    }
}

// -- A real ES256 signer ----------------------------------------------------

struct Signer {
    jwk: Value,
    pair: ring::signature::EcdsaKeyPair,
    rng: ring::rand::SystemRandom,
}

fn signer() -> Signer {
    use ring::signature::{EcdsaKeyPair, KeyPair};
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .expect("generate key");
    let pair = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &rng,
    )
    .expect("load key");
    let public = pair.public_key().as_ref();
    assert_eq!(public.len(), 65, "uncompressed P-256 point");
    let jwk = json!({
        "kty": "EC",
        "kid": "integration-key",
        "alg": "ES256",
        "use": "sig",
        "crv": "P-256",
        "x": oidc::b64url(&public[1..33]),
        "y": oidc::b64url(&public[33..65]),
    });
    Signer { jwk, pair, rng }
}

impl Signer {
    fn sign_claims(&self, header: Value, claims: &Value) -> String {
        let header = oidc::b64url(&serde_json::to_vec(&header).unwrap());
        let payload = oidc::b64url(&serde_json::to_vec(claims).unwrap());
        let signing_input = format!("{header}.{payload}");
        let signature = self
            .pair
            .sign(&self.rng, signing_input.as_bytes())
            .expect("sign")
            .as_ref()
            .to_vec();
        format!("{signing_input}.{}", oidc::b64url(&signature))
    }

    fn sign(&self, claims: &Value) -> String {
        self.sign_claims(
            json!({"alg": "ES256", "typ": "JWT", "kid": "integration-key"}),
            claims,
        )
    }
}

fn id_token_claims(issuer: &str, audience: &str, nonce: &str) -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "iss": issuer,
        "aud": audience,
        "sub": "9f8e7d6c",
        "preferred_username": "alice",
        "name": "Alice Martin",
        "email": "alice@example.test",
        "groups": ["admins"],
        "iat": now - 5,
        "exp": now + 300,
        "nonce": nonce,
    })
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

/// A complete, enabled configuration pointing at a provider.
fn settings_body(issuer: &str, secret: Option<&str>) -> Value {
    json!({
        "enabled": true,
        "label": "Sign in with Test IDP",
        "issuer": issuer,
        "client_id": CLIENT_ID,
        "client_secret": secret,
        "redirect_url": format!("{issuer}/api/auth/oidc/callback"),
        "scopes": "openid profile email",
        "attr_username": "preferred_username",
        "attr_display_name": "name",
        "attr_email": "email",
        "attr_groups": "groups",
        "default_role": "user",
        "group_roles": [],
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
        "/api/oidc/settings",
        Some(token),
        Some(body),
    )
    .await
}

/// Run a whole sign-in up to (not including) the ticket exchange.
///
/// Returns the callback answer and the fake provider, so each test can assert
/// on the provider's view of the flow as well as the appliance's.
struct SignIn {
    callback: Answer,
    idp: FakeIdp,
    state: String,
    challenge: String,
}

impl SignIn {
    fn ticket(&self) -> String {
        self.callback
            .location()
            .split("#sso=")
            .nth(1)
            .unwrap_or_default()
            .split('&')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn error_code(&self) -> String {
        self.callback
            .location()
            .split("sso_error=")
            .nth(1)
            .unwrap_or_default()
            .to_string()
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_sign_in(
    router: &mut Router,
    idp: &FakeIdp,
    signer: &Signer,
    code: &str,
    send_cookie: bool,
    tamper: impl FnOnce(&mut Value),
) -> SignIn {
    // 1. Start: the appliance answers with the provider URL and binds the flow
    //    to this browser with a cookie.
    let start = send_full(
        router,
        Method::POST,
        "/api/auth/oidc/start",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.raw());
    let authorize_url = start.body["data"]["authorize_url"]
        .as_str()
        .expect("authorize_url")
        .to_string();
    let parsed = url::Url::parse(&authorize_url).expect("authorize url");
    let params: std::collections::HashMap<String, String> =
        parsed.query_pairs().into_owned().collect();

    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(params.get("client_id").map(String::as_str), Some(CLIENT_ID));
    assert!(
        !authorize_url.contains("client_secret"),
        "the secret must never reach the browser"
    );
    let state = params["state"].clone();
    let nonce = params["nonce"].clone();
    let challenge = params["code_challenge"].clone();
    assert_eq!(
        start.cookie(oidc::STATE_COOKIE).as_deref(),
        Some(state.as_str()),
        "the state cookie must match the state in the URL"
    );

    // 2. The provider signs an ID token carrying that nonce.
    let mut claims = id_token_claims(&idp.issuer, CLIENT_ID, &nonce);
    tamper(&mut claims);
    idp.set_token(&signer.sign(&claims));

    // 3. The browser comes back.
    let cookie = if send_cookie {
        Some(format!("{}={state}", oidc::STATE_COOKIE))
    } else {
        None
    };
    let callback = send_full(
        router,
        Method::GET,
        &format!(
            "/api/auth/oidc/callback?code={code}&state={state}",
            code = percent(code),
            state = percent(&state)
        ),
        None,
        cookie.as_deref(),
        None,
    )
    .await;

    SignIn {
        callback,
        idp: FakeIdp {
            issuer: idp.issuer.clone(),
            state: idp.state.clone(),
        },
        state,
        challenge,
    }
}

fn percent(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// Create a principal the way an earlier OIDC sign-in would.
async fn seed_identity(
    h: &Harness,
    username: &str,
    groups: Vec<String>,
    role_id: i64,
) -> homevault::storage::RemoteIdentity {
    h.repo()
        .upsert_login(&IdentityUpdate {
            provider: oidc::PROVIDER_OIDC.to_string(),
            subject: format!("https://idp.example.test#{username}"),
            username: username.to_string(),
            display_name: username.to_string(),
            email: format!("{username}@example.test"),
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
async fn test_the_oidc_screen_is_admin_only() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;

    for token in [None, Some(member.as_str())] {
        let (status, _) = send(&mut router, Method::GET, "/api/oidc", token, None).await;
        assert_eq!(
            status,
            if token.is_none() {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );

        let (status, body) = send(
            &mut router,
            Method::PUT,
            "/api/oidc/settings",
            token,
            Some(settings_body("https://idp.example.test", None)),
        )
        .await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{body}"
        );

        let (status, _) = send(
            &mut router,
            Method::POST,
            "/api/oidc/test",
            token,
            Some(json!({})),
        )
        .await;
        assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn test_a_member_cannot_read_the_settings() {
    let h = setup().await;
    let (mut router, _admin, member) = signed_in(&h).await;
    let (status, body) = send(&mut router, Method::GET, "/api/oidc", Some(&member), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!body.to_string().contains("client_secret"));
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_the_client_secret_is_sealed_and_never_returned() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    let (status, body) = put_settings(
        &mut router,
        &admin,
        settings_body("https://idp.example.test", Some(CLIENT_SECRET)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["has_client_secret"], json!(true));
    assert!(
        !body.to_string().contains(CLIENT_SECRET),
        "the response must not echo the secret"
    );

    let (status, body) = send(&mut router, Method::GET, "/api/oidc", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["settings"]["has_client_secret"], json!(true));
    assert!(!body.to_string().contains(CLIENT_SECRET));

    // At rest it is sealed, not stored in the clear.
    let document = std::fs::read_to_string(h.settings_path()).expect("settings file");
    assert!(document.contains("client_secret_sealed"));
    assert!(
        !document.contains(CLIENT_SECRET),
        "the secret must not appear in oidc.json"
    );

    // An update that omits the secret keeps the stored one.
    let (status, body) = put_settings(
        &mut router,
        &admin,
        with(
            &settings_body("https://idp.example.test", None),
            "label",
            json!("Another label"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["has_client_secret"], json!(true));

    // An empty string clears it (a public client).
    let (status, body) = put_settings(
        &mut router,
        &admin,
        settings_body("https://idp.example.test", Some("")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["settings"]["has_client_secret"], json!(false));
}

#[tokio::test]
async fn test_settings_validation_refuses_unsafe_urls() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;

    for (key, value, needle) in [
        ("issuer", json!("http://idp.example.test"), "issuer"),
        ("client_id", json!(""), "client ID"),
        (
            "redirect_url",
            json!("https://nas.example.test/login"),
            oidc::CALLBACK_PATH,
        ),
        ("scopes", json!("profile email"), "openid"),
        ("default_role", json!("owner"), "owner"),
        ("attr_username", json!("preferred username"), "login name"),
    ] {
        let body = with(
            &settings_body("https://idp.example.test", Some(CLIENT_SECRET)),
            key,
            value,
        );
        let (status, body) = put_settings(&mut router, &admin, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}: {body}");
        let message = body["message"]
            .as_str()
            .unwrap_or(&body.to_string())
            .to_string();
        assert!(message.contains(needle), "{key}: {message}");
    }

    // Nothing was written by a refused save.
    let (status, body) = send(&mut router, Method::GET, "/api/oidc", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["settings"]["enabled"], json!(false));
}

#[tokio::test]
async fn test_saving_new_group_mappings_rescores_existing_principals() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let identity = seed_identity(&h, "alice", vec!["admins".into()], 1).await;
    assert_eq!(identity.role_id, 1, "starts as a member");

    let body = with(
        &settings_body("https://idp.example.test", None),
        "group_roles",
        json!([{ "group": "admins", "role": "admin" }]),
    );
    let (status, body) = put_settings(&mut router, &admin, body).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let stored = h.repo().find(identity.id).await.unwrap().expect("identity");
    assert_eq!(stored.role_id, 2, "the mapping promotes immediately");
    assert_eq!(body["data"]["counts"]["admins"], json!(1));

    // And a mapping removed demotes just as fast.
    let (status, _) = put_settings(
        &mut router,
        &admin,
        settings_body("https://idp.example.test", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stored = h.repo().find(identity.id).await.unwrap().expect("identity");
    assert_eq!(stored.role_id, 1, "the mapped group is gone");
}

#[tokio::test]
async fn test_the_screen_reports_the_break_glass_administrator() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let (status, body) = send(&mut router, Method::GET, "/api/oidc", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["break_glass"]["local_admins"], json!(1));
    assert_eq!(body["data"]["status"]["enabled"], json!(false));
    assert_eq!(body["data"]["counts"]["total"], json!(0));
}

#[tokio::test]
async fn test_the_test_endpoint_reports_an_unreachable_provider() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    // A loopback port that is closed: the probe must answer with a sentence,
    // not hang and not save anything.
    let body = with(
        &settings_body("http://127.0.0.1:1", None),
        "redirect_url",
        json!("http://127.0.0.1:1/api/auth/oidc/callback"),
    );
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/oidc/test",
        Some(&admin),
        Some(json!({ "settings": body })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("unavailable"), "{message}");
}

// ---------------------------------------------------------------------------
// What the login screen is told
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_auth_methods_offers_single_sign_on_only_when_it_works() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    create_test_user(&h.state, "admin", "pass", 2).await;
    let admin = login(&mut router, "admin", "pass").await;

    let (status, body) = send(&mut router, Method::GET, "/api/auth/methods", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["password"], json!(true));
    assert_eq!(body["data"]["oidc"]["enabled"], json!(false));

    let (status, _) = put_settings(
        &mut router,
        &admin,
        settings_body("https://idp.example.test", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&mut router, Method::GET, "/api/auth/methods", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["oidc"]["enabled"], json!(true));
    assert_eq!(
        body["data"]["oidc"]["label"],
        json!("Sign in with Test IDP")
    );
    // The public endpoint must not leak the issuer or the client ID.
    assert!(!body.to_string().contains("idp.example.test"));
    assert!(!body.to_string().contains(CLIENT_ID));
}

#[tokio::test]
async fn test_start_refuses_a_disabled_integration() {
    let h = setup().await;
    let (mut router, _admin, _member) = signed_in(&h).await;
    let answer = send_full(
        &mut router,
        Method::POST,
        "/api/auth/oidc/start",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!answer.raw().contains("authorize_url"));
}

// ---------------------------------------------------------------------------
// The full sign-in
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_complete_sign_in_creates_a_session_and_a_mirrored_account() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    // The provider's "admins" group is what makes this principal an
    // administrator — the mapping, not the token, decides.
    let body = with(
        &settings_body(&idp.issuer, Some(CLIENT_SECRET)),
        "group_roles",
        json!([{ "group": "admins", "role": "admin" }]),
    );
    let (status, body) = put_settings(&mut router, &admin, body).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let sign_in = run_sign_in(&mut router, &idp, &signer, "code-1", true, |_| {}).await;

    // The callback redirects to the login screen, ticket in the fragment.
    assert_eq!(
        sign_in.callback.status,
        StatusCode::SEE_OTHER,
        "{}",
        sign_in.callback.raw()
    );
    let location = sign_in.callback.location();
    assert!(
        location.starts_with("/login#sso="),
        "expected a ticket redirect, got {location}"
    );
    assert!(location.contains("next=%2Fdesktop"), "{location}");
    assert!(
        sign_in.callback.cookie(oidc::STATE_COOKIE).as_deref() == Some(""),
        "the state cookie must be cleared"
    );

    // PKCE really happened: the verifier sent to the token endpoint hashes to
    // the challenge the browser was given.
    let form = sign_in.idp.token_form();
    let fields: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect();
    assert_eq!(
        fields.get("grant_type").map(String::as_str),
        Some("authorization_code")
    );
    assert_eq!(fields.get("code").map(String::as_str), Some("code-1"));
    let verifier = fields.get("code_verifier").expect("PKCE verifier");
    assert_eq!(
        oidc::pkce_challenge(verifier),
        sign_in.challenge,
        "the verifier must match the challenge"
    );
    // A confidential client authenticates with HTTP Basic, never a URL field.
    let basic = sign_in.idp.token_auth();
    assert!(basic.starts_with("Basic "), "{basic}");
    assert!(
        !form.contains(CLIENT_SECRET),
        "the secret must never be in the body"
    );
    assert!(!fields.contains_key("client_secret"));

    // The ticket buys the session the SPA stores.
    let ticket = sign_in.ticket();
    assert!(!ticket.is_empty(), "no ticket in {location}");
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/auth/oidc/complete",
        None,
        Some(json!({ "code": ticket })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["username"], json!("alice"));
    assert_eq!(
        body["data"]["is_admin"],
        json!(true),
        "the admins group maps to admin"
    );
    let session = body["data"]["token"].as_str().expect("token").to_string();
    assert!(!session.is_empty());

    // The session works against the API and the mirrored account is real.
    let (status, body) = send(&mut router, Method::GET, "/api/oidc", Some(&session), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let identity = h
        .repo()
        .find_by_username("alice")
        .await
        .unwrap()
        .expect("the principal is mirrored");
    assert_eq!(identity.provider, oidc::PROVIDER_OIDC);
    assert_eq!(identity.display_name, "Alice Martin");
    assert_eq!(identity.email, "alice@example.test");
    assert_eq!(identity.subject, format!("{}#9f8e7d6c", idp.issuer));
    assert_eq!(
        h.repo().auth_source("alice").await.unwrap().as_deref(),
        Some("directory"),
        "an OIDC account is never a local password account"
    );

    // The one-shot ticket is gone.
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/auth/oidc/complete",
        None,
        Some(json!({ "code": sign_in.ticket() })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_the_secret_is_used_only_when_one_is_stored() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    // A public client: PKCE only, no secret.
    let (status, _) = put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;
    assert_eq!(status, StatusCode::OK);

    let sign_in = run_sign_in(&mut router, &idp, &signer, "code-2", true, |_| {}).await;
    assert!(sign_in.callback.location().starts_with("/login#sso="));
    assert_eq!(
        sign_in.idp.token_auth(),
        "",
        "a public client must not send an Authorization header"
    );
    let fields: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(sign_in.idp.token_form().as_bytes())
            .into_owned()
            .collect();
    assert_eq!(fields.get("client_id").map(String::as_str), Some(CLIENT_ID));
    assert!(fields.contains_key("code_verifier"));
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_callback_without_the_binding_cookie_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    let sign_in = run_sign_in(&mut router, &idp, &signer, "code-3", false, |_| {}).await;
    assert_eq!(sign_in.callback.status, StatusCode::SEE_OTHER);
    assert_eq!(sign_in.error_code(), "state");
    assert!(
        sign_in.idp.token_form().is_empty(),
        "a state mismatch must not even reach the token endpoint"
    );
    assert!(h.repo().find_by_username("alice").await.unwrap().is_none());
}

#[tokio::test]
async fn test_a_replayed_callback_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    let sign_in = run_sign_in(&mut router, &idp, &signer, "code-4", true, |_| {}).await;
    assert!(sign_in.callback.location().starts_with("/login#sso="));

    // Replaying the same code+state+cookie finds nothing: the state was
    // consumed by the first callback.
    let replay = send_full(
        &mut router,
        Method::GET,
        &format!(
            "/api/auth/oidc/callback?code=code-4&state={}",
            percent(&sign_in.state)
        ),
        None,
        Some(&format!("{}={}", oidc::STATE_COOKIE, sign_in.state)),
        None,
    )
    .await;
    assert_eq!(replay.status, StatusCode::SEE_OTHER);
    assert_eq!(
        replay.location(),
        "/login?sso_error=expired",
        "{:?}",
        replay.location()
    );
}

#[tokio::test]
async fn test_a_token_for_another_nonce_audience_or_issuer_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    for (label, tamper) in [
        (
            "nonce",
            Box::new(|claims: &mut Value| {
                claims["nonce"] = json!("someone-elses-nonce");
            }) as Box<dyn FnOnce(&mut Value)>,
        ),
        (
            "audience",
            Box::new(|claims: &mut Value| {
                claims["aud"] = json!("another-application");
            }),
        ),
        (
            "issuer",
            Box::new(|claims: &mut Value| {
                claims["iss"] = json!("https://evil.example.test");
            }),
        ),
        (
            "expiry",
            Box::new(|claims: &mut Value| {
                claims["exp"] = json!(chrono::Utc::now().timestamp() - 3_600);
            }),
        ),
    ] {
        let sign_in = run_sign_in(&mut router, &idp, &signer, "code-tampered", true, tamper).await;
        assert_eq!(sign_in.error_code(), "refused", "{label} must be refused");
        assert!(
            !sign_in.callback.raw().contains("sso="),
            "{label} must not mint a ticket"
        );
    }
    assert!(h.repo().find_by_username("alice").await.unwrap().is_none());
}

#[tokio::test]
async fn test_an_unsigned_token_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    let start = send_full(
        &mut router,
        Method::POST,
        "/api/auth/oidc/start",
        None,
        None,
        None,
    )
    .await;
    let authorize_url = start.body["data"]["authorize_url"].as_str().unwrap();
    let params: std::collections::HashMap<String, String> = url::Url::parse(authorize_url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    let state = params["state"].clone();

    // `alg: none` with an empty signature: the classic JWT bypass. Every claim
    // is otherwise perfect, so only the algorithm can refuse it.
    let forged_header =
        oidc::b64url(&serde_json::to_vec(&json!({"alg": "none", "typ": "JWT"})).unwrap());
    let forged_claims = oidc::b64url(
        &serde_json::to_vec(&id_token_claims(&idp.issuer, CLIENT_ID, &params["nonce"])).unwrap(),
    );
    idp.set_token(&format!("{forged_header}.{forged_claims}."));

    let callback = send_full(
        &mut router,
        Method::GET,
        &format!(
            "/api/auth/oidc/callback?code=code-none&state={}",
            percent(&state)
        ),
        None,
        Some(&format!("{}={state}", oidc::STATE_COOKIE)),
        None,
    )
    .await;
    assert_eq!(callback.location(), "/login?sso_error=refused");
    assert!(h.repo().find_by_username("alice").await.unwrap().is_none());
}

#[tokio::test]
async fn test_a_token_signed_by_an_unpublished_key_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let published = signer();
    let idp = start_idp(json!({ "keys": [published.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    // Signed by a key the provider never published, but with a matching `kid`
    // so the key set is not even refetched.
    let impostor = signer();
    let sign_in = run_sign_in(&mut router, &idp, &impostor, "code-impostor", true, |_| {}).await;
    assert_eq!(sign_in.error_code(), "refused");
    assert!(h.repo().find_by_username("alice").await.unwrap().is_none());
}

#[tokio::test]
async fn test_switching_the_integration_off_refuses_a_pending_callback() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    // Start a flow, then switch the integration off before the provider answers.
    let start = send_full(
        &mut router,
        Method::POST,
        "/api/auth/oidc/start",
        None,
        None,
        None,
    )
    .await;
    let authorize_url = start.body["data"]["authorize_url"].as_str().unwrap();
    let params: std::collections::HashMap<String, String> = url::Url::parse(authorize_url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    let state = params["state"].clone();
    idp.set_token(&signer.sign(&id_token_claims(&idp.issuer, CLIENT_ID, &params["nonce"])));

    let (status, _) = put_settings(
        &mut router,
        &admin,
        with(&settings_body(&idp.issuer, None), "enabled", json!(false)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let callback = send_full(
        &mut router,
        Method::GET,
        &format!(
            "/api/auth/oidc/callback?code=code-off&state={}",
            percent(&state)
        ),
        None,
        Some(&format!("{}={state}", oidc::STATE_COOKIE)),
        None,
    )
    .await;
    assert_eq!(callback.location(), "/login?sso_error=disabled");
    assert!(h.repo().find_by_username("alice").await.unwrap().is_none());
}

#[tokio::test]
async fn test_a_provider_error_lands_on_the_login_screen() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    let start = send_full(
        &mut router,
        Method::POST,
        "/api/auth/oidc/start",
        None,
        None,
        None,
    )
    .await;
    let state = start.cookie(oidc::STATE_COOKIE).expect("state cookie");

    // The user clicked "cancel" at the provider.
    let callback = send_full(
        &mut router,
        Method::GET,
        "/api/auth/oidc/callback?error=access_denied&state=whatever",
        None,
        Some(&format!("{}={state}", oidc::STATE_COOKIE)),
        None,
    )
    .await;
    assert_eq!(callback.status, StatusCode::SEE_OTHER);
    assert_eq!(callback.location(), "/login?sso_error=denied");
}

#[tokio::test]
async fn test_a_callback_without_a_code_or_state_is_refused() {
    let h = setup().await;
    let (mut router, _admin, _member) = signed_in(&h).await;
    for uri in [
        "/api/auth/oidc/callback",
        "/api/auth/oidc/callback?code=only-a-code",
        "/api/auth/oidc/callback?state=only-a-state",
        "/api/auth/oidc/callback?code=&state=",
    ] {
        let callback = send_full(&mut router, Method::GET, uri, None, None, None).await;
        assert_eq!(callback.status, StatusCode::SEE_OTHER, "{uri}");
        assert_eq!(callback.location(), "/login?sso_error=state", "{uri}");
    }
}

// ---------------------------------------------------------------------------
// The mirrored account is not a local account
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_a_single_sign_on_account_cannot_use_the_password_form() {
    let h = setup().await;
    let (mut router, _admin, _member) = signed_in(&h).await;
    seed_identity(&h, "alice", vec![], 1).await;

    // No password exists for this account, and the form must say where to go
    // instead of "invalid credentials".
    let (status, body) = send(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": "alice", "password": "anything"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("single sign-on"),
        "the message must point at the right door: {message}"
    );

    // A local account with the same kind of password still works.
    let local = login(&mut router, "member", "pass").await;
    assert!(!local.is_empty());
}

#[tokio::test]
async fn test_a_disabled_mirrored_account_cannot_sign_in() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    // First sign-in mirrors the account, then an administrator disables it.
    let first = run_sign_in(&mut router, &idp, &signer, "code-first", true, |_| {}).await;
    let ticket = first.ticket();
    let (status, _) = send(
        &mut router,
        Method::POST,
        "/api/auth/oidc/complete",
        None,
        Some(json!({ "code": ticket })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let identity = h.repo().find_by_username("alice").await.unwrap().unwrap();
    h.repo()
        .set_disabled(identity.id, &identity.username, true)
        .await
        .unwrap();

    let second = run_sign_in(&mut router, &idp, &signer, "code-second", true, |_| {}).await;
    assert_eq!(second.error_code(), "account_disabled");
    assert!(second.ticket().is_empty());
}

#[tokio::test]
async fn test_a_claim_without_a_usable_name_is_refused_with_a_pointer() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    let sign_in = run_sign_in(
        &mut router,
        &idp,
        &signer,
        "code-bad-name",
        true,
        |claims| {
            claims["preferred_username"] = json!("x");
        },
    )
    .await;
    assert_eq!(sign_in.error_code(), "claim");
}

#[tokio::test]
async fn test_a_name_collision_with_a_local_account_is_refused() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    // The provider vouches for "member", which is a local account here.
    let sign_in = run_sign_in(
        &mut router,
        &idp,
        &signer,
        "code-collision",
        true,
        |claims| {
            claims["preferred_username"] = json!("member");
        },
    )
    .await;
    assert_eq!(
        sign_in.error_code(),
        "account",
        "a remote principal must never take over a local account"
    );
    // The local account's password still works and it is still local.
    assert!(!login(&mut router, "member", "pass").await.is_empty());
    assert_eq!(
        h.repo().auth_source("member").await.unwrap().as_deref(),
        Some("local")
    );
}

#[tokio::test]
async fn test_a_reserved_account_name_is_refused_as_a_claim() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    let signer = signer();
    let idp = start_idp(json!({ "keys": [signer.jwk.clone()] })).await;
    put_settings(&mut router, &admin, settings_body(&idp.issuer, None)).await;

    // "admin" is a reserved HomeVault name: refused before touching the
    // database, so no account and no chance of a takeover.
    let sign_in = run_sign_in(
        &mut router,
        &idp,
        &signer,
        "code-reserved",
        true,
        |claims| {
            claims["preferred_username"] = json!("admin");
        },
    )
    .await;
    assert_eq!(sign_in.error_code(), "claim");
    assert_eq!(
        h.repo().auth_source("admin").await.unwrap().as_deref(),
        Some("local")
    );
}

#[tokio::test]
async fn test_an_unreachable_provider_does_not_block_local_sign_in() {
    let h = setup().await;
    let (mut router, admin, _member) = signed_in(&h).await;
    // Enabled, pointing at a port that refuses connections.
    let body = with(
        &settings_body("http://127.0.0.1:1", None),
        "redirect_url",
        json!("http://127.0.0.1:1/api/auth/oidc/callback"),
    );
    let (status, _) = put_settings(&mut router, &admin, body).await;
    assert_eq!(status, StatusCode::OK);

    // Starting a flow fails with a sentence, not a hang or a panic.
    let start = send_full(
        &mut router,
        Method::POST,
        "/api/auth/oidc/start",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(start.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(start.raw().contains("unavailable"), "{}", start.raw());

    // And the local administrator can still get in: break-glass.
    assert!(!login(&mut router, "admin", "pass").await.is_empty());
    assert!(!login(&mut router, "member", "pass").await.is_empty());
}
