//! Explorer "share by link" repository: one row per public download link a
//! user created from the File Explorer (`/share/{token}`).
//!
//! A share link is an access grant: it names the file, who created it, how
//! recipients authenticate (`anyone` / `signed_in`) and when it stops
//! working (`expires_at`). It is personal data, so listing/creating/revoking
//! is always scoped to the owning user (admins additionally see/revoke all).

use crate::impl_repository;
use crate::storage::database::DbError;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShareLink {
    pub id: i64,
    pub token: String,
    pub username: String,
    pub path: String,
    pub mode: String,
    /// RFC-3339 UTC, or `None` for a link that never expires.
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Clone)]
pub struct ShareLinkRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(ShareLinkRepository);

impl ShareLinkRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Every share link created by one user, newest first.
    pub async fn list_by_user(&self, username: &str) -> Result<Vec<ShareLink>, DbError> {
        let rows =
            sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String)>(
                "SELECT id, token, username, path, mode, expires_at, created_at
             FROM explorer_share_links WHERE username = ? ORDER BY id DESC",
            )
            .bind(username)
            .fetch_all(self.pool.as_ref())
            .await?;
        Ok(rows.into_iter().map(share_link_from_row).collect())
    }

    /// The share link for one file created by one user, if any.
    pub async fn find_by_user_path(
        &self,
        username: &str,
        path: &str,
    ) -> Result<Option<ShareLink>, DbError> {
        let row =
            sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String)>(
                "SELECT id, token, username, path, mode, expires_at, created_at
             FROM explorer_share_links WHERE username = ? AND path = ? ORDER BY id DESC LIMIT 1",
            )
            .bind(username)
            .bind(path)
            .fetch_optional(self.pool.as_ref())
            .await?;
        Ok(row.map(share_link_from_row))
    }

    /// Look a link up by its public token (used by the unauthenticated
    /// `/share/{token}` download). `None` for an unknown token.
    pub async fn find_by_token(&self, token: &str) -> Result<Option<ShareLink>, DbError> {
        let row =
            sqlx::query_as::<_, (i64, String, String, String, String, Option<String>, String)>(
                "SELECT id, token, username, path, mode, expires_at, created_at
             FROM explorer_share_links WHERE token = ?",
            )
            .bind(token)
            .fetch_optional(self.pool.as_ref())
            .await?;
        Ok(row.map(share_link_from_row))
    }

    pub async fn create(
        &self,
        username: &str,
        token: &str,
        path: &str,
        mode: &str,
        expires_at: Option<&str>,
    ) -> Result<ShareLink, DbError> {
        sqlx::query(
            "INSERT INTO explorer_share_links (token, username, path, mode, expires_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(token)
        .bind(username)
        .bind(path)
        .bind(mode)
        .bind(expires_at)
        .execute(self.pool.as_ref())
        .await?;

        self.find_by_token(token).await?.ok_or(DbError::NotFound)
    }

    /// Revoke one link. Returns `NotFound` when the link doesn't exist.
    pub async fn delete(&self, token: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM explorer_share_links WHERE token = ?")
            .bind(token)
            .execute(self.pool.as_ref())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Purge links whose expiry has passed (callable on demand; the public
    /// download already refuses expired tokens, this just keeps the table tidy).
    pub async fn delete_expired(&self, now: &str) -> Result<u64, DbError> {
        let result = sqlx::query(
            "DELETE FROM explorer_share_links WHERE expires_at IS NOT NULL AND expires_at < ?",
        )
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected())
    }
}

fn share_link_from_row(
    (id, token, username, path, mode, expires_at, created_at): (
        i64,
        String,
        String,
        String,
        String,
        Option<String>,
        String,
    ),
) -> ShareLink {
    ShareLink {
        id,
        token,
        username,
        path,
        mode,
        expires_at,
        created_at,
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic unit tests only (no DB): the SQL-backed behaviour is
    //! exercised end to end in `tests/share_links_integration.rs`.

    #[test]
    fn test_generate_share_token_length_and_charset() {
        let token = crate::handlers::share_links::generate_share_token();
        assert!(
            token.len() >= 32,
            "token must be at least 32 chars, got {}",
            token.len()
        );
        assert!(
            token.chars().all(|c| c.is_ascii_alphanumeric()),
            "token must be URL-safe alphanumeric"
        );
        // Two draws must not collide.
        assert_ne!(token, crate::handlers::share_links::generate_share_token());
    }

    #[test]
    fn test_compute_expiry_none_for_zero() {
        assert_eq!(crate::handlers::share_links::compute_expiry(0), None);
    }

    #[test]
    fn test_compute_expiry_positive_days_is_forward_dated() {
        let expiry = crate::handlers::share_links::compute_expiry(7)
            .expect("7-day link must have an expiry");
        let parsed = chrono::DateTime::parse_from_rfc3339(&expiry).expect("valid RFC3339");
        assert!(parsed.timestamp() > chrono::Utc::now().timestamp());
    }
}
