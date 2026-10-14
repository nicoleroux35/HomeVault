/* =============================================================================
 * File          : whats_new_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-user what's-new acknowledgements (version + user rows) so the
 *   release modal shows once per version per account.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Per-user what's-new acknowledgements.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Persistence for per-user "What's New" acknowledgement.
///
/// One row per `(user_id, version)` records that the user has dismissed the
/// release modal for that version. `user_id` is the `users.username` primary
/// key — HomeVault has no surrogate user id.
pub struct WhatsNewRepository {
    pool: Arc<SqlitePool>,
}

impl WhatsNewRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Whether `user_id` has acknowledged `version`.
    pub async fn has_seen(&self, user_id: &str, version: &str) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM whats_new_seen WHERE user_id = ? AND version = ?",
        )
        .bind(user_id)
        .bind(version)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    /// Records acknowledgement; idempotent.
    pub async fn mark_seen(&self, user_id: &str, version: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO whats_new_seen (user_id, version, seen_at) VALUES (?, ?, ?)
             ON CONFLICT(user_id, version) DO UPDATE SET seen_at = excluded.seen_at",
        )
        .bind(user_id)
        .bind(version)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}
