/* =============================================================================
 * File          : content_index.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Content-index endpoints (phase D1): the administrator's configuration and
 *   status, an on-demand rebuild, "delete my index", and the ACL-filtered
 *   content search the ⌘K overlay calls beside the name search.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::content_index, crate::global_search,
 *     crate::handlers::explorer
 *
 * Usage:
 *   Routes under /api/search in src/web/mod.rs; see
 *   docs/guides/content-index.md.
 * ============================================================================= */

//! The content index API, see docs/guides/content-index.md.
//!
//! * `GET    /api/search/index`            — config, candidates, state, size
//! * `PUT    /api/search/index`            — save (enabling requires `confirmed`)
//! * `POST   /api/search/index/run`        — rebuild now
//! * `DELETE /api/search/index?confirm=true` — delete my index
//! * `GET    /api/search/content?q=&limit=` — ACL-filtered content matches

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::content_index::{
    self, repo, ContentIndexConfig, MAX_DOCS, MAX_FILE_BYTES, MAX_ROOTS, MAX_TOTAL_BYTES,
};
use crate::global_search::{validate_query, SearchRoot};
use crate::handlers::admin_guard::{require_admin, require_session_token};
use crate::handlers::explorer::explorer_roots_for_session;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;

/// Maximum candidate hits pulled from FTS before the ACL filter. The caller
/// then gets `limit` of them; the extra room covers hits under roots the
/// caller cannot read.
const SEARCH_CANDIDATES: i64 = 50;

/// `GET /api/search/index`
pub async fn handle_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    let config = match content_index::load_persisted().await {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };
    let candidates = content_index::candidate_roots(&state).await;
    let (docs, bytes) = match repo::stats(&state.db.pool()).await {
        Ok(stats) => stats,
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };
    ResponseHelper::json_success(IndexGetResponse {
        config,
        candidates,
        state: content_index::state(),
        docs,
        bytes,
        limits: IndexLimits::default(),
    })
}

/// `PUT /api/search/index`
pub async fn handle_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    let request: IndexUpdateRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return ResponseHelper::json_error(format!("Malformed request body: {e}")),
    };
    let config = match content_index::validate_config(request.config) {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_error(e),
    };

    let candidates = content_index::candidate_roots(&state).await;
    for root in &config.roots {
        if !content_index::root_is_allowed(root, &candidates) {
            return ResponseHelper::json_error(format!(
                "{root} is not one of this appliance's Explorer roots."
            ));
        }
    }

    let previous = content_index::load_persisted().await.unwrap_or_default();
    if let Err(e) = content_index::save_persisted(&config).await {
        return ResponseHelper::json_internal_error(e);
    }

    // A root removed from the configuration stops being indexed immediately,
    // not at the next run.
    for root in previous
        .roots
        .iter()
        .filter(|root| !config.roots.contains(root))
    {
        if let Err(e) = repo::delete_root(&state.db.pool(), root).await {
            tracing::warn!("Content index: could not drop {root}: {e}");
        }
    }

    if config.enabled {
        content_index::trigger(state.clone());
    }

    crate::events::log_event(format!(
        "Admin '{admin}' {} the content index ({} root(s), {} day retention)",
        if config.enabled {
            "enabled"
        } else {
            "disabled"
        },
        config.roots.len(),
        config.retention_days
    ));

    ResponseHelper::json_success(serde_json::json!({
        "config": config,
        "state": content_index::state(),
    }))
}

/// `POST /api/search/index/run`
pub async fn handle_run(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    let config = match content_index::load_persisted().await {
        Ok(config) => config,
        Err(e) => return ResponseHelper::json_internal_error(e),
    };
    if !config.enabled {
        return ResponseHelper::json_error(
            "The content index is off; turn it on and choose a folder first.".to_string(),
        );
    }
    let started = content_index::trigger(state);
    ResponseHelper::json_accepted(serde_json::json!({ "started": started }))
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    #[serde(default)]
    pub confirm: Option<bool>,
}

/// `DELETE /api/search/index?confirm=true`
pub async fn handle_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<DeleteQuery>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(admin) => admin,
        Err(response) => return response,
    };
    if query.confirm != Some(true) {
        return ResponseHelper::json_error(
            "Deleting the index must be confirmed; add ?confirm=true.".to_string(),
        );
    }

    let deleted = match repo::delete_all(&state.db.pool()).await {
        Ok(deleted) => deleted,
        Err(e) => return ResponseHelper::json_internal_error(e.to_string()),
    };

    // Disable it as well: leaving it on would rebuild the index on the next
    // tick, which is not what "delete my index" means.
    let mut config = content_index::load_persisted().await.unwrap_or_default();
    config.enabled = false;
    config.confirmed = false;
    let config = content_index::validate_config(config).unwrap_or_default();
    if let Err(e) = content_index::save_persisted(&config).await {
        tracing::warn!("Content index: could not persist the disabled state: {e}");
    }

    crate::events::log_event(format!(
        "Admin '{admin}' deleted the content index ({deleted} document(s))"
    ));
    ResponseHelper::json_success(serde_json::json!({
        "deleted": deleted,
        "config": config,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ContentSearchQuery {
    pub q: String,
    pub limit: Option<u32>,
}

/// `GET /api/search/content`
pub async fn handle_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ContentSearchQuery>,
    headers: HeaderMap,
) -> Response {
    let session_token = match require_session_token(&state, &headers).await {
        Ok(token) => token,
        Err(response) => return response,
    };
    let session = match state.auth.get_session(&session_token).await {
        Ok(session) => session,
        Err(_) => return ResponseHelper::json_unauthorized("Session invalid".to_string()),
    };

    let query = match validate_query(&params.q) {
        Ok(query) => query,
        Err(error) => return ResponseHelper::json_error(error.message()),
    };
    let limit = content_index::clamp_limit(params.limit);
    let started = std::time::Instant::now();

    let config = content_index::load_persisted().await.unwrap_or_default();
    if !content_index::content_search_available(&config) {
        return ResponseHelper::json_success(ContentSearchResponse {
            available: false,
            query,
            hits: Vec::new(),
            truncated: false,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }

    // The ACL filter is the Explorer's own root list, exactly as the name
    // search uses it. Nothing is returned before this is applied.
    let roots: Vec<SearchRoot> = match explorer_roots_for_session(&state, &session, true).await {
        Ok(roots) => roots
            .into_iter()
            .map(|root| SearchRoot {
                path: root.path,
                kind: root.kind,
                name: root.name,
            })
            .collect(),
        Err(e) => return ResponseHelper::json_error(e),
    };
    let caller_roots = content_index::unique_roots(roots.iter().map(|root| root.path.clone()));

    let expression = match content_index::search_expression(&query) {
        Some(expression) => expression,
        None => {
            return ResponseHelper::json_success(ContentSearchResponse {
                available: true,
                query,
                hits: Vec::new(),
                truncated: false,
                elapsed_ms: started.elapsed().as_millis() as u64,
            })
        }
    };

    let candidates = match repo::search(&state.db.pool(), &expression, SEARCH_CANDIDATES).await {
        Ok(candidates) => candidates,
        Err(e) => {
            return ResponseHelper::json_internal_error(format!("Content search failed: {e}"))
        }
    };
    let visible: Vec<repo::ContentHit> = candidates
        .into_iter()
        .filter(|hit| content_index::hit_is_visible(&hit.path, &caller_roots))
        .collect();
    let (hits, truncated) = content_index::take_hits(visible, limit);

    ResponseHelper::json_success(ContentSearchResponse {
        available: true,
        query,
        hits: hits
            .into_iter()
            .map(|hit| {
                let root = roots
                    .iter()
                    .filter(|root| content_index::path_is_under(&hit.path, &root.path))
                    .max_by_key(|root| root.path.len());
                ContentSearchHit {
                    path: hit.path,
                    name: hit.name,
                    size_bytes: hit.size_bytes,
                    modified: hit.modified,
                    snippet: hit.snippet,
                    root: root.map(|root| root.path.clone()).unwrap_or(hit.root),
                    root_kind: root.map(|root| root.kind.clone()).unwrap_or_default(),
                    root_name: root.map(|root| root.name.clone()).unwrap_or_default(),
                }
            })
            .collect(),
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Debug, Serialize)]
pub struct IndexGetResponse {
    pub config: ContentIndexConfig,
    pub candidates: Vec<String>,
    pub state: content_index::RunState,
    pub docs: i64,
    pub bytes: i64,
    pub limits: IndexLimits,
}

#[derive(Debug, Serialize)]
pub struct IndexLimits {
    pub max_roots: usize,
    pub max_docs: i64,
    pub max_total_bytes: i64,
    pub max_file_bytes: u64,
}

impl Default for IndexLimits {
    fn default() -> Self {
        Self {
            max_roots: MAX_ROOTS,
            max_docs: MAX_DOCS,
            max_total_bytes: MAX_TOTAL_BYTES,
            max_file_bytes: MAX_FILE_BYTES,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct IndexUpdateRequest {
    pub config: ContentIndexConfig,
}

#[derive(Debug, Serialize)]
pub struct ContentSearchResponse {
    pub available: bool,
    pub query: String,
    pub hits: Vec<ContentSearchHit>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentSearchHit {
    pub path: String,
    pub name: String,
    pub size_bytes: i64,
    pub modified: i64,
    pub snippet: String,
    pub root: String,
    pub root_kind: String,
    pub root_name: String,
}
