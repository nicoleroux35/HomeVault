/* =============================================================================
 * File          : disk_power_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for per-disk spindown settings (hdparm APM level + idle
 *   timeout), persisted so they can be re-applied at startup.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   handlers/power.rs; startup re-apply in web/mod.rs.
 * =============================================================================
 */

//! Per-disk spindown settings storage.

use crate::power::DiskSpindownSetting;
use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

pub struct DiskPowerRepository {
    pool: Arc<SqlitePool>,
}

impl DiskPowerRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<DiskSpindownSetting>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, Option<i64>, Option<i64>)>(
            "SELECT device, apm_level, spindown_minutes FROM disk_power_settings \
             ORDER BY device",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(device, apm, minutes)| DiskSpindownSetting {
                device,
                apm_level: apm.map(|v| v as u8),
                spindown_minutes: minutes.map(|v| v as u32),
            })
            .collect())
    }

    /// Upserts one disk's settings. A row with both fields `None` is the
    /// "nothing configured" state and is deleted instead of stored.
    pub async fn upsert(&self, setting: &DiskSpindownSetting) -> Result<(), sqlx::Error> {
        if setting.apm_level.is_none() && setting.spindown_minutes.is_none() {
            return self.delete(&setting.device).await;
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO disk_power_settings (device, apm_level, spindown_minutes, updated_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(device) DO UPDATE SET \
             apm_level = excluded.apm_level, \
             spindown_minutes = excluded.spindown_minutes, \
             updated_at = excluded.updated_at",
        )
        .bind(&setting.device)
        .bind(setting.apm_level.map(|v| v as i64))
        .bind(setting.spindown_minutes.map(|v| v as i64))
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn delete(&self, device: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM disk_power_settings WHERE device = ?")
            .bind(device)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> DiskPowerRepository {
        let pool = Arc::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap(),
        );
        sqlx::query(
            "CREATE TABLE disk_power_settings (device TEXT PRIMARY KEY, apm_level INTEGER, \
             spindown_minutes INTEGER, updated_at TEXT NOT NULL)",
        )
        .execute(pool.as_ref())
        .await
        .unwrap();
        DiskPowerRepository::new(pool)
    }

    #[tokio::test]
    async fn upsert_then_list_round_trips() {
        let repo = repo().await;
        repo.upsert(&DiskSpindownSetting {
            device: "sda".into(),
            apm_level: Some(127),
            spindown_minutes: Some(10),
        })
        .await
        .unwrap();
        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].apm_level, Some(127));
        assert_eq!(list[0].spindown_minutes, Some(10));
    }

    #[tokio::test]
    async fn empty_setting_deletes_the_row() {
        let repo = repo().await;
        repo.upsert(&DiskSpindownSetting {
            device: "sda".into(),
            apm_level: Some(127),
            spindown_minutes: None,
        })
        .await
        .unwrap();
        // Saving "nothing configured" removes the row — no phantom entries.
        repo.upsert(&DiskSpindownSetting {
            device: "sda".into(),
            apm_level: None,
            spindown_minutes: None,
        })
        .await
        .unwrap();
        assert!(repo.list().await.unwrap().is_empty());
    }
}
