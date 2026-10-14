/* =============================================================================
 * File          : antivirus_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Antivirus storage: singleton settings row, scan-run history and
 *   quarantined-threat records.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for antivirus (ClamAV) scanning: a singleton settings row,
//! a history of scan runs, and the threats found during those scans.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// Singleton antivirus configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AvSettings {
    pub quarantine_dir: String,
    pub scheduled_scan_enabled: bool,
    pub scheduled_scan_cron: String,
    pub scheduled_scan_path: Option<String>,
    pub scan_archives: bool,
    pub last_scan_at: Option<String>,
}

impl Default for AvSettings {
    fn default() -> Self {
        Self {
            // Respects the `DATA_DIRECTORY` override (see
            // `crate::config::paths`) rather than hardcoding
            // `/var/lib/homevault/quarantine` — an admin running with a
            // customized data directory would otherwise get a default
            // quarantine path outside it entirely.
            quarantine_dir: crate::config::paths::quarantine_dir()
                .to_string_lossy()
                .to_string(),
            scheduled_scan_enabled: false,
            scheduled_scan_cron: "0 3 * * 0".to_string(),
            scheduled_scan_path: None,
            scan_archives: true,
            last_scan_at: None,
        }
    }
}

/// A single scan run (manual or scheduled).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AvScan {
    pub id: String,
    pub scan_path: String,
    pub trigger: String,
    pub status: String,
    pub files_scanned: u64,
    pub threats_found: u64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error_message: Option<String>,
}

/// A threat found during a scan.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AvThreat {
    pub id: String,
    pub scan_id: String,
    pub file_path: String,
    pub signature_name: String,
    pub action: String,
    pub quarantine_path: Option<String>,
    pub detected_at: String,
    pub resolved_at: Option<String>,
}

pub struct AntivirusRepository {
    pool: Arc<SqlitePool>,
}

impl AntivirusRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn get_settings(&self) -> Result<AvSettings, sqlx::Error> {
        let row = sqlx::query_as::<_, AvSettingsRow>(
            "SELECT quarantine_dir, scheduled_scan_enabled, scheduled_scan_cron, scheduled_scan_path, scan_archives, last_scan_at FROM av_settings WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => AvSettings::default(),
        })
    }

    pub async fn save_settings(
        &self,
        quarantine_dir: &str,
        scheduled_scan_enabled: bool,
        scheduled_scan_cron: &str,
        scheduled_scan_path: Option<&str>,
        scan_archives: bool,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO av_settings (id, quarantine_dir, scheduled_scan_enabled, scheduled_scan_cron, scheduled_scan_path, scan_archives, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                quarantine_dir = excluded.quarantine_dir,
                scheduled_scan_enabled = excluded.scheduled_scan_enabled,
                scheduled_scan_cron = excluded.scheduled_scan_cron,
                scheduled_scan_path = excluded.scheduled_scan_path,
                scan_archives = excluded.scan_archives,
                updated_at = excluded.updated_at",
        )
        .bind(quarantine_dir)
        .bind(scheduled_scan_enabled as i64)
        .bind(scheduled_scan_cron)
        .bind(scheduled_scan_path)
        .bind(scan_archives as i64)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn set_last_scan_at(&self, at: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE av_settings SET last_scan_at = ? WHERE id = 1")
            .bind(at)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn create_scan(&self, scan_path: &str, trigger: &str) -> Result<AvScan, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO av_scans (id, scan_path, trigger, status, files_scanned, threats_found, started_at)
             VALUES (?, ?, ?, 'running', 0, 0, ?)",
        )
        .bind(&id)
        .bind(scan_path)
        .bind(trigger)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(AvScan {
            id,
            scan_path: scan_path.to_string(),
            trigger: trigger.to_string(),
            status: "running".to_string(),
            files_scanned: 0,
            threats_found: 0,
            started_at: now,
            finished_at: None,
            error_message: None,
        })
    }

    pub async fn finish_scan(
        &self,
        id: &str,
        status: &str,
        files_scanned: u64,
        threats_found: u64,
        error_message: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE av_scans SET status = ?, files_scanned = ?, threats_found = ?, finished_at = ?, error_message = ? WHERE id = ?",
        )
        .bind(status)
        .bind(files_scanned as i64)
        .bind(threats_found as i64)
        .bind(&now)
        .bind(error_message)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn list_scans(&self, limit: i64) -> Result<Vec<AvScan>, sqlx::Error> {
        let rows = sqlx::query_as::<_, AvScanRow>(
            "SELECT id, scan_path, trigger, status, files_scanned, threats_found, started_at, finished_at, error_message
             FROM av_scans ORDER BY started_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    pub async fn add_threat(
        &self,
        scan_id: &str,
        file_path: &str,
        signature_name: &str,
        action: &str,
        quarantine_path: Option<&str>,
    ) -> Result<AvThreat, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO av_threats (id, scan_id, file_path, signature_name, action, quarantine_path, detected_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(scan_id)
        .bind(file_path)
        .bind(signature_name)
        .bind(action)
        .bind(quarantine_path)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(AvThreat {
            id,
            scan_id: scan_id.to_string(),
            file_path: file_path.to_string(),
            signature_name: signature_name.to_string(),
            action: action.to_string(),
            quarantine_path: quarantine_path.map(|s| s.to_string()),
            detected_at: now,
            resolved_at: None,
        })
    }

    pub async fn list_threats(&self, unresolved_only: bool) -> Result<Vec<AvThreat>, sqlx::Error> {
        let sql = if unresolved_only {
            "SELECT id, scan_id, file_path, signature_name, action, quarantine_path, detected_at, resolved_at FROM av_threats WHERE resolved_at IS NULL ORDER BY detected_at DESC"
        } else {
            "SELECT id, scan_id, file_path, signature_name, action, quarantine_path, detected_at, resolved_at FROM av_threats ORDER BY detected_at DESC"
        };
        let rows = sqlx::query_as::<_, AvThreatRow>(sql)
            .fetch_all(self.pool.as_ref())
            .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    pub async fn get_threat(&self, id: &str) -> Result<Option<AvThreat>, sqlx::Error> {
        let row = sqlx::query_as::<_, AvThreatRow>(
            "SELECT id, scan_id, file_path, signature_name, action, quarantine_path, detected_at, resolved_at FROM av_threats WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|r| r.into_model()))
    }

    pub async fn resolve_threat(&self, id: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE av_threats SET resolved_at = ? WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct AvSettingsRow {
    quarantine_dir: String,
    scheduled_scan_enabled: i64,
    scheduled_scan_cron: String,
    scheduled_scan_path: Option<String>,
    scan_archives: i64,
    last_scan_at: Option<String>,
}

impl AvSettingsRow {
    fn into_model(self) -> AvSettings {
        AvSettings {
            quarantine_dir: self.quarantine_dir,
            scheduled_scan_enabled: self.scheduled_scan_enabled != 0,
            scheduled_scan_cron: self.scheduled_scan_cron,
            scheduled_scan_path: self.scheduled_scan_path,
            scan_archives: self.scan_archives != 0,
            last_scan_at: self.last_scan_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AvScanRow {
    id: String,
    scan_path: String,
    trigger: String,
    status: String,
    files_scanned: i64,
    threats_found: i64,
    started_at: String,
    finished_at: Option<String>,
    error_message: Option<String>,
}

impl AvScanRow {
    fn into_model(self) -> AvScan {
        AvScan {
            id: self.id,
            scan_path: self.scan_path,
            trigger: self.trigger,
            status: self.status,
            files_scanned: self.files_scanned.max(0) as u64,
            threats_found: self.threats_found.max(0) as u64,
            started_at: self.started_at,
            finished_at: self.finished_at,
            error_message: self.error_message,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AvThreatRow {
    id: String,
    scan_id: String,
    file_path: String,
    signature_name: String,
    action: String,
    quarantine_path: Option<String>,
    detected_at: String,
    resolved_at: Option<String>,
}

impl AvThreatRow {
    fn into_model(self) -> AvThreat {
        AvThreat {
            id: self.id,
            scan_id: self.scan_id,
            file_path: self.file_path,
            signature_name: self.signature_name,
            action: self.action,
            quarantine_path: self.quarantine_path,
            detected_at: self.detected_at,
            resolved_at: self.resolved_at,
        }
    }
}
