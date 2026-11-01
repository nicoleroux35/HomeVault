/* =============================================================================
 * File          : identity.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Sign-in continuity: trusted devices (a browser that may skip the second
 *   factor for a bounded time, revocable one by one) and a verified rescue
 *   e-mail address that can receive account-recovery codes. Token and code
 *   handling is pure so the security-relevant parts are unit-tested.
 *
 * Dependencies:
 *   - sqlx (>= 0.9, SQLite), sha2, rand, crate::email
 *
 * Usage:
 *   handlers::identity exposes the endpoints; crate::handlers::auth consults
 *   `DeviceStore::find_valid` on the 2FA path; see docs/guides/identity.md.
 * =============================================================================
 */

//! Trusted devices and rescue e-mail (see docs/guides/identity.md).
//!
//! Two rules shape everything here:
//!
//! * **Nothing sensitive is stored in the clear.** A trusted-device cookie is
//!   stored as a SHA-256 hash, and a rescue code as a salted hash with an
//!   attempt counter — a database copy must not be enough to bypass a second
//!   factor or take over an account.
//! * **A trusted device is a time-boxed convenience, not a credential.** It
//!   expires, it can be listed and revoked by the account owner, and every
//!   sign-in through it is still recorded in the login history.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// Cookie that carries a trusted-device token to the browser.
pub const DEVICE_COOKIE: &str = "hv_trusted_device";

/// How long a device stays trusted before the second factor is asked again.
pub const DEVICE_LIFETIME_DAYS: i64 = 30;

/// Rescue codes are short-lived: they are typed by a human, so long enough to
/// read from a phone, short enough that a leaked mailbox is not a standing key.
pub const CODE_TTL_MINUTES: i64 = 15;

/// After this many wrong guesses the pending code is destroyed.
pub const MAX_CODE_ATTEMPTS: i64 = 5;

/// What an outstanding rescue code is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodePurpose {
    /// Prove ownership of a new rescue address.
    VerifyEmail,
    /// Recover the account from that address.
    RecoverAccount,
}

impl CodePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            CodePurpose::VerifyEmail => "verify-email",
            CodePurpose::RecoverAccount => "recover-account",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "verify-email" => Some(CodePurpose::VerifyEmail),
            "recover-account" => Some(CodePurpose::RecoverAccount),
            _ => None,
        }
    }
}

/// One trusted browser, as the account owner sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub id: String,
    pub username: String,
    pub label: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
}

// --------------------------------------------------------------------------
// Tokens, codes and labels (pure)
// --------------------------------------------------------------------------

/// A fresh device token: 32 random bytes, base64url without padding.
pub fn new_device_token() -> String {
    use rand::Rng;
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    crate::acme::base64url_nopad(&buf)
}

/// The stored form of a device token. Hex is used rather than base64 so the
/// value is unambiguous in a database dump and cannot be confused with a live
/// token by eye.
pub fn device_token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A six-digit rescue code. Uniform over `000000`–`999999`.
pub fn new_rescue_code() -> String {
    use rand::RngExt;
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{n:06}")
}

/// Hash of a rescue code, salted with the account and the address it was sent
/// to: the same code issued twice does not produce the same hash, and a hash
/// cannot be replayed against another account.
pub fn rescue_code_hash(username: &str, email: &str, code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"homevault-rescue-code\0");
    hasher.update(username.to_ascii_lowercase().as_bytes());
    hasher.update(b"\0");
    hasher.update(email.to_ascii_lowercase().as_bytes());
    hasher.update(b"\0");
    hasher.update(code.trim().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A short label for a device, from its user agent. Deliberately coarse: a
/// home appliance is reached from a handful of clients, and the owner needs to
/// recognise one in a list, not identify a build.
pub fn device_label(user_agent: Option<&str>) -> String {
    let ua = user_agent.unwrap_or("").to_ascii_lowercase();
    let platform = if ua.contains("iphone") || ua.contains("ipad") {
        "iOS"
    } else if ua.contains("android") {
        "Android"
    } else if ua.contains("windows") {
        "Windows"
    } else if ua.contains("mac os") || ua.contains("macintosh") {
        "macOS"
    } else if ua.contains("linux") {
        "Linux"
    } else {
        "Unknown device"
    };
    let browser = if ua.contains("edg/") {
        "Edge"
    } else if ua.contains("firefox") {
        "Firefox"
    } else if ua.contains("chrome") || ua.contains("crios") {
        "Chrome"
    } else if ua.contains("safari") {
        "Safari"
    } else {
        ""
    };
    if browser.is_empty() {
        platform.to_string()
    } else {
        format!("{browser} on {platform}")
    }
}

/// A rescue address must be a bare address: it is handed to the MTA and shown
/// back to the account owner, so anything else is a configuration mistake.
pub fn validate_rescue_email(email: &str) -> Result<String, String> {
    let e = email.trim();
    if e.is_empty() {
        return Err("a rescue address is required".to_string());
    }
    if e.len() > 254 {
        return Err("the address is too long".to_string());
    }
    if e.chars().any(char::is_whitespace) || e.contains(':') || e.contains('<') {
        return Err("enter a bare e-mail address".to_string());
    }
    let (local, domain) = e
        .split_once('@')
        .ok_or_else(|| "enter a valid e-mail address".to_string())?;
    if local.is_empty() || domain.is_empty() || !domain.contains('.') || domain.starts_with('.') {
        return Err("enter a valid e-mail address".to_string());
    }
    Ok(e.to_ascii_lowercase())
}

/// Whether a stored code is still usable at `now` (RFC 3339 string, as stored).
pub fn is_expired(expires_at: &str, now: &str) -> bool {
    // Lexicographic comparison is correct for RFC 3339 UTC strings with the
    // same precision, which is what every writer here produces.
    expires_at <= now
}

// --------------------------------------------------------------------------
// Trusted devices
// --------------------------------------------------------------------------

/// Persistence for trusted devices, scoped to one account.
pub struct DeviceStore<'a> {
    pool: &'a SqlitePool,
}

impl<'a> DeviceStore<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Persist a new device for `username` and return the row plus the raw
    /// token the browser must receive. The raw token exists only here.
    pub async fn create(
        &self,
        username: &str,
        label: &str,
    ) -> Result<(TrustedDevice, String), String> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let token = new_device_token();
        let now = crate::acme::now_rfc3339();
        let expires = (time::OffsetDateTime::now_utc()
            + time::Duration::days(DEVICE_LIFETIME_DAYS))
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO trusted_devices (id, username, label, token_hash, created_at, last_seen_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(username)
        .bind(label)
        .bind(device_token_hash(&token))
        .bind(&now)
        .bind(&now)
        .bind(&expires)
        .execute(self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok((
            TrustedDevice {
                id,
                username: username.to_string(),
                label: label.to_string(),
                created_at: now.clone(),
                last_seen_at: now,
                expires_at: expires,
            },
            token,
        ))
    }

    /// The device a cookie refers to, when it is still valid. An expired row is
    /// deleted on sight rather than left to accumulate.
    pub async fn find_valid(&self, token: &str) -> Result<Option<TrustedDevice>, String> {
        if token.trim().is_empty() {
            return Ok(None);
        }
        let hash = device_token_hash(token.trim());
        let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, username, label, created_at, last_seen_at, expires_at
             FROM trusted_devices WHERE token_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(self.pool)
        .await
        .map_err(|e| e.to_string())?;
        let Some((id, username, label, created_at, last_seen_at, expires_at)) = row else {
            return Ok(None);
        };
        if is_expired(&expires_at, &crate::acme::now_rfc3339()) {
            let _ = sqlx::query("DELETE FROM trusted_devices WHERE id = ?")
                .bind(&id)
                .execute(self.pool)
                .await;
            return Ok(None);
        }
        Ok(Some(TrustedDevice {
            id,
            username,
            label,
            created_at,
            last_seen_at,
            expires_at,
        }))
    }

    /// Record that the device was just used. Best effort: a failed touch must
    /// not turn a valid sign-in into an error.
    pub async fn touch(&self, id: &str) {
        let _ = sqlx::query("UPDATE trusted_devices SET last_seen_at = ? WHERE id = ?")
            .bind(crate::acme::now_rfc3339())
            .bind(id)
            .execute(self.pool)
            .await;
    }

    pub async fn list(&self, username: &str) -> Result<Vec<TrustedDevice>, String> {
        let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, username, label, created_at, last_seen_at, expires_at
             FROM trusted_devices WHERE username = ? ORDER BY last_seen_at DESC",
        )
        .bind(username)
        .fetch_all(self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .map(
                |(id, username, label, created_at, last_seen_at, expires_at)| TrustedDevice {
                    id,
                    username,
                    label,
                    created_at,
                    last_seen_at,
                    expires_at,
                },
            )
            .collect())
    }

    /// Revoke one device. Scoped to the owner so an id from another account is
    /// a no-op rather than a cross-account delete.
    pub async fn revoke(&self, username: &str, id: &str) -> Result<bool, String> {
        let result = sqlx::query("DELETE FROM trusted_devices WHERE id = ? AND username = ?")
            .bind(id)
            .bind(username)
            .execute(self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    /// Revoke every device of one account (used when the password or the
    /// second factor changes: both invalidate the trust).
    pub async fn revoke_all(&self, username: &str) -> Result<u64, String> {
        let result = sqlx::query("DELETE FROM trusted_devices WHERE username = ?")
            .bind(username)
            .execute(self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.rows_affected())
    }
}

// --------------------------------------------------------------------------
// Rescue e-mail codes
// --------------------------------------------------------------------------

/// Issue a code for `purpose` and store its hash, replacing any previous one.
pub async fn issue_code(
    pool: &SqlitePool,
    username: &str,
    email: &str,
    purpose: CodePurpose,
) -> Result<String, String> {
    let code = new_rescue_code();
    let now = time::OffsetDateTime::now_utc();
    let expires = (now + time::Duration::minutes(CODE_TTL_MINUTES))
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| e.to_string())?;
    sqlx::query(
        "INSERT INTO rescue_email_codes
            (username, email, purpose, code_hash, attempts, created_at, expires_at)
         VALUES (?, ?, ?, ?, 0, ?, ?)
         ON CONFLICT(username) DO UPDATE SET
            email = excluded.email,
            purpose = excluded.purpose,
            code_hash = excluded.code_hash,
            attempts = 0,
            created_at = excluded.created_at,
            expires_at = excluded.expires_at",
    )
    .bind(username)
    .bind(email)
    .bind(purpose.as_str())
    .bind(rescue_code_hash(username, email, &code))
    .bind(crate::acme::now_rfc3339())
    .bind(&expires)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(code)
}

/// Result of checking a submitted code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeCheck {
    Ok(String),
    Wrong,
    Expired,
    TooManyAttempts,
    Missing,
}

/// Check `code` against the outstanding code for `purpose`.
///
/// A wrong guess increments the attempt counter and destroys the code at
/// [`MAX_CODE_ATTEMPTS`]; a correct guess deletes it, so a code is single-use.
pub async fn check_code(
    pool: &SqlitePool,
    username: &str,
    purpose: CodePurpose,
    code: &str,
) -> Result<CodeCheck, String> {
    let row: Option<(String, String, String, i64, String)> = sqlx::query_as(
        "SELECT email, purpose, code_hash, attempts, expires_at
         FROM rescue_email_codes WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    let Some((email, stored_purpose, code_hash, attempts, expires_at)) = row else {
        return Ok(CodeCheck::Missing);
    };
    if stored_purpose != purpose.as_str() {
        return Ok(CodeCheck::Missing);
    }
    if is_expired(&expires_at, &crate::acme::now_rfc3339()) {
        let _ = sqlx::query("DELETE FROM rescue_email_codes WHERE username = ?")
            .bind(username)
            .execute(pool)
            .await;
        return Ok(CodeCheck::Expired);
    }
    if attempts >= MAX_CODE_ATTEMPTS {
        let _ = sqlx::query("DELETE FROM rescue_email_codes WHERE username = ?")
            .bind(username)
            .execute(pool)
            .await;
        return Ok(CodeCheck::TooManyAttempts);
    }
    if rescue_code_hash(username, &email, code) != code_hash {
        let next = attempts + 1;
        if next >= MAX_CODE_ATTEMPTS {
            let _ = sqlx::query("DELETE FROM rescue_email_codes WHERE username = ?")
                .bind(username)
                .execute(pool)
                .await;
        } else {
            let _ = sqlx::query("UPDATE rescue_email_codes SET attempts = ? WHERE username = ?")
                .bind(next)
                .bind(username)
                .execute(pool)
                .await;
        }
        return Ok(CodeCheck::Wrong);
    }
    let _ = sqlx::query("DELETE FROM rescue_email_codes WHERE username = ?")
        .bind(username)
        .execute(pool)
        .await;
    Ok(CodeCheck::Ok(email))
}

/// The verified rescue address of an account, if it has one.
pub async fn rescue_address(pool: &SqlitePool, username: &str) -> Result<Option<String>, String> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT rescue_email, rescue_email_verified_at FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(row.and_then(|(email, verified)| match (email, verified) {
        (Some(email), Some(_)) => Some(email),
        _ => None,
    }))
}

/// Store a newly verified rescue address.
pub async fn set_rescue_address(
    pool: &SqlitePool,
    username: &str,
    email: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE users SET rescue_email = ?, rescue_email_verified_at = ? WHERE username = ?",
    )
    .bind(email.to_ascii_lowercase())
    .bind(crate::acme::now_rfc3339())
    .bind(username)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Remove the rescue address (the admin no longer wants mail-based recovery).
pub async fn clear_rescue_address(pool: &SqlitePool, username: &str) -> Result<(), String> {
    sqlx::query(
        "UPDATE users SET rescue_email = NULL, rescue_email_verified_at = NULL WHERE username = ?",
    )
    .bind(username)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_tokens_are_unique_and_url_safe() {
        let a = new_device_token();
        let b = new_device_token();
        assert_ne!(a, b);
        assert!(a.len() >= 43, "32 bytes of base64url is 43 characters");
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_device_token_hash_is_stable_and_not_reversible() {
        let token = "a-fixed-token";
        assert_eq!(device_token_hash(token), device_token_hash(token));
        assert_eq!(device_token_hash(token).len(), 64);
        assert!(!device_token_hash(token).contains(token));
        assert_ne!(device_token_hash("a"), device_token_hash("b"));
    }

    #[test]
    fn test_rescue_codes_are_six_digits() {
        for _ in 0..50 {
            let code = new_rescue_code();
            assert_eq!(code.len(), 6, "{code}");
            assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
        }
    }

    #[test]
    fn test_rescue_code_hash_is_bound_to_the_account_and_address() {
        let code = "123456";
        let base = rescue_code_hash("alice", "alice@example.com", code);
        assert_eq!(base.len(), 64);
        assert_ne!(base, rescue_code_hash("bob", "alice@example.com", code));
        assert_ne!(base, rescue_code_hash("alice", "other@example.com", code));
        assert_ne!(
            base,
            rescue_code_hash("alice", "alice@example.com", "654321")
        );
        assert_eq!(
            base,
            rescue_code_hash("ALICE", "Alice@Example.com", " 123456 ")
        );
    }

    #[test]
    fn test_device_label_recognises_common_clients() {
        assert_eq!(
            device_label(Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0) AppleWebKit/605.1.15 Safari/604.1"
            )),
            "Safari on iOS"
        );
        assert_eq!(
            device_label(Some(
                "Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0 Safari/537.36"
            )),
            "Chrome on Linux"
        );
        assert_eq!(
            device_label(Some("Mozilla/5.0 (Windows NT 10.0) Edg/120.0")),
            "Edge on Windows"
        );
        assert_eq!(device_label(None), "Unknown device");
        assert_eq!(device_label(Some("curl/8.0")), "Unknown device");
    }

    #[test]
    fn test_rescue_email_validation() {
        assert_eq!(
            validate_rescue_email(" Admin@Example.COM ").unwrap(),
            "admin@example.com"
        );
        for bad in [
            "",
            "admin",
            "@example.com",
            "admin@example",
            "admin@example.com extra",
            "admin <admin@example.com>",
            "mailto:admin@example.com",
        ] {
            assert!(validate_rescue_email(bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn test_expiry_comparison_uses_rfc3339_ordering() {
        assert!(is_expired("2026-01-01T00:00:00Z", "2026-01-02T00:00:00Z"));
        assert!(!is_expired("2026-01-02T00:00:00Z", "2026-01-01T00:00:00Z"));
        assert!(is_expired("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"));
    }

    #[test]
    fn test_code_purpose_roundtrips() {
        assert_eq!(
            CodePurpose::parse(CodePurpose::VerifyEmail.as_str()),
            Some(CodePurpose::VerifyEmail)
        );
        assert_eq!(
            CodePurpose::parse(CodePurpose::RecoverAccount.as_str()),
            Some(CodePurpose::RecoverAccount)
        );
        assert_eq!(CodePurpose::parse("something-else"), None);
    }
}
