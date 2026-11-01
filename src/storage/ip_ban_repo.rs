/* =============================================================================
 * File          : ip_ban_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for login bans and the auto-block policy: the blocked addresses
 *   (with their expiry and whether a firewall rule was installed) and the
 *   singleton settings row that decides when an address is blocked.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::auto_block
 *
 * Usage:
 *   src/auto_block.rs (enforcement + sweep), handlers/auto_block.rs (card).
 * =============================================================================
 */

//! Login-ban storage (`ip_bans` + `auto_block_settings`, migration 065).

use sqlx::SqlitePool;
use std::sync::Arc;

use crate::auto_block::{AutoBlockSettings, AutoBlockStatus, IpBan};
use crate::storage::database::DbError;

#[derive(Clone)]
pub struct IpBanRepository {
    pool: Arc<SqlitePool>,
}

impl IpBanRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    // ---- settings ------------------------------------------------------

    /// The saved policy, or the defaults when the row was never written.
    ///
    /// A missing row is normal on an upgraded box (the migration inserts no
    /// row), so this never fails on that path.
    pub async fn settings(&self) -> Result<AutoBlockSettings, DbError> {
        let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
            "SELECT enabled, threshold, window_seconds, ban_seconds, apply_firewall, \
                    dos_enabled, dos_requests_per_minute \
             FROM auto_block_settings WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(match row {
            Some((enabled, threshold, window, ban, firewall, dos, rpm)) => AutoBlockSettings {
                enabled: enabled != 0,
                threshold: threshold.clamp(1, 100) as u32,
                window_seconds: window.clamp(0, 86_400) as u32,
                ban_seconds: ban.clamp(0, 31_536_000) as u32,
                apply_firewall: firewall != 0,
                dos_enabled: dos != 0,
                dos_requests_per_minute: rpm.clamp(0, 1_000_000) as u32,
            },
            None => AutoBlockSettings::default(),
        })
    }

    pub async fn save_settings(&self, settings: &AutoBlockSettings) -> Result<(), DbError> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO auto_block_settings \
                (id, enabled, threshold, window_seconds, ban_seconds, apply_firewall, \
                 dos_enabled, dos_requests_per_minute, updated_at) \
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
                enabled = excluded.enabled, \
                threshold = excluded.threshold, \
                window_seconds = excluded.window_seconds, \
                ban_seconds = excluded.ban_seconds, \
                apply_firewall = excluded.apply_firewall, \
                dos_enabled = excluded.dos_enabled, \
                dos_requests_per_minute = excluded.dos_requests_per_minute, \
                updated_at = excluded.updated_at",
        )
        .bind(i64::from(settings.enabled))
        .bind(i64::from(settings.threshold))
        .bind(i64::from(settings.window_seconds))
        .bind(i64::from(settings.ban_seconds))
        .bind(i64::from(settings.apply_firewall))
        .bind(i64::from(settings.dos_enabled))
        .bind(i64::from(settings.dos_requests_per_minute))
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    // ---- bans ----------------------------------------------------------

    /// Every ban row, newest first (expired ones included: the card shows
    /// "expired" honestly rather than hiding a row that still exists).
    pub async fn list(&self) -> Result<Vec<IpBan>, DbError> {
        Ok(sqlx::query_as::<_, IpBan>(
            "SELECT id, ip, reason, failures, created_at, expires_at, firewall_rule \
             FROM ip_bans ORDER BY created_at DESC",
        )
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    /// The live ban for `ip`, if one exists and has not expired.
    ///
    /// `expires_at` is stored as RFC 3339; the comparison is a string compare
    /// on the same format, which SQLite orders correctly for RFC 3339 UTC.
    pub async fn find_active(&self, ip: &str, now: &str) -> Result<Option<IpBan>, DbError> {
        Ok(sqlx::query_as::<_, IpBan>(
            "SELECT id, ip, reason, failures, created_at, expires_at, firewall_rule \
             FROM ip_bans WHERE ip = ? AND expires_at > ?",
        )
        .bind(ip)
        .bind(now)
        .fetch_optional(self.pool.as_ref())
        .await?)
    }

    /// Create or extend a ban. Re-banning the same address replaces its expiry
    /// and its counter instead of stacking rows (one address, one row).
    pub async fn upsert(
        &self,
        ip: &str,
        reason: &str,
        failures: i64,
        created_at: &str,
        expires_at: &str,
        apply_firewall: bool,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO ip_bans (ip, reason, failures, created_at, expires_at, firewall_rule) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(ip) DO UPDATE SET \
                reason = excluded.reason, \
                failures = excluded.failures, \
                created_at = excluded.created_at, \
                expires_at = excluded.expires_at, \
                firewall_rule = MAX(ip_bans.firewall_rule, excluded.firewall_rule)",
        )
        .bind(ip)
        .bind(reason)
        .bind(failures)
        .bind(created_at)
        .bind(expires_at)
        .bind(i64::from(apply_firewall))
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Mark a ban as also installed in the firewall (or not).
    pub async fn set_firewall_rule(&self, ip: &str, installed: bool) -> Result<(), DbError> {
        sqlx::query("UPDATE ip_bans SET firewall_rule = ? WHERE ip = ?")
            .bind(i64::from(installed))
            .bind(ip)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn delete(&self, ip: &str) -> Result<bool, DbError> {
        let result = sqlx::query("DELETE FROM ip_bans WHERE ip = ?")
            .bind(ip)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Rows whose expiry has passed (the sweep's work list).
    pub async fn expired(&self, now: &str) -> Result<Vec<IpBan>, DbError> {
        Ok(sqlx::query_as::<_, IpBan>(
            "SELECT id, ip, reason, failures, created_at, expires_at, firewall_rule \
             FROM ip_bans WHERE expires_at <= ?",
        )
        .bind(now)
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    /// The whole picture the Security card renders.
    pub async fn status(&self, now: &str) -> Result<AutoBlockStatus, DbError> {
        let settings = self.settings().await?;
        let bans = self.list().await?;
        let mut active = 0usize;
        let mut firewall_rules = 0usize;
        for ban in &bans {
            if ban.is_active(now) {
                active += 1;
                if ban.firewall_rule {
                    firewall_rules += 1;
                }
            }
        }
        Ok(AutoBlockStatus {
            settings,
            bans,
            active,
            firewall_rules,
        })
    }
}
