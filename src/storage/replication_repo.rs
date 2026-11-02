/* =============================================================================
 * File          : replication_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for snapshot-replication jobs: CRUD, due-job selection and the
 *   run outcome bookkeeping (status, error, last replicated snapshot, next
 *   run).
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::replication
 *
 * Usage:
 *   handlers/replication.rs, replication_scheduler.rs.
 * =============================================================================
 */

//! Snapshot-replication job storage (`replication_jobs`, migration 069).

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::replication::ReplicationJob;
use crate::storage::database::DbError;

#[derive(Clone)]
pub struct ReplicationRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(ReplicationRepository);

impl ReplicationRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<ReplicationJob>, DbError> {
        Ok(sqlx::query_as::<_, ReplicationJob>(
            "SELECT * FROM replication_jobs ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    pub async fn find(&self, id: i64) -> Result<Option<ReplicationJob>, DbError> {
        Ok(
            sqlx::query_as::<_, ReplicationJob>("SELECT * FROM replication_jobs WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool.as_ref())
                .await?,
        )
    }

    /// Whether another job already uses this name (case-insensitively).
    pub async fn name_taken(&self, name: &str, except: Option<i64>) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM replication_jobs WHERE name = ? COLLATE NOCASE AND id IS NOT ?",
        )
        .bind(name)
        .bind(except)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    /// Insert one job. `next_run_at` is derived by the caller (validated).
    pub async fn create(&self, job: &ReplicationJob) -> Result<ReplicationJob, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO replication_jobs \
             (name, source_dataset, backend, destination_kind, destination_dataset, \
              destination_host, destination_user, destination_port, interval_minutes, \
              keep_last, bandwidth_kbps, enabled, next_run_at, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&job.name)
        .bind(&job.source_dataset)
        .bind(&job.backend)
        .bind(&job.destination_kind)
        .bind(&job.destination_dataset)
        .bind(&job.destination_host)
        .bind(&job.destination_user)
        .bind(job.destination_port)
        .bind(job.interval_minutes)
        .bind(job.keep_last)
        .bind(job.bandwidth_kbps)
        .bind(job.enabled)
        .bind(&job.next_run_at)
        .bind(&job.created_by)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.find(result.last_insert_rowid() as i64)
            .await?
            .ok_or(DbError::NotFound)
    }

    pub async fn update(&self, job: &ReplicationJob) -> Result<ReplicationJob, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE replication_jobs SET name = ?, source_dataset = ?, backend = ?, \
             destination_kind = ?, destination_dataset = ?, destination_host = ?, \
             destination_user = ?, destination_port = ?, interval_minutes = ?, keep_last = ?, \
             bandwidth_kbps = ?, enabled = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&job.name)
        .bind(&job.source_dataset)
        .bind(&job.backend)
        .bind(&job.destination_kind)
        .bind(&job.destination_dataset)
        .bind(&job.destination_host)
        .bind(&job.destination_user)
        .bind(job.destination_port)
        .bind(job.interval_minutes)
        .bind(job.keep_last)
        .bind(job.bandwidth_kbps)
        .bind(job.enabled)
        .bind(&job.next_run_at)
        .bind(&now)
        .bind(job.id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.find(job.id).await?.ok_or(DbError::NotFound)
    }

    pub async fn delete(&self, id: i64) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM replication_jobs WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Jobs whose next run is due, oldest first. A job with no schedule
    /// (`interval_minutes = 0`) is never due.
    pub async fn due(&self, now: DateTime<Utc>) -> Result<Vec<ReplicationJob>, DbError> {
        Ok(sqlx::query_as::<_, ReplicationJob>(
            "SELECT * FROM replication_jobs \
             WHERE enabled = 1 AND interval_minutes > 0 AND next_run_at IS NOT NULL \
               AND next_run_at <= ? \
             ORDER BY next_run_at",
        )
        .bind(now.to_rfc3339())
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    /// Record one finished run. `next_run_at` is only advanced on success, so a
    /// failing job is retried on the next tick instead of being postponed by a
    /// whole interval.
    pub async fn record_run(
        &self,
        id: i64,
        success: bool,
        message: &str,
        replicated_snapshot: Option<&str>,
        next_run_at: Option<&str>,
    ) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE replication_jobs SET last_run_at = ?, last_status = ?, last_error = ?, \
             last_snapshot = COALESCE(?, last_snapshot), next_run_at = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(&now)
        .bind(if success { "success" } else { "failed" })
        .bind(if success { None } else { Some(message) })
        .bind(replicated_snapshot)
        .bind(next_run_at)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Set the next run without touching the last outcome (a "run now" request
    /// only brings the schedule forward).
    pub async fn set_next_run(&self, id: i64, next_run_at: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE replication_jobs SET next_run_at = ? WHERE id = ?")
            .bind(next_run_at)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}
