/* =============================================================================
 * File          : app_auth.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   App-proxy session store: the second, narrower credential in front of
 *   proxied catalog apps. Minted by the /app-auth entry from the main
 *   session cookie, validated by the proxy's auth gate, and meaningless
 *   against the API. Entries expire and are swept lazily.
 *
 * Dependencies:
 *   - none (in-memory store)
 *
 * Usage:
 *   handlers/app_auth.rs mints; app_proxy/serve.rs validates every
 *   proxied request.
 * =============================================================================
 */

//! App-proxy session store — the "second auth entry" for proxied catalog apps.
//!
//! ## Why this exists
//!
//! HomeVault's API is header-authenticated (`x-session-token`). A browser
//! navigating straight to a proxied app's port sends no such header, so login
//! also dropped the *full API session token* as an `hv_session` cookie, and the
//! app proxy's auth gate read it directly. That is a privilege leak: the token
//! is valid against the whole dashboard, and (before stripping) it was even
//! forwarded upstream to the container.
//!
//! This module replaces that with a **separate, app-proxy-scoped credential**:
//!
//! 1. The proxy gate rejects a request with no valid `hv_app_session` and
//!    redirects to the server-chosen entry `GET /app-auth?app=<ns_id>` on the
//!    main origin.
//! 2. `/app-auth` validates the real `hv_session` (browser cookie) and mints an
//!    opaque app session token via [`AppAuthStore::mint`], then sets it as
//!    `hv_app_session` and redirects back to the proxy origin.
//! 3. The proxy validates only `hv_app_session` and **strips both HomeVault
//!    cookies** from the request before forwarding upstream, so an installed
//!    app never sees either credential.
//!
//! The app session grants *proxy access only* — it is meaningless against the
//! API — and is revoked when the app's proxy stops. It lives in memory, like
//! the SSE ticket store, so a restart simply forces one re-auth round-trip.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long an app session stays valid. Shorter than the API session: an app
/// proxy is a convenience surface, not a place to hold a long-lived grant.
const APP_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// Global store, mirroring `APP_PROXY`'s process-wide singleton.
pub static APP_AUTH: Lazy<AppAuthStore> = Lazy::new(AppAuthStore::new);

struct AppSession {
    username: String,
    expires_at: Instant,
}

/// In-memory map of app session token → owning user + expiry.
pub struct AppAuthStore {
    sessions: Mutex<HashMap<String, AppSession>>,
}

impl Default for AppAuthStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AppAuthStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Mints a fresh app session token for `username`.
    pub fn mint(&self, username: &str) -> String {
        let token = format!("hva_{}", Uuid::new_v4().simple());
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_expired(&mut sessions);
        sessions.insert(
            token.clone(),
            AppSession {
                username: username.to_string(),
                expires_at: Instant::now() + APP_SESSION_TTL,
            },
        );
        token
    }

    /// Returns the owning username when `token` is a live app session.
    pub fn validate(&self, token: &str) -> Option<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(token).and_then(|s| {
            if s.expires_at > Instant::now() {
                Some(s.username.clone())
            } else {
                None
            }
        })
    }

    /// Drops every session owned by `username` (used on logout).
    pub fn revoke_user(&self, username: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| s.username != username);
    }

    /// Drops every session (used on shutdown/restart paths).
    pub fn revoke_all(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    fn purge_expired(sessions: &mut HashMap<String, AppSession>) {
        let now = Instant::now();
        sessions.retain(|_, s| s.expires_at > now);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mint_and_validate_round_trip() {
        let store = AppAuthStore::new();
        let token = store.mint("alice");
        assert_eq!(store.validate(&token).as_deref(), Some("alice"));
    }

    #[test]
    fn test_validate_rejects_unknown_and_empty_tokens() {
        let store = AppAuthStore::new();
        assert_eq!(store.validate("nope"), None);
        assert_eq!(store.validate(""), None);
    }

    #[test]
    fn test_mint_is_unique_per_call() {
        let store = AppAuthStore::new();
        let a = store.mint("alice");
        let b = store.mint("alice");
        assert_ne!(a, b);
        assert_eq!(store.validate(&a).as_deref(), Some("alice"));
        assert_eq!(store.validate(&b).as_deref(), Some("alice"));
    }

    #[test]
    fn test_revoke_user_removes_only_that_users_sessions() {
        let store = AppAuthStore::new();
        let alice = store.mint("alice");
        let bob = store.mint("bob");
        store.revoke_user("alice");
        assert_eq!(store.validate(&alice), None);
        assert_eq!(store.validate(&bob).as_deref(), Some("bob"));
    }

    #[test]
    fn test_revoke_all_clears_everything() {
        let store = AppAuthStore::new();
        let a = store.mint("alice");
        let b = store.mint("bob");
        store.revoke_all();
        assert_eq!(store.validate(&a), None);
        assert_eq!(store.validate(&b), None);
        assert_eq!(store.len(), 0);
    }
}
