/* =============================================================================
 * File          : reverse_proxy.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Endpoints for the generic reverse proxy: list/create/update/delete rules,
 *   plus the dashboard-side auth entry `GET /proxy-auth` that hands the
 *   listener a one-time ticket. Reads need a session; every mutation is
 *   admin-only, because a rule publishes a LAN service under a host name.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::reverse_proxy, crate::app_proxy::origin
 *
 * Usage:
 *   Routes under /api/proxy/rules and /proxy-auth in src/web/mod.rs; see
 *   docs/guides/reverse-proxy.md.
 * ============================================================================= */

//! The generic reverse proxy's API and its browser auth entry.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header::LOCATION, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

use crate::app_proxy::cookie::session_from_cookie_header;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::reverse_proxy::proxy::{safe_next_path, ticket_callback_location};
use crate::reverse_proxy::rules;
use crate::reverse_proxy::ticket::TICKETS;
use crate::reverse_proxy::{
    resolve_upstream, revoke_rule_credentials, validate_input, ProxyRuleInput, Upstream, MAX_RULES,
    REVERSE_PROXY,
};
use crate::utils::ResponseHelper;

fn q(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// GET /api/proxy/rules — every rule plus the listener's state.
pub async fn handle_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let pool = state.db.pool();
    let rules = match rules::list(&pool).await {
        Ok(rules) => rules,
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };
    let listener = REVERSE_PROXY.status().await;
    ResponseHelper::json_success(serde_json::json!({
        "rules": rules,
        "listener": listener,
        "max_rules": MAX_RULES,
    }))
}

/// POST /api/proxy/rules — validate, resolve the upstream, persist, reload.
pub async fn handle_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let input: ProxyRuleInput = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(e) => {
            return ResponseHelper::json_error(format!("Malformed request body: {e}"));
        }
    };
    let rule = match validate_input(&input) {
        Ok(rule) => rule,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let pool = state.db.pool();
    match rules::list(&pool).await {
        Ok(existing) if existing.len() >= MAX_RULES => {
            return ResponseHelper::json_error(format!(
                "A home network does not need more than {MAX_RULES} proxy rules."
            ));
        }
        Ok(_) => {}
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    }
    match rules::host_path_taken(&pool, &rule.host, &rule.path, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(
                "A rule for that host and path already exists.".to_string(),
            );
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    }
    if let Err(e) = check_upstream(&rule.upstream).await {
        return ResponseHelper::json_error(e);
    }

    match rules::create(&pool, &rule).await {
        Ok(created) => {
            if let Err(e) = REVERSE_PROXY.reload(&pool).await {
                tracing::warn!("reverse proxy: {e}");
            }
            crate::events::log_event(format!(
                "Admin '{admin}' added reverse-proxy rule {} -> {} (gate: {})",
                display_host(&created),
                created.upstream,
                if created.auth_required { "on" } else { "off" }
            ));
            ResponseHelper::json_success(serde_json::json!({ "rule": created }))
        }
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// PUT /api/proxy/rules/{id} — replace one rule's mutable fields.
pub async fn handle_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let input: ProxyRuleInput = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(e) => {
            return ResponseHelper::json_error(format!("Malformed request body: {e}"));
        }
    };
    let rule = match validate_input(&input) {
        Ok(rule) => rule,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let pool = state.db.pool();
    match rules::get(&pool, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ResponseHelper::json_error_with_status(
                StatusCode::NOT_FOUND,
                "No proxy rule with that id.".to_string(),
            );
        }
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    }
    match rules::host_path_taken(&pool, &rule.host, &rule.path, Some(id)).await {
        Ok(true) => {
            return ResponseHelper::json_error(
                "Another rule already uses that host and path.".to_string(),
            );
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    }
    if let Err(e) = check_upstream(&rule.upstream).await {
        return ResponseHelper::json_error(e);
    }

    match rules::update(&pool, id, &rule).await {
        Ok(Some(updated)) => {
            if let Err(e) = REVERSE_PROXY.reload(&pool).await {
                tracing::warn!("reverse proxy: {e}");
            }
            crate::events::log_event(format!(
                "Admin '{admin}' changed reverse-proxy rule {} -> {} (gate: {})",
                display_host(&updated),
                updated.upstream,
                if updated.auth_required { "on" } else { "off" }
            ));
            ResponseHelper::json_success(serde_json::json!({ "rule": updated }))
        }
        Ok(None) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No proxy rule with that id.".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

/// DELETE /api/proxy/rules/{id} — remove the rule and every credential it
/// minted.
pub async fn handle_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(resp) => return resp,
    };
    let pool = state.db.pool();
    let existing = match rules::get(&pool, id).await {
        Ok(Some(rule)) => rule,
        Ok(None) => {
            return ResponseHelper::json_error_with_status(
                StatusCode::NOT_FOUND,
                "No proxy rule with that id.".to_string(),
            );
        }
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };
    match rules::delete(&pool, id).await {
        Ok(true) => {
            revoke_rule_credentials(id);
            if let Err(e) = REVERSE_PROXY.reload(&pool).await {
                tracing::warn!("reverse proxy: {e}");
            }
            crate::events::log_event(format!(
                "Admin '{admin}' removed reverse-proxy rule {}",
                display_host(&existing)
            ));
            ResponseHelper::json_success(serde_json::json!({ "id": id }))
        }
        Ok(false) => ResponseHelper::json_error_with_status(
            StatusCode::NOT_FOUND,
            "No proxy rule with that id.".to_string(),
        ),
        Err(e) => ResponseHelper::json_internal_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ProxyAuthQuery {
    /// The rule the gate was serving.
    rule: i64,
    /// The path to return to, validated again before it is used.
    #[serde(default)]
    next: Option<String>,
}

/// GET /proxy-auth — the dashboard side of the cross-host hand-off.
///
/// This is the **only** place the generic proxy reads the real API session.
/// It mints a single-use ticket bound to one rule and redirects to that rule's
/// own origin, where the listener exchanges it for a rule-scoped session.
pub async fn handle_proxy_auth(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ProxyAuthQuery>,
    headers: HeaderMap,
) -> Response {
    let next = safe_next_path(query.next.as_deref().unwrap_or("/"));

    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(session_from_cookie_header)
        .map(str::to_string);

    let username = match cookie {
        Some(token) => state.auth.get_username_from_token(&token).await,
        None => None,
    };
    let Some(username) = username else {
        let next = format!("/proxy-auth?rule={}&next={}", query.rule, q(&next));
        return redirect(&format!("/login?next={}", q(&next)), None);
    };
    if state.auth.get_user(&username).await.is_err() {
        return redirect("/login", None);
    }

    let pool = state.db.pool();
    let rule = match rules::get(&pool, query.rule).await {
        Ok(Some(rule)) if rule.enabled => rule,
        Ok(_) => return (StatusCode::NOT_FOUND, "No such proxy rule.").into_response(),
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };

    let listener = REVERSE_PROXY.status().await;
    if !listener.listening {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "The reverse proxy is not listening on port {}. {}",
                listener.port,
                listener
                    .error
                    .unwrap_or_else(|| "Check the port and permissions.".to_string())
            ),
        )
            .into_response();
    }

    let ticket = TICKETS.mint(rule.id, &username, &next);
    let location = ticket_callback_location(&rule.host, listener.port, rule.id, &ticket, &next);
    redirect(&location, Some("no-referrer"))
}

fn redirect(location: &str, referrer_policy: Option<&str>) -> Response {
    let mut response = (StatusCode::FOUND, "Redirecting").into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    if let Some(policy) = referrer_policy {
        if let Ok(value) = HeaderValue::from_str(policy) {
            response
                .headers_mut()
                .insert(axum::http::header::REFERRER_POLICY, value);
        }
    }
    response
}

/// Resolve the upstream once at save time, so the user learns immediately that
/// the address is public, unresolvable or a self-loop.
async fn check_upstream(upstream: &str) -> Result<(), String> {
    let parsed = Upstream::parse(upstream)?;
    let listen_port = REVERSE_PROXY.effective_listen_port().await;
    let local = REVERSE_PROXY.local_ips().await;
    resolve_upstream(&parsed, listen_port, REVERSE_PROXY.main_port(), &local)
        .await
        .map(|_| ())
}

fn display_host(rule: &crate::reverse_proxy::ProxyRule) -> String {
    if rule.path.is_empty() {
        rule.host.clone()
    } else {
        format!("{}{}", rule.host, rule.path)
    }
}
