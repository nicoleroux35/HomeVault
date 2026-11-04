/* =============================================================================
 * File          : repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SQLite persistence and FTS5 queries for the content index (phase D1).
 *   Documents and their postings are kept in sync by triggers; the module
 *   only ever writes the `content_index_docs` row.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   crate::content_index (the worker) and handlers/content_index.rs (search).
 * ============================================================================= */

//! Documents, FTS5 search and retention for the content index.

use sqlx::SqlitePool;
use std::collections::HashMap;

/// A file as the walker sees it (metadata only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDoc {
    pub path: String,
    pub root: String,
    pub name: String,
    pub size_bytes: i64,
    pub modified: i64,
}

/// One search hit. `snippet` is produced by FTS5 over the body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ContentHit {
    pub path: String,
    pub root: String,
    pub name: String,
    pub size_bytes: i64,
    pub modified: i64,
    pub snippet: String,
}

/// `path -> (modified, size)` for every indexed document. One query per run,
/// so the common case (nothing changed) never reads a file.
pub async fn unchanged(pool: &SqlitePool) -> Result<HashMap<String, (i64, i64)>, sqlx::Error> {
    let rows: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT path, modified, size_bytes FROM content_index_docs")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(path, modified, size)| (path, (modified, size)))
        .collect())
}

/// Insert or replace one document. The update trigger refreshes the postings
/// when `name`/`body` change; a metadata-only touch below does not.
pub async fn upsert(
    pool: &SqlitePool,
    doc: &ContentDoc,
    body: &str,
    seen_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO content_index_docs \
            (path, root, name, body, size_bytes, modified, last_seen, indexed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(path) DO UPDATE SET \
            root = excluded.root, \
            name = excluded.name, \
            body = excluded.body, \
            size_bytes = excluded.size_bytes, \
            modified = excluded.modified, \
            last_seen = excluded.last_seen, \
            indexed_at = excluded.indexed_at",
    )
    .bind(&doc.path)
    .bind(&doc.root)
    .bind(&doc.name)
    .bind(body)
    .bind(doc.size_bytes)
    .bind(doc.modified)
    .bind(seen_at)
    .bind(seen_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an unchanged document as seen by this run.
pub async fn touch(pool: &SqlitePool, path: &str, seen_at: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE content_index_docs SET last_seen = ? WHERE path = ?")
        .bind(seen_at)
        .bind(path)
        .execute(pool)
        .await?;
    Ok(())
}

/// Drop every document under `root` that this run did not see. Called only
/// after the root was walked completely, so a permission error cannot delete
/// an index the worker simply could not refresh.
pub async fn prune_root(pool: &SqlitePool, root: &str, seen_at: &str) -> Result<u64, sqlx::Error> {
    let affected = sqlx::query("DELETE FROM content_index_docs WHERE root = ? AND last_seen < ?")
        .bind(root)
        .bind(seen_at)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected)
}

/// Retention: documents not seen before `cutoff` (ISO instant) are dropped.
/// This is the safety net for a root that stayed unreadable for longer than
/// the configured retention.
pub async fn prune_older_than(pool: &SqlitePool, cutoff: &str) -> Result<u64, sqlx::Error> {
    let affected = sqlx::query("DELETE FROM content_index_docs WHERE last_seen < ?")
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected)
}

/// Remove every document under one root (configuration change).
pub async fn delete_root(pool: &SqlitePool, root: &str) -> Result<u64, sqlx::Error> {
    let affected = sqlx::query("DELETE FROM content_index_docs WHERE root = ?")
        .bind(root)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected)
}

/// "Delete my index": every document, every posting.
pub async fn delete_all(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let affected = sqlx::query("DELETE FROM content_index_docs")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected)
}

/// `(documents, stored bytes)` for the status line.
pub async fn stats(pool: &SqlitePool) -> Result<(i64, i64), sqlx::Error> {
    let row: (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM content_index_docs")
            .fetch_one(pool)
            .await?;
    Ok(row)
}

/// The bounded FTS5 query. `expression` is built by
/// [`crate::content_index::search_expression`] — quoted literals only, never
/// raw user syntax.
pub async fn search(
    pool: &SqlitePool,
    expression: &str,
    limit: i64,
) -> Result<Vec<ContentHit>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, String)>(
        "SELECT d.path, d.root, d.name, d.size_bytes, d.modified, \
                snippet(content_index_fts, 1, '', '', '…', 24) AS snippet \
         FROM content_index_fts \
         JOIN content_index_docs d ON d.id = content_index_fts.rowid \
         WHERE content_index_fts MATCH ? \
         ORDER BY bm25(content_index_fts, 5.0, 1.0) \
         LIMIT ?",
    )
    .bind(expression)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(path, root, name, size_bytes, modified, snippet)| ContentHit {
                path,
                root,
                name,
                size_bytes,
                modified,
                snippet,
            },
        )
        .collect())
}
