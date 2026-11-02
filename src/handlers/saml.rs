/* =============================================================================
 * File          : saml.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The HTTP half of the SAML 2.0 integration (ROADMAP P3.1, stage 3).
 *
 *   Two audiences, two shapes:
 *   - the **admin** half (`/api/saml`, `/api/saml/settings`, `/api/saml/test`,
 *     `/api/saml/metadata`) is `require_admin` and answers the same envelope as
 *     the OIDC screen, so the SPA renders both providers with one component;
 *   - the **sign-in** half (`/api/auth/saml/start`, `/api/auth/saml/acs`,
 *     `/api/auth/saml/complete`) is public, because a signed-out browser has to
 *     reach it. None of the three trusts a credential from the request body:
 *     the assertion is verified against the configured certificate, and the
 *     one-shot ticket is spent at most once.
 *
 *   The ACS is the one endpoint an identity provider posts to. It always
 *   answers a redirect: on success to `/login#sso=<ticket>`, on any failure to
 *   `/login?sso_error=<code>`. Never a JSON error, never a message — an
 *   anonymous visitor must not be able to use the ACS as a parser oracle.
 *
 * Dependencies:
 *   - `crate::saml` for everything protocol-shaped; this file only moves bytes.
 * =============================================================================
 */

use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Extension, Form, State},
    http::{header::SET_COOKIE, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::directory::{GroupRole, ROLE_ADMIN};
use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::middleware::ArrivedOverTls;
use crate::oidc::{random_token, IssuedSession, ISSUED};
use crate::saml;
use crate::storage::directory_repo::{DirectoryRepository, IdentityUpdate};
use crate::storage::models::{LoginResponse, ADMIN_ROLE_ID, USER_ROLE_ID};
use crate::utils::ResponseHelper;

/// The cookie that binds a sign-in to the browser that started it.
pub const STATE_COOKIE: &str = "hv_saml_state";

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GroupRoleRequest {
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub role: String,
}

/// The settings as the screen edits them. Field names match the JSON the SPA
/// sends, which is the same shape `GET /api/saml` returns.
#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    pub enabled: bool,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub idp_metadata_url: String,
    #[serde(default)]
    pub idp_entity_id: String,
    #[serde(default)]
    pub idp_sso_url: String,
    #[serde(default)]
    pub idp_certificates: Vec<String>,
    #[serde(default)]
    pub sp_entity_id: String,
    #[serde(default)]
    pub acs_url: String,
    #[serde(default)]
    pub attr_display_name: String,
    #[serde(default)]
    pub attr_email: String,
    #[serde(default)]
    pub attr_groups: String,
    #[serde(default)]
    pub default_role: String,
    #[serde(default)]
    pub group_roles: Vec<GroupRoleRequest>,
}

#[derive(Debug, Deserialize)]
pub struct TestRequest {
    /// The form as typed. Absent means "test what is saved".
    #[serde(default)]
    pub settings: Option<SettingsRequest>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    #[serde(default)]
    pub code: String,
}

/// The form the IdP posts. Axum's `Form` extractor already rejects anything
/// that is not `application/x-www-form-urlencoded`.
#[derive(Debug, Deserialize)]
pub struct AcsForm {
    #[serde(rename = "SAMLResponse", default)]
    pub saml_response: String,
    #[serde(rename = "RelayState", default)]
    pub relay_state: String,
}

// ---------------------------------------------------------------------------
// GET /api/saml
// ---------------------------------------------------------------------------

/// The admin payload: settings, health, the shared principal list and the
/// break-glass count.
pub async fn handle_saml_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let settings = saml::load();
    let problem = settings.validate().err();
    let repo = DirectoryRepository::new(state.db.pool());
    let identities = repo.list().await.unwrap_or_default();
    let saml_identities: Vec<_> = identities
        .iter()
        .filter(|identity| identity.provider == saml::PROVIDER_SAML)
        .collect();
    let disabled = saml_identities.iter().filter(|i| i.disabled).count() as i64;

    ResponseHelper::json_success(json!({
        "settings": settings,
        "status": {
            "enabled": settings.enabled,
            "usable": settings.usable(),
            "problem": problem,
            "pending_requests": saml::pending_requests(),
        },
        "counts": {
            "total": saml_identities.len() as i64,
            "admins": saml_identities.iter().filter(|i| i.is_admin()).count() as i64,
            "users": saml_identities.iter().filter(|i| !i.is_admin()).count() as i64,
            "disabled": disabled,
        },
        "break_glass": {
            "local_admins": state.auth.count_local_admins().await,
            "policy": "A local account always keeps its own password, so single sign-on can never lock an administrator out."
        }
    }))
}

// ---------------------------------------------------------------------------
// PUT /api/saml/settings
// ---------------------------------------------------------------------------

fn build_settings(request: SettingsRequest, updated_by: Option<&str>) -> saml::SamlSettings {
    saml::SamlSettings {
        enabled: request.enabled,
        label: request.label.trim().to_string(),
        idp_metadata_url: request.idp_metadata_url.trim().to_string(),
        idp_entity_id: request.idp_entity_id.trim().to_string(),
        idp_sso_url: request.idp_sso_url.trim().to_string(),
        idp_certificates: request
            .idp_certificates
            .iter()
            .flat_map(|entry| saml::extract_pem_blocks(entry))
            .collect(),
        sp_entity_id: request.sp_entity_id.trim().to_string(),
        acs_url: request.acs_url.trim().to_string(),
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
        updated_at: chrono::Utc::now().to_rfc3339(),
        updated_by: updated_by.map(str::to_string),
    }
}

/// Parses an admin request body only after `require_admin` has answered, so an
/// anonymous visitor cannot use the body parser as a format oracle.
fn parse_admin_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, Response> {
    serde_json::from_slice(body)
        .map_err(|_| ResponseHelper::json_error("The request body is not valid JSON".to_string()))
}

pub async fn handle_saml_settings_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let request: SettingsRequest = match parse_admin_body(&body) {
        Ok(request) => request,
        Err(resp) => return resp,
    };
    let settings = build_settings(request, Some(&admin));
    if let Err(e) = settings.validate() {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = saml::save(&settings) {
        return ResponseHelper::json_error(e);
    }

    // A mapping change demotes immediately, exactly like LDAP and OIDC: re-score
    // every SAML principal's stored group list under the new table.
    let repo = DirectoryRepository::new(state.db.pool());
    let identities = repo.list().await.unwrap_or_default();
    let mut rescored = 0usize;
    for identity in identities
        .iter()
        .filter(|identity| identity.provider == saml::PROVIDER_SAML)
    {
        let role = crate::directory::resolve_groups_role(
            &identity.groups,
            &settings.group_roles,
            &settings.default_role,
        );
        let role_id = if role == ROLE_ADMIN {
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
        "SAML settings updated by '{admin}' (enabled={}, provider={}, {rescored} role(s) rescored)",
        settings.enabled, settings.idp_entity_id
    ));

    handle_saml_get(State(state), headers).await
}

// ---------------------------------------------------------------------------
// POST /api/saml/test
// ---------------------------------------------------------------------------

/// Reads the IdP's metadata document for the settings as typed and answers the
/// values to fill in. Saves nothing: the admin still presses Save, because a
/// hidden save of a security setting is a surprise.
pub async fn handle_saml_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let request: TestRequest = match parse_admin_body(&body) {
        Ok(request) => request,
        Err(resp) => return resp,
    };
    let (metadata_url, current) = match request.settings {
        Some(inline) => {
            let url = inline.idp_metadata_url.trim().to_string();
            (url, Some(build_settings(inline, None)))
        }
        None => (saml::load().idp_metadata_url, None),
    };
    if metadata_url.is_empty() {
        return ResponseHelper::json_error(
            "Paste the address of the identity provider's metadata document".to_string(),
        );
    }
    match saml::fetch_metadata(&metadata_url).await {
        Ok(imported) => {
            let mut warnings = Vec::new();
            if let Some(settings) = &current {
                if !settings.acs_url.trim().is_empty()
                    && !settings.acs_url.trim().ends_with(saml::ACS_PATH)
                {
                    warnings.push(format!("The callback URL must end with {}", saml::ACS_PATH));
                }
            }
            ResponseHelper::json_success(json!({
                "ok": true,
                "message": "The identity provider answered.",
                "entity_id": imported.entity_id,
                "sso_url": imported.sso_url,
                "certificates": imported.certificates,
                "warnings": warnings,
            }))
        }
        Err(e) => ResponseHelper::json_error(e.detail().to_string()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/saml/metadata
// ---------------------------------------------------------------------------

/// This appliance's own metadata document, for the admin to paste into the IdP
/// (or to serve from the entity ID).
pub async fn handle_saml_metadata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let settings = saml::load();
    if settings.sp_entity_id.trim().is_empty() || settings.acs_url.trim().is_empty() {
        return ResponseHelper::json_error(
            "Fill in this appliance's entity ID and callback URL first".to_string(),
        );
    }
    ResponseHelper::json_success(json!({
        "entity_id": settings.sp_entity_id,
        "acs_url": settings.acs_url,
        "xml": saml::metadata(&settings),
    }))
}

// ---------------------------------------------------------------------------
// POST /api/auth/saml/start
// ---------------------------------------------------------------------------

/// Start a sign-in and answer the URL the browser must visit.
///
/// `POST` for the same reason as the OIDC start: a link that silently starts a
/// sign-in is a login-CSRF primitive. The request id is also written to a
/// cookie so the ACS can tell that *this* browser started the flow — on a
/// cross-site POST a `Lax` cookie is not sent, so over https the cookie is
/// `SameSite=None; Secure` and the ACS requires it.
pub async fn handle_saml_start(over_tls: Option<Extension<ArrivedOverTls>>) -> Response {
    let over_tls = over_tls.is_some();
    let settings = saml::load();
    if !settings.usable() {
        return ResponseHelper::json_error_with_status(
            StatusCode::SERVICE_UNAVAILABLE,
            "Single sign-on is not configured".to_string(),
        );
    }
    let (url, request_id) = match saml::begin_sign_in(&settings, chrono::Utc::now()) {
        Ok(pair) => pair,
        Err(e) => {
            crate::events::log_event_warn(format!("SAML sign-in could not start: {e}"));
            return ResponseHelper::json_error_with_status(
                StatusCode::SERVICE_UNAVAILABLE,
                e.detail().to_string(),
            );
        }
    };

    let mut response = ResponseHelper::json_success(json!({
        "authorize_url": url,
        "state": request_id,
    }));
    let secure = if over_tls { "; Secure" } else { "" };
    let cookie = format!(
        "{name}={value}; Path=/api/auth/saml; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        name = STATE_COOKIE,
        value = request_id,
        max_age = saml::REQUEST_TTL.as_secs(),
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

// ---------------------------------------------------------------------------
// POST /api/auth/saml/acs
// ---------------------------------------------------------------------------

/// The assertion consumer service: the one endpoint the IdP posts to.
///
/// Everything that can go wrong lands on the login screen with a short code.
/// The order is deliberate: the pending request is spent *before* the assertion
/// is parsed, so a replay of the same `RelayState` cannot be attempted twice
/// even if the first attempt failed late.
pub async fn handle_saml_acs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    over_tls: Option<Extension<ArrivedOverTls>>,
    Form(form): Form<AcsForm>,
) -> Response {
    let over_tls = over_tls.is_some();
    let ip_address = crate::handlers::auth::client_ip(&headers, peer.0);

    let settings = saml::load();
    if !settings.usable() {
        return acs_failure("disabled");
    }

    let relay = form.relay_state.trim().to_string();
    if relay.is_empty() {
        return acs_failure("state");
    }

    // The browser binding: over https the cookie is `SameSite=None; Secure` and
    // must be present; on a loopback http deployment it is `Lax` (which a
    // cross-site POST does not send), so only a *wrong* cookie is refused.
    let cookie = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| crate::app_proxy::cookie::named_cookie_from_header(value, STATE_COOKIE));
    match cookie {
        Some(value) if value != relay => return acs_failure("state"),
        None if settings.acs_url.starts_with("https://") => return acs_failure("state"),
        _ => {}
    }

    // Single use, and it is the `InResponseTo` the assertion must carry.
    let Some(pending) = saml::take_pending_request(&relay) else {
        return acs_failure("expired");
    };

    let encoded: String = form
        .saml_response
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if encoded.is_empty() {
        return acs_failure("state");
    }
    let raw = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        encoded.as_bytes(),
    ) {
        Ok(bytes) => bytes,
        Err(_) => return acs_failure("refused"),
    };
    let xml = match String::from_utf8(raw) {
        Ok(text) => text,
        Err(_) => return acs_failure("refused"),
    };

    let principal = match saml::parse_response(&xml, &settings, &relay, chrono::Utc::now()) {
        Ok(principal) => principal,
        Err(e) => {
            crate::events::log_event_warn(format!(
                "SAML sign-in from {ip_address} was refused ({}): {e}",
                e.code()
            ));
            return acs_failure(e.code());
        }
    };

    let role = saml::role_for(&principal, &settings);
    let role_id = if role == ROLE_ADMIN {
        ADMIN_ROLE_ID
    } else {
        USER_ROLE_ID
    };
    let repo = DirectoryRepository::new(state.db.pool());
    let update = IdentityUpdate {
        provider: saml::PROVIDER_SAML.to_string(),
        // The NameID is the provider's stable identifier for this person; the
        // sanitised login name is only how they are displayed and stored.
        subject: principal.name_id.clone(),
        username: principal.username.clone(),
        display_name: principal.display_name.clone().unwrap_or_default(),
        email: principal.email.clone().unwrap_or_default(),
        groups: principal.groups.clone(),
        role_id,
    };
    let identity = match repo.upsert_login(&update).await {
        Ok(identity) => identity,
        Err(e) => {
            crate::events::log_event_warn(format!(
                "SAML sign-in for '{}' from {ip_address} was refused: {e}",
                principal.username
            ));
            return acs_failure("account");
        }
    };
    if identity.disabled {
        return acs_failure("account_disabled");
    }

    let session = match state
        .auth
        .create_session(identity.username.clone(), ip_address.clone(), false)
        .await
    {
        Ok(session) => session,
        Err(e) => {
            crate::events::log_event_warn(format!("SAML sign-in: session creation failed: {e}"));
            return acs_failure("session");
        }
    };

    crate::events::log_event(format!(
        "User '{}' signed in through saml from {ip_address} as {role}",
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

    let issued = IssuedSession {
        token: session.token.clone(),
        csrf_token: session.csrf_token.clone(),
        username: session.username.clone(),
        is_admin: session.is_admin,
    };
    let ticket = match random_token(32) {
        Ok(ticket) => ticket,
        Err(_) => return acs_failure("session"),
    };
    if ISSUED.insert(&ticket, issued).is_err() {
        return acs_failure("session");
    }

    let next: String = url::form_urlencoded::byte_serialize(pending.return_to.as_bytes()).collect();
    let mut response =
        Redirect::to(&format!("/login#sso={ticket}&p=saml&next={next}")).into_response();
    response = crate::app_proxy::cookie::attach_session(response, &session.token, false, over_tls);
    clear_state_cookie(response)
}

/// A refusal lands on the login screen with a code the SPA translates.
fn acs_failure(code: &str) -> Response {
    clear_state_cookie(Redirect::to(&format!("/login?sso_error={code}")).into_response())
}

fn clear_state_cookie(mut response: Response) -> Response {
    let cookie = format!(
        "{}=; Path=/api/auth/saml; HttpOnly; SameSite=Lax; Max-Age=0",
        STATE_COOKIE
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

// ---------------------------------------------------------------------------
// POST /api/auth/saml/complete
// ---------------------------------------------------------------------------

/// Trade the one-shot ticket for the session the SPA keeps in `localStorage`.
/// The same store as the OIDC half: a ticket is a ticket, and it is spent once.
pub async fn handle_saml_complete(Json(request): Json<CompleteRequest>) -> Response {
    let Some(issued) = ISSUED.take(request.code.trim()) else {
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

// ---------------------------------------------------------------------------
// GET /api/auth/saml/metadata (public)
// ---------------------------------------------------------------------------

/// The SP metadata document, served so an IdP that fetches metadata by URL can
/// read it. Public on purpose: it carries no secret, and some IdPs will not
/// accept an uploaded file.
pub async fn handle_public_saml_metadata() -> Response {
    let settings = saml::load();
    if settings.sp_entity_id.trim().is_empty() || settings.acs_url.trim().is_empty() {
        return (
            StatusCode::NOT_FOUND,
            [("content-type", "text/plain; charset=utf-8")],
            Body::from("SAML is not configured on this appliance\n"),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(
            "content-type",
            "application/samlmetadata+xml; charset=utf-8",
        )],
        Body::from(saml::metadata(&settings)),
    )
        .into_response()
}
