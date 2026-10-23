/* =============================================================================
 * File          : notification_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Persisted notifications (the source of truth behind the bell
 *   panel and its 30 s poll) with **per-user read state** (ROADMAP 21.2):
 *   the notifications feed is broadcast (no per-user targeting, matching
 *   `NotificationSettings`/`EmailSettings`), but each account's read/unread
 *   view lives in the `notification_reads` join table (migration 058), so
 *   one user marking "all read" never clears another user's badge.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Persisted notifications + per-user read state.

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// A system notification (health alert, backup result, ...). Broadcast-style:
/// there is no per-user targeting, matching how `NotificationSettings` and
/// `EmailSettings` are already global rather than per-user. `read_at` is the
/// **caller's** read timestamp (resolved through the per-user join table),
/// not a global column.
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
            "INSERT INTO notifications (id, title, message, severity, category, source, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
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

    /// Lists notifications newest-first for one caller, resolving that
    /// caller's read timestamp (or `NULL` when unread for them).
    pub async fn list_recent(
        &self,
        username: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Notification>, sqlx::Error> {
        sqlx::query_as::<_, Notification>(
            "SELECT n.id, n.title, n.message, n.severity, n.category, n.source, n.created_at, r.read_at
             FROM notifications n
             LEFT JOIN notification_reads r
                ON r.notification_id = n.id AND r.username = ?
             ORDER BY n.created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(username)
        .bind(limit)
        .bind(offset)
        .fetch_all(self.pool.as_ref())
        .await
    }

    /// Total rows in the broadcast feed, for pagination metadata.
    pub async fn total_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(self.pool.as_ref())
            .await
    }

    /// Unread rows for one caller: notifications with no read row for them.
    pub async fn unread_count_for(&self, username: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications n
             WHERE NOT EXISTS (
                 SELECT 1 FROM notification_reads r
                 WHERE r.notification_id = n.id AND r.username = ?
             )",
        )
        .bind(username)
        .fetch_one(self.pool.as_ref())
        .await
    }

    /// Marks one notification as read **for the caller only**. Returns
    /// `false` when no such notification exists (the caller would otherwise
    /// be writing a read row for nothing).
    pub async fn mark_read(&self, username: &str, id: &str) -> Result<bool, sqlx::Error> {
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM notifications WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool.as_ref())
                .await?;
        if exists.is_none() {
            return Ok(false);
        }

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO notification_reads (notification_id, username, read_at) VALUES (?, ?, ?)
             ON CONFLICT(notification_id, username) DO UPDATE SET read_at = excluded.read_at",
        )
        .bind(id)
        .bind(username)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(true)
    }

    /// Marks every notification as read **for the caller only**.
    pub async fn mark_all_read(&self, username: &str) -> Result<u64, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO notification_reads (notification_id, username, read_at)
             SELECT id, ?, ? FROM notifications
             ON CONFLICT(notification_id, username) DO UPDATE SET read_at = excluded.read_at",
        )
        .bind(username)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected())
    }

    /// Deletes one notification and its read rows. Admin-only at the handler
    /// level: the feed is broadcast, so deleting an alert trail entry is a
    /// shared, destructive operation (ROADMAP 21.2). Returns `false` when no
    /// such notification exists.
    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Explicit read-row cleanup rather than relying on `ON DELETE
        // CASCADE` alone — deleting explicitly keeps the cleanup working
        // even if `PRAGMA foreign_keys` is ever off on a pool.
        sqlx::query("DELETE FROM notification_reads WHERE notification_id = ?")
            .bind(id)
            .execute(tx.as_mut())
            .await?;
        let result = sqlx::query("DELETE FROM notifications WHERE id = ?")
            .bind(id)
            .execute(tx.as_mut())
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    /// Deletes every notification (and with them every read row) in one
    /// operation — the "clear all" path (ROADMAP 21.3). Admin-only at the
    /// handler level, same rationale as `delete`.
    pub async fn clear_all(&self) -> Result<u64, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM notification_reads")
            .execute(tx.as_mut())
            .await?;
        let result = sqlx::query("DELETE FROM notifications")
            .execute(tx.as_mut())
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}
