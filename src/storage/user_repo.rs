/* =============================================================================
 * File          : user_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   User account records: credentials (Argon2 hash), role, lock
 *   state, avatar seed; create/find/update/delete/count.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! User account records (credentials, role, lock, avatar seed).

use crate::impl_repository;
use crate::storage::database::DbError;
use crate::storage::models::User;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Repository for CRUD operations on the `users` table.
#[derive(Clone)]
pub struct UserRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(UserRepository);

impl UserRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Look up a user by their unique username.
    pub async fn find_by_username(&self, username: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(
            "SELECT username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed, must_change_password, expires_at FROM users WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(user)
    }

    /// Insert a new user row.
    pub async fn create(&self, user: &User) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed, must_change_password, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(&user.fullname)
        .bind(&user.email)
        .bind(user.role_id)
        .bind(user.locked)
        .bind(user.created_at)
        .bind(&user.avatar_seed)
        .bind(user.must_change_password)
        .bind(user.expires_at)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Update all mutable fields of a user.
    pub async fn update(&self, user: &User) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE users SET password_hash = ?, fullname = ?, email = ?, role_id = ?, locked = ?, must_change_password = ? WHERE username = ?"
        )
        .bind(&user.password_hash)
        .bind(&user.fullname)
        .bind(&user.email)
        .bind(user.role_id)
        .bind(user.locked)
        .bind(user.must_change_password)
        .bind(&user.username)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Delete a user by username.
    pub async fn delete(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM users WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Total number of users.
    pub async fn count(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// Number of users with a specific role.
    pub async fn count_by_role(&self, role_id: i64) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE role_id = ?")
            .bind(role_id)
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// List all users.
    pub async fn find_all(&self) -> Result<Vec<User>, DbError> {
        let users = sqlx::query_as::<_, User>(
            "SELECT username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed, must_change_password, expires_at FROM users",
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(users)
    }

    /// List users with pagination (newest-first).
    pub async fn find_all_paginated(&self, limit: i64, offset: i64) -> Result<Vec<User>, DbError> {
        let users = sqlx::query_as::<_, User>(
            "SELECT username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed, must_change_password, expires_at
             FROM users
             ORDER BY created_at DESC
             LIMIT ? OFFSET ?"
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(users)
    }

    /// Update only the role_id of a user.
    pub async fn update_role(&self, username: &str, role_id: i64) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET role_id = ? WHERE username = ?")
            .bind(role_id)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Set (or clear with `None` = never) the admin expiry horizon. Deliberately
    /// not part of [`Self::update`] — profile saves must never shift the horizon
    /// as a side effect, only this endpoint may move it.
    pub async fn update_expires_at(
        &self,
        username: &str,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET expires_at = ? WHERE username = ?")
            .bind(expires_at)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Update only the locked status of a user.
    pub async fn update_locked_status(&self, username: &str, locked: bool) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET locked = ? WHERE username = ?")
            .bind(locked)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Delete multiple users by username in a single query.
    pub async fn delete_many(&self, usernames: &[String]) -> Result<u64, DbError> {
        if usernames.is_empty() {
            return Ok(0);
        }

        use sqlx::QueryBuilder;

        let mut qb: QueryBuilder<sqlx::Sqlite> =
            QueryBuilder::new("DELETE FROM users WHERE username IN (");
        let mut separated = qb.separated(", ");
        for username in usernames {
            separated.push_bind(username);
        }
        separated.push_unseparated(")");

        let result = qb.build().execute(self.pool.as_ref()).await?;
        Ok(result.rows_affected())
    }

    /// Look up a user by their email address.
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>, DbError> {
        let user = sqlx::query_as::<_, User>(
            "SELECT username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed, must_change_password, expires_at FROM users WHERE email = ?"
        )
        .bind(email)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(user)
    }

    /// Check whether a given username is already taken.
    pub async fn exists(&self, username: &str) -> Result<bool, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE username = ?")
            .bind(username)
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0 > 0)
    }

    /// Update the DiceBear avatar seed for deterministic avatar generation.
    pub async fn update_avatar_seed(
        &self,
        username: &str,
        avatar_seed: &str,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET avatar_seed = ? WHERE username = ?")
            .bind(avatar_seed)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }
}
