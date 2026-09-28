use crate::storage::database::DbError;
use crate::impl_repository;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Favorite {
    pub id: i64,
    pub username: String,
    pub path: String,
    pub label: String,
    pub sort_order: i32,
    pub created_at: String,
}

#[derive(Clone)]
pub struct FavoriteRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(FavoriteRepository);

impl FavoriteRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self, username: &str) -> Result<Vec<Favorite>, DbError> {
        let rows = sqlx::query_as::<_, (i64, String, String, String, i32, String)>(
            "SELECT id, username, path, label, sort_order, created_at FROM explorer_favorites WHERE username = ? ORDER BY sort_order, created_at"
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(rows.into_iter().map(|(id, username, path, label, sort_order, created_at)| Favorite { id, username, path, label, sort_order, created_at }).collect())
    }

    pub async fn add(&self, username: &str, path: &str, label: &str) -> Result<Favorite, DbError> {
        // Find the max sort_order for this user
        let max_order: Option<i32> = sqlx::query_scalar(
            "SELECT MAX(sort_order) FROM explorer_favorites WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?
        .flatten();

        let next_order = max_order.unwrap_or(-1) + 1;

        sqlx::query(
            "INSERT OR IGNORE INTO explorer_favorites (username, path, label, sort_order) VALUES (?, ?, ?, ?)"
        )
        .bind(username)
        .bind(path)
        .bind(label)
        .bind(next_order)
        .execute(self.pool.as_ref())
        .await?;

        let row = sqlx::query_as::<_, (i64, String, String, String, i32, String)>(
            "SELECT id, username, path, label, sort_order, created_at FROM explorer_favorites WHERE username = ? AND path = ?"
        )
        .bind(username)
        .bind(path)
        .fetch_optional(self.pool.as_ref())
        .await?;

        row.map(|(id, username, path, label, sort_order, created_at)| Favorite { id, username, path, label, sort_order, created_at })
            .ok_or(DbError::NotFound)
    }

    pub async fn remove(&self, username: &str, path: &str) -> Result<(), DbError> {
        let result = sqlx::query(
            "DELETE FROM explorer_favorites WHERE username = ? AND path = ?"
        )
        .bind(username)
        .bind(path)
        .execute(self.pool.as_ref())
        .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    pub async fn contains(&self, username: &str, path: &str) -> Result<bool, DbError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM explorer_favorites WHERE username = ? AND path = ?"
        )
        .bind(username)
        .bind(path)
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(count > 0)
    }
}
