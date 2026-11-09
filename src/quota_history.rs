/* =============================================================================
 * File          : quota_history.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026
 * Last modified : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Hourly sampler for transfer-quota usage (P1). It stores one point per
 *   subject, prunes after 90 days, and raises a notification when a subject
 *   crosses 90% of a limit within one counting period. The crossing rule is
 *   pure and unit-tested; the sampler is best-effort and never blocks a
 *   transfer.
 *
 * Dependencies:
 *   - sqlx, chrono, uuid, tokio
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; history is read from
 *   GET /api/transfer-quotas/history.
 * =============================================================================
 */

//! Transfer-quota usage history and threshold alerts.

use crate::handlers::AppState;
use crate::storage::NotificationRepository;
use crate::storage::transfer_quota_repo::{TransferQuota, TransferQuotaRepository};
use chrono::Utc;
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// How long a sample is kept.
pub const RETENTION_DAYS: u32 = 90;
/// Alert when a limit is at or above this percentage.
pub const ALERT_THRESHOLD_PERCENT: f64 = 90.0;
/// One sample per hour: enough for a trend without growing the table.
const POLL_INTERVAL_SECS: u64 = 3600;
/// Hard cap for one history request.
pub const HISTORY_MAX_POINTS: u32 = 500;

/// One point returned to the Quotas screen.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QuotaHistoryPoint {
    pub observed_at: String,
    pub period: String,
    pub upload_used_bytes: i64,
    pub upload_limit_bytes: i64,
    pub download_used_bytes: i64,
    pub download_limit_bytes: i64,
}

/// The crossing rule: alert only when the current sample is at/above the
/// threshold and the previous one was below it (or there is no previous
/// sample). Staying above the threshold never re-alerts; dropping below and
/// crossing again does.
pub fn should_alert(previous_percent: Option<f64>, current_percent: f64, threshold: f64) -> bool {
    current_percent >= threshold && previous_percent.map(|p| p < threshold).unwrap_or(true)
}

/// Percentage of a limit, or `None` when the limit is not configured.
pub fn percent(used: i64, limit: i64) -> Option<f64> {
    if limit <= 0 {
        None
    } else {
        Some((used.max(0) as f64 / limit as f64) * 100.0)
    }
}

/// One sampling tick: read every quota, store a point, notify on a crossing
/// and prune the retention window. Returns how many points were written.
pub async fn sample_once(pool: &SqlitePool) -> Result<u64, String> {
    let repo = TransferQuotaRepository::new(Arc::new(pool.clone()));
    let mut written = 0u64;
    for subject_type in ["user", "group"] {
        let quotas = repo.list(subject_type).await.map_err(|e| e.to_string())?;
        for quota in quotas {
            if let Err(e) = sample_quota(pool, &quota).await {
                tracing::warn!(
                    "Quota history sample failed for {} '{}': {}",
                    quota.subject_type,
                    quota.subject_name,
                    e
                );
            } else {
                written += 1;
            }
        }
    }
    prune(pool).await?;
    Ok(written)
}

async fn sample_quota(pool: &SqlitePool, quota: &TransferQuota) -> Result<(), String> {
    let previous = last_point(pool, quota).await?;
    let observed_at = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO transfer_quota_history \
         (id, observed_at, subject_type, subject_name, period, \
          upload_used_bytes, upload_limit_bytes, download_used_bytes, download_limit_bytes) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&observed_at)
    .bind(&quota.subject_type)
    .bind(&quota.subject_name)
    .bind(&quota.period)
    .bind(quota.upload_used_bytes)
    .bind(quota.upload_limit_bytes)
    .bind(quota.download_used_bytes)
    .bind(quota.download_limit_bytes)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    // A period change or a counter reset is a new baseline: comparing against
    // the previous window would either miss the crossing or fire spuriously.
    let comparable = previous.as_ref().filter(|p| p.period == quota.period);
    let previous_upload = comparable
        .and_then(|p| percent(p.upload_used_bytes, p.upload_limit_bytes))
        .filter(|_| previous_is_lower(comparable, quota, true));
    let previous_download = comparable
        .and_then(|p| percent(p.download_used_bytes, p.download_limit_bytes))
        .filter(|_| previous_is_lower(comparable, quota, false));

    if quota.upload_enabled {
        if let Some(current) = percent(quota.upload_used_bytes, quota.upload_limit_bytes) {
            if should_alert(previous_upload, current, ALERT_THRESHOLD_PERCENT) {
                notify(pool, quota, "upload", current, quota.upload_limit_bytes).await;
            }
        }
    }
    if quota.download_enabled {
        if let Some(current) = percent(quota.download_used_bytes, quota.download_limit_bytes) {
            if should_alert(previous_download, current, ALERT_THRESHOLD_PERCENT) {
                notify(pool, quota, "download", current, quota.download_limit_bytes).await;
            }
        }
    }
    Ok(())
}

/// Only compare when the counters did not go backwards (an admin edit or a
/// manual reset): a decrease is a fresh baseline, not a crossing.
fn previous_is_lower(
    previous: Option<&QuotaHistoryPoint>,
    quota: &TransferQuota,
    upload: bool,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if upload {
        previous.upload_used_bytes <= quota.upload_used_bytes
    } else {
        previous.download_used_bytes <= quota.download_used_bytes
    }
}

async fn last_point(
    pool: &SqlitePool,
    quota: &TransferQuota,
) -> Result<Option<QuotaHistoryPoint>, String> {
    let row = sqlx::query(
        "SELECT observed_at, period, upload_used_bytes, upload_limit_bytes, \
                download_used_bytes, download_limit_bytes \
         FROM transfer_quota_history \
         WHERE subject_type = ? AND subject_name = ? \
         ORDER BY observed_at DESC LIMIT 1",
    )
    .bind(&quota.subject_type)
    .bind(&quota.subject_name)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(row.map(|row| QuotaHistoryPoint {
        observed_at: row.get("observed_at"),
        period: row.get("period"),
        upload_used_bytes: row.get("upload_used_bytes"),
        upload_limit_bytes: row.get("upload_limit_bytes"),
        download_used_bytes: row.get("download_used_bytes"),
        download_limit_bytes: row.get("download_limit_bytes"),
    }))
}

async fn notify(
    pool: &SqlitePool,
    quota: &TransferQuota,
    direction: &str,
    current: f64,
    limit: i64,
) {
    let repo = NotificationRepository::new(Arc::new(pool.clone()));
    let message = format!(
        "{} '{}' {} usage is at {:.0}% of its limit ({})",
        quota.subject_type,
        quota.subject_name,
        direction,
        current,
        crate::share_quota::human_size(limit as u64)
    );
    if let Err(e) = repo
        .create(
            "Transfer quota nearly full",
            &message,
            "warning",
            "system",
            "quota",
        )
        .await
    {
        tracing::warn!("Quota alert could not be stored: {e}");
    }
}

/// Deletes samples older than the retention window.
pub async fn prune(pool: &SqlitePool) -> Result<u64, String> {
    let cutoff = Utc::now() - chrono::Duration::days(RETENTION_DAYS as i64);
    let result = sqlx::query("DELETE FROM transfer_quota_history WHERE observed_at < ?")
        .bind(cutoff.to_rfc3339())
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(result.rows_affected())
}

/// Newest-first history for one subject.
pub async fn list(
    pool: &SqlitePool,
    subject_type: &str,
    subject_name: &str,
    limit: u32,
) -> Result<Vec<QuotaHistoryPoint>, String> {
    let limit = limit.clamp(1, HISTORY_MAX_POINTS);
    let rows = sqlx::query(
        "SELECT observed_at, period, upload_used_bytes, upload_limit_bytes, \
                download_used_bytes, download_limit_bytes \
         FROM transfer_quota_history \
         WHERE subject_type = ? AND subject_name = ? \
         ORDER BY observed_at DESC LIMIT ?",
    )
    .bind(subject_type)
    .bind(subject_name)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| QuotaHistoryPoint {
            observed_at: row.get("observed_at"),
            period: row.get("period"),
            upload_used_bytes: row.get("upload_used_bytes"),
            upload_limit_bytes: row.get("upload_limit_bytes"),
            download_used_bytes: row.get("download_used_bytes"),
            download_limit_bytes: row.get("download_limit_bytes"),
        })
        .collect())
}

/// Runs forever, sampling once an hour.
pub async fn run_sampler_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        match sample_once(state.db.pool().as_ref()).await {
            Ok(count) => tracing::debug!("Quota history: {count} sample(s) written"),
            Err(e) => tracing::warn!("Quota history sampler failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_alert_only_on_a_crossing() {
        assert!(should_alert(None, 95.0, 90.0));
        assert!(should_alert(Some(80.0), 90.0, 90.0));
        assert!(!should_alert(Some(91.0), 95.0, 90.0));
        assert!(!should_alert(Some(80.0), 89.9, 90.0));
        assert!(!should_alert(None, 0.0, 90.0));
    }

    #[test]
    fn test_percent_requires_a_positive_limit() {
        assert_eq!(percent(50, 100), Some(50.0));
        assert_eq!(percent(0, 0), None);
        assert_eq!(percent(5, -1), None);
        assert_eq!(percent(-5, 100), Some(0.0));
    }
}
