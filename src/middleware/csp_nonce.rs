/* =============================================================================
 * File          : csp_nonce.rs
 * Author        : Nicolas Le Roux
 * Created       : 17/10/2026
 * Last modified : 17/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-request Content-Security-Policy nonce.
 *
 *   The CSP used to allow `script-src 'unsafe-inline'`, which — combined with
 *   the session token living in `localStorage` — meant any injected script
 *   could exfiltrate a full session. A nonce makes `unsafe-inline` unnecessary:
 *   the policy lists one unpredictable value per response and the single inline
 *   boot script in `index.html` carries it. Injected markup cannot guess it.
 *
 * Dependencies:
 *   - axum (>= 0.8), rand (>= 0.10)
 *
 * Usage:
 *   Generated inside `security_headers_middleware` (the outermost layer), read
 *   back by `handlers::spa::serve_spa` to stamp the HTML.
 * =============================================================================
 */

//! Per-request CSP nonce (replaces `script-src 'unsafe-inline'`).

/// Placeholder in `index.html` that [`stamp_nonce`] replaces at serve time.
///
/// It lives in `frontend/index.html` so the Vite build carries it into
/// `static/dist/index.html`; the backend never serves the unstamped file.
pub const NONCE_PLACEHOLDER: &str = "__HV_CSP_NONCE__";

/// The nonce for the current request, inserted into the request extensions by
/// the security-headers middleware and read by the SPA handler.
#[derive(Clone, Debug)]
pub struct CspNonce(pub String);

impl CspNonce {
    /// Generate a fresh 128-bit nonce, hex-encoded.
    ///
    /// 16 random bytes is the usual CSP guidance: large enough that guessing is
    /// infeasible, short enough to keep the header readable.
    pub fn generate() -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        let mut buf = [0u8; 16];
        rng.fill_bytes(&mut buf);
        Self(buf.iter().map(|b| format!("{:02x}", b)).collect())
    }

    /// The value as it appears in the CSP header and the `nonce` attribute.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Replace every [`NONCE_PLACEHOLDER`] in `html` with `nonce`.
///
/// A no-op when the placeholder is absent, so a stale `index.html` (built
/// before this change) keeps working instead of breaking the shell — the
/// policy would then simply block its inline script, which is the safe failure.
pub fn stamp_nonce(html: &str, nonce: &str) -> String {
    if html.contains(NONCE_PLACEHOLDER) {
        html.replace(NONCE_PLACEHOLDER, nonce)
    } else {
        html.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generated_nonces_are_hex_and_unique() {
        let a = CspNonce::generate();
        let b = CspNonce::generate();
        assert_eq!(a.as_str().len(), 32, "16 bytes hex-encoded");
        assert!(a.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a.as_str(), b.as_str(), "a nonce must never repeat");
    }

    #[test]
    fn test_stamp_replaces_every_occurrence() {
        let html = r#"<script nonce="__HV_CSP_NONCE__">a()</script><script nonce="__HV_CSP_NONCE__">b()</script>"#;
        let stamped = stamp_nonce(html, "abc123");
        assert!(!stamped.contains(NONCE_PLACEHOLDER));
        assert_eq!(stamped.matches("abc123").count(), 2);
    }

    #[test]
    fn test_stamp_is_a_noop_without_the_placeholder() {
        let html = "<script src=\"/assets/x.js\"></script>";
        assert_eq!(stamp_nonce(html, "abc123"), html);
    }
}
