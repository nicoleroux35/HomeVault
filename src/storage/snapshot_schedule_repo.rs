/* =============================================================================
 * File          : snapshot_schedule_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for scheduled pool snapshots (cron + GFS retention counts),
 *   the persistence half of snapshot_scheduler.rs.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   handlers/snapshot_schedules.rs (HTTP) and snapshot_scheduler.rs (loop).
 * =============================================================================
 */

//! Snapshot schedule rows (cron + GFS retention) storage.

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// One scheduled-snapshot configuration for one pool.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SnapshotSchedule {
    pub id: String,
    pub pool: String,
    pub enabled: bool,
    pub cron: String,
    pub prefix: String,
    pub recursive: bool,
    pub gfs_hourly: u32,
    pub gfs_daily: u32,
    pub gfs_weekly: u32,
    pub gfs_monthly: u32,
    pub gfs_yearly: u32,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// The SQL row (bools/ints as they come out of SQLite).
#[derive(sqlx::FromRow)]
struct SnapshotScheduleRow {
    id: String,
    pool: String,
    enabled: i64,
    cron: String,
    prefix: String,
    recursive: i64,
    gfs_hourly: i64,
    gfs_daily: i64,
    gfs_weekly: i64,
    gfs_monthly: i64,
    gfs_yearly: i64,
    last_run_at: Option<String>,
    last_status: Option<String>,
    last_error: Option<String>,
    next_run_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<SnapshotScheduleRow> for SnapshotSchedule {
    fn from(r: SnapshotScheduleRow) -> Self {
        Self {
            id: r.id,
            pool: r.pool,
            enabled: r.enabled != 0,
            cron: r.cron,
            prefix: r.prefix,
            recursive: r.recursive != 0,
            gfs_hourly: r.gfs_hourly as u32,
            gfs_daily: r.gfs_daily as u32,
            gfs_weekly: r.gfs_weekly as u32,
            gfs_monthly: r.gfs_monthly as u32,
            gfs_yearly: r.gfs_yearly as u32,
            last_run_at: r.last_run_at,
            last_status: r.last_status,
            last_error: r.last_error,
            next_run_at: r.next_run_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub struct SnapshotScheduleRepository {
    pool: Arc<SqlitePool>,
}

impl SnapshotScheduleRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<SnapshotSchedule>, sqlx::Error> {
        sqlx::query_as::<_, SnapshotScheduleRow>(
            "SELECT id, pool, enabled, cron, prefix, recursive, gfs_hourly, gfs_daily, \
             gfs_weekly, gfs_monthly, gfs_yearly, last_run_at, last_status, last_error, \
             next_run_at, created_at, updated_at \
             FROM snapshot_schedules ORDER BY pool, prefix",
        )
        .fetch_all(self.pool.as_ref())
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_enabled(&self) -> Result<Vec<SnapshotSchedule>, sqlx::Error> {
        sqlx::query_as::<_, SnapshotScheduleRow>(
            "SELECT id, pool, enabled, cron, prefix, recursive, gfs_hourly, gfs_daily, \
             gfs_weekly, gfs_monthly, gfs_yearly, last_run_at, last_status, last_error, \
             next_run_at, created_at, updated_at \
             FROM snapshot_schedules WHERE enabled = 1 ORDER BY pool",
        )
        .fetch_all(self.pool.as_ref())
        .await
        .map(|rows| rows.into_iter().map(Into::into).collect())
    }

    pub async fn get(&self, id: &str) -> Result<Option<SnapshotSchedule>, sqlx::Error> {
        sqlx::query_as::<_, SnapshotScheduleRow>(
            "SELECT id, pool, enabled, cron, prefix, recursive, gfs_hourly, gfs_daily, \
             gfs_weekly, gfs_monthly, gfs_yearly, last_run_at, last_status, last_error, \
             next_run_at, created_at, updated_at \
             FROM snapshot_schedules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await
        .map(|r| r.map(Into::into))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        pool: &str,
        cron: &str,
        prefix: &str,
        recursive: bool,
        policy: crate::snapshot_scheduler::GfsPolicy,
        next_run_at: Option<&str>,
    ) -> Result<SnapshotSchedule, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO snapshot_schedules (id, pool, enabled, cron, prefix, recursive, \
             gfs_hourly, gfs_daily, gfs_weekly, gfs_monthly, gfs_yearly, next_run_at, \
             created_at, updated_at) VALUES (?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(pool)
        .bind(cron)
        .bind(prefix)
        .bind(recursive)
        .bind(policy.hourly as i64)
        .bind(policy.daily as i64)
        .bind(policy.weekly as i64)
        .bind(policy.monthly as i64)
        .bind(policy.yearly as i64)
        .bind(next_run_at)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(self.get(&id).await?.expect("row just inserted"))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: &str,
        pool: &str,
        cron: &str,
        prefix: &str,
        recursive: bool,
        enabled: bool,
        policy: crate::snapshot_scheduler::GfsPolicy,
        next_run_at: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE snapshot_schedules SET pool = ?, cron = ?, prefix = ?, recursive = ?, \
             enabled = ?, gfs_hourly = ?, gfs_daily = ?, gfs_weekly = ?, gfs_monthly = ?, \
             gfs_yearly = ?, next_run_at = ?, last_status = NULL, last_error = NULL, \
             updated_at = ? WHERE id = ?",
        )
        .bind(pool)
        .bind(cron)
        .bind(prefix)
        .bind(recursive)
        .bind(enabled)
        .bind(policy.hourly as i64)
        .bind(policy.daily as i64)
        .bind(policy.weekly as i64)
        .bind(policy.monthly as i64)
        .bind(policy.yearly as i64)
        .bind(next_run_at)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM snapshot_schedules WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn mark_finished(
        &self,
        id: &str,
        success: bool,
        message: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let (status, error) = if success {
            ("success", None)
        } else {
            ("failed", Some(message))
        };
        sqlx::query(
            "UPDATE snapshot_schedules SET last_run_at = ?, last_status = ?, last_error = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&now)
        .bind(status)
        .bind(error)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn set_next_run(&self, id: &str, next_run_at: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE snapshot_schedules SET next_run_at = ? WHERE id = ?")
            .bind(next_run_at)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> SnapshotScheduleRepository {
        let pool = Arc::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap(),
        );
        sqlx::query(
            "CREATE TABLE snapshot_schedules (id TEXT PRIMARY KEY, pool TEXT NOT NULL, \
             enabled INTEGER NOT NULL DEFAULT 1, cron TEXT NOT NULL, prefix TEXT NOT NULL, \
             recursive INTEGER NOT NULL DEFAULT 1, gfs_hourly INTEGER NOT NULL, \
             gfs_daily INTEGER NOT NULL, gfs_weekly INTEGER NOT NULL, \
             gfs_monthly INTEGER NOT NULL, gfs_yearly INTEGER NOT NULL, \
             last_run_at TEXT, last_status TEXT, last_error TEXT, next_run_at TEXT, \
             created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
        )
        .execute(pool.as_ref())
        .await
        .unwrap();
        SnapshotScheduleRepository::new(pool)
    }

    #[tokio::test]
    async fn create_and_get_round_trip() {
        let repo = repo().await;
        let schedule = repo
            .create(
                "tank",
                "0 */4 * * *",
                "hv-auto",
                true,
                crate::snapshot_scheduler::GfsPolicy {
                    hourly: 24,
                    daily: 7,
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(schedule.pool, "tank");
        assert_eq!(schedule.gfs_hourly, 24);
        assert!(schedule.enabled);
        assert_eq!(repo.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn update_resets_status_and_toggles_enabled() {
        let repo = repo().await;
        let s = repo
            .create(
                "tank",
                "* * * * *",
                "hv-auto",
                true,
                Default::default(),
                None,
            )
            .await
            .unwrap();
        repo.mark_finished(&s.id, false, "boom").await.unwrap();
        repo.update(
            &s.id,
            "tank",
            "0 3 * * *",
            "hv-auto",
            false,
            false,
            Default::default(),
            None,
        )
        .await
        .unwrap();
        let edited = repo.get(&s.id).await.unwrap().unwrap();
        assert!(!edited.enabled);
        assert!(edited.last_status.is_none(), "edit must reset last_status");
        assert!(repo.list_enabled().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_removes_the_row() {
        let repo = repo().await;
        let s = repo
            .create(
                "tank",
                "* * * * *",
                "hv-auto",
                true,
                Default::default(),
                None,
            )
            .await
            .unwrap();
        repo.delete(&s.id).await.unwrap();
        assert!(repo.get(&s.id).await.unwrap().is_none());
    }
}
