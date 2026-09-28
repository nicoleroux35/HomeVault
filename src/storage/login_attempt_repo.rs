use crate::storage::database::DbError;
use crate::impl_repository;
use chrono::{Duration, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;

/// A single login attempt record (successful or failed).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LoginAttempt {
    pub id: i64,
    pub username: String,
    pub ip_address: String,
    /// `true` for a successful login, `false` for a failed one.
    pub success: bool,
    /// Browser user-agent captured from the login request, if any.
    pub user_agent: Option<String>,
    /// ISO-8601 timestamp of the attempt.
    pub attempted_at: String,
}

/// Repository for login attempt tracking (rate-limiting and audit).
#[derive(Clone)]
pub struct LoginAttemptRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(LoginAttemptRepository);

impl LoginAttemptRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Recording a login attempt (successful or failed) for the audit history
    /// and rate-limiting.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_attempt(
        &self,
        username: &str,
        ip_address: &str,
        success: bool,
        user_agent: Option<&str>,
    ) -> Result<(), DbError> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO login_attempts (username, ip_address, success, user_agent, attempted_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(username)
        .bind(ip_address)
        .bind(success as i64)
        .bind(user_agent)
        .bind(now.to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Count failed attempts for a user within the last `window_seconds`.
    pub async fn get_attempts_count(
        &self,
        username: &str,
        window_seconds: i64,
    ) -> Result<i64, DbError> {
        let cutoff = Utc::now() - Duration::seconds(window_seconds);
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM login_attempts
             WHERE username = ? AND success = 0 AND attempted_at > ?",
        )
        .bind(username)
        .bind(cutoff.to_rfc3339())
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(row.0)
    }

    /// Count failed attempts from a given IP within the time window.
    pub async fn get_attempts_count_by_ip(
        &self,
        ip_address: &str,
        window_seconds: i64,
    ) -> Result<i64, DbError> {
        let cutoff = Utc::now() - Duration::seconds(window_seconds);
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM login_attempts
             WHERE ip_address = ? AND success = 0 AND attempted_at > ?",
        )
        .bind(ip_address)
        .bind(cutoff.to_rfc3339())
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(row.0)
    }

    /// Clear the failed-attempt history for a user (e.g. after a successful
    /// login). Successful attempts are kept for the audit trail.
    pub async fn clear_attempts(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM login_attempts WHERE username = ? AND success = 0")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Delete attempts older than the given number of days.
    pub async fn prune_old_entries(&self, older_than_days: i64) -> Result<u64, DbError> {
        let cutoff = Utc::now() - Duration::days(older_than_days);
        let result = sqlx::query("DELETE FROM login_attempts WHERE attempted_at < ?")
            .bind(cutoff.to_rfc3339())
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected())
    }

    /// List the most recent attempts for a user.
    pub async fn get_recent_attempts(
        &self,
        username: &str,
        limit: i64,
    ) -> Result<Vec<LoginAttempt>, DbError> {
        let attempts = sqlx::query_as::<_, LoginAttempt>(
            "SELECT id, username, ip_address, success, user_agent, attempted_at
             FROM login_attempts
             WHERE username = ?
             ORDER BY attempted_at DESC
             LIMIT ?"
        )
        .bind(username)
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(attempts)
    }

    /// List the most recent attempts across all users (admin view).
    pub async fn get_all_recent(&self, limit: i64) -> Result<Vec<LoginAttempt>, DbError> {
        let attempts = sqlx::query_as::<_, LoginAttempt>(
            "SELECT id, username, ip_address, success, user_agent, attempted_at
             FROM login_attempts
             ORDER BY attempted_at DESC
             LIMIT ?"
        )
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(attempts)
    }
}
