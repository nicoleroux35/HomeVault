/* =============================================================================
 * File          : transfer_quota_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
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
}

impl TransferQuota {
    /// The default, unconfigured state for a subject with no saved row yet:
    /// both directions disabled (unlimited/not tracked), zero usage.
    fn default_for(subject_type: &str, subject_name: &str) -> Self {
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
             download_enabled, download_limit_bytes, download_used_bytes, updated_at \
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
             download_enabled, download_limit_bytes, download_used_bytes, updated_at \
             FROM transfer_quotas WHERE subject_type = ?",
        )
        .bind(subject_type)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    /// Insert or replace a subject's quota configuration. Usage counters are
    /// left untouched — this only updates the admin-configured limits/toggles.
    pub async fn set_limits(
        &self,
        subject_type: &str,
        subject_name: &str,
        upload_enabled: bool,
        upload_limit_bytes: i64,
        download_enabled: bool,
        download_limit_bytes: i64,
    ) -> Result<TransferQuota, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO transfer_quotas \
             (id, subject_type, subject_name, upload_enabled, upload_limit_bytes, upload_used_bytes, \
              download_enabled, download_limit_bytes, download_used_bytes, updated_at) \
             VALUES (?, ?, ?, ?, ?, 0, ?, ?, 0, ?) \
             ON CONFLICT(subject_type, subject_name) DO UPDATE SET \
                upload_enabled = excluded.upload_enabled, \
                upload_limit_bytes = excluded.upload_limit_bytes, \
                download_enabled = excluded.download_enabled, \
                download_limit_bytes = excluded.download_limit_bytes, \
                updated_at = excluded.updated_at",
        )
        .bind(&id)
        .bind(subject_type)
        .bind(subject_name)
        .bind(upload_enabled as i64)
        .bind(upload_limit_bytes)
        .bind(download_enabled as i64)
        .bind(download_limit_bytes)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        self.get(subject_type, subject_name).await
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

    /// Add `bytes` to a subject's recorded upload usage. No-op if the
    /// subject has no saved quota row (nothing to add to).
    pub async fn add_upload_usage(
        &self,
        subject_type: &str,
        subject_name: &str,
        bytes: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE transfer_quotas SET upload_used_bytes = upload_used_bytes + ? \
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
    /// subject has no saved quota row.
    pub async fn add_download_usage(
        &self,
        subject_type: &str,
        subject_name: &str,
        bytes: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE transfer_quotas SET download_used_bytes = download_used_bytes + ? \
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
        }
    }
}
