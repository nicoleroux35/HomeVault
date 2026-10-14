/* =============================================================================
 * File          : user_home_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Home-folder assignments: enabled flag, hosting disk or explicit
 *   path, and the hard size quota.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Home-folder assignments (disk/path + hard quota).

use crate::impl_repository;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;

/// A user's home directory configuration (disk and path).
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UserHome {
    pub username: String,
    /// Whether the user's home directory is active.
    pub enabled: bool,
    /// The disk/volume where the home resides.
    pub disk: Option<String>,
    /// Absolute path to the home directory.
    pub path: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Hard disk-quota limit for the home directory, in bytes.
    /// `None` or `0` means no limit is enforced.
    #[sqlx(default)]
    pub size_limit_bytes: Option<i64>,
}

/// Repository for user home directory settings.
#[derive(Clone)]
pub struct UserHomeRepository {
    pool: Arc<sqlx::SqlitePool>,
}

impl UserHomeRepository {
    pub fn new(pool: Arc<sqlx::SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up a user's home configuration.
    pub async fn find_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserHome>, crate::storage::DbError> {
        let home = sqlx::query_as::<_, UserHome>(
            "SELECT username, enabled, disk, path, created_at, size_limit_bytes FROM user_homes WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(home)
    }

    /// Insert or update a user's home configuration.
    pub async fn upsert(&self, home: &UserHome) -> Result<(), crate::storage::DbError> {
        sqlx::query(
            "INSERT INTO user_homes (username, enabled, disk, path, created_at, size_limit_bytes)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(username) DO UPDATE SET
               enabled = excluded.enabled,
               disk = excluded.disk,
               path = excluded.path,
               size_limit_bytes = excluded.size_limit_bytes",
        )
        .bind(&home.username)
        .bind(home.enabled)
        .bind(&home.disk)
        .bind(&home.path)
        .bind(home.created_at)
        .bind(home.size_limit_bytes)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Remove a user's home configuration.
    pub async fn delete(&self, username: &str) -> Result<(), crate::storage::DbError> {
        sqlx::query("DELETE FROM user_homes WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    pub fn pool(&self) -> Arc<sqlx::SqlitePool> {
        Arc::clone(&self.pool)
    }
}

impl_repository!(UserHomeRepository);
