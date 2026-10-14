/* =============================================================================
 * File          : recent_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Explorer recent locations, per user, for the quick-access card.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Explorer recent locations (per user).

use crate::impl_repository;
use crate::storage::database::DbError;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecentEntry {
    pub path: String,
    pub label: String,
    pub last_opened_at: String,
}

#[derive(Clone)]
pub struct RecentRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(RecentRepository);

impl RecentRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    fn label_for(path: &str) -> String {
        path.split('/')
            .rfind(|s| !s.is_empty())
            .unwrap_or(path)
            .to_string()
    }

    pub async fn list(&self, username: &str, limit: i64) -> Result<Vec<RecentEntry>, DbError> {
        let limit = limit.clamp(1, 50);
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT path, last_opened_at FROM explorer_recents WHERE username = ? ORDER BY last_opened_at DESC LIMIT ?",
        )
        .bind(username)
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(path, last_opened_at)| {
                let label = Self::label_for(&path);
                RecentEntry {
                    path,
                    label,
                    last_opened_at,
                }
            })
            .collect())
    }

    pub async fn touch(&self, username: &str, path: &str) -> Result<(), DbError> {
        let path = path.trim();
        if path.is_empty() || path.len() > 4096 || path.contains('\0') {
            return Err(DbError::InvalidInput("Invalid path".to_string()));
        }
        sqlx::query(
            "INSERT INTO explorer_recents (username, path, last_opened_at) VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(username, path) DO UPDATE SET last_opened_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
        )
        .bind(username)
        .bind(path)
        .execute(self.pool.as_ref())
        .await?;
        // Keep the list bounded: latest 50 per user.
        sqlx::query(
            "DELETE FROM explorer_recents WHERE username = ? AND id NOT IN (
                 SELECT id FROM explorer_recents WHERE username = ? ORDER BY last_opened_at DESC LIMIT 50
             )",
        )
        .bind(username)
        .bind(username)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn clear(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM explorer_recents WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn purge_user(&self, username: &str) -> Result<(), DbError> {
        self.clear(username).await
    }
}
