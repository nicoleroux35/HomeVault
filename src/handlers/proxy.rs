/* =============================================================================
 * File          : proxy.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026
 * Last modified : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Outbound HTTP(S) proxy endpoints: read and save the configuration used by
 *   certificate automation, dynamic DNS, update checks, notifications and Web
 *   Push. Admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::proxy, crate::handlers::admin_guard
 *
 * Usage:
 *   Routes /api/settings/proxy in src/web/mod.rs; see docs/guides/settings.md.
 * =============================================================================
 */

//! Outbound proxy endpoints (admin-only), see docs/guides/settings.md.
//!
//! * `GET /api/settings/proxy` — configuration + the effective exemption list
//! * `PUT /api/settings/proxy` — validate, persist and apply to new calls

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};

use crate::handlers::admin_guard::require_admin;
use crate::proxy::{self, ProxySettings};
use crate::utils::ResponseHelper;

use super::AppState;

/// What the Settings card renders. Nothing here is a secret: credentials in a
/// proxy URL are refused at validation time.
pub fn settings_payload(settings: &ProxySettings) -> serde_json::Value {
    serde_json::json!({
        "enabled": settings.enabled,
        "http_proxy": settings.http_proxy,
        "https_proxy": settings.https_proxy,
        "no_proxy": settings.no_proxy,
        "effective_no_proxy": proxy::effective_no_proxy(&settings.no_proxy),
        "supported_schemes": ["http", "https"],
        // Webhook and push clients rebuild when this value changes, and every
        // other outbound call builds a client per request — no restart needed.
        "requires_restart": false,
    })
}

/// `GET /api/settings/proxy`
pub async fn handle_proxy_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(settings_payload(&proxy::load_settings()))
}

/// `PUT /api/settings/proxy`
pub async fn handle_proxy_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ProxySettings>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(user) => user,
        Err(resp) => return resp,
    };

    if let Err(e) = proxy::validate_settings(&body) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = proxy::save_settings(&body) {
        return ResponseHelper::json_internal_error(e);
    }
    let saved = proxy::load_settings();
    crate::events::log_event(format!(
        "Admin '{}' updated the outbound proxy (enabled={}, http={}, https={}, no_proxy={})",
        username,
        saved.enabled,
        if saved.http_proxy.is_empty() {
            "—"
        } else {
            "set"
        },
        if saved.https_proxy.is_empty() {
            "—"
        } else {
            "set"
        },
        if saved.no_proxy.is_empty() {
            "—"
        } else {
            "set"
        }
    ));
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "settings": settings_payload(&saved),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_payload_always_names_the_loopback_exemptions() {
        let payload = settings_payload(&ProxySettings::default());
        assert_eq!(payload["enabled"], false);
        assert_eq!(payload["requires_restart"], false);
        let effective = payload["effective_no_proxy"].as_str().unwrap();
        for exempt in proxy::ALWAYS_DIRECT {
            assert!(effective.split(',').any(|e| e == *exempt), "{effective}");
        }
        assert_eq!(payload["supported_schemes"][0], "http");
        assert_eq!(payload["supported_schemes"][1], "https");
    }

    #[test]
    fn test_settings_payload_never_contains_a_credential() {
        // A credential cannot be saved (validation refuses it), so the payload
        // can only ever carry the bare URLs.
        let payload = settings_payload(&ProxySettings {
            enabled: true,
            http_proxy: "http://proxy.lan:3128".to_string(),
            ..Default::default()
        });
        assert!(!payload.to_string().contains('@'));
        assert_eq!(payload["http_proxy"], "http://proxy.lan:3128");
    }
}
