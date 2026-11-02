/* =============================================================================
 * File          : saml_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SAML 2.0 end-to-end at the HTTP boundary (ROADMAP P3.1, stage 3): the real
 *   router, the real handlers, the real JSON store on disk.
 *
 *   The identity provider is in-process: it holds an EC P-256 key, a
 *   self-signed certificate, and it signs assertions the way a real IdP does
 *   (exclusive C14N of `<SignedInfo>`, enveloped-signature digest over the
 *   assertion). Nothing is stubbed inside the appliance, so a passing test
 *   means the ACS really verified a signature.
 *
 *   The three questions this file answers:
 *   - is the admin half unreachable without an administrator, and the sign-in
 *     half reachable without a session?
 *   - does a correct response produce a session, a mirrored principal and the
 *     right role — and does a wrong one produce only a short error code?
 *   - does anything survive a replay, a second ticket exchange, a disabled
 *     principal or a demotion?
 *
 * Dependencies:
 *   - `ring` (keys and signatures), `rcgen` (the certificate), the appliance's
 *     own `saml` module for canonicalization.
 * =============================================================================
 */

#![allow(clippy::await_holding_lock)]

use std::sync::{Arc, LazyLock, Mutex};

use axum::{
    routing::{get, post, put},
    Router,
};
use base64::Engine as _;
use chrono::Utc;
use http::{Method, Request, StatusCode};
use ring::rand::SystemRandom;
use ring::signature::EcdsaKeyPair;
use serde_json::{json, Value};
use tower::{Service, ServiceExt};

use homevault::auth::AuthService;
use homevault::config::PathGuard;
use homevault::handlers::auth::handle_login;
use homevault::handlers::oidc::handle_auth_methods;
use homevault::handlers::saml::{
    handle_public_saml_metadata, handle_saml_acs, handle_saml_complete, handle_saml_get,
    handle_saml_metadata, handle_saml_settings_put, handle_saml_start, handle_saml_test,
};
use homevault::handlers::terminal_pty::PtyManager;
use homevault::handlers::AppState;
use homevault::middleware::rate_limit::RateLimitState;
use homevault::saml;
use homevault::storage::{Database, WidgetRepository};

type Req = Request<axum::body::Body>;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const ACS: &str = "https://nas.example.test/api/auth/saml/acs";
const SP_ENTITY: &str = "https://nas.example.test/api/auth/saml/metadata";
const IDP_ENTITY: &str = "https://idp.example.test/metadata";

// ---------------------------------------------------------------------------
// A fake identity provider
// ---------------------------------------------------------------------------

struct Idp {
    key: EcdsaKeyPair,
    cert_pem: String,
}

impl Idp {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .expect("generate a key");
        let key = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &rng,
        )
        .expect("read the key");
        let pem = pkcs8_pem(pkcs8.as_ref());
        let rcgen_key =
            rcgen::KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &rcgen::PKCS_ECDSA_P256_SHA256)
                .expect("rcgen key");
        let params =
            rcgen::CertificateParams::new(vec!["idp.example.test".to_string()]).expect("params");
        let cert = params.self_signed(&rcgen_key).expect("self-signed");

        Self {
            key,
            cert_pem: cert.pem(),
        }
    }

    fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.key
            .sign(&SystemRandom::new(), data)
            .expect("sign")
            .as_ref()
            .to_vec()
    }

    /// A complete, signed `SAMLResponse` answering `request_id`.
    fn response(&self, request_id: &str, name_id: &str, groups: &[&str]) -> String {
        self.response_with(
            request_id,
            name_id,
            groups,
            Utc::now() + chrono::Duration::minutes(5),
        )
    }

    fn response_with(
        &self,
        request_id: &str,
        name_id: &str,
        groups: &[&str],
        not_on_or_after: chrono::DateTime<Utc>,
    ) -> String {
        let instant = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let noa = not_on_or_after.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let group_values: String = groups
            .iter()
            .map(|group| format!("<saml:AttributeValue>{group}</saml:AttributeValue>"))
            .collect();

        let assertion = format!(
            r#"<saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="_assert1" Version="2.0" IssueInstant="{instant}"><saml:Issuer>{IDP_ENTITY}</saml:Issuer><saml:Subject><saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified">{name_id}</saml:NameID><saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer"><saml:SubjectConfirmationData NotOnOrAfter="{noa}" Recipient="{ACS}" InResponseTo="{request_id}"/></saml:SubjectConfirmation></saml:Subject><saml:Conditions NotBefore="{instant}" NotOnOrAfter="{noa}"><saml:AudienceRestriction><saml:Audience>{SP_ENTITY}</saml:Audience></saml:AudienceRestriction></saml:Conditions><saml:AuthnStatement AuthnInstant="{instant}" SessionIndex="_session1"/><saml:AttributeStatement><saml:Attribute Name="urn:oid:2.16.840.1.113730.3.1.241"><saml:AttributeValue>Alice Example</saml:AttributeValue></saml:Attribute><saml:Attribute Name="urn:oid:0.9.2342.19200300.100.1.3"><saml:AttributeValue>alice@example.test</saml:AttributeValue></saml:Attribute><saml:Attribute Name="http://schemas.xmlsoap.org/claims/Group">{group_values}</saml:Attribute></saml:AttributeStatement></saml:Assertion>"#
        );

        let node = saml::parse_tree(assertion.as_bytes()).expect("parse the assertion");
        let digest = base64::engine::general_purpose::STANDARD.encode(
            ring::digest::digest(&ring::digest::SHA256, saml::c14n(&node).as_bytes()).as_ref(),
        );

        let document = format!(
            r##"<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol" xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="_resp1" Version="2.0" IssueInstant="{instant}" Destination="{ACS}" InResponseTo="{request_id}"><saml:Issuer>{IDP_ENTITY}</saml:Issuer><samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status><saml:Assertion xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion" ID="_assert1" Version="2.0" IssueInstant="{instant}"><saml:Issuer>{IDP_ENTITY}</saml:Issuer><saml:Subject><saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified">{name_id}</saml:NameID><saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer"><saml:SubjectConfirmationData NotOnOrAfter="{noa}" Recipient="{ACS}" InResponseTo="{request_id}"/></saml:SubjectConfirmation></saml:Subject><saml:Conditions NotBefore="{instant}" NotOnOrAfter="{noa}"><saml:AudienceRestriction><saml:Audience>{SP_ENTITY}</saml:Audience></saml:AudienceRestriction></saml:Conditions><saml:AuthnStatement AuthnInstant="{instant}" SessionIndex="_session1"/><saml:AttributeStatement><saml:Attribute Name="urn:oid:2.16.840.1.113730.3.1.241"><saml:AttributeValue>Alice Example</saml:AttributeValue></saml:Attribute><saml:Attribute Name="urn:oid:0.9.2342.19200300.100.1.3"><saml:AttributeValue>alice@example.test</saml:AttributeValue></saml:Attribute><saml:Attribute Name="http://schemas.xmlsoap.org/claims/Group">{group_values}</saml:Attribute></saml:AttributeStatement><ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:SignedInfo><ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/><ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256"/><ds:Reference URI="#_assert1"><ds:Transforms><ds:Transform Algorithm="http://www.w3.org/2000/09/xmldsig#enveloped-signature"/><ds:Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/></ds:Transforms><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>{digest}</ds:DigestValue></ds:Reference></ds:SignedInfo><ds:SignatureValue>PLACEHOLDER</ds:SignatureValue></ds:Signature></saml:Assertion></samlp:Response>"##
        );

        let parsed = saml::parse_tree(document.as_bytes()).expect("parse the response");
        let signed_info = parsed
            .descendants()
            .into_iter()
            .find(|node| node.local == "SignedInfo")
            .expect("SignedInfo");
        let signature = base64::engine::general_purpose::STANDARD
            .encode(self.sign(saml::c14n(signed_info).as_bytes()));
        document.replace("PLACEHOLDER", &signature)
    }
}

fn pkcs8_pem(der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut body = String::new();
    for chunk in b64.as_bytes().chunks(64) {
        body.push_str(&String::from_utf8_lossy(chunk));
        body.push('\n');
    }
    format!("-----BEGIN PRIVATE KEY-----\n{body}-----END PRIVATE KEY-----\n")
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    state: Arc<AppState>,
    root: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

async fn setup() -> Harness {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
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
        .route("/api/auth/methods", get(handle_auth_methods))
        .route("/api/saml", get(handle_saml_get))
        .route("/api/saml/settings", put(handle_saml_settings_put))
        .route("/api/saml/test", post(handle_saml_test))
        .route("/api/saml/metadata", get(handle_saml_metadata))
        .route("/api/auth/saml/start", post(handle_saml_start))
        .route("/api/auth/saml/acs", post(handle_saml_acs))
        .route("/api/auth/saml/complete", post(handle_saml_complete))
        .route("/api/auth/saml/metadata", get(handle_public_saml_metadata))
        .with_state(state)
}

struct Sent {
    status: StatusCode,
    body: Value,
    headers: http::HeaderMap,
}

async fn send(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    content_type: &str,
    body: Option<String>,
) -> Sent {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        req = req.header("x-session-token", token);
    }
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    let req = match body {
        Some(body) => req
            .header("content-type", content_type)
            .body(axum::body::Body::from(body))
            .unwrap(),
        None => req.body(axum::body::Body::empty()).unwrap(),
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
    Sent {
        status,
        body,
        headers,
    }
}

async fn json(
    router: &mut Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Sent {
    send(
        router,
        method,
        uri,
        token,
        None,
        "application/json",
        body.map(|b| b.to_string()),
    )
    .await
}

/// A form post, the way an identity provider submits to the ACS.
async fn form(
    router: &mut Router,
    uri: &str,
    fields: &[(&str, &str)],
    cookie: Option<&str>,
) -> Sent {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in fields {
        serializer.append_pair(key, value);
    }
    let body = serializer.finish();
    send(
        router,
        Method::POST,
        uri,
        None,
        cookie,
        "application/x-www-form-urlencoded",
        Some(body),
    )
    .await
}

async fn admin(h: &Harness) -> (Router, String) {
    let hash = h.state.auth.hash_password("pass").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES ('root', ?, 'Root', 'root@example.com', 2, false, datetime('now'), 'seed')",
    )
    .bind(&hash)
    .execute(&*h.state.db.pool())
    .await
    .expect("insert admin");
    let mut router = router(h.state.clone());
    let sent = json(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": "root", "password": "pass"})),
    )
    .await;
    let token = sent.body["data"]["token"]
        .as_str()
        .expect("a login token")
        .to_string();
    (router, token)
}

fn settings_payload(idp: &Idp, enabled: bool) -> Value {
    json!({
        "enabled": enabled,
        "label": "Company sign-in",
        "idp_metadata_url": "https://idp.example.test/metadata",
        "idp_entity_id": IDP_ENTITY,
        "idp_sso_url": "https://idp.example.test/sso",
        "idp_certificates": [idp.cert_pem],
        "sp_entity_id": SP_ENTITY,
        "acs_url": ACS,
        "attr_display_name": "urn:oid:2.16.840.1.113730.3.1.241",
        "attr_email": "urn:oid:0.9.2342.19200300.100.1.3",
        "attr_groups": "http://schemas.xmlsoap.org/claims/Group",
        "default_role": "user",
        "group_roles": [{"group": "admins", "role": "admin"}],
    })
}

async fn configure(router: &mut Router, admin: &str, idp: &Idp) {
    let sent = json(
        router,
        Method::PUT,
        "/api/saml/settings",
        Some(admin),
        Some(settings_payload(idp, true)),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
}

/// Starts a sign-in and answers (relay state, cookie header).
async fn start(router: &mut Router) -> (String, String) {
    let sent = json(router, Method::POST, "/api/auth/saml/start", None, None).await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
    let state = sent.body["data"]["state"]
        .as_str()
        .expect("a relay state")
        .to_string();
    let url = sent.body["data"]["authorize_url"]
        .as_str()
        .expect("an authorize URL")
        .to_string();
    assert!(
        url.starts_with("https://idp.example.test/sso?"),
        "unexpected URL: {url}"
    );
    let cookie = format!("hv_saml_state={state}");
    (state, cookie)
}

fn error_code(sent: &Sent) -> String {
    sent.headers
        .get("location")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split("sso_error=").nth(1))
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// The admin half
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_admin_half_is_unreachable_without_an_administrator() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    for (method, uri) in [
        (Method::GET, "/api/saml"),
        (Method::PUT, "/api/saml/settings"),
        (Method::POST, "/api/saml/test"),
        (Method::GET, "/api/saml/metadata"),
    ] {
        let sent = json(&mut router, method.clone(), uri, None, None).await;
        assert!(
            sent.status == StatusCode::UNAUTHORIZED || sent.status == StatusCode::FORBIDDEN,
            "{uri} answered {} without a session",
            sent.status
        );
    }
}

#[tokio::test]
async fn the_sign_in_half_is_reachable_without_a_session() {
    let h = setup().await;
    let mut router = router(h.state.clone());
    // Not 401/403: the answer is "there is nothing to sign in to yet".
    let sent = json(
        &mut router,
        Method::POST,
        "/api/auth/saml/start",
        None,
        None,
    )
    .await;
    assert_eq!(
        sent.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        sent.body
    );

    let sent = json(&mut router, Method::GET, "/api/auth/methods", None, None).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.body["data"]["saml"]["enabled"], false);
}

#[tokio::test]
async fn saves_and_reads_the_settings_without_ever_returning_a_secret() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let sent = json(&mut router, Method::GET, "/api/saml", Some(&admin), None).await;
    assert_eq!(sent.status, StatusCode::OK);
    let data = &sent.body["data"];
    assert_eq!(data["settings"]["enabled"], true);
    assert_eq!(data["settings"]["idp_entity_id"], IDP_ENTITY);
    assert_eq!(data["status"]["usable"], true);
    assert!(data["status"]["problem"].is_null());
    // The certificate is not a secret, the local admin count is the point.
    assert!(data["break_glass"]["local_admins"].as_i64().unwrap() >= 1);

    // On disk, and the label reached the login screen.
    let stored = h.root.join("data/saml.json");
    assert!(stored.exists(), "the settings must be persisted");
    let sent = json(&mut router, Method::GET, "/api/auth/methods", None, None).await;
    assert_eq!(sent.body["data"]["saml"]["enabled"], true);
    assert_eq!(sent.body["data"]["saml"]["label"], "Company sign-in");
}

#[tokio::test]
async fn refuses_an_incomplete_configuration_but_lets_you_switch_it_off() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();

    let mut payload = settings_payload(&idp, true);
    payload["idp_sso_url"] = json!("");
    let sent = json(
        &mut router,
        Method::PUT,
        "/api/saml/settings",
        Some(&admin),
        Some(payload),
    )
    .await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{}", sent.body);

    // Disabling is always accepted, even with a broken document, otherwise a
    // bad edit could not be undone.
    let mut payload = settings_payload(&idp, false);
    payload["idp_sso_url"] = json!("");
    payload["idp_certificates"] = json!([]);
    let sent = json(
        &mut router,
        Method::PUT,
        "/api/saml/settings",
        Some(&admin),
        Some(payload),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
}

#[tokio::test]
async fn refuses_a_metadata_url_that_is_not_https() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let sent = json(
        &mut router,
        Method::POST,
        "/api/saml/test",
        Some(&admin),
        Some(json!({"settings": {"enabled": true, "idp_metadata_url": "http://idp.example.test/metadata"}})),
    )
    .await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST, "{}", sent.body);
    assert!(
        sent.body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("https"),
        "{}",
        sent.body
    );
}

#[tokio::test]
async fn serves_this_appliances_metadata_document() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let sent = json(
        &mut router,
        Method::GET,
        "/api/saml/metadata",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    let xml = sent.body["data"]["xml"].as_str().unwrap();
    assert!(xml.contains(ACS));
    assert!(xml.contains(SP_ENTITY));

    // The public copy, for an IdP that fetches by URL.
    let sent = json(
        &mut router,
        Method::GET,
        "/api/auth/saml/metadata",
        None,
        None,
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(sent.body["raw"].as_str().unwrap().contains(SP_ENTITY));
}

// ---------------------------------------------------------------------------
// The sign-in flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_signed_response_produces_a_session_a_principal_and_the_right_role() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, cookie) = start(&mut router).await;
    let response = idp.response(&relay, "alice@example.test", &["admins", "staff"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(sent.status, StatusCode::SEE_OTHER, "{}", sent.body);
    let location = sent.headers.get("location").unwrap().to_str().unwrap();
    assert!(location.starts_with("/login#sso="), "{location}");
    let ticket = location
        .split("sso=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();

    let sent = json(
        &mut router,
        Method::POST,
        "/api/auth/saml/complete",
        None,
        Some(json!({ "code": ticket })),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
    assert_eq!(sent.body["data"]["username"], "alice");
    assert_eq!(sent.body["data"]["is_admin"], true);

    // The principal is mirrored, with the provider that created it.
    let identities: Vec<(String, String)> =
        sqlx::query_as("SELECT username, provider FROM remote_identities ORDER BY username")
            .fetch_all(&*h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0].0, "alice");
    assert_eq!(identities[0].1, "saml");

    // The ticket is spent: a second exchange must fail.
    let sent = json(
        &mut router,
        Method::POST,
        "/api/auth/saml/complete",
        None,
        Some(json!({ "code": ticket })),
    )
    .await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refuses_a_response_without_the_browser_cookie() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, _cookie) = start(&mut router).await;
    let response = idp.response(&relay, "alice@example.test", &["staff"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        None,
    )
    .await;
    assert_eq!(sent.status, StatusCode::SEE_OTHER);
    assert_eq!(error_code(&sent), "state");
}

#[tokio::test]
async fn refuses_a_replayed_relay_state() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, cookie) = start(&mut router).await;
    let response = idp.response(&relay, "alice@example.test", &["staff"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let first = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(first.status, StatusCode::SEE_OTHER);

    let second = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(error_code(&second), "expired");
}

#[tokio::test]
async fn refuses_a_tampered_response_with_a_short_code_only() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, cookie) = start(&mut router).await;
    let response = idp
        .response(&relay, "alice@example.test", &["staff"])
        .replace("Alice Example", "Mallory Example");
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(sent.status, StatusCode::SEE_OTHER);
    assert_eq!(error_code(&sent), "bad_digest");
    // The redirect carries the code and nothing else.
    let location = sent.headers.get("location").unwrap().to_str().unwrap();
    assert!(!location.contains("Mallory"), "{location}");
}

#[tokio::test]
async fn refuses_an_expired_assertion() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, cookie) = start(&mut router).await;
    let response = idp.response_with(
        &relay,
        "alice@example.test",
        &["staff"],
        Utc::now() - chrono::Duration::minutes(30),
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(error_code(&sent), "expired");
}

#[tokio::test]
async fn a_mapping_change_demotes_immediately() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, cookie) = start(&mut router).await;
    let response = idp.response(&relay, "alice@example.test", &["admins"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;

    let role_before: i64 =
        sqlx::query_scalar("SELECT role_id FROM remote_identities WHERE username = 'alice'")
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(role_before, 2, "an admin mapping must grant admin");

    // Drop the mapping: the next save must rescore the stored principal.
    let mut payload = settings_payload(&idp, true);
    payload["group_roles"] = json!([]);
    let sent = json(
        &mut router,
        Method::PUT,
        "/api/saml/settings",
        Some(&admin),
        Some(payload),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);

    let role_after: i64 =
        sqlx::query_scalar("SELECT role_id FROM remote_identities WHERE username = 'alice'")
            .fetch_one(&*h.state.db.pool())
            .await
            .unwrap();
    assert_eq!(
        role_after, 1,
        "the default role must apply once the mapping is gone"
    );
}

#[tokio::test]
async fn refuses_a_disabled_principal() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    // Sign in once so the principal exists, then disable it the way the screen
    // does (the directory route owns the shared principal list).
    let (relay, cookie) = start(&mut router).await;
    let response = idp.response(&relay, "alice@example.test", &["staff"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    sqlx::query("UPDATE remote_identities SET disabled = 1 WHERE username = 'alice'")
        .execute(&*h.state.db.pool())
        .await
        .unwrap();

    let (relay, cookie) = start(&mut router).await;
    let response = idp.response(&relay, "alice@example.test", &["staff"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(error_code(&sent), "account_disabled");
}

#[tokio::test]
async fn refuses_a_reserved_login_name() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let (relay, cookie) = start(&mut router).await;
    let response = idp.response(&relay, "admin", &["admins"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(error_code(&sent), "unusable_name");
    // Nothing was mirrored for the refused name.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_identities")
        .fetch_one(&*h.state.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn a_disabled_integration_refuses_the_acs() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;
    let (relay, cookie) = start(&mut router).await;

    let mut payload = settings_payload(&idp, false);
    payload["idp_certificates"] = json!([]);
    let sent = json(
        &mut router,
        Method::PUT,
        "/api/saml/settings",
        Some(&admin),
        Some(payload),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);

    let response = idp.response(&relay, "alice@example.test", &["staff"]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(response.as_bytes());
    let sent = form(
        &mut router,
        "/api/auth/saml/acs",
        &[("SAMLResponse", &encoded), ("RelayState", &relay)],
        Some(&cookie),
    )
    .await;
    assert_eq!(error_code(&sent), "disabled");
}

#[tokio::test]
async fn the_break_glass_count_never_hides_the_local_administrator() {
    let h = setup().await;
    let (mut router, admin) = admin(&h).await;
    let idp = Idp::new();
    configure(&mut router, &admin, &idp).await;

    let sent = json(&mut router, Method::GET, "/api/saml", Some(&admin), None).await;
    let local = sent.body["data"]["break_glass"]["local_admins"]
        .as_i64()
        .unwrap();
    assert!(local >= 1, "the local administrator must always be counted");
    // And the local password still works while SAML is on.
    let sent = json(
        &mut router,
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({"username": "root", "password": "pass"})),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
}
