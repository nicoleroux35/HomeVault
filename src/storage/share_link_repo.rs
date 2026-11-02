/* =============================================================================
 * File          : share_link_repo.rs
 * Description   : Persistent share-link access grants.
 * =============================================================================
 */

use crate::impl_repository;
use crate::storage::database::DbError;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ShareLink {
    pub id: i64,
    pub token: String,
    pub username: String,
    pub path: String,
    pub mode: String,
    pub expires_at: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub max_downloads: Option<i64>,
    pub download_count: i64,
    #[serde(skip_serializing)]
    pub active_downloads: i64,
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

    pub async fn list_by_user(&self, username: &str) -> Result<Vec<ShareLink>, DbError> {
        Ok(
            sqlx::query_as(
                "SELECT * FROM explorer_share_links WHERE username = ? ORDER BY id DESC",
            )
            .bind(username)
            .fetch_all(self.pool.as_ref())
            .await?,
        )
    }

    pub async fn find_by_user_path(
        &self,
        username: &str,
        path: &str,
    ) -> Result<Option<ShareLink>, DbError> {
        Ok(sqlx::query_as("SELECT * FROM explorer_share_links WHERE username = ? AND path = ? ORDER BY id DESC LIMIT 1")
            .bind(username).bind(path).fetch_optional(self.pool.as_ref()).await?)
    }

    pub async fn find_by_token(&self, token: &str) -> Result<Option<ShareLink>, DbError> {
        Ok(
            sqlx::query_as("SELECT * FROM explorer_share_links WHERE token = ?")
                .bind(token)
                .fetch_optional(self.pool.as_ref())
                .await?,
        )
    }

    pub async fn create(
        &self,
        username: &str,
        token: &str,
        path: &str,
        mode: &str,
        expires_at: Option<&str>,
        password_hash: Option<&str>,
        max_downloads: Option<i64>,
    ) -> Result<ShareLink, DbError> {
        sqlx::query("INSERT INTO explorer_share_links (token, username, path, mode, expires_at, password_hash, max_downloads) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(token).bind(username).bind(path).bind(mode).bind(expires_at).bind(password_hash).bind(max_downloads)
            .execute(self.pool.as_ref()).await?;
        self.find_by_token(token).await?.ok_or(DbError::NotFound)
    }

    /// Reserve atomically so concurrent downloads cannot exceed the cap.
    pub async fn reserve_download(&self, token: &str) -> Result<bool, DbError> {
        let result = sqlx::query("UPDATE explorer_share_links SET active_downloads = active_downloads + 1 WHERE token = ? AND (max_downloads IS NULL OR download_count + active_downloads < max_downloads)")
            .bind(token).execute(self.pool.as_ref()).await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn finish_download(&self, token: &str, completed: bool) -> Result<(), DbError> {
        let result = sqlx::query("UPDATE explorer_share_links SET active_downloads = MAX(active_downloads - 1, 0), download_count = download_count + ? WHERE token = ? AND active_downloads > 0")
            .bind(i64::from(completed)).bind(token).execute(self.pool.as_ref()).await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use crate::handlers::share_links::{compute_expiry, generate_share_token};

    #[test]
    fn test_generate_share_token_length_and_charset() {
        let token = generate_share_token();
        assert!(token.len() >= 32 && token.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(token, generate_share_token());
    }

    #[test]
    fn test_compute_expiry_none_for_zero() {
        assert_eq!(compute_expiry(0), None);
    }

    #[test]
    fn test_compute_expiry_positive_days_is_forward_dated() {
        let expiry = compute_expiry(7).unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(&expiry)
                .unwrap()
                .timestamp()
                > chrono::Utc::now().timestamp()
        );
    }
}
