/* =============================================================================
 * File          : syslog.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026
 * Last modified : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Remote syslog endpoints: read/save the forwarding configuration, send a
 *   test message now, and report the live queue counters. Admin-only, because
 *   the destination decides where this box's audit trail goes.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::syslog_forward
 *
 * Usage:
 *   Routes /api/settings/syslog in src/web/mod.rs; see docs/guides/journal.md.
 * =============================================================================
 */

//! Remote syslog endpoints (admin-only), see docs/guides/journal.md.
//!
//! * `GET  /api/settings/syslog`      — configuration + live queue counters
//! * `PUT  /api/settings/syslog`      — save (validation, TLS CA required)
//! * `POST /api/settings/syslog/test` — send one test message now

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};
use serde::Deserialize;

use crate::handlers::admin_guard::require_admin;
use crate::syslog_forward::{self, SyslogFormat, SyslogProtocol, SyslogSettings, SyslogStatus};
use crate::utils::ResponseHelper;

use super::AppState;

/// What the Settings card renders: the stored configuration, what the worker
/// has done since start, and the fixed option lists.
pub fn settings_payload(s: &SyslogSettings) -> serde_json::Value {
    let stats = syslog_forward::queue_stats();
    serde_json::json!({
        "enabled": s.enabled,
        "protocol": s.protocol,
        "protocols": SyslogProtocol::ALL.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
        "host": s.host,
        "port": s.port,
        "format": s.format,
        "formats": SyslogFormat::ALL.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "facility": s.facility,
        "ca_pem": s.ca_pem,
        "ca_set": !s.ca_pem.trim().is_empty(),
        "include_app_events": s.include_app_events,
        "status": s.status,
        "last_sent_at": s.last_sent_at,
        "last_error": s.last_error,
        "queue": {
            "pending": stats.pending,
            "enqueued": stats.enqueued,
            "dropped": stats.dropped,
            "sent": stats.sent,
            "failed": stats.failed,
        },
    })
}

/// `GET /api/settings/syslog`
pub async fn handle_syslog_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(settings_payload(&syslog_forward::load_settings()))
}

/// Request body for `PUT /api/settings/syslog`.
///
/// Deliberately flat and explicit rather than `SyslogSettings`: the client must
/// not be able to write `status`, `last_error` or `last_sent_at` (server-owned
/// observations), and a missing field keeps its stored value so the card can
/// save a subset.
#[derive(Debug, Deserialize)]
pub struct SyslogSettingsRequest {
    pub enabled: Option<bool>,
    pub protocol: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub format: Option<String>,
    pub facility: Option<u8>,
    pub ca_pem: Option<String>,
    pub include_app_events: Option<bool>,
}

/// `PUT /api/settings/syslog`
pub async fn handle_syslog_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SyslogSettingsRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let stored = syslog_forward::load_settings();
    let mut next = stored.clone();

    if let Some(enabled) = body.enabled {
        next.enabled = enabled;
    }
    if let Some(protocol) = body.protocol.as_deref() {
        match SyslogProtocol::parse_strict(protocol) {
            Ok(p) => next.protocol = p,
            Err(e) => return ResponseHelper::json_error(e),
        }
    }
    if let Some(format) = body.format.as_deref() {
        match SyslogFormat::parse_strict(format) {
            Ok(f) => next.format = f,
            Err(e) => return ResponseHelper::json_error(e),
        }
    }
    if let Some(host) = body.host {
        next.host = host.trim().to_string();
    }
    if let Some(port) = body.port {
        next.port = port;
    }
    if let Some(facility) = body.facility {
        next.facility = facility;
    }
    if let Some(ca_pem) = body.ca_pem {
        next.ca_pem = ca_pem;
    }
    if let Some(include) = body.include_app_events {
        next.include_app_events = include;
    }

    // Changing the destination invalidates the recorded outcome: the admin just
    // acted on it, and the old "refused by 10.0.0.5" would describe a server
    // nobody is targeting any more.
    if next.host != stored.host
        || next.port != stored.port
        || next.protocol != stored.protocol
        || next.format != stored.format
    {
        next.status = SyslogStatus::Idle;
        next.last_error = None;
        next.last_sent_at = None;
    }

    if let Err(e) = syslog_forward::validate_settings(&next) {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = syslog_forward::save_settings(&next) {
        return ResponseHelper::json_internal_error(e);
    }

    crate::events::log_event(format!(
        "Admin '{}' updated remote syslog forwarding (enabled={}, {} over {} to {}:{})",
        username,
        next.enabled,
        next.format.as_str(),
        next.protocol.as_str(),
        next.host.trim(),
        next.port
    ));

    ResponseHelper::json_success(serde_json::json!({
        "settings": settings_payload(&syslog_forward::load_settings()),
    }))
}

/// `POST /api/settings/syslog/test`
///
/// Sends one message with the saved configuration and answers with the
/// collector's own reason on failure — the quickest way to tell a wrong address
/// from a wrong port from a missing CA.
pub async fn handle_syslog_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    match syslog_forward::send_test_message().await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({
            "sent": true,
            // The wording matters: a UDP datagram is fire-and-forget, so this
            // can only ever claim the packet left, never that it was received.
            "message": "Test message sent — the collector must show it to confirm the path.",
        })),
        Err(reason) => {
            ResponseHelper::json_error(format!("The test message could not be sent: {reason}"))
        }
    }
}
