/* =============================================================================
 * File          : password_policy_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for the global password-expiry policy and the per-account
 *   `password_changed_at` marker. Both are deliberately outside the `User`
 *   model: only the sign-in check and the two password-writing paths read
 *   them.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), chrono
 * Usage:
 *   Imported by `crate::auth::AuthService`.
 * =============================================================================
 */

//! Global password-expiry policy (tasklist P1-2).
//!
//! * `password_policy.expiry_days = 0` — never expire (the default).
//! * `users.password_changed_at` — RFC 3339; `NULL` on legacy rows, in which
//!   case the caller falls back to `created_at`.

use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Upper bound accepted by the API: ten years. Beyond that the setting is
/// indistinguishable from “never” and only hides a misconfiguration.
pub const MAX_EXPIRY_DAYS: i64 = 3650;

pub struct PasswordPolicyRepository {
    pool: Arc<SqlitePool>,
}

impl PasswordPolicyRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// `0` when the policy row is missing (a pre-migration database or a
    /// hand-edited file), which reads as “never expire”.
    pub async fn get_expiry_days(&self) -> Result<i64, sqlx::Error> {
        let days: Option<i64> =
            sqlx::query_scalar("SELECT expiry_days FROM password_policy WHERE id = 1")
                .fetch_optional(self.pool.as_ref())
                .await?;
        Ok(days.unwrap_or(0).clamp(0, MAX_EXPIRY_DAYS))
    }

    pub async fn set_expiry_days(&self, days: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO password_policy (id, expiry_days) VALUES (1, ?)
             ON CONFLICT(id) DO UPDATE SET expiry_days = excluded.expiry_days",
        )
        .bind(days.clamp(0, MAX_EXPIRY_DAYS))
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// When the current password was set. Falls back to `created_at` for rows
    /// written before the column existed, and `None` for an unknown user.
    pub async fn password_changed_at(
        &self,
        username: &str,
    ) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        let row: Option<(Option<String>, String)> =
            sqlx::query_as("SELECT password_changed_at, created_at FROM users WHERE username = ?")
                .bind(username)
                .fetch_optional(self.pool.as_ref())
                .await?;

        let Some((changed_at, created_at)) = row else {
            return Ok(None);
        };
        Ok(parse_rfc3339(changed_at.as_deref().unwrap_or(&created_at)))
    }

    /// Flag the account so `middleware::password_change_guard` limits its
    /// session to the password-change flow. Returns whether a row changed.
    pub async fn mark_must_change(&self, username: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE users SET must_change_password = 1
             WHERE username = ? AND must_change_password = 0",
        )
        .bind(username)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Record that the password was just chosen by its owner (self change or
    /// recovery). Called by the two password-writing paths, after the row is
    /// updated.
    pub async fn touch_password_changed_at(
        &self,
        username: &str,
        at: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET password_changed_at = ? WHERE username = ?")
            .bind(at.to_rfc3339())
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    // `created_at` and rows written by older builds use SQLite's
    // `datetime('now')` shape (`YYYY-MM-DD HH:MM:SS`, no offset, UTC). A
    // legacy account must still be governed by the expiry policy instead of
    // silently falling back to "never expires".
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return Some(naive.and_utc());
        }
    }
    None
}

/// Whether `changed_at + expiry_days` is in the past. Pure, so the edge cases
/// (day 0, exact boundary, future timestamp) are unit-tested without a DB.
pub fn is_expired(changed_at: DateTime<Utc>, expiry_days: i64, now: DateTime<Utc>) -> bool {
    if expiry_days <= 0 {
        return false;
    }
    let horizon = changed_at + chrono::Duration::days(expiry_days);
    now > horizon
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + seconds, 0).unwrap()
    }

    #[test]
    fn test_zero_days_never_expires() {
        assert!(!is_expired(at(0), 0, at(10 * 365 * 86_400)));
    }

    #[test]
    fn test_expiry_is_strictly_after_the_horizon() {
        let changed = at(0);
        assert!(!is_expired(
            changed,
            30,
            changed + chrono::Duration::days(29)
        ));
        assert!(!is_expired(
            changed,
            30,
            changed + chrono::Duration::days(30)
        ));
        assert!(is_expired(
            changed,
            30,
            changed + chrono::Duration::days(30) + chrono::Duration::seconds(1)
        ));
    }

    #[test]
    fn test_future_timestamp_is_not_expired() {
        assert!(!is_expired(at(86_400), 1, at(0)));
    }

    #[test]
    fn test_parse_rfc3339_accepts_offsets_and_rejects_garbage() {
        assert!(parse_rfc3339("2026-11-05T10:00:00+01:00").is_some());
        assert!(parse_rfc3339("not a date").is_none());
    }
}
