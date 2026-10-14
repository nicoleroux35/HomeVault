/* =============================================================================
 * File          : ransomware_watcher.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Background filesystem watcher for anti-ransomware protection: watches
 *   ONLY the admin-declared protected paths (never the whole filesystem —
 *   the appliance targets low-power boards), rates change bursts and
 *   honeypot touches, and hands suspects to the response layer. Runs as a
 *   long-lived task with a command channel for path updates.
 *
 * Dependencies:
 *   - notify (>= 8), tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup; settings changes arrive over the watch
 *   command channel (AppState.ransomware_watch_tx).
 * =============================================================================
 */

//! Background filesystem watcher for anti-ransomware protection.
//!
//! Watches **only** the paths an admin has explicitly added as "protected"
//! (typically shares) — never the whole filesystem — since HomeVault targets
//! low-power boards/mini-PCs where a filesystem-wide watch would be a real
//! resource cost. Detects two kinds of suspicious activity:
//!
//! * A honeypot decoy file (see `src/ransomware.rs::place_honeypot`) being
//!   touched at all — fires immediately, bypassing the rate window, since a
//!   single touch on a file nothing legitimate should ever open is already
//!   suspicious.
//! * A burst of modify/rename/delete events on a protected path within a
//!   short window — a heuristic for "something is rewriting/encrypting many
//!   files fast", which can both false-positive (a legitimate bulk restore
//!   or rsync) and false-negative (a slow, low-and-slow attack). See the
//!   scope notes in `docs/CLAUDE.md`.

use crate::handlers::AppState;
use crate::ransomware;
use crate::storage::RansomwareRepository;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Commands sent from HTTP handlers to reconfigure the running watcher
/// without restarting it.
#[derive(Debug)]
pub enum WatchCommand {
    AddPath {
        id: String,
        path: PathBuf,
        honeypot_filename: Option<String>,
    },
    RemovePath {
        id: String,
    },
    ReloadSettings,
}

struct ProtectedEntry {
    path: PathBuf,
    honeypot_filename: Option<String>,
}

/// Local, event-loop-owned copy of the rate-detection thresholds — refreshed
/// on `WatchCommand::ReloadSettings` rather than re-read from the DB on
/// every single filesystem event, so a genuine burst of activity doesn't
/// also hammer SQLite with reads.
struct CachedSettings {
    rate_threshold_events: usize,
    rate_threshold_seconds: u64,
}

/// Runs forever, translating raw filesystem events on protected paths into
/// honeypot/rate-based ransomware detection. Intended to be `tokio::spawn`ed
/// once at server startup, with the `WatchCommand` sender half stored on
/// `AppState` so handlers can add/remove protected paths live.
pub async fn run_watcher_loop(state: Arc<AppState>, mut rx: mpsc::UnboundedReceiver<WatchCommand>) {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();

    let mut watcher: RecommendedWatcher =
        match notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = event_tx.send(event);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    "Ransomware watcher: failed to create filesystem watcher: {}",
                    e
                );
                return;
            }
        };

    let mut entries: HashMap<String, ProtectedEntry> = HashMap::new();
    let repo = RansomwareRepository::new(state.db.pool());
    match repo.list_protected_paths().await {
        Ok(paths) => {
            for p in paths {
                let path = PathBuf::from(&p.path);
                if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
                    tracing::warn!("Ransomware watcher: failed to watch '{}': {}", p.path, e);
                    continue;
                }
                entries.insert(
                    p.id,
                    ProtectedEntry {
                        path,
                        honeypot_filename: p.honeypot_filename,
                    },
                );
            }
        }
        Err(e) => tracing::warn!("Ransomware watcher: failed to load protected paths: {}", e),
    }

    let mut cached = load_cached_settings(&repo).await;
    let mut windows: HashMap<String, VecDeque<Instant>> = HashMap::new();
    // Debounces repeated honeypot_touched firings for the same protected
    // path. Verified live: a single `>>` append to a file reliably produces
    // two raw notify events (a data-modify and a close-write) a millisecond
    // apart, which without this would record two events and send two
    // "critical" notifications for one logical touch.
    let mut honeypot_debounce: HashMap<String, Instant> = HashMap::new();
    const HONEYPOT_DEBOUNCE: Duration = Duration::from_secs(10);

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(WatchCommand::AddPath { id, path, honeypot_filename }) => {
                        if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
                            tracing::warn!("Ransomware watcher: failed to watch '{}': {}", path.display(), e);
                        } else {
                            entries.insert(id, ProtectedEntry { path, honeypot_filename });
                        }
                    }
                    Some(WatchCommand::RemovePath { id }) => {
                        if let Some(entry) = entries.remove(&id) {
                            let _ = watcher.unwatch(&entry.path);
                        }
                        windows.remove(&id);
                        honeypot_debounce.remove(&id);
                    }
                    Some(WatchCommand::ReloadSettings) => {
                        cached = load_cached_settings(&repo).await;
                    }
                    None => break,
                }
            }
            event = event_rx.recv() => {
                let Some(event) = event else { break };
                handle_event(&state, &entries, &mut windows, &mut honeypot_debounce, HONEYPOT_DEBOUNCE, &cached, &event).await;
            }
        }
    }
}

async fn load_cached_settings(repo: &RansomwareRepository) -> CachedSettings {
    match repo.get_settings().await {
        Ok(s) => CachedSettings {
            rate_threshold_events: s.rate_threshold_events.max(1) as usize,
            rate_threshold_seconds: s.rate_threshold_seconds.max(1) as u64,
        },
        Err(_) => CachedSettings {
            rate_threshold_events: 50,
            rate_threshold_seconds: 10,
        },
    }
}

/// Find the protected-path entry whose watched root is an ancestor of the
/// event's path (there are only ever a handful of protected paths, so a
/// linear scan is fine).
fn find_owning_entry<'a>(
    entries: &'a HashMap<String, ProtectedEntry>,
    event_path: &Path,
) -> Option<(&'a str, &'a ProtectedEntry)> {
    entries
        .iter()
        .find(|(_, e)| event_path.starts_with(&e.path))
        .map(|(id, e)| (id.as_str(), e))
}

fn classify_event(kind: &EventKind) -> Option<&'static str> {
    match kind {
        EventKind::Remove(_) => Some("mass_delete"),
        EventKind::Modify(notify::event::ModifyKind::Name(_)) => Some("mass_rename"),
        EventKind::Modify(_) | EventKind::Create(_) => Some("mass_modify"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_event(
    state: &Arc<AppState>,
    entries: &HashMap<String, ProtectedEntry>,
    windows: &mut HashMap<String, VecDeque<Instant>>,
    honeypot_debounce: &mut HashMap<String, Instant>,
    honeypot_debounce_window: Duration,
    cached: &CachedSettings,
    event: &Event,
) {
    for event_path in &event.paths {
        let Some((protected_id, entry)) = find_owning_entry(entries, event_path) else {
            continue;
        };

        let filename = event_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string());
        if let (Some(honeypot), Some(name)) = (&entry.honeypot_filename, &filename) {
            if honeypot == name {
                let now = Instant::now();
                let already_fired = honeypot_debounce
                    .get(protected_id)
                    .is_some_and(|last| now.duration_since(*last) < honeypot_debounce_window);
                honeypot_debounce.insert(protected_id.to_string(), now);
                if already_fired {
                    continue;
                }
                let username = guess_owner(event_path);
                let detail = format!(
                    "Honeypot file '{}' was touched in '{}'.",
                    honeypot,
                    entry.path.display()
                );
                crate::events::log_event_warn(format!(
                    "Ransomware protection alert: {}{}",
                    detail,
                    username
                        .as_deref()
                        .map(|u| format!(" (by user '{}')", u))
                        .unwrap_or_default()
                ));
                if let Err(e) = ransomware::record_event(
                    state,
                    Some(protected_id),
                    "honeypot_touched",
                    Some(&detail),
                    username.as_deref(),
                )
                .await
                {
                    tracing::warn!("Ransomware watcher: failed to record honeypot event: {}", e);
                }
                continue;
            }
        }

        let Some(event_type) = classify_event(&event.kind) else {
            continue;
        };

        let now = Instant::now();
        let window = windows.entry(protected_id.to_string()).or_default();
        let cutoff = Duration::from_secs(cached.rate_threshold_seconds);
        while let Some(front) = window.front() {
            if now.duration_since(*front) > cutoff {
                window.pop_front();
            } else {
                break;
            }
        }
        window.push_back(now);

        if window.len() >= cached.rate_threshold_events {
            window.clear(); // avoid re-firing on every subsequent event in the same burst
            let username = guess_owner(event_path);
            let detail = format!(
                "{} file events on '{}' within {}s (threshold: {}).",
                cached.rate_threshold_events,
                entry.path.display(),
                cached.rate_threshold_seconds,
                cached.rate_threshold_events
            );
            crate::events::log_event_warn(format!(
                "Ransomware protection alert: {}{}",
                detail,
                username
                    .as_deref()
                    .map(|u| format!(" (by user '{}')", u))
                    .unwrap_or_default()
            ));
            if let Err(e) = ransomware::record_event(
                state,
                Some(protected_id),
                event_type,
                Some(&detail),
                username.as_deref(),
            )
            .await
            {
                tracing::warn!("Ransomware watcher: failed to record rate event: {}", e);
            }
        }
    }
}

/// Best-effort file-owner lookup for auto-response attribution. Returns
/// `None` if the file no longer exists (e.g. a delete event) or the UID
/// doesn't resolve to a known account — see `ransomware::resolve_username_from_uid`.
fn guess_owner(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = std::fs::metadata(path).ok()?.uid();
        ransomware::resolve_username_from_uid(uid)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for event classification and protected-path
    //! ownership matching. The full watcher loop (honeypot debounce, rate
    //! window, auto-response) needs a real filesystem and `AppState` and is
    //! covered by `tests/e2e/security/security.spec.ts`.

    use super::*;
    use notify::event::{CreateKind, MetadataKind, ModifyKind, RemoveKind, RenameMode};

    #[test]
    fn test_classify_event_delete() {
        assert_eq!(
            classify_event(&EventKind::Remove(RemoveKind::File)),
            Some("mass_delete")
        );
    }

    #[test]
    fn test_classify_event_rename() {
        assert_eq!(
            classify_event(&EventKind::Modify(ModifyKind::Name(RenameMode::Both))),
            Some("mass_rename")
        );
    }

    #[test]
    fn test_classify_event_modify_and_create_are_mass_modify() {
        assert_eq!(
            classify_event(&EventKind::Modify(ModifyKind::Data(
                notify::event::DataChange::Content
            ))),
            Some("mass_modify")
        );
        assert_eq!(
            classify_event(&EventKind::Create(CreateKind::File)),
            Some("mass_modify")
        );
    }

    #[test]
    fn test_classify_event_ignores_access_and_metadata_only() {
        assert_eq!(
            classify_event(&EventKind::Access(notify::event::AccessKind::Read)),
            None
        );
        assert_eq!(
            classify_event(&EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::Permissions
            ))),
            Some("mass_modify"),
            "a permissions change is still a Modify(_) and counts toward the burst"
        );
    }

    fn entry(path: &str, honeypot: Option<&str>) -> ProtectedEntry {
        ProtectedEntry {
            path: PathBuf::from(path),
            honeypot_filename: honeypot.map(String::from),
        }
    }

    #[test]
    fn test_find_owning_entry_matches_the_ancestor_protected_path() {
        let mut entries = HashMap::new();
        entries.insert("p1".to_string(), entry("/srv/share-a", None));
        entries.insert("p2".to_string(), entry("/srv/share-b", None));

        let hit = find_owning_entry(&entries, Path::new("/srv/share-b/docs/report.docx"));
        assert_eq!(hit.map(|(id, _)| id), Some("p2"));
    }

    #[test]
    fn test_find_owning_entry_returns_none_for_unwatched_path() {
        let mut entries = HashMap::new();
        entries.insert("p1".to_string(), entry("/srv/share-a", None));
        assert!(find_owning_entry(&entries, Path::new("/home/someone/file")).is_none());
    }

    #[test]
    fn test_find_owning_entry_exact_root_match() {
        let mut entries = HashMap::new();
        entries.insert("p1".to_string(), entry("/srv/share-a", Some("decoy.zip")));
        let hit = find_owning_entry(&entries, Path::new("/srv/share-a"));
        assert_eq!(hit.map(|(id, _)| id), Some("p1"));
    }
}
