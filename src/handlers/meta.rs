/* =============================================================================
 * File          : meta.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The client/server contract handshake endpoint: returns the server's API
 *   contract and build versions so a stale cached frontend can detect a
 *   mismatch and reload. Deliberately unauthenticated, and exempt from the
 *   contract middleware itself (it is the discovery channel for mismatches).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *
 * Usage:
 *   GET /api/meta in src/web/mod.rs; called by the frontend bootstrap.
 * =============================================================================
 */

//! `GET /api/meta` — the versioned client/server contract handshake.
//!
//! Deliberately unauthenticated: the SPA needs to detect a contract mismatch
//! *before* it has a session (e.g. a stale cached bundle on the login screen),
//! and the payload exposes nothing sensitive (two version numbers).

use crate::api_contract::MetaInfo;
use crate::utils::ResponseHelper;
use axum::response::Response;

/// Returns the server's contract + build versions.
pub async fn handle_meta() -> Response {
    ResponseHelper::json_success(MetaInfo::current())
}
