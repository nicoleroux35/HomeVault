/* =============================================================================
 * File          : app_auth.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
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
//! API — and is revoked when **its** app's proxy stops (other apps keep their
//! sessions). It lives in memory, like the SSE ticket store, so a restart
//! simply forces one re-auth round-trip.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long an app session stays valid. Shorter than the API session: an app
/// proxy is a convenience surface, not a place to hold a long-lived grant.
const APP_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// Upper bound on live app sessions. `/app-auth` is a plain GET that mints on
/// every hit, so a misbehaving browser tab or script could otherwise grow this
/// store unboundedly between the 12-hour expiries. When the cap is hit, the
/// soonest-expiring session is evicted to make room — a home instance has only
/// a couple of apps and users, so legitimate traffic never reaches this.
const MAX_APP_SESSIONS: usize = 512;

/// Global store, mirroring `APP_PROXY`'s process-wide singleton.
pub static APP_AUTH: Lazy<AppAuthStore> = Lazy::new(AppAuthStore::new);

struct AppSession {
    username: String,
    /// Namespaced id of the app this session was minted for. A token is only
    /// valid on that app's proxy: minting is a per-app grant, never a
    /// household-wide one.
    ns_id: String,
    expires_at: Instant,
}

/// In-memory map of app session token → owning user + app + expiry.
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

    /// Mints a fresh app session token for `username`, valid only on the
    /// proxy of `ns_id`.
    pub fn mint(&self, username: &str, ns_id: &str) -> String {
        let token = format!("hva_{}", Uuid::new_v4().simple());
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_expired(&mut sessions);
        if sessions.len() >= MAX_APP_SESSIONS {
            Self::evict_soonest(&mut sessions);
        }
        sessions.insert(
            token.clone(),
            AppSession {
                username: username.to_string(),
                ns_id: ns_id.to_string(),
                expires_at: Instant::now() + APP_SESSION_TTL,
            },
        );
        token
    }

    /// Returns the owning username when `token` is a live app session **for
    /// `ns_id`**. A token minted for another app is rejected, so one gated app
    /// can never be reached with another app's credential.
    pub fn validate(&self, token: &str, ns_id: &str) -> Option<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.get(token).and_then(|s| {
            if s.expires_at > Instant::now() && s.ns_id == ns_id {
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

    /// Drops every session minted for one app's proxy, leaving every other
    /// app's sessions untouched.
    pub fn revoke_app(&self, ns_id: &str) {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| s.ns_id != ns_id);
    }

    fn purge_expired(sessions: &mut HashMap<String, AppSession>) {
        let now = Instant::now();
        sessions.retain(|_, s| s.expires_at > now);
    }

    /// Drops the live session with the earliest expiry to make room for a new
    /// mint (only called at the [`MAX_APP_SESSIONS`] cap).
    fn evict_soonest(sessions: &mut HashMap<String, AppSession>) {
        let Some(oldest) = sessions
            .iter()
            .min_by_key(|(_, s)| s.expires_at)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        sessions.remove(&oldest);
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
        let token = store.mint("alice", "default:joplin");
        assert_eq!(
            store.validate(&token, "default:joplin").as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn test_validate_rejects_unknown_and_empty_tokens() {
        let store = AppAuthStore::new();
        assert_eq!(store.validate("nope", "default:joplin"), None);
        assert_eq!(store.validate("", "default:joplin"), None);
    }

    /// ROADMAP 6.2: a session is a per-app grant. The token minted for one
    /// app's proxy must be useless on another app's proxy.
    #[test]
    fn test_session_is_bound_to_the_app_it_was_minted_for() {
        let store = AppAuthStore::new();
        let token = store.mint("alice", "default:joplin");
        assert_eq!(
            store.validate(&token, "default:joplin").as_deref(),
            Some("alice")
        );
        assert_eq!(store.validate(&token, "default:immich"), None);
        assert_eq!(store.validate(&token, "store two:docmost"), None);
    }

    #[test]
    fn test_mint_is_unique_per_call() {
        let store = AppAuthStore::new();
        let a = store.mint("alice", "default:joplin");
        let b = store.mint("alice", "default:joplin");
        assert_ne!(a, b);
        assert_eq!(
            store.validate(&a, "default:joplin").as_deref(),
            Some("alice")
        );
        assert_eq!(
            store.validate(&b, "default:joplin").as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn test_revoke_user_removes_only_that_users_sessions() {
        let store = AppAuthStore::new();
        let alice = store.mint("alice", "default:joplin");
        let bob = store.mint("bob", "default:joplin");
        store.revoke_user("alice");
        assert_eq!(store.validate(&alice, "default:joplin"), None);
        assert_eq!(
            store.validate(&bob, "default:joplin").as_deref(),
            Some("bob")
        );
    }

    /// ROADMAP 6.2: stopping one app revokes that app's sessions only —
    /// stopping app A must never sign the household out of apps B..Z.
    #[test]
    fn test_revoke_app_leaves_other_apps_signed_in() {
        let store = AppAuthStore::new();
        let joplin = store.mint("alice", "default:joplin");
        let immich = store.mint("alice", "default:immich");
        let docmost = store.mint("bob", "home:docmost");
        store.revoke_app("default:joplin");
        assert_eq!(store.validate(&joplin, "default:joplin"), None);
        assert_eq!(
            store.validate(&immich, "default:immich").as_deref(),
            Some("alice")
        );
        assert_eq!(
            store.validate(&docmost, "home:docmost").as_deref(),
            Some("bob")
        );
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_revoke_app_with_no_sessions_is_a_no_op() {
        let store = AppAuthStore::new();
        let kept = store.mint("alice", "default:joplin");
        store.revoke_app("default:absent");
        assert_eq!(
            store.validate(&kept, "default:joplin").as_deref(),
            Some("alice")
        );
    }

    /// Minting is bounded: at the cap the soonest-expiring session is evicted
    /// so the in-memory store cannot grow without limit under repeated
    /// `/app-auth` hits.
    #[test]
    fn test_mint_is_capped_and_keeps_the_newest_sessions() {
        let store = AppAuthStore::new();
        for i in 0..MAX_APP_SESSIONS {
            store.mint(&format!("user{i}"), "default:joplin");
        }
        assert_eq!(store.len(), MAX_APP_SESSIONS);
        // One more mint evicts the oldest entry instead of growing the store…
        let overflow = store.mint("overflow-user", "default:joplin");
        assert_eq!(
            store.validate(&overflow, "default:joplin").as_deref(),
            Some("overflow-user")
        );
        assert_eq!(store.len(), MAX_APP_SESSIONS);
        // …and the store's oldest entry was the one evicted.
        let evicted = store
            .sessions
            .lock()
            .unwrap()
            .values()
            .any(|s| s.username == "user0");
        assert!(!evicted);
    }
}
