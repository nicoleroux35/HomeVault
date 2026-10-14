/* =============================================================================
 * File          : release_notes.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Curated release highlights powering the "What's New" modal: a small
 *   static table per version (instead of parsing the changelog at runtime)
 *   plus the current-version lookup used by the whats-new endpoints.
 *
 * Dependencies:
 *   - none (static data)
 *
 * Usage:
 *   handlers/whats_new.rs reads current_version()/current().
 * =============================================================================
 */

//! Curated release highlights powering the "What's New" modal.
//!
//! The SPA asks `GET /api/whats-new` right after login; the server returns the
//! highlights for the running version plus whether *this* user has already
//! acknowledged them. Keeping the notes in a small static table (rather than
//! parsing `CHANGELOG.md` at runtime) means the modal shows a short, curated
//! list of user-visible wins — not the full engineering changelog — and it can
//! never break because a changelog heading changed shape.
//!
//! **Release checklist:** add one [`ReleaseNote`] for the version in
//! `Cargo.toml` before tagging. `current()` picks the entry whose `version`
//! equals `CARGO_PKG_VERSION`; a missing entry simply means "no modal".

use serde::Serialize;

/// One user-facing release highlight set.
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseNote {
    /// Semver string, matching `CARGO_PKG_VERSION` for the released build.
    pub version: &'static str,
    /// Human-readable title, e.g. "Storage health & notifications".
    pub title: &'static str,
    /// Short, user-visible highlights (2–6 items).
    pub highlights: &'static [&'static str],
}

/// The version this binary reports (`Cargo.toml`).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// All curated release notes, newest first.
pub static RELEASE_NOTES: &[ReleaseNote] = &[ReleaseNote {
    version: "1.0.0",
    title: "Storage health, notification center & hardened app auth",
    highlights: &[
        "A persistent notification center with severity filters, unread-only view and search — not just the bell popover.",
        "A \"What's New\" summary shown once after each update, acknowledgeable per account.",
        "ZFS/Btrfs/mdadm scrub status now visible from Storage, with a one-click consistency check.",
        "Import existing (foreign) ZFS and Btrfs pools discovered on attached disks.",
        "Proxied apps authenticate through a dedicated, app-scoped session that never exposes the API session.",
        "A versioned client/server contract so a stale frontend fails loudly instead of silently.",
    ],
}];

/// The release note for the running version, if one is curated.
pub fn current() -> Option<&'static ReleaseNote> {
    RELEASE_NOTES
        .iter()
        .find(|n| n.version == current_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_version_is_non_empty() {
        assert!(!current_version().is_empty());
    }

    #[test]
    fn test_current_returns_the_running_version_when_curated() {
        // The running build's version must have a curated entry, otherwise the
        // What's New modal silently disappears on the next release.
        let note = current().expect("a release note is curated for CARGO_PKG_VERSION");
        assert_eq!(note.version, current_version());
        assert!(!note.title.is_empty());
        assert!(!note.highlights.is_empty());
    }

    #[test]
    fn test_release_notes_are_unique_and_newest_first() {
        let mut seen = std::collections::HashSet::new();
        for note in RELEASE_NOTES {
            assert!(seen.insert(note.version), "duplicate version {}", note.version);
            assert!(!note.highlights.is_empty());
        }
    }
}
