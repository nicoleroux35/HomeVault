/* =============================================================================
 * File          : cache_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Cache-tier storage: settings (cache pool, mover schedule and
 *   thresholds), cache shares and mover-run history.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for the disk cache tier.
//!
//! Three tables: a singleton `cache_settings` row, one `cache_shares` row per
//! cache-enabled dataset, and a `cache_mover_runs` history table.

use crate::cache::CacheMode;
use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// Singleton cache-tier configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheSettings {
    pub cache_pool: Option<String>,
    pub mover_cron: String,
    pub mover_enabled: bool,
    pub min_free_percent: u32,
    pub mover_last_run_at: Option<String>,
    /// Only move files modified more than this many days ago (0 = no age filter).
    pub mover_age_days: u32,
    /// Space/comma separated glob patterns to exclude from the mover.
    pub mover_excludes: Option<String>,
    /// rsync bandwidth limit in KB/s (0 = unlimited).
    pub mover_bwlimit: u32,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self {
            cache_pool: None,
            mover_cron: "0 3 * * *".to_string(),
            mover_enabled: false,
            min_free_percent: 20,
            mover_last_run_at: None,
            mover_age_days: 0,
            mover_excludes: None,
            mover_bwlimit: 0,
        }
    }
}

/// A single cache-enabled dataset ("cache share").
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheShare {
    pub id: String,
    pub name: String,
    pub cache_mode: String,
    pub target_pool: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// A recorded mover run (manual or scheduled).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheMoverRun {
    pub id: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub files_moved: u64,
    pub bytes_moved: u64,
    pub error_message: Option<String>,
    /// Per-share outcome, in processing order. Empty for runs recorded before
    /// migration 052.
    #[serde(default)]
    pub shares: Vec<CacheMoverRunShare>,
}

/// One share's outcome inside a mover run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheMoverRunShare {
    pub share_name: String,
    pub mode: String,
    pub status: String,
    pub files_moved: u64,
    pub bytes_moved: u64,
    pub message: Option<String>,
}

/// A per-share outcome about to be written for a run.
#[derive(Debug, Clone)]
pub struct ShareOutcomeRecord {
    pub share_name: String,
    pub mode: String,
    pub status: String,
    pub files_moved: u64,
    pub bytes_moved: u64,
    pub message: Option<String>,
}

pub struct CacheRepository {
    pool: Arc<SqlitePool>,
}

impl CacheRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Load settings, returning the default row if none exists yet.
    pub async fn get_settings(&self) -> Result<CacheSettings, sqlx::Error> {
        let row = sqlx::query_as::<_, CacheSettingsRow>(
            "SELECT cache_pool, mover_cron, mover_enabled, min_free_percent, mover_last_run_at, mover_age_days, mover_excludes, mover_bwlimit FROM cache_settings WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => CacheSettings::default(),
        })
    }

    /// Insert or replace the singleton settings row.
    pub async fn save_settings(&self, settings: &CacheSettings) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO cache_settings (id, cache_pool, mover_cron, mover_enabled, min_free_percent, mover_last_run_at, mover_age_days, mover_excludes, mover_bwlimit, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                cache_pool = excluded.cache_pool,
                mover_cron = excluded.mover_cron,
                mover_enabled = excluded.mover_enabled,
                min_free_percent = excluded.min_free_percent,
                mover_last_run_at = excluded.mover_last_run_at,
                mover_age_days = excluded.mover_age_days,
                mover_excludes = excluded.mover_excludes,
                mover_bwlimit = excluded.mover_bwlimit,
                updated_at = excluded.updated_at",
        )
        .bind(settings.cache_pool.as_deref())
        .bind(settings.mover_cron.as_str())
        .bind(settings.mover_enabled as i64)
        .bind(settings.min_free_percent as i64)
        .bind(settings.mover_last_run_at.as_deref())
        .bind(settings.mover_age_days as i64)
        .bind(settings.mover_excludes.as_deref())
        .bind(settings.mover_bwlimit as i64)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn set_mover_last_run(&self, at: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE cache_settings SET mover_last_run_at = ? WHERE id = 1")
            .bind(at)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn list_shares(&self) -> Result<Vec<CacheShare>, sqlx::Error> {
        let rows = sqlx::query_as::<_, CacheShareRow>(
            "SELECT id, name, cache_mode, target_pool, enabled, created_at, updated_at FROM cache_shares ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    pub async fn get_share(&self, name: &str) -> Result<Option<CacheShare>, sqlx::Error> {
        let row = sqlx::query_as::<_, CacheShareRow>(
            "SELECT id, name, cache_mode, target_pool, enabled, created_at, updated_at FROM cache_shares WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|r| r.into_model()))
    }

    pub async fn create_share(
        &self,
        name: &str,
        cache_mode: &str,
        target_pool: &str,
    ) -> Result<CacheShare, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO cache_shares (id, name, cache_mode, target_pool, enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(cache_mode)
        .bind(target_pool)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(CacheShare {
            id,
            name: name.to_string(),
            cache_mode: cache_mode.to_string(),
            target_pool: target_pool.to_string(),
            enabled: true,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_share(
        &self,
        name: &str,
        cache_mode: Option<&str>,
        target_pool: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<Option<CacheShare>, sqlx::Error> {
        let existing = self.get_share(name).await?;
        let Some(share) = existing else {
            return Ok(None);
        };
        let final_mode = cache_mode.unwrap_or(&share.cache_mode);
        let final_target = target_pool.unwrap_or(&share.target_pool);
        let final_enabled = enabled.unwrap_or(share.enabled);
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE cache_shares SET cache_mode = ?, target_pool = ?, enabled = ?, updated_at = ? WHERE name = ?",
        )
        .bind(final_mode)
        .bind(final_target)
        .bind(final_enabled as i64)
        .bind(&now)
        .bind(name)
        .execute(self.pool.as_ref())
        .await?;
        Ok(Some(CacheShare {
            cache_mode: final_mode.to_string(),
            target_pool: final_target.to_string(),
            enabled: final_enabled,
            updated_at: now,
            ..share
        }))
    }

    pub async fn delete_share(&self, name: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM cache_shares WHERE name = ?")
            .bind(name)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_runs(&self, limit: u32) -> Result<Vec<CacheMoverRun>, sqlx::Error> {
        let rows = sqlx::query_as::<_, CacheMoverRunRow>(
            "SELECT id, status, started_at, finished_at, files_moved, bytes_moved, error_message FROM cache_mover_runs ORDER BY started_at DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(self.pool.as_ref())
        .await?;

        // One extra query for the per-share rows of exactly these runs (no
        // N+1): the same `LIMIT` selects them, ordered for grouping.
        let share_rows = sqlx::query_as::<_, CacheMoverRunShareRow>(
            "SELECT run_id, share_name, mode, status, files_moved, bytes_moved, message
             FROM cache_mover_run_shares
             WHERE run_id IN (
                 SELECT id FROM cache_mover_runs ORDER BY started_at DESC LIMIT ?
             )
             ORDER BY run_id, share_name",
        )
        .bind(limit as i64)
        .fetch_all(self.pool.as_ref())
        .await?;

        let mut by_run: std::collections::HashMap<String, Vec<CacheMoverRunShare>> =
            std::collections::HashMap::new();
        for row in share_rows {
            let run_id = row.run_id.clone();
            by_run.entry(run_id).or_default().push(row.into_model());
        }

        Ok(rows
            .into_iter()
            .map(|r| {
                let mut run = r.into_model();
                run.shares = by_run.remove(&run.id).unwrap_or_default();
                run
            })
            .collect())
    }

    /// Record the per-share outcome of a run.
    pub async fn record_run_shares(
        &self,
        run_id: &str,
        shares: &[ShareOutcomeRecord],
    ) -> Result<(), sqlx::Error> {
        for share in shares {
            sqlx::query(
                "INSERT OR REPLACE INTO cache_mover_run_shares
                 (run_id, share_name, mode, status, files_moved, bytes_moved, message)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(run_id)
            .bind(&share.share_name)
            .bind(&share.mode)
            .bind(&share.status)
            .bind(share.files_moved as i64)
            .bind(share.bytes_moved as i64)
            .bind(share.message.as_deref())
            .execute(self.pool.as_ref())
            .await?;
        }
        Ok(())
    }

    /// Close out rows left on `running` by a previous process.
    ///
    /// The single-flight guard is process-local, so a restart in the middle of
    /// a move leaves its row `running` forever. The caller only invokes this
    /// while holding that guard, which proves nothing is actually running.
    pub async fn fail_stale_runs(&self, message: &str) -> Result<u64, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE cache_mover_runs SET status = 'failed', finished_at = ?, error_message = ? WHERE status = 'running'",
        )
        .bind(&now)
        .bind(message)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn create_run(&self, status: &str) -> Result<String, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO cache_mover_runs (id, status, started_at) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(status)
            .bind(&now)
            .execute(self.pool.as_ref())
            .await?;
        Ok(id)
    }

    pub async fn update_run(
        &self,
        id: &str,
        status: &str,
        files_moved: u64,
        bytes_moved: u64,
        error_message: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE cache_mover_runs SET status = ?, finished_at = ?, files_moved = ?, bytes_moved = ?, error_message = ? WHERE id = ?",
        )
        .bind(status)
        .bind(&now)
        .bind(files_moved as i64)
        .bind(bytes_moved as i64)
        .bind(error_message)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct CacheSettingsRow {
    cache_pool: Option<String>,
    mover_cron: String,
    mover_enabled: i64,
    min_free_percent: i64,
    mover_last_run_at: Option<String>,
    mover_age_days: i64,
    mover_excludes: Option<String>,
    mover_bwlimit: i64,
}

impl CacheSettingsRow {
    fn into_model(self) -> CacheSettings {
        CacheSettings {
            cache_pool: self.cache_pool,
            mover_cron: self.mover_cron,
            mover_enabled: self.mover_enabled != 0,
            min_free_percent: self.min_free_percent.max(0) as u32,
            mover_last_run_at: self.mover_last_run_at,
            mover_age_days: self.mover_age_days.max(0) as u32,
            mover_excludes: self.mover_excludes,
            mover_bwlimit: self.mover_bwlimit.max(0) as u32,
        }
    }
}

#[derive(sqlx::FromRow)]
struct CacheShareRow {
    id: String,
    name: String,
    cache_mode: String,
    target_pool: String,
    enabled: i64,
    created_at: String,
    updated_at: String,
}

impl CacheShareRow {
    fn into_model(self) -> CacheShare {
        CacheShare {
            id: self.id,
            name: self.name,
            cache_mode: self.cache_mode,
            target_pool: self.target_pool,
            enabled: self.enabled != 0,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct CacheMoverRunRow {
    id: String,
    status: String,
    started_at: String,
    finished_at: Option<String>,
    files_moved: i64,
    bytes_moved: i64,
    error_message: Option<String>,
}

impl CacheMoverRunRow {
    fn into_model(self) -> CacheMoverRun {
        CacheMoverRun {
            id: self.id,
            status: self.status,
            started_at: self.started_at,
            finished_at: self.finished_at,
            files_moved: self.files_moved.max(0) as u64,
            bytes_moved: self.bytes_moved.max(0) as u64,
            error_message: self.error_message,
            shares: Vec::new(),
        }
    }
}

#[derive(sqlx::FromRow)]
struct CacheMoverRunShareRow {
    run_id: String,
    share_name: String,
    mode: String,
    status: String,
    files_moved: i64,
    bytes_moved: i64,
    message: Option<String>,
}

impl CacheMoverRunShareRow {
    fn into_model(self) -> CacheMoverRunShare {
        CacheMoverRunShare {
            share_name: self.share_name,
            mode: self.mode,
            status: self.status,
            files_moved: self.files_moved.max(0) as u64,
            bytes_moved: self.bytes_moved.max(0) as u64,
            message: self.message,
        }
    }
}

/// Validate that `mode` is one of the four supported cache modes.
pub fn validate_cache_mode(mode: &str) -> Option<CacheMode> {
    CacheMode::parse(mode)
}
