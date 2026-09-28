use crate::storage::database::DbError;
use crate::impl_repository;
use chrono::Utc;
use rand::RngExt;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Database row from the `user_2fa` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User2faRow {
    pub username: String,
    pub totp_secret_encrypted: String,
    pub enabled: bool,
    pub backup_codes: Option<String>,
    pub created_at: String,
}

/// A user's two-factor authentication configuration.
#[derive(Debug, Clone)]
pub struct User2fa {
    pub username: String,
    /// AES-encrypted TOTP secret (decrypted at use time).
    pub totp_secret_encrypted: String,
    pub enabled: bool,
    /// JSON array of one-time backup codes.
    pub backup_codes: Option<String>,
    pub created_at: String,
}

impl From<User2faRow> for User2fa {
    fn from(row: User2faRow) -> Self {
        Self {
            username: row.username,
            totp_secret_encrypted: row.totp_secret_encrypted,
            enabled: row.enabled,
            backup_codes: row.backup_codes,
            created_at: row.created_at,
        }
    }
}

/// Repository for two-factor authentication settings.
#[derive(Clone)]
pub struct User2faRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(User2faRepository);

impl User2faRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Load 2FA configuration for a user.
    pub async fn get_by_username(&self, username: &str) -> Result<Option<User2fa>, DbError> {
        let row = sqlx::query_as::<_, User2faRow>(
            "SELECT username, totp_secret_encrypted, enabled, backup_codes, created_at FROM user_2fa WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(row.map(User2fa::from))
    }

    /// Insert or update 2FA configuration for a user (enabled = 1).
    pub async fn save(
        &self,
        username: &str,
        totp_secret_encrypted: &str,
        backup_codes: &str,
    ) -> Result<(), DbError> {
        let now = Utc::now();
        sqlx::query(
            "INSERT OR REPLACE INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at) VALUES (?, ?, 1, ?, ?)"
        )
        .bind(username)
        .bind(totp_secret_encrypted)
        .bind(backup_codes)
        .bind(now.to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Mark 2FA as enabled for a user.
    pub async fn enable(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE user_2fa SET enabled = 1 WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Store a freshly generated (not yet enabled) TOTP secret as the pending
    /// enrollment. The previously generated secret, if any, is replaced so the
    /// subsequent verification step validates against the secret shown to the user.
    pub async fn save_pending(
        &self,
        username: &str,
        totp_secret: &str,
    ) -> Result<(), DbError> {
        let now = Utc::now();
        sqlx::query(
            "INSERT OR REPLACE INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at) VALUES (?, ?, 0, '', ?)"
        )
        .bind(username)
        .bind(totp_secret)
        .bind(now.to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// Disable 2FA for a user.
    pub async fn disable(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE user_2fa SET enabled = 0 WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    /// Remove all 2FA data for a user.
    pub async fn delete(&self, username: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM user_2fa WHERE username = ?")
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(())
    }

    pub fn generate_backup_codes(count: usize) -> Vec<String> {
        let mut rng = rand::rng();
        (0..count)
            .map(|_| {
                let code: String = (0..8)
                    .map(|_| {
                        let idx = rng.random_range(0..36);
                        if idx < 10 {
                            (b'0' + idx) as char
                        } else {
                            (b'a' + idx - 10) as char
                        }
                    })
                    .collect();
                code
            })
            .collect()
    }

    pub async fn mark_backup_code_used(&self, username: &str, code: &str) -> Result<bool, DbError> {
        let user_2fa = self.get_by_username(username).await?;

        if let Some(user_2fa) = user_2fa {
            if let Some(codes_json) = user_2fa.backup_codes {
                let mut codes: Vec<String> = serde_json::from_str(&codes_json).unwrap_or_default();

                if let Some(pos) = codes.iter().position(|c| c == code) {
                    codes.remove(pos);
                    let new_json = serde_json::to_string(&codes).unwrap_or_default();

                    sqlx::query("UPDATE user_2fa SET backup_codes = ? WHERE username = ?")
                        .bind(&new_json)
                        .bind(username)
                        .execute(self.pool.as_ref())
                        .await?;

                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Count users who have 2FA enabled.
    pub async fn count_enabled(&self) -> Result<i64, DbError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_2fa WHERE enabled = 1")
            .fetch_one(self.pool.as_ref())
            .await?;

        Ok(row.0)
    }

    /// List all usernames that have 2FA enabled.
    pub async fn list_enabled_users(&self) -> Result<Vec<String>, DbError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT username FROM user_2fa WHERE enabled = 1 ORDER BY username"
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Check whether a specific user has 2FA enabled.
    pub async fn is_enabled(&self, username: &str) -> Result<bool, DbError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM user_2fa WHERE username = ? AND enabled = 1"
        )
        .bind(username)
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(row.0 > 0)
    }

    /// Count how many unused backup codes a user still has.
    pub async fn count_remaining_backup_codes(&self, username: &str) -> Result<usize, DbError> {
        let user_2fa = self.get_by_username(username).await?;

        if let Some(user_2fa) = user_2fa {
            if let Some(codes_json) = user_2fa.backup_codes {
                let codes: Vec<String> = serde_json::from_str(&codes_json).unwrap_or_default();
                return Ok(codes.len());
            }
        }

        Ok(0)
    }

    /// Generate new backup codes and persist them for the user.
    pub async fn regenerate_backup_codes(&self, username: &str, count: usize) -> Result<Vec<String>, DbError> {
        let codes = Self::generate_backup_codes(count);
        let codes_json = serde_json::to_string(&codes).unwrap_or_default();

        sqlx::query("UPDATE user_2fa SET backup_codes = ? WHERE username = ?")
            .bind(&codes_json)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;

        Ok(codes)
    }
}
