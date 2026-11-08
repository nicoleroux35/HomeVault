/* =============================================================================
 * File          : file_request_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Persistent public file-request links ("ask someone to upload into this
 *   folder"). A request is a token, a destination folder, an optional
 *   password/expiry/upload cap, and an atomic completed-upload counter.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   handlers::file_requests; rows live in explorer_file_requests (migration
 *   085).
 * =============================================================================
 */

//! Repository for public file-request links.

use crate::impl_repository;
use crate::storage::database::DbError;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct FileRequest {
    pub id: i64,
    pub token: String,
    pub username: String,
    pub path: String,
    pub message: String,
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    pub expires_at: Option<String>,
    pub max_uploads: Option<i64>,
    pub upload_count: i64,
    #[serde(skip_serializing)]
    pub active_uploads: i64,
    pub created_at: String,
}

#[derive(Clone)]
pub struct FileRequestRepository {
    pool: Arc<SqlitePool>,
}
impl_repository!(FileRequestRepository);

impl FileRequestRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list_by_user(&self, username: &str) -> Result<Vec<FileRequest>, DbError> {
        Ok(sqlx::query_as(
            "SELECT * FROM explorer_file_requests WHERE username = ? ORDER BY id DESC",
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    pub async fn find_by_token(&self, token: &str) -> Result<Option<FileRequest>, DbError> {
        Ok(
            sqlx::query_as("SELECT * FROM explorer_file_requests WHERE token = ?")
                .bind(token)
                .fetch_optional(self.pool.as_ref())
                .await?,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        username: &str,
        token: &str,
        path: &str,
        message: &str,
        password_hash: Option<&str>,
        expires_at: Option<&str>,
        max_uploads: Option<i64>,
    ) -> Result<FileRequest, DbError> {
        sqlx::query(
            "INSERT INTO explorer_file_requests
                (token, username, path, message, password_hash, expires_at, max_uploads)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(token)
        .bind(username)
        .bind(path)
        .bind(message)
        .bind(password_hash)
        .bind(expires_at)
        .bind(max_uploads)
        .execute(self.pool.as_ref())
        .await?;
        self.find_by_token(token).await?.ok_or(DbError::NotFound)
    }

    /// Reserve one upload slot atomically. The count only rises on completion
    /// (`finish_upload`), so a failed/abandoned upload never spends the cap.
    pub async fn reserve_upload(&self, token: &str) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE explorer_file_requests
             SET active_uploads = active_uploads + 1
             WHERE token = ?
               AND (max_uploads IS NULL OR upload_count + active_uploads < max_uploads)",
        )
        .bind(token)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn finish_upload(&self, token: &str, completed: bool) -> Result<(), DbError> {
        let result = sqlx::query(
            "UPDATE explorer_file_requests
             SET active_uploads = MAX(active_uploads - 1, 0),
                 upload_count = upload_count + ?
             WHERE token = ? AND active_uploads > 0",
        )
        .bind(i64::from(completed))
        .bind(token)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    pub async fn delete(&self, token: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM explorer_file_requests WHERE token = ?")
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
            "DELETE FROM explorer_file_requests WHERE expires_at IS NOT NULL AND expires_at < ?",
        )
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected())
    }
}
