/* =============================================================================
 * File          : sync_job_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled cloud-sync jobs (rclone): source/remote pair, cadence
 *   and last-run state.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Scheduled cloud-sync jobs (rclone).

use crate::impl_repository;
use crate::storage::database::DbError;
use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

/// A saved Cloud Sync (rclone) job definition.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SyncJob {
    pub id: i64,
    pub name: String,
    pub source_type: String,
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: String,
    pub destination: String,
    pub dest_path: Option<String>,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
}

/// Incoming payload for creating a job (mirrors [`SyncJob`] minus the
/// server-assigned id/timestamp).
#[derive(Debug, Clone)]
pub struct NewSyncJob {
    pub name: String,
    pub source_type: String,
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: String,
    pub destination: String,
    pub dest_path: Option<String>,
}

impl NewSyncJob {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.source.trim().is_empty()
            || self.destination.trim().is_empty()
        {
            return Err("name, source and destination are required".to_string());
        }
        if !matches!(self.source_type.as_str(), "cloud" | "local") {
            return Err("source type must be \"cloud\" or \"local\"".to_string());
        }
        if !matches!(self.dest_type.as_str(), "cloud" | "local") {
            return Err("destination type must be \"cloud\" or \"local\"".to_string());
        }
        Ok(())
    }
}

/// Repository for sync job persistence.
#[derive(Clone)]
pub struct SyncJobRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SyncJobRepository);

impl SyncJobRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// List every saved job, oldest first.
    pub async fn list_all(&self) -> Result<Vec<SyncJob>, DbError> {
        let jobs = sqlx::query_as::<_, SyncJob>(
            "SELECT id, name, source_type, source, source_path, dest_type, destination, dest_path, created_at
             FROM sync_jobs
             ORDER BY id ASC",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(jobs)
    }

    /// Load a single job, if it exists.
    pub async fn get_by_id(&self, id: i64) -> Result<Option<SyncJob>, DbError> {
        let job = sqlx::query_as::<_, SyncJob>(
            "SELECT id, name, source_type, source, source_path, dest_type, destination, dest_path, created_at
             FROM sync_jobs
             WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(job)
    }

    /// Persist a new job and return its id.
    pub async fn insert(&self, job: &NewSyncJob) -> Result<i64, DbError> {
        let now = Utc::now();
        let result = sqlx::query(
            "INSERT INTO sync_jobs
                (name, source_type, source, source_path, dest_type, destination, dest_path, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(job.name.trim())
        .bind(&job.source_type)
        .bind(job.source.trim())
        .bind(&job.source_path)
        .bind(&job.dest_type)
        .bind(job.destination.trim())
        .bind(&job.dest_path)
        .bind(now.to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Delete a job by id; returns whether a row was removed.
    pub async fn delete(&self, id: i64) -> Result<bool, DbError> {
        let result = sqlx::query("DELETE FROM sync_jobs WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
