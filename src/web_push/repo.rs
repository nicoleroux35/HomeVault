/* =============================================================================
 * File          : repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Persistence for Web Push subscriptions. One row per device a user
 *   explicitly subscribed; the endpoint and keys are never returned by the
 *   API. Delivery outcomes are recorded here so an expired device is removed
 *   instead of being retried forever.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), uuid, chrono
 *
 * Usage:
 *   handlers/web_push.rs (the user's own devices) and
 *   crate::web_push::dispatch (the sender).
 * ============================================================================= */

//! SQLite persistence for [`crate::web_push::PushSubscription`].

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// One stored subscription. `p256dh`/`auth` are secrets: they stay inside the
/// sender and are never serialised by the handler.
#[derive(Debug, Clone)]
pub struct PushSubscription {
    pub id: String,
    pub username: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub device_label: String,
    pub created_at: String,
    pub last_success_at: Option<String>,
    pub last_failure_at: Option<String>,
    pub failure_count: i64,
}

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
);

fn row_to_subscription(row: Row) -> PushSubscription {
    PushSubscription {
        id: row.0,
        username: row.1,
        endpoint: row.2,
        p256dh: row.3,
        auth: row.4,
        device_label: row.5,
        created_at: row.6,
        last_success_at: row.7,
        last_failure_at: row.8,
        failure_count: row.9,
    }
}

pub struct PushSubscriptionRepository {
    pool: Arc<SqlitePool>,
}

impl PushSubscriptionRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// The caller's devices, newest first.
    pub async fn list_for_user(
        &self,
        username: &str,
    ) -> Result<Vec<PushSubscription>, sqlx::Error> {
        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, username, endpoint, p256dh, auth, device_label, created_at, \
                    last_success_at, last_failure_at, failure_count \
             FROM push_subscriptions WHERE username = ? ORDER BY created_at DESC",
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(row_to_subscription).collect())
    }

    /// Every device, for the sender. A home instance has a handful per user.
    pub async fn list_all(&self) -> Result<Vec<PushSubscription>, sqlx::Error> {
        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, username, endpoint, p256dh, auth, device_label, created_at, \
                    last_success_at, last_failure_at, failure_count \
             FROM push_subscriptions ORDER BY created_at",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(row_to_subscription).collect())
    }

    pub async fn count_for_user(&self, username: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM push_subscriptions WHERE username = ?")
            .bind(username)
            .fetch_one(self.pool.as_ref())
            .await
    }

    /// Whether this exact endpoint is already registered for the user (a
    /// re-subscribe must not be refused by the device cap).
    pub async fn endpoint_registered(
        &self,
        username: &str,
        endpoint: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM push_subscriptions WHERE username = ? AND endpoint = ?)",
        )
        .bind(username)
        .bind(endpoint)
        .fetch_one(self.pool.as_ref())
        .await
    }

    /// Insert or refresh a device. A re-subscribe replaces the keys, clears
    /// the failure counter and keeps the original id.
    pub async fn upsert(
        &self,
        username: &str,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        device_label: &str,
    ) -> Result<PushSubscription, sqlx::Error> {
        sqlx::query(
            "INSERT INTO push_subscriptions (id, username, endpoint, p256dh, auth, device_label) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(username, endpoint) DO UPDATE SET \
                p256dh = excluded.p256dh, \
                auth = excluded.auth, \
                device_label = excluded.device_label, \
                failure_count = 0, \
                last_failure_at = NULL",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(username)
        .bind(endpoint)
        .bind(p256dh)
        .bind(auth)
        .bind(device_label)
        .execute(self.pool.as_ref())
        .await?;

        let row = sqlx::query_as::<_, Row>(
            "SELECT id, username, endpoint, p256dh, auth, device_label, created_at, \
                    last_success_at, last_failure_at, failure_count \
             FROM push_subscriptions WHERE username = ? AND endpoint = ?",
        )
        .bind(username)
        .bind(endpoint)
        .fetch_one(self.pool.as_ref())
        .await?;
        Ok(row_to_subscription(row))
    }

    /// Remove one of **this user's** devices. A foreign id is `false`, so the
    /// caller can answer `404` without disclosing that it exists.
    pub async fn delete_for_user(&self, username: &str, id: &str) -> Result<bool, sqlx::Error> {
        let affected = sqlx::query("DELETE FROM push_subscriptions WHERE id = ? AND username = ?")
            .bind(id)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Remove one device by id (the sender's cleanup path).
    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let affected = sqlx::query("DELETE FROM push_subscriptions WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn record_success(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE push_subscriptions SET last_success_at = ?, failure_count = 0, \
                    last_failure_at = NULL WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Record a failed delivery and return the new consecutive failure count.
    pub async fn record_failure(&self, id: &str) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "UPDATE push_subscriptions SET last_failure_at = ?, \
                    failure_count = failure_count + 1 WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        sqlx::query_scalar("SELECT failure_count FROM push_subscriptions WHERE id = ?")
            .bind(id)
            .fetch_one(self.pool.as_ref())
            .await
    }
}
