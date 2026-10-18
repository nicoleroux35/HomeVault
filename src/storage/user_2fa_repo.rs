/* =============================================================================
 * File          : user_2fa_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 18/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Two-factor credentials per user: the TOTP secret is stored
 *   AES-256-GCM-encrypted at rest (key outside the database), plus the
 *   one-time backup codes and the enable/disable state. Rows written by
 *   pre-encryption versions hold the raw base32 secret and are re-encrypted
 *   in place on first read.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *   - aes-gcm (>= 0.10)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Two-factor credentials (TOTP secret encrypted at rest, backup codes).
//!
//! `User2fa.totp_secret` is the **plaintext base32** secret, in memory only.
//! What the `totp_secret_encrypted` column holds is an `hv1:`-prefixed
//! AES-256-GCM blob — see [`encrypt_totp_secret`]. The field/column name
//! matched the documentation's claim before 2026-10-18; now it matches
//! reality.

use crate::impl_repository;
use crate::storage::database::DbError;
use chrono::Utc;
use rand::RngExt;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Database row from the `user_2fa` table — exactly what is stored, so the
/// secret field here is the encrypted blob, never the plaintext.
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
    /// Plaintext base32 TOTP secret — **in memory only**. The database row
    /// stores it AES-256-GCM-encrypted (see [`User2faRow`]).
    pub totp_secret: String,
    pub enabled: bool,
    /// JSON array of one-time backup codes.
    pub backup_codes: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// TOTP secret encryption at rest
// ---------------------------------------------------------------------------

/// Prefix marking an encrypted secret blob: `hv1:<base64(nonce ‖ ciphertext ‖ tag)>`.
/// Rows written before 2026-10-18 hold the raw base32 secret with no prefix;
/// they are read transparently and re-encrypted in place on first use.
const ENCRYPTED_PREFIX: &str = "hv1:";

/// Path of the AES-256 key file: `{DATA_DIRECTORY}/data/2fa.key`, mode 0600.
///
/// The key deliberately lives OUTSIDE the database. The realistic threat for a
/// NAS is a leak of `homevault.db` alone — a `VACUUM INTO` snapshot copied
/// offsite (see `db_backup.rs`), a SQLite browser pointed at the file — and
/// that snapshot must not carry the TOTP secrets. An attacker holding the
/// whole data directory gets the key too; that is a host compromise, out of
/// scope for at-rest encryption.
#[cfg(test)]
static TEST_KEY_PATH: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

fn totp_key_path() -> std::path::PathBuf {
    // Test seam: the env var is process-global and other test modules
    // (health, …) mutate `DATA_DIRECTORY` concurrently — unit tests pin the
    // key path here instead. Compiled out of release builds.
    #[cfg(test)]
    if let Some(path) = TEST_KEY_PATH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return path;
    }
    crate::config::paths::data_dir().join("2fa.key")
}

/// Decode the base64 content of the key file into the raw 32-byte AES key.
fn parse_key_file(content: &str, path: &std::path::Path) -> Result<[u8; 32], String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content.trim())
        .map_err(|e| format!("corrupt 2FA key file {}: {e}", path.display()))?;
    if bytes.len() != 32 {
        return Err(format!(
            "2FA key file {} must hold 32 bytes, found {}",
            path.display(),
            bytes.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Load the TOTP-encryption key, creating the key file on first use.
///
/// 32 random bytes, base64-encoded, written with `create_new` so two racing
/// processes cannot clobber each other (the loser re-reads the winner's key).
/// Losing this file makes stored secrets undecryptable — back it up with the
/// data directory (backup codes keep working: they never touch the key).
fn load_or_create_totp_key() -> Result<[u8; 32], String> {
    use base64::Engine;
    use std::io::Write;

    let path = totp_key_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        return parse_key_file(&content, &path);
    }

    let key: [u8; 32] = rand::rng().random();
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        // Creation mode, not a post-hoc chmod: the key must never exist on
        // disk with group/world bits, even for the instant between create
        // and chmod.
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(&path) {
        Ok(mut file) => {
            file.write_all(encoded.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("write {}: {e}", path.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another process won the create race — its key is authoritative.
            let content = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            return parse_key_file(&content, &path);
        }
        Err(e) => return Err(format!("create {}: {e}", path.display())),
    }
    Ok(key)
}

/// Encrypt a plaintext base32 TOTP secret into the stored `hv1:` blob.
///
/// The username is the AES-GCM associated data: a blob swapped between rows
/// (e.g. user A's secret pasted over user B's) fails authentication instead
/// of silently verifying A's codes for B.
fn encrypt_totp_secret(username: &str, plaintext: &str) -> Result<String, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine;

    let key = load_or_create_totp_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let nonce_bytes: [u8; 12] = rand::rng().random();
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: username.as_bytes(),
            },
        )
        .map_err(|e| format!("encrypt TOTP secret: {e}"))?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!(
        "{ENCRYPTED_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    ))
}

/// Decrypt a stored secret back to plaintext base32.
///
/// Values without the [`ENCRYPTED_PREFIX`] are legacy plaintext rows: they are
/// returned as-is (the caller re-encrypts them in place) so an upgrade never
/// locks existing users out of their second factor.
fn decrypt_totp_secret(username: &str, stored: &str) -> Result<String, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};
    use base64::Engine;

    let Some(blob64) = stored.strip_prefix(ENCRYPTED_PREFIX) else {
        return Ok(stored.to_string());
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(blob64)
        .map_err(|e| format!("corrupt stored TOTP secret: {e}"))?;
    if blob.len() < 12 + 16 {
        return Err("stored TOTP secret blob is truncated".to_string());
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let nonce_bytes: [u8; 12] = nonce_bytes
        .try_into()
        .map_err(|_| "stored TOTP secret blob is truncated".to_string())?;
    let key = load_or_create_totp_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    let plaintext = cipher
        .decrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad: username.as_bytes(),
            },
        )
        .map_err(|_| "stored TOTP secret could not be decrypted".to_string())?;
    String::from_utf8(plaintext).map_err(|_| "stored TOTP secret is not valid UTF-8".to_string())
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
    ///
    /// The returned [`User2fa.totp_secret`] is the decrypted plaintext. A
    /// legacy row (pre-encryption, raw base32 in the column) is transparently
    /// re-encrypted in place, best-effort — a failed rewrite never blocks a
    /// legitimate login.
    pub async fn get_by_username(&self, username: &str) -> Result<Option<User2fa>, DbError> {
        let row = sqlx::query_as::<_, User2faRow>(
            "SELECT username, totp_secret_encrypted, enabled, backup_codes, created_at FROM user_2fa WHERE username = ?"
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let secret = decrypt_totp_secret(&row.username, &row.totp_secret_encrypted)
            .map_err(DbError::InvalidInput)?;

        // Legacy plaintext row: upgrade it in place now that we hold the
        // plaintext. Best-effort — the read still succeeds if the write fails.
        if !row.totp_secret_encrypted.starts_with(ENCRYPTED_PREFIX) {
            match encrypt_totp_secret(&row.username, &secret) {
                Ok(encrypted) => {
                    if let Err(e) = sqlx::query(
                        "UPDATE user_2fa SET totp_secret_encrypted = ? WHERE username = ?",
                    )
                    .bind(&encrypted)
                    .bind(&row.username)
                    .execute(self.pool.as_ref())
                    .await
                    {
                        tracing::warn!(
                            username = %row.username,
                            error = %e,
                            "failed to re-encrypt a legacy TOTP secret in place"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        username = %row.username,
                        error = %e,
                        "failed to re-encrypt a legacy TOTP secret in place"
                    );
                }
            }
        }

        Ok(Some(User2fa {
            username: row.username,
            totp_secret: secret,
            enabled: row.enabled,
            backup_codes: row.backup_codes,
            created_at: row.created_at,
        }))
    }

    /// Insert or update 2FA configuration for a user (enabled = 1).
    ///
    /// `totp_secret` is the plaintext base32; it is encrypted before it
    /// reaches the database.
    pub async fn save(
        &self,
        username: &str,
        totp_secret: &str,
        backup_codes: &str,
    ) -> Result<(), DbError> {
        let encrypted = encrypt_totp_secret(username, totp_secret).map_err(DbError::InvalidInput)?;
        let now = Utc::now();
        sqlx::query(
            "INSERT OR REPLACE INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at) VALUES (?, ?, 1, ?, ?)"
        )
        .bind(username)
        .bind(&encrypted)
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
    ///
    /// `totp_secret` is the plaintext base32; it is encrypted before it
    /// reaches the database.
    pub async fn save_pending(&self, username: &str, totp_secret: &str) -> Result<(), DbError> {
        let encrypted = encrypt_totp_secret(username, totp_secret).map_err(DbError::InvalidInput)?;
        let now = Utc::now();
        sqlx::query(
            "INSERT OR REPLACE INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at) VALUES (?, ?, 0, '', ?)"
        )
        .bind(username)
        .bind(&encrypted)
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
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT username FROM user_2fa WHERE enabled = 1 ORDER BY username")
                .fetch_all(self.pool.as_ref())
                .await?;

        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Check whether a specific user has 2FA enabled.
    pub async fn is_enabled(&self, username: &str) -> Result<bool, DbError> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM user_2fa WHERE username = ? AND enabled = 1")
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
    pub async fn regenerate_backup_codes(
        &self,
        username: &str,
        count: usize,
    ) -> Result<Vec<String>, DbError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // The encryption functions read the key path via [`totp_key_path`];
    // pin it to this test's temp dir through the test seam (see there for why
    // the env var is not used here) and serialise these tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    type RepoSetup = (
        User2faRepository,
        tempfile::TempDir,
        std::sync::MutexGuard<'static, ()>,
    );

    async fn repo() -> RepoSetup {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().expect("temp dir");
        *TEST_KEY_PATH.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(tmp.path().join("2fa.key"));
        let db = crate::storage::Database::new("sqlite::memory:")
            .await
            .expect("in-memory database");
        db.run_migrations().await.expect("migrations");
        (User2faRepository::new(db.pool()), tmp, guard)
    }

    /// Raw value of the stored secret column — what a database leak would see.
    async fn raw_stored_secret(repo: &User2faRepository, username: &str) -> String {
        let (secret,): (String,) =
            sqlx::query_as("SELECT totp_secret_encrypted FROM user_2fa WHERE username = ?")
                .bind(username)
                .fetch_one(repo.pool.as_ref())
                .await
                .unwrap();
        secret
    }

    #[tokio::test]
    async fn test_save_encrypts_the_secret_at_rest() {
        let (repo, _tmp, _guard) = repo().await;
        repo.save("alice", "JBSWY3DPEHPK3PXP", "[]").await.unwrap();

        let stored = raw_stored_secret(&repo, "alice").await;
        assert_ne!(stored, "JBSWY3DPEHPK3PXP", "plaintext must never hit the DB");
        assert!(stored.starts_with(ENCRYPTED_PREFIX), "got: {stored}");

        // Round-trip: the caller reads the plaintext back.
        let row = repo.get_by_username("alice").await.unwrap().unwrap();
        assert_eq!(row.totp_secret, "JBSWY3DPEHPK3PXP");
        assert!(row.enabled);
    }

    #[tokio::test]
    async fn test_save_pending_encrypts_the_secret_too() {
        let (repo, _tmp, _guard) = repo().await;
        repo.save_pending("bob", "JBSWY3DPEHPK3PXP").await.unwrap();

        let stored = raw_stored_secret(&repo, "bob").await;
        assert_ne!(stored, "JBSWY3DPEHPK3PXP");
        assert!(stored.starts_with(ENCRYPTED_PREFIX));

        let row = repo.get_by_username("bob").await.unwrap().unwrap();
        assert_eq!(row.totp_secret, "JBSWY3DPEHPK3PXP");
        assert!(!row.enabled, "pending enrollment must stay disabled");
    }

    #[tokio::test]
    async fn test_legacy_plaintext_row_is_read_and_reencrypted_in_place() {
        let (repo, _tmp, _guard) = repo().await;
        // A row exactly as a pre-2026-10-18 binary wrote it: raw base32.
        sqlx::query(
            "INSERT INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at) VALUES ('carol', 'JBSWY3DPEHPK3PXP', 1, '[]', '2026-10-01T00:00:00Z')"
        )
        .execute(repo.pool.as_ref())
        .await
        .unwrap();

        // The read still yields the plaintext secret…
        let row = repo.get_by_username("carol").await.unwrap().unwrap();
        assert_eq!(row.totp_secret, "JBSWY3DPEHPK3PXP");
        assert!(row.enabled);

        // …and the row was upgraded in place.
        let stored = raw_stored_secret(&repo, "carol").await;
        assert!(stored.starts_with(ENCRYPTED_PREFIX), "got: {stored}");

        // Idempotent: a second read keeps working.
        let again = repo.get_by_username("carol").await.unwrap().unwrap();
        assert_eq!(again.totp_secret, "JBSWY3DPEHPK3PXP");
    }

    #[tokio::test]
    async fn test_tampered_blob_is_an_error_not_a_silent_mismatch() {
        let (repo, _tmp, _guard) = repo().await;
        repo.save("dave", "JBSWY3DPEHPK3PXP", "[]").await.unwrap();

        // Flip one ciphertext byte — AES-GCM must reject it outright instead
        // of letting a corrupted secret produce wrong codes forever.
        let stored = raw_stored_secret(&repo, "dave").await;
        let mut bytes = stored.as_bytes().to_vec();
        let last = bytes.len() - 1;
        bytes[last] = match bytes[last] {
            b'A' => b'B',
            other => other.wrapping_add(1),
        };
        let tampered = String::from_utf8(bytes).unwrap();
        sqlx::query("UPDATE user_2fa SET totp_secret_encrypted = ? WHERE username = ?")
            .bind(&tampered)
            .bind("dave")
            .execute(repo.pool.as_ref())
            .await
            .unwrap();

        let err = repo.get_by_username("dave").await.unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("decrypt") || msg.contains("corrupt"),
            "a tampered blob must be rejected, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_blob_is_bound_to_its_username() {
        let (repo, _tmp, _guard) = repo().await;
        repo.save("erin", "JBSWY3DPEHPK3PXP", "[]").await.unwrap();

        // Paste erin's blob over frank's row: the AAD check (username) must
        // reject it rather than silently verifying erin's codes for frank.
        let stored = raw_stored_secret(&repo, "erin").await;
        sqlx::query(
            "INSERT INTO user_2fa (username, totp_secret_encrypted, enabled, backup_codes, created_at) VALUES ('frank', ?, 1, '[]', '2026-10-01T00:00:00Z')"
        )
        .bind(&stored)
        .execute(repo.pool.as_ref())
        .await
        .unwrap();

        let err = repo.get_by_username("frank").await.unwrap_err();
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("decrypt") || msg.contains("corrupt"),
            "a blob pasted over another user's row must be rejected, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_key_file_is_created_once_and_read_stably() {
        let (repo, _tmp, _guard) = repo().await;
        repo.save("gina", "JBSWY3DPEHPK3PXP", "[]").await.unwrap();

        let path = totp_key_path();
        let meta = std::fs::metadata(&path).expect("key file created");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                meta.permissions().mode() & 0o777,
                0o600,
                "the key file must be owner-only"
            );
        }
        assert!(meta.len() > 0);

        // A second use reads the same key — secrets stay decryptable.
        let first = load_or_create_totp_key().unwrap();
        let second = load_or_create_totp_key().unwrap();
        assert_eq!(first, second);

        // …and a secret written before the re-read still decrypts.
        let row = repo.get_by_username("gina").await.unwrap().unwrap();
        assert_eq!(row.totp_secret, "JBSWY3DPEHPK3PXP");
    }
}
