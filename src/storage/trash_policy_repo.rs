/* =============================================================================
 * File          : trash_policy_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Explorer Trash retention policy (singleton row: retention days, size
 *   ceiling, last-purge timestamp).
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the trash scheduler and the Explorer trash handlers.
 * =============================================================================
 */

//! Storage layer for the Explorer Trash retention policy: a singleton
//! `trash_policy` row controlling how long deleted files are kept and how
//! large a trash root may grow before the oldest items are removed.

use sqlx::SqlitePool;
use std::sync::Arc;

/// Persisted trash retention policy plus the scheduler's last-run stamp.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrashPolicySettings {
    /// Days after which a trashed item is permanently removed. 0 = forever.
    pub retention_days: u32,
    /// Size ceiling for one trash root, in bytes. 0 = no ceiling.
    pub max_bytes: u64,
    /// When the background pass last ran, RFC 3339. `None` = never.
    pub last_purge_at: Option<String>,
}

impl Default for TrashPolicySettings {
    /// Boundary by default (GDPR Art. 5(1)(e) storage limitation): a home
    /// user should not have to discover a retention setting for a forgotten
    /// trash to stop growing. Matches `crate::browser::TrashPolicy::default`.
    fn default() -> Self {
        let policy = crate::browser::TrashPolicy::default();
        Self {
            retention_days: policy.retention_days,
            max_bytes: policy.max_bytes,
            last_purge_at: None,
        }
    }
}

impl TrashPolicySettings {
    /// The filesystem-facing policy (age + size limits only).
    pub fn policy(&self) -> crate::browser::TrashPolicy {
        crate::browser::TrashPolicy {
            retention_days: self.retention_days,
            max_bytes: self.max_bytes,
        }
    }
}

pub struct TrashPolicyRepository {
    pool: Arc<SqlitePool>,
}

impl TrashPolicyRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Load the policy, returning the default (`TrashPolicySettings::default`)
    /// when nothing has been saved yet.
    pub async fn get_settings(&self) -> Result<TrashPolicySettings, sqlx::Error> {
        let row = sqlx::query_as::<_, TrashPolicyRow>(
            "SELECT retention_days, max_bytes, last_purge_at FROM trash_policy WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => TrashPolicySettings::default(),
        })
    }

    /// Insert or replace the singleton policy row. Zero is a valid value for
    /// both limits and means "limit disabled", so it is preserved verbatim.
    pub async fn save_settings(
        &self,
        retention_days: u32,
        max_bytes: u64,
    ) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO trash_policy (id, retention_days, max_bytes, updated_at)
             VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                retention_days = excluded.retention_days,
                max_bytes = excluded.max_bytes,
                updated_at = excluded.updated_at",
        )
        .bind(retention_days as i64)
        .bind(max_bytes as i64)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Record when the last retention pass ran. Upserts the singleton row so a
    /// plain `UPDATE ... WHERE id = 1` cannot silently no-op on a fresh
    /// install before the settings were ever saved.
    pub async fn set_last_purge(&self, at: &str) -> Result<(), sqlx::Error> {
        let defaults = TrashPolicySettings::default();
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO trash_policy (id, retention_days, max_bytes, last_purge_at, updated_at)
             VALUES (1, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                last_purge_at = excluded.last_purge_at,
                updated_at = excluded.updated_at",
        )
        .bind(defaults.retention_days as i64)
        .bind(defaults.max_bytes as i64)
        .bind(at)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct TrashPolicyRow {
    retention_days: i64,
    max_bytes: i64,
    last_purge_at: Option<String>,
}

impl TrashPolicyRow {
    fn into_model(self) -> TrashPolicySettings {
        TrashPolicySettings {
            retention_days: self.retention_days.max(0) as u32,
            max_bytes: self.max_bytes.max(0) as u64,
            last_purge_at: self.last_purge_at,
        }
    }
}
