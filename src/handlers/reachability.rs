/* =============================================================================
 * File          : reachability.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The reachability advisor endpoint: one read-only report an admin can open
 *   before (or after) exposing HomeVault to the internet.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::reachability
 *
 * Usage:
 *   `GET /api/settings/reachability`; see docs/guides/reachability.md.
 * =============================================================================
 */

//! `GET /api/settings/reachability` (admin-only).

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response};

use crate::handlers::admin_guard::require_admin;
use crate::reachability;
use crate::utils::ResponseHelper;

use super::AppState;

/// Payload: the checks, the worst state and the counts.
pub fn report_payload(checks: &[reachability::Check]) -> serde_json::Value {
    let (ok, warn, fail) = reachability::counts(checks);
    serde_json::json!({
        "checks": checks,
        "state": reachability::worst(checks).as_str(),
        "counts": { "ok": ok, "warn": warn, "fail": fail },
    })
}

/// `GET /api/settings/reachability`
pub async fn handle_reachability_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let checks = reachability::current_report();
    ResponseHelper::json_success(report_payload(&checks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_reports_the_worst_state_and_the_counts() {
        let checks = vec![
            reachability::Check {
                id: "a",
                state: reachability::CheckState::Ok,
                detail: "fine".to_string(),
                fix: None,
            },
            reachability::Check {
                id: "b",
                state: reachability::CheckState::Warn,
                detail: "meh".to_string(),
                fix: Some("do it".to_string()),
            },
        ];
        let payload = report_payload(&checks);
        assert_eq!(payload["state"], "warn");
        assert_eq!(payload["counts"]["ok"], 1);
        assert_eq!(payload["counts"]["fail"], 0);
        assert_eq!(payload["checks"][1]["fix"], "do it");
    }
}
