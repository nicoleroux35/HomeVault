use crate::storage::database::DbError;
use crate::storage::models::User;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Coordinates multi-table atomic operations within single transactions.
pub struct TransactionalService {
    pool: Arc<SqlitePool>,
}

impl TransactionalService {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Insert a user row together with an initial password-history entry in one transaction.
    pub async fn create_user_with_history(&self, user: &User) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(&user.fullname)
        .bind(&user.email)
        .bind(user.role_id)
        .bind(user.locked)
        .bind(user.created_at)
        .bind(&user.avatar_seed)
        .execute(tx.as_mut())
        .await?;

        // Record the password hash in the history table for reuse checks.
        sqlx::query(
            "INSERT INTO password_history (username, password_hash, created_at) VALUES (?, ?, ?)",
        )
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(user.created_at)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Update a user's password and archive the old hash in password_history.
    pub async fn update_password_with_history(
        &self,
        username: &str,
        new_password_hash: &str,
        old_password_hash: &str,
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        let now = chrono::Utc::now().to_rfc3339();

        // Archive the old password hash before overwriting it.
        sqlx::query(
            "INSERT INTO password_history (username, password_hash, created_at) VALUES (?, ?, ?)",
        )
        .bind(username)
        .bind(old_password_hash)
        .bind(&now)
        .execute(tx.as_mut())
        .await?;

        sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?")
            .bind(new_password_hash)
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Delete a user and all of their associated data (sessions, widgets, etc.) in one transaction.
    pub async fn delete_user_cascade(&self, username: &str) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM sessions WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        sqlx::query("DELETE FROM widgets WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        sqlx::query("DELETE FROM password_history WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        sqlx::query("DELETE FROM login_attempts WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        sqlx::query("DELETE FROM user_2fa WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        sqlx::query("DELETE FROM users WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Delete multiple users and all their associated data in one transaction.
    pub async fn delete_users_cascade_batch(&self, usernames: &[String]) -> Result<(), DbError> {
        if usernames.is_empty() {
            return Ok(());
        }

        use sqlx::QueryBuilder;

        let mut tx = self.pool.begin().await?;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM sessions WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");
        qb.build().execute(tx.as_mut()).await?;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM widgets WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");
        qb.build().execute(tx.as_mut()).await?;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM password_history WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");
        qb.build().execute(tx.as_mut()).await?;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM login_attempts WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");
        qb.build().execute(tx.as_mut()).await?;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM user_2fa WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");
        qb.build().execute(tx.as_mut()).await?;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM users WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");
        qb.build().execute(tx.as_mut()).await?;

        tx.commit().await?;
        Ok(())
    }

    /// Update a user's role and sync the `is_admin` flag on all active sessions.
    pub async fn update_user_role_with_sessions(
        &self,
        username: &str,
        new_role_id: i64,
        is_admin: bool,
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("UPDATE users SET role_id = ? WHERE username = ?")
            .bind(new_role_id)
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        // Keep sessions valid but update their cached admin flag.
        sqlx::query("UPDATE sessions SET is_admin = ? WHERE username = ?")
            .bind(is_admin)
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Lock a user account and invalidate all of their sessions.
    pub async fn lock_user_and_clear_sessions(&self, username: &str) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("UPDATE users SET locked = 1 WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        // Force-logout by deleting all sessions.
        sqlx::query("DELETE FROM sessions WHERE username = ?")
            .bind(username)
            .execute(tx.as_mut())
            .await?;

        tx.commit().await?;
        Ok(())
    }

    // `create_group_with_quota` / `delete_group_cascade` (which wrote the
    // `hv_quotas` table) were removed 2026-09-09 with the per-group
    // filesystem-quota feature — they had no callers.

    /// Purge expired sessions and old login attempts beyond the given age (maintenance task).
    pub async fn cleanup_old_data(
        &self,
        sessions_older_than_days: i64,
        login_attempts_older_than_days: i64,
    ) -> Result<(u64, u64), DbError> {
        let mut tx = self.pool.begin().await?;
        let now = chrono::Utc::now();

        let session_cutoff = now - chrono::Duration::days(sessions_older_than_days);
        let sessions_deleted = sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
            .bind(session_cutoff)
            .execute(tx.as_mut())
            .await?
            .rows_affected();

        let login_cutoff = now - chrono::Duration::days(login_attempts_older_than_days);
        let attempts_deleted = sqlx::query("DELETE FROM login_attempts WHERE attempted_at < ?")
            .bind(login_cutoff.to_rfc3339())
            .execute(tx.as_mut())
            .await?
            .rows_affected();

        tx.commit().await?;
        Ok((sessions_deleted, attempts_deleted))
    }
}
