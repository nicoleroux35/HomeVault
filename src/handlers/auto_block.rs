/* =============================================================================
 * File          : auto_block.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Security-card endpoints for the login auto-block: read/save the policy,
 *   list the blocked addresses with their remaining time, lift one, lift all,
 *   and expire everything that is due right now.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::auto_block, crate::storage::IpBanRepository
 *
 * Usage:
 *   Routes /api/security/auto-block* in src/web/mod.rs; see
 *   docs/guides/security.md.
 * =============================================================================
 */

//! Login auto-block endpoints (admin-only), see docs/guides/security.md.
//!
//! * `GET    /api/security/auto-block`        — policy + bans + counters
//! * `PUT    /api/security/auto-block`        — save the policy (validated)
//! * `POST   /api/security/auto-block/pardon` — lift one address `{ ip }`
//! * `POST   /api/security/auto-block/clear`  — lift every ban
//! * `POST   /api/security/auto-block/sweep`  — expire what is due now

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};
use chrono::Utc;
use serde::Deserialize;

use crate::auto_block::{self, AutoBlockSettings};
use crate::handlers::admin_guard::require_admin;
use crate::storage::IpBanRepository;
use crate::utils::ResponseHelper;

use super::AppState;

/// One ban, as the card renders it: the stored row plus how long is left.
fn ban_payload(ban: &crate::auto_block::IpBan, now: &str) -> serde_json::Value {
    let remaining = ban.remaining_seconds(now);
    serde_json::json!({
        "ip": ban.ip,
        "reason": ban.reason,
        "failures": ban.failures,
        "created_at": ban.created_at,
        "expires_at": ban.expires_at,
        "firewall_rule": ban.firewall_rule,
        "active": ban.is_active(now),
        "remaining_seconds": remaining,
        "remaining_minutes": auto_block::remaining_minutes(remaining),
    })
}

async fn status_payload(state: &Arc<AppState>) -> Result<serde_json::Value, String> {
    let now = Utc::now().to_rfc3339();
    let repo = IpBanRepository::new(state.db.pool());
    let status = repo.status(&now).await.map_err(|e| e.to_string())?;
    let mut bans: Vec<serde_json::Value> = status
        .bans
        .iter()
        .map(|ban| ban_payload(ban, &now))
        .collect();
    // Active first (the ones an admin can act on), then expired ones by date.
    bans.sort_by_key(|ban| {
        (
            !ban["active"].as_bool().unwrap_or(false),
            ban["expires_at"].as_str().unwrap_or("").to_string(),
        )
    });
    Ok(serde_json::json!({
        "settings": status.settings,
        "bans": bans,
        "active": status.active,
        "firewall_rules": status.firewall_rules,
        "firewall_available": crate::firewall::ufw_available(),
        "limits": {
            "min_threshold": auto_block::MIN_THRESHOLD,
            "max_threshold": auto_block::MAX_THRESHOLD,
            "max_window_seconds": auto_block::MAX_WINDOW_SECONDS,
            "max_ban_seconds": auto_block::MAX_BAN_SECONDS,
            "max_dos_requests_per_minute": auto_block::MAX_DOS_REQUESTS_PER_MINUTE,
        },
        "now": now,
    }))
}

/// `GET /api/security/auto-block`
pub async fn handle_auto_block_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    match status_payload(&state).await {
        Ok(payload) => ResponseHelper::json_success(payload),
        Err(e) => ResponseHelper::json_error(format!("Failed to read the auto-block state: {e}")),
    }
}

/// Request body for the policy. Flat and explicit, so a client cannot write a
/// field the server owns (there is none today, and this keeps it that way).
#[derive(Debug, Deserialize)]
pub struct AutoBlockSettingsRequest {
    pub enabled: Option<bool>,
    pub threshold: Option<u32>,
    pub window_seconds: Option<u32>,
    pub ban_seconds: Option<u32>,
    pub apply_firewall: Option<bool>,
    pub dos_enabled: Option<bool>,
    pub dos_requests_per_minute: Option<u32>,
}

/// `PUT /api/security/auto-block`
pub async fn handle_auto_block_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AutoBlockSettingsRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let repo = IpBanRepository::new(state.db.pool());
    let stored = match repo.settings().await {
        Ok(settings) => settings,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to read the saved policy: {e}"))
        }
    };
    let mut next = AutoBlockSettings {
        enabled: body.enabled.unwrap_or(stored.enabled),
        threshold: body.threshold.unwrap_or(stored.threshold),
        window_seconds: body.window_seconds.unwrap_or(stored.window_seconds),
        ban_seconds: body.ban_seconds.unwrap_or(stored.ban_seconds),
        apply_firewall: body.apply_firewall.unwrap_or(stored.apply_firewall),
        dos_enabled: body.dos_enabled.unwrap_or(stored.dos_enabled),
        dos_requests_per_minute: body
            .dos_requests_per_minute
            .unwrap_or(stored.dos_requests_per_minute),
    };
    // Validate the raw values first (so an out-of-range number is a refusal, not
    // a silent clamp), then store the clamped form.
    if let Err(e) = next.validate() {
        return ResponseHelper::json_error(e);
    }
    next = next.clamped();
    if let Err(e) = repo.save_settings(&next).await {
        return ResponseHelper::json_internal_error(format!(
            "Failed to save the auto-block policy: {e}"
        ));
    }

    crate::events::log_event(format!(
        "Admin '{}' updated the login auto-block (enabled={}, {} failures in {}s → {}s ban, \
         firewall={}, DoS cap={})",
        username,
        next.enabled,
        next.threshold,
        next.window_seconds,
        next.ban_seconds,
        next.apply_firewall,
        if next.dos_enabled {
            format!("{}/min", next.dos_requests_per_minute)
        } else {
            "off".to_string()
        }
    ));

    match status_payload(&state).await {
        Ok(payload) => ResponseHelper::json_success(serde_json::json!({ "state": payload })),
        Err(e) => ResponseHelper::json_error(format!("Saved, but could not re-read it: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct PardonRequest {
    pub ip: String,
}

/// `POST /api/security/auto-block/pardon`
///
/// Lifts one address: the row and (when it was installed) its firewall rule.
/// A rule that cannot be removed is reported, because a leftover `ufw deny` is
/// invisible otherwise — the address stays refused with no explanation on the
/// card.
pub async fn handle_auto_block_pardon(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PardonRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let ip = body.ip.trim().to_string();
    if ip.is_empty() {
        return ResponseHelper::json_error("An address is required".to_string());
    }
    match auto_block::pardon(&state, &ip).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{}' pardoned the login ban on '{}'",
                username, ip
            ));
            match status_payload(&state).await {
                Ok(payload) => {
                    ResponseHelper::json_success(serde_json::json!({ "state": payload }))
                }
                Err(e) => {
                    ResponseHelper::json_error(format!("Lifted, but could not re-read it: {e}"))
                }
            }
        }
        Ok(false) => ResponseHelper::json_error(format!("'{ip}' is not blocked")),
        Err(reason) => ResponseHelper::json_error(reason),
    }
}

/// `POST /api/security/auto-block/clear`
pub async fn handle_auto_block_clear(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match auto_block::pardon_all(&state).await {
        Ok(lifted) => {
            if lifted > 0 {
                crate::events::log_event(format!(
                    "Admin '{}' pardoned every login ban ({lifted})",
                    username
                ));
            }
            match status_payload(&state).await {
                Ok(payload) => ResponseHelper::json_success(serde_json::json!({
                    "lifted": lifted,
                    "state": payload,
                })),
                Err(e) => {
                    ResponseHelper::json_error(format!("Lifted, but could not re-read it: {e}"))
                }
            }
        }
        Err(reason) => ResponseHelper::json_error(reason),
    }
}

/// `POST /api/security/auto-block/sweep`
///
/// Expire everything that is due now. The same sweep runs on every login
/// check; this endpoint exists so an admin can force it (and see the count)
/// without waiting for a check to happen.
pub async fn handle_auto_block_sweep(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = IpBanRepository::new(state.db.pool());
    let now = Utc::now().to_rfc3339();
    match auto_block::sweep_expired(&repo, &now).await {
        Ok(removed) => match status_payload(&state).await {
            Ok(payload) => ResponseHelper::json_success(serde_json::json!({
                "removed": removed,
                "state": payload,
            })),
            Err(e) => ResponseHelper::json_error(format!("Swept, but could not re-read it: {e}")),
        },
        Err(e) => ResponseHelper::json_error(format!("Failed to expire the due bans: {e}")),
    }
}
