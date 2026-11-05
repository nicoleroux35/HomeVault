/* =============================================================================
 * File          : password_policy.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin endpoints for the global password-expiry policy: read and save
 *   `expiry_days` (`0` = never). The enforcement itself lives in
 *   `AuthService::enforce_password_expiry`, called at sign-in.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::storage::PasswordPolicyRepository,
 *     crate::handlers::admin_guard
 *
 * Usage:
 *   Routes /api/settings/password-policy in src/web/mod.rs; Settings →
 *   Security card; see docs/guides/security.md.
 * =============================================================================
 */

//! Password-expiry policy endpoints (admin-only).

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response, Json};
use serde::{Deserialize, Serialize};

use crate::handlers::admin_guard::require_admin;
use crate::storage::{PasswordPolicyRepository, MAX_EXPIRY_DAYS};
use crate::utils::ResponseHelper;

use super::AppState;

#[derive(Debug, Serialize)]
pub struct PasswordPolicyView {
    pub expiry_days: i64,
    pub max_expiry_days: i64,
}

#[derive(Debug, Deserialize)]
pub struct PasswordPolicyUpdate {
    pub expiry_days: i64,
}

pub async fn handle_password_policy_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo = PasswordPolicyRepository::new(state.db.pool());
    match repo.get_expiry_days().await {
        Ok(expiry_days) => ResponseHelper::json_success(PasswordPolicyView {
            expiry_days,
            max_expiry_days: MAX_EXPIRY_DAYS,
        }),
        Err(e) => {
            ResponseHelper::json_internal_error(format!("Failed to read the password policy: {e}"))
        }
    }
}

pub async fn handle_password_policy_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PasswordPolicyUpdate>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    // Reject rather than clamp: a client asking for 10 000 days made a
    // mistake, and silently storing 3 650 would hide it.
    if body.expiry_days < 0 || body.expiry_days > MAX_EXPIRY_DAYS {
        return ResponseHelper::json_error(format!(
            "expiry_days must be between 0 and {MAX_EXPIRY_DAYS}"
        ));
    }

    let repo = PasswordPolicyRepository::new(state.db.pool());
    match repo.set_expiry_days(body.expiry_days).await {
        Ok(()) => ResponseHelper::json_success(PasswordPolicyView {
            expiry_days: body.expiry_days,
            max_expiry_days: MAX_EXPIRY_DAYS,
        }),
        Err(e) => {
            ResponseHelper::json_internal_error(format!("Failed to save the password policy: {e}"))
        }
    }
}
