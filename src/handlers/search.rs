/* =============================================================================
 * File          : search.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The global file search endpoint behind the desktop's ⌘K overlay (QW-12):
 *   one query, every root the caller may read, merged and capped. Answers a
 *   truthful report — including which roots timed out or could not be read —
 *   rather than a silently incomplete list.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::global_search, crate::browser, crate::permissions
 *
 * Usage:
 *   Route /api/search/files in src/web/mod.rs; see docs/guides/explorer.md.
 * =============================================================================
 */

//! Global file search endpoint, see docs/guides/explorer.md.
//!
//! * `GET /api/search/files?q=<name>&limit=<n>` — search every readable root.

use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;

use crate::browser::FileBrowser;
use crate::global_search::{
    clamp_limit, dedupe_roots, merge_hits, tag_hits, validate_query, SearchHit, SearchReport,
    SearchRoot, PER_ROOT_TIMEOUT, TOTAL_TIMEOUT,
};
use crate::handlers::admin_guard::require_session_token;
use crate::handlers::explorer::explorer_roots_for_session;
use crate::utils::ResponseHelper;

use super::AppState;

#[derive(Debug, Deserialize)]
pub struct GlobalSearchQuery {
    /// The file name fragment. Required.
    pub q: String,
    /// Result cap; clamped into `1..=MAX_RESULTS` server-side.
    pub limit: Option<u32>,
    /// Return file names only (no directories). Defaults to `false`.
    #[serde(default)]
    pub files_only: bool,
}

/// `GET /api/search/files`
///
/// Every signed-in user may call it: the answer is confined to the roots their
/// own Explorer session would list, and the ACL filter is applied to the hits
/// as well as to the roots. An empty, one-character, over-long or
/// path-shaped query is a `400` naming the rule — never a silent empty list,
/// which would read as "no such file".
pub async fn handle_search_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<GlobalSearchQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = match require_session_token(&state, &headers).await {
        Ok(token) => token,
        Err(resp) => return resp,
    };
    let session = match state.auth.get_session(&session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let query = match validate_query(&params.q) {
        Ok(q) => q,
        Err(e) => return ResponseHelper::json_error(e.message()),
    };
    let limit = clamp_limit(params.limit);

    let started = Instant::now();

    // The roots are the Explorer's own list, so the search can never see a
    // folder the Explorer sidebar would not offer. Reading them off the async
    // runtime (`/proc`, `stat`, an optional pool scan) is the handler's job.
    let roots: Vec<SearchRoot> =
        match explorer_roots_for_session(&state, &session, !params.files_only).await {
            Ok(roots) => dedupe_roots(
                roots
                    .into_iter()
                    .map(|root| SearchRoot {
                        path: root.path,
                        kind: root.kind,
                        name: root.name,
                    })
                    .collect(),
            ),
            Err(message) => return ResponseHelper::json_error(message),
        };

    // Fan out. Each root gets its own budget and its own task: a hung network
    // mount costs its own 1.5 s and nothing else. The overall deadline stops
    // the answer from growing past the overlay's patience.
    let fanout_deadline = tokio::time::Instant::now() + TOTAL_TIMEOUT;
    let mut walks = Vec::with_capacity(roots.len());
    for root in roots.clone() {
        let query = query.clone();
        let take = limit;
        let files_only = params.files_only;
        walks.push(async move {
            let outcome = tokio::time::timeout(
                PER_ROOT_TIMEOUT,
                FileBrowser::search(&root.path, &query, take.saturating_mul(4).max(32)),
            )
            .await;
            (root, outcome, files_only)
        });
    }

    let results =
        tokio::time::timeout_at(fanout_deadline, futures_util::future::join_all(walks)).await;

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut timed_out: Vec<String> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();

    match results {
        Ok(outcomes) => {
            for (root, outcome, files_only) in outcomes {
                match outcome {
                    Ok(Ok(infos)) => {
                        let mut tagged = tag_hits(&root, infos);
                        if files_only {
                            tagged.retain(|hit| !hit.info.is_directory);
                        }
                        // The walk is already confined to `root`, but the
                        // answer is handed to a client that opens the path,
                        // so the rule is asserted rather than assumed.
                        tagged.retain(crate::global_search::hit_is_confined);
                        hits.extend(tagged);
                    }
                    Ok(Err(e)) => {
                        tracing::debug!("Global search: root '{}' failed: {e}", root.path);
                        unreadable.push(root.path);
                    }
                    // The per-root budget expired.
                    Err(_) => timed_out.push(root.path),
                }
            }
        }
        Err(_) => {
            // The overall deadline expired: everything still outstanding is
            // reported as timed out rather than dropped silently.
            timed_out.extend(roots.iter().map(|r| r.path.clone()));
        }
    }

    let (hits, truncated) = merge_hits(hits, &query, limit);

    ResponseHelper::json_success(SearchReport {
        query,
        hits,
        roots,
        timed_out,
        unreadable,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}
