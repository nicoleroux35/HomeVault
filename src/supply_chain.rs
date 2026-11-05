/* =============================================================================
 * File          : supply_chain.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Supply-chain checks (tasklist P1-27): pin Docker images by digest and
 *   verify Ed25519 signatures on catalog/update artifacts before use.
 *
 * Dependencies:
 *   - ring (already a direct dependency for WebAuthn), sha2
 *
 * Usage:
 *   Called by the Docker pull path and the catalog/update loaders; see
 *   docs/guides/apps.md.
 * =============================================================================
 */

//! Digest pinning and Ed25519 signature verification.

use ring::signature;

/// A canonical `sha256:<64 lowercase hex>` digest.
pub fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// The digest an image reference is pinned to (`repo@sha256:…`), if any.
pub fn pinned_digest(image: &str) -> Option<&str> {
    let (_, digest) = image.trim().rsplit_once('@')?;
    is_sha256_digest(digest).then_some(digest)
}

/// Whether an image is pinned (and therefore reproducible). A tag is mutable;
/// only a digest names one exact manifest.
pub fn is_pinned(image: &str) -> bool {
    pinned_digest(image).is_some()
}

/// Refuse an image reference whose `@digest` is malformed. An unpinned tag is
/// allowed for interactive pulls (Docker itself resolves it); callers that
/// require reproducibility use [`is_pinned`] explicitly.
pub fn validate_image_reference(image: &str) -> Result<(), String> {
    let image = image.trim();
    if image.is_empty() {
        return Err("an image reference is required".to_string());
    }
    if image.matches('@').count() > 1 {
        return Err("an image reference may contain at most one '@digest'".to_string());
    }
    if let Some((_, digest)) = image.rsplit_once('@') {
        if !is_sha256_digest(digest) {
            return Err(format!(
                "'{digest}' is not a sha256 digest; use '@sha256:<64 hex characters>'"
            ));
        }
    }
    Ok(())
}

/// `sha256:<hex>` of a byte slice.
pub fn sha256_digest(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let hex = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}

/// Verify an Ed25519 signature. `public_key` is the raw 32-byte key and
/// `signature` the raw 64-byte signature, both as shipped by the store.
pub fn verify_ed25519(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), String> {
    if public_key.len() != 32 {
        return Err(format!(
            "an Ed25519 public key is 32 bytes, got {}",
            public_key.len()
        ));
    }
    if signature.len() != 64 {
        return Err(format!(
            "an Ed25519 signature is 64 bytes, got {}",
            signature.len()
        ));
    }
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(message, signature)
        .map_err(|_| "the signature does not match the content".to_string())
}

/// Verify a signed manifest/update: the signature covers the exact bytes on
/// disk. A missing signature is a refusal, never an implicit "trusted".
pub fn verify_artifact(
    bytes: &[u8],
    signature: Option<&[u8]>,
    public_key: Option<&[u8]>,
) -> Result<(), String> {
    let signature = signature.ok_or_else(|| "the artifact has no signature".to_string())?;
    let public_key = public_key.ok_or_else(|| "no signing key is configured".to_string())?;
    verify_ed25519(public_key, bytes, signature)
}

/// Whether the operator asked for every catalog image to be digest-pinned.
/// Off by default: enabling it is a deliberate supply-chain strictness switch.
pub fn require_pinned_images(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// Every `image:` value in a rendered compose file, in file order.
pub fn compose_image_references(compose: &str) -> Vec<String> {
    compose
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let value = line.strip_prefix("image:")?.trim();
            let value = value.split(" #").next().unwrap_or(value).trim();
            let value = value.trim_matches('"').trim_matches('\'').trim();
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

/// Refuse a compose file containing unpinned images when strict mode is on.
/// An image pinned by `@sha256:…` is accepted; a tag (even a version tag) is
/// mutable and names a different manifest over time.
pub fn enforce_pinned_images(compose: &str, require: bool) -> Result<(), String> {
    if !require {
        return Ok(());
    }
    let unpinned: Vec<String> = compose_image_references(compose)
        .into_iter()
        .filter(|image| !is_pinned(image))
        .collect();
    if unpinned.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "these images are not digest-pinned: {}; pin them with '@sha256:<64 hex characters>' or disable HOMEVAULT_REQUIRE_PINNED_IMAGES",
            unpinned.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::KeyPair;

    #[test]
    fn digest_syntax_is_strict() {
        let digest = format!("sha256:{}", "a".repeat(64));
        assert!(is_sha256_digest(&digest));
        assert!(!is_sha256_digest(&format!("sha256:{}", "a".repeat(63))));
        assert!(!is_sha256_digest("sha512:abc"));
        assert!(!is_sha256_digest("abc"));
        assert_eq!(
            pinned_digest(&format!("nginx@{digest}")),
            Some(digest.as_str())
        );
        assert!(is_pinned(&format!("nginx@{digest}")));
        assert!(!is_pinned("nginx:latest"));
    }

    #[test]
    fn compose_images_are_checked_only_in_strict_mode() {
        let digest = format!("sha256:{}", "b".repeat(64));
        let compose = format!(
            "services:\n  web:\n    image: \"nginx:1.27\"\n  db:\n    image: postgres@{digest} # pinned\n"
        );
        assert_eq!(
            compose_image_references(&compose),
            vec!["nginx:1.27".to_string(), format!("postgres@{digest}")]
        );
        assert!(enforce_pinned_images(&compose, false).is_ok());
        let error = enforce_pinned_images(&compose, true).unwrap_err();
        assert!(error.contains("nginx:1.27"));
        assert!(!error.contains("postgres@"));
        assert!(enforce_pinned_images(&format!("image: postgres@{digest}"), true).is_ok());
        assert!(require_pinned_images(Some("on")));
        assert!(!require_pinned_images(Some("0")));
    }

    #[test]
    fn image_references_reject_a_bad_digest() {
        assert!(validate_image_reference("nginx:latest").is_ok());
        assert!(validate_image_reference(&format!("nginx@sha256:{}", "b".repeat(64))).is_ok());
        assert!(validate_image_reference("nginx@sha256:short").is_err());
        assert!(validate_image_reference("nginx@sha256:aaaa@sha256:bbbb").is_err());
        assert!(validate_image_reference("").is_err());
    }

    #[test]
    fn a_valid_ed25519_signature_verifies_and_a_tampered_one_does_not() {
        let rng = SystemRandom::new();
        let pkcs8 = signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let keypair = signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = keypair.public_key().as_ref().to_vec();
        let message = b"homevault-app.toml";
        let signature = keypair.sign(message).as_ref().to_vec();

        assert!(verify_artifact(message, Some(&signature), Some(&public)).is_ok());
        assert!(verify_ed25519(&public, b"tampered", &signature).is_err());
        assert!(verify_artifact(message, None, Some(&public)).is_err());
        assert!(verify_artifact(message, Some(&signature), None).is_err());
        assert_eq!(
            sha256_digest(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
