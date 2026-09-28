//! Real-time push for system notifications (health alerts, backup results,
//! antivirus/ransomware/cache events, ...) to the topbar bell panel.
//!
//! Every notification is still persisted first via
//! `NotificationRepository::create` (the source of truth, used by the 30s
//! poll `GET /api/notifications` falls back to) — this module just also
//! fans it out live to any connected `/api/events` client, so the bell
//! panel updates instantly instead of waiting for the next poll.
//!
//! `EventSource` cannot set a custom `X-Session-Token` header, so the
//! `/api/events` connection itself can't be authenticated with
//! `require_auth` the normal way. This uses the same short-lived,
//! single-use ticket pattern already established for the Terminal page's
//! WebSocket (`handlers/terminal_pty.rs`'s `issue_ticket`/`consume_ticket`)
//! rather than putting the real session token in a URL query string.

use crate::storage::notification_repo::Notification;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 100;

/// Matches the Terminal WebSocket ticket's TTL: just long enough to cover
/// issuing the ticket and immediately opening the SSE connection with it.
const TICKET_TTL: Duration = Duration::from_secs(15);

/// Global broadcast channel every `/api/events` connection subscribes to.
/// A lagging/absent receiver is never an error here — notifications are
/// always readable afterward via `GET /api/notifications`, this channel is
/// purely a "wake up sooner" signal.
pub static NOTIFICATION_BUS: LazyLock<broadcast::Sender<Notification>> =
    LazyLock::new(|| broadcast::channel(CHANNEL_CAPACITY).0);

/// Publishes a freshly persisted notification to every connected client.
pub fn publish(notification: &Notification) {
    let _ = NOTIFICATION_BUS.send(notification.clone());
}

static TICKETS: LazyLock<RwLock<HashMap<String, Instant>>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Issues a new single-use ticket, opportunistically sweeping expired ones.
pub fn issue_ticket() -> String {
    let ticket = uuid::Uuid::new_v4().to_string();
    let now = Instant::now();
    let mut tickets = TICKETS.write();
    tickets.retain(|_, issued_at| now.duration_since(*issued_at) < TICKET_TTL);
    tickets.insert(ticket.clone(), now);
    ticket
}

/// Validates and consumes a ticket. Each ticket can only be redeemed once
/// and expires after `TICKET_TTL`.
pub fn consume_ticket(ticket: &str) -> bool {
    match TICKETS.write().remove(ticket) {
        Some(issued_at) => issued_at.elapsed() < TICKET_TTL,
        None => false,
    }
}
