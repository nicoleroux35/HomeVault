/* =============================================================================
 * File          : whats_new.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Release-highlights endpoints: returns the running version's what's-new
 *   payload (title, highlights, per-user seen flag) and records the user's
 *   acknowledgement, so the welcome modal shows once per version per user.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   GET /api/whats-new (auth) and POST /api/whats-new/seen (auth), routed
 *   in src/web/mod.rs.
 * =============================================================================
 */

//! What's New endpoints: the release-highlight payload the SPA shows once
//! per version, and the per-user acknowledgement.
//!
//! `GET  /api/whats-new` returns `{ version, title, highlights, seen }` for the
//! running build; `seen` is per-user so the modal stays dismissed across
//! devices. `POST /api/whats-new/seen` records the acknowledgement.
//!
//! The first request after login that returns `seen: false` triggers the
//! modal; the SPA then posts `seen` and never shows it again for this version.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::release_notes;
use crate::storage::WhatsNewRepository;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

/// Returns the running version's release highlights plus whether the caller
/// has already acknowledged them.
pub async fn handle_whats_new(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let version = release_notes::current_version().to_string();
    let note = release_notes::current();

    let repo = WhatsNewRepository::new(state.db.pool());
    let seen: bool = match repo.has_seen(&username, &version).await {
        Ok(s) => s,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to read what's-new state: {}", e))
        }
    };

    let highlights: Vec<&str> = note.map(|n| n.highlights.to_vec()).unwrap_or_default();
    let payload = serde_json::json!({
        "version": version,
        "title": note.map(|n| n.title).unwrap_or_default(),
        "highlights": highlights,
        "seen": seen,
    });
    ResponseHelper::json_success(payload)
}

/// Records that the caller has acknowledged the current release.
pub async fn handle_whats_new_seen(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let version = release_notes::current_version();
    let repo = WhatsNewRepository::new(state.db.pool());
    match repo.mark_seen(&username, version).await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "seen": true })),
        Err(e) => ResponseHelper::json_error(format!("Failed to record acknowledgement: {}", e)),
    }
}
