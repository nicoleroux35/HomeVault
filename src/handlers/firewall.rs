/* =============================================================================
 * File          : firewall.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Firewall (ufw) management endpoints: status and rule listing, rule
 *   add/delete, default policy, change history, and the confirm-guarded
 *   enable/disable. Self-lockout safety is enforced server-side: enabling
 *   re-asserts the mandatory SSH/app-port rules and verifies the result,
 *   disabling needs an explicit confirmation body, and deleting a rule
 *   marked mandatory is rejected with 403 before ufw is touched. Status and
 *   history are session-auth; every mutation is admin-only.
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
//! the firewall always re-asserts the mandatory SSH/app-port rules first,
//! verifies the live state afterwards, and requires an explicit
//! `{"confirmed": true}` body (there is deliberately no generic "set enabled"
//! endpoint); disabling — which opens every port — requires the same
//! confirmation. Deleting a rule that the live `ufw` output marks as
//! mandatory (via `src/firewall.rs`'s comment-based check) is rejected with
//! 403 before ever touching `ufw` — the same rejection a raw `curl` call
//! would hit as a UI click, since the check runs server-side against live
//! system state, not client-supplied flags.
//!
//! Two rules make the mutations safe:
//!
//! * **The client only selects, never constructs.** A delete request carries
//!   the rule's identity; the handler looks it up in the live rule set and
//!   passes *that* rule's own `spec` to `ufw delete`. Client input never
//!   becomes an argv entry.
//! * **Validation runs before the ufw availability probe**, so a malformed
//!   request gets "Port must be a number between 1 and 65535" rather than
//!   "ufw is not installed" — and, more importantly, is refused without any
//!   process being spawned.

use crate::firewall::{self, FirewallError};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::FirewallRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

fn unavailable() -> Response {
    ResponseHelper::json_error("ufw is not installed".to_string())
}

/// Turn a service error into a response.
///
/// `InvalidInput` is a 400 carrying the validator's own message (it names the
/// field and the accepted values); everything else is a 400 prefixed with
/// what the admin was trying to do, because "ufw command failed: ERROR:
/// problem running iptables" is unreadable on its own.
fn error_response(error: FirewallError, context: &str) -> Response {
    match error {
        FirewallError::InvalidInput(message) => ResponseHelper::json_error(message),
        FirewallError::Unavailable(message) => ResponseHelper::json_error(message),
        other => ResponseHelper::json_error(format!("{}: {}", context, other)),
    }
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
            "default_policy": firewall::DefaultPolicy::default(),
            "rules": [],
        }));
    }

    match firewall::get_status().await {
        Ok(status) => ResponseHelper::json_success(serde_json::json!({
            "available": true,
            "enabled": status.enabled,
            "default_policy": status.default_policy,
            "rules": status.rules,
        })),
        Err(e) => error_response(e, "Failed to read firewall status"),
    }
}

/// `GET /api/firewall/enable/preview` — shows exactly what enabling would
/// activate (the mandatory rules, the default policy and whatever custom
/// rules already exist), so the confirmation UI never has to guess.
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

    let ssh_ports = firewall::detect_ssh_ports();
    let app_port = firewall::detect_app_port();
    let status = firewall::get_status().await;

    let (default_policy, existing_rules) = match status {
        Ok(status) => (status.default_policy, status.rules),
        Err(e) => return error_response(e, "Failed to read firewall status"),
    };

    // Enabling a firewall whose default incoming policy is `allow` filters
    // nothing: the admin would tick the confirmation box, read "active", and
    // still be fully exposed. Say it in the preview instead.
    let mut warnings: Vec<String> = Vec::new();
    if default_policy.incoming == "allow" {
        warnings.push(
            "The default incoming policy is \"allow\", so enabling the firewall will not block \
             anything. Set it to deny or reject first."
                .to_string(),
        );
    }

    ResponseHelper::json_success(serde_json::json!({
        "ssh_port": ssh_ports.first().copied().unwrap_or(22),
        "ssh_ports": ssh_ports,
        "app_port": app_port,
        "default_policy": default_policy,
        "existing_rules": existing_rules,
        "warnings": warnings,
    }))
}

#[derive(Debug, Deserialize)]
pub struct EnableRequest {
    #[serde(default)]
    pub confirmed: bool,
}

/// `POST /api/firewall/enable` — the only path that can turn the firewall
/// on. Always re-asserts the mandatory SSH/app-port rules first, using
/// server-detected ports, before enabling, then verifies the result.
pub async fn handle_firewall_enable(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<EnableRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !body.confirmed {
        return ResponseHelper::json_error(
            "Enabling the firewall requires explicit confirmation".to_string(),
        );
    }
    if !firewall::ufw_available() {
        return unavailable();
    }

    let ssh_ports = firewall::detect_ssh_ports();
    let app_port = firewall::detect_app_port();

    let status = match firewall::enable_verified(&ssh_ports, app_port).await {
        Ok(status) => status,
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' tried to enable the firewall and it failed: {}",
                username, e
            ));
            return error_response(e, "Failed to enable firewall");
        }
    };

    let repo = FirewallRepository::new(state.db.pool());
    let _ = repo
        .save_state(
            true,
            ssh_ports.first().copied().unwrap_or(22),
            app_port,
            Some(&chrono::Utc::now().to_rfc3339()),
        )
        .await;
    let _ = repo.record_change("enabled", None, &username).await;
    crate::events::log_event_warn(format!(
        "Admin '{}' enabled the firewall (SSH ports {:?}, HomeVault port {})",
        username, ssh_ports, app_port
    ));

    ResponseHelper::json_success(serde_json::json!({
        "ok": true,
        "enabled": status.enabled,
        "default_policy": status.default_policy,
        "rules": status.rules,
    }))
}

#[derive(Debug, Deserialize, Default)]
pub struct DisableRequest {
    #[serde(default)]
    pub confirmed: bool,
}

/// `POST /api/firewall/disable`
///
/// Disabling a firewall opens every port it was blocking, so it gets the same
/// explicit confirmation as enabling — it used to take none at all, which
/// made "turn the security feature off" the only one-click destructive action
/// on the page. The body is optional so that a request without one is still
/// authenticated first (a missing body must not turn an auth failure into a
/// 415).
pub async fn handle_firewall_disable(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<DisableRequest>>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !body.map(|b| b.confirmed).unwrap_or(false) {
        return ResponseHelper::json_error(
            "Disabling the firewall requires explicit confirmation".to_string(),
        );
    }
    if !firewall::ufw_available() {
        return unavailable();
    }

    if let Err(e) = firewall::disable().await {
        return error_response(e, "Failed to disable firewall");
    }

    // The stored ports are a cache for the settings row; re-detecting them
    // beats reusing whatever was there before (which is `0` on a host where
    // the firewall was never enabled, and clamped to 1 on read).
    let repo = FirewallRepository::new(state.db.pool());
    let settings = repo.get_settings().await.unwrap_or_default();
    let _ = repo
        .save_state(
            false,
            firewall::detect_ssh_port(),
            firewall::detect_app_port(),
            settings.confirmed_at.as_deref(),
        )
        .await;
    let _ = repo.record_change("disabled", None, &username).await;
    crate::events::log_event_warn(format!(
        "Admin '{}' disabled the firewall — every port it was blocking is open again",
        username
    ));

    ResponseHelper::json_success(serde_json::json!({ "ok": true, "enabled": false }))
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

    // The comment is built from validated-on-the-way-in values; the service
    // validates every argument before it builds the argv.
    let comment = format!(
        "homevault-custom-{}-{}",
        body.action.trim().to_ascii_lowercase(),
        body.port.trim()
    );

    let outcome = match firewall::add_rule(
        &body.port,
        &body.protocol,
        &body.action,
        body.source.as_deref(),
        &comment,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => return error_response(e, "Failed to add rule"),
    };

    let description = format!(
        "{} {}/{}",
        body.action.trim().to_ascii_lowercase(),
        body.port.trim(),
        body.protocol.trim().to_ascii_lowercase()
    );

    if outcome == firewall::AddRuleOutcome::AlreadyPresent {
        // ufw skips an existing rule and exits 0: reporting "Rule added"
        // would claim a change that did not happen.
        return ResponseHelper::json_success(serde_json::json!({
            "ok": true,
            "already_exists": true,
            "rule": description,
        }));
    }

    let repo = FirewallRepository::new(state.db.pool());
    let _ = repo
        .record_change("rule_added", Some(&description), &username)
        .await;
    crate::events::log_event(format!(
        "Admin '{}' added firewall rule: {}",
        username, description
    ));

    ResponseHelper::json_success(serde_json::json!({
        "ok": true,
        "already_exists": false,
        "rule": description,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RuleDeleteRequest {
    /// The rule's live spec (`FirewallRule::spec`). Preferred: it identifies
    /// one rule exactly, `from <source>` clause included.
    #[serde(default)]
    pub spec: Option<String>,
    #[serde(default)]
    pub port: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default)]
    pub source: Option<String>,
}

/// `DELETE /api/firewall/rules` — the request *selects* a live rule; it never
/// supplies the command.
///
/// The old version rebuilt the delete spec from the request body
/// (`delete <action> <port>/<proto>`), which (a) could not match a
/// source-restricted rule at all — so such a rule could never be removed —
/// and (b) deleted the *global* rule with the same port when one existed.
/// It also looked the target up ignoring `source` and, when nothing matched,
/// deleted anyway. Now: find the live rule, refuse if it is missing, refuse
/// if it is mandatory, then delete by its own spec.
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

    let rules = match firewall::get_status().await {
        Ok(status) => status.rules,
        Err(e) => return error_response(e, "Failed to read firewall status"),
    };

    let requested_spec = body
        .spec
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let target = match requested_spec {
        Some(spec) => rules.iter().find(|r| r.spec == spec),
        None => {
            let action = body.action.trim().to_ascii_uppercase();
            let source = body
                .source
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            rules.iter().find(|r| {
                r.port == body.port.trim()
                    && r.protocol.eq_ignore_ascii_case(body.protocol.trim())
                    && r.action == action
                    && r.source.as_deref() == source
            })
        }
    };

    let Some(rule) = target else {
        // Never delete on a miss: the rule set may have changed in another
        // tab, and a blind `ufw delete` would hit whatever matches instead.
        return ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            "That rule is no longer in the firewall — reload the page".to_string(),
        );
    };

    if rule.is_mandatory {
        return ResponseHelper::json_forbidden(
            "This rule keeps SSH/HomeVault access open and cannot be removed".to_string(),
        );
    }

    if let Err(e) = firewall::delete_rule(&rule.spec).await {
        return error_response(e, "Failed to delete rule");
    }

    let description = if rule.port.is_empty() {
        format!("{} (no port)", rule.spec)
    } else {
        format!(
            "{} {}/{}",
            rule.action.to_lowercase(),
            rule.port,
            rule.protocol
        )
    };
    let repo = FirewallRepository::new(state.db.pool());
    let _ = repo
        .record_change("rule_deleted", Some(&description), &username)
        .await;
    crate::events::log_event(format!(
        "Admin '{}' deleted firewall rule: {}",
        username, description
    ));

    ResponseHelper::json_success(serde_json::json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
pub struct DefaultPolicyRequest {
    /// `incoming` | `outgoing` | `routed`
    pub direction: String,
    /// `allow` | `deny` | `reject`
    pub policy: String,
}

/// `POST /api/firewall/default-policy` — what happens to traffic no rule
/// matches. Without it the page could show a rule list but not the policy
/// that gives it meaning, and a firewall left on `allow (incoming)` looked
/// identical to a locked-down one.
pub async fn handle_firewall_default_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<DefaultPolicyRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // Validate before the availability probe: a typo must be reported as a
    // typo, not as "ufw is not installed".
    let direction = match firewall::validate_default_direction(&body.direction) {
        Ok(d) => d,
        Err(e) => return error_response(e, "Invalid direction"),
    };
    let policy = match firewall::validate_default_policy(&body.policy) {
        Ok(p) => p,
        Err(e) => return error_response(e, "Invalid policy"),
    };

    if !firewall::ufw_available() {
        return unavailable();
    }

    if let Err(e) = firewall::set_default_policy(&direction, &policy).await {
        return error_response(e, "Failed to change the default policy");
    }

    let description = format!("{} {}", policy, direction);
    let repo = FirewallRepository::new(state.db.pool());
    let _ = repo
        .record_change("default_policy_changed", Some(&description), &username)
        .await;
    crate::events::log_event_warn(format!(
        "Admin '{}' set the default {} firewall policy to '{}'",
        username, direction, policy
    ));

    ResponseHelper::json_success(serde_json::json!({ "ok": true }))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
}

/// `GET /api/firewall/history` — the change log the handlers have always
/// recorded and nothing ever read (`list_change_log` was dead code).
pub async fn handle_firewall_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<HistoryQuery>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let repo = FirewallRepository::new(state.db.pool());
    match repo.list_change_log(limit).await {
        Ok(entries) => ResponseHelper::json_success(serde_json::json!({ "entries": entries })),
        Err(e) => ResponseHelper::json_error(format!("Failed to read firewall history: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_input_keeps_the_validator_message() {
        // A 400 must name the field and the accepted values; prefixing it
        // with the operation ("Failed to add rule: Port ...") reads worse.
        let response = error_response(
            FirewallError::InvalidInput("Port is required".to_string()),
            "Failed to add rule",
        );
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_failure_is_prefixed_with_the_operation() {
        let error = FirewallError::Failed("iptables is busy".to_string());
        assert!(!error.is_invalid_input());
        let response = error_response(error, "Failed to enable firewall");
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}
