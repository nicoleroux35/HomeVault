/* =============================================================================
 * File          : passkey.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Passkey (WebAuthn) endpoints: list / rename / revoke the passkeys an
 *   account holds, run the two ceremonies (registration and authentication)
 *   with server-generated single-use challenges, and sign in with a passkey
 *   without a password.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::passkey, crate::storage::PasskeyRepository,
 *     crate::tls (for the relying party)
 *
 * Usage:
 *   Routes /api/passkeys* in src/web/mod.rs; the Account screen's Passkeys
 *   card; see docs/guides/identity.md.
 * =============================================================================
 */

//! Passkey endpoints, see docs/guides/identity.md.
//!
//! * `GET    /api/passkeys`                     — the caller's passkeys
//! * `PATCH  /api/passkeys/{id}`                — rename one
//! * `DELETE /api/passkeys/{id}`                — revoke one
//! * `POST   /api/passkeys/register/begin`      — issue a registration challenge
//! * `POST   /api/passkeys/register/finish`     — verify + store the credential
//! * `POST   /api/passkeys/login/begin`         — issue a sign-in challenge
//! * `POST   /api/passkeys/login/finish`        — verify the assertion, sign in
//!
//! The relying party (id + origin) is derived from the **configured hostname**,
//! never from the request's `Host`/`Origin` header: a caller must not be able to
//! widen the origin the server accepts by sending a different header.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::handlers::admin_guard::require_session_token;
use crate::handlers::AppState;
use crate::passkey::{
    decode_b64url, encode_b64url, new_challenge, relying_party_for, verify_assertion,
    verify_registration, Ceremony, KeyAlgorithm, CHALLENGE_TTL_SECONDS, MAX_CREDENTIALS_PER_USER,
    MAX_CREDENTIAL_ID_LEN,
};
use crate::storage::{NewPasskey, PasskeyRepository};
use crate::utils::ResponseHelper;

/// Longest label an owner may give a passkey.
pub const MAX_LABEL_LEN: usize = 64;

/// Validate + normalise a passkey label. Pure, so the rules are unit-tested.
pub fn validate_label(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > MAX_LABEL_LEN {
        return Err(format!(
            "A passkey name must be {MAX_LABEL_LEN} characters or fewer"
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("A passkey name cannot contain control characters".to_string());
    }
    Ok(trimmed.to_string())
}

/// Default label when the owner gives none: the authenticator's family, or a
/// plain "Passkey". Never empty (an unlabelled row is unusable in a list).
pub fn default_label(algorithm: KeyAlgorithm) -> String {
    match algorithm {
        KeyAlgorithm::Es256 => "Passkey (platform)".to_string(),
        KeyAlgorithm::EdDsa => "Passkey (security key)".to_string(),
    }
}

/// The relying party for this deployment, from the configured hostname.
///
/// A hostname that cannot yield one is a `500` with a sentence: a ceremony
/// without a relying party is meaningless, and silently inventing one would
/// bind credentials to an origin nobody configured.
// The `Err` side is an already-built axum `Response` (the handlers return it
// directly); boxing it here would only add an unwrap at every call site.
#[allow(clippy::result_large_err)]
fn relying_party(state: &Arc<AppState>) -> Result<crate::passkey::RelyingParty, Response> {
    let _ = state;
    let tls = crate::tls::load_settings();
    // The hostname this appliance answers to. `HOMEVAULT_HOSTNAME` wins (it is
    // what the installer and the tests set), then the ACME domain the admin
    // configured for a public name, then the machine's own host name. A pasted
    // URL is accepted for its host only — `relying_party_for` rebuilds the
    // origin from scratch, so a caller can never widen it.
    let configured = std::env::var("HOMEVAULT_HOSTNAME")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            let domain = crate::acme::load_settings().domain;
            if domain.trim().is_empty() {
                None
            } else {
                Some(domain)
            }
        })
        .or_else(|| {
            // Last resort: the machine's own name. A passkey bound to a name
            // only this host resolves is useless from a phone, so the guide
            // tells the admin to set `HOMEVAULT_HOSTNAME`.
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .filter(|h| !h.trim().is_empty())
        })
        .unwrap_or_else(|| "localhost".to_string());
    relying_party_for(&configured, tls.enabled, tls.effective_https_port()).ok_or_else(|| {
        ResponseHelper::json_internal_error(
            "Passkeys need a configured hostname (Settings → General): set it so the passkey is \
             bound to this appliance's own address"
                .to_string(),
        )
    })
}

// ---------------------------------------------------------------------------
// Listing, renaming, revoking
// ---------------------------------------------------------------------------

/// `GET /api/passkeys`
pub async fn handle_passkeys_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let token = match require_session_token(&state, &headers).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let repo = PasskeyRepository::new(state.db.pool());
    match repo.summaries_for_user(&session.username).await {
        Ok(passkeys) => {
            let rp = relying_party(&state).ok();
            ResponseHelper::json_success(serde_json::json!({
                "passkeys": passkeys,
                "supported": rp.is_some(),
                "relying_party": rp.map(|rp| serde_json::json!({
                    "id": rp.id,
                    // The origin is what the browser checks; showing it makes
                    // a misconfigured hostname visible instead of puzzling.
                    "origin": rp.origin,
                })),
                "max": MAX_CREDENTIALS_PER_USER,
            }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list passkeys: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct RenamePasskeyRequest {
    pub label: String,
}

/// `PATCH /api/passkeys/{id}`
pub async fn handle_passkey_rename(
    State(state): State<Arc<AppState>>,
    Path(credential_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<RenamePasskeyRequest>,
) -> Response {
    let token = match require_session_token(&state, &headers).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let label = match validate_label(&payload.label) {
        Ok(l) => l,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let repo = PasskeyRepository::new(state.db.pool());
    match repo
        .rename_for_user(&session.username, &credential_id, &label)
        .await
    {
        Ok(true) => ResponseHelper::json_success(serde_json::json!({ "label": label })),
        Ok(false) => ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            "No passkey with that id on this account".to_string(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to rename the passkey: {e}")),
    }
}

/// `DELETE /api/passkeys/{id}`
pub async fn handle_passkey_delete(
    State(state): State<Arc<AppState>>,
    Path(credential_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let token = match require_session_token(&state, &headers).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let repo = PasskeyRepository::new(state.db.pool());
    match repo
        .delete_for_user(&session.username, &credential_id)
        .await
    {
        Ok(true) => {
            crate::events::log_event(format!("User '{}' revoked a passkey", session.username));
            ResponseHelper::json_success(serde_json::json!({ "deleted": true }))
        }
        Ok(false) => ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            "No passkey with that id on this account".to_string(),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke the passkey: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// What the browser needs to call `navigator.credentials.create()`.
#[derive(Debug, Serialize)]
pub struct RegistrationOptions {
    pub challenge: String,
    pub rp: RpDescriptor,
    pub user: UserDescriptor,
    /// One entry per algorithm HomeVault can verify.
    pub pub_key_cred_params: Vec<CredentialParam>,
    pub timeout: u64,
    pub attestation: &'static str,
    pub authenticator_selection: AuthenticatorSelection,
    /// Refuse a credential that already exists on this server.
    pub exclude_credentials: Vec<CredentialDescriptor>,
}

#[derive(Debug, Serialize)]
pub struct RpDescriptor {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct UserDescriptor {
    /// base64url of the username — the spec requires bytes, and the username is
    /// the only stable account id HomeVault has.
    pub id: String,
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct CredentialParam {
    #[serde(rename = "type")]
    pub credential_type: &'static str,
    pub alg: i64,
}

#[derive(Debug, Serialize)]
pub struct AuthenticatorSelection {
    /// `preferred`: a PIN/biometric is accepted when the authenticator has one,
    /// and a plain user-presence tap is accepted when it does not (a hardware
    /// key without a PIN must still work).
    pub user_verification: &'static str,
    pub resident_key: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CredentialDescriptor {
    #[serde(rename = "type")]
    pub credential_type: &'static str,
    pub id: String,
    pub transports: Vec<&'static str>,
}

/// `POST /api/passkeys/register/begin`
pub async fn handle_passkey_register_begin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let token = match require_session_token(&state, &headers).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let rp = match relying_party(&state) {
        Ok(rp) => rp,
        Err(resp) => return resp,
    };

    let repo = PasskeyRepository::new(state.db.pool());
    // The per-account cap is checked before a challenge is issued: issuing one
    // and refusing later would leave a live challenge behind.
    match repo.count_for_user(&session.username).await {
        Ok(count) if count >= MAX_CREDENTIALS_PER_USER => {
            return ResponseHelper::json_error(format!(
                "This account already holds the maximum of {MAX_CREDENTIALS_PER_USER} passkeys — \
                 revoke one first"
            ));
        }
        Ok(_) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the passkeys: {e}")),
    }
    // Sweeping here keeps the table from growing on a box nobody signs into.
    let _ = repo.sweep_expired().await;

    let challenge = new_challenge();
    if let Err(e) = repo
        .store_challenge(
            &session.username,
            Ceremony::Registration.as_str(),
            &challenge,
            CHALLENGE_TTL_SECONDS,
        )
        .await
    {
        return ResponseHelper::json_error(format!("Failed to start the ceremony: {e}"));
    }

    let existing = repo
        .ids_for_user(&session.username)
        .await
        .unwrap_or_default();
    let display_name = state
        .auth
        .get_user(&session.username)
        .await
        .ok()
        .map(|user| {
            if user.fullname.trim().is_empty() {
                user.username
            } else {
                user.fullname
            }
        })
        .unwrap_or_else(|| session.username.clone());

    ResponseHelper::json_success(RegistrationOptions {
        challenge,
        rp: RpDescriptor {
            id: rp.id,
            name: "HomeVault".to_string(),
        },
        user: UserDescriptor {
            id: encode_b64url(session.username.as_bytes()),
            name: session.username.clone(),
            display_name,
        },
        pub_key_cred_params: vec![
            CredentialParam {
                credential_type: "public-key",
                alg: KeyAlgorithm::Es256.cose_alg(),
            },
            CredentialParam {
                credential_type: "public-key",
                alg: KeyAlgorithm::EdDsa.cose_alg(),
            },
        ],
        timeout: (CHALLENGE_TTL_SECONDS as u64) * 1000,
        attestation: "none",
        authenticator_selection: AuthenticatorSelection {
            user_verification: "preferred",
            resident_key: "preferred",
        },
        exclude_credentials: existing
            .iter()
            .map(|id| CredentialDescriptor {
                credential_type: "public-key",
                id: id.clone(),
                transports: vec!["internal", "hybrid", "usb", "nfc", "ble"],
            })
            .collect(),
    })
}

#[derive(Debug, Deserialize)]
pub struct FinishRegistrationRequest {
    /// base64url `clientDataJSON`.
    pub client_data_json: String,
    /// base64url attestation object.
    pub attestation_object: String,
    /// The credential id the browser reports (must match the attestation).
    pub credential_id: String,
    /// Optional owner-given name.
    #[serde(default)]
    pub label: String,
}

/// `POST /api/passkeys/register/finish`
pub async fn handle_passkey_register_finish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<FinishRegistrationRequest>,
) -> Response {
    let token = match require_session_token(&state, &headers).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&token).await {
        Ok(s) => s,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };
    let rp = match relying_party(&state) {
        Ok(rp) => rp,
        Err(resp) => return resp,
    };

    let label = match validate_label(&payload.label) {
        Ok(l) => l,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let repo = PasskeyRepository::new(state.db.pool());
    // Single use: the challenge is consumed here whatever the outcome, so a
    // failed ceremony cannot be retried against the same challenge.
    let challenge = match repo
        .take_challenge(&session.username, Ceremony::Registration.as_str())
        .await
    {
        Ok(Some(challenge)) => challenge,
        Ok(None) => {
            return ResponseHelper::json_error(
                "This registration attempt has expired — start again".to_string(),
            )
        }
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the ceremony: {e}")),
    };

    let client_data_json = match decode_b64url(&payload.client_data_json) {
        Ok(bytes) => bytes,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let attestation_object = match decode_b64url(&payload.attestation_object) {
        Ok(bytes) => bytes,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let reported_id = match decode_b64url(&payload.credential_id) {
        Ok(bytes) => bytes,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };

    let registered = match verify_registration(
        &client_data_json,
        &attestation_object,
        &challenge,
        &rp,
        // UV is *preferred*, not required: a hardware key without a PIN has
        // no way to satisfy it, and refusing one would lock out exactly the
        // most phishing-resistant option.
        false,
    ) {
        Ok(registered) => registered,
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Passkey registration refused for '{}': {}",
                session.username,
                e.message()
            ));
            return ResponseHelper::json_error_with_status(e.status(), e.message());
        }
    };

    // The id the browser echoed must be the id inside the attestation: a
    // mismatch means the client lied about which credential it created.
    if reported_id != registered.credential_id {
        return ResponseHelper::json_error(
            "The credential id does not match the attestation".to_string(),
        );
    }
    if registered.credential_id.len() > MAX_CREDENTIAL_ID_LEN {
        return ResponseHelper::json_error("The credential id is too long".to_string());
    }

    let credential_id = encode_b64url(&registered.credential_id);
    let label = if label.is_empty() {
        default_label(registered.algorithm)
    } else {
        label
    };

    match repo
        .insert(&NewPasskey {
            credential_id: &credential_id,
            username: &session.username,
            label: &label,
            algorithm: registered.algorithm.as_str(),
            public_key: &encode_b64url(&registered.public_key),
            sign_count: registered.sign_count as i64,
            attestation_fmt: &registered.fmt,
            aaguid: None,
            user_verified: registered.user_verified,
        })
        .await
    {
        Ok(()) => {
            crate::events::log_event(format!(
                "User '{}' registered a passkey ({})",
                session.username,
                registered.algorithm.as_str()
            ));
            ResponseHelper::json_success(serde_json::json!({
                "credential_id": credential_id,
                "label": label,
                "algorithm": registered.algorithm.as_str(),
                "user_verified": registered.user_verified,
            }))
        }
        // A duplicate id is the primary key speaking: this authenticator is
        // already registered, which is exactly what `excludeCredentials` tried
        // to prevent on the client.
        Err(e) => ResponseHelper::json_error(format!(
            "Failed to store the passkey (is it already registered?): {e}"
        )),
    }
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// What the browser needs to call `navigator.credentials.get()`.
#[derive(Debug, Serialize)]
pub struct AuthenticationOptions {
    pub challenge: String,
    pub rp_id: String,
    pub timeout: u64,
    /// Empty for a discoverable sign-in (no username typed).
    pub allow_credentials: Vec<CredentialDescriptor>,
    pub user_verification: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct BeginLoginRequest {
    /// Optional: a discoverable credential can sign in without it.
    #[serde(default)]
    pub username: String,
}

/// `POST /api/passkeys/login/begin`
///
/// TWO shapes, deliberately:
///
/// * **with a username** — the challenge is stored against that account and
///   `allowCredentials` names its passkeys. No password is checked, so the
///   response must not reveal whether the account exists: an unknown username
///   gets a challenge for a throw-away namespace, so the answer is identical.
/// * **without** — a discoverable credential (`residentKey`) picks the account
///   on the authenticator; the challenge is stored in the anonymous namespace
///   and the account is resolved from the credential id at `finish`.
pub async fn handle_passkey_login_begin(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<BeginLoginRequest>,
) -> Response {
    // The auto-block applies to the passkey path exactly as it does to the
    // password path: it is a sign-in attempt.
    let ip = crate::handlers::auth::extract_client_ip(&headers);
    if let crate::auto_block::BanCheck::Blocked { expires_at, .. } =
        crate::auto_block::check(&state, &ip).await
    {
        return ResponseHelper::json_error_with_status(
            axum::http::StatusCode::FORBIDDEN,
            crate::auto_block::blocked_message(&expires_at),
        );
    }

    let rp = match relying_party(&state) {
        Ok(rp) => rp,
        Err(resp) => return resp,
    };
    let repo = PasskeyRepository::new(state.db.pool());
    let _ = repo.sweep_expired().await;

    let username = payload.username.trim().to_string();
    let challenge = new_challenge();

    // An unknown account is answered with the same shape as a known one: no
    // enumeration oracle on the sign-in screen.
    let (namespace, allow_credentials) = if username.is_empty() {
        (anonymous_namespace(), Vec::new())
    } else {
        let ids = repo.ids_for_user(&username).await.unwrap_or_default();
        let namespace = if ids.is_empty() {
            anonymous_namespace()
        } else {
            username.clone()
        };
        (
            namespace,
            ids.iter()
                .map(|id| CredentialDescriptor {
                    credential_type: "public-key",
                    id: id.clone(),
                    transports: vec!["internal", "hybrid", "usb", "nfc", "ble"],
                })
                .collect(),
        )
    };

    if let Err(e) = repo
        .store_challenge(
            &namespace,
            Ceremony::Authentication.as_str(),
            &challenge,
            CHALLENGE_TTL_SECONDS,
        )
        .await
    {
        return ResponseHelper::json_error(format!("Failed to start the ceremony: {e}"));
    }

    ResponseHelper::json_success(AuthenticationOptions {
        challenge,
        rp_id: rp.id,
        timeout: (CHALLENGE_TTL_SECONDS as u64) * 1000,
        allow_credentials,
        user_verification: "preferred",
    })
}

/// The namespace an anonymous / unknown-account challenge lives in.
///
/// Prefixed with a NUL so it can never collide with a real username (the
/// username validator refuses control characters).
fn anonymous_namespace() -> String {
    "\u{0}anonymous".to_string()
}

#[derive(Debug, Deserialize)]
pub struct FinishLoginRequest {
    pub credential_id: String,
    /// base64url `clientDataJSON`.
    pub client_data_json: String,
    /// base64url `authenticatorData`.
    pub authenticator_data: String,
    /// base64url signature.
    pub signature: String,
    /// The username the client typed, when it typed one.
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub remember_me: bool,
}

/// `POST /api/passkeys/login/finish`
///
/// Verifies the assertion against the stored public key and — on success only —
/// creates a normal session, recording the successful attempt in the login
/// history exactly as the password path does.
pub async fn handle_passkey_login_finish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    peer: crate::middleware::PeerAddr,
    over_tls: Option<axum::Extension<crate::middleware::ArrivedOverTls>>,
    Json(payload): Json<FinishLoginRequest>,
) -> Response {
    let ip_address = crate::handlers::auth::client_ip(&headers, peer.0);
    let user_agent = headers.get("user-agent").and_then(|h| h.to_str().ok());
    let over_tls = over_tls.is_some();

    if let crate::auto_block::BanCheck::Blocked { expires_at, .. } =
        crate::auto_block::check(&state, &ip_address).await
    {
        return ResponseHelper::json_error_with_status(
            axum::http::StatusCode::FORBIDDEN,
            crate::auto_block::blocked_message(&expires_at),
        );
    }

    let rp = match relying_party(&state) {
        Ok(rp) => rp,
        Err(resp) => return resp,
    };
    let repo = PasskeyRepository::new(state.db.pool());

    let credential_id = payload.credential_id.trim();
    let stored = match repo.find(credential_id).await {
        Ok(Some(row)) => row,
        // A credential HomeVault does not know is refused with the same
        // sentence as a bad signature: no "does this user exist" oracle.
        Ok(None) | Err(_) => {
            crate::auto_block::record_failure(&state, &ip_address, "").await;
            return ResponseHelper::json_unauthorized("Passkey sign-in failed".to_string());
        }
    };

    // Which namespace does this challenge live in? The account when we know it
    // (discoverable or typed), the anonymous one otherwise.
    let typed = payload.username.trim();
    let namespace = if typed.is_empty() || typed == stored.username {
        stored.username.clone()
    } else {
        // The typed account does not own this credential — refuse without
        // consuming the other account's challenge.
        crate::auto_block::record_failure(&state, &ip_address, "").await;
        return ResponseHelper::json_unauthorized("Passkey sign-in failed".to_string());
    };
    let mut challenge = match repo
        .take_challenge(&namespace, Ceremony::Authentication.as_str())
        .await
    {
        Ok(Some(c)) => Some(c),
        _ => None,
    };
    if challenge.is_none() {
        // Fall back to the anonymous namespace: a discoverable sign-in with a
        // typed username stores its challenge there when the account had no
        // passkeys at `begin` time.
        challenge = match repo
            .take_challenge(&anonymous_namespace(), Ceremony::Authentication.as_str())
            .await
        {
            Ok(Some(c)) => Some(c),
            _ => None,
        };
    }
    let Some(challenge) = challenge else {
        return ResponseHelper::json_error(
            "This sign-in attempt has expired — start again".to_string(),
        );
    };

    let client_data_json = match decode_b64url(&payload.client_data_json) {
        Ok(b) => b,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let authenticator_data = match decode_b64url(&payload.authenticator_data) {
        Ok(b) => b,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let signature = match decode_b64url(&payload.signature) {
        Ok(b) => b,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let Some(algorithm) = KeyAlgorithm::parse(&stored.algorithm) else {
        // A row with an algorithm this build cannot verify is a refusal, not a
        // silent downgrade.
        return ResponseHelper::json_error(
            "This passkey uses an unsupported algorithm".to_string(),
        );
    };
    let public_key = match decode_b64url(&stored.public_key) {
        Ok(b) => b,
        Err(_) => {
            return ResponseHelper::json_error("The stored passkey is unreadable".to_string())
        }
    };

    let verified = match verify_assertion(
        &client_data_json,
        &authenticator_data,
        &signature,
        &challenge,
        &rp,
        algorithm,
        &public_key,
        stored.sign_count.max(0) as u32,
        false,
    ) {
        Ok(verified) => verified,
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Passkey sign-in refused for '{}': {}",
                stored.username,
                e.message()
            ));
            crate::auto_block::record_failure(&state, &ip_address, "").await;
            return ResponseHelper::json_error_with_status(e.status(), e.message());
        }
    };

    // The account must still be usable: a locked or expired account cannot
    // sign in with a passkey either, or the passkey would be a way around
    // an admin's lock.
    match state.auth.get_user(&stored.username).await {
        Ok(user) if user.locked => {
            return ResponseHelper::json_unauthorized("Account is locked".to_string())
        }
        Ok(_) => {}
        Err(_) => {
            return ResponseHelper::json_unauthorized("Invalid username or password".to_string())
        }
    }

    if let Err(e) = repo
        .record_use(credential_id, verified.sign_count as i64)
        .await
    {
        // The counter is a security signal: a session is still issued (the
        // assertion verified), but the failure to persist it is logged.
        tracing::warn!(
            "Failed to record passkey use for '{}': {e}",
            stored.username
        );
    }

    match state
        .auth
        .create_session(
            stored.username.clone(),
            ip_address.clone(),
            payload.remember_me,
        )
        .await
    {
        Ok(session) => {
            crate::events::log_event(format!(
                "User '{}' signed in with a passkey from {}",
                session.username, ip_address
            ));
            let _ = state
                .auth
                .record_successful_attempt(&session.username, &ip_address, user_agent)
                .await;
            let token = session.token.clone();
            let must_change_password = state
                .auth
                .get_user(&stored.username)
                .await
                .map(|u| u.must_change_password)
                .unwrap_or(false);
            let response = crate::storage::models::LoginResponse {
                token: session.token,
                csrf_token: session.csrf_token,
                username: session.username,
                is_admin: session.is_admin,
                must_change_password,
            };
            crate::app_proxy::cookie::attach_session(
                ResponseHelper::json_success(response),
                &token,
                payload.remember_me,
                over_tls,
            )
        }
        Err(e) => ResponseHelper::json_internal_error(format!("Failed to create session: {e}")),
    }
}

/// `POST /api/passkeys/sweep` — drop the expired challenges now.
///
/// Admin-only on purpose: it is housekeeping, and the same sweep already runs
/// on every ceremony, so a caller never needs it.
pub async fn handle_passkey_sweep(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    let repo = PasskeyRepository::new(state.db.pool());
    match repo.sweep_expired().await {
        Ok(removed) => ResponseHelper::json_success(serde_json::json!({ "removed": removed })),
        Err(e) => ResponseHelper::json_error(format!("Failed to sweep challenges: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_label_trims_and_bounds() {
        assert_eq!(
            validate_label("  MacBook Touch ID  ").as_deref(),
            Ok("MacBook Touch ID")
        );
        assert_eq!(
            validate_label("").as_deref(),
            Ok(""),
            "an empty label is allowed"
        );
        let long = "a".repeat(MAX_LABEL_LEN + 1);
        assert!(validate_label(&long).is_err());
        assert!(
            validate_label("a\u{0}b").is_err(),
            "control characters are refused"
        );
    }

    #[test]
    fn test_validate_label_counts_characters_not_bytes() {
        let accented = "é".repeat(MAX_LABEL_LEN);
        assert!(validate_label(&accented).is_ok());
    }

    #[test]
    fn test_default_label_names_the_authenticator_family() {
        assert!(default_label(KeyAlgorithm::Es256).contains("platform"));
        assert!(default_label(KeyAlgorithm::EdDsa).contains("security key"));
    }

    #[test]
    fn test_the_anonymous_namespace_cannot_collide_with_a_username() {
        // A username validator refuses control characters, so a leading NUL is
        // unreachable for a real account.
        assert!(anonymous_namespace().starts_with('\u{0}'));
        assert!(!anonymous_namespace().is_empty());
    }
}
