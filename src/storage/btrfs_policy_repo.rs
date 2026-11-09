/* =============================================================================
 * File          : btrfs_policy_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for per-path Btrfs compression policies (migration 091): the
 *   requested zstd level, whether existing extents are defragmented, and when
 *   the policy was last applied.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   `BtrfsPolicyRepository` from handlers/btrfs_maintenance.rs and the
 *   maintenance loop; see docs/guides/storage.md.
 * =============================================================================
 */

//! Per-share Btrfs compression policies.

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, sqlx::FromRow, PartialEq, Eq)]
pub struct BtrfsCompressionPolicy {
    pub path: String,
    pub level: String,
    pub defrag: bool,
    pub last_applied_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct BtrfsPolicyRepository {
    pool: Arc<SqlitePool>,
}

impl BtrfsPolicyRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<BtrfsCompressionPolicy>, sqlx::Error> {
        sqlx::query_as::<_, BtrfsCompressionPolicy>(
            "SELECT path, level, defrag, last_applied_at, created_at, updated_at \
             FROM btrfs_compression_policies ORDER BY path",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn get(&self, path: &str) -> Result<Option<BtrfsCompressionPolicy>, sqlx::Error> {
        sqlx::query_as::<_, BtrfsCompressionPolicy>(
            "SELECT path, level, defrag, last_applied_at, created_at, updated_at \
             FROM btrfs_compression_policies WHERE path = ?",
        )
        .bind(path)
        .fetch_optional(self.pool.as_ref())
        .await
    }

    pub async fn upsert(
        &self,
        path: &str,
        level: &str,
        defrag: bool,
    ) -> Result<BtrfsCompressionPolicy, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO btrfs_compression_policies \
             (path, level, defrag, created_at, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(path) DO UPDATE SET \
             level = excluded.level, defrag = excluded.defrag, updated_at = excluded.updated_at",
        )
        .bind(path)
        .bind(level)
        .bind(defrag)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.get(path).await?.ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn delete(&self, path: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM btrfs_compression_policies WHERE path = ?")
            .bind(path)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_applied(&self, path: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE btrfs_compression_policies SET last_applied_at = ?, updated_at = ? WHERE path = ?",
        )
        .bind(&now)
        .bind(&now)
        .bind(path)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}
