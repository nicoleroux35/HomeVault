/* =============================================================================
 * File          : transfers.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Transfer visibility endpoints for the File Explorer (HV-UX-01/HV-ADV-02):
 *   one live view of everything in flight (chunked uploads plus the copy,
 *   move and checksum jobs of the progress manager, grouped into the tray's
 *   upload / download / copy zones) and the finished-transfer audit trail.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/explorer/transfers in src/web/mod.rs; consumed by
 *   `TransferTray.svelte`. See docs/guides/explorer.md → "Transfer tray".
 * =============================================================================
 */

//! Live transfer list and transfer history for the Explorer.
//!
//! ## Why a second endpoint next to `/api/activity`
//!
//! `/api/activity` is the **desktop** taskbar: it merges every background job
//! of the appliance (backups, scrubs, scans) and drops a job the moment it
//! finishes. The Explorer's tray has a narrower question — "what is *this*
//! file operation doing?" — and two things the taskbar cannot answer:
//!
//! 1. **In-flight chunked uploads.** They are not progress jobs at all; they
//!    are rows in `upload_sessions`. Listing them from the database is what
//!    makes the tray correct after a restart.
//! 2. **Zone grouping.** DSM's FileStation has three trays (upload / download /
//!    background); the same shape needs a stable `zone` per row, which is a
//!    question only this product answers ("is `explorer_copy` an upload?" —
//!    no, it is a copy).
//!
//! Both endpoints are read-only and session-authenticated, and both scope rows
//! the same way: a non-admin only ever sees their own.

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::handlers::admin_guard::require_auth;
use crate::handlers::explorer::{active_upload_views, ActiveUploadView};
use crate::handlers::AppState;
use crate::sse::{ActivitySnapshot, ProgressState, PROGRESS_MANAGER};
use crate::transfer_history::{HistoryFilters, TransferHistoryRepository};
use crate::utils::ResponseHelper;

/// How long a finished-with-failure transfer stays in the live list. Matches
/// the desktop feed's grace: the row the user was watching reports its outcome
/// instead of vanishing between two polls.
pub const FAILED_GRACE_SECS: i64 = 60;

/// Which zone of the tray a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferZone {
    Upload,
    Download,
    Copy,
}

/// One row of the live transfer list.
#[derive(Debug, Clone, Serialize)]
pub struct LiveTransfer {
    /// Progress-manager job id, or `upload:<session_id>` for a chunked upload.
    pub id: String,
    pub zone: TransferZone,
    /// Stable kind slug for the client to localise (`explorer_copy`, …).
    pub kind: String,
    pub state: String,
    /// Filename / path tail when known, empty otherwise.
    pub label: String,
    /// `0..=100`, or `None` when the producer cannot estimate.
    pub progress: Option<f64>,
    pub bytes_processed: Option<u64>,
    pub bytes_total: Option<u64>,
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
    /// Copy method / digest slug when the producer reports one.
    pub method: Option<String>,
    pub detail: Option<String>,
    /// API-relative cancellation path (`/progress/{id}` — the client prefixes
    /// `/api`) for a row that can be stopped. `None` for an upload: the client
    /// aborts it with `DELETE /explorer/upload/session?session_id=…`, which
    /// needs the session id rather than a path.
    pub cancel_path: Option<String>,
}

/// Per-zone badges, so the client does not have to count rows itself.
#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct ZoneCounts {
    pub uploads: usize,
    pub downloads: usize,
    pub copies: usize,
    pub total: usize,
}

/// Response of `GET /api/explorer/transfers/active`.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveTransfers {
    pub uploads: Vec<ActiveUploadView>,
    pub transfers: Vec<LiveTransfer>,
    pub counts: ZoneCounts,
}

/// Which zone a job kind belongs to; `None` for a job the Explorer does not
/// own (a backup, a scrub, an antivirus scan).
pub fn zone_for_kind(kind: &str) -> Option<TransferZone> {
    match kind {
        "upload" => Some(TransferZone::Upload),
        "download" | "download_torrent" | "download_url" => Some(TransferZone::Download),
        "explorer_copy" | "explorer_move" | "explorer_checksum" => Some(TransferZone::Copy),
        _ => None,
    }
}

/// `true` while a job should stay in the live list at `now`.
///
/// Live work always does; a failure or a cancellation stays for
/// [`FAILED_GRACE_SECS`] so the outcome is visible; a completed job never does
/// (the screen that started it reports its own success, and the history
/// endpoint is where a finished transfer belongs).
pub fn is_live(state: &ProgressState, updated_at: Option<i64>, now: i64) -> bool {
    match state {
        ProgressState::Pending | ProgressState::Running => true,
        ProgressState::Failed | ProgressState::Cancelled => {
            updated_at.is_some_and(|at| now.saturating_sub(at) <= FAILED_GRACE_SECS)
        }
        ProgressState::Completed => false,
    }
}

fn state_slug(state: &ProgressState) -> &'static str {
    match state {
        ProgressState::Pending => "pending",
        ProgressState::Running => "running",
        ProgressState::Completed => "completed",
        ProgressState::Failed => "failed",
        ProgressState::Cancelled => "cancelled",
    }
}

/// The row's human name: the basename of the path the message ends with, else
/// empty (the client then shows the localised kind label).
///
/// Only the **last whitespace-separated token** is considered, and only when it
/// looks like an absolute path. Splitting the whole message on `/` would turn a
/// progress sentence into a fake filename: `"12/40 files"` would become
/// `"40 files"` and `"Copied a / b"` would become `" b"`.
pub fn label_from_message(message: &str) -> String {
    let Some(last) = message.split_whitespace().last() else {
        return String::new();
    };
    if !last.starts_with('/') {
        return String::new();
    }
    last.rsplit('/')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn live_from_snapshot(s: ActivitySnapshot, zone: TransferZone, now: i64) -> Option<LiveTransfer> {
    if !is_live(&s.state, s.updated_at.map(|t| t.timestamp()), now) {
        return None;
    }
    Some(LiveTransfer {
        id: s.id.clone(),
        zone,
        kind: s.kind_slug(),
        state: state_slug(&s.state).to_string(),
        label: label_from_message(&s.message),
        progress: s.progress.map(|p| p.clamp(0.0, 100.0)),
        bytes_processed: s.bytes_processed,
        bytes_total: s.bytes_total,
        speed_bps: s.speed_bps,
        eta_seconds: s.eta_seconds,
        method: s.method.clone(),
        detail: (!s.message.is_empty()).then(|| s.message.clone()),
        // Only when the producer registered a hook: `/api/progress/{id}` runs it
        // to stop the real work. A row with no hook is not offered a cancel
        // button, because cancelling it would only hide it.
        cancel_path: PROGRESS_MANAGER
            .has_cancel_hook(&s.id)
            .then(|| format!("/progress/{}", s.id)),
    })
}

/// `GET /api/explorer/transfers/active`
pub async fn handle_active_transfers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let is_admin = state.auth.is_admin(&username).await;
    let now = chrono::Utc::now().timestamp();

    let uploads = active_upload_views(&state, &username).await;

    // Same ownership rule as `/api/activity`: an ownerless job is admin-only,
    // and a user's jobs never leak their file paths to anyone else.
    let transfers: Vec<LiveTransfer> = PROGRESS_MANAGER
        .list_activity()
        .into_iter()
        .filter(|s| is_admin || s.owner.as_deref() == Some(username.as_str()))
        .filter_map(|s| {
            let zone = zone_for_kind(&s.kind_slug())?;
            live_from_snapshot(s, zone, now)
        })
        .collect();

    let counts = ZoneCounts {
        uploads: uploads.len(),
        downloads: transfers
            .iter()
            .filter(|t| t.zone == TransferZone::Download)
            .count(),
        copies: transfers
            .iter()
            .filter(|t| t.zone == TransferZone::Copy)
            .count(),
        total: uploads.len() + transfers.len(),
    };

    ResponseHelper::json_success(ActiveTransfers {
        uploads,
        transfers,
        counts,
    })
    .into_response()
}

/// Query of `GET /api/explorer/transfers/history`.
#[derive(Debug, Deserialize)]
pub struct TransferHistoryQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// `upload` | `download` | `copy` | `move` | `checksum`
    pub operation: Option<String>,
    /// `complete` | `failed` | `cancelled`
    pub status: Option<String>,
    /// Unix seconds — only rows finished at or after this instant.
    pub since: Option<i64>,
}

/// Response of `GET /api/explorer/transfers/history`.
#[derive(Debug, Serialize)]
pub struct TransferHistoryResponse {
    pub entries: Vec<crate::transfer_history::TransferEntry>,
    /// `true` when more rows match than this page returned.
    pub has_more: bool,
    pub limit: i64,
    pub offset: i64,
}

/// `GET /api/explorer/transfers/history` — the finished-transfer audit trail.
///
/// A non-admin sees only their own transfers; an administrator sees every
/// user's (that is the audit view). Unknown filter values are a `400`: an
/// unfiltered answer to a typo reads as "no failed transfers" while the list is
/// actually everything.
pub async fn handle_transfer_history(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TransferHistoryQuery>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let is_admin = state.auth.is_admin(&username).await;

    let filters = HistoryFilters {
        operation: params.operation.clone(),
        status: params.status.clone(),
        since: params.since,
    };
    let (operation, status) = match filters.validate() {
        Ok(pair) => pair,
        Err(message) => return ResponseHelper::json_error(message),
    };

    let limit = crate::transfer_history::clamp_limit(params.limit);
    let offset = params.offset.unwrap_or(0) as i64;
    // One row past the page tells us whether another page exists without a
    // second COUNT query over a 90-day table.
    let repo = TransferHistoryRepository::new(state.db.pool());
    let mut entries = match repo
        .list(
            &username,
            is_admin,
            operation,
            status,
            params.since,
            limit + 1,
            offset,
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("Could not read the transfer history: {e}");
            return ResponseHelper::json_error("Could not read the transfer history".to_string());
        }
    };
    let has_more = entries.len() as i64 > limit;
    entries.truncate(limit as usize);

    ResponseHelper::json_success(TransferHistoryResponse {
        entries,
        has_more,
        limit,
        offset,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    //! Pure mapping tests — no DB, no HTTP.
    //! Run: `cargo test handlers::transfers::tests`
    use super::*;

    #[test]
    fn test_zone_for_kind_groups_the_tray() {
        assert_eq!(zone_for_kind("upload"), Some(TransferZone::Upload));
        assert_eq!(zone_for_kind("download"), Some(TransferZone::Download));
        assert_eq!(zone_for_kind("download_torrent"), Some(TransferZone::Download));
        assert_eq!(zone_for_kind("explorer_copy"), Some(TransferZone::Copy));
        assert_eq!(zone_for_kind("explorer_move"), Some(TransferZone::Copy));
        assert_eq!(zone_for_kind("explorer_checksum"), Some(TransferZone::Copy));
        // A job the Explorer does not own is never in its tray.
        assert_eq!(zone_for_kind("backup"), None);
        assert_eq!(zone_for_kind("raid_resync"), None);
    }

    #[test]
    fn test_is_live_keeps_work_and_recent_failures() {
        assert!(is_live(&ProgressState::Running, None, 10_000));
        assert!(is_live(&ProgressState::Pending, None, 10_000));
        assert!(!is_live(&ProgressState::Completed, Some(10_000), 10_000));
        assert!(is_live(&ProgressState::Failed, Some(10_000), 10_030));
        assert!(!is_live(&ProgressState::Failed, Some(10_000), 10_061));
        assert!(
            !is_live(&ProgressState::Cancelled, None, 10_000),
            "a cancellation with no timestamp cannot be aged, so it is not shown"
        );
    }

    #[test]
    fn test_is_live_survives_a_clock_that_went_backwards() {
        // `now` before `updated_at` must not show a failure for ever.
        assert!(is_live(&ProgressState::Failed, Some(10_000), 9_999));
    }

    #[test]
    fn test_label_from_message_takes_the_path_tail() {
        assert_eq!(label_from_message("Copied /data/files/movie.mkv"), "movie.mkv");
        assert_eq!(label_from_message("/data/files/movie.mkv"), "movie.mkv");
        assert_eq!(label_from_message(""), "");
        assert_eq!(label_from_message("   "), "");
        assert_eq!(
            label_from_message("Hashing with sha256..."),
            "",
            "prose is not a filename"
        );
        assert_eq!(
            label_from_message("12/40 files"),
            "",
            "a progress fraction is not a path"
        );
        assert_eq!(label_from_message("Copied a / b"), "");
        assert_eq!(label_from_message("Moved /a/b/"), "");
    }

    #[test]
    fn test_state_slug_covers_every_state() {
        assert_eq!(state_slug(&ProgressState::Pending), "pending");
        assert_eq!(state_slug(&ProgressState::Running), "running");
        assert_eq!(state_slug(&ProgressState::Completed), "completed");
        assert_eq!(state_slug(&ProgressState::Failed), "failed");
        assert_eq!(state_slug(&ProgressState::Cancelled), "cancelled");
    }
}
