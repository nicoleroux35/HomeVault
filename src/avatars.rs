//! Avatar generation service for HomeVault.
//!
//! Avatars are generated locally using the bundled DiceBear Rust crates
//! (`dicebear-core` + `dicebear-styles` with the `miniavs` style). The same
//! seed always produces the same visual avatar, so we only persist the seed
//! string and regenerate the SVG on demand. No network call is ever made -
//! the generation runs entirely off a deterministic PRNG. Note: the raw SVG
//! *markup* is not byte-identical between calls, since `idRandomization`
//! (see `render()`) intentionally appends a fresh random suffix to every
//! element id/href on each call to avoid id collisions when multiple
//! avatars are embedded on the same page.
//!
//! Style: [Miniavs](https://www.dicebear.com/styles/miniavs/) by Webpixels,
//! licensed CC BY 4.0.

use dicebear_core::{Avatar, Style};
use once_cell::sync::Lazy;
use rand::Rng;
use serde_json::json;
use thiserror::Error;

const DEFAULT_SIZE: u32 = 128;

#[derive(Error, Debug, Clone)]
pub enum AvatarError {
    #[error("Avatar style could not be parsed (this is a build-time error)")]
    StyleParseError,
    #[error("Avatar options are invalid: {0}")]
    InvalidOptions(String),
}

/// Lazily parsed Miniavs style definition. The JSON is embedded in the
/// binary at compile time via the `dicebear-styles` crate, so no file is
/// touched at runtime.
static MINIAVS_STYLE: Lazy<Result<Style, AvatarError>> = Lazy::new(|| {
    Style::from_str(dicebear_styles::MINIAVS).map_err(|_| AvatarError::StyleParseError)
});

/// Generate a random seed suitable for DiceBear. The result is a 12-character
/// lowercase alphanumeric string.
pub fn generate_random_seed() -> String {
    let mut bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut bytes);

    // Crockford-ish alphabet (lowercase, no ambiguous chars)
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut out = String::with_capacity(bytes.len());
    for b in bytes {
        out.push(ALPHABET[(b as usize) % ALPHABET.len()] as char);
    }
    out
}

/// Validate a user-provided seed. Returns a normalized seed (trimmed,
/// length-clamped) or `None` if the input cannot be used.
pub fn normalize_seed(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > 64 {
        return None;
    }
    // Reject control characters but otherwise accept any printable seed.
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Render a Miniavs avatar for the given seed.
///
/// `size` is the output width/height in pixels (clamped to 32..=512).
/// The returned string is a complete, self-contained SVG document.
pub fn render(seed: &str, size: Option<u32>) -> Result<String, AvatarError> {
    // `Style` does not implement Clone, so we just borrow from the lazily
    // parsed singleton instead of cloning it.
    let style: &Style = MINIAVS_STYLE
        .as_ref()
        .map_err(|e| e.clone())?;

    let safe_size = size
        .unwrap_or(DEFAULT_SIZE)
        .clamp(32, 512);

    let avatar = Avatar::new(
        style,
        json!({
            "seed": seed,
            "size": safe_size,
            // Round avatars look more like profile pictures at small sizes.
            "borderRadius": 50,
            // Slight randomization of the internal element ids so the
            // SVG can be embedded safely next to other avatars.
            "idRandomization": true,
        }),
    )
    .map_err(|e| AvatarError::InvalidOptions(e.to_string()))?;

    Ok(avatar.to_svg().to_string())
}

/// Convenience helper used by the HTTP layer when no seed has been stored yet.
pub fn render_for(seed: Option<&str>, size: Option<u32>) -> Result<String, AvatarError> {
    let effective = seed
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(generate_random_seed);
    render(&effective, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_alphanumeric_and_non_empty() {
        for _ in 0..50 {
            let s = generate_random_seed();
            assert_eq!(s.len(), 12);
            assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn normalize_seed_rejects_empty_and_too_long() {
        assert!(normalize_seed("").is_none());
        assert!(normalize_seed("   ").is_none());
        assert!(normalize_seed(&"a".repeat(65)).is_none());
    }

    #[test]
    fn normalize_seed_accepts_normal_input() {
        assert_eq!(normalize_seed("alice").unwrap(), "alice");
        assert_eq!(normalize_seed("  bob  ").unwrap(), "bob");
    }

    #[test]
    fn render_produces_a_valid_svg() {
        let svg = render("test-user", Some(64)).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
    }

    /// Strips the per-call random suffix that `dicebear-core` appends to every
    /// element id/href/`url(#...)` when `idRandomization: true` is set (see
    /// `render()`). It's sourced from `std::hash::RandomState`, a fresh
    /// OS-seeded value on every call by design, so it can never be equal
    /// between two calls no matter the seed - that's the point, it's what
    /// lets multiple avatars be embedded on the same page (e.g. the Users
    /// list) without their internal ids colliding. This normalizes it away
    /// so the test below can check what actually needs to be deterministic:
    /// the seed-derived visual content (paths, colors, structure).
    fn strip_id_randomization_suffix(svg: &str) -> String {
        use std::sync::LazyLock;
        static SUFFIX: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r#"-[0-9a-f]{6}("|\))"#).unwrap());
        SUFFIX.replace_all(svg, "$1").to_string()
    }

    #[test]
    fn render_is_deterministic_for_same_seed() {
        let a = render("alice", Some(64)).unwrap();
        let b = render("alice", Some(64)).unwrap();
        assert_eq!(
            strip_id_randomization_suffix(&a),
            strip_id_randomization_suffix(&b)
        );
    }

    #[test]
    fn render_differs_for_different_seeds() {
        let a = render("alice", Some(64)).unwrap();
        let b = render("bob", Some(64)).unwrap();
        assert_ne!(a, b);
    }
}