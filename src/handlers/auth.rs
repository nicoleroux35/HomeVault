/* =============================================================================
 * File          : auth.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Every authentication and first-run setup endpoint: login with optional
 *   two-factor verification, logout, current-user lookup, the whole Setup
 *   Wizard sequence (status, username check, account creation, TOTP
 *   verification, progress tracking, re-baseline) and the recovery-code based
 *   password reset. The endpoints that run before any account exists are
 *   additionally restricted to the local network by the setup guard
 *   middleware.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes under /api/auth and /api/setup in src/web/mod.rs; this is the only
 *   place that also sets the app-proxy session cookie (on login).
 * =============================================================================
 */

//! Authentication and first-run setup endpoints: login (with optional 2FA),
//! logout, current-user lookup, the Setup Wizard flow, and the recovery-code
//! password reset.

use crate::auth::service::AuthError;
use crate::handlers::AppState;
use crate::setup::SetupService;
use crate::storage::models::{
    LoginRequest, LoginResponse, SetupRequest, SetupStatusResponse, TwoFactorSetupRequest,
    TwoFactorVerifyRequest, ADMIN_ROLE_ID,
};
use crate::storage::SetupProgressRepository;
use crate::storage::SetupStateRepository;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Confirmation of a successful password reset.
#[derive(Debug, Serialize)]
pub struct ResetPasswordResponse {
    pub message: String,
}

/// Response carrying a freshly generated recovery code (shown to the user once).
#[derive(Debug, Serialize)]
pub struct RecoveryCodeResponse {
    pub recovery_code: String,
}

/// Query flags for [`handle_generate_recovery_code`].
#[derive(Debug, Deserialize)]
pub struct RecoveryCodeQuery {
    /// When true, replace an existing recovery code instead of returning a
    /// conflict.
    #[serde(default)]
    pub regenerate: bool,
}

/// Body for resetting a password with a numeric recovery code.
#[derive(Debug, Deserialize)]
pub struct ResetPasswordRecoveryRequest {
    pub username: String,
    pub recovery_code: String,
    pub new_password: String,
}

/// Body for checking a recovery code without resetting anything.
#[derive(Debug, Deserialize)]
pub struct VerifyRecoveryCodeRequest {
    pub username: String,
    pub recovery_code: String,
}

/// Resolves the username from the `x-session-token` header, if present.
pub async fn extract_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Option<String> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return None;
    }

    state.auth.get_username_from_token(session_token).await
}

/// Resolves the client IP for audit records and rate limiting.
///
/// The **TCP peer** is the authority. `X-Forwarded-For` / `X-Real-IP` are
/// client-settable, so trusting them unconditionally (as this used to do) let
/// anyone forge the address recorded in the login history and — now that the
/// authentication throttle is keyed by IP — sidestep it entirely by rotating a
/// header value.
///
/// They are honoured only when the request really came through a reverse proxy
/// on this host, i.e. when the peer is loopback, or when the peer is listed in
/// `HOMEVAULT_TRUSTED_PROXIES` (comma-separated IPs). Otherwise the peer address
/// wins and the headers are ignored.
pub fn client_ip(headers: &HeaderMap, peer: Option<std::net::IpAddr>) -> String {
    let forwarded = || {
        headers
            .get("x-forwarded-for")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                headers
                    .get("x-real-ip")
                    .and_then(|h| h.to_str().ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
    };

    match peer {
        Some(ip) if ip.is_loopback() || is_trusted_proxy(&ip) => {
            forwarded().unwrap_or_else(|| ip.to_string())
        }
        Some(ip) => ip.to_string(),
        // No connection metadata (in-process calls, e.g. tests): fall back to
        // the headers, then to "unknown".
        None => forwarded().unwrap_or_else(|| "unknown".to_string()),
    }
}

/// Whether `ip` is listed in `HOMEVAULT_TRUSTED_PROXIES`.
fn is_trusted_proxy(ip: &std::net::IpAddr) -> bool {
    std::env::var("HOMEVAULT_TRUSTED_PROXIES")
        .ok()
        .map(|list| {
            list.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .any(|s| {
                    s.parse::<std::net::IpAddr>()
                        .map(|p| p == *ip)
                        .unwrap_or(false)
                })
        })
        .unwrap_or(false)
}

/// Legacy helper kept for callers that have no connection metadata.
///
/// Prefer [`client_ip`]: this one trusts the forwarding headers outright and is
/// only correct for in-process calls.
pub fn extract_client_ip(headers: &HeaderMap) -> String {
    client_ip(headers, None)
}

/// Guards a handler to admins only.
///
/// Returns `Ok(username)` on success or an error response on failure.
// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
pub async fn require_admin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<String, Response> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Err(ResponseHelper::json_error(
            "No session token provided".to_string(),
        ));
    }

    match state.auth.get_username_from_token(session_token).await {
        Some(username) => {
            if state.auth.is_admin(&username).await {
                Ok(username)
            } else {
                Err(ResponseHelper::json_forbidden(
                    "Admin privileges required".to_string(),
                ))
            }
        }
        None => Err(ResponseHelper::json_unauthorized(
            "Invalid session".to_string(),
        )),
    }
}

#[axum_macros::debug_handler]
/// Authenticates a user and returns a session token.
///
/// If 2FA is enabled for the account, returns a 2FA-required signal instead.
pub async fn handle_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    over_tls: Option<axum::Extension<crate::middleware::ArrivedOverTls>>,
    req: axum::extract::Json<LoginRequest>,
) -> Response {
    let ip_address = client_ip(&headers, peer.0);
    let user_agent = headers.get("user-agent").and_then(|h| h.to_str().ok());
    let over_tls = over_tls.is_some();

    match state
        .auth
        .authenticate(&req.username, &req.password, &ip_address, user_agent)
        .await
    {
        Ok(_) => {
            let two_fa_enabled = state
                .auth
                .is_2fa_enabled(&req.username)
                .await
                .unwrap_or(false);

            if two_fa_enabled {
                return ResponseHelper::json_2fa_required();
            }

            match state
                .auth
                .create_session(req.username.clone(), ip_address.clone(), req.remember_me)
                .await
            {
                Ok(session) => {
                    crate::events::log_event(format!(
                        "User '{}' logged in successfully from {}",
                        session.username, ip_address
                    ));
                    // Keep the audit history truthful: record the successful
                    // login (with the browser fingerprint) now that a session
                    // exists.
                    let _ = state
                        .auth
                        .record_successful_attempt(&session.username, &ip_address, user_agent)
                        .await;
                    // The API is header-authenticated; the cookie is read
                    // ONLY by the per-app reverse proxy's auth gate
                    // (crate::app_proxy). Additive — response body unchanged.
                    let token = session.token.clone();
                    // An account still holding an admin-chosen password
                    // (creation/reset) must change it before anything else:
                    // the client routes to the change screen, and the server
                    // enforces the same restriction per-request
                    // (middleware::password_change_guard). A PAM/system
                    // account has no users row and no flag.
                    let must_change_password = state
                        .auth
                        .get_user(&req.username)
                        .await
                        .map(|u| u.must_change_password)
                        .unwrap_or(false);
                    let response = LoginResponse {
                        token: session.token,
                        csrf_token: session.csrf_token,
                        username: session.username,
                        is_admin: session.is_admin,
                        must_change_password,
                    };
                    crate::app_proxy::cookie::attach_session(
                        ResponseHelper::json_success(response),
                        &token,
                        req.remember_me,
                        over_tls,
                    )
                }
                Err(e) => {
                    ResponseHelper::json_internal_error(format!("Failed to create session: {}", e))
                }
            }
        }
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Failed login attempt for user '{}' from {}",
                req.username, ip_address
            ));
            let (message, status) = match &e {
                AuthError::UserNotFound => (
                    "Invalid username or password".to_string(),
                    axum::http::StatusCode::UNAUTHORIZED,
                ),
                AuthError::InvalidPassword => (
                    "Invalid username or password".to_string(),
                    axum::http::StatusCode::UNAUTHORIZED,
                ),
                AuthError::UserLocked => (
                    "Account is locked".to_string(),
                    axum::http::StatusCode::UNAUTHORIZED,
                ),
                AuthError::AccountExpired => (
                    "Account has expired — contact your administrator".to_string(),
                    axum::http::StatusCode::UNAUTHORIZED,
                ),
                AuthError::TooManyAttempts => (
                    "Too many login attempts. Please try again later.".to_string(),
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                ),
                AuthError::AccountLocked => (
                    "Account temporarily locked due to too many failed attempts.".to_string(),
                    axum::http::StatusCode::UNAUTHORIZED,
                ),
                _ => (
                    format!("Authentication failed: {}", e),
                    axum::http::StatusCode::UNAUTHORIZED,
                ),
            };

            // Honour the status chosen above: a throttled client must see 429,
            // not a generic 400 (the frontend branches on it, and a 400 tells a
            // monitoring tool the request was malformed rather than rate
            // limited).
            ResponseHelper::json_error_with_status(status, message)
        }
    }
}

#[axum_macros::debug_handler]
/// Verifies a 2FA TOTP code and issues a session on success.
pub async fn handle_2fa_verify(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    over_tls: Option<axum::Extension<crate::middleware::ArrivedOverTls>>,
    req: axum::extract::Json<TwoFactorVerifyRequest>,
) -> Response {
    let ip_address = client_ip(&headers, peer.0);
    let user_agent = headers.get("user-agent").and_then(|h| h.to_str().ok());
    let over_tls = over_tls.is_some();

    match state
        .auth
        .authenticate_with_2fa(
            &req.username,
            &req.password,
            &req.totp_code,
            &ip_address,
            req.remember_me,
            user_agent,
        )
        .await
    {
        Ok(session) => {
            crate::events::log_event(format!(
                "User '{}' logged in successfully with 2FA from {}",
                session.username, ip_address
            ));
            let token = session.token.clone();
            let must_change_password = state
                .auth
                .get_user(&req.username)
                .await
                .map(|u| u.must_change_password)
                .unwrap_or(false);
            let response = LoginResponse {
                token: session.token,
                csrf_token: session.csrf_token,
                username: session.username,
                is_admin: session.is_admin,
                must_change_password,
            };
            crate::app_proxy::cookie::attach_session(
                ResponseHelper::json_success(response),
                &token,
                req.remember_me,
                over_tls,
            )
        }
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Failed 2FA login attempt for user '{}' from {}",
                req.username, ip_address
            ));
            let message = match e {
                AuthError::InvalidTwoFactorCode => "Invalid verification code".to_string(),
                AuthError::InvalidPassword => "Invalid username or password".to_string(),
                _ => format!("Authentication failed: {}", e),
            };
            ResponseHelper::json_unauthorized(message)
        }
    }
}

#[axum_macros::debug_handler]
/// Manages 2FA lifecycle: generate secret, verify & enable, disable, or query status.
pub async fn handle_2fa_setup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<TwoFactorSetupRequest>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    match req.action.as_str() {
        "generate" => match state.auth.generate_totp_secret(&username).await {
            Ok((secret, qrcode_url)) => {
                // Persist as the pending (not yet enabled) secret so the
                // subsequent verification step validates against this exact secret.
                let repo = crate::storage::User2faRepository::new(state.db.pool());
                if let Err(e) = repo.save_pending(&username, &secret).await {
                    return ResponseHelper::json_error(format!("Failed to store secret: {}", e));
                }
                ResponseHelper::json_success(serde_json::json!({
                    "secret": secret,
                    "qrcode_url": qrcode_url
                }))
            }
            Err(e) => ResponseHelper::json_error(format!("Failed to generate secret: {}", e)),
        },
        "verify" => {
            let code = match &req.totp_code {
                Some(c) => c,
                None => {
                    return ResponseHelper::json_error("Verification code required".to_string())
                }
            };

            let user_repo = crate::storage::UserRepository::new(state.db.pool());
            let user = match user_repo.find_by_username(&username).await {
                Ok(Some(u)) => u,
                _ => return ResponseHelper::json_error("User not found".to_string()),
            };

            if !state
                .auth
                .verify_password(req.password.as_deref().unwrap_or(""), &user.password_hash)
            {
                return ResponseHelper::json_unauthorized("Invalid password".to_string());
            }

            // Read the pending secret that was shown to the user during "generate".
            let repo = crate::storage::User2faRepository::new(state.db.pool());
            let pending = match repo.get_by_username(&username).await {
                Ok(Some(p)) => p,
                _ => {
                    return ResponseHelper::json_error(
                        "No pending 2FA secret. Generate one first.".to_string(),
                    )
                }
            };
            let secret = pending.totp_secret;

            let code_valid = state
                .auth
                .verify_totp_code(&username, code)
                .await
                .unwrap_or(false);
            if !code_valid {
                return ResponseHelper::json_unauthorized("Invalid verification code".to_string());
            }

            match state.auth.enable_2fa(&username, &secret).await {
                Ok(backup_codes) => ResponseHelper::json_success(serde_json::json!({
                    "enabled": true,
                    "backup_codes": backup_codes
                })),
                Err(e) => ResponseHelper::json_error(format!("Failed to enable 2FA: {}", e)),
            }
        }
        "disable" => {
            let user_repo = crate::storage::UserRepository::new(state.db.pool());
            let user = match user_repo.find_by_username(&username).await {
                Ok(Some(u)) => u,
                _ => return ResponseHelper::json_error("User not found".to_string()),
            };

            let password = match &req.password {
                Some(p) => p,
                None => return ResponseHelper::json_error("Password required".to_string()),
            };

            if !state.auth.verify_password(password, &user.password_hash) {
                return ResponseHelper::json_unauthorized("Invalid password".to_string());
            }

            match state.auth.disable_2fa(&username).await {
                Ok(_) => ResponseHelper::json_success(serde_json::json!({
                    "enabled": false
                })),
                Err(e) => ResponseHelper::json_error(format!("Failed to disable 2FA: {}", e)),
            }
        }
        "status" => {
            let enabled = state.auth.is_2fa_enabled(&username).await.unwrap_or(false);
            ResponseHelper::json_success(serde_json::json!({
                "enabled": enabled
            }))
        }
        "backup_codes" => {
            let codes = state
                .auth
                .get_backup_codes(&username)
                .await
                .unwrap_or_default();
            ResponseHelper::json_success(serde_json::json!({
                "codes": codes
            }))
        }
        _ => ResponseHelper::json_error("Invalid action".to_string()),
    }
}

#[axum_macros::debug_handler]
/// Invalidates the current session, logging the user out.
pub async fn handle_logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(username) => username,
        // Unlike every other authenticated endpoint, this handler previously
        // only checked the header was non-empty and reported success
        // regardless — a garbage/invalid token got the same "Logged out
        // successfully" response as a real one instead of a rejection.
        None => return ResponseHelper::json_error("Invalid session".to_string()),
    };

    match state.auth.invalidate_session(session_token).await {
        Ok(_) => {
            crate::events::log_event(format!("User '{}' logged out", username));
            // Drop any app-proxy sessions this user holds, then expire both
            // cookies so a proxied app can't be reached after logout.
            crate::app_proxy::app_auth::APP_AUTH.revoke_user(&username);
            crate::app_proxy::cookie::clear_app_session(crate::app_proxy::cookie::clear_session(
                ResponseHelper::json_success("Logged out successfully".to_string()),
            ))
        }
        Err(e) => ResponseHelper::json_error(format!("Logout failed: {}", e)),
    }
}

#[axum_macros::debug_handler]
/// Returns profile details for the currently authenticated user.
pub async fn handle_get_current_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    match state.auth.get_session(session_token).await {
        Ok(session) => match state.auth.get_user(&session.username).await {
            Ok(user) => {
                let two_fa_enabled = state
                    .auth
                    .is_2fa_enabled(&session.username)
                    .await
                    .unwrap_or(false);

                let user_data = serde_json::json!({
                    "username": user.username,
                    "fullname": user.fullname,
                    "email": user.email,
                    "role": user.role_name().to_string(),
                    "role_id": user.role_id,
                    "is_admin": session.is_admin,
                    "created_at": user.created_at,
                    "two_fa_enabled": two_fa_enabled,
                    "avatar_seed": user.avatar_seed,
                    "avatar_url": format!("/api/users/{}/avatar.svg", user.username),
                    "must_change_password": user.must_change_password,
                });
                ResponseHelper::json_success(user_data)
            }
            Err(e) => ResponseHelper::json_error(format!("User not found: {}", e)),
        },
        Err(e) => ResponseHelper::json_error(format!("Session invalid: {}", e)),
    }
}

#[axum_macros::debug_handler]
/// Reports whether the initial setup has been completed and whether hardware changed.
///
/// "Configured" is derived from the database — the existence of an admin
/// account — never from a marker file, so the answer can never disagree with
/// the account table (see `storage/setup_state_repo.rs`).
pub async fn handle_setup_status(State(state): State<Arc<AppState>>) -> Response {
    let repo = SetupStateRepository::new(state.db.pool());

    let admin_exists = match repo.has_admin().await {
        Ok(exists) => exists,
        Err(e) => {
            tracing::error!("Failed to check for an admin account: {}", e);
            false
        }
    };
    let recorded = repo.get().await.ok().flatten();

    // A fingerprint only counts as "changed" when one was actually recorded at
    // setup time — a database that predates the `setup_state` table (migration
    // 037) but has an admin is not a hardware change, just a missing record.
    let hardware_changed = recorded
        .as_ref()
        .map(|s| !SetupService::system_hash_matches(&s.system_hash))
        .unwrap_or(false);

    if hardware_changed {
        tracing::warn!("Hardware change detected since setup");
    }

    ResponseHelper::json_success(SetupStatusResponse {
        is_first_run: !admin_exists,
        is_setup_valid: admin_exists && !hardware_changed,
        hardware_changed,
        needs_recovery: admin_exists && hardware_changed,
        server_machine_id: SetupService::get_machine_id(),
        sudo_available: SetupService::is_command_available("sudo"),
        system_language: SetupService::detect_system_language().to_string(),
    })
}

/// Reports whether a candidate administrator username is available for the
/// first-run setup. Validates the format, reserved/system names and existing
/// database or system accounts.
///
/// First-run-only, like the other bootstrap endpoints (403 once an admin
/// exists, mirroring `/auth/setup/verify-totp` and `/auth/setup/progress`):
/// after setup the wizard is gone and this endpoint would be a free username
/// oracle — an unauthenticated client could enumerate HomeVault accounts and
/// host system accounts, distinguishing "exists" from "does not exist". The
/// local-network + same-origin guard already limits who can ask; the
/// `has_admin()` gate removes the reason to ask at all.
pub async fn handle_check_username(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<CheckUsernameQuery>,
) -> Response {
    let setup_repo = SetupStateRepository::new(state.db.pool());
    if setup_repo.has_admin().await.unwrap_or(true) {
        return ResponseHelper::json_forbidden("Setup already complete".to_string());
    }

    let username = params.username.trim();

    if let Err(e) = SetupService::validate_username(username) {
        return ResponseHelper::json_success(serde_json::json!({
            "available": false,
            "reason": e,
        }));
    }

    // Already taken in the HomeVault database?
    let user_repo = crate::storage::UserRepository::new(state.db.pool());
    if let Ok(Some(_)) = user_repo.find_by_username(username).await {
        return ResponseHelper::json_success(serde_json::json!({
            "available": false,
            "reason": "Username already exists",
        }));
    }

    // Already an existing system account?
    if crate::auth::service::AuthService::is_system_user(username) {
        return ResponseHelper::json_success(serde_json::json!({
            "available": false,
            "reason": "Username already exists on this system",
        }));
    }

    ResponseHelper::json_success(serde_json::json!({
        "available": true,
        "reason": "",
    }))
}

/// Query parameters for [`handle_check_username`].
#[derive(Debug, Deserialize)]
pub struct CheckUsernameQuery {
    pub username: String,
}

/// Body for the pre-creation TOTP code check used by the Setup Wizard's
/// password step.
#[derive(Debug, Deserialize)]
pub struct SetupTotpVerifyRequest {
    /// Client-generated base32 TOTP secret (never stored by this endpoint).
    pub secret: String,
    /// The 6-digit code the admin entered from their authenticator app.
    pub code: String,
}

#[axum_macros::debug_handler]
/// Verifies a TOTP code against a client-generated secret during the first-run
/// Setup Wizard, before the admin account exists.
///
/// Stateless: nothing is persisted here, so this endpoint only answers
/// "is this code valid for this secret right now?" and gives an attacker with
/// a stolen secret nothing they could not compute offline themselves. Only
/// reachable while the server is still unconfigured (first run).
pub async fn handle_setup_verify_totp(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Json<SetupTotpVerifyRequest>,
) -> Response {
    let setup_repo = SetupStateRepository::new(state.db.pool());
    if setup_repo.has_admin().await.unwrap_or(true) {
        return ResponseHelper::json_forbidden("Setup already complete".to_string());
    }

    let valid =
        crate::auth::service::AuthService::verify_totp_code_with_secret(&req.secret, &req.code);
    ResponseHelper::json_success(serde_json::json!({ "valid": valid }))
}

/// Body for persisting first-run wizard progress.
#[derive(Debug, Deserialize)]
pub struct SetupProgressRequest {
    pub step: i64,
    /// Operator choices (currently just the interface language) as a JSON
    /// object. Never secrets — the password and the TOTP secret are
    /// intentionally not part of this payload (see
    /// `storage/setup_progress_repo.rs`).
    pub data: serde_json::Value,
}

/// Reports the persisted Setup Wizard progress so a reboot or a closed
/// browser resumes the onboarding where it left off.
///
/// First-run-only (403 once an admin exists): the stored choices describe an
/// unconfigured machine and stop being meaningful after setup.
pub async fn handle_setup_progress_get(State(state): State<Arc<AppState>>) -> Response {
    let setup_repo = SetupStateRepository::new(state.db.pool());
    if setup_repo.has_admin().await.unwrap_or(true) {
        return ResponseHelper::json_forbidden("Setup already complete".to_string());
    }
    let repo = SetupProgressRepository::new(state.db.pool());
    match repo.get().await {
        Ok(p) => {
            let data: serde_json::Value =
                serde_json::from_str(&p.data_json).unwrap_or(serde_json::json!({}));
            ResponseHelper::json_success(serde_json::json!({
                "step": p.step,
                "data": data,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to load setup progress: {}", e)),
    }
}

/// Persists the Setup Wizard's current step and choices (first-run-only).
pub async fn handle_setup_progress_put(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Json<SetupProgressRequest>,
) -> Response {
    let setup_repo = SetupStateRepository::new(state.db.pool());
    if setup_repo.has_admin().await.unwrap_or(true) {
        return ResponseHelper::json_forbidden("Setup already complete".to_string());
    }
    if !req.data.is_object() {
        return ResponseHelper::json_error("data must be a JSON object".to_string());
    }
    let repo = SetupProgressRepository::new(state.db.pool());
    let data_json = req.data.to_string();
    match repo.save(req.step, &data_json).await {
        Ok(p) => ResponseHelper::json_success(serde_json::json!({
            "step": p.step,
            "saved": true,
        })),
        Err(crate::storage::DbError::InvalidInput(msg)) => ResponseHelper::json_error(msg),
        Err(e) => ResponseHelper::json_error(format!("Failed to save setup progress: {}", e)),
    }
}

#[axum_macros::debug_handler]
/// Bootstraps the first admin user, creates the system group, and issues a session.
///
/// Only works while the app is in its initial unconfigured state.
pub async fn handle_setup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    over_tls: Option<axum::Extension<crate::middleware::ArrivedOverTls>>,
    req: axum::extract::Json<SetupRequest>,
) -> Response {
    let over_tls = over_tls.is_some();
    let setup_repo = SetupStateRepository::new(state.db.pool());

    // The database is the single source of truth for "already configured": a
    // lost marker file can no longer re-open setup while an admin exists, and
    // a surviving admin can no longer dead-end the wizard on "already
    // complete" after a hardware change.
    match setup_repo.has_admin().await {
        Ok(true) => return ResponseHelper::json_error("Setup already complete".to_string()),
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_internal_error(format!("Database error: {}", e)),
    }

    if let Err(e) = SetupService::mark_setup_in_progress() {
        return match e {
            crate::setup::SetupError::SetupInProgress => {
                ResponseHelper::json_error("Setup is already in progress".to_string())
            }
            _ => ResponseHelper::json_internal_error(format!("Setup lock failed: {}", e)),
        };
    }

    if let Err(e) = SetupService::validate_username(&req.username) {
        SetupService::cleanup_setup_lock();
        return ResponseHelper::json_error(e);
    }

    if let Err(e) = SetupService::validate_password(&req.password) {
        SetupService::cleanup_setup_lock();
        return ResponseHelper::json_error(e);
    }

    // Optional TOTP enrollment from the wizard's password step: the 6-digit
    // code must verify against the client-generated secret BEFORE any row is
    // created, so an invalid code is a clean rejection with nothing
    // persisted. Both fields are required together; a lone secret is ignored.
    if let (Some(secret), Some(code)) = (&req.totp_secret, &req.totp_code) {
        let secret = secret.trim();
        let code = code.trim();
        if secret.is_empty()
            || code.is_empty()
            || !crate::auth::service::AuthService::verify_totp_code_with_secret(secret, code)
        {
            SetupService::cleanup_setup_lock();
            return ResponseHelper::json_error("Invalid two-factor verification code".to_string());
        }
    }

    // Fullname is optional in the wizard; only validate it when provided.
    if let Some(fullname) = req.fullname.as_deref() {
        if let Err(e) = SetupService::validate_fullname(fullname) {
            SetupService::cleanup_setup_lock();
            return ResponseHelper::json_error(e);
        }
    }
    let _fullname = req.fullname.clone().unwrap_or_default();

    let avatar_seed = match req
        .avatar_seed
        .as_deref()
        .and_then(crate::avatars::normalize_seed)
    {
        Some(seed) => seed,
        None => crate::avatars::generate_random_seed(),
    };

    let password_hash = match state.auth.hash_password(&req.password) {
        Ok(h) => h,
        Err(_) => {
            SetupService::cleanup_setup_lock();
            return ResponseHelper::json_error("Failed to hash password".to_string());
        }
    };

    let mut tx = match state.db.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            SetupService::cleanup_setup_lock();
            return ResponseHelper::json_internal_error(format!("Database error: {}", e));
        }
    };

    let created_at = chrono::Utc::now().to_rfc3339();
    if let Err(e) = sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&req.username)
    .bind(&password_hash)
    .bind(req.fullname.as_deref().unwrap_or(""))
    .bind("") // Empty email
    .bind(ADMIN_ROLE_ID)
    .bind(false)
    .bind(&created_at)
    .bind(&avatar_seed)
    .execute(&mut *tx)
    .await
    {
        let _ = tx.rollback().await;
        SetupService::cleanup_setup_lock();

        if let sqlx::Error::Database(ref db_err) = e {
            if db_err.code().map(|c| c == "2067").unwrap_or(false) {
                return ResponseHelper::json_error("Username already exists".to_string());
            }
        }
        return ResponseHelper::json_error(format!("Failed to create user: {}", e));
    }

    if let Err(e) = state.db.ensure_admin_group().await {
        tracing::warn!(
            "Failed to ensure admin group: {}. Continuing without system group integration.",
            e
        );
    }

    if let Err(e) = state.db.sync_admin_user_to_group(&req.username).await {
        tracing::warn!(
            "Failed to sync admin user to group: {}. Continuing without system group integration.",
            e
        );
    }

    if let Err(e) = state.db.create_admin_home_directory(&req.username).await {
        tracing::warn!("Failed to create admin home directory: {}", e);
    }

    if let Err(e) = tx.commit().await {
        SetupService::cleanup_setup_lock();
        return ResponseHelper::json_internal_error(format!("Transaction commit failed: {}", e));
    }

    // Persist the hardware fingerprint in the same durable store as the
    // account. Best-effort: the admin row is already committed, so a failure
    // here (the fingerprint only exists to detect a moved/reflashed system)
    // must not fail an otherwise-complete setup.
    let system_hash = SetupService::generate_system_hash();
    let machine_id = SetupService::get_machine_id();
    if let Err(e) = setup_repo.mark_complete(&system_hash, &machine_id).await {
        tracing::warn!(
            "Failed to persist setup state (setup is still complete): {}",
            e
        );
    }
    SetupService::cleanup_setup_lock();

    // Enable 2FA with the secret the admin scanned at the password step (the
    // code was already verified above). Persists the encrypted secret and
    // returns the one-time backup codes. Best-effort: a failure here leaves
    // the account fully usable without 2FA — surfaced to the wizard via
    // `totp_enabled` so the admin is never silently left unprotected.
    let mut totp_enabled = false;
    let mut backup_codes: Vec<String> = Vec::new();
    if let Some(secret) = req.totp_secret.as_deref() {
        let secret = secret.trim();
        let code_present = req
            .totp_code
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
        if !secret.is_empty() && code_present {
            match state.auth.enable_2fa(&req.username, secret).await {
                Ok(codes) => {
                    crate::events::log_event(format!(
                        "Two-factor authentication enabled for '{}' during setup",
                        req.username
                    ));
                    totp_enabled = true;
                    backup_codes = codes;
                }
                Err(e) => tracing::error!(
                    username = %req.username,
                    error = %e,
                    "Setup: failed to enable 2FA at creation; account continues without it"
                ),
            }
        }
    }

    // No sudoers file to generate any more: HomeVault runs as root (see
    // the systemd unit installer/install.sh writes — no User=/Group=, and
    // install.sh no longer creates a "homevault" service account), so every
    // `sudo` call already in the codebase (RAID/mdadm, ZFS, VPN, Docker, useradd/
    // groupadd above, ...) already succeeds against the default Debian/
    // Ubuntu `root ALL=(ALL:ALL) ALL` sudoers rule — nothing left to grant.
    // `SetupService::configure_sudoers()` is kept in setup.rs (unused) as a
    // reference for anyone reviving a non-root deployment mode later.

    let ip_address = extract_client_ip(&headers);

    match state
        .auth
        .create_session(req.username.clone(), ip_address.clone(), false)
        .await
    {
        Ok(session) => {
            tracing::info!(
                username = %req.username,
                ip_address = %ip_address,
                "First setup completed - admin user created"
            );

            let token = session.token.clone();
            // Enriched LoginResponse: carries the optional TOTP enrolment
            // outcome so the wizard can show the one-time backup codes (or a
            // warning when the server could not persist 2FA).
            let data = serde_json::json!({
                "token": session.token,
                "csrf_token": session.csrf_token,
                "username": session.username,
                "is_admin": session.is_admin,
                // The setup password was chosen by its owner on this form.
                "must_change_password": false,
                "totp_enabled": totp_enabled,
                "backup_codes": backup_codes,
            });
            crate::app_proxy::cookie::attach_session(
                ResponseHelper::json_success(data),
                &token,
                false,
                over_tls,
            )
        }
        Err(e) => {
            tracing::error!("Setup complete but failed to create session: {}", e);
            ResponseHelper::json_internal_error(format!(
                "Setup complete but failed to create session: {}",
                e
            ))
        }
    }
}

#[axum_macros::debug_handler]
/// Re-baselines the recorded hardware fingerprint after a confirmed hardware
/// change (VM clone, hostname/OS change, restored database).
///
/// Admin-only: it does not grant anything new — it only stops
/// `/auth/setup-status` from reporting `needs_recovery` forever. The setup
/// endpoints stay closed because an admin already exists; this is the
/// documented recovery path that replaced the old dead-end where a changed
/// fingerprint re-showed the Wizard only for `/auth/setup` to reject it with
/// "Setup already complete".
pub async fn handle_setup_rebaseline(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(State(state.clone()), headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    let repo = SetupStateRepository::new(state.db.pool());
    let system_hash = SetupService::generate_system_hash();
    let machine_id = SetupService::get_machine_id();

    if let Err(e) = repo.update_system_hash(&system_hash, &machine_id).await {
        tracing::error!("Failed to re-baseline setup state: {}", e);
        return ResponseHelper::json_internal_error(
            "Failed to persist the hardware fingerprint".to_string(),
        );
    }

    crate::events::log_event(format!(
        "Hardware fingerprint re-baselined by '{}'",
        username
    ));

    ResponseHelper::json_success(serde_json::json!({
        "system_hash": system_hash,
        "machine_id": machine_id,
    }))
}

#[axum_macros::debug_handler]
/// Generates a numeric recovery code for the authenticated user.
///
/// Returns the plaintext code once; only its hash is stored. Refuses if a code
/// already exists unless `?regenerate=true` is passed.
pub async fn handle_generate_recovery_code(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<RecoveryCodeQuery>,
) -> Response {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let username = match state.auth.get_username_from_token(session_token).await {
        Some(u) => u,
        None => return ResponseHelper::json_unauthorized("Invalid session".to_string()),
    };

    if !params.regenerate {
        match state.auth.has_recovery_code(&username).await {
            Ok(true) => {
                return ResponseHelper::json_error(
                    "A recovery code is already set for this account.".to_string(),
                );
            }
            Ok(false) => {}
            Err(e) => {
                return ResponseHelper::json_internal_error(format!(
                    "Failed to check recovery code: {}",
                    e
                ));
            }
        }
    }

    match state.auth.set_recovery_code(&username).await {
        Ok(code) => {
            crate::events::log_event(format!(
                "User '{}' generated a password recovery code",
                username
            ));
            ResponseHelper::json_success(RecoveryCodeResponse {
                recovery_code: code,
            })
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to generate recovery code: {}", e)),
    }
}

#[axum_macros::debug_handler]
/// Checks a recovery code without changing anything, so the login UI can gate
/// the "choose a new password" step. Rate-limited via the same per-user counter
/// as the reset itself.
pub async fn handle_verify_recovery_code(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<VerifyRecoveryCodeRequest>,
) -> Response {
    let ip_address = extract_client_ip(&headers);
    let username = req.username.trim();

    match state
        .auth
        .verify_recovery_code(username, &req.recovery_code)
        .await
    {
        Ok(true) => ResponseHelper::json_success(serde_json::json!({ "valid": true })),
        Ok(false) => {
            crate::events::log_event_warn(format!(
                "Failed recovery-code verification for user '{}' from {}",
                username, ip_address
            ));
            ResponseHelper::json_error("The recovery code is incorrect.".to_string())
        }
        Err(AuthError::TooManyRecoveryAttempts) => ResponseHelper::json_error(
            "Too many attempts. Please wait a few minutes and try again.".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(format!("Verification failed: {}", e)),
    }
}

#[axum_macros::debug_handler]
/// Completes a password reset using a numeric recovery code (email-free flow).
///
/// Responses are deliberately generic so a caller cannot tell whether the
/// username exists or whether it was the code or the password that was rejected
/// (beyond password-policy feedback).
pub async fn handle_reset_password_recovery(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    req: axum::extract::Json<ResetPasswordRecoveryRequest>,
) -> Response {
    let ip_address = extract_client_ip(&headers);
    let username = req.username.trim();

    match state
        .auth
        .reset_password_with_recovery_code(username, &req.recovery_code, &req.new_password)
        .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Password for user '{}' reset via recovery code from {}",
                username, ip_address
            ));
            ResponseHelper::json_success(ResetPasswordResponse {
                message: "Password successfully reset. You can now log in with your new password."
                    .to_string(),
            })
        }
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Failed recovery-code password reset for user '{}' from {}",
                username, ip_address
            ));
            match e {
                AuthError::WeakPassword => ResponseHelper::json_error(
                    "Password does not meet security requirements. It must be at least 12 characters long, contain uppercase, lowercase, and numbers, and not be a commonly used password.".to_string(),
                ),
                AuthError::PasswordReused => ResponseHelper::json_error(
                    "This password was used recently. Please choose a different password.".to_string(),
                ),
                AuthError::TooManyRecoveryAttempts => ResponseHelper::json_error(
                    "Too many attempts. Please wait a few minutes and try again.".to_string(),
                ),
                // InvalidRecoveryCode, UserNotFound, and anything else collapse
                // to one generic message.
                _ => ResponseHelper::json_error(
                    "The recovery code is incorrect.".to_string(),
                ),
            }
        }
    }
}

#[cfg(test)]
mod client_ip_tests {
    use super::client_ip;
    use axum::http::{HeaderMap, HeaderValue};
    use std::net::IpAddr;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// The common case: a LAN client talking to HomeVault directly. A forged
    /// `X-Forwarded-For` must not be believed.
    #[test]
    fn test_direct_peer_wins_over_a_forged_forwarded_header() {
        let h = headers(&[("x-forwarded-for", "1.2.3.4"), ("x-real-ip", "5.6.7.8")]);
        assert_eq!(client_ip(&h, Some(ip("192.168.1.50"))), "192.168.1.50");
    }

    /// A reverse proxy on the same host is the one case where the header is
    /// authoritative.
    #[test]
    fn test_loopback_peer_honours_the_forwarded_header() {
        let h = headers(&[("x-forwarded-for", "203.0.113.7, 10.0.0.1")]);
        assert_eq!(client_ip(&h, Some(ip("127.0.0.1"))), "203.0.113.7");
    }

    #[test]
    fn test_loopback_peer_without_a_header_reports_loopback() {
        let h = HeaderMap::new();
        assert_eq!(client_ip(&h, Some(ip("::1"))), "::1");
    }

    #[test]
    fn test_forwarded_header_is_ignored_for_a_non_loopback_peer() {
        let h = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert_eq!(client_ip(&h, Some(ip("10.0.0.9"))), "10.0.0.9");
    }

    /// In-process calls (tests, `oneshot`) have no peer metadata: the header is
    /// the only signal, then `unknown`.
    #[test]
    fn test_missing_peer_falls_back_to_the_header_then_unknown() {
        let h = headers(&[("x-real-ip", "198.51.100.4")]);
        assert_eq!(client_ip(&h, None), "198.51.100.4");
        assert_eq!(client_ip(&HeaderMap::new(), None), "unknown");
    }

    #[test]
    fn test_blank_forwarded_values_are_ignored() {
        let h = headers(&[("x-forwarded-for", "  ")]);
        assert_eq!(client_ip(&h, Some(ip("127.0.0.1"))), "127.0.0.1");
    }
}
