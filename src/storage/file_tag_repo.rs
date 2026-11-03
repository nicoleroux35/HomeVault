//! Per-user tags for files and folders in Explorer.

use crate::impl_repository;
use crate::storage::database::DbError;
use sqlx::SqlitePool;
use std::sync::Arc;

const MAX_TAGS_PER_PATH: i64 = 20;
const MAX_TAG_LENGTH: usize = 32;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileTag {
    pub path: String,
    pub tag: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct FileTagRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(FileTagRepository);

impl FileTagRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub fn normalize_query_tag(tag: &str) -> Result<String, String> {
        Self::normalize_tag(tag).map_err(|e| e.to_string())
    }

    pub fn validate_tag(tag: &str) -> Result<(), String> {
        Self::normalize_query_tag(tag).map(|_| ())
    }

    fn normalize_tag(tag: &str) -> Result<String, DbError> {
        let tag = tag.trim();
        if tag.is_empty()
            || tag.chars().count() > MAX_TAG_LENGTH
            || tag.chars().any(|c| c.is_control())
            || tag.contains('/')
            || tag.contains('\\')
        {
            return Err(DbError::InvalidInput(format!(
                "Tag must contain 1 to {MAX_TAG_LENGTH} visible characters"
            )));
        }
        Ok(tag.to_string())
    }

    pub async fn list(&self, owner: &str, path: Option<&str>) -> Result<Vec<FileTag>, DbError> {
        let rows = if let Some(path) = path {
            sqlx::query_as::<_, (String, String, String)>(
                "SELECT path, tag, created_at FROM explorer_file_tags WHERE owner = ? AND path = ? ORDER BY tag COLLATE NOCASE",
            )
            .bind(owner)
            .bind(path)
            .fetch_all(self.pool.as_ref())
            .await?
        } else {
            sqlx::query_as::<_, (String, String, String)>(
                "SELECT path, tag, created_at FROM explorer_file_tags WHERE owner = ? ORDER BY tag COLLATE NOCASE, path",
            )
            .bind(owner)
            .fetch_all(self.pool.as_ref())
            .await?
        };
        Ok(rows
            .into_iter()
            .map(|(path, tag, created_at)| FileTag {
                path,
                tag,
                created_at,
            })
            .collect())
    }

    pub async fn add(&self, owner: &str, path: &str, tag: &str) -> Result<FileTag, DbError> {
        let tag = Self::normalize_tag(tag)?;
        let mut tx = self.pool.begin().await?;
        // Acquire SQLite's write reservation before counting, so concurrent
        // additions cannot both pass the per-path bound.
        sqlx::query("UPDATE explorer_file_tags SET tag = tag WHERE 0")
            .execute(tx.as_mut())
            .await?;
        let result = self.add_with_lock(owner, path, &tag, &mut tx).await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }

    async fn add_with_lock<'a>(
        &self,
        owner: &str,
        path: &str,
        tag: &str,
        tx: &mut sqlx::Transaction<'a, sqlx::Sqlite>,
    ) -> Result<FileTag, DbError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM explorer_file_tags WHERE owner = ? AND path = ?",
        )
        .bind(owner)
        .bind(path)
        .fetch_one(tx.as_mut())
        .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM explorer_file_tags WHERE owner = ? AND path = ? AND tag = ?)",
        )
        .bind(owner)
        .bind(path)
        .bind(tag)
        .fetch_one(tx.as_mut())
        .await?;
        if count >= MAX_TAGS_PER_PATH && !exists {
            return Err(DbError::InvalidInput(format!(
                "A path can have at most {MAX_TAGS_PER_PATH} tags"
            )));
        }
        sqlx::query("INSERT OR IGNORE INTO explorer_file_tags (owner, path, tag) VALUES (?, ?, ?)")
            .bind(owner)
            .bind(path)
            .bind(tag)
            .execute(tx.as_mut())
            .await?;
        let created_at: String = sqlx::query_scalar(
            "SELECT created_at FROM explorer_file_tags WHERE owner = ? AND path = ? AND tag = ?",
        )
        .bind(owner)
        .bind(path)
        .bind(tag)
        .fetch_one(tx.as_mut())
        .await?;
        Ok(FileTag {
            path: path.to_string(),
            tag: tag.to_string(),
            created_at,
        })
    }

    pub async fn remove(&self, owner: &str, path: &str, tag: &str) -> Result<(), DbError> {
        let tag = Self::normalize_tag(tag)?;
        let result =
            sqlx::query("DELETE FROM explorer_file_tags WHERE owner = ? AND path = ? AND tag = ?")
                .bind(owner)
                .bind(path)
                .bind(tag)
                .execute(self.pool.as_ref())
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    pub async fn paths_for_tag(
        &self,
        owner: &str,
        tag: &str,
    ) -> Result<std::collections::HashSet<String>, DbError> {
        let tag = Self::normalize_tag(tag)?;
        let paths = sqlx::query_scalar::<_, String>(
            "SELECT path FROM explorer_file_tags WHERE owner = ? AND tag = ?",
        )
        .bind(owner)
        .bind(tag)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(paths.into_iter().collect())
    }

    pub async fn purge_user(&self, owner: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM explorer_file_tags WHERE owner = ?")
            .bind(owner)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_tag_trims_and_preserves_case() {
        assert_eq!(
            FileTagRepository::normalize_query_tag("  Family ").as_deref(),
            Ok("Family")
        );
    }

    #[test]
    fn test_normalize_tag_rejects_empty_control_and_path_shapes() {
        for tag in ["", "  ", "../private", "a\\b", "bad\ntag"] {
            assert!(
                FileTagRepository::validate_tag(tag).is_err(),
                "accepted {tag:?}"
            );
        }
    }

    #[test]
    fn test_normalize_tag_limits_characters_not_bytes() {
        assert!(FileTagRepository::validate_tag(&"é".repeat(MAX_TAG_LENGTH)).is_ok());
        assert!(FileTagRepository::validate_tag(&"x".repeat(MAX_TAG_LENGTH + 1)).is_err());
    }
}
