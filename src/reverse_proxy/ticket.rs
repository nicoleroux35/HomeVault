/* =============================================================================
 * File          : ticket.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One-time hand-off tickets for the generic proxy's auth gate. The dashboard
 *   mints a ticket after validating the real session; the proxy listener
 *   redeems it exactly once and mints the rule-scoped session on the rule's
 *   own host. In memory, like the app-proxy session store: a restart simply
 *   forces one re-auth round-trip.
 *
 * Dependencies:
 *   - uuid (>= 1), tokio (time)
 *
 * Usage:
 *   handlers/reverse_proxy.rs mints in `GET /proxy-auth`;
 *   reverse_proxy::proxy redeems on `__hv_proxy_auth`.
 * ============================================================================= */

//! Single-use, short-lived tickets for the cross-host auth hand-off.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long a ticket may sit between mint and redemption. A browser follows
/// the redirect immediately; two minutes leaves room for a slow device and
/// keeps a leaked URL useless.
const TICKET_TTL: Duration = Duration::from_secs(120);

/// Upper bound on live tickets. `/proxy-auth` is a GET that mints on every
/// hit, so without a cap a misbehaving client could grow the map between
/// expiries. On overflow the soonest-expiring ticket is evicted.
const MAX_TICKETS: usize = 256;

#[derive(Debug, Clone)]
pub struct Ticket {
    pub rule_id: i64,
    pub username: String,
    /// Validated relative path to return to after redemption.
    pub next: String,
    expires_at: Instant,
}

pub struct TicketStore {
    tickets: Mutex<HashMap<String, Ticket>>,
}

impl Default for TicketStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TicketStore {
    pub fn new() -> Self {
        Self {
            tickets: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a ticket bound to one rule and one validated relative path.
    pub fn mint(&self, rule_id: i64, username: &str, next: &str) -> String {
        let token = format!("hvt_{}", Uuid::new_v4().simple());
        let mut tickets = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_expired(&mut tickets);
        if tickets.len() >= MAX_TICKETS {
            Self::evict_soonest(&mut tickets);
        }
        tickets.insert(
            token.clone(),
            Ticket {
                rule_id,
                username: username.to_string(),
                next: next.to_string(),
                expires_at: Instant::now() + TICKET_TTL,
            },
        );
        token
    }

    /// Redeem a ticket for `rule_id`. A ticket minted for another rule, used
    /// twice, or past its TTL is refused.
    pub fn redeem(&self, token: &str, rule_id: i64) -> Option<Ticket> {
        let mut tickets = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = tickets.remove(token)?;
        if ticket.expires_at <= Instant::now() || ticket.rule_id != rule_id {
            return None;
        }
        Some(ticket)
    }

    /// Drop every ticket minted for a rule (deleted or disabled).
    pub fn revoke_rule(&self, rule_id: i64) {
        let mut tickets = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        tickets.retain(|_, t| t.rule_id != rule_id);
    }

    fn purge_expired(tickets: &mut HashMap<String, Ticket>) {
        let now = Instant::now();
        tickets.retain(|_, t| t.expires_at > now);
    }

    fn evict_soonest(tickets: &mut HashMap<String, Ticket>) {
        if let Some(key) = tickets
            .iter()
            .min_by_key(|(_, t)| t.expires_at)
            .map(|(k, _)| k.clone())
        {
            tickets.remove(&key);
        }
    }
}

/// Process-wide store, mirroring `APP_AUTH`.
pub static TICKETS: Lazy<TicketStore> = Lazy::new(TicketStore::new);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redeem_is_single_use() {
        let store = TicketStore::new();
        let token = store.mint(7, "alice", "/app");
        assert_eq!(store.redeem(&token, 7).unwrap().username, "alice");
        assert!(store.redeem(&token, 7).is_none());
    }

    #[test]
    fn test_redeem_refuses_another_rule() {
        let store = TicketStore::new();
        let token = store.mint(7, "alice", "/");
        assert!(store.redeem(&token, 8).is_none());
    }

    #[test]
    fn test_revoke_rule_drops_its_tickets_only() {
        let store = TicketStore::new();
        let seven = store.mint(7, "alice", "/");
        let eight = store.mint(8, "alice", "/");
        store.revoke_rule(7);
        assert!(store.redeem(&seven, 7).is_none());
        assert!(store.redeem(&eight, 8).is_some());
    }

    #[test]
    fn test_tickets_are_opaque_and_unique() {
        let store = TicketStore::new();
        let a = store.mint(1, "alice", "/");
        let b = store.mint(1, "alice", "/");
        assert_ne!(a, b);
        assert!(a.starts_with("hvt_"));
        assert!(!a.contains("alice"));
    }
}
