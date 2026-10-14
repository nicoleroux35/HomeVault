/* =============================================================================
 * File          : firewall_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Firewall storage: singleton enable/settings row and an audit trail
 *   of changes (live rules always read from ufw, never mirrored).
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Storage layer for firewall (ufw) management: a singleton settings row and
//! an audit trail of changes. Rules themselves are never mirrored here —
//! `ufw status` on the live system is always the source of truth (see
//! `src/firewall.rs`), avoiding drift between the DB and reality.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// Singleton firewall configuration/state cache.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FirewallSettings {
    pub enabled: bool,
    pub ssh_port: u16,
    pub app_port: u16,
    pub confirmed_at: Option<String>,
}

impl Default for FirewallSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            ssh_port: 22,
            app_port: 8080,
            confirmed_at: None,
        }
    }
}

/// A single audit-log entry for a firewall change made through HomeVault.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FirewallChangeLogEntry {
    pub id: String,
    pub action: String,
    pub rule_description: Option<String>,
    pub performed_by: String,
    pub created_at: String,
}

pub struct FirewallRepository {
    pool: Arc<SqlitePool>,
}

impl FirewallRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn get_settings(&self) -> Result<FirewallSettings, sqlx::Error> {
        let row = sqlx::query_as::<_, FirewallSettingsRow>(
            "SELECT enabled, ssh_port, app_port, confirmed_at FROM firewall_settings WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(match row {
            Some(r) => r.into_model(),
            None => FirewallSettings::default(),
        })
    }

    pub async fn save_state(
        &self,
        enabled: bool,
        ssh_port: u16,
        app_port: u16,
        confirmed_at: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO firewall_settings (id, enabled, ssh_port, app_port, confirmed_at, updated_at)
             VALUES (1, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                enabled = excluded.enabled,
                ssh_port = excluded.ssh_port,
                app_port = excluded.app_port,
                confirmed_at = excluded.confirmed_at,
                updated_at = excluded.updated_at",
        )
        .bind(enabled as i64)
        .bind(ssh_port as i64)
        .bind(app_port as i64)
        .bind(confirmed_at)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn record_change(
        &self,
        action: &str,
        rule_description: Option<&str>,
        performed_by: &str,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO firewall_change_log (id, action, rule_description, performed_by, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(action)
        .bind(rule_description)
        .bind(performed_by)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn list_change_log(
        &self,
        limit: i64,
    ) -> Result<Vec<FirewallChangeLogEntry>, sqlx::Error> {
        let rows = sqlx::query_as::<_, FirewallChangeLogRow>(
            "SELECT id, action, rule_description, performed_by, created_at FROM firewall_change_log ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }
}

#[derive(sqlx::FromRow)]
struct FirewallSettingsRow {
    enabled: i64,
    ssh_port: i64,
    app_port: i64,
    confirmed_at: Option<String>,
}

impl FirewallSettingsRow {
    fn into_model(self) -> FirewallSettings {
        FirewallSettings {
            enabled: self.enabled != 0,
            ssh_port: self.ssh_port.clamp(1, 65535) as u16,
            app_port: self.app_port.clamp(1, 65535) as u16,
            confirmed_at: self.confirmed_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct FirewallChangeLogRow {
    id: String,
    action: String,
    rule_description: Option<String>,
    performed_by: String,
    created_at: String,
}

impl FirewallChangeLogRow {
    fn into_model(self) -> FirewallChangeLogEntry {
        FirewallChangeLogEntry {
            id: self.id,
            action: self.action,
            rule_description: self.rule_description,
            performed_by: self.performed_by,
            created_at: self.created_at,
        }
    }
}
