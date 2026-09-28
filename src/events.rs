//! Centralized "event log" helper for user-facing, plain-English audit
//! events (login, user/group/share changes, Docker actions, backup runs,
//! security alerts, etc.).
//!
//! These are written through the same `tracing` pipeline as every other
//! log line in this codebase, so they flow into the System Journal app's
//! "app" source (see `src/handlers/journal.rs`) with no new infrastructure
//! — the only thing that sets them apart is a fixed `target` (so they're
//! easy to grep for) and a convention: always a complete, plain-English
//! sentence naming the actor and the thing that happened, never a raw
//! struct dump or a terse technical fragment.
//!
//! Always English, regardless of the active UI language — this matches
//! the project-wide precedent of leaving log/check-style content
//! untranslated (see CLAUDE.md's Health/Downloader/Syncthing sections);
//! only this page's own UI chrome is localized.
//!
//! Callers should read as a sentence, e.g.:
//! `log_event(format!("User '{username}' logged in successfully"))`.

/// Target used for every event logged through this module, so they can be
/// filtered/greped independently of routine internal `tracing` calls.
pub const EVENT_TARGET: &str = "homevault::event";

/// Record a plain-English application event at info level (the normal
/// case: something happened successfully).
pub fn log_event(message: impl std::fmt::Display) {
    tracing::info!(target: EVENT_TARGET, "{}", message);
}

/// Record a plain-English application event at warn level — failed logins,
/// detected threats, degraded arrays, and other events that succeeded as
/// an *operation* (nothing crashed) but represent something an admin
/// should notice.
pub fn log_event_warn(message: impl std::fmt::Display) {
    tracing::warn!(target: EVENT_TARGET, "{}", message);
}
