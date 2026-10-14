/* =============================================================================
 * File          : firewall.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Firewall (ufw) management endpoints: status and rule listing, rule
 *   add/delete, and enable/disable. Self-lockout safety is enforced
 *   server-side: enabling always re-asserts the mandatory SSH/app-port
 *   rules first and needs an explicit confirmation body, and deleting a
 *   rule marked mandatory is rejected with 403 before ufw is touched.
 *   Status is session-auth; every mutation is admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/firewall in src/web/mod.rs; system changes via
 *   crate::firewall, persistence via FirewallRepository.
 * =============================================================================
 */

//! HTTP handlers for firewall (ufw) management.
//!
//! Self-lockout safety is enforced here, not just in the frontend: enabling
//! the firewall always re-asserts the mandatory SSH/app-port rules first and
//! requires an explicit `{"confirmed": true}` body (there is deliberately no
//! generic "set enabled" endpoint), and deleting a rule that the live `ufw`
//! output marks as mandatory (via `src/firewall.rs`'s comment-based check)
//! is rejected with 403 before ever touching `ufw` — the same rejection a
//! raw `curl` call would hit as a UI click, since the check runs server-side
//! against live system state, not client-supplied flags.

use crate::firewall;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::FirewallRepository;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json};
use serde::Deserialize;
use std::sync::Arc;

fn unavailable() -> Response {
    ResponseHelper::json_error("ufw is not installed".to_string())
}

/// `GET /api/firewall/status`
pub async fn handle_firewall_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    if !firewall::ufw_available() {
        return ResponseHelper::json_success(serde_json::json!({
            "available": false,
            "enabled": false,
            "rules": [],
        }));
    }

    match firewall::get_status() {
        Ok((enabled, rules)) => ResponseHelper::json_success(serde_json::json!({
            "available": true,
            "enabled": enabled,
            "rules": rules,
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to read firewall status: {}", e)),
    }
}

/// `GET /api/firewall/enable/preview` — shows exactly what enabling would
/// activate (the mandatory rules plus whatever custom rules already exist),
/// so the confirmation UI never has to guess.
pub async fn handle_firewall_enable_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    if !firewall::ufw_available() {
        return unavailable();
    }

    let ssh_port = firewall::detect_ssh_port();
    let app_port = firewall::detect_app_port();
    let existing_rules = firewall::get_status()
        .map(|(_, rules)| rules)
        .unwrap_or_default();

    ResponseHelper::json_success(serde_json::json!({
        "ssh_port": ssh_port,
        "app_port": app_port,
        "existing_rules": existing_rules,
    }))
}

#[derive(Debug, Deserialize)]
pub struct EnableRequest {
    #[serde(default)]
    pub confirmed: bool,
}

/// `POST /api/firewall/enable` — the only path that can turn the firewall
/// on. Always re-asserts the mandatory SSH/app-port rules first, using
/// server-detected ports, before enabling.
pub async fn handle_firewall_enable(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EnableRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !firewall::ufw_available() {
        return unavailable();
    }
    if !body.confirmed {
        return ResponseHelper::json_error(
            "Enabling the firewall requires explicit confirmation".to_string(),
        );
    }

    let ssh_port = firewall::detect_ssh_port();
    let app_port = firewall::detect_app_port();

    if let Err(e) = firewall::ensure_mandatory_rules(ssh_port, app_port) {
        return ResponseHelper::json_error(format!("Failed to assert mandatory rules: {}", e));
    }
    if let Err(e) = firewall::enable() {
        return ResponseHelper::json_error(format!("Failed to enable firewall: {}", e));
    }

    let repo = FirewallRepository::new(state.db.pool());
    let _ = repo
        .save_state(
            true,
            ssh_port,
            app_port,
            Some(&chrono::Utc::now().to_rfc3339()),
        )
        .await;
    let _ = repo.record_change("enabled", None, &username).await;
    crate::events::log_event_warn(format!("Admin '{}' enabled the firewall", username));

    ResponseHelper::json_success(serde_json::json!({ "ok": true }))
}

/// `POST /api/firewall/disable`
pub async fn handle_firewall_disable(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !firewall::ufw_available() {
        return unavailable();
    }
    if let Err(e) = firewall::disable() {
        return ResponseHelper::json_error(format!("Failed to disable firewall: {}", e));
    }

    let repo = FirewallRepository::new(state.db.pool());
    let settings = repo.get_settings().await.unwrap_or_default();
    let _ = repo
        .save_state(
            false,
            settings.ssh_port,
            settings.app_port,
            settings.confirmed_at.as_deref(),
        )
        .await;
    let _ = repo.record_change("disabled", None, &username).await;
    crate::events::log_event_warn(format!("Admin '{}' disabled the firewall", username));

    ResponseHelper::json_success(serde_json::json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
pub struct RuleRequest {
    pub port: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_action")]
    pub action: String,
    pub source: Option<String>,
}

fn default_protocol() -> String {
    "tcp".to_string()
}

fn default_action() -> String {
    "allow".to_string()
}

/// `POST /api/firewall/rules`
pub async fn handle_firewall_rule_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RuleRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !firewall::ufw_available() {
        return unavailable();
    }
    if body.port.trim().is_empty() {
        return ResponseHelper::json_error("Port is required".to_string());
    }

    let comment = format!("homevault-custom-{}-{}", body.action, body.port);
    if let Err(e) = firewall::add_rule(
        &body.port,
        &body.protocol,
        &body.action,
        body.source.as_deref(),
        &comment,
    ) {
        return ResponseHelper::json_error(format!("Failed to add rule: {}", e));
    }

    let repo = FirewallRepository::new(state.db.pool());
    let description = format!("{} {}/{}", body.action, body.port, body.protocol);
    let _ = repo
        .record_change("rule_added", Some(&description), &username)
        .await;
    crate::events::log_event(format!(
        "Admin '{}' added firewall rule: {}",
        username, description
    ));

    ResponseHelper::json_success(serde_json::json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
pub struct RuleDeleteRequest {
    pub port: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_action")]
    pub action: String,
}

/// `DELETE /api/firewall/rules` — spec-based (port/protocol/action), not
/// index-based. Rejects the mandatory SSH/app-port rules server-side by
/// re-checking the live `ufw` status's own `is_mandatory` flag, so this
/// can't be bypassed by calling the API directly.
pub async fn handle_firewall_rule_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RuleDeleteRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !firewall::ufw_available() {
        return unavailable();
    }

    let rules = match firewall::get_status() {
        Ok((_, rules)) => rules,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to read firewall status: {}", e))
        }
    };

    let target = rules.iter().find(|r| {
        r.port == body.port
            && r.protocol.eq_ignore_ascii_case(&body.protocol)
            && r.action.split_whitespace().next() == Some(body.action.to_uppercase().as_str())
    });

    if let Some(rule) = target {
        if rule.is_mandatory {
            return ResponseHelper::json_forbidden(
                "This rule keeps SSH/HomeVault access open and cannot be removed".to_string(),
            );
        }
    }

    if let Err(e) = firewall::delete_rule_by_spec(&body.port, &body.protocol, &body.action) {
        return ResponseHelper::json_error(format!("Failed to delete rule: {}", e));
    }

    let repo = FirewallRepository::new(state.db.pool());
    let description = format!("{} {}/{}", body.action, body.port, body.protocol);
    let _ = repo
        .record_change("rule_deleted", Some(&description), &username)
        .await;
    crate::events::log_event(format!(
        "Admin '{}' deleted firewall rule: {}",
        username, description
    ));

    ResponseHelper::json_success(serde_json::json!({ "ok": true }))
}
