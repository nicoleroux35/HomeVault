/* =============================================================================
 * File          : scheduler_health.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Admin endpoint for the background-loop supervisor (FIAB-02).
 *
 *   `GET /api/health/schedulers` reports, for every supervised loop, whether
 *   it is running, how many times it was restarted and why it last stopped.
 *   The payload carries internal module names but no user data; it stays
 *   admin-only so a member cannot infer host activity from it.
 *
 * Dependencies:
 *   - axum, crate::scheduler_supervisor
 *
 * Usage:
 *   Route in src/web/mod.rs; rendered by the Health screen.
 * =============================================================================
 */

//! Admin view of the supervised background loops.

use std::sync::Arc;

use axum::{extract::State, http::HeaderMap, response::Response};

use crate::handlers::AppState;
use crate::utils::ResponseHelper;

/// `GET /api/health/schedulers` — state of every supervised loop.
pub async fn handle_scheduler_health(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }
    ResponseHelper::json_success(crate::scheduler_supervisor::snapshot())
}
