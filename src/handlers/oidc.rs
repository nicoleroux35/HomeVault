/* =============================================================================
 * File          : oidc.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface for OpenID Connect sign-in (ROADMAP P3.1, stage 2).
 *
 *   Two halves, deliberately separated:
 *   - `/api/oidc*` is admin-only. The settings carry a client secret and decide
 *     who becomes an administrator; the secret is sealed and never returned.
 *   - `/api/auth/oidc/…` is public because it *is* the sign-in: `/start` hands
 *     the browser a provider URL, `/callback` redeems the code server-side and
 *     `/complete` trades a one-shot ticket for the session the SPA stores.
 *
 *   The browser is never trusted to say who it is. The code is exchanged
 *   server-side with PKCE, the ID token's signature and `iss`/`aud`/`exp`/`nonce`
 *   are checked, and only then is a mirrored account created.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::oidc, crate::storage::directory_repo
 *
 * Usage:
 *   Routes in src/web/mod.rs; see docs/guides/directory-sso.md.
 * =============================================================================
 */

//! OpenID Connect sign-in endpoints (see docs/guides/directory-sso.md).
//!
//! | Route                                  | Guard  |
//! |----------------------------------------|--------|
//! | `GET    /api/oidc`                     | admin  |
//! | `PUT    /api/oidc/settings`            | admin  |
//! | `POST   /api/oidc/test`                | admin  |
//! | `GET    /api/auth/methods`             | public |
//! | `POST   /api/auth/oidc/start`          | public |
//! | `GET    /api/auth/oidc/callback`       | public |
//! | `POST   /api/auth/oidc/complete`       | public |

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{
        header::{HeaderValue, SET_COOKIE},
        HeaderMap, StatusCode,
    },
    response::{IntoResponse, Redirect, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::directory::GroupRole;
use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::oidc::{self, OidcSettings};
use crate::storage::directory_repo::{DirectoryRepository, IdentityUpdate};
use crate::storage::models::{LoginResponse, ADMIN_ROLE_ID, USER_ROLE_ID};
use crate::utils::ResponseHelper;

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// The settings as the screen needs them: everything but the sealed secret.
#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub enabled: bool,
    pub label: String,
    pub issuer: String,
    pub client_id: String,
    /// Whether a client secret is stored — never the secret itself.
    pub has_client_secret: bool,
    pub redirect_url: String,
    pub scopes: String,
    pub attr_username: String,
    pub attr_display_name: String,
    pub attr_email: String,
    pub attr_groups: String,
    pub default_role: String,
    pub group_roles: Vec<GroupRole>,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
}

impl SettingsView {
    fn from_settings(settings: &OidcSettings) -> Self {
        Self {
            enabled: settings.enabled,
            label: settings.label.clone(),
            issuer: settings.issuer.clone(),
            client_id: settings.client_id.clone(),
            has_client_secret: settings.client_secret_sealed.is_some(),
            redirect_url: settings.redirect_url.clone(),
            scopes: settings.scopes.clone(),
            attr_username: settings.attr_username.clone(),
            attr_display_name: settings.attr_display_name.clone(),
            attr_email: settings.attr_email.clone(),
            attr_groups: settings.attr_groups.clone(),
            default_role: settings.default_role.clone(),
            group_roles: settings.group_roles.clone(),
            updated_at: settings.updated_at.clone(),
            updated_by: settings.updated_by.clone(),
        }
    }
}

/// What the screen may change. `client_secret` is write-only: absent keeps the
/// stored secret, an empty string clears it (a public client).
#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    pub enabled: bool,
    #[serde(default = "default_label")]
    pub label: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_url: String,
    #[serde(default = "default_scopes")]
    pub scopes: String,
    #[serde(default = "default_username_claim")]
    pub attr_username: String,
    #[serde(default = "default_display_claim")]
    pub attr_display_name: String,
    #[serde(default = "default_email_claim")]
    pub attr_email: String,
    #[serde(default = "default_groups_claim")]
    pub attr_groups: String,
    #[serde(default = "default_role")]
    pub default_role: String,
    #[serde(default)]
    pub group_roles: Vec<GroupRole>,
}

fn default_label() -> String {
    oidc::DEFAULT_LABEL.to_string()
}

fn default_scopes() -> String {
    "openid profile email".to_string()
}

fn default_username_claim() -> String {
    "preferred_username".to_string()
}

fn default_display_claim() -> String {
    "name".to_string()
}

fn default_email_claim() -> String {
    "email".to_string()
}

fn default_groups_claim() -> String {
    "groups".to_string()
}

fn default_role() -> String {
    crate::directory::ROLE_USER.to_string()
}

#[derive(Debug, Deserialize)]
pub struct TestRequest {
    /// Unsaved settings to try. Absent means "test what is stored".
    pub settings: Option<SettingsRequest>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /api/oidc
// ---------------------------------------------------------------------------

/// The admin payload: settings, health and how many local administrators can
/// still sign in if the provider is unreachable.
pub async fn handle_oidc_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let settings = oidc::load();
    let problem = oidc::validate(&settings).err();
    let repo = DirectoryRepository::new(state.db.pool());
    let identities = repo.list().await.unwrap_or_default();
    let oidc_identities: Vec<_> = identities
        .iter()
        .filter(|identity| identity.provider == oidc::PROVIDER_OIDC)
        .collect();
    let disabled = oidc_identities.iter().filter(|i| i.disabled).count() as i64;

    ResponseHelper::json_success(json!({
        "settings": SettingsView::from_settings(&settings),
        "status": {
            "enabled": settings.enabled,
            "usable": settings.usable(),
            "problem": problem,
        },
        "counts": {
            "total": oidc_identities.len() as i64,
            "admins": oidc_identities.iter().filter(|i| i.is_admin()).count() as i64,
            "users": oidc_identities.iter().filter(|i| !i.is_admin()).count() as i64,
            "disabled": disabled,
        },
        "break_glass": {
            "local_admins": state.auth.count_local_admins().await,
            "policy": "A local account always keeps its own password, so single sign-on can never lock an administrator out."
        }
    }))
}

// ---------------------------------------------------------------------------
// PUT /api/oidc/settings
// ---------------------------------------------------------------------------

/// Build the settings document from a request, sealing a new secret when one
/// was typed and never unsealing the stored one.
fn build_settings(
    request: SettingsRequest,
    current: &OidcSettings,
    updated_by: Option<&str>,
) -> Result<OidcSettings, String> {
    let sealed = match request.client_secret.as_deref() {
        // Absent: keep whatever is stored.
        None => current.client_secret_sealed.clone(),
        // Explicitly cleared: a public client.
        Some("") => None,
        Some(secret) => Some(oidc::seal_client_secret(secret)?),
    };
    Ok(OidcSettings {
        enabled: request.enabled,
        label: request.label.trim().to_string(),
        issuer: request.issuer.trim().trim_end_matches('/').to_string(),
        client_id: request.client_id.trim().to_string(),
        client_secret_sealed: sealed,
        redirect_url: request.redirect_url.trim().to_string(),
        scopes: request.scopes.trim().to_string(),
        attr_username: request.attr_username.trim().to_string(),
        attr_display_name: request.attr_display_name.trim().to_string(),
        attr_email: request.attr_email.trim().to_string(),
        attr_groups: request.attr_groups.trim().to_string(),
        default_role: request.default_role.trim().to_ascii_lowercase(),
        group_roles: request
            .group_roles
            .into_iter()
            .map(|mapping| GroupRole {
                group: mapping.group.trim().to_string(),
                role: mapping.role.trim().to_ascii_lowercase(),
            })
            .filter(|mapping| !mapping.group.is_empty() || !mapping.role.is_empty())
            .collect(),
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        updated_by: updated_by.map(str::to_string),
    })
}

pub async fn handle_oidc_settings_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SettingsRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let current = oidc::load();
    let settings = match build_settings(request, &current, Some(&admin)) {
        Ok(settings) => settings,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if let Err(e) = oidc::validate(&settings) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = oidc::save(&settings) {
        return ResponseHelper::json_error(e);
    }

    // A mapping change must demote immediately, exactly like LDAP: re-score the
    // stored group lists of every OIDC principal under the new table.
    let repo = DirectoryRepository::new(state.db.pool());
    let identities = repo.list().await.unwrap_or_default();
    let mut rescored = 0usize;
    for identity in identities
        .iter()
        .filter(|identity| identity.provider == oidc::PROVIDER_OIDC)
    {
        let role = crate::directory::resolve_groups_role(
            &identity.groups,
            &settings.group_roles,
            &settings.default_role,
        );
        let role_id = if role == crate::directory::ROLE_ADMIN {
            ADMIN_ROLE_ID
        } else {
            USER_ROLE_ID
        };
        if identity.role_id != role_id {
            rescored += 1;
        }
        let _ = repo
            .set_role(identity.id, &identity.username, role_id)
            .await;
    }

    crate::events::log_event(format!(
        "Single sign-on settings updated by '{admin}' (enabled={}, issuer={}, {rescored} role(s) rescored)",
        settings.enabled, settings.issuer
    ));

    handle_oidc_get(State(state), headers).await
}

// ---------------------------------------------------------------------------
// POST /api/oidc/test
// ---------------------------------------------------------------------------

/// Fetch the provider's discovery document for the settings as typed, without
/// saving anything. Proves the issuer, the TLS trust and the endpoints in one
/// round trip, which is the failure an admin is most likely to hit.
pub async fn handle_oidc_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<TestRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let settings = match request.settings {
        Some(inline) => {
            let current = oidc::load();
            match build_settings(inline, &current, None) {
                Ok(settings) => settings,
                Err(e) => return ResponseHelper::json_error(e),
            }
        }
        None => oidc::load(),
    };
    if let Err(e) = oidc::validate(&settings) {
        return ResponseHelper::json_error(e);
    }
    match oidc::discovery_for(&settings.issuer).await {
        Ok(document) => ResponseHelper::json_success(json!({
            "ok": true,
            "message": "The identity provider answered.",
            "issuer": document.issuer,
            "authorization_endpoint": document.authorization_endpoint,
            "token_endpoint": document.token_endpoint,
            "jwks_uri": document.jwks_uri,
            "algorithms": document.id_token_signing_alg_values_supported,
        })),
        Err(e) => ResponseHelper::json_error(e.message()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/auth/methods
// ---------------------------------------------------------------------------

/// What the login screen may offer. Public, and deliberately says nothing about
/// providers that are switched off.
pub async fn handle_auth_methods() -> Response {
    let settings = oidc::load();
    let saml_settings = crate::saml::load();
    ResponseHelper::json_success(json!({
        "password": true,
        "oidc": {
            "enabled": settings.usable(),
            "label": if settings.label.trim().is_empty() {
                oidc::DEFAULT_LABEL.to_string()
            } else {
                settings.label.trim().to_string()
            },
        },
        "saml": {
            "enabled": saml_settings.usable(),
            "label": if saml_settings.label.trim().is_empty() {
                crate::saml::DEFAULT_LABEL.to_string()
            } else {
                saml_settings.label.trim().to_string()
            },
        }
    }))
}

// ---------------------------------------------------------------------------
// POST /api/auth/oidc/start
// ---------------------------------------------------------------------------

/// Start a sign-in and answer the URL the browser must visit.
///
/// `POST` rather than `GET` on purpose: a link that silently starts a sign-in is
/// a login-CSRF primitive. The state is also bound to this browser by a cookie,
/// so a state an attacker obtained elsewhere cannot be completed here.
pub async fn handle_oidc_start(
    over_tls: Option<Extension<crate::middleware::ArrivedOverTls>>,
) -> Response {
    let over_tls = over_tls.is_some();
    let settings = oidc::load();
    if !settings.usable() {
        return ResponseHelper::json_error_with_status(
            StatusCode::SERVICE_UNAVAILABLE,
            "Single sign-on is not configured".to_string(),
        );
    }
    let discovery = match oidc::discovery_for(&settings.issuer).await {
        Ok(document) => document,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC sign-in could not start: {}", e.message()));
            return ResponseHelper::json_error_with_status(
                StatusCode::SERVICE_UNAVAILABLE,
                e.message(),
            );
        }
    };
    let (Ok(state_value), Ok(nonce), Ok(verifier)) = (
        oidc::random_token(32),
        oidc::random_token(32),
        oidc::random_token(32),
    ) else {
        return ResponseHelper::json_internal_error(
            "Could not generate the sign-in parameters".to_string(),
        );
    };
    let challenge = oidc::pkce_challenge(&verifier);
    let authorized = match oidc::authorize_url(
        &discovery.authorization_endpoint,
        &settings,
        &state_value,
        &nonce,
        &challenge,
    ) {
        Ok(url) => url,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let pending = oidc::PendingLogin {
        verifier,
        nonce,
        return_to: "/desktop".to_string(),
        issued_at: std::time::Instant::now(),
    };
    if let Err(e) = oidc::PENDING.insert(&state_value, pending) {
        return ResponseHelper::json_error(e);
    }

    let mut response = ResponseHelper::json_success(json!({
        "authorize_url": authorized,
        "state": state_value,
    }));
    let secure = if over_tls { "; Secure" } else { "" };
    let cookie = format!(
        "{state_cookie}={state_value}; Path=/api/auth/oidc; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        state_cookie = oidc::STATE_COOKIE,
        max_age = oidc::PENDING_TTL.as_secs(),
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

// ---------------------------------------------------------------------------
// GET /api/auth/oidc/callback
// ---------------------------------------------------------------------------

/// The redirect URI registered at the provider. Every failure lands back on the
/// login screen with a short code — never a message, never a token.
pub async fn handle_oidc_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    over_tls: Option<Extension<crate::middleware::ArrivedOverTls>>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let over_tls = over_tls.is_some();
    if query.error.is_some() {
        return callback_failure("denied");
    }
    let (Some(code), Some(state_value)) = (query.code.as_deref(), query.state.as_deref()) else {
        return callback_failure("state");
    };
    if code.trim().is_empty() {
        return callback_failure("state");
    }

    // The cookie is what proves this callback belongs to the browser that
    // started the flow. Without it, a link an attacker prepared could sign the
    // victim into the attacker's account (login CSRF).
    let cookie = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            crate::app_proxy::cookie::named_cookie_from_header(value, oidc::STATE_COOKIE)
        });
    if cookie != Some(state_value) {
        return callback_failure("state");
    }

    let Some(pending) = oidc::PENDING.take(state_value) else {
        return callback_failure("expired");
    };
    let settings = oidc::load();
    if !settings.usable() {
        return callback_failure("disabled");
    }

    let ip_address = crate::handlers::auth::client_ip(&headers, peer.0);
    let discovery = match oidc::discovery_for(&settings.issuer).await {
        Ok(document) => document,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC callback: {e:?}"));
            return callback_failure("unavailable");
        }
    };
    let secret = match oidc::open_client_secret(&settings.client_secret_sealed) {
        Ok(secret) => secret,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC callback: {e}"));
            return callback_failure("unavailable");
        }
    };
    let id_token = match oidc::exchange_code(
        &settings,
        &discovery,
        code,
        &pending.verifier,
        secret.as_deref(),
    )
    .await
    {
        Ok(token) => token,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC callback from {ip_address}: {e:?}"));
            return callback_failure("refused");
        }
    };

    let expected = oidc::ExpectedToken {
        issuer: &settings.issuer,
        client_id: &settings.client_id,
        nonce: &pending.nonce,
        now: chrono::Utc::now().timestamp(),
    };
    let mut keys = match oidc::jwks_for(&discovery.jwks_uri, false).await {
        Ok(keys) => keys,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC callback: {e:?}"));
            return callback_failure("unavailable");
        }
    };
    // A key rotation between our cache and this token is worth exactly one
    // refetch — and only when the token names a key we do not have, so an
    // invalid token cannot turn every attempt into a key-set download.
    let unknown_key = oidc::token_key_id(&id_token).is_some_and(|kid| {
        !keys
            .iter()
            .any(|key| key.kid.as_deref() == Some(kid.as_str()))
    });
    if unknown_key {
        if let Ok(refreshed) = oidc::jwks_for(&discovery.jwks_uri, true).await {
            keys = refreshed;
        }
    }
    let claims = match oidc::verify_id_token(&id_token, &keys, &expected) {
        Ok(claims) => claims,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC callback from {ip_address}: {e:?}"));
            return callback_failure("refused");
        }
    };
    let principal = match oidc::principal_from_claims(&claims.claims, &settings) {
        Ok(principal) => principal,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC callback: {e:?}"));
            return callback_failure("claim");
        }
    };

    let role = crate::directory::resolve_groups_role(
        &principal.groups,
        &settings.group_roles,
        &settings.default_role,
    );
    let role_id = if role == crate::directory::ROLE_ADMIN {
        ADMIN_ROLE_ID
    } else {
        USER_ROLE_ID
    };
    let repo = DirectoryRepository::new(state.db.pool());
    let update = IdentityUpdate {
        provider: oidc::PROVIDER_OIDC.to_string(),
        subject: principal.subject.clone(),
        username: principal.username.clone(),
        display_name: principal.display_name.clone(),
        email: principal.email.clone(),
        groups: principal.groups.clone(),
        role_id,
    };
    let identity = match repo.upsert_login(&update).await {
        Ok(identity) => identity,
        Err(e) => {
            crate::events::log_event_warn(format!(
                "OIDC sign-in for '{}' from {ip_address} was refused: {e}",
                principal.username
            ));
            return callback_failure("account");
        }
    };
    if identity.disabled {
        return callback_failure("account_disabled");
    }

    let session = match state
        .auth
        .create_session(identity.username.clone(), ip_address.clone(), false)
        .await
    {
        Ok(session) => session,
        Err(e) => {
            crate::events::log_event_warn(format!("OIDC sign-in: session creation failed: {e}"));
            return callback_failure("session");
        }
    };

    crate::events::log_event(format!(
        "User '{}' signed in through oidc from {ip_address} as {role}",
        session.username
    ));
    let _ = state
        .auth
        .record_successful_attempt(
            &session.username,
            &ip_address,
            headers.get("user-agent").and_then(|h| h.to_str().ok()),
        )
        .await;

    let issued = oidc::IssuedSession {
        token: session.token.clone(),
        csrf_token: session.csrf_token.clone(),
        username: session.username.clone(),
        is_admin: session.is_admin,
    };
    let ticket = match oidc::random_token(32) {
        Ok(ticket) => ticket,
        Err(_) => return callback_failure("session"),
    };
    if oidc::ISSUED.insert(&ticket, issued).is_err() {
        return callback_failure("session");
    }

    // The ticket travels in the fragment: it is never sent to a server, never
    // appears in a Referer and never lands in the access log. It is also
    // single-use and expires in two minutes.
    let mut response = Redirect::to(&format!(
        "/login#sso={}&p=oidc&next={}",
        ticket,
        urlencode(&pending.return_to)
    ))
    .into_response();
    response = crate::app_proxy::cookie::attach_session(response, &session.token, false, over_tls);
    clear_state_cookie(response)
}

/// A refusal lands on the login screen with a code the SPA translates.
fn callback_failure(code: &str) -> Response {
    clear_state_cookie(Redirect::to(&format!("/login?sso_error={code}")).into_response())
}

fn clear_state_cookie(mut response: Response) -> Response {
    let cookie = format!(
        "{}=; Path=/api/auth/oidc; HttpOnly; SameSite=Lax; Max-Age=0",
        oidc::STATE_COOKIE
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

// ---------------------------------------------------------------------------
// POST /api/auth/oidc/complete
// ---------------------------------------------------------------------------

/// Trade a one-shot ticket for the session the SPA stores in `localStorage`.
///
/// The API authenticates with `X-Session-Token`, not the cookie (the cookie is
/// for the app proxy), so the browser needs the token itself. Handing it over
/// through a fragment ticket keeps it out of the URL that reaches the server.
pub async fn handle_oidc_complete(Json(request): Json<CompleteRequest>) -> Response {
    let Some(issued) = oidc::ISSUED.take(request.code.trim()) else {
        return ResponseHelper::json_error_with_status(
            StatusCode::UNAUTHORIZED,
            "This sign-in has expired — start again".to_string(),
        );
    };
    ResponseHelper::json_success(LoginResponse {
        token: issued.token,
        csrf_token: issued.csrf_token,
        username: issued.username,
        is_admin: issued.is_admin,
        must_change_password: false,
    })
}
