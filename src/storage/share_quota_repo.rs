/* =============================================================================
 * File          : share_quota_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for per-share size quotas: the declared intent, keyed by the
 *   share's name and protocol. The enforcement lives in the filesystem (see
 *   `crate::share_quota`), so this table holds one number per share and
 *   nothing else.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   src/share_quota.rs (inspect/apply) and handlers/share_quota.rs.
 * =============================================================================
 */

//! Per-share quota storage (`share_quotas`, migration 066).

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::storage::database::DbError;

#[derive(Clone)]
pub struct ShareQuotaRepository {
    pool: Arc<SqlitePool>,
}

impl ShareQuotaRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// The declared limit for one share, in bytes. `None` = no limit.
    pub async fn quota(&self, name: &str, share_type: &str) -> Result<Option<u64>, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT quota_bytes FROM share_quotas WHERE share_name = ? AND share_type = ?",
        )
        .bind(name)
        .bind(share_type)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|(bytes,)| bytes.max(0) as u64).filter(|b| *b > 0))
    }

    /// Every declared limit, as `(share, type) -> bytes`.
    ///
    /// Read once by the Explorer: a transfer must be checked against the quota
    /// of the share its destination lives in, and looking that up per file
    /// would be a query per file.
    pub async fn all(&self) -> Result<Vec<(String, String, u64)>, DbError> {
        let rows = sqlx::query_as::<_, (String, String, i64)>(
            "SELECT share_name, share_type, quota_bytes FROM share_quotas ORDER BY share_name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, _, bytes)| *bytes > 0)
            .map(|(name, kind, bytes)| (name, kind, bytes as u64))
            .collect())
    }

    /// Set (or clear, with `0`) the limit of one share.
    ///
    /// Clearing deletes the row rather than storing a zero, so "no quota" has
    /// exactly one representation on disk.
    pub async fn set(&self, name: &str, share_type: &str, quota_bytes: u64) -> Result<(), DbError> {
        if quota_bytes == 0 {
            return self.clear(name, share_type).await;
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO share_quotas (share_name, share_type, quota_bytes, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(share_name, share_type) DO UPDATE SET \
                quota_bytes = excluded.quota_bytes, \
                updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(share_type)
        .bind(quota_bytes as i64)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn clear(&self, name: &str, share_type: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM share_quotas WHERE share_name = ? AND share_type = ?")
            .bind(name)
            .bind(share_type)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Drop every row for a share — called when the share itself is deleted, so
    /// a re-created share with the same name cannot inherit a forgotten limit.
    pub async fn clear_all_for(&self, name: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM share_quotas WHERE share_name = ?")
            .bind(name)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }
}
