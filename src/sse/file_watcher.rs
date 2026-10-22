/* =============================================================================
 * File          : file_watcher.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Pushes live directory-change events (create/modify/remove) to Explorer
 *   clients: an inotify watcher ref-counts the directories SSE clients
 *   currently browse and broadcasts events on a global channel. Access is
 *   gated by short-lived, single-use tickets because the browser's
 *   EventSource cannot send session headers.
 *
 * Dependencies:
 *   - notify (>= 8)
 *   - parking_lot (>= 0.12)
 *   - serde (>= 1.0)
 *   - tokio (>= 1.52)
 *   - uuid (>= 1.0)
 *
 * Usage:
 *   The /api/explorer/watch SSE handler validates a ticket via
 *   `consume_ticket`, then calls `watch_path`/`unwatch_path` around the
 *   connection and streams FILE_CHANGE_BUS events.
 * =============================================================================
 */

//! Real-time filesystem watcher for the Explorer.
//!
//! Uses the `notify` crate (inotify on Linux) to push directory-change events
//! to SSE-connected clients.  Auth uses the same ticket pattern as the
//! notification bus (`sse/notifications.rs`) since `EventSource` cannot set
//! custom headers.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 256;
const TICKET_TTL: Duration = Duration::from_secs(15);

/// A lightweight event pushed to every SSE subscriber.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileChangeEvent {
    pub kind: String,
    pub paths: Vec<String>,
}

/// Global broadcast channel every `/api/explorer/watch` SSE connection
/// subscribes to.
pub static FILE_CHANGE_BUS: LazyLock<broadcast::Sender<FileChangeEvent>> =
    LazyLock::new(|| broadcast::channel(CHANNEL_CAPACITY).0);

// --- ticket auth (same pattern as notifications, bound to user + path) ---

/// A watch ticket binds the SSE connection to exactly one user and one path.
/// The browser's `EventSource` cannot send session headers, so the ticket
/// grants access to this path — and nothing else — until it expires (ROADMAP
/// 10.2: used to be valid for *any* `?path=` the caller typed).
#[derive(Debug, Clone)]
struct WatchTicketInfo {
    issued_at: Instant,
    user: String,
    path: String,
}

static TICKETS: LazyLock<RwLock<HashMap<String, WatchTicketInfo>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Mints a ticket for `user` to watch `path`. The path must already have been
/// validated against the caller's allowed roots.
pub fn issue_ticket(user: &str, path: &str) -> String {
    let ticket = uuid::Uuid::new_v4().to_string();
    let now = Instant::now();
    let mut tickets = TICKETS.write();
    tickets.retain(|_, info| now.duration_since(info.issued_at) < TICKET_TTL);
    tickets.insert(
        ticket.clone(),
        WatchTicketInfo {
            issued_at: now,
            user: user.to_string(),
            path: path.to_string(),
        },
    );
    ticket
}

/// Redeems the ticket, expecting `path` to match the path the ticket was
/// minted for. The ticket is consumed only on a valid, non-expired,
/// path-matching attempt — a *mismatched* path does not burn the ticket
/// (a stale UI retrying the right path after one typo'd link must still
/// work), but wrong-path requests are of course still refused.
pub fn consume_ticket(ticket: &str, path: &str) -> Option<String> {
    let mut tickets = TICKETS.write();
    let info = tickets.get(ticket)?;
    if info.issued_at.elapsed() >= TICKET_TTL {
        tickets.remove(ticket);
        return None;
    }
    if info.path != path {
        return None;
    }
    let user = info.user.clone();
    tickets.remove(ticket);
    Some(user)
}

// --- internal watcher state ---

static WATCHER_STATE: LazyLock<RwLock<WatcherState>> =
    LazyLock::new(|| RwLock::new(WatcherState::new()));

struct WatcherState {
    watcher: Option<RecommendedWatcher>,
    /// Paths we are currently watching → reference count (multiple SSE
    /// clients may request the same path).
    watches: HashMap<PathBuf, usize>,
}

impl WatcherState {
    fn new() -> Self {
        Self {
            watcher: None,
            watches: HashMap::new(),
        }
    }

    fn ensure_watcher(&mut self) -> &mut RecommendedWatcher {
        if self.watcher.is_none() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();
            let w = notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            });
            if w.is_ok() {
                // Spawn a task that translates raw notify events into broadcast
                // events. This task lives as long as the process.
                let bus = FILE_CHANGE_BUS.clone();
                tokio::spawn(async move {
                    while let Some(result) = rx.recv().await {
                        if let Ok(event) = result {
                            let kind = match event.kind {
                                EventKind::Create(_) => "create".into(),
                                EventKind::Modify(_) => "modify".into(),
                                EventKind::Remove(_) => "remove".into(),
                                EventKind::Access(_) => "access".into(),
                                _ => "other".into(),
                            };
                            let paths: Vec<String> = event
                                .paths
                                .iter()
                                .map(|p| p.to_string_lossy().into_owned())
                                .collect();
                            let _ = bus.send(FileChangeEvent { kind, paths });
                        }
                    }
                });
            }
            self.watcher = w.ok();
        }
        self.watcher.as_mut().unwrap()
    }
}

/// Ensures `path` is being watched (ref-counted).
pub fn watch_path(path: &Path) {
    let mut state = WATCHER_STATE.write();
    let entry = state.watches.entry(path.to_path_buf()).or_insert(0);
    *entry += 1;
    if *entry == 1 {
        let watcher = state.ensure_watcher();
        let _ = watcher.watch(path, RecursiveMode::NonRecursive);
    }
}

/// Decrements the ref-count for `path`; stops watching when it reaches zero.
pub fn unwatch_path(path: &Path) {
    let mut state = WATCHER_STATE.write();
    if let Some(count) = state.watches.get_mut(path) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            state.watches.remove(path);
            if let Some(ref mut watcher) = state.watcher {
                let _ = watcher.unwatch(path);
            }
        }
    }
}

/// RAII guard holding one inotify watch alive for the lifetime of an SSE
/// stream. Dropped when the client disconnects and the stream (and hence the
/// response carrying it) is dropped, so watches are always released —
/// ROADMAP 10.2: they used to leak permanently once a disconnect happened
/// mid-browse, until `max_user_watches` was exhausted.
pub struct WatchGuard(PathBuf);

impl Drop for WatchGuard {
    fn drop(&mut self) {
        unwatch_path(&self.0);
    }
}

/// Watch `path` (ref-counted) and return the guard that releases it again
/// when dropped.
pub fn watch_path_guarded(path: &Path) -> WatchGuard {
    watch_path(path);
    WatchGuard(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ROADMAP 10.2: a ticket is bound to the exact path it was minted for —
    /// reusing it for a different `?path=` the caller requests must fail.
    #[test]
    fn ticket_is_bound_to_its_path() {
        let ticket = issue_ticket("alice", "/media/raid/home");
        assert_eq!(
            consume_ticket(&ticket, "/media/raid/home").as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn ticket_rejected_for_other_path() {
        let ticket = issue_ticket("bob", "/media/raid/home");
        assert_eq!(consume_ticket(&ticket, "/media/raid"), None);
        assert_eq!(consume_ticket(&ticket, "/media/raid/home/child"), None);
        assert_eq!(consume_ticket(&ticket, "/media/raid/homee"), None);
        // A refused attempt does not burn the ticket.
        assert_eq!(
            consume_ticket(&ticket, "/media/raid/home").as_deref(),
            Some("bob")
        );
    }

    #[test]
    fn ticket_is_single_use() {
        let ticket = issue_ticket("carol", "/etc");
        assert_eq!(consume_ticket(&ticket, "/etc").as_deref(), Some("carol"));
        assert_eq!(consume_ticket(&ticket, "/etc"), None);
    }

    #[test]
    fn unknown_and_empty_tickets_are_rejected() {
        assert_eq!(consume_ticket("not-a-real-ticket", "/etc"), None);
        assert_eq!(consume_ticket("", "/etc"), None);
        assert!(!issue_ticket("dave", "/tmp").is_empty());
    }
}
