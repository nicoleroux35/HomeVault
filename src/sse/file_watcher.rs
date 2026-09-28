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

// --- ticket auth (same pattern as notifications) ---

static TICKETS: LazyLock<RwLock<HashMap<String, Instant>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn issue_ticket() -> String {
    let ticket = uuid::Uuid::new_v4().to_string();
    let now = Instant::now();
    let mut tickets = TICKETS.write();
    tickets.retain(|_, issued_at| now.duration_since(*issued_at) < TICKET_TTL);
    tickets.insert(ticket.clone(), now);
    ticket
}

pub fn consume_ticket(ticket: &str) -> bool {
    match TICKETS.write().remove(ticket) {
        Some(issued_at) => issued_at.elapsed() < TICKET_TTL,
        None => false,
    }
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
