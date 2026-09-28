//! Upload/download transfer quota management for the Users and Groups
//! pages — admin-configurable per-user or per-group caps on total data
//! uploaded/downloaded, distinct from the filesystem disk-space quota
//! system in `src/handlers/quotas.rs`. See
//! `src/storage/transfer_quota_repo.rs` for the storage layer.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::AppStateArc;
use crate::storage::transfer_quota_repo::TransferQuotaRepository;

fn valid_subject_type(subject_type: &str) -> bool {
    matches!(subject_type, "user" | "group")
}

/// `GET /api/transfer-quotas/{subject_type}/{subject_name}`
pub async fn handle_transfer_quota_get(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path((subject_type, subject_name)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    if !valid_subject_type(&subject_type) {
        return Json(json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }))
            .into_response();
    }

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo.get(&subject_type, &subject_name).await {
        Ok(quota) => Json(json!({ "success": true, "data": quota })).into_response(),
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
pub async fn handle_transfer_quotas_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(subject_type): Path<String>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    if !valid_subject_type(&subject_type) {
        return Json(json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }))
            .into_response();
    }

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo.list(&subject_type).await {
        Ok(quotas) => Json(json!({ "success": true, "data": quotas })).into_response(),
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
        return Json(json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }))
            .into_response();
    }
    if body.upload_limit_bytes < 0 || body.download_limit_bytes < 0 {
        return Json(json!({ "success": false, "error": "Quota limits cannot be negative" }))
            .into_response();
    }

    let repo = TransferQuotaRepository::new(state.db.pool());
    match repo
        .set_limits(
            &subject_type,
            &subject_name,
            body.upload_enabled,
            body.upload_limit_bytes,
            body.download_enabled,
            body.download_limit_bytes,
        )
        .await
    {
        Ok(quota) => {
            crate::events::log_event(format!(
                "Admin '{}' updated transfer quota for {} '{}' (upload: {}, download: {})",
                admin_username,
                subject_type,
                subject_name,
                if body.upload_enabled { format!("{} bytes", body.upload_limit_bytes) } else { "disabled".to_string() },
                if body.download_enabled { format!("{} bytes", body.download_limit_bytes) } else { "disabled".to_string() },
            ));
            Json(json!({ "success": true, "data": quota })).into_response()
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
        return Json(json!({ "success": false, "error": "subject_type must be 'user' or 'group'" }))
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
