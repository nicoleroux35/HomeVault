/* =============================================================================
 * File          : catalog_signature.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Detached Ed25519 signatures for community app-store manifests (P1-27).
 *   A keyed build refuses an unsigned or tampered store before its apps reach
 *   the catalog; a build with no key keeps the historical behaviour unless
 *   HV_REQUIRE_SIGNED_STORES=1 makes the key mandatory.
 *
 *   What is signed: the exact bytes of `homevault-store.toml` and of each
 *   app's `homevault-app.toml`, with the signature in `<file>.sig`. The key is
 *   32 raw bytes, hex- or base64-encoded, from HV_CATALOG_PUBKEY, the
 *   compile-time HOMEVAULT_CATALOG_PUBKEY, or the packaged default path.
 *
 * Dependencies:
 *   - ring (Ed25519), sha2, base64, std::fs
 *
 * Usage:
 *   app_catalog::store::sync and mod::scan_store call `state_for_store`; see
 *   docs/guides/docker.md → "Signed community stores".
 * =============================================================================
 */

//! Ed25519 signatures for community app-store manifests (see
//! docs/guides/docker.md).

use std::path::{Path, PathBuf};

use base64::Engine as _;
use sha2::{Digest, Sha256};

/// Runtime override: the public key inline (hex/base64) or an absolute path to
/// a file holding it.
pub const KEY_ENV: &str = "HV_CATALOG_PUBKEY";

/// Opt-in hard mode: a missing key itself refuses every store, instead of
/// degrading to the historical unsigned behaviour.
pub const REQUIRE_ENV: &str = "HV_REQUIRE_SIGNED_STORES";

/// Packaged key location, installed by `installer/install.sh` when a release
/// ships one. Absence is not an error unless [`REQUIRE_ENV`] is set.
pub const DEFAULT_KEY_PATH: &str = "/usr/local/lib/homevault/catalog-key.ed25519";

/// Root descriptor of a store checkout.
pub const STORE_DESCRIPTOR: &str = "homevault-store.toml";

/// One app's manifest inside a store checkout.
pub const APP_MANIFEST: &str = "homevault-app.toml";

/// Detached signature suffix (`homevault-store.toml.sig`).
pub const SIGNATURE_SUFFIX: &str = ".sig";

/// Ed25519 public keys and signatures are fixed-length.
const PUBLIC_KEY_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;

/// Outcome of checking a store against the configured trust anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureState {
    /// No key is configured: the store is accepted as before, and the UI says
    /// the signature was not checked.
    Unconfigured,
    /// Every manifest carries a valid signature from the configured key.
    Verified,
    /// A signature is missing, malformed or does not match.
    Invalid(String),
}

impl SignatureState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::Verified => "verified",
            Self::Invalid(_) => "invalid",
        }
    }
}

/// Resolve the trust anchor: runtime env, then the compile-time pin, then the
/// packaged file. `Ok(None)` means "no key configured".
pub fn configured_key() -> Result<Option<Vec<u8>>, String> {
    if let Ok(value) = std::env::var(KEY_ENV) {
        let value = value.trim();
        if !value.is_empty() {
            return load_key(value).map(Some);
        }
    }
    if let Some(value) = option_env!("HOMEVAULT_CATALOG_PUBKEY") {
        let value = value.trim();
        if !value.is_empty() {
            return load_key(value).map(Some);
        }
    }
    let path = Path::new(DEFAULT_KEY_PATH);
    if path.is_file() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {DEFAULT_KEY_PATH}: {e}"))?;
        return decode_public_key(&text).map(Some);
    }
    Ok(None)
}

/// A value is a path when it is absolute; anything else is inline material.
fn load_key(value: &str) -> Result<Vec<u8>, String> {
    if value.starts_with('/') {
        let path = PathBuf::from(value);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read the catalog public key: {e}"))?;
        decode_public_key(&text)
    } else {
        decode_public_key(value)
    }
}

/// Whether a missing key must refuse stores.
pub fn require_signed() -> bool {
    std::env::var(REQUIRE_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Decode a 32-byte public key from hex or standard base64.
pub fn decode_public_key(text: &str) -> Result<Vec<u8>, String> {
    let bytes = decode_material(text)?;
    if bytes.len() != PUBLIC_KEY_LEN {
        return Err(format!(
            "the catalog public key must be {PUBLIC_KEY_LEN} bytes (got {})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Decode hex or standard base64, tolerating whitespace/newlines. Hex is tried
/// first because it is unambiguous for an even-length lowercase string.
pub fn decode_material(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err("the value is empty".to_string());
    }
    if cleaned.len() % 2 == 0 && cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        let mut out = Vec::with_capacity(cleaned.len() / 2);
        let bytes = cleaned.as_bytes();
        for pair in bytes.chunks_exact(2) {
            let high = ((pair[0] as char).to_digit(16)).ok_or("invalid hex")?;
            let low = ((pair[1] as char).to_digit(16)).ok_or("invalid hex")?;
            out.push(((high << 4) | low) as u8);
        }
        return Ok(out);
    }
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|_| "the value is neither hex nor base64".to_string())
}

/// Verify one detached signature. `ring` returns a uniform error, so nothing
/// about the key leaks through a failure message.
pub fn verify_bytes(key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), String> {
    if key.len() != PUBLIC_KEY_LEN {
        return Err("the catalog public key has the wrong length".to_string());
    }
    if signature.len() != SIGNATURE_LEN {
        return Err("the signature has the wrong length".to_string());
    }
    let verifier = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key);
    verifier
        .verify(message, signature)
        .map_err(|_| "the signature does not match the manifest".to_string())
}

/// Verify `path` against `path.sig`. The message is the file's exact bytes, so
/// a one-character edit invalidates the signature.
pub fn verify_manifest(path: &Path, key: &[u8]) -> Result<(), String> {
    let signature_path = PathBuf::from(format!("{}{}", path.display(), SIGNATURE_SUFFIX));
    let text = std::fs::read_to_string(&signature_path).map_err(|_| {
        format!(
            "{} has no detached signature{}",
            path.display(),
            SIGNATURE_SUFFIX
        )
    })?;
    let signature = decode_material(&text)?;
    let message =
        std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    verify_bytes(key, &message, &signature)
}

/// Check a store checkout against an explicit key. Pure so the policy is
/// unit-testable without touching process environment.
pub fn verify_checkout_with_key(
    root: &Path,
    key: Option<&[u8]>,
    require: bool,
) -> Result<SignatureState, String> {
    let Some(key) = key else {
        return if require {
            Err(
                "this installation requires signed catalog stores, but no Ed25519 public key is configured"
                    .to_string(),
            )
        } else {
            // Even without a key, an incomplete checkout is not a store.
            if !root.join(STORE_DESCRIPTOR).is_file() {
                return Err(format!("{STORE_DESCRIPTOR} is missing"));
            }
            Ok(SignatureState::Unconfigured)
        };
    };
    let descriptor = root.join(STORE_DESCRIPTOR);
    if !descriptor.is_file() {
        return Err(format!("{STORE_DESCRIPTOR} is missing"));
    }
    verify_manifest(&descriptor, key)?;
    for app in crate::app_catalog::store::app_dirs(root) {
        let manifest = app.join(APP_MANIFEST);
        verify_manifest(&manifest, key)?;
    }
    Ok(SignatureState::Verified)
}

/// The environment-aware check every caller uses.
pub fn verify_store_checkout(root: &Path) -> Result<SignatureState, String> {
    let key = configured_key()?;
    verify_checkout_with_key(root, key.as_deref(), require_signed())
}

/// Check and fold a failure into `Invalid`, so a listing can report it without
/// failing the whole request.
pub fn state_for_store(root: &Path) -> SignatureState {
    match verify_store_checkout(root) {
        Ok(state) => state,
        Err(e) => SignatureState::Invalid(e),
    }
}

/// Short fingerprint of the configured key, for the status line ("verified
/// with key a1b2c3d4"). Never the key itself.
pub fn key_fingerprint(key: &[u8]) -> String {
    let digest = Sha256::digest(key);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair as _;

    fn keypair(seed_byte: u8) -> ring::signature::Ed25519KeyPair {
        let seed = [seed_byte; 32];
        ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed)
            .expect("a deterministic test key")
    }

    fn sign(keypair: &ring::signature::Ed25519KeyPair, message: &[u8]) -> Vec<u8> {
        keypair.sign(message).as_ref().to_vec()
    }

    fn write_signed(path: &Path, message: &[u8], keypair: &ring::signature::Ed25519KeyPair) {
        std::fs::write(path, message).unwrap();
        let signature = sign(keypair, message);
        std::fs::write(
            format!("{}{}", path.display(), SIGNATURE_SUFFIX),
            hex(&signature),
        )
        .unwrap();
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn test_decode_material_accepts_hex_and_base64() {
        let raw = [0u8; 32];
        assert_eq!(decode_material(&hex(&raw)).unwrap(), raw.to_vec());
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(decode_material(&b64).unwrap(), raw.to_vec());
        // Whitespace/newlines are tolerated (a file written with a newline).
        assert_eq!(
            decode_material(&format!("{}\n", hex(&raw))).unwrap(),
            raw.to_vec()
        );
        assert!(decode_material("").is_err());
        assert!(decode_material("zz-not-hex-or-base64!!").is_err());
    }

    #[test]
    fn test_decode_public_key_requires_32_bytes() {
        assert!(decode_public_key(&hex(&[7u8; 32])).is_ok());
        assert!(decode_public_key(&hex(&[7u8; 31])).is_err());
    }

    #[test]
    fn test_verify_bytes_accepts_the_right_signature_only() {
        let pair = keypair(1);
        let message = b"homevault-store.toml bytes";
        let signature = sign(&pair, message);
        assert!(verify_bytes(pair.public_key().as_ref(), message, &signature).is_ok());
        // A single changed byte invalidates it.
        assert!(verify_bytes(
            pair.public_key().as_ref(),
            b"homevault-store.toml bytez",
            &signature
        )
        .is_err());
        // Another key does not verify it.
        assert!(verify_bytes(keypair(2).public_key().as_ref(), message, &signature).is_err());
        // A truncated signature is refused before reaching ring.
        assert!(verify_bytes(pair.public_key().as_ref(), message, &signature[..63]).is_err());
    }

    #[test]
    fn test_verify_manifest_reports_a_missing_or_tampered_signature() {
        let dir = tempfile::tempdir().unwrap();
        let pair = keypair(3);
        let manifest = dir.path().join(APP_MANIFEST);
        std::fs::write(&manifest, b"id = \"demo\"\n").unwrap();

        // No .sig yet.
        let err = verify_manifest(&manifest, pair.public_key().as_ref()).unwrap_err();
        assert!(err.contains("no detached signature"), "{err}");

        // A valid signature passes, then a tampered manifest fails.
        let signature = sign(&pair, b"id = \"demo\"\n");
        std::fs::write(
            format!("{}{}", manifest.display(), SIGNATURE_SUFFIX),
            hex(&signature),
        )
        .unwrap();
        assert!(verify_manifest(&manifest, pair.public_key().as_ref()).is_ok());
        std::fs::write(&manifest, b"id = \"evil\"\n").unwrap();
        let err = verify_manifest(&manifest, pair.public_key().as_ref()).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn test_verify_checkout_policy() {
        let dir = tempfile::tempdir().unwrap();
        let pair = keypair(4);
        std::fs::create_dir_all(dir.path().join("demo")).unwrap();
        write_signed(
            &dir.path().join(STORE_DESCRIPTOR),
            b"id = \"demo\"\nname = \"Demo\"\n",
            &pair,
        );
        write_signed(
            &dir.path().join("demo").join(APP_MANIFEST),
            b"id = \"demo-app\"\n",
            &pair,
        );

        // No key: unconfigured unless the hard mode demands one.
        assert_eq!(
            verify_checkout_with_key(dir.path(), None, false).unwrap(),
            SignatureState::Unconfigured
        );
        assert!(verify_checkout_with_key(dir.path(), None, true).is_err());

        // The right key verifies the store and every app manifest.
        assert_eq!(
            verify_checkout_with_key(dir.path(), Some(pair.public_key().as_ref()), false).unwrap(),
            SignatureState::Verified
        );

        // An app added without its signature is refused (the store cannot add
        // an unsigned app to a signed one).
        std::fs::create_dir_all(dir.path().join("second")).unwrap();
        std::fs::write(
            dir.path().join("second").join(APP_MANIFEST),
            b"id = \"second\"\n",
        )
        .unwrap();
        let err = verify_checkout_with_key(dir.path(), Some(pair.public_key().as_ref()), false)
            .unwrap_err();
        assert!(err.contains("no detached signature"), "{err}");

        // A different key refuses the whole checkout.
        let other = keypair(5);
        assert!(
            verify_checkout_with_key(dir.path(), Some(other.public_key().as_ref()), false).is_err()
        );
    }

    #[test]
    fn test_state_for_store_folds_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(STORE_DESCRIPTOR), b"id = \"x\"\n").unwrap();
        // No key configured in the test environment: Unconfigured.
        assert_eq!(state_for_store(dir.path()), SignatureState::Unconfigured);
        assert_eq!(SignatureState::Verified.as_str(), "verified");
        assert_eq!(
            state_for_store(&dir.path().join("gone")).as_str(),
            "invalid"
        );
    }

    #[test]
    fn test_key_fingerprint_is_short_and_stable() {
        let key = [9u8; 32];
        let first = key_fingerprint(&key);
        assert_eq!(first.len(), 16);
        assert_eq!(first, key_fingerprint(&key));
        assert_ne!(first, key_fingerprint(&[10u8; 32]));
    }
}
