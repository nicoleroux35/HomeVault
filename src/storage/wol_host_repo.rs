/* =============================================================================
 * File          : wol_host_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for the hosts the box may wake: a label, the target NIC address
 *   and an optional destination override, plus the CRUD the Settings card and
 *   the scheduled-task engine need.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::wol
 *
 * Usage:
 *   handlers/wol.rs and scheduled_tasks_scheduler.rs.
 * =============================================================================
 */

//! Wake-on-LAN host storage (`wol_hosts`, migration 064).

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::storage::database::DbError;
use crate::wol::WolHost;

#[derive(Clone)]
pub struct WolHostRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(WolHostRepository);

impl WolHostRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Every saved host, alphabetically — the order the card and the
    /// scheduled-task picker both show.
    pub async fn list(&self) -> Result<Vec<WolHost>, DbError> {
        Ok(sqlx::query_as::<_, WolHost>(
            "SELECT id, label, mac, broadcast, created_at, updated_at \
             FROM wol_hosts ORDER BY label COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    pub async fn find(&self, id: i64) -> Result<Option<WolHost>, DbError> {
        Ok(sqlx::query_as::<_, WolHost>(
            "SELECT id, label, mac, broadcast, created_at, updated_at \
             FROM wol_hosts WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?)
    }

    /// Whether another row already uses this label (case-insensitively).
    ///
    /// Checked explicitly so the handler can answer with a clear message
    /// instead of surfacing the unique-index constraint as a 500.
    pub async fn label_taken(&self, label: &str, except_id: Option<i64>) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM wol_hosts WHERE label = ? COLLATE NOCASE AND id IS NOT ?",
        )
        .bind(label)
        .bind(except_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    pub async fn create(
        &self,
        label: &str,
        mac: &str,
        broadcast: Option<&str>,
    ) -> Result<WolHost, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO wol_hosts (label, mac, broadcast, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(label)
        .bind(mac)
        .bind(broadcast)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.find(result.last_insert_rowid() as i64)
            .await?
            .ok_or(DbError::NotFound)
    }

    pub async fn update(
        &self,
        id: i64,
        label: &str,
        mac: &str,
        broadcast: Option<&str>,
    ) -> Result<WolHost, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE wol_hosts SET label = ?, mac = ?, broadcast = ?, updated_at = ? WHERE id = ?",
        )
        .bind(label)
        .bind(mac)
        .bind(broadcast)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.find(id).await?.ok_or(DbError::NotFound)
    }

    pub async fn delete(&self, id: i64) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM wol_hosts WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
