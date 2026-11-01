/* =============================================================================
 * File          : ddns.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Dynamic DNS endpoints: read/save the configuration, send an update now,
 *   and probe what the configured provider would be told. Admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::ddns, crate::outbound
 *
 * Usage:
 *   Routes /api/settings/ddns in src/web/mod.rs; see docs/guides/ddns.md.
 * =============================================================================
 */

//! Dynamic DNS endpoints (admin-only), see docs/guides/ddns.md.
//!
//! * `GET  /api/settings/ddns`        — configuration + status + detection
//! * `PUT  /api/settings/ddns`        — save (token write-only)
//! * `POST /api/settings/ddns/update` — send an update now

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};

use crate::ddns::{self, DdnsSettings};
use crate::handlers::admin_guard::require_admin;
use crate::utils::ResponseHelper;

use super::AppState;

/// Payload the Settings card renders. The token is redacted; everything else
/// is what the admin typed plus what the last attempt did.
pub fn settings_payload(s: &DdnsSettings) -> serde_json::Value {
    let redacted = ddns::redacted(s.clone());
    serde_json::json!({
        "enabled": redacted.enabled,
        "provider": redacted.provider,
        "providers": ddns::DdnsProvider::ALL.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        "hostname": redacted.hostname,
        "token": redacted.token,
        "custom_update_url": redacted.custom_update_url,
        "status": redacted.status,
        "last_ip": redacted.last_ip,
        "last_update": redacted.last_update,
        "last_error": redacted.last_error,
        "current_ip": ddns::detect_public_ip(),
    })
}

/// `GET /api/settings/ddns`
pub async fn handle_ddns_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(settings_payload(&ddns::load_settings()))
}

/// `PUT /api/settings/ddns`
pub async fn handle_ddns_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut body): Json<DdnsSettings>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let stored = ddns::load_settings();
    if body.token == "set" {
        body.token = stored.token.clone();
    }
    // Provider-side state is never client input.
    body.status = stored.status;
    body.last_ip = stored.last_ip.clone();
    body.last_update = stored.last_update.clone();
    body.last_error = stored.last_error.clone();
    // Changing the target or the provider invalidates the recorded result:
    // the admin just acted on it.
    if body.hostname != stored.hostname || body.provider != stored.provider {
        body.status = ddns::DdnsStatus::Unknown;
        body.last_ip = None;
        body.last_error = None;
    }

    if let Err(e) = ddns::validate_settings(&body) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = ddns::save_settings(&body) {
        return ResponseHelper::json_internal_error(e);
    }
    crate::events::log_event(format!(
        "Admin '{}' updated dynamic DNS (enabled={}, provider={}, hostname={})",
        username,
        body.enabled,
        body.provider.as_str(),
        body.hostname
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "settings": settings_payload(&ddns::load_settings()),
    }))
}

/// `POST /api/settings/ddns/update`
///
/// Sends an update with the address the host currently uses, regardless of
/// whether the worker thinks one is due. This is the "did I get the token
/// right?" button, so its failure is reported verbatim.
pub async fn handle_ddns_update_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let settings = ddns::load_settings();
    if !settings.enabled {
        return ResponseHelper::json_error("enable dynamic DNS first".to_string());
    }
    if let Err(e) = ddns::validate_settings(&settings) {
        return ResponseHelper::json_error(e);
    }
    let ip = match ddns::detect_public_ip() {
        Some(ip) => ip,
        None => {
            return ResponseHelper::json_error(
                "no public IPv4 address is available on this host".to_string(),
            )
        }
    };
    let transport = match crate::outbound::ReqwestTransport::new() {
        Ok(transport) => transport,
        Err(e) => return ResponseHelper::json_internal_error(e.message),
    };

    let mut saved = settings.clone();
    match ddns::update_now(&transport, &settings, &ip).await {
        Ok(()) => {
            saved.status = ddns::DdnsStatus::UpToDate;
            saved.last_ip = Some(ip.clone());
            saved.last_update = Some(crate::acme::now_rfc3339());
            saved.last_error = None;
            let _ = ddns::save_settings(&saved);
            crate::events::log_event(format!(
                "Admin '{}' updated dynamic DNS {} to {}",
                username, settings.hostname, ip
            ));
            ResponseHelper::json_success(serde_json::json!({
                "success": true,
                "ip": ip,
                "settings": settings_payload(&ddns::load_settings()),
            }))
        }
        Err(e) => {
            saved.status = ddns::DdnsStatus::Error;
            saved.last_error = Some(e.clone());
            let _ = ddns::save_settings(&saved);
            ResponseHelper::json_error(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_payload_redacts_the_token() {
        let s = DdnsSettings {
            enabled: true,
            hostname: "myhome.duckdns.org".to_string(),
            token: "secret".to_string(),
            ..Default::default()
        };
        let payload = settings_payload(&s);
        assert_eq!(payload["token"], "set");
        assert!(!payload.to_string().contains("\"secret\""));
        assert_eq!(payload["providers"][3], "custom");
    }

    #[test]
    fn test_settings_payload_reports_the_detected_address() {
        let payload = settings_payload(&DdnsSettings::default());
        // Either an address or `null` in a sandbox with no route out; the key
        // must exist so the UI can show "unknown" instead of guessing.
        assert!(payload.get("current_ip").is_some());
        assert_eq!(payload["status"], "disabled");
    }
}
