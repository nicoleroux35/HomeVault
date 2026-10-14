/* =============================================================================
 * File          : rag.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   RAG knowledge base core: document chunking, embeddings generated via
 *   the local Ollama service, vector storage in SQLite, and
 *   cosine-similarity search with context building — grounding the chat
 *   assistant's answers in indexed project documents.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), serde (>= 1.0)
 *
 * Usage:
 *   handlers/rag.rs (API), handlers/chat.rs (prompt grounding),
 *   rag_indexers.rs (document sources); see docs/guides/rag.md.
 * =============================================================================
 */

//! RAG (Retrieval-Augmented Generation) knowledge base.
//!
//! Provides document chunking, embedding via Ollama, storage in SQLite, and
//! cosine-similarity search to enrich chat prompts with relevant context.
//!
//! Architecture:
//! 1. Documents are split into ~500-token chunks with overlap
//! 2. Each chunk is embedded via Ollama's `/api/embed` endpoint
//! 3. Embeddings are stored as f32 BLOBs in SQLite
//! 4. Query embedding → cosine similarity → top-K chunks → inject into system prompt

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

// ── Constants ────────────────────────────────────────────────────────

const OLLAMA_BASE: &str = "http://localhost:11434";
const EMBEDDING_MODEL: &str = "nomic-embed-text";
const CHUNK_SIZE: usize = 400; // approximate tokens per chunk
const CHUNK_OVERLAP: usize = 80;
const MAX_CONTEXT_CHARS: usize = 3000;

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RagDocument {
    pub id: String,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub content: String,
    pub language: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct RagChunk {
    pub id: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub content: String,
    pub token_count: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RagIndexStatus {
    pub last_indexed_at: Option<String>,
    pub total_documents: i32,
    pub total_chunks: i32,
    pub embedding_model: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub document_id: String,
    pub title: String,
    pub content: String,
    pub score: f32,
}

// ── Ollama embedding request/response ────────────────────────────────

#[derive(Debug, Serialize)]
struct OllamaEmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

// ── Chunking ─────────────────────────────────────────────────────────

/// Split text into overlapping chunks by word boundary.
/// Returns (chunk_text, chunk_index) pairs.
pub fn chunk_text(text: &str) -> Vec<(String, usize)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    let mut chunk_idx = 0;

    while start < words.len() {
        let end = (start + CHUNK_SIZE).min(words.len());
        let chunk_words = &words[start..end];
        let chunk = chunk_words.join(" ");
        chunks.push((chunk, chunk_idx));
        chunk_idx += 1;

        if end >= words.len() {
            break;
        }
        start += CHUNK_SIZE - CHUNK_OVERLAP;
    }

    chunks
}

/// Rough token count (word-based approximation, ~1.3 tokens per word).
pub fn estimate_tokens(text: &str) -> usize {
    (text.split_whitespace().count() * 13) / 10
}

// ── Embedding via Ollama ─────────────────────────────────────────────

/// Generate embeddings for a batch of texts via Ollama's /api/embed endpoint.
/// Returns a vector of f32 embedding vectors, one per input text.
pub async fn generate_embeddings(texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(vec![]);
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let req = OllamaEmbedRequest {
        model: EMBEDDING_MODEL.to_string(),
        input: texts.to_vec(),
    };

    let response = client
        .post(format!("{}/api/embed", OLLAMA_BASE))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Cannot reach Ollama for embeddings: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Ollama embed returned {}: {}", status, body));
    }

    let resp: OllamaEmbedResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Ollama embed response: {}", e))?;

    Ok(resp.embeddings)
}

// ── Vector math ──────────────────────────────────────────────────────

/// Cosine similarity between two f32 vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Serialize f32 vector to bytes for BLOB storage.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Deserialize f32 vector from BLOB bytes.
pub fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

// ── Database operations ──────────────────────────────────────────────

/// Upsert a document and its chunks with embeddings.
pub async fn upsert_document_with_chunks(
    pool: &SqlitePool,
    doc_id: &str,
    source: &str,
    source_id: &str,
    title: &str,
    content: &str,
    language: &str,
) -> Result<usize, String> {
    let chunks = chunk_text(content);
    if chunks.is_empty() {
        return Ok(0);
    }

    // Upsert the document
    sqlx::query(
        "INSERT INTO rag_documents (id, source, source_id, title, content, language, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET title=excluded.title, content=excluded.content,
         language=excluded.language, updated_at=datetime('now')",
    )
    .bind(doc_id)
    .bind(source)
    .bind(source_id)
    .bind(title)
    .bind(content)
    .bind(language)
    .execute(pool)
    .await
    .map_err(|e| format!("DB error upserting document: {}", e))?;

    // Delete old chunks for this document
    sqlx::query("DELETE FROM rag_chunks WHERE document_id = ?")
        .bind(doc_id)
        .execute(pool)
        .await
        .map_err(|e| format!("DB error deleting old chunks: {}", e))?;

    // Generate embeddings in batch
    let chunk_texts: Vec<String> = chunks.iter().map(|(text, _)| text.clone()).collect();
    let embeddings = match generate_embeddings(&chunk_texts).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to generate embeddings for '{}': {}", title, e);
            vec![]
        }
    };

    // Insert chunks with embeddings
    let mut inserted = 0;
    for (i, (text, _)) in chunks.iter().enumerate() {
        let chunk_id = format!("{}:{}", doc_id, i);
        let embedding_bytes = embeddings.get(i).map(|e| embedding_to_bytes(e));
        let tokens = estimate_tokens(text) as i32;

        sqlx::query(
            "INSERT INTO rag_chunks (id, document_id, chunk_index, content, embedding, token_count)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&chunk_id)
        .bind(doc_id)
        .bind(i as i32)
        .bind(text)
        .bind(embedding_bytes.as_deref())
        .bind(tokens)
        .execute(pool)
        .await
        .map_err(|e| format!("DB error inserting chunk: {}", e))?;

        inserted += 1;
    }

    Ok(inserted)
}

/// Delete all documents and chunks for a given source.
pub async fn clear_source(pool: &SqlitePool, source: &str) -> Result<(), String> {
    let doc_ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM rag_documents WHERE source = ?")
        .bind(source)
        .fetch_all(pool)
        .await
        .map_err(|e| e.to_string())?;

    for (doc_id,) in &doc_ids {
        sqlx::query("DELETE FROM rag_chunks WHERE document_id = ?")
            .bind(doc_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
    }

    sqlx::query("DELETE FROM rag_documents WHERE source = ?")
        .bind(source)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Cosine similarity search across all chunks.
pub async fn search_similar(
    pool: &SqlitePool,
    query_embedding: &[f32],
    top_k: usize,
) -> Result<Vec<SearchResult>, String> {
    // Fetch all chunks with embeddings
    let rows: Vec<(String, String, String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT rc.id, rc.document_id, rd.title, rc.content, rc.embedding
         FROM rag_chunks rc
         JOIN rag_documents rd ON rc.document_id = rd.id
         WHERE rc.embedding IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("DB error searching chunks: {}", e))?;

    let mut scored: Vec<SearchResult> = rows
        .into_iter()
        .filter_map(|(chunk_id, doc_id, title, content, emb_bytes)| {
            let emb = bytes_to_embedding(&emb_bytes);
            let score = cosine_similarity(query_embedding, &emb);
            if score > 0.1 {
                // minimum threshold
                Some(SearchResult {
                    chunk_id,
                    document_id: doc_id,
                    title,
                    content,
                    score,
                })
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(top_k);
    Ok(scored)
}

/// Raw `rag_index_status` row: `(last_indexed_at, total_documents, total_chunks, embedding_model, status)`.
type RagStatusRow = (Option<String>, i32, i32, Option<String>, Option<String>);

/// Get index status.
pub async fn get_index_status(pool: &SqlitePool) -> Result<RagIndexStatus, String> {
    let row: Option<RagStatusRow> = sqlx::query_as(
        "SELECT last_indexed_at, total_documents, total_chunks, embedding_model, status
             FROM rag_index_status WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    match row {
        Some((at, docs, chunks, model, status)) => Ok(RagIndexStatus {
            last_indexed_at: at,
            total_documents: docs,
            total_chunks: chunks,
            embedding_model: model.unwrap_or_default(),
            status: status.unwrap_or_else(|| "idle".to_string()),
        }),
        None => Ok(RagIndexStatus {
            last_indexed_at: None,
            total_documents: 0,
            total_chunks: 0,
            embedding_model: String::new(),
            status: "idle".to_string(),
        }),
    }
}

/// Update index status after a full reindex.
pub async fn update_index_status(
    pool: &SqlitePool,
    status: &str,
    docs: i32,
    chunks: i32,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO rag_index_status (id, last_indexed_at, total_documents, total_chunks, embedding_model, status)
         VALUES (1, datetime('now'), ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
           last_indexed_at=datetime('now'),
           total_documents=excluded.total_documents,
           total_chunks=excluded.total_chunks,
           embedding_model=excluded.embedding_model,
           status=excluded.status",
    )
    .bind(docs)
    .bind(chunks)
    .bind(EMBEDDING_MODEL)
    .bind(status)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Build RAG context string from search results to inject into the system prompt.
pub fn build_rag_context(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }

    let mut context = String::from("\n\n--- Relevant knowledge base context ---\n");
    let mut total_chars = 0;

    for (i, r) in results.iter().enumerate() {
        let entry = format!(
            "\n[{}] Source: {} | Title: {}\n{}\n",
            i + 1,
            r.document_id.split(':').next().unwrap_or("unknown"),
            r.title,
            r.content,
        );
        total_chars += entry.len();
        if total_chars > MAX_CONTEXT_CHARS {
            break;
        }
        context.push_str(&entry);
    }

    context.push_str("\n--- End of context ---\n");
    context
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_basic() {
        let text = "word ".repeat(1000);
        let chunks = chunk_text(text.trim());
        assert!(chunks.len() > 1, "should produce multiple chunks");
        // All chunks should have sequential indices
        for (i, (_, idx)) in chunks.iter().enumerate() {
            assert_eq!(*idx, i);
        }
    }

    #[test]
    fn test_chunk_text_empty() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   ").is_empty());
    }

    #[test]
    fn test_chunk_text_short_text() {
        let text = "Hello world, this is a short text.";
        let chunks = chunk_text(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, text);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn test_embedding_roundtrip() {
        let original = vec![1.5, -2.3, 0.0, 42.7];
        let bytes = embedding_to_bytes(&original);
        let recovered = bytes_to_embedding(&bytes);
        assert_eq!(original.len(), recovered.len());
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("hello world") >= 2);
        assert!(estimate_tokens("") == 0);
    }

    #[test]
    fn test_build_rag_context_empty() {
        assert!(build_rag_context(&[]).is_empty());
    }

    #[test]
    fn test_build_rag_context_includes_results() {
        let results = vec![SearchResult {
            chunk_id: "c1".into(),
            document_id: "catalog:jellyfin".into(),
            title: "Jellyfin".into(),
            content: "Free media server".into(),
            score: 0.95,
        }];
        let ctx = build_rag_context(&results);
        assert!(ctx.contains("Jellyfin"));
        assert!(ctx.contains("Free media server"));
    }
}
