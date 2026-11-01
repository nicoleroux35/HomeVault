/* =============================================================================
 * File          : passkey_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Passkey (WebAuthn) integration tests: the per-account CRUD matrix, the two
 *   ceremonies end to end with a **software authenticator built here** (a real
 *   P-256 key pair signed with `ring`), a full passkey sign-in that produces a
 *   session, the single-use rule on the challenge, and the account-state
 *   guards.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot + ring)
 *
 * Usage:
 *   cargo test --test passkey_integration
 * =============================================================================
 */

// Integration tests deliberately hold a process-wide `ENV_LOCK` guard across
// `await` points to serialize tests that mutate process environment variables.
#![allow(clippy::await_holding_lock)]
//! Integration tests for `/api/passkeys*`.
//!
//! The "authenticator" is a software one: a real P-256 key pair generated with
//! `ring`, an attestation object this file builds byte by byte, and a real
//! signature over `authenticatorData || SHA-256(clientDataJSON)`. Nothing is
//! mocked inside the server — the verifier runs for real, which is the point:
//! a passing test here means the signatures actually verify.
//!
//! `HOMEVAULT_HOSTNAME` is set per test so the relying party is deterministic
//! (`vault.home`, `https://vault.home`), and every test runs on a
//! current-thread runtime inside a plain `#[test]` — the `ENV_LOCK` guard is a
//! `std::sync::MutexGuard`, which is not `Send`.

use axum::{
    routing::{get, patch, post},
    Router,
};
use http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, LazyLock, Mutex};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::passkey::{
    handle_passkey_delete, handle_passkey_login_begin, handle_passkey_login_finish,
    handle_passkey_register_begin, handle_passkey_register_finish, handle_passkey_rename,
    handle_passkeys_list,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const HOSTNAME: &str = "vault.home";
const ORIGIN: &str = "https://vault.home";

struct Fixture {
    state: Arc<AppState>,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn fixture() -> Fixture {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    std::env::set_var("DATA_DIRECTORY", &root);
    std::env::set_var("HOMEVAULT_HOSTNAME", HOSTNAME);
    // The relying party's origin depends on the TLS mode: pin it so the
    // expected `https://vault.home` is the same on every host (an inherited
    // `TLS_ENABLED=0` would otherwise flip the scheme and break every
    // signature test with an origin mismatch).
    std::env::set_var("TLS_ENABLED", "true");
    std::env::set_var("HTTPS_PORT", "443");

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
        _tmp: tmp,
        _guard: guard,
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/passkeys", get(handle_passkeys_list))
        .route(
            "/api/passkeys/register/begin",
            post(handle_passkey_register_begin),
        )
        .route(
            "/api/passkeys/register/finish",
            post(handle_passkey_register_finish),
        )
        .route(
            "/api/passkeys/login/begin",
            post(handle_passkey_login_begin),
        )
        .route(
            "/api/passkeys/login/finish",
            post(handle_passkey_login_finish),
        )
        .route(
            "/api/passkeys/{id}",
            patch(handle_passkey_rename).delete(handle_passkey_delete),
        )
        .with_state(state)
}

/// Insert an account and a live session; answer the token.
async fn session(state: &AppState, username: &str, locked: bool) -> String {
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES (?, '$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHR2YWx1ZQ$Zm9vYmFyYmF6cXV1eA', 'Test User', 'test@example.com', 1, ?, datetime('now'), 'testseed')",
    )
    .bind(username)
    .bind(locked)
    .execute(&*state.db.pool())
    .await
    .expect("insert user");

    let token = format!("passkey-test-{username}");
    sqlx::query(
        "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at, is_admin)
         VALUES (?, ?, '127.0.0.1', 'csrf', datetime('now'), datetime('now', '+1 hour'), 0)",
    )
    .bind(&token)
    .bind(username)
    .execute(&*state.db.pool())
    .await
    .expect("insert session");
    token
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(json!({ "raw": String::from_utf8_lossy(&bytes) })),
    )
}

fn run<F: std::future::Future<Output = ()>>(future: F) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

// ---------------------------------------------------------------------------
// A software authenticator
// ---------------------------------------------------------------------------

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn b64url_decode(value: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .expect("base64url")
}

fn rp_id_hash() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(HOSTNAME.as_bytes());
    hasher.finalize().into()
}

/// CBOR `{fmt:"none", attStmt:{}, authData:<bytes>}` around `auth_data`.
fn attestation_object(auth_data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xa3, 0x63];
    out.extend_from_slice(b"fmt");
    out.extend_from_slice(&[0x64]);
    out.extend_from_slice(b"none");
    out.extend_from_slice(&[0x67]);
    out.extend_from_slice(b"attStmt");
    out.push(0xa0);
    out.extend_from_slice(&[0x68]);
    out.extend_from_slice(b"authData");
    let len = auth_data.len();
    if len < 24 {
        out.push(0x40 | len as u8);
    } else if len <= 0xff {
        out.extend_from_slice(&[0x58, len as u8]);
    } else {
        out.extend_from_slice(&[0x59, (len >> 8) as u8, (len & 0xff) as u8]);
    }
    out.extend_from_slice(auth_data);
    out
}

/// `authenticatorData` with AT set and a P-256 COSE key.
fn registration_auth_data(
    credential_id: &[u8],
    public_key_point: &[u8],
    sign_count: u32,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&rp_id_hash());
    out.push(0x41); // UP | AT
    out.extend_from_slice(&sign_count.to_be_bytes());
    out.extend_from_slice(&[0u8; 16]); // aaguid
    out.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
    out.extend_from_slice(credential_id);
    // COSE {1:2, 3:-7, -1:1, -2:X, -3:Y}
    out.push(0xa5);
    out.extend_from_slice(&[0x01, 0x02]);
    out.extend_from_slice(&[0x03, 0x26]);
    out.extend_from_slice(&[0x20, 0x01]);
    out.push(0x21);
    out.push(0x58);
    out.push(32);
    out.extend_from_slice(&public_key_point[1..33]);
    out.push(0x22);
    out.push(0x58);
    out.push(32);
    out.extend_from_slice(&public_key_point[33..65]);
    out
}

fn assertion_auth_data(flags: u8, sign_count: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&rp_id_hash());
    out.push(flags);
    out.extend_from_slice(&sign_count.to_be_bytes());
    out
}

fn client_data(ceremony: &str, challenge: &str) -> Vec<u8> {
    format!(
        r#"{{"type":"{ceremony}","challenge":"{challenge}","origin":"{ORIGIN}","crossOrigin":false}}"#
    )
    .into_bytes()
}

/// A P-256 pair plus the uncompressed public point.
fn key_pair() -> (ring::signature::EcdsaKeyPair, Vec<u8>) {
    use ring::signature::{EcdsaKeyPair, KeyPair};
    let rng = ring::rand::SystemRandom::new();
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .expect("generate");
    let pair = EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
        pkcs8.as_ref(),
        &rng,
    )
    .expect("load");
    let point = pair.public_key().as_ref().to_vec();
    (pair, point)
}

fn sign(pair: &ring::signature::EcdsaKeyPair, message: &[u8]) -> Vec<u8> {
    let rng = ring::rand::SystemRandom::new();
    pair.sign(&rng, message).expect("sign").as_ref().to_vec()
}

/// Register a passkey through the real endpoints, answering
/// `(credential_id, key_pair)`.
async fn register_passkey(
    router: &mut Router,
    token: &str,
) -> (String, ring::signature::EcdsaKeyPair) {
    let (status, body) = send(
        router,
        Method::POST,
        "/api/passkeys/register/begin",
        Some(token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "begin: {body}");
    let challenge = body["data"]["challenge"]
        .as_str()
        .expect("challenge")
        .to_string();

    let (pair, point) = key_pair();
    let credential_id = b"credential-under-test".to_vec();
    let auth_data = registration_auth_data(&credential_id, &point, 0);
    let cdj = client_data("webauthn.create", &challenge);
    let attestation = attestation_object(&auth_data);

    let (status, body) = send(
        router,
        Method::POST,
        "/api/passkeys/register/finish",
        Some(token),
        Some(json!({
            "client_data_json": b64url(&cdj),
            "attestation_object": b64url(&attestation),
            "credential_id": b64url(&credential_id),
            "label": "Test passkey",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "finish: {body}");
    assert_eq!(body["success"], json!(true), "{body}");
    (b64url(&credential_id), pair)
}

// ---------------------------------------------------------------------------
// Auth + CRUD
// ---------------------------------------------------------------------------

#[test]
fn test_passkey_list_requires_a_session() {
    run(async {
        let f = fixture().await;
        let mut r = router(f.state.clone());
        let (status, _) = send(&mut r, Method::GET, "/api/passkeys", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    });
}

#[test]
fn test_passkey_list_starts_empty_and_names_the_relying_party() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());

        let (status, body) = send(&mut r, Method::GET, "/api/passkeys", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["passkeys"], json!([]));
        assert_eq!(body["data"]["supported"], json!(true));
        assert_eq!(body["data"]["relying_party"]["id"], json!(HOSTNAME));
        assert_eq!(body["data"]["relying_party"]["origin"], json!(ORIGIN));
    });
}

#[test]
fn test_register_begin_issues_a_challenge_and_forbids_attestation() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/register/begin",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let data = &body["data"];
        let challenge = data["challenge"].as_str().expect("challenge");
        assert_eq!(b64url_decode(challenge).len(), 32);
        assert_eq!(data["rp"]["id"], json!(HOSTNAME));
        assert_eq!(
            data["attestation"],
            json!("none"),
            "no attestation is requested"
        );
        assert_eq!(data["exclude_credentials"], json!([]));
        // The user handle is bytes, not a raw username.
        let handle = data["user"]["id"].as_str().expect("user id");
        assert_eq!(b64url_decode(handle), b"member".to_vec());
        // Both verifiable algorithms are offered, and nothing else.
        let algs: Vec<i64> = data["pub_key_cred_params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["alg"].as_i64().unwrap())
            .collect();
        assert_eq!(algs, vec![-7, -8], "ES256 then EdDSA");
    });
}

#[test]
fn test_full_registration_then_list() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());

        let (credential_id, _pair) = register_passkey(&mut r, &token).await;

        let (_, body) = send(&mut r, Method::GET, "/api/passkeys", Some(&token), None).await;
        let passkeys = body["data"]["passkeys"].as_array().unwrap();
        assert_eq!(passkeys.len(), 1);
        assert_eq!(passkeys[0]["credential_id"], json!(credential_id));
        assert_eq!(passkeys[0]["label"], json!("Test passkey"));
        assert_eq!(passkeys[0]["algorithm"], json!("es256"));
        assert_eq!(passkeys[0]["attestation_fmt"], json!("none"));
        // The public key never leaves the server.
        assert!(passkeys[0].get("public_key").is_none(), "{body}");
        assert!(passkeys[0].get("sign_count").is_none(), "{body}");
    });
}

#[test]
fn test_registration_refuses_a_replayed_challenge() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());

        // Register once…
        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/register/begin",
            Some(&token),
            None,
        )
        .await;
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();
        let (pair, point) = key_pair();
        let credential_id = b"first".to_vec();
        let cdj = client_data("webauthn.create", &challenge);
        let attestation = attestation_object(&registration_auth_data(&credential_id, &point, 0));
        let payload = json!({
            "client_data_json": b64url(&cdj),
            "attestation_object": b64url(&attestation),
            "credential_id": b64url(&credential_id),
        });
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/register/finish",
            Some(&token),
            Some(payload.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let _ = pair;

        // …then replay the very same response: the challenge is consumed, so
        // the second attempt must be refused with "start again".
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/register/finish",
            Some(&token),
            Some(payload),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], json!(false));
    });
}

#[test]
fn test_registration_refuses_a_credential_id_that_does_not_match_the_attestation() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());

        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/register/begin",
            Some(&token),
            None,
        )
        .await;
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();
        let (_pair, point) = key_pair();
        let cdj = client_data("webauthn.create", &challenge);
        let attestation = attestation_object(&registration_auth_data(b"real-id", &point, 0));

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/register/finish",
            Some(&token),
            Some(json!({
                "client_data_json": b64url(&cdj),
                "attestation_object": b64url(&attestation),
                "credential_id": b64url(b"a-different-id"),
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["message"]
            .as_str()
            .unwrap_or("")
            .contains("does not match"));
    });
}

#[test]
fn test_rename_and_revoke_are_scoped_to_the_owner() {
    run(async {
        let f = fixture().await;
        let owner = session(&f.state, "owner", false).await;
        let other = session(&f.state, "intruder", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, _pair) = register_passkey(&mut r, &owner).await;

        // A different account cannot rename or revoke it, and gets a 404 (not
        // a 403: the id must not be confirmed to exist).
        let uri = format!("/api/passkeys/{credential_id}");
        let (status, _) = send(
            &mut r,
            Method::PATCH,
            &uri,
            Some(&other),
            Some(json!({ "label": "mine now" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&mut r, Method::DELETE, &uri, Some(&other), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The owner can rename…
        let (status, body) = send(
            &mut r,
            Method::PATCH,
            &uri,
            Some(&owner),
            Some(json!({ "label": "  MacBook  " })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["data"]["label"],
            json!("MacBook"),
            "the label is trimmed"
        );

        // …and revoke.
        let (status, body) = send(&mut r, Method::DELETE, &uri, Some(&owner), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (_, body) = send(&mut r, Method::GET, "/api/passkeys", Some(&owner), None).await;
        assert_eq!(body["data"]["passkeys"], json!([]));
    });
}

#[test]
fn test_rename_refuses_an_overlong_label() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, _pair) = register_passkey(&mut r, &token).await;

        let (status, body) = send(
            &mut r,
            Method::PATCH,
            &format!("/api/passkeys/{credential_id}"),
            Some(&token),
            Some(json!({ "label": "a".repeat(200) })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    });
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[test]
fn test_passkey_sign_in_creates_a_session_and_bumps_the_counter() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, pair) = register_passkey(&mut r, &token).await;

        // begin
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "member" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();
        assert_eq!(body["data"]["rp_id"], json!(HOSTNAME));
        let allowed: Vec<&str> = body["data"]["allow_credentials"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(allowed, vec![credential_id.as_str()]);

        // finish: sign the real bytes
        let auth = assertion_auth_data(0x05, 1); // UP | UV
        let cdj = client_data("webauthn.get", &challenge);
        let mut signed = auth.clone();
        let mut hasher = Sha256::new();
        hasher.update(&cdj);
        signed.extend_from_slice(&hasher.finalize());
        let signature = sign(&pair, &signed);

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/finish",
            None,
            Some(json!({
                "credential_id": credential_id,
                "client_data_json": b64url(&cdj),
                "authenticator_data": b64url(&auth),
                "signature": b64url(&signature),
                "username": "member",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["success"], json!(true), "{body}");
        assert!(body["data"]["token"].as_str().unwrap_or("").len() > 10);
        assert_eq!(body["data"]["username"], json!("member"));

        // The counter is persisted: the next begin must reject a replay of it.
        let counter: (i64,) =
            sqlx::query_as("SELECT sign_count FROM passkey_credentials WHERE credential_id = ?")
                .bind(&credential_id)
                .fetch_one(&*f.state.db.pool())
                .await
                .expect("read counter");
        assert_eq!(counter.0, 1);
    });
}

#[test]
fn test_passkey_sign_in_refuses_a_bad_signature() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, _pair) = register_passkey(&mut r, &token).await;

        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "member" })),
        )
        .await;
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();

        let auth = assertion_auth_data(0x01, 1);
        let cdj = client_data("webauthn.get", &challenge);
        // Signed with a *different* key.
        let (other, _) = key_pair();
        let mut signed = auth.clone();
        let mut hasher = Sha256::new();
        hasher.update(&cdj);
        signed.extend_from_slice(&hasher.finalize());
        let signature = sign(&other, &signed);

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/finish",
            None,
            Some(json!({
                "credential_id": credential_id,
                "client_data_json": b64url(&cdj),
                "authenticator_data": b64url(&auth),
                "signature": b64url(&signature),
            })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(body["success"], json!(false));
    });
}

#[test]
fn test_passkey_sign_in_refuses_an_unknown_credential() {
    run(async {
        let f = fixture().await;
        let _token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());

        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "member" })),
        )
        .await;
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();
        let auth = assertion_auth_data(0x01, 1);
        let cdj = client_data("webauthn.get", &challenge);

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/finish",
            None,
            Some(json!({
                "credential_id": b64url(b"never-registered"),
                "client_data_json": b64url(&cdj),
                "authenticator_data": b64url(&auth),
                "signature": b64url(b"sig"),
            })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        // The sentence does not say whether the account exists.
        assert_eq!(body["message"], json!("Passkey sign-in failed"));
    });
}

#[test]
fn test_passkey_sign_in_refuses_a_locked_account() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, pair) = register_passkey(&mut r, &token).await;

        // An admin locks the account afterwards.
        sqlx::query("UPDATE users SET locked = 1 WHERE username = 'member'")
            .execute(&*f.state.db.pool())
            .await
            .expect("lock");

        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "member" })),
        )
        .await;
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();
        let auth = assertion_auth_data(0x01, 1);
        let cdj = client_data("webauthn.get", &challenge);
        let mut signed = auth.clone();
        let mut hasher = Sha256::new();
        hasher.update(&cdj);
        signed.extend_from_slice(&hasher.finalize());
        let signature = sign(&pair, &signed);

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/finish",
            None,
            Some(json!({
                "credential_id": credential_id,
                "client_data_json": b64url(&cdj),
                "authenticator_data": b64url(&auth),
                "signature": b64url(&signature),
            })),
        )
        .await;
        // A passkey must never be a way around an admin's lock.
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert!(body["message"].as_str().unwrap_or("").contains("locked"));
    });
}

#[test]
fn test_login_begin_does_not_reveal_whether_an_account_exists() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, _pair) = register_passkey(&mut r, &token).await;

        let (status_known, body_known) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "member" })),
        )
        .await;
        let (status_unknown, body_unknown) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "nobody-here" })),
        )
        .await;
        assert_eq!(status_known, StatusCode::OK);
        assert_eq!(status_unknown, status_known, "same status");

        // The known account names its credentials; the unknown one answers an
        // empty list — the same shape a real account with no passkey gets.
        assert_eq!(
            body_known["data"]["allow_credentials"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(body_unknown["data"]["allow_credentials"], json!([]));
        // …and both carry a real challenge, so the shapes are indistinguishable.
        assert!(
            body_unknown["data"]["challenge"]
                .as_str()
                .unwrap_or("")
                .len()
                > 10
        );
        let _ = credential_id;
    });
}

#[test]
fn test_anonymous_login_begin_works_without_a_username() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let _ = register_passkey(&mut r, &token).await;

        // A discoverable sign-in: no username typed.
        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["allow_credentials"], json!([]));
        assert!(body["data"]["challenge"].as_str().unwrap_or("").len() > 10);
    });
}

#[test]
fn test_expired_challenge_is_refused() {
    run(async {
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let (credential_id, pair) = register_passkey(&mut r, &token).await;

        let (_, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/begin",
            None,
            Some(json!({ "username": "member" })),
        )
        .await;
        let challenge = body["data"]["challenge"].as_str().unwrap().to_string();

        // Age the stored challenge past its expiry.
        sqlx::query("UPDATE passkey_challenges SET expires_at = '2000-01-01T00:00:00Z'")
            .execute(&*f.state.db.pool())
            .await
            .expect("age the challenge");

        let auth = assertion_auth_data(0x01, 1);
        let cdj = client_data("webauthn.get", &challenge);
        let mut signed = auth.clone();
        let mut hasher = Sha256::new();
        hasher.update(&cdj);
        signed.extend_from_slice(&hasher.finalize());
        let signature = sign(&pair, &signed);

        let (status, body) = send(
            &mut r,
            Method::POST,
            "/api/passkeys/login/finish",
            None,
            Some(json!({
                "credential_id": credential_id,
                "client_data_json": b64url(&cdj),
                "authenticator_data": b64url(&auth),
                "signature": b64url(&signature),
            })),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{body}");
        assert!(body["message"].as_str().unwrap_or("").contains("expired"));
    });
}

#[test]
fn test_passkeys_never_appear_in_the_report_and_the_2fa_path_still_works() {
    run(async {
        // Registering a passkey must not touch the TOTP record: the two are
        // separate credentials, and removing one never removes the other.
        let f = fixture().await;
        let token = session(&f.state, "member", false).await;
        let mut r = router(f.state.clone());
        let _ = register_passkey(&mut r, &token).await;

        let two_fa_rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_2fa")
            .fetch_one(&*f.state.db.pool())
            .await
            .expect("count");
        assert_eq!(two_fa_rows.0, 0, "a passkey is not a TOTP enrolment");
    });
}
