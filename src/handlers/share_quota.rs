/* =============================================================================
 * File          : share_quota.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-share quota endpoints: read the ceiling and the live usage of one
 *   share, set or clear it (admin), and report whether the filesystem that
 *   holds the share can enforce one at all.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::share_quota, crate::shares, crate::storage
 *
 * Usage:
 *   Routes /api/shares/{name}/{share_type}/quota in src/web/mod.rs; see
 *   docs/guides/quotas.md.
 * =============================================================================
 */

//! Per-share quota endpoints (admin-only), see docs/guides/quotas.md.
//!
//! * `GET    /api/shares/{name}/{share_type}/quota` — state + usage + support
//! * `PUT    /api/shares/{name}/{share_type}/quota` — `{ quota_bytes }` (0 = none)
//! * `DELETE /api/shares/{name}/{share_type}/quota` — clear the limit

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;

use crate::handlers::admin_guard::require_admin;
use crate::share_quota::{self, ShareQuotaState};
use crate::shares::{ShareType, SharesService};
use crate::storage::ShareQuotaRepository;
use crate::utils::ResponseHelper;

use super::AppState;

/// The share's configured path, or a refusal naming the share.
async fn share_path(name: &str, share_type: ShareType) -> Result<String, String> {
    let name_owned = name.to_string();
    let resolved = tokio::task::spawn_blocking(move || {
        SharesService::get_share(&name_owned, share_type)
    })
    .await
    .map_err(|e| format!("the share lookup did not finish: {e}"))?;
    match resolved {
        Ok(share) => Ok(share.path),
        Err(_) => Err(format!("Share '{name}' does not exist")),
    }
}

/// The payload the Shares card renders: the state plus the intent, so the card
/// can show "10 GB of 40 GB" and hide the field when unsupported.
fn payload(state: &ShareQuotaState, name: &str, share_type: &str) -> serde_json::Value {
    serde_json::json!({
        "share_name": name,
        "share_type": share_type,
        "backend": state.backend,
        "supported": state.supported,
        "filesystem": state.filesystem,
        "reason": state.reason,
        "quota_bytes": state.quota_bytes,
        "used_bytes": state.used_bytes,
        "free_bytes": state.free_bytes,
        "percent": state.percent,
        "quota_human": share_quota::human_size(state.quota_bytes),
        "used_human": share_quota::human_size(state.used_bytes),
        "min_bytes": share_quota::MIN_QUOTA_BYTES,
        "max_bytes": share_quota::MAX_QUOTA_BYTES,
    })
}

async fn current_state(
    state: &Arc<AppState>,
    name: &str,
    share_type: ShareType,
    path: &str,
) -> Result<ShareQuotaState, String> {
    let repo = ShareQuotaRepository::new(state.db.pool());
    let quota = repo
        .quota(name, share_type.as_str())
        .await
        .map_err(|e| format!("could not read the saved quota: {e}"))?
        .unwrap_or(0);
    share_quota::inspect(path, quota).await
}

/// `GET /api/shares/{name}/{share_type}/quota`
pub async fn handle_share_quota_get(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let path = match share_path(&name, share_type.clone()).await {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };
    match current_state(&state, &name, share_type.clone(), &path).await {
        Ok(quota_state) => {
            ResponseHelper::json_success(payload(&quota_state, &name, share_type.as_str()))
        }
        Err(e) => ResponseHelper::json_error(e),
    }
}

#[derive(Debug, Deserialize)]
pub struct ShareQuotaRequest {
    /// The ceiling in bytes; `0` removes it.
    pub quota_bytes: u64,
}

/// `PUT /api/shares/{name}/{share_type}/quota`
///
/// The filesystem is changed **first**: the database only records an intent the
/// filesystem actually accepted, so the card can never claim a limit that is
/// not in force.
pub async fn handle_share_quota_set(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<ShareQuotaRequest>,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let quota_bytes = match share_quota::validate_quota(body.quota_bytes) {
        Ok(bytes) => bytes,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let path = match share_path(&name, share_type.clone()).await {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let backend = match share_quota::apply(&path, quota_bytes).await {
        Ok(backend) => backend,
        Err(e) => return ResponseHelper::json_error(format!("Could not set the quota: {e}")),
    };

    let repo = ShareQuotaRepository::new(state.db.pool());
    if let Err(e) = repo
        .set(&name, share_type.as_str(), quota_bytes)
        .await
    {
        return ResponseHelper::json_internal_error(format!(
            "The filesystem accepted the quota but it could not be saved: {e}"
        ));
    }

    crate::events::log_event(format!(
        "Admin '{}' set the quota of share '{}' ({}) to {} [{}]",
        username,
        name,
        share_type.as_str(),
        if quota_bytes == 0 {
            "no limit".to_string()
        } else {
            share_quota::human_size(quota_bytes)
        },
        backend.as_str()
    ));

    match current_state(&state, &name, share_type.clone(), &path).await {
        Ok(quota_state) => {
            ResponseHelper::json_success(payload(&quota_state, &name, share_type.as_str()))
        }
        Err(e) => ResponseHelper::json_error(format!("Saved, but could not re-read it: {e}")),
    }
}

/// `DELETE /api/shares/{name}/{share_type}/quota` — remove the ceiling.
pub async fn handle_share_quota_clear(
    State(state): State<Arc<AppState>>,
    Path((name, share_type)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let Some(share_type) = ShareType::parse(&share_type) else {
        return ResponseHelper::json_error("Invalid share type".to_string());
    };
    let path = match share_path(&name, share_type.clone()).await {
        Ok(path) => path,
        Err(e) => return ResponseHelper::json_error(e),
    };

    // Remove the limit before forgetting it: a row that says "no quota" while
    // the filesystem still enforces one would be the worst of both.
    if let Err(e) = share_quota::apply(&path, 0).await {
        // An unsupported filesystem has nothing to clear — that is not a
        // failure, the intent just goes away with the row.
        tracing::debug!("Share quota: nothing to clear on '{}': {e}", path);
    }
    let repo = ShareQuotaRepository::new(state.db.pool());
    if let Err(e) = repo.clear(&name, share_type.as_str()).await {
        return ResponseHelper::json_internal_error(format!(
            "The quota could not be cleared: {e}"
        ));
    }
    crate::events::log_event(format!(
        "Admin '{}' removed the quota of share '{}' ({})",
        username,
        name,
        share_type.as_str()
    ));

    match current_state(&state, &name, share_type.clone(), &path).await {
        Ok(quota_state) => {
            ResponseHelper::json_success(payload(&quota_state, &name, share_type.as_str()))
        }
        Err(e) => ResponseHelper::json_error(format!("Cleared, but could not re-read it: {e}")),
    }
}
