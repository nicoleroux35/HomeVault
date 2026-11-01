/* =============================================================================
 * File          : acme.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Certificate automation endpoints: read/save the settings, start an
 *   issuance, confirm a manual DNS record, probe the HTTP-01 challenge route,
 *   and answer the CA's challenge fetch. Admin-only except the challenge route.
 *
 * Dependencies:
 *   - axum (>= 0.8), reqwest (>= 0.13)
 *
 * Usage:
 *   Routes in src/web/mod.rs; see docs/guides/acme.md.
 * =============================================================================
 */

//! Certificate automation endpoints (see docs/guides/acme.md).
//!
//! * `GET  /api/settings/acme`                — settings + status + certificate
//! * `PUT  /api/settings/acme`                — save settings
//! * `POST /api/settings/acme/issue`          — start an issuance
//! * `POST /api/settings/acme/confirm`        — resume after a manual DNS record
//! * `POST /api/settings/acme/test-challenge` — prove the challenge route is reachable
//! * `GET  /.well-known/acme-challenge/{token}` — the CA's fetch (no auth)

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use crate::acme::{self, AcmeSettings, AcmeStatus, ChallengeType};
use crate::handlers::admin_guard::require_admin;
use crate::utils::ResponseHelper;

use super::AppState;

/// Everything the Settings card renders, secrets redacted.
pub fn settings_payload(s: &AcmeSettings) -> serde_json::Value {
    let cert = if acme::issued_cert_path().exists() {
        Some(crate::tls::certificate_info(&crate::tls::TlsSettings {
            mode: crate::tls::TlsMode::Acme,
            ..Default::default()
        }))
    } else {
        None
    };
    let redacted = acme::redacted(s.clone());
    serde_json::json!({
        "enabled": redacted.enabled,
        "domain": redacted.domain,
        "directory_url": redacted.directory_url,
        "contact_email": redacted.contact_email,
        "challenge": redacted.challenge,
        "provider": redacted.provider,
        "dns_api_token": redacted.dns_api_token,
        "tos_agreed": redacted.tos_agreed,
        "dns_propagation_seconds": redacted.dns_propagation_seconds,
        "status": redacted.status,
        "last_error": redacted.last_error,
        "last_attempt": redacted.last_attempt,
        "last_success": redacted.last_success,
        "pending_dns_record": redacted.pending_dns_record,
        "account_url": redacted.account_url,
        "consecutive_failures": redacted.consecutive_failures,
        "run_in_progress": acme::run_in_progress(),
        "automation_active": acme::automation_active(),
        "providers": acme::DnsProvider::ALL
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
        "default_directory": acme::DEFAULT_DIRECTORY_URL,
        "staging_directory": acme::STAGING_DIRECTORY_URL,
        "cert": cert,
    })
}

/// `GET /api/settings/acme`
pub async fn handle_acme_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let mut settings = acme::load_settings();
    if acme::refresh_status(&mut settings) {
        let _ = acme::save_settings(&settings);
    }
    ResponseHelper::json_success(settings_payload(&settings))
}

/// `PUT /api/settings/acme`
///
/// The token is write-only: the client sends `"set"` to keep the stored value,
/// an empty string to clear it, or a new token to replace it.
pub async fn handle_acme_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<AcmeSettings>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let mut stored = acme::load_settings();
    if body.dns_api_token == "set" {
        body.dns_api_token = stored.dns_api_token.clone();
    }

    if body.enabled {
        if let Err(e) = acme::validate_settings(&body, http_port()) {
            return ResponseHelper::json_error(e);
        }
    } else if !body.domain.trim().is_empty() {
        // A disabled configuration may be incomplete, but whatever is there
        // still has to be a sane domain — it is rendered back to the UI.
        if let Err(e) = acme::validate_domain(&body.domain) {
            return ResponseHelper::json_error(e);
        }
    }

    // Changing the target or the challenge invalidates the recorded failure:
    // the admin just acted on it, so the backoff must not keep the worker
    // parked.
    if body.domain != stored.domain
        || body.challenge != stored.challenge
        || body.provider != stored.provider
        || body.directory_url != stored.directory_url
    {
        body.consecutive_failures = 0;
        body.last_error = None;
        body.status = AcmeStatus::Idle;
    } else {
        body.consecutive_failures = stored.consecutive_failures;
    }
    // `pending_dns_record` and `account_url` are server-owned state, never
    // client input.
    body.pending_dns_record = stored.pending_dns_record.clone();
    body.account_url = stored.account_url.clone();
    body.last_attempt = stored.last_attempt.clone();
    body.last_success = stored.last_success.clone();
    if body.status == AcmeStatus::Idle {
        stored.status = AcmeStatus::Idle;
    }

    if let Err(e) = acme::save_settings(&body) {
        return ResponseHelper::json_internal_error(e);
    }
    let mut saved = body.clone();
    acme::refresh_status(&mut saved);
    if saved.status != body.status {
        let _ = acme::save_settings(&saved);
    }

    crate::events::log_event(format!(
        "Admin '{}' updated certificate automation (enabled={}, domain={}, challenge={:?}, provider={})",
        username, body.enabled, body.domain, body.challenge, body.provider.as_str()
    ));

    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "requires_restart": acme::automation_active(),
        "settings": settings_payload(&saved),
    }))
}

fn http_port() -> u16 {
    crate::config::Settings::load().port
}

/// `POST /api/settings/acme/issue`
pub async fn handle_acme_issue(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let settings = acme::load_settings();
    if !settings.enabled {
        return ResponseHelper::json_error("enable certificate automation first".to_string());
    }
    if let Err(e) = acme::validate_settings(&settings, http_port()) {
        return ResponseHelper::json_error(e);
    }
    if acme::load_pending_run().is_some() {
        return ResponseHelper::json_error(
            "a DNS record is already waiting for confirmation".to_string(),
        );
    }
    if !crate::acme_worker::spawn_issuance(state.clone(), crate::acme_worker::IssueTrigger::Manual)
    {
        return ResponseHelper::json_error("an issuance is already running".to_string());
    }

    crate::events::log_event(format!(
        "Admin '{}' started a certificate issuance for {}",
        username, settings.domain
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "started": true,
        "status": "pending",
    }))
}

/// `POST /api/settings/acme/confirm`
pub async fn handle_acme_confirm(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let run = match acme::load_pending_run() {
        Some(run) => run,
        None => return ResponseHelper::json_error("there is no DNS record to confirm".to_string()),
    };
    if !crate::acme_worker::spawn_issuance(
        state.clone(),
        crate::acme_worker::IssueTrigger::ConfirmDns,
    ) {
        return ResponseHelper::json_error("an issuance is already running".to_string());
    }
    crate::events::log_event(format!(
        "Admin '{}' confirmed the DNS record {} and resumed the issuance",
        username, run.record.name
    ));
    ResponseHelper::json_success(serde_json::json!({ "success": true, "started": true }))
}

/// `POST /api/settings/acme/test-challenge`
///
/// Publishes a throwaway token and asks for it over plain HTTP at the public
/// name. This is the one check that tells an admin whether port 80 actually
/// reaches HomeVault *before* a CA fails the order for them.
pub async fn handle_acme_test_challenge(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let settings = acme::load_settings();
    let domain = match acme::validate_domain(&settings.domain) {
        Ok(d) => d,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let token = format!("homevault-{}", uuid::Uuid::new_v4().simple());
    let key = match acme::load_or_create_account_key() {
        Ok(key) => key,
        Err(e) => return ResponseHelper::json_internal_error(e.message),
    };
    let answer = format!("{token}.{}", key.thumbprint());
    if let Err(e) = acme::register_challenge(&token, &answer) {
        return ResponseHelper::json_internal_error(e);
    }

    let url = format!("http://{domain}/.well-known/acme-challenge/{token}");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            acme::remove_challenge(&token);
            return ResponseHelper::json_internal_error(format!(
                "cannot build the HTTP client: {e}"
            ));
        }
    };
    let fetched = client.get(&url).send().await;
    let result = match fetched {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if status == 200 && body.trim() == answer {
                Ok(())
            } else if status != 200 {
                Err(format!(
                    "the name answered HTTP {status} instead of serving the challenge"
                ))
            } else {
                Err("the name answered with something other than the challenge".to_string())
            }
        }
        Err(e) => Err(format!("cannot reach http://{domain}: {e}")),
    };
    acme::remove_challenge(&token);

    match result {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({
            "reachable": true,
            "url": url,
            "detail": "the challenge route answered correctly over HTTP",
        })),
        Err(detail) => ResponseHelper::json_success(serde_json::json!({
            "reachable": false,
            "url": url,
            "detail": detail,
        })),
    }
}

/// `GET /.well-known/acme-challenge/{token}` — the CA's fetch.
///
/// Deliberately unauthenticated (the CA has no session) and deliberately
/// silent about what went wrong: a miss is a 404 with no body, and a token
/// that is not pending is a miss. The registry is bounded and tokens expire
/// with the run that created them.
pub async fn handle_acme_challenge(Path(token): Path<String>) -> Response {
    if !acme::is_valid_token(&token) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match acme::challenge_answer(&token) {
        Some(answer) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            answer,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Whether the configured challenge can work at all from this instance.
/// Reported in the settings payload so the UI can warn before an attempt.
pub fn challenge_hint(settings: &AcmeSettings) -> Option<&'static str> {
    match settings.challenge {
        ChallengeType::Http01 => None,
        ChallengeType::Dns01 if !settings.provider.is_automated() => Some("manual"),
        ChallengeType::Dns01 => Some("automated"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_payload_never_carries_the_token() {
        let s = AcmeSettings {
            enabled: true,
            domain: "nas.example.com".to_string(),
            dns_api_token: "super-secret".to_string(),
            ..Default::default()
        };
        let payload = settings_payload(&s);
        assert_eq!(payload["dns_api_token"], "set");
        assert!(!payload.to_string().contains("super-secret"));
        assert_eq!(payload["domain"], "nas.example.com");
        assert_eq!(payload["default_directory"], acme::DEFAULT_DIRECTORY_URL);
    }

    #[test]
    fn test_settings_payload_lists_the_providers() {
        let payload = settings_payload(&AcmeSettings::default());
        assert_eq!(payload["providers"][0], "manual");
        assert_eq!(payload["providers"][1], "cloudflare");
        assert_eq!(payload["status"], "disabled");
    }

    #[test]
    fn test_challenge_hint_distinguishes_manual_from_automated() {
        let mut s = AcmeSettings::default();
        assert_eq!(challenge_hint(&s), None);
        s.challenge = ChallengeType::Dns01;
        assert_eq!(challenge_hint(&s), Some("manual"));
        s.provider = acme::DnsProvider::Cloudflare;
        assert_eq!(challenge_hint(&s), Some("automated"));
    }
}
