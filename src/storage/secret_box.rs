/* =============================================================================
 * File          : secret_box.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Authenticated encryption for secrets the appliance must be able to read
 *   back (a backup destination's repository password). AES-256-GCM with the
 *   same appliance key the 2FA secret uses, one `hv1:` blob per value, and a
 *   per-scope AAD so a blob cannot be moved from one row to another.
 *
 * Dependencies:
 *   - aes-gcm (>= 0.11), rand, base64
 *
 * Usage:
 *   src/storage/backup_tenant_repo.rs; see docs/guides/backup-target.md.
 * =============================================================================
 */

//! Sealed secrets at rest (`hv1:` blobs), see docs/guides/backup-target.md.
//!
//! The scope is the AES-GCM associated data. Two tenants can hold the same
//! password and their blobs are still not interchangeable: decrypting one under
//! the other's scope fails authentication instead of silently returning the
//! wrong secret (the same rule `user_2fa_repo` applies with the username).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;

/// Prefix marking a sealed blob.
pub const SEALED_PREFIX: &str = "hv1:";

fn key() -> Result<[u8; 32], String> {
    crate::storage::user_2fa_repo::load_or_create_totp_key()
}

/// Whether a stored value is already sealed.
pub fn is_sealed(value: &str) -> bool {
    value.starts_with(SEALED_PREFIX)
}

/// Seal `plaintext` under `scope` with the appliance key.
pub fn seal(scope: &str, plaintext: &str) -> Result<String, String> {
    seal_with_key(&key()?, scope, plaintext)
}

/// Seal with an explicit key — the pure core, so its round-trip and
/// authentication rules are unit-testable without touching the data
/// directory's key file.
pub fn seal_with_key(key: &[u8; 32], scope: &str, plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Err("refusing to seal an empty secret".to_string());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    use rand::RngExt;
    let nonce_bytes: [u8; 12] = rand::rng().random();
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: plaintext.as_bytes(),
                aad: scope.as_bytes(),
            },
        )
        .map_err(|e| format!("could not seal the secret: {e}"))?;
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(format!(
        "{SEALED_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    ))
}

/// Open a sealed value under the same scope with the appliance key.
pub fn open(scope: &str, stored: &str) -> Result<String, String> {
    open_with_key(&key()?, scope, stored)
}

/// Open with an explicit key (see [`seal_with_key`]).
pub fn open_with_key(key: &[u8; 32], scope: &str, stored: &str) -> Result<String, String> {
    let Some(encoded) = stored.strip_prefix(SEALED_PREFIX) else {
        return Err("the stored value is not a sealed secret".to_string());
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| format!("the sealed secret is corrupt: {e}"))?;
    if blob.len() < 12 + 16 {
        return Err("the sealed secret is truncated".to_string());
    }
    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let nonce_bytes: [u8; 12] = nonce_bytes
        .try_into()
        .map_err(|_| "the sealed secret is truncated".to_string())?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let plaintext = cipher
        .decrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad: scope.as_bytes(),
            },
        )
        .map_err(|_| {
            "the sealed secret could not be opened (wrong scope or corrupt data)".to_string()
        })?;
    String::from_utf8(plaintext).map_err(|_| "the sealed secret is not UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed test key: the round-trip rules do not depend on where the
    /// production key lives.
    const KEY: [u8; 32] = [7u8; 32];

    fn seal(scope: &str, plaintext: &str) -> Result<String, String> {
        seal_with_key(&KEY, scope, plaintext)
    }

    fn open(scope: &str, stored: &str) -> Result<String, String> {
        open_with_key(&KEY, scope, stored)
    }

    #[test]
    fn a_secret_round_trips_under_its_own_scope() {
        let scope = "backup-tenant:family";
        let sealed = super::tests::seal(scope, "correct horse battery staple").unwrap();
        assert!(sealed.starts_with(SEALED_PREFIX));
        assert_ne!(sealed, "correct horse battery staple");
        assert!(is_sealed(&sealed));
        assert_eq!(
            super::tests::open(scope, &sealed).unwrap(),
            "correct horse battery staple"
        );
    }

    #[test]
    fn the_same_secret_seals_differently_each_time() {
        let a = super::tests::seal("scope", "same").unwrap();
        let b = super::tests::seal("scope", "same").unwrap();
        assert_ne!(a, b, "a fresh nonce per blob");
        assert_eq!(
            super::tests::open("scope", &a).unwrap(),
            super::tests::open("scope", &b).unwrap()
        );
    }

    #[test]
    fn a_blob_cannot_be_moved_to_another_scope() {
        let sealed = super::tests::seal("backup-tenant:family", "secret").unwrap();
        assert!(super::tests::open("backup-tenant:guest", &sealed).is_err());
        assert!(super::tests::open("", &sealed).is_err());
    }

    #[test]
    fn tampering_and_garbage_are_refused() {
        let sealed = super::tests::seal("scope", "secret").unwrap();
        // Flip a character in the payload.
        let mut tampered: Vec<char> = sealed.chars().collect();
        let last = tampered.len() - 2;
        tampered[last] = if tampered[last] == 'A' { 'B' } else { 'A' };
        let tampered: String = tampered.into_iter().collect();
        assert!(super::tests::open("scope", &tampered).is_err());
        assert!(super::tests::open("scope", "not-sealed").is_err());
        assert!(super::tests::open("scope", "hv1:!!!!").is_err());
        assert!(super::tests::seal("scope", "").is_err());
    }
}
