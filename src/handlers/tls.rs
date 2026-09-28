//! HTTPS/TLS settings endpoints (admin-only).
//!
//! * `GET  /api/settings/tls`            — current config + certificate info
//! * `PUT  /api/settings/tls`            — save config (applies after a restart)
//! * `POST /api/settings/tls/regenerate` — force self-signed regeneration
//!
//! The listener itself is bound once at startup ([`crate::web::run`]); saving
//! here persists the choice and reports `requires_restart: true`, mirroring how
//! OpenMediaVault restarts its web server to apply a certificate change.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};

use crate::handlers::admin_guard::require_admin;
use crate::tls::{self, TlsSettings};
use crate::utils::ResponseHelper;

use super::AppState;

fn http_port() -> u16 {
    crate::config::Settings::load().port
}

fn settings_payload(s: &TlsSettings) -> serde_json::Value {
    serde_json::json!({
        "enabled": s.enabled,
        "https_port": s.https_port,
        "effective_https_port": s.effective_https_port(),
        "http_port": http_port(),
        "mode": s.mode,
        "cert_path": s.cert_path,
        "key_path": s.key_path,
        "redirect": s.redirect,
        "active": tls::tls_active(),
        "cert_info": tls::certificate_info(s),
    })
}

/// `GET /api/settings/tls`
pub async fn handle_tls_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let s = tls::load_settings();
    ResponseHelper::json_success(settings_payload(&s))
}

/// `PUT /api/settings/tls`
pub async fn handle_tls_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TlsSettings>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if let Err(e) = tls::validate_settings(&body, http_port()) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = tls::save_settings(&body) {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' updated HTTPS/TLS settings (enabled={}, mode={:?}, port={}, redirect={})",
        username,
        body.enabled,
        body.mode,
        body.effective_https_port(),
        body.redirect
    ));

    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "requires_restart": true,
        "settings": settings_payload(&body),
    }))
}

/// `POST /api/settings/tls/regenerate`
pub async fn handle_tls_regenerate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if let Err(e) = tls::generate_self_signed() {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' regenerated the self-signed TLS certificate",
        username
    ));

    let s = tls::load_settings();
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "requires_restart": tls::tls_active(),
        "cert_info": tls::certificate_info(&crate::tls::TlsSettings {
            mode: crate::tls::TlsMode::SelfSigned,
            ..s
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_payload_shape() {
        let s = TlsSettings {
            enabled: true,
            https_port: Some(8443),
            ..Default::default()
        };
        let v = settings_payload(&s);
        assert_eq!(v["enabled"], true);
        assert_eq!(v["effective_https_port"], 8443);
        assert_eq!(v["mode"], "self-signed");
        assert!(v.get("cert_info").is_some());
    }
}
