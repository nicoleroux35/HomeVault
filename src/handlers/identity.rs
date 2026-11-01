/* =============================================================================
 * File          : identity.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Identity endpoints: list and revoke the browsers trusted to skip the second
 *   factor, and manage the rescue address that can receive account-recovery
 *   mail. Two routes in here are public by necessity (a locked-out user cannot
 *   sign in first) and therefore answer identically whether or not the account
 *   exists.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::identity, crate::email
 *
 * Usage:
 *   Routes in src/web/mod.rs; see docs/guides/identity.md.
 * =============================================================================
 */

//! Trusted devices and rescue e-mail (see docs/guides/identity.md).
//!
//! | Route                                          | Guard  |
//! |------------------------------------------------|--------|
//! | `GET    /api/identity/devices`                 | signed |
//! | `POST   /api/identity/devices/revoke`          | signed |
//! | `POST   /api/identity/devices/revoke-all`      | signed |
//! | `GET    /api/users/{username}/trusted-devices` | admin  |
//! | `POST   /api/users/{username}/trusted-devices/revoke` | admin |
//! | `DELETE /api/users/{username}/trusted-devices` | admin  |
//! | `GET    /api/identity/rescue-email`            | signed |
//! | `POST   /api/identity/rescue-email`            | signed |
//! | `POST   /api/identity/rescue-email/verify`     | signed |
//! | `DELETE /api/identity/rescue-email`            | signed |
//! | `POST   /api/auth/recover`                     | public |
//! | `POST   /api/auth/recover/verify`              | public |
//!
//! The signed routes act on the caller's own account only: the username comes
//! from the session, never from the body, so there is no id to tamper with.
//! The `admin` routes exist for the one case self-service cannot cover — a
//! user who lost the device that carried their second factor and cannot sign
//! in to revoke it. They are admin-only and always name the target account
//! explicitly, so an admin never acts on "whoever the session belongs to".

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::Response,
    Json,
};
use serde::Deserialize;

use crate::handlers::admin_guard::require_auth;
use crate::identity::{self, CodeCheck, CodePurpose, DeviceStore};
use crate::utils::ResponseHelper;

use super::AppState;

/// The most recent recovery-mail request per account. A public endpoint that
/// sends mail must not be usable to flood a mailbox, and this is the smallest
/// thing that prevents it: one message per account per window.
pub const RECOVERY_MAIL_WINDOW: Duration = Duration::from_secs(60);

fn throttle() -> &'static Mutex<HashMap<String, Instant>> {
    static THROTTLE: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    THROTTLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether a recovery mail may be sent for `username` right now; records the
/// attempt when it may. Keyed by the *submitted* name so a wrong guess and a
/// right one are throttled the same way (the timing must not be a signal).
fn take_mail_slot(username: &str) -> bool {
    let now = Instant::now();
    let mut map = throttle()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Keep the map bounded: anything older than the window is irrelevant.
    map.retain(|_, at| now.duration_since(*at) < RECOVERY_MAIL_WINDOW);
    match map.get(username) {
        Some(last) if now.duration_since(*last) < RECOVERY_MAIL_WINDOW => false,
        _ => {
            map.insert(username.to_string(), now);
            true
        }
    }
}

/// Drop the throttle entry, so a successful send does not block the next
/// deliberate one inside a test or after an operator cleared it.
#[cfg(test)]
fn clear_mail_slots() {
    throttle()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

// --------------------------------------------------------------------------
// Mail delivery
// --------------------------------------------------------------------------

/// Send one plain-text message with the configured relay. Blocking, so it must
/// run inside `spawn_blocking`: sending shells out to an MTA.
fn send_plain(to: &str, subject: &str, body: &str) -> Result<(), String> {
    let settings = crate::handlers::settings::EMAIL_SETTINGS.read().clone();
    let config = crate::email::EmailConfig::from(&settings);
    let message = crate::email::EmailMessage {
        to: vec![crate::email::EmailRecipient {
            email: to.to_string(),
            name: None,
        }],
        subject: subject.to_string(),
        body: body.to_string(),
        is_html: false,
    };
    crate::email::EmailService::send_email(&message, &config).map_err(|e| e.to_string())
}

async fn deliver(to: &str, subject: &str, body: &str) -> Result<(), String> {
    let (to, subject, body) = (to.to_string(), subject.to_string(), body.to_string());
    match tokio::task::spawn_blocking(move || send_plain(&to, &subject, &body)).await {
        Ok(result) => result,
        Err(e) => Err(format!("the mail worker stopped: {e}")),
    }
}

fn code_mail_body(code: &str, purpose: CodePurpose) -> String {
    match purpose {
        CodePurpose::VerifyEmail => format!(
            "Your HomeVault verification code is {code}.\n\n\
             Enter it in Settings → Account → Rescue e-mail within {} minutes. \
             If you did not ask for this, ignore this message; nothing changes.\n",
            identity::CODE_TTL_MINUTES
        ),
        CodePurpose::RecoverAccount => format!(
            "Your HomeVault account recovery code is {code}.\n\n\
             Enter it on the recovery page within {} minutes. It can be used \
             once. If you did not ask for this, someone typed your account name \
             into the recovery form — your password is unchanged.\n",
            identity::CODE_TTL_MINUTES
        ),
    }
}

// --------------------------------------------------------------------------
// Trusted devices
// --------------------------------------------------------------------------

/// `GET /api/identity/devices`
pub async fn handle_devices_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match DeviceStore::new(&state.db.pool()).list(&username).await {
        Ok(devices) => ResponseHelper::json_success(serde_json::json!({
            "devices": devices,
            "total": devices.len(),
            "lifetime_days": identity::DEVICE_LIFETIME_DAYS,
        })),
        Err(e) => ResponseHelper::json_internal_error(format!("cannot list devices: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct DeviceRevokeRequest {
    pub id: String,
}

/// `POST /api/identity/devices/revoke`
pub async fn handle_device_revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DeviceRevokeRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match DeviceStore::new(&state.db.pool())
        .revoke(&username, req.id.trim())
        .await
    {
        Ok(true) => {
            crate::events::log_event(format!("User '{username}' revoked a trusted device"));
            ResponseHelper::json_success(serde_json::json!({ "success": true }))
        }
        // An id that is not the caller's is a miss, not an error: answering
        // "not found" for someone else's id would confirm it exists.
        Ok(false) => ResponseHelper::json_error("no such trusted device".to_string()),
        Err(e) => ResponseHelper::json_internal_error(format!("cannot revoke the device: {e}")),
    }
}

/// `POST /api/identity/devices/revoke-all`
pub async fn handle_devices_revoke_all(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match DeviceStore::new(&state.db.pool())
        .revoke_all(&username)
        .await
    {
        Ok(count) => {
            if count > 0 {
                crate::events::log_event(format!(
                    "User '{username}' revoked all {count} trusted devices"
                ));
            }
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "revoked": count,
            }))
        }
        Err(e) => ResponseHelper::json_internal_error(format!("cannot revoke devices: {e}")),
    }
}

// --------------------------------------------------------------------------
// Trusted devices, admin view (lost-device recovery)
// --------------------------------------------------------------------------

/// `GET /api/users/{username}/trusted-devices`
///
/// An admin sees which browsers skip the second factor for another account.
/// Read-only: the token hash is never part of the payload, and the device
/// labels are all the operator needs to tell a laptop from a phone.
pub async fn handle_admin_user_devices(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(username): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    match DeviceStore::new(&state.db.pool()).list(&username).await {
        Ok(devices) => ResponseHelper::json_success(serde_json::json!({
            "devices": devices,
            "total": devices.len(),
            "lifetime_days": identity::DEVICE_LIFETIME_DAYS,
        })),
        Err(e) => ResponseHelper::json_internal_error(format!("cannot list devices: {e}")),
    }
}

/// `POST /api/users/{username}/trusted-devices/revoke` — body `{ "id": "…" }`.
///
/// The device is resolved against the *target* account, so an id from any
/// other account is a miss rather than a cross-account delete.
pub async fn handle_admin_device_revoke(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(username): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(req): Json<DeviceRevokeRequest>,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match DeviceStore::new(&state.db.pool())
        .revoke(&username, req.id.trim())
        .await
    {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{admin}' revoked a trusted device of user '{username}'"
            ));
            ResponseHelper::json_success(serde_json::json!({ "success": true }))
        }
        Ok(false) => ResponseHelper::json_error("no such trusted device".to_string()),
        Err(e) => ResponseHelper::json_internal_error(format!("cannot revoke the device: {e}")),
    }
}

/// `DELETE /api/users/{username}/trusted-devices` — revoke every device.
///
/// The recovery path for "I lost my phone and cannot sign in to revoke it":
/// an admin clears the trust, the user signs in with the second factor they
/// still have (or a recovery code) and enrols a new device.
pub async fn handle_admin_devices_revoke_all(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(username): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match DeviceStore::new(&state.db.pool()).revoke_all(&username).await {
        Ok(count) => {
            if count > 0 {
                crate::events::log_event(format!(
                    "Admin '{admin}' revoked all {count} trusted device(s) of user '{username}'"
                ));
            }
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "revoked": count,
            }))
        }
        Err(e) => ResponseHelper::json_internal_error(format!("cannot revoke devices: {e}")),
    }
}

// --------------------------------------------------------------------------
// Rescue e-mail (signed in)
// --------------------------------------------------------------------------

async fn rescue_payload(pool: &sqlx::SqlitePool, username: &str) -> serde_json::Value {
    let address = identity::rescue_address(pool, username)
        .await
        .ok()
        .flatten();
    serde_json::json!({
        "email": address,
        "configured": address.is_some(),
        "code_ttl_minutes": identity::CODE_TTL_MINUTES,
    })
}

/// `GET /api/identity/rescue-email`
pub async fn handle_rescue_email_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    ResponseHelper::json_success(rescue_payload(&state.db.pool(), &username).await)
}

#[derive(Debug, Deserialize)]
pub struct RescueEmailRequest {
    pub email: String,
}

/// `POST /api/identity/rescue-email`
///
/// Starts verification: the address is *not* stored as usable until the code
/// sent to it comes back. Saving it optimistically would let a typo hand
/// account recovery to a stranger.
pub async fn handle_rescue_email_set(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RescueEmailRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let email = match identity::validate_rescue_email(&req.email) {
        Ok(e) => e,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let code = match identity::issue_code(
        &state.db.pool(),
        &username,
        &email,
        CodePurpose::VerifyEmail,
    )
    .await
    {
        Ok(code) => code,
        Err(e) => return ResponseHelper::json_internal_error(format!("cannot issue a code: {e}")),
    };

    if let Err(e) = deliver(
        &email,
        "HomeVault: confirm your rescue e-mail address",
        &code_mail_body(&code, CodePurpose::VerifyEmail),
    )
    .await
    {
        // The pending code is useless without the mail, so drop it rather than
        // leave a stale row that the next attempt would overwrite anyway.
        let _ = sqlx::query("DELETE FROM rescue_email_codes WHERE username = ?")
            .bind(&username)
            .execute(&*state.db.pool())
            .await;
        return ResponseHelper::json_error(format!(
            "the code could not be sent: {e}. Check the e-mail settings under Notifications."
        ));
    }

    crate::events::log_event(format!(
        "User '{username}' asked to verify a rescue e-mail address"
    ));
    ResponseHelper::json_success(serde_json::json!({
        "sent": true,
        "email": email,
        "code_ttl_minutes": identity::CODE_TTL_MINUTES,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RescueCodeRequest {
    pub code: String,
}

/// `POST /api/identity/rescue-email/verify`
pub async fn handle_rescue_email_verify(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RescueCodeRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match identity::check_code(
        &state.db.pool(),
        &username,
        CodePurpose::VerifyEmail,
        &req.code,
    )
    .await
    {
        Ok(CodeCheck::Ok(email)) => {
            if let Err(e) = identity::set_rescue_address(&state.db.pool(), &username, &email).await
            {
                return ResponseHelper::json_internal_error(format!(
                    "cannot store the rescue address: {e}"
                ));
            }
            crate::events::log_event(format!(
                "User '{username}' verified a rescue e-mail address"
            ));
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "email": email,
            }))
        }
        Ok(CodeCheck::Wrong) => ResponseHelper::json_error("that code is not correct".to_string()),
        Ok(CodeCheck::Expired) => {
            ResponseHelper::json_error("that code has expired; ask for a new one".to_string())
        }
        Ok(CodeCheck::TooManyAttempts) => {
            ResponseHelper::json_error("too many wrong attempts; ask for a new code".to_string())
        }
        Ok(CodeCheck::Missing) => {
            ResponseHelper::json_error("no code is waiting to be confirmed".to_string())
        }
        Err(e) => ResponseHelper::json_internal_error(format!("cannot check the code: {e}")),
    }
}

/// `DELETE /api/identity/rescue-email`
pub async fn handle_rescue_email_clear(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    if let Err(e) = identity::clear_rescue_address(&state.db.pool(), &username).await {
        return ResponseHelper::json_internal_error(format!(
            "cannot clear the rescue address: {e}"
        ));
    }
    crate::events::log_event(format!(
        "User '{username}' removed their rescue e-mail address"
    ));
    ResponseHelper::json_success(serde_json::json!({ "success": true }))
}

// --------------------------------------------------------------------------
// Account recovery (public)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RecoverRequest {
    pub username: String,
}

/// `POST /api/auth/recover`
///
/// Sends a code to the account's verified rescue address. The answer is the
/// same in every case — unknown account, no rescue address, throttled, or a
/// code really sent — because this endpoint is public and must not tell an
/// attacker which names exist.
pub async fn handle_recover_request(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RecoverRequest>,
) -> Response {
    let username = req.username.trim().to_string();
    let generic = serde_json::json!({
        "success": true,
        "message": "If that account has a verified rescue address, a code is on its way.",
    });

    if username.is_empty() || !take_mail_slot(&username.to_ascii_lowercase()) {
        return ResponseHelper::json_success(generic);
    }

    // Only a verified address is usable; a pending one belongs to whoever typed
    // it last, which is not necessarily the owner.
    let address = identity::rescue_address(&state.db.pool(), &username)
        .await
        .ok()
        .flatten();
    let Some(address) = address else {
        return ResponseHelper::json_success(generic);
    };

    let code = match identity::issue_code(
        &state.db.pool(),
        &username,
        &address,
        CodePurpose::RecoverAccount,
    )
    .await
    {
        Ok(code) => code,
        Err(e) => {
            tracing::warn!("cannot issue a recovery code for '{username}': {e}");
            return ResponseHelper::json_success(generic);
        }
    };
    if let Err(e) = deliver(
        &address,
        "HomeVault: your account recovery code",
        &code_mail_body(&code, CodePurpose::RecoverAccount),
    )
    .await
    {
        tracing::warn!("cannot send the recovery mail for '{username}': {e}");
    }
    crate::events::log_event(format!(
        "Account recovery was requested for user '{username}'"
    ));
    ResponseHelper::json_success(generic)
}

#[derive(Debug, Deserialize)]
pub struct RecoverVerifyRequest {
    pub username: String,
    pub code: String,
}

/// `POST /api/auth/recover/verify`
///
/// A correct code proves control of the rescue mailbox, so it is exchanged for
/// a *fresh offline recovery code* — the same code the existing reset screen
/// already consumes. That keeps one password-reset path, with one password
/// policy and one attempt limit, instead of a second weaker one.
pub async fn handle_recover_verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RecoverVerifyRequest>,
) -> Response {
    let username = req.username.trim().to_string();
    if username.is_empty() {
        return ResponseHelper::json_error("that recovery code is not valid".to_string());
    }
    match identity::check_code(
        &state.db.pool(),
        &username,
        CodePurpose::RecoverAccount,
        &req.code,
    )
    .await
    {
        Ok(CodeCheck::Ok(_)) => match state.auth.set_recovery_code(&username).await {
            Ok(recovery_code) => {
                crate::events::log_event(format!(
                    "User '{username}' recovered their account through the rescue e-mail"
                ));
                ResponseHelper::json_success(serde_json::json!({
                    "success": true,
                    "username": username,
                    "recovery_code": recovery_code,
                }))
            }
            Err(e) => {
                ResponseHelper::json_internal_error(format!("cannot issue a recovery code: {e}"))
            }
        },
        // Every failure is one message: the code is short, so "wrong" and
        // "expired" are not worth telling an attacker apart.
        Ok(_) => ResponseHelper::json_error("that recovery code is not valid".to_string()),
        Err(e) => ResponseHelper::json_internal_error(format!("cannot check the code: {e}")),
    }
}

// --------------------------------------------------------------------------
// Trusted-device cookie (used by handlers/auth)
// --------------------------------------------------------------------------

/// Read the trusted-device token the browser sent, when it sent one.
pub fn device_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == identity::DEVICE_COOKIE)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Whether `username` may skip the second factor from this request. A device
/// belonging to somebody else is ignored — the cookie is not a credential for
/// any account but its own.
pub async fn may_skip_second_factor(
    pool: &sqlx::SqlitePool,
    username: &str,
    headers: &HeaderMap,
) -> bool {
    let Some(token) = device_cookie(headers) else {
        return false;
    };
    match DeviceStore::new(pool).find_valid(&token).await {
        Ok(Some(device)) if device.username == username => {
            DeviceStore::new(pool).touch(&device.id).await;
            true
        }
        _ => false,
    }
}

/// Trust this browser for [`identity::DEVICE_LIFETIME_DAYS`] and attach the
/// cookie. Best effort: failing to remember a device must not fail a sign-in
/// that already succeeded.
pub async fn trust_this_browser(
    pool: &sqlx::SqlitePool,
    username: &str,
    user_agent: Option<&str>,
    over_tls: bool,
    response: Response,
) -> Response {
    let label = identity::device_label(user_agent);
    let Ok((_device, token)) = DeviceStore::new(pool).create(username, &label).await else {
        return response;
    };
    attach_device_cookie(response, &token, over_tls)
}

/// `Set-Cookie: hv_trusted_device=…`, HttpOnly and SameSite=Strict: the API
/// never reads it as a session, only the 2FA gate does, and it must not travel
/// with a cross-site navigation.
pub fn attach_device_cookie(mut response: Response, token: &str, over_tls: bool) -> Response {
    let secure = if over_tls { "; Secure" } else { "" };
    let max_age = identity::DEVICE_LIFETIME_DAYS * 24 * 60 * 60;
    let cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict{secure}; Max-Age={max_age}",
        identity::DEVICE_COOKIE
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// Forget the browser entirely (revoke-all already deleted the row).
pub fn clear_device_cookie(mut response: Response) -> Response {
    let cookie = format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        identity::DEVICE_COOKIE
    );
    if let Ok(value) = axum::http::HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(
            header::COOKIE,
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
        map
    }

    #[test]
    fn test_device_cookie_is_found_among_others() {
        let headers = headers_with_cookie("theme=dark; hv_trusted_device=abc123; lang=fr");
        assert_eq!(device_cookie(&headers).as_deref(), Some("abc123"));
    }

    #[test]
    fn test_a_missing_or_empty_device_cookie_is_none() {
        assert_eq!(device_cookie(&HeaderMap::new()), None);
        assert_eq!(
            device_cookie(&headers_with_cookie("hv_trusted_device=")),
            None
        );
        assert_eq!(device_cookie(&headers_with_cookie("theme=dark")), None);
    }

    #[test]
    fn test_the_device_cookie_is_http_only_strict_and_bounded() {
        let resp = clear_device_cookie(attach_device_cookie(
            Response::new(axum::body::Body::empty()),
            "tok",
            true,
        ));
        let cookies: Vec<&str> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        assert_eq!(cookies.len(), 2);
        assert!(cookies[0].starts_with("hv_trusted_device=tok;"));
        assert!(cookies[0].contains("HttpOnly"));
        assert!(cookies[0].contains("SameSite=Strict"));
        assert!(cookies[0].contains("Secure"));
        // 30 days, in seconds.
        assert!(cookies[0].contains(&format!("Max-Age={}", 30 * 24 * 3600)));
        assert!(cookies[1].contains("Max-Age=0"));
    }

    #[test]
    fn test_no_secure_flag_over_plain_http() {
        let resp = attach_device_cookie(Response::new(axum::body::Body::empty()), "tok", false);
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap();
        assert!(!cookie.contains("Secure"));
    }

    #[test]
    fn test_recovery_mail_is_throttled_per_account() {
        clear_mail_slots();
        assert!(take_mail_slot("alice"), "the first request sends");
        assert!(
            !take_mail_slot("alice"),
            "a second within the window does not"
        );
        assert!(take_mail_slot("bob"), "another account is unaffected");
        clear_mail_slots();
    }

    #[test]
    fn test_codes_mail_body_names_the_code_and_the_purpose() {
        let verify = code_mail_body("123456", CodePurpose::VerifyEmail);
        assert!(verify.contains("123456"));
        assert!(verify.contains("Rescue e-mail") || verify.contains("verification"));
        let recover = code_mail_body("654321", CodePurpose::RecoverAccount);
        assert!(recover.contains("654321"));
        assert!(recover.contains("recovery"));
        assert!(
            recover.contains("unchanged"),
            "a recovery mail must reassure a user who did not ask for it"
        );
    }
}
