/* =============================================================================
 * File          : snapshot_lock_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for immutable-snapshot locks: create, list active/historical rows,
 *   mark a row released. Kept separate from the command layer so the API and
 *   the release loop share one persistence path.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::snapshot_locks
 *
 * Usage:
 *   handlers/snapshot_locks.rs, snapshot_scheduler.rs, snapshot_lock_scheduler.rs.
 * =============================================================================
 */

//! Immutable-snapshot lock storage (`snapshot_locks`, migration 068).

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::snapshot_locks::{SnapshotLock, LOCK_TAG};
use crate::storage::database::DbError;

#[derive(Clone)]
pub struct SnapshotLockRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SnapshotLockRepository);

impl SnapshotLockRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Active locks (unreleased), optionally filtered to one pool.
    pub async fn active(&self, pool: Option<&str>) -> Result<Vec<SnapshotLock>, DbError> {
        match pool {
            Some(pool) => Ok(sqlx::query_as::<_, SnapshotLock>(
                "SELECT * FROM snapshot_locks WHERE released_at IS NULL AND pool = ? \
                 ORDER BY locked_until",
            )
            .bind(pool)
            .fetch_all(self.pool.as_ref())
            .await?),
            None => Ok(sqlx::query_as::<_, SnapshotLock>(
                "SELECT * FROM snapshot_locks WHERE released_at IS NULL ORDER BY pool, locked_until",
            )
            .fetch_all(self.pool.as_ref())
            .await?),
        }
    }

    /// Every row, released or not — the audit history the UI shows on demand.
    pub async fn history(&self, limit: i64) -> Result<Vec<SnapshotLock>, DbError> {
        Ok(sqlx::query_as::<_, SnapshotLock>(
            "SELECT * FROM snapshot_locks ORDER BY id DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    pub async fn find(&self, id: i64) -> Result<Option<SnapshotLock>, DbError> {
        Ok(
            sqlx::query_as::<_, SnapshotLock>("SELECT * FROM snapshot_locks WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool.as_ref())
                .await?,
        )
    }

    /// Whether this exact snapshot already has an unreleased lock.
    pub async fn active_for(
        &self,
        pool: &str,
        snapshot: &str,
    ) -> Result<Option<SnapshotLock>, DbError> {
        let bare = snapshot.rsplit('@').next().unwrap_or(snapshot);
        Ok(sqlx::query_as::<_, SnapshotLock>(
            "SELECT * FROM snapshot_locks WHERE released_at IS NULL AND pool = ? \
             AND (snapshot = ? OR snapshot = ? OR snapshot LIKE '%@' || ?) LIMIT 1",
        )
        .bind(pool)
        .bind(snapshot)
        .bind(bare)
        .bind(bare)
        .fetch_optional(self.pool.as_ref())
        .await?)
    }

    /// Persist the row **after** the filesystem hold succeeded, so a row can
    /// never claim protection the pool does not actually provide.
    pub async fn create(
        &self,
        pool: &str,
        snapshot: &str,
        locked_until: &str,
        reason: Option<&str>,
        created_by: &str,
    ) -> Result<SnapshotLock, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO snapshot_locks \
             (pool, snapshot, tag, locked_until, reason, created_by, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(pool)
        .bind(snapshot)
        .bind(LOCK_TAG)
        .bind(locked_until)
        .bind(reason)
        .bind(created_by)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.find(result.last_insert_rowid())
            .await?
            .ok_or(DbError::NotFound)
    }

    /// Mark one lock released. `reason` records a break-glass release.
    pub async fn mark_released(
        &self,
        id: i64,
        released_by: &str,
        reason: Option<&str>,
    ) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE snapshot_locks SET released_at = ?, released_by = ?, release_reason = ? \
             WHERE id = ? AND released_at IS NULL",
        )
        .bind(&now)
        .bind(released_by)
        .bind(reason)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Drop every unreleased lock of a pool whose snapshot no longer exists
    /// (deleted out-of-band). Without this a stale row would protect a name
    /// that can never reappear, and the release loop would retry forever.
    pub async fn forget(
        &self,
        pool: &str,
        snapshot: &str,
        released_by: &str,
        reason: &str,
    ) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE snapshot_locks SET released_at = ?, released_by = ?, release_reason = ? \
             WHERE released_at IS NULL AND pool = ? AND snapshot = ?",
        )
        .bind(&now)
        .bind(released_by)
        .bind(reason)
        .bind(pool)
        .bind(snapshot)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}
