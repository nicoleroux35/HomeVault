/* =============================================================================
 * File          : transfer_quota_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-user/per-group upload/download transfer quotas and their
 *   current usage counters.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for per-user/per-group upload and download transfer
//! quotas (`transfer_quotas` table). One row per (subject_type, subject_name)
//! pair, created on first save and updated in place afterward.
//!
//! The two counters belong to the row's `period` window (migration 055): the
//! reset logic itself lives in `src/transfer_quota.rs`, which owns the
//! calendar arithmetic — this layer only reads and writes what it is told.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// A subject's upload/download quota configuration and running usage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransferQuota {
    pub subject_type: String,
    pub subject_name: String,
    pub upload_enabled: bool,
    pub upload_limit_bytes: i64,
    pub upload_used_bytes: i64,
    pub download_enabled: bool,
    pub download_limit_bytes: i64,
    pub download_used_bytes: i64,
    pub updated_at: String,
    /// `lifetime` | `day` | `week` | `month` — see `transfer_quota::Period`.
    pub period: String,
    /// When the current counting window opened (RFC3339), `None` until the
    /// enforcement path or the reset loop opens it.
    pub period_started_at: Option<String>,
    /// Wire-only: when the counters next reset. Computed by the handler from
    /// `period` + `period_started_at`; never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_resets_at: Option<String>,
}

impl TransferQuota {
    /// The default, unconfigured state for a subject with no saved row yet:
    /// both directions disabled (unlimited/not tracked), zero usage, counted
    /// for ever.
    pub(crate) fn default_for(subject_type: &str, subject_name: &str) -> Self {
        Self {
            subject_type: subject_type.to_string(),
            subject_name: subject_name.to_string(),
            upload_enabled: false,
            upload_limit_bytes: 0,
            upload_used_bytes: 0,
            download_enabled: false,
            download_limit_bytes: 0,
            download_used_bytes: 0,
            updated_at: Utc::now().to_rfc3339(),
            period: "lifetime".to_string(),
            period_started_at: None,
            period_resets_at: None,
        }
    }
}

/// The admin-submitted limits of one row, as one value.
///
/// `period` is the validated wire name (`transfer_quota::Period::as_str`);
/// `period_started_at` is the instant the window being saved opens at, so the
/// storage layer never has to know about timezones or calendars.
pub struct QuotaLimits<'a> {
    pub subject_type: &'a str,
    pub subject_name: &'a str,
    pub upload_enabled: bool,
    pub upload_limit_bytes: i64,
    pub download_enabled: bool,
    pub download_limit_bytes: i64,
    pub period: &'a str,
    pub period_started_at: &'a str,
}

impl<'a> QuotaLimits<'a> {
    /// A `lifetime` quota — the default, and the shape a caller that does not
    /// care about counting periods (tests, internal tools) wants.
    pub fn lifetime(
        subject_type: &'a str,
        subject_name: &'a str,
        upload_enabled: bool,
        upload_limit_bytes: i64,
        download_enabled: bool,
        download_limit_bytes: i64,
    ) -> Self {
        Self {
            subject_type,
            subject_name,
            upload_enabled,
            upload_limit_bytes,
            download_enabled,
            download_limit_bytes,
            period: "lifetime",
            period_started_at: "",
        }
    }
}

pub struct TransferQuotaRepository {
    pool: Arc<SqlitePool>,
}

impl TransferQuotaRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Load a subject's quota, returning the disabled/unlimited default if
    /// no row has been saved for it yet.
    pub async fn get(
        &self,
        subject_type: &str,
        subject_name: &str,
    ) -> Result<TransferQuota, sqlx::Error> {
        let row = sqlx::query_as::<_, TransferQuotaRow>(
            "SELECT subject_type, subject_name, upload_enabled, upload_limit_bytes, upload_used_bytes, \
             download_enabled, download_limit_bytes, download_used_bytes, updated_at, \
             period, period_started_at \
             FROM transfer_quotas WHERE subject_type = ? AND subject_name = ?",
        )
            .bind(subject_type)
            .bind(subject_name)
            .fetch_optional(self.pool.as_ref())
            .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => TransferQuota::default_for(subject_type, subject_name),
        })
    }

    /// List every configured quota for a given subject type (`user` or
    /// `group`), e.g. for a Users/Groups page that wants to show every
    /// account's quota at once without one request per row.
    pub async fn list(&self, subject_type: &str) -> Result<Vec<TransferQuota>, sqlx::Error> {
        let rows = sqlx::query_as::<_, TransferQuotaRow>(
            "SELECT subject_type, subject_name, upload_enabled, upload_limit_bytes, upload_used_bytes, \
             download_enabled, download_limit_bytes, download_used_bytes, updated_at, \
             period, period_started_at \
             FROM transfer_quotas WHERE subject_type = ?",
        )
            .bind(subject_type)
            .fetch_all(self.pool.as_ref())
            .await?;

        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    /// Insert or replace a subject's quota configuration.
    ///
    /// Editing a limit leaves the usage counters alone — an admin raising a cap
    /// must not wipe what was already transferred. **Changing the period is a
    /// different thing**: the counters were counted against the old window, so
    /// they are zeroed and the new window opened, otherwise switching from
    /// "lifetime" to "per month" would instantly bill the whole history against
    /// a monthly cap.
    pub async fn set_limits(&self, limits: QuotaLimits<'_>) -> Result<TransferQuota, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO transfer_quotas \
             (id, subject_type, subject_name, upload_enabled, upload_limit_bytes, upload_used_bytes, \
              download_enabled, download_limit_bytes, download_used_bytes, updated_at, period, period_started_at) \
             VALUES (?, ?, ?, ?, ?, 0, ?, ?, 0, ?, ?, ?) \
             ON CONFLICT(subject_type, subject_name) DO UPDATE SET \
                upload_enabled = excluded.upload_enabled, \
                upload_limit_bytes = excluded.upload_limit_bytes, \
                download_enabled = excluded.download_enabled, \
                download_limit_bytes = excluded.download_limit_bytes, \
                updated_at = excluded.updated_at, \
                upload_used_bytes = CASE WHEN transfer_quotas.period IS NOT excluded.period \
                    THEN 0 ELSE transfer_quotas.upload_used_bytes END, \
                download_used_bytes = CASE WHEN transfer_quotas.period IS NOT excluded.period \
                    THEN 0 ELSE transfer_quotas.download_used_bytes END, \
                period_started_at = CASE WHEN transfer_quotas.period IS NOT excluded.period \
                    THEN excluded.period_started_at ELSE transfer_quotas.period_started_at END, \
                period = excluded.period",
        )
        .bind(&id)
        .bind(limits.subject_type)
        .bind(limits.subject_name)
        .bind(limits.upload_enabled as i64)
        .bind(limits.upload_limit_bytes)
        .bind(limits.download_enabled as i64)
        .bind(limits.download_limit_bytes)
        .bind(&now)
        .bind(limits.period)
        .bind(limits.period_started_at)
        .execute(self.pool.as_ref())
        .await?;

        self.get(limits.subject_type, limits.subject_name).await
    }

    /// Remove a subject's saved quota row entirely (back to the disabled
    /// default), e.g. when the user/group itself is deleted.
    pub async fn delete(&self, subject_type: &str, subject_name: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM transfer_quotas WHERE subject_type = ? AND subject_name = ?")
            .bind(subject_type)
            .bind(subject_name)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Zero both counters of one subject and re-open its window at
    /// `new_start`. Used by the enforcement path when it finds a stale window.
    pub async fn reset_window(
        &self,
        subject_type: &str,
        subject_name: &str,
        new_start: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE transfer_quotas SET upload_used_bytes = 0, download_used_bytes = 0, \
             period_started_at = ? WHERE subject_type = ? AND subject_name = ?",
        )
        .bind(new_start)
        .bind(subject_type)
        .bind(subject_name)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Zero the counters of every row of `period` whose window opened before
    /// `before`, and re-open it at `new_start`. Returns how many rows changed.
    ///
    /// A row whose window was never opened (`period_started_at IS NULL`) is
    /// opened too — its counters are zero anyway, and leaving it `NULL` would
    /// keep it out of every later comparison.
    pub async fn reset_expired(
        &self,
        period: &str,
        before: &str,
        new_start: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE transfer_quotas SET upload_used_bytes = 0, download_used_bytes = 0, \
             period_started_at = ? \
             WHERE period = ? AND (period_started_at IS NULL OR period_started_at < ?)",
        )
        .bind(new_start)
        .bind(period)
        .bind(before)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected())
    }

    /// Add `bytes` to a subject's recorded upload usage. No-op if the
    /// subject has no saved quota row (nothing to add to). The counter is
    /// floored at 0 so a refund landing after a window reset cannot make the
    /// new window's usage negative.
    pub async fn add_upload_usage(
        &self,
        subject_type: &str,
        subject_name: &str,
        bytes: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE transfer_quotas SET upload_used_bytes = MAX(0, upload_used_bytes + ?) \
             WHERE subject_type = ? AND subject_name = ?",
        )
        .bind(bytes)
        .bind(subject_type)
        .bind(subject_name)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Add `bytes` to a subject's recorded download usage. No-op if the
    /// subject has no saved quota row. Floored at 0, like `add_upload_usage`.
    pub async fn add_download_usage(
        &self,
        subject_type: &str,
        subject_name: &str,
        bytes: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE transfer_quotas SET download_used_bytes = MAX(0, download_used_bytes + ?) \
             WHERE subject_type = ? AND subject_name = ?",
        )
        .bind(bytes)
        .bind(subject_type)
        .bind(subject_name)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct TransferQuotaRow {
    subject_type: String,
    subject_name: String,
    upload_enabled: i64,
    upload_limit_bytes: i64,
    upload_used_bytes: i64,
    download_enabled: i64,
    download_limit_bytes: i64,
    download_used_bytes: i64,
    updated_at: String,
    period: String,
    period_started_at: Option<String>,
}

impl TransferQuotaRow {
    fn into_model(self) -> TransferQuota {
        TransferQuota {
            subject_type: self.subject_type,
            subject_name: self.subject_name,
            upload_enabled: self.upload_enabled != 0,
            upload_limit_bytes: self.upload_limit_bytes,
            upload_used_bytes: self.upload_used_bytes,
            download_enabled: self.download_enabled != 0,
            download_limit_bytes: self.download_limit_bytes,
            download_used_bytes: self.download_used_bytes,
            updated_at: self.updated_at,
            period: self.period,
            period_started_at: self.period_started_at,
            period_resets_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Database;

    async fn repo() -> TransferQuotaRepository {
        let db = Database::new("sqlite::memory:").await.expect("db");
        db.run_migrations().await.expect("migrations");
        TransferQuotaRepository::new(db.pool())
    }

    fn limits<'a>(period: &'a str, start: &'a str) -> QuotaLimits<'a> {
        QuotaLimits {
            subject_type: "user",
            subject_name: "alice",
            upload_enabled: true,
            upload_limit_bytes: 1_000,
            download_enabled: true,
            download_limit_bytes: 2_000,
            period,
            period_started_at: start,
        }
    }

    #[tokio::test]
    async fn a_subject_with_no_row_is_lifetime_and_disabled() {
        let repo = repo().await;
        let quota = repo.get("user", "nobody").await.unwrap();
        assert!(!quota.upload_enabled && !quota.download_enabled);
        assert_eq!(quota.period, "lifetime");
        assert_eq!(quota.period_started_at, None);
    }

    #[tokio::test]
    async fn the_period_and_its_window_start_round_trip() {
        let repo = repo().await;
        repo.set_limits(limits("month", "2026-10-01T00:00:00+00:00"))
            .await
            .unwrap();
        let quota = repo.get("user", "alice").await.unwrap();
        assert_eq!(quota.period, "month");
        assert_eq!(
            quota.period_started_at.as_deref(),
            Some("2026-10-01T00:00:00+00:00")
        );
    }

    #[tokio::test]
    async fn editing_a_limit_keeps_the_counters_of_the_same_window() {
        let repo = repo().await;
        repo.set_limits(limits("month", "2026-10-01T00:00:00+00:00"))
            .await
            .unwrap();
        repo.add_upload_usage("user", "alice", 400).await.unwrap();
        repo.add_download_usage("user", "alice", 700).await.unwrap();

        // Same period, a bigger cap: what was already transferred stays.
        let mut raised = limits("month", "2026-10-01T00:00:00+00:00");
        raised.upload_limit_bytes = 10_000;
        let quota = repo.set_limits(raised).await.unwrap();
        assert_eq!(quota.upload_used_bytes, 400);
        assert_eq!(quota.download_used_bytes, 700);
        assert_eq!(quota.upload_limit_bytes, 10_000);
    }

    #[tokio::test]
    async fn changing_the_period_starts_a_fresh_window() {
        let repo = repo().await;
        repo.set_limits(limits("lifetime", "2026-01-01T00:00:00+00:00"))
            .await
            .unwrap();
        repo.add_upload_usage("user", "alice", 900).await.unwrap();
        repo.add_download_usage("user", "alice", 1_900).await.unwrap();

        // Switching to a monthly window must not bill the whole lifetime
        // history against the new cap.
        let quota = repo
            .set_limits(limits("month", "2026-10-01T00:00:00+00:00"))
            .await
            .unwrap();
        assert_eq!(quota.period, "month");
        assert_eq!(quota.upload_used_bytes, 0);
        assert_eq!(quota.download_used_bytes, 0);
        assert_eq!(
            quota.period_started_at.as_deref(),
            Some("2026-10-01T00:00:00+00:00")
        );
    }

    #[tokio::test]
    async fn reset_window_zeroes_the_counters_and_moves_the_start() {
        let repo = repo().await;
        repo.set_limits(limits("day", "2026-10-22T00:00:00+00:00"))
            .await
            .unwrap();
        repo.add_upload_usage("user", "alice", 500).await.unwrap();

        repo.reset_window("user", "alice", "2026-10-23T00:00:00+00:00")
            .await
            .unwrap();
        let quota = repo.get("user", "alice").await.unwrap();
        assert_eq!(quota.upload_used_bytes, 0);
        assert_eq!(quota.download_used_bytes, 0);
        assert_eq!(
            quota.period_started_at.as_deref(),
            Some("2026-10-23T00:00:00+00:00")
        );
    }

    #[tokio::test]
    async fn reset_expired_only_touches_its_own_period_and_older_windows() {
        let repo = repo().await;
        repo.set_limits(limits("day", "2026-10-22T00:00:00+00:00"))
            .await
            .unwrap();
        repo.add_upload_usage("user", "alice", 500).await.unwrap();
        // A second subject on a monthly window, opened before the cutoff but
        // belonging to another period — it must not be reset by the day pass.
        let mut monthly = limits("month", "2026-09-01T00:00:00+00:00");
        monthly.subject_name = "bob";
        repo.set_limits(monthly).await.unwrap();
        repo.add_upload_usage("user", "bob", 300).await.unwrap();

        let changed = repo
            .reset_expired("day", "2026-10-23T00:00:00+00:00", "2026-10-23T00:00:00+00:00")
            .await
            .unwrap();
        assert_eq!(changed, 1, "only the day row was in that period");
        assert_eq!(repo.get("user", "alice").await.unwrap().upload_used_bytes, 0);
        assert_eq!(repo.get("user", "bob").await.unwrap().upload_used_bytes, 300);
    }

    #[tokio::test]
    async fn reset_expired_is_a_no_op_for_a_window_already_open() {
        let repo = repo().await;
        repo.set_limits(limits("day", "2026-10-23T00:00:00+00:00"))
            .await
            .unwrap();
        repo.add_upload_usage("user", "alice", 500).await.unwrap();

        let changed = repo
            .reset_expired("day", "2026-10-23T00:00:00+00:00", "2026-10-23T00:00:00+00:00")
            .await
            .unwrap();
        assert_eq!(changed, 0);
        assert_eq!(
            repo.get("user", "alice").await.unwrap().upload_used_bytes,
            500
        );
    }

    #[tokio::test]
    async fn usage_is_floored_at_zero() {
        let repo = repo().await;
        repo.set_limits(limits("lifetime", "2026-01-01T00:00:00+00:00"))
            .await
            .unwrap();
        repo.add_upload_usage("user", "alice", 100).await.unwrap();
        // A refund of a reservation that was itself reset must not go negative.
        repo.add_upload_usage("user", "alice", -500).await.unwrap();
        assert_eq!(repo.get("user", "alice").await.unwrap().upload_used_bytes, 0);
    }

    #[tokio::test]
    async fn usage_on_a_subject_without_a_row_is_a_no_op() {
        let repo = repo().await;
        repo.add_upload_usage("user", "ghost", 100).await.unwrap();
        assert_eq!(
            repo.get("user", "ghost").await.unwrap().upload_used_bytes,
            0
        );
    }
}
