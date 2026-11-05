/* =============================================================================
 * File          : openapi.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin-only `GET /api/openapi.json`: serves the generated OpenAPI 3
 *   document assembled by `crate::openapi` from the Axum route table.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::openapi
 *
 * Usage:
 *   Mounted in src/web/mod.rs; see docs/API.md → "OpenAPI 3".
 * =============================================================================
 */

//! `GET /api/openapi.json` — the machine-readable API contract.

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use std::sync::Arc;

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;

/// Serve the OpenAPI document to an administrator.
///
/// The document describes every administrator-only endpoint, including this
/// one, so it is never exposed without a session. It is built once per process
/// and serialization is deterministic (sorted keys), which is what makes the
/// stable-snapshot test meaningful.
pub async fn handle_openapi_json(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    Json(crate::openapi::spec()).into_response()
}
