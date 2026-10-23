/* =============================================================================
 * File          : transfer_quotas.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Upload/download transfer quota management for the Users and Groups
 *   pages — admin-configurable per-user or per-group caps on total data
 *   uploaded/downloaded, distinct from the filesystem disk-space quota
 *   system in src/handlers/quotas.rs. Reads are self-or-admin; set/delete
 *   are admin-only. Backs the read-only Quotas app.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/transfer-quotas in src/web/mod.rs; storage by
 *   TransferQuotaRepository.
 * =============================================================================
 */

//! Upload/download transfer quota management for the Users and Groups
//! pages — admin-configurable per-user or per-group caps on total data
//! uploaded/downloaded, distinct from the filesystem disk-space quota
//! system in `src/handlers/quotas.rs`. See
//! `src/storage/transfer_quota_repo.rs` for the storage layer.
//!
//! **Authorisation is enforced here, not in the screen** (ROADMAP 23.3): the
//! list endpoint is admin-only, and a single subject's quota is readable by
//! that subject or by an admin — never by another household user, who would
//! otherwise read somebody else's limits *and* consumed bytes. `Quotas.svelte`
//! still redirects non-admins, but that redirect is a courtesy, not the
//! boundary.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use crate::handlers::AppStateArc;
use crate::storage::transfer_quota_repo::{QuotaLimits, TransferQuotaRepository};
use crate::transfer_quota::{self, Period};

fn valid_subject_type(subject_type: &str) -> bool {
    matches!(subject_type, "user" | "group")
}

/// Serialises one row, adding the computed "next reset" instant the UI shows.
fn quota_json(quota: &crate::storage::transfer_quota_repo::TransferQuota) -> serde_json::Value {
    let mut quota = quota.clone();
    transfer_quota::fill_resets_at(&mut quota, Utc::now(), transfer_quota::appliance_timezone());
    serde_json::to_value(quota).unwrap_or_else(|_| json!(null))
}

/// `GET /api/transfer-quotas/{subject_type}/{subject_name}`
///
/// A regular user may read **their own** quota (so a client can warn before a
/// transfer is refused); everything else needs an admin.
pub async fn handle_transfer_quota_get(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path((subject_type, subject_name)): Path<(String, String)>,
) -> Response {
    let caller = match crate::handlers::admin_guard::require_auth(&state, &headers).await {
        Ok(caller) => caller,
        Err(resp) => return resp,
    };

    if !valid_subject_type(&subject_type) {
        return Json(
            json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }),
        )
        .into_response();
    }

    let is_self = subject_type == "user" && caller == subject_name;
    if !is_self && !state.auth.is_admin(&caller).await {
        return crate::utils::ResponseHelper::json_forbidden(
            "You can only read your own transfer quota".to_string(),
        );
    }

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo.get(&subject_type, &subject_name).await {
        Ok(quota) => Json(json!({ "success": true, "data": quota_json(&quota) })).into_response(),
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Failed to load quota: {}", e)
        }))
        .into_response(),
    }
}

/// `GET /api/transfer-quotas/{subject_type}` — every configured quota for
/// that subject type, so the Users/Groups list views can show a quota
/// summary without one request per row.
///
/// Admin-only: this answers every subject's limits **and** their consumed
/// bytes, which is exactly the payload ROADMAP 23.3 closes.
pub async fn handle_transfer_quotas_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(subject_type): Path<String>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    if !valid_subject_type(&subject_type) {
        return Json(
            json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }),
        )
        .into_response();
    }

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo.list(&subject_type).await {
        Ok(quotas) => {
            let data: Vec<serde_json::Value> = quotas.iter().map(quota_json).collect();
            Json(json!({ "success": true, "data": data })).into_response()
        }
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Failed to list quotas: {}", e)
        }))
        .into_response(),
    }
}

/// Request body for setting a subject's quota limits. Byte values are
/// computed client-side from the admin's chosen value + unit (MB/TB/PB) —
/// the backend only ever stores/validates plain bytes.
#[derive(Deserialize, Debug)]
pub struct SetTransferQuotaRequest {
    pub upload_enabled: bool,
    pub upload_limit_bytes: i64,
    pub download_enabled: bool,
    pub download_limit_bytes: i64,
    /// `lifetime` (default, so a client that predates counting periods keeps
    /// the old behaviour) | `day` | `week` | `month`.
    #[serde(default = "default_period")]
    pub period: String,
}

fn default_period() -> String {
    Period::Lifetime.as_str().to_string()
}

/// `PUT /api/transfer-quotas/{subject_type}/{subject_name}`
pub async fn handle_transfer_quota_set(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path((subject_type, subject_name)): Path<(String, String)>,
    Json(body): Json<SetTransferQuotaRequest>,
) -> Response {
    let admin_username = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if !valid_subject_type(&subject_type) {
        return Json(
            json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }),
        )
        .into_response();
    }
    if body.upload_limit_bytes < 0 || body.download_limit_bytes < 0 {
        return Json(json!({ "success": false, "error": "Quota limits cannot be negative" }))
            .into_response();
    }
    let period = match Period::parse(&body.period) {
        Ok(period) => period,
        Err(e) => return Json(json!({ "success": false, "error": e })).into_response(),
    };

    // The window a saved quota is counted in opens at the current calendar
    // boundary (midnight / Monday / the 1st, in the appliance timezone), so
    // "100 GB this month" means this calendar month.
    let window_start = transfer_quota::current_window_start(
        Utc::now(),
        period,
        transfer_quota::appliance_timezone(),
    )
    .to_rfc3339();

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo
        .set_limits(QuotaLimits {
            subject_type: &subject_type,
            subject_name: &subject_name,
            upload_enabled: body.upload_enabled,
            upload_limit_bytes: body.upload_limit_bytes,
            download_enabled: body.download_enabled,
            download_limit_bytes: body.download_limit_bytes,
            period: period.as_str(),
            period_started_at: &window_start,
        })
        .await
    {
        Ok(quota) => {
            crate::events::log_event(format!(
                "Admin '{}' updated transfer quota for {} '{}' (upload: {}, download: {}, period: {})",
                admin_username,
                subject_type,
                subject_name,
                if body.upload_enabled {
                    format!("{} bytes", body.upload_limit_bytes)
                } else {
                    "disabled".to_string()
                },
                if body.download_enabled {
                    format!("{} bytes", body.download_limit_bytes)
                } else {
                    "disabled".to_string()
                },
                period.as_str(),
            ));
            Json(json!({ "success": true, "data": quota_json(&quota) })).into_response()
        }
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Failed to save quota: {}", e)
        }))
        .into_response(),
    }
}

/// `DELETE /api/transfer-quotas/{subject_type}/{subject_name}` — clears a
/// subject's quota row entirely (back to disabled/unlimited).
pub async fn handle_transfer_quota_delete(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path((subject_type, subject_name)): Path<(String, String)>,
) -> Response {
    let admin_username = match crate::handlers::admin_guard::require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if !valid_subject_type(&subject_type) {
        return Json(
            json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }),
        )
        .into_response();
    }

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo.delete(&subject_type, &subject_name).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' removed transfer quota for {} '{}'",
                admin_username, subject_type, subject_name
            ));
            Json(json!({ "success": true })).into_response()
        }
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Failed to remove quota: {}", e)
        }))
        .into_response(),
    }
}
