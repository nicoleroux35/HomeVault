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
        if settings.allowed_roots.iter().all(|r| !Path::new(r).exists()) {
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
                        "sysfs", "proc", "devtmpfs", "devpts", "tmpfs", "securityfs",
                        "cgroup", "cgroup2", "pstore", "bpf", "tracefs", "debugfs",
                        "hugetlbfs", "mqueue", "fusectl", "configfs", "ramfs",
                        "fuse.portal", "autofs", "binfmt_misc", "overlay", "rpc_pipefs",
                        "nfsd", "procfs", "sysfs", "devpts"
                    ];
                    if !skip_types.contains(fs_type) {
                        let mp = Path::new(mount_point);
                        if mp.exists() && mp.is_dir() && !mounts.contains(&mount_point.to_string())
                            && std::fs::read_dir(mp).is_ok() {
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
                            return Some(candidate);
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

        // Path doesn't exist (new file). Validate the parent directory.
        if let Some(parent) = p.parent() {
            if let Ok(parent_resolved) = std::fs::canonicalize(parent) {
                if self.is_under_root(&parent_resolved) {
                    // Return the original path for non-existent files
                    return Some(PathBuf::from(path));
                }
            }
        }

        None
    }

    /// Check if the resolved path is under any allowed root
    fn is_under_root(&self, path: &PathBuf) -> bool {
        self.allowed_roots.iter().any(|root| {
            path.starts_with(root) || path == root
        })
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
        assert!(guard.validate(&format!("{}/../../root/.ssh", root)).is_none());
    }

    #[test]
    fn test_path_guard_allows_valid_paths() {
        let (guard, tmp) = create_guard();
        let root = tmp.path().to_string_lossy();
        std::fs::create_dir_all(tmp.path().join("subdir")).unwrap();

        // Valid paths under allowed root should pass
        assert!(guard.validate(&root).is_some());
        assert!(guard.validate(&format!("{}/subdir/file.txt", root)).is_some());
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
}
