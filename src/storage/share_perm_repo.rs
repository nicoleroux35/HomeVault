use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::storage::database::DbError;
use crate::impl_repository;

/// Database row from `share_permissions`.
#[derive(Debug, Clone, FromRow)]
pub struct SharePermissionRow {
    pub id: i64,
    pub share_name: String,
    /// `"samba"`, `"ftp"`, or `"webdav"`.
    pub share_type: String,
    /// `"user"` or `"group"`.
    pub grantee_type: String,
    pub grantee_name: String,
    /// Access level (e.g. `"read"`, `"write"`, `"full"`).
    pub permission: String,
    /// Whether the permission applies to sub-paths.
    pub recursive: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A permission granting access to a share for a user or group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharePermission {
    pub id: i64,
    pub share_name: String,
    pub share_type: String,
    pub grantee_type: String,
    pub grantee_name: String,
    pub permission: String,
    pub recursive: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<SharePermissionRow> for SharePermission {
    fn from(row: SharePermissionRow) -> Self {
        Self {
            id: row.id,
            share_name: row.share_name,
            share_type: row.share_type,
            grantee_type: row.grantee_type,
            grantee_name: row.grantee_name,
            permission: row.permission,
            recursive: row.recursive,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Repository for share permission entries.
#[derive(Clone)]
pub struct SharePermissionRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SharePermissionRepository);

impl SharePermissionRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up a permission by its ID.
    pub async fn find_by_id(&self, id: i64) -> Result<Option<SharePermission>, DbError> {
        let perm = sqlx::query_as::<_, SharePermissionRow>(
            "SELECT id, share_name, share_type, grantee_type, grantee_name, permission, recursive, created_at, updated_at FROM share_permissions WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(perm.map(SharePermission::from))
    }

    /// List all permissions for a specific share.
    pub async fn get_for_share(&self, share_name: &str, share_type: &str) -> Result<Vec<SharePermission>, DbError> {
        let perms = sqlx::query_as::<_, SharePermissionRow>(
            "SELECT id, share_name, share_type, grantee_type, grantee_name, permission, recursive, created_at, updated_at FROM share_permissions WHERE share_name = ? AND share_type = ?",
        )
        .bind(share_name)
        .bind(share_type)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(perms.into_iter().map(SharePermission::from).collect())
    }

    /// List all permissions assigned to a specific user or group.
    pub async fn get_for_grantee(&self, grantee_type: &str, grantee_name: &str) -> Result<Vec<SharePermission>, DbError> {
        let perms = sqlx::query_as::<_, SharePermissionRow>(
            "SELECT id, share_name, share_type, grantee_type, grantee_name, permission, recursive, created_at, updated_at FROM share_permissions WHERE grantee_type = ? AND grantee_name = ?",
        )
        .bind(grantee_type)
        .bind(grantee_name)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(perms.into_iter().map(SharePermission::from).collect())
    }

    /// List all permissions (across all shares).
    pub async fn list(&self) -> Result<Vec<SharePermission>, DbError> {
        let perms = sqlx::query_as::<_, SharePermissionRow>(
            "SELECT id, share_name, share_type, grantee_type, grantee_name, permission, recursive, created_at, updated_at FROM share_permissions ORDER BY share_name, share_type",
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(perms.into_iter().map(SharePermission::from).collect())
    }

    /// Grant or update a permission for a share.
    pub async fn upsert(
        &self,
        share_name: &str,
        share_type: &str,
        grantee_type: &str,
        grantee_name: &str,
        permission: &str,
        recursive: bool,
    ) -> Result<SharePermission, DbError> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"
            INSERT INTO share_permissions (share_name, share_type, grantee_type, grantee_name, permission, recursive, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(share_name, share_type, grantee_type, grantee_name) DO UPDATE SET
                permission = excluded.permission,
                recursive = excluded.recursive,
                updated_at = excluded.updated_at
            "#
        )
        .bind(share_name)
        .bind(share_type)
        .bind(grantee_type)
        .bind(grantee_name)
        .bind(permission)
        .bind(recursive)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        let perms = self.get_for_share(share_name, share_type).await?;
        perms.into_iter()
            .find(|p| p.grantee_type == grantee_type && p.grantee_name == grantee_name)
            .ok_or(DbError::NotFound)
    }

    /// Revoke a specific permission entry.
    pub async fn delete(&self, share_name: &str, share_type: &str, grantee_type: &str, grantee_name: &str) -> Result<bool, DbError> {
        let result = sqlx::query(
            "DELETE FROM share_permissions WHERE share_name = ? AND share_type = ? AND grantee_type = ? AND grantee_name = ?"
        )
        .bind(share_name)
        .bind(share_type)
        .bind(grantee_type)
        .bind(grantee_name)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Remove all permissions for a share.
    pub async fn delete_for_share(&self, share_name: &str, share_type: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM share_permissions WHERE share_name = ? AND share_type = ?")
            .bind(share_name)
            .bind(share_type)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected())
    }

    /// Remove all permissions assigned to a particular user or group.
    pub async fn delete_for_grantee(&self, grantee_type: &str, grantee_name: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM share_permissions WHERE grantee_type = ? AND grantee_name = ?")
            .bind(grantee_type)
            .bind(grantee_name)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected())
    }

    /// Total number of permission entries.
    pub async fn count(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM share_permissions")
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// Check whether a specific permission string exists for the given grantee.
    pub async fn has_permission(
        &self,
        share_name: &str,
        share_type: &str,
        grantee_type: &str,
        grantee_name: &str,
        required_permission: &str,
    ) -> Result<bool, DbError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM share_permissions
             WHERE share_name = ? AND share_type = ?
             AND grantee_type = ? AND grantee_name = ?
             AND permission = ?"
        )
        .bind(share_name)
        .bind(share_type)
        .bind(grantee_type)
        .bind(grantee_name)
        .bind(required_permission)
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(row.0 > 0)
    }
}
