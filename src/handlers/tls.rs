/* =============================================================================
 * File          : tls.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTPS/TLS settings endpoints: read the current configuration plus
 *   certificate info, save the configuration (a certificate change is pushed
 *   to the live listener; enabling/disabling or moving the port needs a
 *   restart), and regenerate the self-signed certificate. Admin-only
 *   throughout.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   Routes /api/settings/tls in src/web/mod.rs; see docs/guides/tls.md for
 *   the listener and certificate model.
 * =============================================================================
 */

//! HTTPS/TLS settings endpoints (admin-only).
//!
//! * `GET  /api/settings/tls`            — current config + certificate info
//! * `PUT  /api/settings/tls`            — save config (certificate changes go
//!   live at once; enabling/disabling or a port change needs a restart)
//! * `POST /api/settings/tls/regenerate` — force self-signed regeneration
//!
//! The listener itself is bound once at startup ([`crate::web::run`]). A pair
//! change on that bound listener is pushed through the shared `RustlsConfig`
//! swap cell ([`crate::tls::reload_tls_config`]), so only a bind-affecting
//! change — enabling HTTPS, disabling it, moving the port — still reports
//! `requires_restart: true`.

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
    let previous = tls::load_settings();
    if let Err(e) = tls::save_settings(&body) {
        return ResponseHelper::json_internal_error(e);
    }

    // A certificate change on an already-bound listener can be pushed live:
    // `RustlsConfig` is a shared swap cell, so new connections pick the new
    // pair up without a restart. Only enabling/disabling HTTPS or moving the
    // port still needs one — those change what the listener binds.
    let rebind_needed = previous.enabled != body.enabled
        || previous.effective_https_port() != body.effective_https_port();
    let mut requires_restart = rebind_needed || !body.enabled;
    let mut reload_error: Option<String> = None;
    if !rebind_needed && body.enabled && tls::reload_available() {
        match tls::reload_tls_config().await {
            Ok(()) => requires_restart = false,
            Err(e) => reload_error = Some(e),
        }
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
        "requires_restart": requires_restart,
        "reload_error": reload_error,
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

    // The regenerated pair replaces the one on disk; push it to the running
    // listener when the current mode is the self-signed one.
    let mut requires_restart = tls::tls_active();
    if tls::reload_available() && tls::load_settings().mode == tls::TlsMode::SelfSigned {
        match tls::reload_tls_config().await {
            Ok(()) => requires_restart = false,
            Err(e) => tracing::warn!("TLS: regenerated certificate not pushed live: {e}"),
        }
    }

    crate::events::log_event(format!(
        "Admin '{}' regenerated the self-signed TLS certificate",
        username
    ));

    let s = tls::load_settings();
    ResponseHelper::json_success(serde_json::json!({
        "success": true,
        "requires_restart": requires_restart,
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
