/* =============================================================================
 * File          : session_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Session rows backing authentication: token, CSRF token, IP,
 *   expiry, admin flag; create/find/delete/expiry-cleanup.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Session rows (token + CSRF + expiry) backing authentication.

use crate::impl_repository;
use crate::storage::database::DbError;
use crate::storage::models::{Session, SessionRow};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Repository for the `sessions` table (login sessions).
#[derive(Clone)]
pub struct SessionRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SessionRepository);

impl SessionRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up a session by its bearer token.
    pub async fn find_by_token(&self, token: &str) -> Result<Option<Session>, DbError> {
        let session = sqlx::query_as::<_, SessionRow>(
            "SELECT token, username, ip_address, csrf_token, created_at, expires_at, is_admin FROM sessions WHERE token = ?"
        )
        .bind(token)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(session.map(|s| s.into()))
    }

    /// Persist a new session.
    pub async fn create(&self, session: &Session) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO sessions (token, username, ip_address, csrf_token, created_at, expires_at, is_admin) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&session.token)
        .bind(&session.username)
        .bind(&session.ip_address)
        .bind(&session.csrf_token)
        .bind(session.created_at)
        .bind(session.expires_at)
        .bind(session.is_admin)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Delete a session by token.
    pub async fn delete(&self, token: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM sessions WHERE token = ?")
            .bind(token)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Delete all sessions belonging to a user.
    pub async fn delete_by_username(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM sessions WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Remove all sessions past their expiry date.
    pub async fn delete_expired(&self) -> Result<u64, DbError> {
        let now = chrono::Utc::now();
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(now)
            .execute(self.pool.as_ref())
            .await?;

        Ok(result.rows_affected())
    }

    /// Total number of active sessions.
    pub async fn count(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// Number of distinct users with at least one currently valid (non-expired)
    /// session — unlike `count()`, which counts every row including expired
    /// ones not yet swept by `delete_expired`, and doesn't dedupe a user with
    /// multiple concurrent sessions (several tabs/devices) into one.
    pub async fn count_distinct_active_users(&self) -> Result<i64, DbError> {
        let now = chrono::Utc::now();
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(DISTINCT username) FROM sessions WHERE expires_at > ?")
                .bind(now)
                .fetch_one(self.pool.as_ref())
                .await?;

        Ok(row.0)
    }

    /// Number of sessions belonging to a specific user.
    pub async fn count_by_username(&self, username: &str) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions WHERE username = ?")
            .bind(username)
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// Retrieve all sessions for a user (newest-first).
    pub async fn find_all_by_username(&self, username: &str) -> Result<Vec<Session>, DbError> {
        let sessions = sqlx::query_as::<_, SessionRow>(
            "SELECT token, username, ip_address, csrf_token, created_at, expires_at, is_admin
             FROM sessions
             WHERE username = ?
             ORDER BY created_at DESC",
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(sessions.into_iter().map(|s| s.into()).collect())
    }

    /// Delete multiple sessions by token in a single query.
    pub async fn delete_many(&self, tokens: &[String]) -> Result<u64, DbError> {
        if tokens.is_empty() {
            return Ok(0);
        }

        use sqlx::QueryBuilder;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM sessions WHERE token IN (");
        let mut separated = qb.separated(", ");
        for token in tokens {
            separated.push_bind(token);
        }
        separated.push_unseparated(")");

        let result = qb.build().execute(self.pool.as_ref()).await?;
        Ok(result.rows_affected())
    }
}
