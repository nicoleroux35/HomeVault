//! Durable first-run setup state, stored alongside the account data.
//!
//! The existence of a row in `users` with the admin role is the **single source
//! of truth** for "the appliance has been configured". The previous design
//! derived that from a `.setup_complete` flag file under `base_dir()`, which
//! could drift out of sync with the database in both directions (flag present
//! but no admin, or admin present but flag lost). The `setup_state` singleton
//! row only carries what the account table cannot: the hardware fingerprint
//! recorded at setup time, used to detect a moved/reflashed system.
//!
//! See `sqlx/migrations/037_setup_state.sql` for the migration rationale.

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::storage::models::ADMIN_ROLE_ID;

/// The recorded hardware fingerprint of a completed setup.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SetupState {
    /// RFC 3339 timestamp of when setup was completed.
    pub completed_at: String,
    /// SHA-256 hardware fingerprint recorded at setup time.
    pub system_hash: String,
    /// Machine identifier (`/etc/machine-id`, or a hostname fallback).
    pub machine_id: String,
}

/// Repository for the singleton `setup_state` row.
#[derive(Clone)]
pub struct SetupStateRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SetupStateRepository);

impl SetupStateRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Whether at least one administrator account exists.
    ///
    /// This is the canonical "is the appliance configured?" check — an
    /// appliance with an admin is configured regardless of what any marker
    /// file on disk says.
    pub async fn has_admin(&self) -> Result<bool, sqlx::Error> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE role_id = ?")
            .bind(ADMIN_ROLE_ID)
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0 > 0)
    }

    /// Load the recorded setup state, if setup has been completed.
    pub async fn get(&self) -> Result<Option<SetupState>, sqlx::Error> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT completed_at, system_hash, machine_id FROM setup_state WHERE id = 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(
            row.map(|(completed_at, system_hash, machine_id)| SetupState {
                completed_at,
                system_hash,
                machine_id,
            }),
        )
    }

    /// Whether a completed-setup fingerprint has been recorded.
    pub async fn is_recorded(&self) -> Result<bool, sqlx::Error> {
        Ok(self.get().await?.is_some())
    }

    /// Record (or replace) the completed-setup fingerprint.
    pub async fn mark_complete(
        &self,
        system_hash: &str,
        machine_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO setup_state (id, completed_at, system_hash, machine_id)
             VALUES (1, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                 completed_at = excluded.completed_at,
                 system_hash = excluded.system_hash,
                 machine_id = excluded.machine_id",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(system_hash)
        .bind(machine_id)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Re-baseline the recorded fingerprint after an admin confirms a hardware
    /// change. Inserts the row when it is missing (e.g. a database restored
    /// from before migration 037) so the recovery flow can always complete.
    pub async fn update_system_hash(
        &self,
        system_hash: &str,
        machine_id: &str,
    ) -> Result<(), sqlx::Error> {
        self.mark_complete(system_hash, machine_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::repository::Repository;

    async fn repo() -> SetupStateRepository {
        let db = crate::storage::Database::new("sqlite::memory:")
            .await
            .expect("in-memory database");
        db.run_migrations().await.expect("migrations");
        SetupStateRepository::new(db.pool())
    }

    #[tokio::test]
    async fn test_has_admin_false_on_fresh_db() {
        let repo = repo().await;
        assert!(!repo.has_admin().await.unwrap());
    }

    #[tokio::test]
    async fn test_has_admin_true_after_admin_inserted() {
        let repo = repo().await;
        sqlx::query(
            "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at)
             VALUES ('boss', 'x', 'Boss', '', 2, 0, '2026-01-01T00:00:00Z')",
        )
        .execute(repo.pool().as_ref())
        .await
        .unwrap();

        assert!(repo.has_admin().await.unwrap());
    }

    #[tokio::test]
    async fn test_has_admin_ignores_regular_users() {
        let repo = repo().await;
        sqlx::query(
            "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at)
             VALUES ('guest', 'x', 'Guest', '', 1, 0, '2026-01-01T00:00:00Z')",
        )
        .execute(repo.pool().as_ref())
        .await
        .unwrap();

        assert!(!repo.has_admin().await.unwrap());
    }

    #[tokio::test]
    async fn test_mark_complete_round_trip() {
        let repo = repo().await;
        assert!(!repo.is_recorded().await.unwrap());

        repo.mark_complete("abc123", "machine-1").await.unwrap();

        let state = repo.get().await.unwrap().expect("recorded");
        assert_eq!(state.system_hash, "abc123");
        assert_eq!(state.machine_id, "machine-1");
        assert!(!state.completed_at.is_empty());
    }

    #[tokio::test]
    async fn test_mark_complete_is_an_upsert() {
        let repo = repo().await;
        repo.mark_complete("old", "machine-1").await.unwrap();
        repo.mark_complete("new", "machine-2").await.unwrap();

        let state = repo.get().await.unwrap().expect("recorded");
        assert_eq!(state.system_hash, "new");
        assert_eq!(state.machine_id, "machine-2");
    }

    #[tokio::test]
    async fn test_update_system_hash_inserts_missing_row() {
        let repo = repo().await;
        repo.update_system_hash("fresh", "machine-9").await.unwrap();

        let state = repo.get().await.unwrap().expect("inserted");
        assert_eq!(state.system_hash, "fresh");
    }
}
