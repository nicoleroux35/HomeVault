/* =============================================================================
 * File          : group_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   User-group records backing the group memberships shown in the
 *   Users/Groups apps.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! User-group records (memberships, homevault-prefixed groups).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::storage::database::DbError;

/// Database row from `hv_groups`.
#[derive(Debug, Clone, FromRow)]
pub struct HvGroupRow {
    pub id: i64,
    /// Short group name (used in URLs).
    pub name: String,
    /// Full system group name (e.g. `homevault_<name>`).
    pub full_name: String,
    /// Optional POSIX GID when synced to the OS.
    pub gid: Option<i64>,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    /// System groups (e.g. `homevault_admin`) cannot be deleted.
    pub is_system: bool,
}

/// A user group with optional OS-level GID mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HvGroup {
    pub id: i64,
    pub name: String,
    pub full_name: String,
    pub gid: Option<i64>,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub is_system: bool,
}

impl From<HvGroupRow> for HvGroup {
    fn from(row: HvGroupRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            full_name: row.full_name,
            gid: row.gid,
            description: row.description,
            created_at: row.created_at,
            is_system: row.is_system,
        }
    }
}

/// Repository for HomeVault user groups.
#[derive(Clone)]
pub struct HvGroupRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(HvGroupRepository);

impl HvGroupRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up a group by its short name.
    pub async fn find_by_name(&self, name: &str) -> Result<Option<HvGroup>, DbError> {
        let group = sqlx::query_as::<_, HvGroupRow>(
            "SELECT id, name, full_name, gid, description, created_at, is_system FROM hv_groups WHERE name = ?"
        )
        .bind(name)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(group.map(HvGroup::from))
    }

    /// Look up a group by its internal ID.
    pub async fn find_by_id(&self, id: i64) -> Result<Option<HvGroup>, DbError> {
        let group = sqlx::query_as::<_, HvGroupRow>(
            "SELECT id, name, full_name, gid, description, created_at, is_system FROM hv_groups WHERE id = ?"
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(group.map(HvGroup::from))
    }

    /// Look up a group by its POSIX GID.
    pub async fn find_by_gid(&self, gid: i64) -> Result<Option<HvGroup>, DbError> {
        let group = sqlx::query_as::<_, HvGroupRow>(
            "SELECT id, name, full_name, gid, description, created_at, is_system FROM hv_groups WHERE gid = ?"
        )
        .bind(gid)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(group.map(HvGroup::from))
    }

    /// List all groups (alphabetical).
    pub async fn list(&self) -> Result<Vec<HvGroup>, DbError> {
        let groups = sqlx::query_as::<_, HvGroupRow>(
            "SELECT id, name, full_name, gid, description, created_at, is_system FROM hv_groups ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(groups.into_iter().map(HvGroup::from).collect())
    }

    /// Create a new group (prefixes name with `homevault_`).
    pub async fn create(&self, name: &str, description: Option<&str>) -> Result<HvGroup, DbError> {
        let now = Utc::now().to_rfc3339();
        let full_name = format!("homevault_{}", name);

        sqlx::query(
            "INSERT INTO hv_groups (name, full_name, description, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(name)
        .bind(&full_name)
        .bind(description)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        self.find_by_name(name).await?.ok_or(DbError::NotFound)
    }

    /// Move a mirror row to a new display name (group rename). The `full_name`
    /// follows the `homevault_<name>` convention; `gid` is preserved because
    /// `groupmod -n` keeps it.
    pub async fn rename(&self, old_name: &str, new_name: &str) -> Result<bool, DbError> {
        let full_name = format!("homevault_{}", new_name);
        let result = sqlx::query(
            "UPDATE hv_groups SET name = ?, full_name = ? WHERE name = ? AND is_system = 0",
        )
        .bind(new_name)
        .bind(&full_name)
        .bind(old_name)
        .execute(self.pool.as_ref())
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Set the POSIX GID for a group (after OS-level group creation).
    pub async fn update_gid(&self, name: &str, gid: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE hv_groups SET gid = ? WHERE name = ?")
            .bind(gid)
            .bind(name)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Delete a non-system group by name. Returns `true` if deleted.
    pub async fn delete(&self, name: &str) -> Result<bool, DbError> {
        let result = sqlx::query("DELETE FROM hv_groups WHERE name = ? AND is_system = 0")
            .bind(name)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// List only non-system (user-created) groups.
    pub async fn list_non_system(&self) -> Result<Vec<HvGroup>, DbError> {
        let groups = sqlx::query_as::<_, HvGroupRow>(
            "SELECT id, name, full_name, gid, description, created_at, is_system
             FROM hv_groups
             WHERE is_system = 0
             ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(groups.into_iter().map(HvGroup::from).collect())
    }

    /// Total number of groups.
    pub async fn count(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM hv_groups")
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// Check whether a group name is already taken.
    pub async fn exists(&self, name: &str) -> Result<bool, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM hv_groups WHERE name = ?")
            .bind(name)
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0 > 0)
    }

    /// Update a group's description. Returns `false` when no mirror row
    /// matches — a group created straight on the host has none yet, and the
    /// caller then creates one instead of silently doing nothing.
    pub async fn update_description(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<bool, DbError> {
        let result = sqlx::query("UPDATE hv_groups SET description = ? WHERE name = ?")
            .bind(description)
            .bind(name)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected() > 0)
    }
}
