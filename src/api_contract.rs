/* =============================================================================
 * File          : api_contract.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The versioned client/server contract: a single version constant shared
 *   with the frontend's api.ts, bumped only on breaking API changes. The
 *   contract middleware rejects explicit mismatches with 426 and /api/meta
 *   lets a stale cached bundle detect and prompt for a reload.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   middleware/contract.rs enforces it; handlers/meta.rs exposes the
 *   handshake; the frontend mirrors CONTRACT_VERSION.
 * =============================================================================
 */

//! Versioned client/server contract.
//!
//! HomeVault ships a Rust backend and a Svelte SPA that are built and released
//! together, but a browser can hold a cached `app.js` from the previous build
//! after an update (service worker, aggressive proxy cache, a bookmarked tab).
//! Previously that stale client would silently call endpoints whose shape had
//! changed and fail in confusing ways. This module gives both sides an explicit
//! contract version so a mismatch fails loudly and early.
//!
//! - `CONTRACT_VERSION` is bumped **only** on a breaking API change (a renamed
//!   field, a removed endpoint, a changed request/response shape). Additive
//!   changes do not bump it.
//! - The SPA sends `X-HomeVault-Contract: <n>` on every request (see
//!   `frontend/src/lib/services/api.ts`).
//! - `GET /api/meta` reports both versions; the SPA compares its own compiled
//!   `CONTRACT_VERSION` against the server's and surfaces a reload prompt when
//!   they differ.
//!
//! `SERVER_VERSION` is the human-facing `Cargo.toml` version and is allowed to
//! change on every release; only `CONTRACT_VERSION` gates compatibility.

use serde::Serialize;

/// Breaking-change counter. Bump on any incompatible API change.
pub const CONTRACT_VERSION: u32 = 2;

/// Human-facing build version (`Cargo.toml`).
pub fn server_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether a client advertising `client` is compatible with this server.
///
/// Accepts an optional `X-HomeVault-Contract` header value. A missing or
/// non-numeric header is treated as compatible (older clients and third-party
/// tools that predate the header must keep working); a present header that
/// differs from [`CONTRACT_VERSION`] is incompatible.
pub fn client_is_compatible(client: Option<&str>) -> bool {
    match client.map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some(raw) => raw
            .parse::<u32>()
            .map(|v| v == CONTRACT_VERSION)
            .unwrap_or(true),
    }
}

/// Payload for `GET /api/meta`.
#[derive(Debug, Clone, Serialize)]
pub struct MetaInfo {
    pub contract_version: u32,
    pub server_version: &'static str,
}

impl MetaInfo {
    pub fn current() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            server_version: server_version(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_header_is_compatible() {
        assert!(client_is_compatible(None));
        assert!(client_is_compatible(Some("")));
        assert!(client_is_compatible(Some("   ")));
    }

    #[test]
    fn test_matching_contract_is_compatible() {
        assert!(client_is_compatible(Some(&CONTRACT_VERSION.to_string())));
        assert!(client_is_compatible(Some(" 1 ")));
    }

    #[test]
    fn test_mismatched_contract_is_incompatible() {
        assert!(!client_is_compatible(Some("0")));
        assert!(!client_is_compatible(Some("999")));
    }

    #[test]
    fn test_non_numeric_header_is_treated_as_legacy_and_allowed() {
        // A third-party tool sending a junk value must not be locked out.
        assert!(client_is_compatible(Some("v1")));
        assert!(client_is_compatible(Some("latest")));
    }

    #[test]
    fn test_meta_info_reports_current_contract() {
        let meta = MetaInfo::current();
        assert_eq!(meta.contract_version, CONTRACT_VERSION);
        assert_eq!(meta.server_version, server_version());
    }
}
