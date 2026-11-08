/* =============================================================================
 * File          : directory.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface for the LDAP/AD directory integration (ROADMAP P3.1, stage 1)
 *   plus the hook the login handler calls: `directory_login` returns
 *   `Some(response)` only when the account being signed into belongs to the
 *   directory, so a local sign-in is never touched.
 *
 *   Reads and writes are admin-only — the settings carry a service-account
 *   password and decide who becomes an administrator. The bind password itself
 *   is sealed with the appliance key and never leaves the server: the screen
 *   sees `has_bind_password`, not the value.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::directory, crate::storage::directory_repo
 *
 * Usage:
 *   Routes in src/web/mod.rs; see docs/guides/directory-sso.md.
 * =============================================================================
 */

//! Directory sign-in endpoints and the login hook (see
//! docs/guides/directory-sso.md).
//!
//! | Route                                            | Guard |
//! |--------------------------------------------------|-------|
//! | `GET    /api/directory`                          | admin |
//! | `PUT    /api/directory/settings`                 | admin |
//! | `POST   /api/directory/test`                     | admin |
//! | `POST   /api/directory/identities/{id}/disabled` | admin |
//! | `DELETE /api/directory/identities/{id}`          | admin |

use std::sync::{Arc, LazyLock};

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::directory::{self, DirectoryError, DirectorySettings, DirectoryUser, GroupRole};
use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::storage::directory_repo::{DirectoryRepository, IdentityUpdate, AUTH_SOURCE_LOCAL};
use crate::storage::models::ADMIN_ROLE_ID;
use crate::utils::ResponseHelper;

/// The process-wide directory service.
///
/// One instance is enough: it holds only the bounded lookup cache, and every
/// call re-reads the settings from disk. Keeping it out of `AppState` means the
/// integration cannot break a router that was assembled without it.
pub fn service() -> &'static directory::DirectoryService {
    static SERVICE: LazyLock<directory::DirectoryService> =
        LazyLock::new(directory::DirectoryService::new);
    &SERVICE
}

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// The settings as the screen needs them: everything but the sealed password.
#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub enabled: bool,
    pub provider: String,
    pub server_url: String,
    pub start_tls: bool,
    pub verify_tls: bool,
    pub bind_dn: String,
    /// Whether a bind password is stored — never the password itself.
    pub has_bind_password: bool,
    pub base_dn: String,
    pub user_filter: String,
    pub group_filter: String,
    pub attr_username: String,
    pub attr_display_name: String,
    pub attr_email: String,
    pub attr_member_of: String,
    pub default_role: String,
    pub group_roles: Vec<GroupRole>,
    pub cache_ttl_seconds: u64,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
}

impl SettingsView {
    fn from_settings(settings: &DirectorySettings) -> Self {
        Self {
            enabled: settings.enabled,
            provider: settings.provider.clone(),
            server_url: settings.server_url.clone(),
            start_tls: settings.start_tls,
            verify_tls: settings.verify_tls,
            bind_dn: settings.bind_dn.clone(),
            has_bind_password: settings.bind_password_sealed.is_some(),
            base_dn: settings.base_dn.clone(),
            user_filter: settings.user_filter.clone(),
            group_filter: settings.group_filter.clone(),
            attr_username: settings.attr_username.clone(),
            attr_display_name: settings.attr_display_name.clone(),
            attr_email: settings.attr_email.clone(),
            attr_member_of: settings.attr_member_of.clone(),
            default_role: settings.default_role.clone(),
            group_roles: settings.group_roles.clone(),
            cache_ttl_seconds: settings.cache_ttl_seconds,
            updated_at: settings.updated_at.clone(),
            updated_by: settings.updated_by.clone(),
        }
    }
}

/// What the screen may change. `bind_password` is write-only: absent keeps the
/// stored secret, an empty string clears it.
#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    pub enabled: bool,
    pub provider: String,
    pub server_url: String,
    #[serde(default)]
    pub start_tls: bool,
    #[serde(default = "default_true")]
    pub verify_tls: bool,
    #[serde(default)]
    pub bind_dn: String,
    pub bind_password: Option<String>,
    pub base_dn: String,
    pub user_filter: String,
    #[serde(default)]
    pub group_filter: String,
    pub attr_username: String,
    pub attr_display_name: String,
    pub attr_email: String,
    pub attr_member_of: String,
    #[serde(default = "default_role")]
    pub default_role: String,
    #[serde(default)]
    pub group_roles: Vec<GroupRole>,
    #[serde(default = "default_ttl")]
    pub cache_ttl_seconds: u64,
}

fn default_true() -> bool {
    true
}

fn default_role() -> String {
    directory::ROLE_USER.to_string()
}

fn default_ttl() -> u64 {
    directory::DEFAULT_CACHE_TTL_SECONDS
}

#[derive(Debug, Deserialize)]
pub struct TestRequest {
    /// Unsaved settings to try. Absent means "test what is stored".
    pub settings: Option<SettingsRequest>,
    /// Attempt a full sign-in as this principal (search + password bind).
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DisabledRequest {
    pub disabled: bool,
}

// ---------------------------------------------------------------------------
// GET /api/directory
// ---------------------------------------------------------------------------

/// The whole screen in one payload: settings, health, principals and the
/// reassurance that a local administrator still exists.
pub async fn handle_directory_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let settings = directory::load();
    let problem = directory::validate(&settings).err();
    let repo = DirectoryRepository::new(state.db.pool());

    let identities = repo.list().await.unwrap_or_default();
    let (admins, users) = repo.role_counts().await.unwrap_or((0, 0));
    let disabled = identities.iter().filter(|i| i.disabled).count() as i64;
    let (hits, misses, entries) = service().statistics();

    ResponseHelper::json_success(json!({
        "settings": SettingsView::from_settings(&settings),
        "status": {
            "enabled": settings.enabled,
            "usable": settings.usable(),
            "problem": problem,
            "cache": { "hits": hits, "misses": misses, "entries": entries },
        },
        "identities": identities,
        "counts": {
            "total": identities.len() as i64,
            "admins": admins,
            "users": users,
            "disabled": disabled,
        },
        "break_glass": {
            // The count that lets an admin see, before saving, that locking
            // themselves out of the directory still leaves a way in.
            "local_admins": state.auth.count_local_admins().await,
            "policy": "A local account always keeps its own password, so directory sign-in can never lock an administrator out."
        }
    }))
}

// ---------------------------------------------------------------------------
// PUT /api/directory/settings
// ---------------------------------------------------------------------------

/// Build the settings document from a request, unsealing nothing and sealing
/// the new password when one was typed.
fn build_settings(
    request: SettingsRequest,
    current: &DirectorySettings,
    updated_by: Option<&str>,
) -> Result<DirectorySettings, String> {
    let sealed = match request.bind_password.as_deref() {
        // Absent: keep whatever is stored.
        None => current.bind_password_sealed.clone(),
        // Explicitly cleared.
        Some("") => None,
        Some(password) => Some(directory::seal_bind_password(password)?),
    };

    Ok(DirectorySettings {
        enabled: request.enabled,
        provider: request.provider,
        server_url: request.server_url.trim().to_string(),
        start_tls: request.start_tls,
        verify_tls: request.verify_tls,
        bind_dn: request.bind_dn.trim().to_string(),
        bind_password_sealed: sealed,
        base_dn: request.base_dn.trim().to_string(),
        user_filter: request.user_filter.trim().to_string(),
        group_filter: request.group_filter.trim().to_string(),
        attr_username: request.attr_username.trim().to_string(),
        attr_display_name: request.attr_display_name.trim().to_string(),
        attr_email: request.attr_email.trim().to_string(),
        attr_member_of: request.attr_member_of.trim().to_string(),
        default_role: request.default_role.trim().to_ascii_lowercase(),
        group_roles: request
            .group_roles
            .into_iter()
            .map(|m| GroupRole {
                group: m.group.trim().to_string(),
                role: m.role.trim().to_ascii_lowercase(),
            })
            .filter(|m| !m.group.is_empty() || !m.role.is_empty())
            .collect(),
        cache_ttl_seconds: request.cache_ttl_seconds,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
        updated_by: updated_by.map(|u| u.to_string()),
    })
}

pub async fn handle_directory_settings_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SettingsRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };

    let current = directory::load();
    let settings = match build_settings(request, &current, Some(&admin)) {
        Ok(settings) => settings,
        Err(e) => return ResponseHelper::json_error(e),
    };
    if let Err(e) = directory::validate(&settings) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = directory::save(&settings) {
        return ResponseHelper::json_error(e);
    }

    // The configuration changed: cached searches belong to the old one, and the
    // stored group lists must be re-scored so a demotion is immediate.
    service().invalidate();
    let repo = DirectoryRepository::new(state.db.pool());
    let mut demoted = 0usize;
    let mut promoted = 0usize;
    for (id, username, groups) in repo.identities_for_resync().await.unwrap_or_default() {
        let role = directory::resolve_role(&groups, &settings);
        let role_id = if role == directory::ROLE_ADMIN {
            ADMIN_ROLE_ID
        } else {
            crate::storage::models::USER_ROLE_ID
        };
        if let Ok(Some(identity)) = repo.find(id).await {
            if identity.role_id != role_id {
                if role_id == ADMIN_ROLE_ID {
                    promoted += 1;
                } else {
                    demoted += 1;
                }
            }
        }
        let _ = repo.set_role(id, &username, role_id).await;
    }

    crate::events::log_event(format!(
        "Directory sign-in settings updated by '{admin}' (enabled={}, provider={}, {} promoted, {} demoted)",
        settings.enabled, settings.provider, promoted, demoted
    ));

    handle_directory_get(State(state), headers).await
}

// ---------------------------------------------------------------------------
// POST /api/directory/test
// ---------------------------------------------------------------------------

/// Try the configuration without saving it.
///
/// With just the service account it proves the bind and the search base; with
/// `username` + `password` it runs the whole sign-in (search, password bind and
/// group→role resolution) and answers the role it would have granted — the
/// quickest way to find out that the mapping is wrong before anyone signs in.
pub async fn handle_directory_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<TestRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let (settings, inline_password) = match request.settings {
        Some(inline) => {
            let current = directory::load();
            let plain = inline.bind_password.clone();
            let mut settings = match build_settings(inline, &current, None) {
                Ok(settings) => settings,
                Err(e) => return ResponseHelper::json_error(e),
            };
            // A test must not persist anything, not even the timestamp.
            settings.updated_at = current.updated_at.clone();
            settings.updated_by = current.updated_by.clone();
            (settings, plain)
        }
        None => (directory::load(), None),
    };

    if let Err(e) = directory::validate(&settings) {
        return ResponseHelper::json_error(e);
    }

    // The password to use: the one typed into the test form, or the stored one.
    let bind_password = match inline_password {
        Some(password) if !password.is_empty() => Some(password),
        _ => match directory::open_bind_password(&settings.bind_password_sealed) {
            Ok(password) => password,
            Err(e) => return ResponseHelper::json_error(e),
        },
    };

    let service = service();
    // A test starts from a clean cache, so it can never be answered by a result
    // stored under a different configuration.
    service.invalidate();

    let Some(username) = request
        .username
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    else {
        // Service-account check only.
        return match probe_service_bind(&settings, bind_password.as_deref()).await {
            Ok(()) => ResponseHelper::json_success(json!({
                "ok": true,
                "message": "Connected to the directory.",
            })),
            Err(e) => ResponseHelper::json_error(e),
        };
    };

    let password = request.password.unwrap_or_default();
    match service
        .authenticate(&settings, bind_password.as_deref(), username, &password)
        .await
    {
        Ok(user) => {
            let role = directory::resolve_role(&user.groups, &settings);
            ResponseHelper::json_success(json!({
                "ok": true,
                "message": format!("'{username}' would sign in as {role}."),
                "user": {
                    "dn": user.dn,
                    "username": user.username,
                    "display_name": user.display_name,
                    "email": user.email,
                    "groups": user.groups,
                },
                "role": role,
            }))
        }
        Err(e) => ResponseHelper::json_error(e.message()),
    }
}

/// Prove the service account can bind and that the search base exists.
async fn probe_service_bind(
    settings: &DirectorySettings,
    bind_password: Option<&str>,
) -> Result<(), String> {
    // A search for a name nobody has: an empty result still proves the bind and
    // the base are good, and it cannot leak whether a specific person exists.
    let filter = directory::render_filter(
        &settings.user_filter,
        "username",
        "homevault-connection-probe",
    );
    match probe(settings, bind_password, &filter).await {
        Ok(()) => Ok(()),
        Err(DirectoryError::InvalidCredentials) => Ok(()),
        Err(e) => Err(e.message()),
    }
}

/// One search, used only by the test button.
async fn probe(
    settings: &DirectorySettings,
    bind_password: Option<&str>,
    filter: &str,
) -> Result<(), DirectoryError> {
    // The probe reuses the real backend's search but must not be confused by a
    // "no such entry" answer, which is the expected outcome.
    let backend = directory::LdapBackend;
    backend
        .search(settings, bind_password, filter)
        .await
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// Principals
// ---------------------------------------------------------------------------

pub async fn handle_identity_disabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(request): Json<DisabledRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let repo = DirectoryRepository::new(state.db.pool());
    let identity = match repo.find(id).await {
        Ok(Some(identity)) => identity,
        Ok(None) => return ResponseHelper::json_error("No such directory account".to_string()),
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };
    if let Err(e) = repo
        .set_disabled(id, &identity.username, request.disabled)
        .await
    {
        return ResponseHelper::json_internal_error(e.to_string());
    }
    service().invalidate();
    crate::events::log_event(format!(
        "Directory account '{}' {} by '{admin}'",
        identity.username,
        if request.disabled {
            "disabled"
        } else {
            "enabled"
        }
    ));
    handle_directory_get(State(state), headers).await
}

pub async fn handle_identity_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let repo = DirectoryRepository::new(state.db.pool());
    match repo.delete(id).await {
        Ok(Some(identity)) => {
            service().invalidate();
            crate::events::log_event(format!(
                "Directory account '{}' forgotten by '{admin}' — its sessions were revoked",
                identity.username
            ));
            handle_directory_get(State(state), headers).await
        }
        Ok(None) => ResponseHelper::json_error("No such directory account".to_string()),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Login hook
// ---------------------------------------------------------------------------

/// What the login handler should do with a name.
pub enum DirectoryLogin {
    /// Not ours: run the normal local password check.
    Local,
    /// Ours: send this response.
    Answer(Response),
}

/// Sign a directory principal in, or hand the request back to the local path.
///
/// The decision is made from `users.auth_source`, so it cannot be influenced by
/// the request body: a name that belongs to a local account always takes the
/// local path, and a name the directory owns always takes the directory path.
pub async fn directory_login(
    state: &Arc<AppState>,
    username: &str,
    password: &str,
    ip_address: &str,
    remember_me: bool,
    user_agent: Option<&str>,
    over_tls: bool,
) -> DirectoryLogin {
    let username = username.trim();
    if username.is_empty() {
        return DirectoryLogin::Local;
    }
    let repo = DirectoryRepository::new(state.db.pool());
    let source = match repo.auth_source(username).await {
        Ok(source) => source,
        Err(_) => return DirectoryLogin::Local,
    };
    if source.as_deref() == Some(AUTH_SOURCE_LOCAL) {
        return DirectoryLogin::Local;
    }

    let settings = directory::load();
    let known = repo.find_by_username(username).await.ok().flatten();
    // The provider stored on the account decides the login path, exactly as
    // `users.auth_source` does. An account mirrored from OpenID Connect (or
    // SAML, when that stage lands) must never be authenticated by an LDAP bind
    // that happens to share its name: two providers vouching for the same
    // string are still two different principals, and a password typed for one
    // must not open the other.
    if let Some(provider) = known.as_ref().map(|identity| identity.provider.as_str()) {
        if !matches!(provider, directory::PROVIDER_LDAP | directory::PROVIDER_AD) {
            return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
                axum::http::StatusCode::FORBIDDEN,
                "This account signs in through single sign-on — use the single sign-on button"
                    .to_string(),
            ));
        }
    }
    if !settings.enabled {
        // A name only the directory knows, with the integration off: say so
        // instead of "wrong password", which would send the user hunting for a
        // password that does not exist here.
        return match (source.as_deref(), known) {
            (Some(_), _) | (None, Some(_)) => {
                DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
                    axum::http::StatusCode::UNAUTHORIZED,
                    "Directory sign-in is turned off — sign in with a local account".to_string(),
                ))
            }
            (None, None) => DirectoryLogin::Local,
        };
    }

    // The local IP limiter still applies: an LDAP bind is a network round trip
    // an attacker could otherwise hammer for free.
    if let Err(e) = state.auth.check_ip_rate_limit(ip_address) {
        return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            format!("{e}"),
        ));
    }

    let bind_password = match directory::open_bind_password(&settings.bind_password_sealed) {
        Ok(password) => password,
        Err(e) => {
            return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                e,
            ))
        }
    };

    let outcome = service()
        .authenticate(&settings, bind_password.as_deref(), username, password)
        .await;

    let user: DirectoryUser = match outcome {
        Ok(user) => user,
        Err(DirectoryError::InvalidCredentials) => {
            crate::events::log_event_warn(format!(
                "Failed directory sign-in for '{username}' from {ip_address}"
            ));
            crate::auto_block::record_failure(state, ip_address, "").await;
            return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
                axum::http::StatusCode::UNAUTHORIZED,
                "Invalid username or password".to_string(),
            ));
        }
        Err(e) => {
            // An outage is not a wrong password: it must not add to the
            // lockout ladder, and it must name the way in that still works.
            crate::events::log_event_warn(format!(
                "Directory sign-in for '{username}' from {ip_address} could not be completed: {e:?}"
            ));
            return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                e.message(),
            ));
        }
    };

    let role = directory::resolve_role(&user.groups, &settings);
    let role_id = if role == directory::ROLE_ADMIN {
        ADMIN_ROLE_ID
    } else {
        crate::storage::models::USER_ROLE_ID
    };
    let update = IdentityUpdate {
        provider: settings.provider.clone(),
        subject: user.dn.clone(),
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        email: user.email.clone(),
        groups: user.groups.clone(),
        role_id,
    };
    let identity = match repo.upsert_login(&update).await {
        Ok(identity) => identity,
        Err(e) => {
            return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
                axum::http::StatusCode::FORBIDDEN,
                format!("{e}"),
            ))
        }
    };
    if identity.disabled {
        return DirectoryLogin::Answer(ResponseHelper::json_error_with_status(
            axum::http::StatusCode::FORBIDDEN,
            "This directory account is disabled on this appliance".to_string(),
        ));
    }

    match state
        .auth
        .create_session(user.username.clone(), ip_address.to_string(), remember_me)
        .await
    {
        Ok(session) => {
            crate::events::log_event(format!(
                "User '{}' signed in through {} from {} as {}",
                session.username, settings.provider, ip_address, role
            ));
            let _ = state
                .auth
                .record_successful_attempt(&session.username, ip_address, user_agent)
                .await;
            let token = session.token.clone();
            let can_manage_accounts = state.auth.can_manage_accounts(&session.username).await;
            let response = crate::storage::models::LoginResponse {
                token: session.token,
                csrf_token: session.csrf_token,
                username: session.username,
                is_admin: session.is_admin,
                can_manage_accounts,
                // A directory password is never HomeVault's to change.
                must_change_password: false,
            };
            DirectoryLogin::Answer(crate::app_proxy::cookie::attach_session(
                ResponseHelper::json_success(response),
                &token,
                remember_me,
                over_tls,
            ))
        }
        Err(e) => DirectoryLogin::Answer(ResponseHelper::json_internal_error(format!(
            "Failed to create session: {e}"
        ))),
    }
}

/// Whether an account's password is managed somewhere else, so the
/// change-password endpoint can say so instead of "wrong current password".
pub async fn password_is_managed_elsewhere(
    state: &Arc<AppState>,
    username: &str,
) -> Option<&'static str> {
    let repo = DirectoryRepository::new(state.db.pool());
    match repo.auth_source(username).await {
        Ok(Some(source)) if source == "directory" => {
            Some("Your password is managed by your directory — change it there, not here.")
        }
        _ => None,
    }
}
