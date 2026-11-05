/* =============================================================================
 * File          : quiescence_hook_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for snapshot quiescence hooks (migration 085): a name, a pre
 *   argv and a post argv, stored as JSON arrays. The scheduler turns enabled
 *   rows into `snapshot_quiescence::QuiesceHook`.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), serde_json
 *
 * Usage:
 *   `QuiescenceHookRepository` from handlers/snapshot_quiescence.rs and
 *   snapshot_scheduler.rs; see docs/guides/snapshots.md.
 * =============================================================================
 */

//! Snapshot quiescence-hook rows.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct QuiescenceHookRow {
    pub id: String,
    pub name: String,
    pub pre_json: String,
    pub post_json: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl QuiescenceHookRow {
    pub fn pre(&self) -> Vec<String> {
        serde_json::from_str(&self.pre_json).unwrap_or_default()
    }

    pub fn post(&self) -> Vec<String> {
        serde_json::from_str(&self.post_json).unwrap_or_default()
    }
}

pub struct QuiescenceHookRepository {
    pool: Arc<SqlitePool>,
}

impl QuiescenceHookRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<QuiescenceHookRow>, sqlx::Error> {
        sqlx::query_as::<_, QuiescenceHookRow>(
            "SELECT id, name, pre_json, post_json, enabled, created_at, updated_at \
             FROM snapshot_quiescence_hooks ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn list_enabled(&self) -> Result<Vec<QuiescenceHookRow>, sqlx::Error> {
        sqlx::query_as::<_, QuiescenceHookRow>(
            "SELECT id, name, pre_json, post_json, enabled, created_at, updated_at \
             FROM snapshot_quiescence_hooks WHERE enabled = 1 ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn get(&self, id: &str) -> Result<Option<QuiescenceHookRow>, sqlx::Error> {
        sqlx::query_as::<_, QuiescenceHookRow>(
            "SELECT id, name, pre_json, post_json, enabled, created_at, updated_at \
             FROM snapshot_quiescence_hooks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await
    }

    pub async fn create(
        &self,
        name: &str,
        pre: &[String],
        post: &[String],
    ) -> Result<QuiescenceHookRow, sqlx::Error> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO snapshot_quiescence_hooks \
             (id, name, pre_json, post_json, enabled, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(serde_json::to_string(pre).unwrap_or_else(|_| "[]".to_string()))
        .bind(serde_json::to_string(post).unwrap_or_else(|_| "[]".to_string()))
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.get(&id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn update(
        &self,
        id: &str,
        name: &str,
        pre: &[String],
        post: &[String],
        enabled: bool,
    ) -> Result<QuiescenceHookRow, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE snapshot_quiescence_hooks SET name = ?, pre_json = ?, post_json = ?, \
             enabled = ?, updated_at = ? WHERE id = ?",
        )
        .bind(name)
        .bind(serde_json::to_string(pre).unwrap_or_else(|_| "[]".to_string()))
        .bind(serde_json::to_string(post).unwrap_or_else(|_| "[]".to_string()))
        .bind(enabled)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        self.get(id).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM snapshot_quiescence_hooks WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
