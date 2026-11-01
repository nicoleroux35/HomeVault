/* =============================================================================
 * File          : trash.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Canonical Trash locations and the retention pass. Both the web Explorer
 *   and the background scheduler resolve trash folders through here, so a
 *   file deleted from a network share and a file deleted from the web UI are
 *   found, listed and cleaned up in exactly the same places.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   handlers/explorer.rs (trash endpoints) and trash_scheduler.rs.
 * =============================================================================
 */

//! Canonical Trash locations and the shared retention pass.
//!
//! A network share's recycle folder and the web Explorer's trash folder are
//! deliberately the *same* name ([`crate::browser::TRASH_DIR_NAME`]), and this
//! module is the single place that lists those folders and purges them, so the
//! two deletion paths never diverge.

use crate::browser::{FileBrowser, TrashPolicy, TrashPurgeReport, TRASH_DIR_NAME};
use crate::handlers::AppState;
use crate::shares::SharesService;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The trash directory an item at `path` belongs to, given the roots a session
/// may browse: the one at the top of whichever allowed root contains it, so
/// trash stays visible and manageable inside the same tree the user already
/// accesses. Anything outside every configured root (an admin browsing a
/// system directory) falls back to a single app-owned folder rather than
/// scattering trash across `/etc` or `/root`.
///
/// Pure — the caller supplies the roots — so the mapping is unit-testable
/// without an `AppState`.
pub fn root_for(allowed_roots: &[PathBuf], path: &str) -> PathBuf {
    let p = Path::new(path);
    for root in allowed_roots {
        if p.starts_with(root) {
            return root.join(TRASH_DIR_NAME);
        }
    }
    crate::config::paths::base_dir().join("trash")
}

/// Every trash directory that is part of the appliance's recoverable set, from
/// its parts: one per allowed root, one per configured SMB share (its network
/// clients recycle into the same folder the Explorer reads), plus — for an
/// admin — the app-owned fallback for browsing outside every configured root.
///
/// The share folders are included regardless of `is_admin` so a non-admin
/// still sees and can restore what they deleted over the network. Pure.
pub fn roots_from(allowed_roots: &[PathBuf], share_paths: &[PathBuf], is_admin: bool) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = allowed_roots
        .iter()
        .map(|r| r.join(TRASH_DIR_NAME))
        .collect();

    for share_path in share_paths {
        let candidate = share_path.join(TRASH_DIR_NAME);
        if !roots.contains(&candidate) {
            roots.push(candidate);
        }
    }

    if is_admin {
        roots.push(crate::config::paths::base_dir().join("trash"));
    }
    roots
}

/// [`root_for`] against the live explorer roots.
pub fn root_for_state(state: &Arc<AppState>, path: &str) -> PathBuf {
    root_for(state.path_guard.allowed_roots(), path)
}

/// [`roots_from`] against the live explorer roots and the configured SMB
/// shares. Reads `smb.conf`, so call it from a blocking context.
pub fn roots_for_state(state: &Arc<AppState>, is_admin: bool) -> Vec<PathBuf> {
    roots_from(
        state.path_guard.allowed_roots(),
        &SharesService::smb_share_paths(),
        is_admin,
    )
}

/// Applies `policy` to every trash root on the appliance and reports what was
/// reclaimed. Runs the scheduler's view of the trash (the admin set), so a
/// forgotten app-owned fallback folder is cleaned up too. Blocking: call it
/// from `spawn_blocking` or the scheduler's own thread, never inline on the
/// async runtime.
pub fn purge_all(state: &Arc<AppState>, policy: &TrashPolicy) -> TrashPurgeReport {
    let mut report = TrashPurgeReport::default();
    for root in roots_for_state(state, true) {
        let partial = FileBrowser::purge_trash(&root, policy);
        report.purged += partial.purged;
        report.freed_bytes = report.freed_bytes.saturating_add(partial.freed_bytes);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/home"),
            crate::config::paths::base_dir().to_path_buf(),
        ]
    }

    #[test]
    fn test_root_for_prefers_the_containing_allowed_root() {
        let base = crate::config::paths::base_dir();
        let path = base.join("photos/cat.jpg");
        assert_eq!(
            root_for(&roots(), &path.to_string_lossy()),
            base.join(TRASH_DIR_NAME)
        );
    }

    #[test]
    fn test_root_for_picks_the_first_matching_root() {
        assert_eq!(
            root_for(&roots(), "/home/alice/doc.txt"),
            PathBuf::from("/home").join(TRASH_DIR_NAME)
        );
    }

    #[test]
    fn test_root_for_falls_back_outside_every_allowed_root() {
        assert_eq!(
            root_for(&roots(), "/proc/self/status"),
            crate::config::paths::base_dir().join("trash")
        );
    }

    #[test]
    fn test_roots_from_covers_allowed_roots_and_app_fallback_for_admin() {
        let out = roots_from(&[PathBuf::from("/home")], &[], true);
        assert!(out.contains(&PathBuf::from("/home").join(TRASH_DIR_NAME)));
        assert!(out.contains(&crate::config::paths::base_dir().join("trash")));
    }

    #[test]
    fn test_roots_from_adds_smb_share_recycle_folders_for_everyone() {
        // A share root outside the explorer roots still contributes a trash
        // folder, and a non-admin gets it too.
        let share = PathBuf::from("/mnt/pool/media");
        let out = roots_from(&[PathBuf::from("/home")], &[share.clone()], false);
        assert!(out.contains(&share.join(TRASH_DIR_NAME)));
        assert!(!out.contains(&crate::config::paths::base_dir().join("trash")));
    }

    #[test]
    fn test_roots_from_deduplicates_a_share_under_an_allowed_root() {
        let home = PathBuf::from("/home");
        let out = roots_from(&[home.clone()], &[home.clone()], false);
        let expected = home.join(TRASH_DIR_NAME);
        assert_eq!(out.iter().filter(|r| **r == expected).count(), 1);
    }
}
