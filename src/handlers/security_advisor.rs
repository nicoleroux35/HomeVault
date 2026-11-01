/* =============================================================================
 * File          : security_advisor.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The Security Advisor endpoint: one read-only answer to "is this box safely
 *   configured?". Admin-only — the report names the accounts, the firewall
 *   state, the certificate and the shares, which is the security posture of
 *   the appliance, not public information.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::security_advisor, crate::release_notes
 *
 * Usage:
 *   Route /api/security/advisor in src/web/mod.rs; see docs/guides/health.md.
 * =============================================================================
 */

//! Security Advisor endpoint (admin-only), see docs/guides/health.md.
//!
//! * `GET /api/security/advisor` — run every check now and answer the report.
//!
//! There is deliberately **no** write route: the advisor never changes a
//! setting. Every fix is a deep link to the screen that owns the setting, so
//! the report can never disagree with the card it points at.
//!
//! Nothing is cached: a finding disappears on the very next run once the
//! setting is fixed, which is the acceptance criterion this endpoint exists to
//! satisfy.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response};
use chrono::Utc;

use crate::handlers::admin_guard::require_admin;
use crate::security_advisor;
use crate::utils::ResponseHelper;

use super::AppState;

/// `GET /api/security/advisor`
pub async fn handle_security_advisor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let facts = security_advisor::collect(&state).await;
    let report = security_advisor::evaluate(
        &facts,
        &Utc::now().to_rfc3339(),
        crate::release_notes::current_version(),
    );

    ResponseHelper::json_success(report)
}
