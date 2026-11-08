/* =============================================================================
 * File          : audit.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin endpoints for the queryable audit trail (P1-18): paged listing with
 *   filters and a CSV/JSON export. The table is written by `crate::audit`
 *   from `events::log_event`; this module only reads it.
 *
 * Dependencies:
 *   - axum (>= 0.8), serde (>= 1.0), chrono
 *
 * Usage:
 *   Routed under /api/audit in src/web/mod.rs.
 * =============================================================================
 */

//! Admin endpoints for the queryable audit trail.

use crate::audit::{AUDIT_EXPORT_MAX, AUDIT_PAGE_MAX, AUDIT_RETENTION_DAYS, AuditQuery};
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

/// `GET /api/audit` — one page, newest first.
pub async fn handle_audit_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let pool = state.db.pool();
    match crate::audit::list(pool.as_ref(), &query, AUDIT_PAGE_MAX).await {
        Ok(page) => ResponseHelper::json_success(serde_json::json!({
            "entries": page.entries,
            "total": page.total,
            "limit": page.limit,
            "offset": page.offset,
            "retention_days": AUDIT_RETENTION_DAYS,
        })),
        Err(message) => ResponseHelper::json_error(message),
    }
}

/// Export query: the same filters plus the output format.
#[derive(Debug, Default, Deserialize)]
pub struct AuditExportQuery {
    #[serde(flatten)]
    pub audit: AuditQuery,
    /// `json` (default) or `csv`.
    pub format: Option<String>,
}

/// `GET /api/audit/export` — download the filtered trail as JSON or CSV.
pub async fn handle_audit_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<AuditExportQuery>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let format = query
        .format
        .as_deref()
        .unwrap_or("json")
        .trim()
        .to_ascii_lowercase();
    if format != "json" && format != "csv" {
        return ResponseHelper::json_error("format must be 'json' or 'csv'".to_string());
    }

    let pool = state.db.pool();
    let page = match crate::audit::list(pool.as_ref(), &query.audit, AUDIT_EXPORT_MAX).await {
        Ok(page) => page,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut response_headers = HeaderMap::new();
    if format == "csv" {
        response_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/csv; charset=utf-8"),
        );
        let disposition = format!("attachment; filename=\"homevault-audit-{stamp}.csv\"");
        response_headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition)
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        (response_headers, crate::audit::to_csv(&page.entries)).into_response()
    } else {
        response_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        let disposition = format!("attachment; filename=\"homevault-audit-{stamp}.json\"");
        response_headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&disposition)
                .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
        );
        let body = serde_json::json!({
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "retention_days": AUDIT_RETENTION_DAYS,
            "total": page.total,
            "entries": page.entries,
        });
        (response_headers, body.to_string()).into_response()
    }
}
