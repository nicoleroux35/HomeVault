use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// A system notification (health alert, backup result, ...). Broadcast-style:
/// there is no per-user targeting, matching how `NotificationSettings` and
/// `EmailSettings` are already global rather than per-user.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub message: String,
    pub severity: String,
    pub category: String,
    pub source: String,
    pub created_at: String,
    pub read_at: Option<String>,
}

pub struct NotificationRepository {
    pool: Arc<SqlitePool>,
}

impl NotificationRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        title: &str,
        message: &str,
        severity: &str,
        category: &str,
        source: &str,
    ) -> Result<Notification, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO notifications (id, title, message, severity, category, source, created_at, read_at) VALUES (?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&id)
        .bind(title)
        .bind(message)
        .bind(severity)
        .bind(category)
        .bind(source)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        let notification = Notification {
            id,
            title: title.to_string(),
            message: message.to_string(),
            severity: severity.to_string(),
            category: category.to_string(),
            source: source.to_string(),
            created_at: now,
            read_at: None,
        };

        // Fan out live to any connected `/api/events` client. Hooked in
        // here rather than at each call site so every current and future
        // notification source gets real-time push automatically.
        crate::sse::notifications::publish(&notification);

        Ok(notification)
    }

    pub async fn list_recent(&self, limit: i64) -> Result<Vec<Notification>, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            "SELECT id, title, message, severity, category, source, created_at, read_at FROM notifications ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn unread_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE read_at IS NULL")
            .fetch_one(self.pool.as_ref())
            .await
    }

    pub async fn mark_read(&self, id: &str) -> Result<bool, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query("UPDATE notifications SET read_at = ? WHERE id = ? AND read_at IS NULL")
            .bind(&now)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_all_read(&self) -> Result<u64, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query("UPDATE notifications SET read_at = ? WHERE read_at IS NULL")
            .bind(&now)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM notifications WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
