/* =============================================================================
 * File          : password_history_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Password history entries per user, powering the reuse-prevention
 *   check when passwords change.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Password history (reuse-prevention).

use crate::impl_repository;
use crate::storage::database::DbError;
use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

/// A single password hash that was previously used by a user.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PasswordHistoryEntry {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: String,
}

/// Repository for the password history table (reuse prevention).
#[derive(Clone)]
pub struct PasswordHistoryRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(PasswordHistoryRepository);

impl PasswordHistoryRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Record a password hash in the history.
    pub async fn add_entry(&self, username: &str, password_hash: &str) -> Result<(), DbError> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO password_history (username, password_hash, created_at) VALUES (?, ?, ?)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(now.to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Return the most recent password hashes (up to `limit`) for a user.
    pub async fn get_history(&self, username: &str, limit: usize) -> Result<Vec<String>, DbError> {
        let entries = sqlx::query_as::<_, PasswordHistoryEntry>(
            "SELECT id, username, password_hash, created_at FROM password_history WHERE username = ? ORDER BY created_at DESC LIMIT ?"
        )
        .bind(username)
        .bind(limit as i64)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(entries.into_iter().map(|e| e.password_hash).collect())
    }

    /// Keep only the most recent `keep_count` entries for a user.
    pub async fn prune_old_entries(
        &self,
        username: &str,
        keep_count: usize,
    ) -> Result<u64, DbError> {
        let result = sqlx::query(
            "DELETE FROM password_history WHERE username = ? AND id NOT IN (SELECT id FROM password_history WHERE username = ? ORDER BY created_at DESC LIMIT ?)"
        )
        .bind(username)
        .bind(username)
        .bind(keep_count as i64)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected())
    }

    /// Number of history entries for a user.
    pub async fn count_entries(&self, username: &str) -> Result<i64, DbError> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM password_history WHERE username = ?")
                .bind(username)
                .fetch_one(self.pool.as_ref())
                .await?;

        Ok(row.0)
    }

    /// Delete all history entries for a user.
    pub async fn delete_all_for_user(&self, username: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM password_history WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected())
    }

    /// Check whether a specific hash exists in a user's history.
    pub async fn hash_exists(&self, username: &str, password_hash: &str) -> Result<bool, DbError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM password_history WHERE username = ? AND password_hash = ?",
        )
        .bind(username)
        .bind(password_hash)
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(row.0 > 0)
    }
}
