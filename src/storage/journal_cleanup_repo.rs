/* =============================================================================
 * File          : journal_cleanup_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   System Journal auto-cleanup setting (singleton row: enabled,
 *   interval days, last-run timestamp).
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for the System Journal app's auto-cleanup setting: a
//! singleton `journal_cleanup_settings` row controlling whether HomeVault's
//! own rotating log files (the "app" source in `src/handlers/journal.rs`)
//! are automatically deleted every `interval_days` days.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Singleton journal auto-cleanup configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JournalCleanupSettings {
    pub auto_cleanup_enabled: bool,
    pub interval_days: u32,
    pub last_cleanup_at: Option<String>,
}

impl Default for JournalCleanupSettings {
    /// Privacy by default (GDPR Art. 25 / ISO 29100 storage limitation):
    /// bounded retention is the out-of-the-box behaviour, not something an
    /// admin has to discover and switch on. 90 days keeps enough security
    /// audit history (login / 2FA / user & permission changes) to be useful
    /// for incident response while still expiring old personal data; the
    /// admin can shorten or disable it from the System Journal app.
    fn default() -> Self {
        Self {
            auto_cleanup_enabled: true,
            interval_days: 90,
            last_cleanup_at: None,
        }
    }
}

pub struct JournalCleanupRepository {
    pool: Arc<SqlitePool>,
}

impl JournalCleanupRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Load settings, returning the default row (auto-cleanup on, 90-day
    /// interval — see `JournalCleanupSettings::default`) if none has been
    /// saved yet.
    pub async fn get_settings(&self) -> Result<JournalCleanupSettings, sqlx::Error> {
        let row = sqlx::query_as::<_, JournalCleanupSettingsRow>(
            "SELECT auto_cleanup_enabled, interval_days, last_cleanup_at FROM journal_cleanup_settings WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => JournalCleanupSettings::default(),
        })
    }

    /// Insert or replace the singleton settings row. `interval_days` is
    /// clamped to at least 1 — an interval of 0 would mean "run on every
    /// scheduler tick", which is never what the user asking for a number of
    /// days actually wants.
    pub async fn save_settings(
        &self,
        auto_cleanup_enabled: bool,
        interval_days: u32,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let interval_days = interval_days.max(1);
        sqlx::query(
            "INSERT INTO journal_cleanup_settings (id, auto_cleanup_enabled, interval_days, updated_at)
             VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                auto_cleanup_enabled = excluded.auto_cleanup_enabled,
                interval_days = excluded.interval_days,
                updated_at = excluded.updated_at",
        )
        .bind(auto_cleanup_enabled as i64)
        .bind(interval_days as i64)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Record when the last cleanup ran. Upserts the singleton row: a plain
    /// `UPDATE ... WHERE id = 1` is a silent no-op until an admin has saved
    /// the settings at least once (the row doesn't exist on a fresh
    /// install), which used to defeat the scheduler's "establish a
    /// baseline" logic and make it re-run on every boot. On insert, the
    /// row's other columns take `JournalCleanupSettings::default()`'s
    /// values, matching what `get_settings()` returns for a missing row.
    pub async fn set_last_cleanup(&self, at: &str) -> Result<(), sqlx::Error> {
        let defaults = JournalCleanupSettings::default();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO journal_cleanup_settings
                (id, auto_cleanup_enabled, interval_days, last_cleanup_at, updated_at)
             VALUES (1, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET last_cleanup_at = excluded.last_cleanup_at",
        )
        .bind(defaults.auto_cleanup_enabled as i64)
        .bind(defaults.interval_days as i64)
        .bind(at)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct JournalCleanupSettingsRow {
    auto_cleanup_enabled: i64,
    interval_days: i64,
    last_cleanup_at: Option<String>,
}

impl JournalCleanupSettingsRow {
    fn into_model(self) -> JournalCleanupSettings {
        JournalCleanupSettings {
            auto_cleanup_enabled: self.auto_cleanup_enabled != 0,
            interval_days: self.interval_days.max(1) as u32,
            last_cleanup_at: self.last_cleanup_at,
        }
    }
}
