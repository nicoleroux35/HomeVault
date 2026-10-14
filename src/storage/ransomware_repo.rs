/* =============================================================================
 * File          : ransomware_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Anti-ransomware storage: singleton settings row, protected paths
 *   with honeypots, and detected-event records.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for anti-ransomware protection: a singleton settings row,
//! user-configured protected paths (with an optional honeypot file), and the
//! history of detected events.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// Singleton anti-ransomware configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RansomwareSettings {
    pub enabled: bool,
    pub honeypots_enabled: bool,
    pub rate_threshold_events: u32,
    pub rate_threshold_seconds: u32,
    /// One of "alert_only" | "lock_account" | "disable_share".
    pub auto_response: String,
}

impl Default for RansomwareSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            honeypots_enabled: true,
            rate_threshold_events: 50,
            rate_threshold_seconds: 10,
            auto_response: "alert_only".to_string(),
        }
    }
}

/// A path watched for suspicious activity, typically backed by a share.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProtectedPath {
    pub id: String,
    pub path: String,
    pub share_name: Option<String>,
    pub share_type: Option<String>,
    pub honeypot_filename: Option<String>,
    pub created_at: String,
}

/// A detected suspicious event on a protected path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RansomwareEvent {
    pub id: String,
    pub protected_path_id: Option<String>,
    pub event_type: String,
    pub detail: Option<String>,
    pub suspected_username: Option<String>,
    pub response_taken: String,
    pub created_at: String,
}

pub struct RansomwareRepository {
    pool: Arc<SqlitePool>,
}

impl RansomwareRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn get_settings(&self) -> Result<RansomwareSettings, sqlx::Error> {
        let row = sqlx::query_as::<_, RansomwareSettingsRow>(
            "SELECT enabled, honeypots_enabled, rate_threshold_events, rate_threshold_seconds, auto_response FROM ransomware_settings WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => RansomwareSettings::default(),
        })
    }

    pub async fn save_settings(
        &self,
        enabled: bool,
        honeypots_enabled: bool,
        rate_threshold_events: u32,
        rate_threshold_seconds: u32,
        auto_response: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO ransomware_settings (id, enabled, honeypots_enabled, rate_threshold_events, rate_threshold_seconds, auto_response, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                enabled = excluded.enabled,
                honeypots_enabled = excluded.honeypots_enabled,
                rate_threshold_events = excluded.rate_threshold_events,
                rate_threshold_seconds = excluded.rate_threshold_seconds,
                auto_response = excluded.auto_response,
                updated_at = excluded.updated_at",
        )
        .bind(enabled as i64)
        .bind(honeypots_enabled as i64)
        .bind(rate_threshold_events as i64)
        .bind(rate_threshold_seconds as i64)
        .bind(auto_response)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn list_protected_paths(&self) -> Result<Vec<ProtectedPath>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ProtectedPathRow>(
            "SELECT id, path, share_name, share_type, honeypot_filename, created_at FROM ransomware_protected_paths ORDER BY created_at",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    pub async fn get_protected_path(&self, id: &str) -> Result<Option<ProtectedPath>, sqlx::Error> {
        let row = sqlx::query_as::<_, ProtectedPathRow>(
            "SELECT id, path, share_name, share_type, honeypot_filename, created_at FROM ransomware_protected_paths WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|r| r.into_model()))
    }

    pub async fn add_protected_path(
        &self,
        path: &str,
        share_name: Option<&str>,
        share_type: Option<&str>,
        honeypot_filename: Option<&str>,
    ) -> Result<ProtectedPath, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO ransomware_protected_paths (id, path, share_name, share_type, honeypot_filename, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(path)
        .bind(share_name)
        .bind(share_type)
        .bind(honeypot_filename)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(ProtectedPath {
            id,
            path: path.to_string(),
            share_name: share_name.map(|s| s.to_string()),
            share_type: share_type.map(|s| s.to_string()),
            honeypot_filename: honeypot_filename.map(|s| s.to_string()),
            created_at: now,
        })
    }

    pub async fn remove_protected_path(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM ransomware_protected_paths WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn record_event(
        &self,
        protected_path_id: Option<&str>,
        event_type: &str,
        detail: Option<&str>,
        suspected_username: Option<&str>,
        response_taken: &str,
    ) -> Result<RansomwareEvent, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO ransomware_events (id, protected_path_id, event_type, detail, suspected_username, response_taken, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(protected_path_id)
        .bind(event_type)
        .bind(detail)
        .bind(suspected_username)
        .bind(response_taken)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(RansomwareEvent {
            id,
            protected_path_id: protected_path_id.map(|s| s.to_string()),
            event_type: event_type.to_string(),
            detail: detail.map(|s| s.to_string()),
            suspected_username: suspected_username.map(|s| s.to_string()),
            response_taken: response_taken.to_string(),
            created_at: now,
        })
    }

    pub async fn list_events(&self, limit: i64) -> Result<Vec<RansomwareEvent>, sqlx::Error> {
        let rows = sqlx::query_as::<_, RansomwareEventRow>(
            "SELECT id, protected_path_id, event_type, detail, suspected_username, response_taken, created_at
             FROM ransomware_events ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }
}

#[derive(sqlx::FromRow)]
struct RansomwareSettingsRow {
    enabled: i64,
    honeypots_enabled: i64,
    rate_threshold_events: i64,
    rate_threshold_seconds: i64,
    auto_response: String,
}

impl RansomwareSettingsRow {
    fn into_model(self) -> RansomwareSettings {
        RansomwareSettings {
            enabled: self.enabled != 0,
            honeypots_enabled: self.honeypots_enabled != 0,
            rate_threshold_events: self.rate_threshold_events.max(0) as u32,
            rate_threshold_seconds: self.rate_threshold_seconds.max(0) as u32,
            auto_response: self.auto_response,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ProtectedPathRow {
    id: String,
    path: String,
    share_name: Option<String>,
    share_type: Option<String>,
    honeypot_filename: Option<String>,
    created_at: String,
}

impl ProtectedPathRow {
    fn into_model(self) -> ProtectedPath {
        ProtectedPath {
            id: self.id,
            path: self.path,
            share_name: self.share_name,
            share_type: self.share_type,
            honeypot_filename: self.honeypot_filename,
            created_at: self.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct RansomwareEventRow {
    id: String,
    protected_path_id: Option<String>,
    event_type: String,
    detail: Option<String>,
    suspected_username: Option<String>,
    response_taken: String,
    created_at: String,
}

impl RansomwareEventRow {
    fn into_model(self) -> RansomwareEvent {
        RansomwareEvent {
            id: self.id,
            protected_path_id: self.protected_path_id,
            event_type: self.event_type,
            detail: self.detail,
            suspected_username: self.suspected_username,
            response_taken: self.response_taken,
            created_at: self.created_at,
        }
    }
}
