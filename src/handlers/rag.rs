//! RAG (Retrieval-Augmented Generation) API handlers.
//!
//! Endpoints:
//! - GET  /api/rag/status          — index status (any session)
//! - POST /api/rag/index           — trigger full reindex (admin only)
//! - POST /api/rag/search          — semantic search (admin only, for debugging)
//! - GET  /api/rag/documents       — list indexed documents (admin only)

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use crate::handlers::AppState;
use axum::http::HeaderMap;
use std::sync::Arc;

// ── Request/Response types ───────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RagStatusResponse {
    pub success: bool,
    pub data: Option<crate::rag::RagIndexStatus>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RagSearchRequest {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    5
}

#[derive(Debug, Serialize)]
pub struct RagSearchResponse {
    pub success: bool,
    pub data: Option<RagSearchData>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RagSearchData {
    pub results: Vec<crate::rag::SearchResult>,
    pub context: String,
}

#[derive(Debug, Serialize)]
pub struct RagIndexResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub data: Option<RagIndexResult>,
}

#[derive(Debug, Serialize)]
pub struct RagIndexResult {
    pub documents: i32,
    pub chunks: i32,
}

#[derive(Debug, Serialize)]
pub struct RagDocumentsResponse {
    pub success: bool,
    pub data: Option<Vec<RagDocSummary>>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RagDocSummary {
    pub id: String,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub language: String,
}

// ── Handlers ─────────────────────────────────────────────────────────

/// GET /api/rag/status — any authenticated session.
pub async fn handle_rag_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<RagStatusResponse> {
    if crate::handlers::admin_guard::require_auth(&state, &headers)
        .await
        .is_err()
    {
        return Json(RagStatusResponse {
            success: false,
            data: None,
            error: Some("Authentication required".to_string()),
        });
    }

    match crate::rag::get_index_status(&state.db.pool()).await {
        Ok(status) => Json(RagStatusResponse {
            success: true,
            data: Some(status),
            error: None,
        }),
        Err(e) => Json(RagStatusResponse {
            success: false,
            data: None,
            error: Some(e),
        }),
    }
}

/// POST /api/rag/index — admin only, triggers full reindex.
/// This is a long-running operation; the response returns immediately
/// while indexing continues in the background.
pub async fn handle_rag_index(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<RagIndexResponse> {
    if crate::handlers::admin_guard::require_admin(&state, &headers)
        .await
        .is_err()
    {
        return Json(RagIndexResponse {
            success: false,
            message: None,
            error: Some("Admin access required".to_string()),
            data: None,
        });
    }

    // Spawn indexing in the background, tracked as a progress job so it shows
    // in the desktop activity taskbar (indeterminate — index_all reports no
    // incremental progress).
    let pool = state.db.pool().clone();
    let job = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("rag_index".to_string()),
        "Indexing knowledge base",
    );
    tokio::spawn(async move {
        match crate::rag_indexers::index_all(&pool).await {
            Ok((docs, chunks)) => {
                tracing::info!(
                    "RAG indexing complete: {} documents, {} chunks",
                    docs,
                    chunks
                );
                crate::sse::PROGRESS_MANAGER
                    .complete_job(&job, &format!("Indexed {} documents", docs));
            }
            Err(e) => {
                tracing::error!("RAG indexing failed: {}", e);
                let _ = crate::rag::update_index_status(&pool, "error", 0, 0).await;
                crate::sse::PROGRESS_MANAGER.fail_job(&job, &e.to_string());
            }
        }
    });

    Json(RagIndexResponse {
        success: true,
        message: Some("Indexing started in background".to_string()),
        error: None,
        data: None,
    })
}

/// POST /api/rag/search — admin only, for debugging/testing the RAG.
pub async fn handle_rag_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RagSearchRequest>,
) -> Json<RagSearchResponse> {
    if crate::handlers::admin_guard::require_admin(&state, &headers)
        .await
        .is_err()
    {
        return Json(RagSearchResponse {
            success: false,
            data: None,
            error: Some("Admin access required".to_string()),
        });
    }

    if req.query.trim().is_empty() {
        return Json(RagSearchResponse {
            success: false,
            data: None,
            error: Some("Query cannot be empty".to_string()),
        });
    }

    // Generate embedding for the query
    let embeddings = match crate::rag::generate_embeddings(&[req.query.clone()]).await {
        Ok(e) => e,
        Err(e) => {
            return Json(RagSearchResponse {
                success: false,
                data: None,
                error: Some(format!("Failed to generate query embedding: {}", e)),
            });
        }
    };

    let query_embedding = match embeddings.first() {
        Some(e) => e,
        None => {
            return Json(RagSearchResponse {
                success: false,
                data: None,
                error: Some("No embedding returned".to_string()),
            });
        }
    };

    match crate::rag::search_similar(&state.db.pool(), query_embedding, req.top_k).await {
        Ok(results) => {
            let context = crate::rag::build_rag_context(&results);
            Json(RagSearchResponse {
                success: true,
                data: Some(RagSearchData { results, context }),
                error: None,
            })
        }
        Err(e) => Json(RagSearchResponse {
            success: false,
            data: None,
            error: Some(e),
        }),
    }
}

/// GET /api/rag/documents — admin only, list indexed documents.
pub async fn handle_rag_documents(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<RagDocumentsResponse> {
    if crate::handlers::admin_guard::require_admin(&state, &headers)
        .await
        .is_err()
    {
        return Json(RagDocumentsResponse {
            success: false,
            data: None,
            error: Some("Admin access required".to_string()),
        });
    }

    let docs: Vec<RagDocSummary> = sqlx::query_as(
        "SELECT id, source, source_id, title, language FROM rag_documents ORDER BY source, title",
    )
    .fetch_all(&*state.db.pool())
    .await
    .map_err(|e| e.to_string())
    .unwrap_or_default();

    Json(RagDocumentsResponse {
        success: true,
        data: Some(docs),
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rag_search_request_defaults() {
        let req: RagSearchRequest = serde_json::from_str(r#"{"query":"test"}"#).unwrap();
        assert_eq!(req.query, "test");
        assert_eq!(req.top_k, 5);
    }

    #[test]
    fn test_rag_search_request_custom_top_k() {
        let req: RagSearchRequest =
            serde_json::from_str(r#"{"query":"docker","top_k":10}"#).unwrap();
        assert_eq!(req.top_k, 10);
    }
}
