/* =============================================================================
 * File          : passkey_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for passkeys (WebAuthn credentials) and the short-lived challenges
 *   a ceremony is bound to. Nothing secret is stored: a credential row holds a
 *   public key, and a challenge row holds the challenge the server itself
 *   generated (single use, swept when it expires).
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   src/handlers/passkey.rs; see docs/guides/identity.md.
 * =============================================================================
 */

//! Passkey credential and challenge storage (`passkey_credentials`,
//! `passkey_challenges`, migration 067).

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::storage::database::DbError;

/// One registered passkey, as stored.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PasskeyRow {
    pub credential_id: String,
    pub username: String,
    pub label: String,
    pub algorithm: String,
    pub public_key: String,
    pub sign_count: i64,
    pub attestation_fmt: String,
    pub aaguid: Option<String>,
    pub user_verified: i64,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// A row as the API answers it — no key material, because a client never needs
/// one and sending it would be one more place it can leak.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PasskeySummary {
    pub credential_id: String,
    pub label: String,
    pub algorithm: String,
    pub attestation_fmt: String,
    pub aaguid: Option<String>,
    pub user_verified: bool,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

impl From<PasskeyRow> for PasskeySummary {
    fn from(row: PasskeyRow) -> Self {
        Self {
            credential_id: row.credential_id,
            label: row.label,
            algorithm: row.algorithm,
            attestation_fmt: row.attestation_fmt,
            aaguid: row.aaguid,
            user_verified: row.user_verified != 0,
            created_at: row.created_at,
            last_used_at: row.last_used_at,
        }
    }
}

/// What registering a credential writes.
pub struct NewPasskey<'a> {
    pub credential_id: &'a str,
    pub username: &'a str,
    pub label: &'a str,
    pub algorithm: &'a str,
    pub public_key: &'a str,
    pub sign_count: i64,
    pub attestation_fmt: &'a str,
    pub aaguid: Option<&'a str>,
    pub user_verified: bool,
}

#[derive(Clone)]
pub struct PasskeyRepository {
    pool: Arc<SqlitePool>,
}

impl PasskeyRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Every passkey an account holds, oldest first.
    pub async fn list_for_user(&self, username: &str) -> Result<Vec<PasskeyRow>, DbError> {
        let rows = sqlx::query_as::<_, PasskeyRow>(
            "SELECT credential_id, username, label, algorithm, public_key, sign_count,
                    attestation_fmt, aaguid, user_verified, created_at, last_used_at
             FROM passkey_credentials
             WHERE username = ?
             ORDER BY created_at ASC",
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows)
    }

    /// How many passkeys an account holds (for the per-user cap).
    pub async fn count_for_user(&self, username: &str) -> Result<i64, DbError> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM passkey_credentials WHERE username = ?")
                .bind(username)
                .fetch_one(self.pool.as_ref())
                .await?;
        Ok(row.0)
    }

    /// One credential by its id, whatever account owns it — the sign-in path
    /// needs to find the owner from the credential the browser named.
    pub async fn find(&self, credential_id: &str) -> Result<Option<PasskeyRow>, DbError> {
        let row = sqlx::query_as::<_, PasskeyRow>(
            "SELECT credential_id, username, label, algorithm, public_key, sign_count,
                    attestation_fmt, aaguid, user_verified, created_at, last_used_at
             FROM passkey_credentials
             WHERE credential_id = ?",
        )
        .bind(credential_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row)
    }

    /// Credential ids for one account, for `allowCredentials` (an empty list
    /// means "a discoverable credential may pick any of them").
    pub async fn ids_for_user(&self, username: &str) -> Result<Vec<String>, DbError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT credential_id FROM passkey_credentials WHERE username = ? ORDER BY created_at",
        )
        .bind(username)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Insert a credential. A duplicate id is refused by the primary key, which
    /// is the correct outcome: a credential id is unique per authenticator.
    pub async fn insert(&self, new: &NewPasskey<'_>) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO passkey_credentials
                (credential_id, username, label, algorithm, public_key, sign_count,
                 attestation_fmt, aaguid, user_verified, created_at, last_used_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(new.credential_id)
        .bind(new.username)
        .bind(new.label)
        .bind(new.algorithm)
        .bind(new.public_key)
        .bind(new.sign_count)
        .bind(new.attestation_fmt)
        .bind(new.aaguid)
        .bind(if new.user_verified { 1i64 } else { 0 })
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Record a successful assertion: the counter and the last-used stamp.
    pub async fn record_use(&self, credential_id: &str, sign_count: i64) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE passkey_credentials SET sign_count = ?, last_used_at = ?
             WHERE credential_id = ?",
        )
        .bind(sign_count)
        .bind(Utc::now().to_rfc3339())
        .bind(credential_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Delete one credential the account owns. Answers whether a row went —
    /// a delete that matched nothing is a `404`, not a silent success.
    pub async fn delete_for_user(
        &self,
        username: &str,
        credential_id: &str,
    ) -> Result<bool, DbError> {
        let result =
            sqlx::query("DELETE FROM passkey_credentials WHERE credential_id = ? AND username = ?")
                .bind(credential_id)
                .bind(username)
                .execute(self.pool.as_ref())
                .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Rename one credential the account owns.
    pub async fn rename_for_user(
        &self,
        username: &str,
        credential_id: &str,
        label: &str,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE passkey_credentials SET label = ? WHERE credential_id = ? AND username = ?",
        )
        .bind(label)
        .bind(credential_id)
        .bind(username)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    // ----------------------------------------------------------------------
    // Challenges
    // ----------------------------------------------------------------------

    /// Store (or replace) the outstanding challenge for one ceremony. The
    /// `(username, ceremony)` primary key is what makes a half-finished
    /// registration impossible to replay: starting again overwrites it.
    pub async fn store_challenge(
        &self,
        username: &str,
        ceremony: &str,
        challenge: &str,
        ttl_seconds: i64,
    ) -> Result<(), DbError> {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO passkey_challenges (username, ceremony, challenge, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(username, ceremony) DO UPDATE SET
                challenge = excluded.challenge,
                created_at = excluded.created_at,
                expires_at = excluded.expires_at",
        )
        .bind(username)
        .bind(ceremony)
        .bind(challenge)
        .bind(now.to_rfc3339())
        .bind((now + Duration::seconds(ttl_seconds)).to_rfc3339())
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Take (and consume) the outstanding challenge for a ceremony.
    ///
    /// **Single use by construction**: the row is deleted in the same
    /// transaction that reads it, so two concurrent ceremonies cannot both
    /// consume it. An expired row is deleted and answered as `None` — a stale
    /// challenge is indistinguishable from no challenge, which is what the
    /// caller needs (both mean "start again").
    pub async fn take_challenge(
        &self,
        username: &str,
        ceremony: &str,
    ) -> Result<Option<String>, DbError> {
        let row: Option<(String, String)> = sqlx::query_as(
            "DELETE FROM passkey_challenges
             WHERE username = ? AND ceremony = ?
             RETURNING challenge, expires_at",
        )
        .bind(username)
        .bind(ceremony)
        .fetch_optional(self.pool.as_ref())
        .await?;

        let Some((challenge, expires_at)) = row else {
            return Ok(None);
        };
        let Ok(expires) = DateTime::parse_from_rfc3339(&expires_at) else {
            return Ok(None);
        };
        if expires < Utc::now() {
            return Ok(None);
        }
        Ok(Some(challenge))
    }

    /// Drop every expired challenge. Runs on the sign-in path and from the
    /// sweep, so a challenge table can never grow without bound.
    pub async fn sweep_expired(&self) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM passkey_challenges WHERE expires_at < ?")
            .bind(Utc::now().to_rfc3339())
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

    /// Drop a pending challenge (a refused ceremony must not leave one alive).
    pub async fn clear_challenge(&self, username: &str, ceremony: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM passkey_challenges WHERE username = ? AND ceremony = ?")
            .bind(username)
            .bind(ceremony)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Every passkey an account holds, as the API answers them.
    pub async fn summaries_for_user(&self, username: &str) -> Result<Vec<PasskeySummary>, DbError> {
        Ok(self
            .list_for_user(username)
            .await?
            .into_iter()
            .map(PasskeySummary::from)
            .collect())
    }
}
