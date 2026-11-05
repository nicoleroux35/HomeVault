/* =============================================================================
 * File          : disk_health_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage layer for the disk-health history: one row per watched SMART
 *   attribute per sample, plus deduplicated findings (serial + condition).
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   `src/disk_health.rs` (sampling/analysis) and `src/handlers/health.rs`
 *   (HTTP reads).
 * =============================================================================
 */

//! Disk-health samples and findings.

use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DiskHealthSample {
    pub id: i64,
    pub serial: String,
    pub model: String,
    pub attr_id: i64,
    pub raw: i64,
    pub normalized: Option<i64>,
    pub sampled_at: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DiskHealthFinding {
    pub id: i64,
    pub fingerprint: String,
    pub serial: String,
    pub model: String,
    pub kind: String,
    pub severity: String,
    pub message: String,
    pub first_seen: String,
    pub last_seen: String,
    pub resolved_at: Option<String>,
}

pub struct DiskHealthRepository {
    pool: Arc<SqlitePool>,
}

impl DiskHealthRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// The most recent raw value for one attribute, before the new sample is
    /// inserted. `None` on the first sample for that disk/attribute.
    pub async fn latest_raw(&self, serial: &str, attr_id: i64) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT raw FROM disk_health_samples \
             WHERE serial = ? AND attr_id = ? ORDER BY sampled_at DESC, id DESC LIMIT 1",
        )
        .bind(serial)
        .bind(attr_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|(raw,)| raw))
    }

    /// The most recent raw values for one attribute, newest first. Used for
    /// thresholds that require a streak (temperature) rather than one sample.
    pub async fn recent_raws(
        &self,
        serial: &str,
        attr_id: i64,
        limit: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            "SELECT raw FROM disk_health_samples \
             WHERE serial = ? AND attr_id = ? ORDER BY sampled_at DESC, id DESC LIMIT ?",
        )
        .bind(serial)
        .bind(attr_id)
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|(raw,)| raw).collect())
    }

    /// Distinct disk serials that have at least one stored sample. Used by
    /// the prediction overview; an empty list is a valid "nothing sampled yet".
    pub async fn list_serials(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT serial FROM disk_health_samples \
             WHERE serial <> '' ORDER BY serial",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|(serial,)| serial).collect())
    }

    pub async fn insert_sample(
        &self,
        serial: &str,
        model: &str,
        attr_id: i64,
        raw: i64,
        normalized: Option<i64>,
        sampled_at: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO disk_health_samples (serial, model, attr_id, raw, normalized, sampled_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(serial)
        .bind(model)
        .bind(attr_id)
        .bind(raw)
        .bind(normalized)
        .bind(sampled_at)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Insert or refresh a finding. The fingerprint makes a persistent
    /// condition one row with a moving `last_seen`, not one row per sample.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_finding(
        &self,
        fingerprint: &str,
        serial: &str,
        model: &str,
        kind: &str,
        severity: &str,
        message: &str,
        now: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO disk_health_findings \
             (fingerprint, serial, model, kind, severity, message, first_seen, last_seen) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(fingerprint) DO UPDATE SET \
               last_seen = excluded.last_seen, \
               severity = excluded.severity, \
               message = excluded.message, \
               resolved_at = NULL",
        )
        .bind(fingerprint)
        .bind(serial)
        .bind(model)
        .bind(kind)
        .bind(severity)
        .bind(message)
        .bind(now)
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Whether a finding is already active. Used to notify only when a
    /// condition first appears, not on every daily sample.
    pub async fn is_active(&self, fingerprint: &str) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM disk_health_findings \
             WHERE fingerprint = ? AND resolved_at IS NULL LIMIT 1",
        )
        .bind(fingerprint)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    pub async fn list_active_findings(&self) -> Result<Vec<DiskHealthFinding>, sqlx::Error> {
        sqlx::query_as::<_, DiskHealthFinding>(
            "SELECT id, fingerprint, serial, model, kind, severity, message, first_seen, \
             last_seen, resolved_at FROM disk_health_findings \
             WHERE resolved_at IS NULL \
             ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'error' THEN 1 \
                      WHEN 'warning' THEN 2 ELSE 3 END, last_seen DESC",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn list_samples(
        &self,
        serial: &str,
        limit: u32,
    ) -> Result<Vec<DiskHealthSample>, sqlx::Error> {
        sqlx::query_as::<_, DiskHealthSample>(
            "SELECT id, serial, model, attr_id, raw, normalized, sampled_at \
             FROM disk_health_samples WHERE serial = ? \
             ORDER BY sampled_at DESC, id DESC LIMIT ?",
        )
        .bind(serial)
        .bind(limit.min(5_000) as i64)
        .fetch_all(self.pool.as_ref())
        .await
    }

    /// The newest sample across every disk.
    pub async fn last_sample_at_any(&self) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT sampled_at FROM disk_health_samples ORDER BY sampled_at DESC, id DESC LIMIT 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|(at,)| at))
    }

    pub async fn last_sample_at(&self, serial: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT sampled_at FROM disk_health_samples WHERE serial = ? \
             ORDER BY sampled_at DESC, id DESC LIMIT 1",
        )
        .bind(serial)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|(at,)| at))
    }
}
