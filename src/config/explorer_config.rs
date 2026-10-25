/* =============================================================================
 * File          : explorer_config.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.1
 *
 * Description:
 *   Defines the directories the file Explorer is allowed to browse (default
 *   roots overridable via HOMEVAULT_EXPLORER_ROOTS, with automatic detection
 *   of mounted filesystems) and PathGuard, the security check that validates
 *   every file path against those roots — resolving symlinks and refusing
 *   anything that escapes them (../ traversal, absolute paths outside the
 *   roots, null bytes).
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *   - tracing (>= 0.1)
 *
 * Usage:
 *   `PathGuard::default()` is built once at startup in src/web/mod.rs;
 *   handlers call `PathGuard::validate(path)` before touching any file.
 * =============================================================================
 */

//! Which directories the file Explorer may browse, and the path-traversal
//! guard that keeps every file operation inside those roots.

use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

/// File explorer settings controlling which directories can be browsed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorerSettings {
    /// List of allowed root directories for file browsing
    pub allowed_roots: Vec<String>,
}

impl Default for ExplorerSettings {
    fn default() -> Self {
        Self {
            allowed_roots: vec![
                "/home".to_string(),
                crate::config::paths::DEFAULT_BASE_DIR.to_string(),
                crate::config::paths::RAID_MOUNT_BASE.to_string(),
                crate::config::paths::POOLS_MOUNT_BASE.to_string(),
                crate::config::paths::ENCRYPTED_MOUNT_BASE.to_string(),
                crate::config::paths::RCLONE_MOUNT_BASE.to_string(),
                "/media".to_string(),
                "/mnt".to_string(),
                "/srv".to_string(),
            ],
        }
    }
}

impl ExplorerSettings {
    /// Load explorer roots from env var or auto-detect mount points.
    pub fn load() -> Self {
        let mut settings = Self::default();

        if let Ok(roots) = env::var("HOMEVAULT_EXPLORER_ROOTS") {
            settings.allowed_roots = roots
                .split(':')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }

        // Auto-detect common mount points if no custom roots are configured
        if settings
            .allowed_roots
            .iter()
            .all(|r| !Path::new(r).exists())
        {
            let auto_roots = Self::detect_mount_points();
            if !auto_roots.is_empty() {
                tracing::info!("Auto-detected mount points for explorer: {:?}", auto_roots);
                settings.allowed_roots = auto_roots;
            }
        }

        Self {
            allowed_roots: settings
                .allowed_roots
                .into_iter()
                .filter(|root| {
                    let p = Path::new(root);
                    p.is_absolute() && p.exists() && p.is_dir()
                })
                .collect(),
        }
    }

    /// Scan the filesystem for mount points that can be browsed.
    fn detect_mount_points() -> Vec<String> {
        let mut mounts: Vec<String> = Vec::new();

        // Common user data locations
        let candidates = [
            "/home",
            crate::config::paths::DEFAULT_BASE_DIR,
            "/var/lib/docker",
            crate::config::paths::RAID_MOUNT_BASE,
            crate::config::paths::POOLS_MOUNT_BASE,
            crate::config::paths::ENCRYPTED_MOUNT_BASE,
            crate::config::paths::RCLONE_MOUNT_BASE,
            "/media",
            "/mnt",
            "/srv",
            "/run/live/persistence",
            "/run/user",
            "/tmp",
        ];

        for candidate in candidates {
            let p = Path::new(candidate);
            if p.exists() && p.is_dir() {
                // Ensure the directory is readable by the current process.
                if std::fs::read_dir(p).is_ok() {
                    mounts.push(candidate.to_string());
                }
            }
        }

        // Also parse /proc/mounts for real mounts
        if let Ok(content) = std::fs::read_to_string("/proc/mounts") {
            for line in content.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let mount_point = parts[1];
                    // Only include real filesystem mounts (not pseudo filesystems)
                    let fs_type = parts.get(2).unwrap_or(&"");
                    let skip_types = [
                        "sysfs",
                        "proc",
                        "devtmpfs",
                        "devpts",
                        "tmpfs",
                        "securityfs",
                        "cgroup",
                        "cgroup2",
                        "pstore",
                        "bpf",
                        "tracefs",
                        "debugfs",
                        "hugetlbfs",
                        "mqueue",
                        "fusectl",
                        "configfs",
                        "ramfs",
                        "fuse.portal",
                        "autofs",
                        "binfmt_misc",
                        "overlay",
                        "rpc_pipefs",
                        "nfsd",
                        "procfs",
                        "sysfs",
                        "devpts",
                    ];
                    if !skip_types.contains(fs_type) {
                        let mp = Path::new(mount_point);
                        if mp.exists()
                            && mp.is_dir()
                            && !mounts.contains(&mount_point.to_string())
                            && std::fs::read_dir(mp).is_ok()
                        {
                            mounts.push(mount_point.to_string());
                        }
                    }
                }
            }
        }

        // Remove duplicates and sort
        mounts.sort();
        mounts.dedup();
        mounts
    }
}

/// PathGuard validates that file paths stay within allowed roots.
/// This prevents path traversal attacks (../, symlinks outside roots, etc.)
#[derive(Clone)]
pub struct PathGuard {
    allowed_roots: Vec<PathBuf>,
}

impl PathGuard {
    pub fn new(roots: Vec<String>) -> Self {
        let allowed_roots: Vec<PathBuf> = roots
            .into_iter()
            .map(|r| {
                let p = PathBuf::from(&r);
                // Canonicalize to resolve symlinks and normalize paths
                std::fs::canonicalize(&p).unwrap_or(p)
            })
            .collect();

        Self { allowed_roots }
    }

    /// Validates that the given path is within an allowed root directory.
    /// Returns the canonical (resolved) path on success.
    /// For non-existent paths (e.g., new files), validates the parent directory.
    /// Returns None on failure (path traversal attempt).
    pub fn validate(&self, path: &str) -> Option<PathBuf> {
        let p = Path::new(path);

        // Reject empty or obviously malicious paths
        if path.is_empty() || path.contains("\0") {
            return None;
        }

        // For relative paths, try to resolve them relative to allowed roots
        if !p.is_absolute() {
            // Try each allowed root and return the first valid resolved path
            for root in &self.allowed_roots {
                let candidate = root.join(p);
                if let Ok(resolved) = std::fs::canonicalize(&candidate) {
                    if self.is_under_root(&resolved) {
                        return Some(resolved);
                    }
                }
                // Also try validating the parent directory for new files
                if let Some(parent) = candidate.parent() {
                    if parent == root || parent.to_string_lossy() == root.to_string_lossy() {
                        // Parent is the root itself
                        if self.is_under_root(&root.to_path_buf()) {
                            return Some(candidate);
                        }
                    }
                    if let Ok(parent_resolved) = std::fs::canonicalize(parent) {
                        if self.is_under_root(&parent_resolved) {
                            return self.resolve_final_component(&parent_resolved, &candidate);
                        }
                    }
                }
            }
            return None;
        }

        // For absolute paths, check they start with an allowed root
        // First try canonicalizing the full path (works if it exists)
        if let Ok(resolved) = std::fs::canonicalize(p) {
            if self.is_under_root(&resolved) {
                return Some(resolved);
            }
            return None;
        }

        // Path doesn't exist yet (new file): validate the parent **and** make
        // the final component definite.
        if let Some(parent) = p.parent() {
            if let Ok(parent_resolved) = std::fs::canonicalize(parent) {
                if self.is_under_root(&parent_resolved) {
                    return self.resolve_final_component(&parent_resolved, p);
                }
            }
        }

        None
    }

    /// Splits the last component of `p` off, refuses anything that could move
    /// the result (`..`, a root), re-reads the leaf with `symlink_metadata` and
    /// returns the joined path with the leaf as the filesystem currently
    /// reports it.
    ///
    /// This is what closes the TOCTOU window of a path whose leaf does not
    /// exist *yet* (ROADMAP 10.5). Returning the caller's string, as this used
    /// to, meant the actual `create`/`rename` could land anywhere the leaf
    /// name pointed to at that instant — including through a symlink planted
    /// after validation. Re-reading the leaf narrows the window to the writer
    /// itself (a device-write path can only be closed with `O_NOFOLLOW` on the
    /// final `open`), which is a *writer* concern, not a path-resolution one.
    fn resolve_final_component(&self, parent_resolved: &Path, p: &Path) -> Option<PathBuf> {
        let leaf = p.file_name()?;
        if leaf == "." || leaf == ".." || leaf.to_string_lossy().contains('\0') {
            return None;
        }
        let candidate = parent_resolved.join(leaf);
        // `symlink_metadata` does not follow the leaf, so a symlink is kept as
        // a symlink and its *target* decides the answer below.
        match std::fs::symlink_metadata(&candidate) {
            Ok(meta) if meta.file_type().is_symlink() => {
                // A symlink pointing outside the roots is refused outright,
                // even though the parent is fine.
                let target = std::fs::canonicalize(&candidate).ok()?;
                if self.is_under_root(&target) {
                    Some(target)
                } else {
                    None
                }
            }
            // A regular leaf that appeared between the parent check and now —
            // resolve it, and re-check the root.
            Ok(_) => {
                let resolved = std::fs::canonicalize(&candidate).ok()?;
                if self.is_under_root(&resolved) {
                    Some(resolved)
                } else {
                    None
                }
            }
            // Still absent: the joined path with the resolved parent.
            Err(_) => Some(candidate),
        }
    }

    /// Check if the resolved path is under any allowed root
    fn is_under_root(&self, path: &PathBuf) -> bool {
        self.allowed_roots
            .iter()
            .any(|root| path.starts_with(root) || path == root)
    }

    /// Get the list of allowed roots (for UI display)
    pub fn allowed_roots(&self) -> &[PathBuf] {
        &self.allowed_roots
    }
}

impl Default for PathGuard {
    fn default() -> Self {
        Self::new(ExplorerSettings::load().allowed_roots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_guard() -> (PathGuard, TempDir) {
        let tmp = TempDir::new().unwrap();
        let guard = PathGuard::new(vec![tmp.path().to_string_lossy().to_string()]);
        (guard, tmp)
    }

    #[test]
    fn test_path_guard_blocks_traversal() {
        let (guard, _tmp) = create_guard();
        let root = _tmp.path().to_string_lossy().to_string();

        // Path traversal attempts should be blocked
        assert!(guard.validate(&format!("{}/../etc/passwd", root)).is_none());
        assert!(guard.validate("/etc/shadow").is_none());
        assert!(guard
            .validate(&format!("{}/../../root/.ssh", root))
            .is_none());
    }

    #[test]
    fn test_path_guard_allows_valid_paths() {
        let (guard, tmp) = create_guard();
        let root = tmp.path().to_string_lossy();
        std::fs::create_dir_all(tmp.path().join("subdir")).unwrap();

        // Valid paths under allowed root should pass
        assert!(guard.validate(&root).is_some());
        assert!(guard
            .validate(&format!("{}/subdir/file.txt", root))
            .is_some());
    }

    #[test]
    fn test_path_guard_multiple_roots() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let guard = PathGuard::new(vec![
            tmp1.path().to_string_lossy().to_string(),
            tmp2.path().to_string_lossy().to_string(),
        ]);

        assert!(guard.validate(&tmp1.path().to_string_lossy()).is_some());
        assert!(guard.validate(&tmp2.path().to_string_lossy()).is_some());
        assert!(guard.validate("/var/log").is_none());
    }

    #[test]
    fn test_path_guard_rejects_null_bytes() {
        let (guard, _tmp) = create_guard();
        assert!(guard.validate("/some/path\0malicious").is_none());
        assert!(guard.validate("").is_none());
    }

    // -----------------------------------------------------------------
    // ROADMAP 10.5 — the final component is resolved, not echoed back
    // -----------------------------------------------------------------

    /// The defect 10.5 named: a path whose leaf does not exist yet used to be
    /// returned **verbatim**, so `..` survived validation and the leaf was
    /// whatever the caller typed. It must come back under the *resolved*
    /// parent, with no traversal component left in it.
    #[test]
    fn a_new_file_is_returned_under_its_resolved_parent() {
        let (guard, tmp) = create_guard();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let parent = std::fs::canonicalize(tmp.path().join("sub")).unwrap();

        let typed = format!("{}/sub/../sub/new.txt", tmp.path().to_string_lossy());
        let got = guard.validate(&typed).expect("a new file under the root");
        assert_eq!(got, parent.join("new.txt"));
        assert!(
            !got.to_string_lossy().contains(".."),
            "the returned path must be normalised: {got:?}"
        );
    }

    /// A symlink planted at the leaf must not smuggle the write outside the
    /// roots — this is the exact shape of the TOCTOU 10.5 describes.
    #[cfg(unix)]
    #[test]
    fn a_symlink_leaf_pointing_outside_the_root_is_refused() {
        let (guard, tmp) = create_guard();
        let outside = TempDir::new().unwrap();
        let victim = outside.path().join("victim.txt");
        std::fs::write(&victim, b"original").unwrap();

        let link = tmp.path().join("escape.txt");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        assert!(
            guard.validate(&link.to_string_lossy()).is_none(),
            "a symlink whose target is outside the roots must be refused"
        );
    }

    /// A symlink that stays inside the roots is resolved to its target, so the
    /// caller writes to the real file rather than through the link.
    #[cfg(unix)]
    #[test]
    fn a_symlink_leaf_inside_the_root_resolves_to_its_target() {
        let (guard, tmp) = create_guard();
        let real = tmp.path().join("real.txt");
        std::fs::write(&real, b"content").unwrap();
        let link = tmp.path().join("alias.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let got = guard
            .validate(&link.to_string_lossy())
            .expect("inside root");
        assert_eq!(got, std::fs::canonicalize(&real).unwrap());
    }

    /// A `.`/`..` leaf is refused outright: it is a directory-shaped request,
    /// not a file to create.
    #[test]
    fn a_dot_or_dotdot_leaf_is_refused() {
        let (guard, tmp) = create_guard();
        for leaf in [".", ".."] {
            let path = format!("{}/sub/{}", tmp.path().to_string_lossy(), leaf);
            std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
            assert!(guard.validate(&path).is_none(), "leaf {leaf:?}");
        }
    }

    /// The relative branch gets the same treatment: the returned path is built
    /// from the resolved parent, not from `root.join(input)`.
    #[test]
    fn a_relative_new_file_is_returned_under_the_root() {
        let (guard, tmp) = create_guard();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let got = guard.validate("new.txt").expect("new file at the root");
        assert_eq!(got, root.join("new.txt"));
    }
}
