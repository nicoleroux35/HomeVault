/* =============================================================================
 * File          : backup_tenant_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for the backup-destination tenants: CRUD, the quota watchdog's
 *   over-quota flag and the verification bookkeeping.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::backup_target
 *
 * Usage:
 *   handlers/backup_target.rs, backup_target_scheduler.rs.
 * =============================================================================
 */

//! Backup-destination tenant storage (`backup_tenants`, migration 070).

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::backup_target::BackupTenant;
use crate::impl_repository;
use crate::storage::database::DbError;

#[derive(Clone)]
pub struct BackupTenantRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(BackupTenantRepository);

impl BackupTenantRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<BackupTenant>, DbError> {
        Ok(sqlx::query_as::<_, BackupTenant>(
            "SELECT * FROM backup_tenants ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    pub async fn find(&self, id: i64) -> Result<Option<BackupTenant>, DbError> {
        Ok(
            sqlx::query_as::<_, BackupTenant>("SELECT * FROM backup_tenants WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool.as_ref())
                .await?,
        )
    }

    pub async fn name_taken(&self, name: &str, except: Option<i64>) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM backup_tenants WHERE name = ? COLLATE NOCASE AND id IS NOT ?",
        )
        .bind(name)
        .bind(except)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    /// Create one tenant. The caller hashes the token and seals the repository
    /// password; nothing secret is ever passed to the database in clear text.
    pub async fn create(
        &self,
        name: &str,
        auth_hash: &str,
        token_hash: &str,
        repo_password_sealed: &str,
        repo_path: &str,
        quota_bytes: i64,
        created_by: &str,
    ) -> Result<BackupTenant, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO backup_tenants \
             (name, auth_hash, token_hash, repo_password_sealed, repo_path, quota_bytes, \
              verify_enabled, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
        )
        .bind(name)
        .bind(auth_hash)
        .bind(token_hash)
        .bind(repo_password_sealed)
        .bind(repo_path)
        .bind(quota_bytes)
        .bind(created_by)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.find(result.last_insert_rowid() as i64)
            .await?
            .ok_or(DbError::NotFound)
    }

    /// Rotate a tenant's credentials (a new token and a new repository
    /// password). Existing backups stay readable with the new password only
    /// because restic re-encrypts nothing — rotating the repository password
    /// therefore also means the source must re-init, which the API documents.
    pub async fn set_credentials(
        &self,
        id: i64,
        auth_hash: &str,
        token_hash: &str,
        repo_password_sealed: &str,
    ) -> Result<BackupTenant, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE backup_tenants SET auth_hash = ?, token_hash = ?, repo_password_sealed = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(auth_hash)
        .bind(token_hash)
        .bind(repo_password_sealed)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.find(id).await?.ok_or(DbError::NotFound)
    }

    pub async fn set_quota_and_verify(
        &self,
        id: i64,
        quota_bytes: i64,
        verify_enabled: bool,
    ) -> Result<BackupTenant, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE backup_tenants SET quota_bytes = ?, verify_enabled = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(quota_bytes)
        .bind(i64::from(verify_enabled))
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.find(id).await?.ok_or(DbError::NotFound)
    }

    pub async fn set_over_quota(&self, id: i64, over_quota: bool) -> Result<(), DbError> {
        sqlx::query("UPDATE backup_tenants SET over_quota = ?, updated_at = ? WHERE id = ?")
            .bind(i64::from(over_quota))
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn mark_seen(&self, id: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE backup_tenants SET last_seen_at = ? WHERE id = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn record_verification(
        &self,
        id: i64,
        success: bool,
        message: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE backup_tenants SET last_verified_at = ?, last_verify_status = ?, \
             last_verify_error = ? WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(if success { "success" } else { "failed" })
        .bind(if success { None } else { Some(message) })
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: i64) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM backup_tenants WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
