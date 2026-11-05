/* =============================================================================
 * File          : replication_witness.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Operational surface for the third-site replication witness (tasklist
 *   P2-15): report the configured endpoint and current lease, and let an
 *   administrator acquire/renew it explicitly.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::replication_witness
 *
 * Usage:
 *   Routes under /api/replication/witness in src/web/mod.rs; see
 *   docs/guides/replication.md.
 * =============================================================================
 */

//! Replication witness endpoints.

use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

use super::admin_guard::{require_admin, require_auth};
use super::AppState;
use crate::utils::ResponseHelper;

fn configured() -> Result<Option<crate::replication_witness::WitnessConfig>, String> {
    crate::replication_witness::witness_config_from_env(|key| std::env::var(key).ok())
}

/// GET /api/replication/witness — configuration + current lease (session).
pub async fn handle_witness_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let config = match configured() {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let Some(config) = config else {
        return ResponseHelper::json_success(serde_json::json!({
            "configured": false,
            "lease": null,
        }));
    };
    let client = match crate::replication_witness::WitnessClient::new(config.clone()) {
        Ok(client) => client,
        Err(e) => return ResponseHelper::json_error(e),
    };
    match client.current().await {
        Ok(lease) => ResponseHelper::json_success(serde_json::json!({
            "configured": true,
            "node": config.node,
            "lease": lease,
        })),
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// POST /api/replication/witness — acquire/renew the lease (admin).
pub async fn handle_witness_acquire(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let config =
        match configured() {
            Ok(Some(config)) => config,
            Ok(None) => return ResponseHelper::json_error(
                "no witness is configured (set HOMEVAULT_WITNESS_URL and HOMEVAULT_WITNESS_NODE)"
                    .to_string(),
            ),
            Err(e) => return ResponseHelper::json_error(e),
        };
    let client = match crate::replication_witness::WitnessClient::new(config) {
        Ok(client) => client,
        Err(e) => return ResponseHelper::json_error(e),
    };
    match client.acquire().await {
        Ok(lease) => ResponseHelper::json_success(lease),
        Err(e) => ResponseHelper::json_error(e),
    }
}
