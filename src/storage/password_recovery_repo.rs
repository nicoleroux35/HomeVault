use chrono::Utc;
use std::sync::Arc;

/// Repository for per-user password recovery codes.
///
/// Only the Argon2 hash of the code is ever persisted (see
/// `sqlx/migrations/028_password_recovery_codes.sql`). The plaintext code is
/// shown to the user exactly once, at generation time.
pub struct PasswordRecoveryCodeRepository {
    pool: Arc<sqlx::SqlitePool>,
}

impl PasswordRecoveryCodeRepository {
    pub fn new(pool: Arc<sqlx::SqlitePool>) -> Self {
        Self { pool }
    }

    /// Store (or replace) the recovery-code hash for a user.
    pub async fn set(&self, username: &str, code_hash: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO password_recovery_codes (username, code_hash, created_at)
             VALUES (?, ?, ?)
             ON CONFLICT(username) DO UPDATE SET
                 code_hash = excluded.code_hash,
                 created_at = excluded.created_at",
        )
        .bind(username)
        .bind(code_hash)
        .bind(Utc::now().to_rfc3339())
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    /// Return the stored recovery-code hash for a user, if one is set.
    pub async fn get_hash(&self, username: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT code_hash FROM password_recovery_codes WHERE username = ?")
                .bind(username)
                .fetch_optional(&*self.pool)
                .await?;

        Ok(row.map(|r| r.0))
    }

    /// Whether the user currently has a recovery code set.
    pub async fn exists(&self, username: &str) -> Result<bool, sqlx::Error> {
        Ok(self.get_hash(username).await?.is_some())
    }

    /// Remove the user's recovery code.
    pub async fn delete(&self, username: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM password_recovery_codes WHERE username = ?")
            .bind(username)
            .execute(&*self.pool)
            .await?;

        Ok(())
    }
}
