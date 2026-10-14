/* =============================================================================
 * File          : app_store_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Community app-store registry: the extra git sources the admin adds
 *   to the Apps Center, with sync state.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for community app stores (the extra git-repo sources the
//! admin adds to the Docker one-click catalog). The built-in "homevault"
//! store is embedded in the binary and never has a row here - this table
//! only holds the user-added ones. See `sqlx/migrations/027_app_stores.sql`
//! and `crate::app_catalog::store`.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

/// One community app store: a git repository cloned into
/// `apps_dir()/stores/<id>/`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppStore {
    pub id: String,
    pub name: String,
    pub git_url: String,
    pub enabled: bool,
    /// Unix seconds of the last successful sync, `None` if never synced.
    pub last_synced_at: Option<i64>,
    /// Last sync failure message, `None` when the last sync succeeded.
    pub last_error: Option<String>,
    pub created_at: String,
    pub created_by: String,
}

pub struct AppStoreRepository {
    pool: Arc<SqlitePool>,
}

impl AppStoreRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<AppStore>, sqlx::Error> {
        let rows = sqlx::query_as::<_, AppStoreRow>(
            "SELECT id, name, git_url, enabled, last_synced_at, last_error, created_at, created_by
             FROM app_stores ORDER BY created_at ASC",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(AppStoreRow::into_model).collect())
    }

    pub async fn get(&self, id: &str) -> Result<Option<AppStore>, sqlx::Error> {
        let row = sqlx::query_as::<_, AppStoreRow>(
            "SELECT id, name, git_url, enabled, last_synced_at, last_error, created_at, created_by
             FROM app_stores WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(AppStoreRow::into_model))
    }

    /// Insert a new store. `id` is caller-derived (a slug of the repo name);
    /// a clash on `id` or `git_url` surfaces as a `sqlx::Error` the handler
    /// turns into a user-facing "already added" message.
    pub async fn add(
        &self,
        id: &str,
        name: &str,
        git_url: &str,
        created_by: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO app_stores (id, name, git_url, enabled, last_synced_at, last_error, created_at, created_by)
             VALUES (?, ?, ?, 1, NULL, NULL, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(git_url)
        .bind(&now)
        .bind(created_by)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM app_stores WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE app_stores SET enabled = ? WHERE id = ?")
            .bind(enabled as i64)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Record the outcome of a sync attempt: on success pass `error = None`
    /// and the current unix time; on failure pass the message and leave
    /// `last_synced_at` untouched by passing `synced_at = None`.
    pub async fn record_sync(
        &self,
        id: &str,
        synced_at: Option<i64>,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        match synced_at {
            Some(ts) => {
                sqlx::query(
                    "UPDATE app_stores SET last_synced_at = ?, last_error = NULL WHERE id = ?",
                )
                .bind(ts)
                .bind(id)
                .execute(self.pool.as_ref())
                .await?;
            }
            None => {
                sqlx::query("UPDATE app_stores SET last_error = ? WHERE id = ?")
                    .bind(error.unwrap_or("sync failed"))
                    .bind(id)
                    .execute(self.pool.as_ref())
                    .await?;
            }
        }
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct AppStoreRow {
    id: String,
    name: String,
    git_url: String,
    enabled: i64,
    last_synced_at: Option<i64>,
    last_error: Option<String>,
    created_at: String,
    created_by: String,
}

impl AppStoreRow {
    fn into_model(self) -> AppStore {
        AppStore {
            id: self.id,
            name: self.name,
            git_url: self.git_url,
            enabled: self.enabled != 0,
            last_synced_at: self.last_synced_at,
            last_error: self.last_error,
            created_at: self.created_at,
            created_by: self.created_by,
        }
    }
}
