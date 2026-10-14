/* =============================================================================
 * File          : browser.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   File-browser engine behind the Explorer: directory listings with
 *   typed file info, previews, fuzzy search scoring, trash with
 *   non-colliding restore paths, and a small per-mount filesystem-type
 *   cache. All operations stay within the caller's validated roots.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   handlers/explorer.rs (list/search/trash/preview endpoints).
 * =============================================================================
 */

//! File-browser engine behind the Explorer (listings, search, trash,
//! previews).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tokio::sync::RwLock;

/// Name of the per-root trash directory files are moved into on delete.
pub const TRASH_DIR_NAME: &str = ".homevault-trash";

/// Hard cap on how many entries a single directory listing returns. A
/// directory with hundreds of thousands of files would otherwise build a
/// huge JSON payload and an equally huge DOM in the browser. When the cap
/// is hit, `DirectoryListing::truncated` is set so the UI can say so.
pub const MAX_DIR_ENTRIES: usize = 10_000;

/// Junk metadata files other operating systems scatter across shared
/// folders — never useful to show in the browser.
fn is_os_junk_file(name: &str) -> bool {
    matches!(
        name,
        ".DS_Store"
            | ".directory"
            | ".localized"
            | "Thumbs.db"
            | "desktop.ini"
            | ".Spotlight-V100"
            | ".Trashes"
            | ".fseventsd"
            | ".TemporaryItems"
            | ".apdisk"
    ) || name.starts_with("._") // AppleDouble resource-fork sidecars
}

/// How a copy / move / upload should treat an existing destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionStrategy {
    /// Overwrite whatever is already there (the historical behaviour).
    Overwrite,
    /// Write alongside it as `name (1).ext`, `name (2).ext`, …
    KeepBoth,
    /// Leave the destination untouched and report the item as skipped.
    Skip,
}

impl CollisionStrategy {
    /// Parses the `?strategy=` query value; anything unrecognised (including
    /// `None`) means `Overwrite`, so existing callers are unaffected.
    pub fn parse(s: Option<&str>) -> Self {
        match s
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_")
            .as_str()
        {
            "keep_both" | "keepboth" => Self::KeepBoth,
            "skip" => Self::Skip,
            _ => Self::Overwrite,
        }
    }
}

/// Subsequence (fuzzy) match score of `query` against a file `name`.
///
/// Both inputs must already be lowercased. Returns 0 when `query` is empty or
/// is not a subsequence of `name`; otherwise a positive score where larger is
/// better: every matched character scores a base, with bonuses for contiguity
/// and word-boundary starts, minus a length penalty so short names win ties.
pub fn fuzzy_score(query: &str, name: &str) -> u64 {
    if query.is_empty() || query.len() > name.len() {
        return 0;
    }
    let q: Vec<char> = query.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let mut qi = 0usize;
    let mut raw: u64 = 0;
    let mut prev_match: Option<usize> = None;
    for (i, c) in n.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            let mut s: u64 = 10;
            if i == 0 || matches!(n[i - 1], '/' | '_' | '-' | '.' | ' ') {
                s += 10;
            }
            if i > 0 && prev_match == Some(i - 1) {
                s += 15;
            }
            raw += s;
            prev_match = Some(i);
            qi += 1;
        }
    }
    if qi != q.len() {
        return 0;
    }
    // Length penalty keeps exact/short names above long incidental matches.
    raw.saturating_mul(1000).saturating_add(1) / (1000 + n.len() as u64)
}

/// Returns a path that does not collide with an existing file/dir by
/// inserting ` (N)` before the extension — Finder / GNOME Files style.
/// `foo.txt` → `foo (1).txt`, `archive.tar.gz` → `archive.tar (1).gz`,
/// `.env` → `.env (1)`. If `dest` is already free it is returned unchanged.
pub fn non_colliding_path(dest: &Path) -> PathBuf {
    if !dest.exists() {
        return dest.to_path_buf();
    }
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = dest.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 1..=9999 {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dest.to_path_buf()
}

/// A single trashed item, reconstructed from its `.trashinfo` sidecar
/// (freedesktop.org Trash spec format: `Path=` + `DeletionDate=`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashEntry {
    pub id: String,
    pub name: String,
    pub original_path: String,
    pub deleted_at: String,
    pub is_directory: bool,
    pub size_bytes: u64,
}

/// Errors that can occur during file browser operations.
#[derive(Error, Debug)]
pub enum BrowserError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Path not found: {0}")]
    NotFound(String),
    #[error("Permission denied: {0}")]
    PermissionDenied(String),
    #[error("Integrity check failed: {0}")]
    IntegrityMismatch(String),
}

/// Metadata about a single file or directory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub modified: String,
    pub permissions: String,
    pub owner: String,
    pub filesystem: Option<String>,
}

/// A directory listing with sorted entries and aggregate stats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryListing {
    pub path: String,
    /// Parent directory path, or `None` for the root.
    pub parent: Option<String>,
    pub entries: Vec<FileInfo>,
    pub total_size: u64,
    pub file_count: usize,
    pub directory_count: usize,
    /// `true` when the directory held more than `MAX_DIR_ENTRIES` items and
    /// only the first `MAX_DIR_ENTRIES` (after sorting) are in `entries`.
    #[serde(default)]
    pub truncated: bool,
}

/// A text preview of a file, truncated if larger than the requested max size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePreview {
    pub name: String,
    pub path: String,
    pub content: String,
    pub is_truncated: bool,
    pub size_bytes: u64,
    pub mime_type: String,
    /// Whether `content` is meaningful text. `false` for anything that isn't
    /// text-like or an image (image previews go through the thumbnail
    /// endpoint instead) — the caller should show "no preview available"
    /// rather than dumping raw/garbled bytes.
    pub previewable: bool,
}

/// Whether a mime type's bytes are worth decoding as text for a preview.
/// Images are handled separately (via the thumbnail endpoint), so they don't
/// count as "previewable" here even though they have a real preview path.
fn is_text_previewable(mime_type: &str) -> bool {
    mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json" | "application/javascript" | "application/xml"
        )
}

/// Thread-safe cache for filesystem types, keyed by mount point.
pub struct FsCache {
    cache: RwLock<HashMap<String, Option<String>>>,
}

impl FsCache {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get(&self, mount_point: &str) -> Option<Option<String>> {
        self.cache.read().await.get(mount_point).cloned()
    }

    pub async fn insert(&self, mount_point: String, fs_type: Option<String>) {
        self.cache.write().await.insert(mount_point, fs_type);
    }
}

impl Default for FsCache {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FileBrowser;

impl FileBrowser {
    /// Async listing using tokio's read_dir + parallel metadata via FuturesUnordered
    pub async fn list_directory(path: &str) -> Result<DirectoryListing, BrowserError> {
        let path = path.to_string();

        tokio::task::spawn_blocking(move || Self::list_directory_sync(&path))
            .await
            .map_err(|e| BrowserError::Io(std::io::Error::other(e.to_string())))?
    }

    fn list_directory_sync(path: &str) -> Result<DirectoryListing, BrowserError> {
        let dir_path = Path::new(path);

        if !dir_path.exists() {
            return Err(BrowserError::NotFound(path.to_string()));
        }

        if !dir_path.is_dir() {
            return Err(BrowserError::NotFound("Not a directory".to_string()));
        }

        let parent = dir_path.parent().map(|p| p.to_string_lossy().to_string());
        let mut entries = Vec::new();
        let mut total_size = 0u64;
        let mut file_count = 0usize;
        let mut directory_count = 0usize;

        let dir_entries = std::fs::read_dir(dir_path)?;
        let passwd_map = Self::load_passwd_map();

        for entry in dir_entries {
            // A read failure on one entry (transient IO, a race with a
            // concurrent delete) must not blow away the whole listing —
            // skip that entry and keep going.
            let Ok(entry) = entry else { continue };

            let name = entry.file_name().to_string_lossy().to_string();
            if is_os_junk_file(&name) {
                continue;
            }

            // `DirEntry::metadata()` does not follow symlinks, but it can
            // still fail (permissions, a filesystem hiccup). Fall back to
            // `symlink_metadata`, then skip the entry rather than aborting.
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => match entry.path().symlink_metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                },
            };

            let file_path = entry.path().to_string_lossy().to_string();

            let permissions = Self::format_permissions(&metadata);
            let modified = Self::format_time(metadata.modified().ok());
            let owner = Self::get_owner(&metadata, &passwd_map);

            let size = metadata.len();

            if metadata.is_dir() {
                directory_count += 1;
            } else {
                file_count += 1;
                total_size += size;
            }

            // Detect filesystem only for mount points (avoid calling df for every entry)
            let mount_point = Self::find_mount_point(&file_path);
            let fs_type = if mount_point.as_deref() == Some(&file_path) {
                Self::detect_filesystem_sync(&file_path)
            } else {
                None
            };

            entries.push(FileInfo {
                name,
                path: file_path,
                is_directory: metadata.is_dir(),
                size_bytes: size,
                modified,
                permissions,
                owner,
                filesystem: fs_type,
            });
        }

        // Sort: directories first, then by name
        entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        // Cap the payload after sorting so the client still gets a stable,
        // predictable slice (dirs first, alphabetical) of a huge directory.
        let truncated = entries.len() > MAX_DIR_ENTRIES;
        if truncated {
            entries.truncate(MAX_DIR_ENTRIES);
        }

        Ok(DirectoryListing {
            path: path.to_string(),
            parent,
            entries,
            total_size,
            file_count,
            directory_count,
            truncated,
        })
    }

    /// Recursive search for files matching a query
    pub async fn search(
        path: &str,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<FileInfo>, BrowserError> {
        let path = path.to_string();
        let query = query.to_lowercase();
        let max_results = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(max_results));

        tokio::task::spawn_blocking(move || {
            let passwd_map = Self::load_passwd_map();
            Self::search_sync(&path, &query, &max_results, 0, &passwd_map)
        })
        .await
        .map_err(|e| BrowserError::Io(std::io::Error::other(e.to_string())))
    }

    /// Fuzzy (subsequence) search over file names under `path`.
    ///
    /// Unlike [`Self::search`] (substring match), a query like `rdm` matches
    /// `readme.md`. Every visited name is scored with [`fuzzy_score`]; only
    /// scoring names are kept, best first, capped at `max_results`. The walk
    /// is bounded (depth 10, 50 000 visited entries) so a huge tree cannot
    /// stall the request.
    pub async fn search_fuzzy(
        path: &str,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<FileInfo>, BrowserError> {
        let path = path.to_string();
        let query = query.to_lowercase();
        tokio::task::spawn_blocking(move || {
            let passwd_map = Self::load_passwd_map();
            let mut scored =
                Self::search_fuzzy_sync(&path, &query, 0, &passwd_map, &mut 0usize);
            scored.sort_by_key(|a| std::cmp::Reverse(a.0));
            scored.truncate(max_results.max(1));
            scored.into_iter().map(|(_, info)| info).collect()
        })
        .await
        .map_err(|e| BrowserError::Io(std::io::Error::other(e.to_string())))
    }

    fn search_fuzzy_sync(
        path: &str,
        query: &str,
        depth: usize,
        passwd_map: &HashMap<u32, String>,
        visited: &mut usize,
    ) -> Vec<(u64, FileInfo)> {
        const MAX_VISITED: usize = 50_000;
        let mut out = Vec::new();
        if depth > 10 || *visited >= MAX_VISITED {
            return out;
        }
        let dir_path = Path::new(path);
        if !dir_path.exists() || !dir_path.is_dir() {
            return out;
        }
        let Ok(dir_entries) = std::fs::read_dir(dir_path) else {
            return out;
        };
        for entry in dir_entries.flatten() {
            if *visited >= MAX_VISITED {
                break;
            }
            *visited += 1;
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let file_path = entry.path().to_string_lossy().to_string();
            let score = fuzzy_score(query, &name.to_lowercase());
            if score > 0 {
                out.push((
                    score,
                    FileInfo {
                        name: name.clone(),
                        path: file_path.clone(),
                        is_directory: metadata.is_dir(),
                        size_bytes: metadata.len(),
                        modified: Self::format_time(metadata.modified().ok()),
                        permissions: Self::format_permissions(&metadata),
                        owner: Self::get_owner(&metadata, passwd_map),
                        filesystem: None,
                    },
                ));
            }
            if metadata.is_dir() && !name.starts_with('.') {
                out.extend(Self::search_fuzzy_sync(
                    &file_path,
                    query,
                    depth + 1,
                    passwd_map,
                    visited,
                ));
            }
        }
        out
    }

    fn search_sync(
        path: &str,
        query: &str,
        max_results: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
        depth: usize,
        passwd_map: &HashMap<u32, String>,
    ) -> Vec<FileInfo> {
        let mut results = Vec::new();

        // Limit recursion depth to avoid infinite loops
        if depth > 10 {
            return results;
        }

        let dir_path = Path::new(path);
        if !dir_path.exists() || !dir_path.is_dir() {
            return results;
        }

        // Check if we've reached max results
        if max_results.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            return results;
        }

        if let Ok(dir_entries) = std::fs::read_dir(dir_path) {
            for entry in dir_entries.flatten() {
                if max_results.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                    break;
                }

                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                let name = entry.file_name().to_string_lossy().to_string();
                let file_path = entry.path().to_string_lossy().to_string();

                // Check if name matches query
                if name.to_lowercase().contains(query) {
                    let file_info = FileInfo {
                        name: name.clone(),
                        path: file_path.clone(),
                        is_directory: metadata.is_dir(),
                        size_bytes: metadata.len(),
                        modified: Self::format_time(metadata.modified().ok()),
                        permissions: Self::format_permissions(&metadata),
                        owner: Self::get_owner(&metadata, passwd_map),
                        filesystem: None,
                    };
                    results.push(file_info);
                    max_results.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);

                    // If we found a directory, don't recurse into it to avoid too many results
                    if metadata.is_dir() {
                        continue;
                    }
                }

                // Recurse into directories
                if metadata.is_dir() && !name.starts_with('.') {
                    let sub_results =
                        Self::search_sync(&file_path, query, max_results, depth + 1, passwd_map);
                    results.extend(sub_results);
                }
            }
        }

        results
    }

    fn find_mount_point(path: &str) -> Option<String> {
        let mut current = Path::new(path);
        while let Some(parent) = current.parent() {
            if let Ok(meta) = std::fs::metadata(current) {
                if meta.is_dir() {
                    // Check if this is a mount point by comparing device IDs
                    if let Ok(current_dev) = std::fs::metadata(current).map(|m| {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::MetadataExt;
                            m.dev()
                        }
                        #[cfg(not(unix))]
                        {
                            0u64
                        }
                    }) {
                        if let Ok(parent_meta) = std::fs::metadata(parent) {
                            let parent_dev = {
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::MetadataExt;
                                    parent_meta.dev()
                                }
                                #[cfg(not(unix))]
                                {
                                    0u64
                                }
                            };
                            if current_dev != parent_dev {
                                return Some(current.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
            current = parent;
        }
        None
    }

    fn detect_filesystem_sync(path: &str) -> Option<String> {
        let output = Command::new("df").args(["-T", path]).output().ok()?;

        if !output.status.success() {
            return None;
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = output_str.lines().collect();

        if lines.len() >= 2 {
            let headers = lines[0].split_whitespace().collect::<Vec<&str>>();
            let values = lines[1].split_whitespace().collect::<Vec<&str>>();

            if let Some(type_idx) = headers.iter().position(|&h| h == "Type" || h == "Fstype") {
                if type_idx < values.len() {
                    let fs_type = values[type_idx].to_lowercase();
                    let pseudo = [
                        "devtmpfs",
                        "tmpfs",
                        "devpts",
                        "sysfs",
                        "proc",
                        "cgroup",
                        "cgroup2",
                        "pstore",
                        "securityfs",
                        "debugfs",
                        "hugetlbfs",
                        "mqueue",
                        "fusectl",
                        "configfs",
                        "autofs",
                        "fuse.portal",
                        "overlay",
                        "squashfs",
                        "ramfs",
                        "binfmt_misc",
                        "fuseblk",
                    ];
                    if !fs_type.is_empty() && !pseudo.contains(&fs_type.as_str()) {
                        return Some(values[type_idx].to_string());
                    }
                }
            }
        }
        None
    }

    pub fn create_directory(path: &str) -> Result<(), BrowserError> {
        std::fs::create_dir_all(path)?;
        Ok(())
    }

    pub fn delete_item(path: &str, recursive: bool) -> Result<(), BrowserError> {
        let item_path = Path::new(path);

        if !item_path.exists() {
            return Err(BrowserError::NotFound(path.to_string()));
        }

        if item_path.is_dir() {
            if recursive {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_dir(path)?;
            }
        } else {
            std::fs::remove_file(path)?;
        }

        Ok(())
    }

    pub fn rename_item(old_path: &str, new_path: &str) -> Result<(), BrowserError> {
        std::fs::rename(old_path, new_path)?;
        Ok(())
    }

    /// Move using atomic rename when possible (same filesystem).
    /// Falls back to copy+delete for cross-filesystem moves.
    pub fn move_item(source: &str, destination: &str) -> Result<(), BrowserError> {
        // Try atomic rename first
        match std::fs::rename(source, destination) {
            Ok(_) => {
                Self::fsync_parent_dir(Path::new(destination));
                return Ok(());
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::CrossesDevices
                    || e.raw_os_error() == Some(libc::EXDEV) =>
            {
                // Cross-device move: fall through to copy+delete
            }
            Err(e) => return Err(BrowserError::Io(e)),
        }

        // Cross-device fallback
        let src_path = Path::new(source);
        if src_path.is_dir() {
            Self::copy_dir_all(src_path, Path::new(destination))?;
            std::fs::remove_dir_all(source)?;
            Self::fsync_parent_dir(Path::new(destination));
        } else {
            // Verified copy first — only remove the source once a
            // byte-identical, fsync'd copy is confirmed on disk.
            Self::copy_file_verified(src_path, Path::new(destination))?;
            std::fs::remove_file(source)?;
            Self::fsync_parent_dir(Path::new(destination));
        }
        Ok(())
    }

    pub fn copy_item(source: &str, destination: &str) -> Result<(), BrowserError> {
        let src_path = Path::new(source);

        if src_path.is_dir() {
            Self::copy_dir_all(src_path, Path::new(destination))?;
        } else {
            Self::copy_file_verified(src_path, Path::new(destination))?;
        }

        Ok(())
    }

    /// Moves an item into `trash_root` (created if needed) instead of
    /// permanently deleting it, recording its original location in a
    /// `.trashinfo` sidecar so it can be restored later.
    pub fn trash_item(path: &str, trash_root: &Path) -> Result<(), BrowserError> {
        let item_path = Path::new(path);
        if !item_path.exists() {
            return Err(BrowserError::NotFound(path.to_string()));
        }
        std::fs::create_dir_all(trash_root)?;

        let id = uuid::Uuid::new_v4().to_string();
        let file_name = item_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "item".to_string());
        let trashed_name = format!("{}_{}", id, file_name);
        let trashed_path = trash_root.join(&trashed_name);

        // Try atomic rename first; fall back to copy+delete cross-filesystem.
        if std::fs::rename(item_path, &trashed_path).is_err() {
            if item_path.is_dir() {
                Self::copy_dir_all(item_path, &trashed_path)?;
                std::fs::remove_dir_all(item_path)?;
            } else {
                // Verified copy — never remove the source unless a
                // byte-identical, fsync'd copy exists in the trash.
                Self::copy_file_verified(item_path, &trashed_path)?;
                std::fs::remove_file(item_path)?;
            }
        }
        Self::fsync_parent_dir(&trashed_path);

        let deleted_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let info_content = format!("Path={}\nDeletionDate={}\n", path, deleted_at);
        std::fs::write(
            trash_root.join(format!("{}.trashinfo", trashed_name)),
            info_content,
        )?;

        Ok(())
    }

    /// Lists everything currently in `trash_root`, newest first.
    pub fn list_trash(trash_root: &Path) -> Vec<TrashEntry> {
        let mut entries = Vec::new();
        let Ok(dir) = std::fs::read_dir(trash_root) else {
            return entries;
        };

        for entry in dir.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.ends_with(".trashinfo") {
                continue;
            }
            let Ok(info_content) =
                std::fs::read_to_string(trash_root.join(format!("{}.trashinfo", file_name)))
            else {
                continue;
            };

            let mut original_path = String::new();
            let mut deleted_at = String::new();
            for line in info_content.lines() {
                if let Some(v) = line.strip_prefix("Path=") {
                    original_path = v.to_string();
                }
                if let Some(v) = line.strip_prefix("DeletionDate=") {
                    deleted_at = v.to_string();
                }
            }

            let id = file_name
                .split('_')
                .next()
                .unwrap_or(&file_name)
                .to_string();
            let name = original_path
                .rsplit('/')
                .next()
                .unwrap_or(&file_name)
                .to_string();
            let metadata = entry.metadata().ok();

            entries.push(TrashEntry {
                id,
                name,
                original_path,
                deleted_at,
                is_directory: metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                size_bytes: metadata.map(|m| m.len()).unwrap_or(0),
            });
        }

        entries.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        entries
    }

    /// Finds the on-disk trashed path + recorded original path for a trash `id`.
    ///
    /// Callers iterate this over every trash root a session can see, treating
    /// `NotFound` as "try the next root". A trash root that does not exist yet
    /// (nothing has ever been trashed under it) must therefore read as
    /// `NotFound`, *not* propagate its `read_dir` IO error — otherwise a
    /// missing earlier root aborts the search before the root that actually
    /// holds the entry is ever checked.
    fn find_trash_entry(trash_root: &Path, id: &str) -> Result<(PathBuf, String), BrowserError> {
        let dir = match std::fs::read_dir(trash_root) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BrowserError::NotFound(id.to_string()));
            }
            Err(e) => return Err(BrowserError::Io(e)),
        };
        let prefix = format!("{}_", id);

        for entry in dir.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.ends_with(".trashinfo") || !file_name.starts_with(&prefix) {
                continue;
            }
            let info_content =
                std::fs::read_to_string(trash_root.join(format!("{}.trashinfo", file_name)))
                    .unwrap_or_default();
            let original_path = info_content
                .lines()
                .find_map(|l| l.strip_prefix("Path="))
                .unwrap_or("")
                .to_string();
            return Ok((entry.path(), original_path));
        }

        Err(BrowserError::NotFound(id.to_string()))
    }

    /// Moves a trashed item back to its original location.
    pub fn restore_trash_item(trash_root: &Path, id: &str) -> Result<(), BrowserError> {
        let (trashed_path, original_path) = Self::find_trash_entry(trash_root, id)?;

        if original_path.is_empty() {
            return Err(BrowserError::NotFound(format!(
                "Trash entry {} has no recorded original location",
                id
            )));
        }
        if Path::new(&original_path).exists() {
            return Err(BrowserError::PermissionDenied(
                "A file already exists at the original location".to_string(),
            ));
        }
        let dest = Path::new(&original_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Atomic rename when the trash dir and the original location share a
        // filesystem; fall back to copy+delete otherwise (e.g. a file trashed
        // from a RAID/pool mount into the pool root's trash, or into the
        // app-owned fallback trash on the data disk). `trash_item` and
        // `move_item` already handle this cascade — restore must too, or a
        // cross-device restore fails with an opaque IO error.
        match std::fs::rename(&trashed_path, dest) {
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::CrossesDevices
                    || e.raw_os_error() == Some(libc::EXDEV) =>
            {
                if trashed_path.is_dir() {
                    Self::copy_dir_all(&trashed_path, dest)?;
                    std::fs::remove_dir_all(&trashed_path)?;
                } else {
                    Self::copy_file_verified(&trashed_path, dest)?;
                    std::fs::remove_file(&trashed_path)?;
                }
            }
            Err(e) => return Err(BrowserError::Io(e)),
        }
        Self::fsync_parent_dir(dest);
        let trashed_name = trashed_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = std::fs::remove_file(trash_root.join(format!("{}.trashinfo", trashed_name)));

        Ok(())
    }

    /// Permanently deletes a single trashed item and its sidecar.
    pub fn purge_trash_item(trash_root: &Path, id: &str) -> Result<(), BrowserError> {
        let (trashed_path, _) = Self::find_trash_entry(trash_root, id)?;
        Self::delete_item(&trashed_path.to_string_lossy(), true)?;
        let trashed_name = trashed_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = std::fs::remove_file(trash_root.join(format!("{}.trashinfo", trashed_name)));
        Ok(())
    }

    /// Permanently deletes everything in `trash_root`.
    pub fn empty_trash(trash_root: &Path) -> Result<(), BrowserError> {
        if trash_root.exists() {
            std::fs::remove_dir_all(trash_root)?;
        }
        Ok(())
    }

    pub fn get_file_info(path: &str) -> Result<FileInfo, BrowserError> {
        let file_path = Path::new(path);

        if !file_path.exists() {
            return Err(BrowserError::NotFound(path.to_string()));
        }

        let metadata = std::fs::metadata(file_path)?;
        let passwd_map = Self::load_passwd_map();

        Ok(FileInfo {
            name: file_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: path.to_string(),
            is_directory: metadata.is_dir(),
            size_bytes: metadata.len(),
            modified: Self::format_time(metadata.modified().ok()),
            permissions: Self::format_permissions(&metadata),
            owner: Self::get_owner(&metadata, &passwd_map),
            filesystem: Self::detect_filesystem_sync(path),
        })
    }

    pub fn preview_file(path: &str, max_size: usize) -> Result<FilePreview, BrowserError> {
        let file_path = Path::new(path);

        if !file_path.exists() {
            return Err(BrowserError::NotFound(path.to_string()));
        }

        if file_path.is_dir() {
            return Err(BrowserError::NotFound(
                "Cannot preview directory".to_string(),
            ));
        }

        let metadata = std::fs::metadata(file_path)?;
        let size_bytes = metadata.len();
        let mime_type = Self::guess_mime_type(file_path);
        let is_image = mime_type.starts_with("image/");
        let previewable = is_text_previewable(&mime_type);

        // Only actually read+decode bytes for text-like files — for anything
        // else (video, audio, pdf, archives, unknown binaries) there's no
        // meaningful "text content" to show, so skip the read entirely
        // rather than lossily converting binary garbage into `content`.
        let (content, is_truncated) = if previewable {
            let file = std::fs::File::open(file_path)?;
            use std::io::Read;
            let mut reader = std::io::BufReader::new(file);
            let mut buffer = vec![0u8; max_size];
            let n = reader.read(&mut buffer)?;
            buffer.truncate(n);
            let is_truncated = size_bytes > max_size as u64;
            (String::from_utf8_lossy(&buffer).to_string(), is_truncated)
        } else {
            (String::new(), false)
        };

        Ok(FilePreview {
            name: file_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: path.to_string(),
            content,
            is_truncated,
            size_bytes,
            mime_type,
            previewable: previewable || is_image,
        })
    }

    /// Compresses `paths` (expected to share the same parent directory — the
    /// common case for a multi-select "compress selection" action) into a
    /// single archive at `destination`. `format` is `"zip"` or anything else
    /// for the default `tar.gz`. The caller is responsible for checking that
    /// the required external tool (`tar`/`gzip` or `zip`) is installed.
    pub fn compress_items(
        paths: &[String],
        destination: &str,
        format: &str,
    ) -> Result<(), BrowserError> {
        if paths.is_empty() {
            return Err(BrowserError::NotFound("No items to compress".to_string()));
        }

        let parent = Path::new(&paths[0])
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| BrowserError::NotFound("Invalid path".to_string()))?;
        let names: Vec<String> = paths
            .iter()
            .filter_map(|p| {
                Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            })
            .collect();

        let output = if format == "zip" {
            Command::new("zip")
                .arg("-r")
                .arg(destination)
                .args(&names)
                .current_dir(&parent)
                .output()?
        } else {
            Command::new("tar")
                .arg("-czf")
                .arg(destination)
                .arg("-C")
                .arg(&parent)
                .args(&names)
                .output()?
        };

        if !output.status.success() {
            return Err(BrowserError::Io(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).to_string(),
            )));
        }
        Ok(())
    }

    fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), BrowserError> {
        std::fs::create_dir_all(dst)?;

        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if ty.is_symlink() {
                Self::copy_symlink(&src_path, &dst_path)?;
            } else if ty.is_dir() {
                Self::copy_dir_all(&src_path, &dst_path)?;
            } else {
                Self::copy_file_verified(&src_path, &dst_path)?;
            }
        }

        if let Ok(meta) = std::fs::symlink_metadata(src) {
            Self::preserve_metadata(&meta, dst);
        }
        Ok(())
    }

    /// Counts the files (not directories) under `path`, recursively — used
    /// to compute a total for progress reporting before a large copy/move.
    pub fn count_files(path: &Path) -> u64 {
        let mut count = 0;
        let Ok(dir) = std::fs::read_dir(path) else {
            return 0;
        };
        for entry in dir.flatten() {
            let Ok(ty) = entry.file_type() else { continue };
            if ty.is_dir() {
                count += Self::count_files(&entry.path());
            } else {
                count += 1;
            }
        }
        count
    }

    /// Same as `copy_dir_all`, but increments `copied` after each file so a
    /// caller polling it from another thread can report progress, and records
    /// every path it *created* (never one it merely overwrote) in `created`.
    /// On failure the caller can use that list — in reverse order, deepest
    /// first — to remove exactly the partial tree this run produced, without
    /// touching anything that existed before the copy started.
    pub fn copy_dir_all_tracked(
        src: &Path,
        dst: &Path,
        copied: &std::sync::atomic::AtomicU64,
        created: &mut Vec<std::path::PathBuf>,
    ) -> Result<(), BrowserError> {
        if !dst.exists() {
            std::fs::create_dir_all(dst)?;
            created.push(dst.to_path_buf());
        } else if !dst.is_dir() {
            return Err(BrowserError::Io(std::io::Error::other(
                "destination exists and is not a directory",
            )));
        }

        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if ty.is_symlink() {
                let existed = dst_path.exists();
                Self::copy_symlink(&src_path, &dst_path)?;
                if !existed {
                    created.push(dst_path);
                }
                copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else if ty.is_dir() {
                Self::copy_dir_all_tracked(&src_path, &dst_path, copied, created)?;
            } else {
                let existed = dst_path.exists();
                Self::copy_file_verified(&src_path, &dst_path)?;
                if !existed {
                    created.push(dst_path);
                }
                copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        if let Ok(meta) = std::fs::symlink_metadata(src) {
            Self::preserve_metadata(&meta, dst);
        }
        Ok(())
    }

    /// Computes the SHA-256 of everything readable from `reader`, in 64 KiB
    /// chunks so arbitrarily large files never sit fully in memory.
    fn sha256_reader<R: std::io::Read>(mut reader: R) -> std::io::Result<[u8; 32]> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher.finalize().into())
    }

    /// Copies a single regular file from `src` to `dst` with an end-to-end
    /// integrity guarantee: the source SHA-256 is computed while streaming the
    /// bytes into a sibling temp file, that temp file is flushed and
    /// `fsync`'d so it is durable on disk, then read back and its own SHA-256
    /// compared to the source's. Only once the copy is confirmed
    /// byte-identical is it atomically `rename`'d onto `dst` — so a reader
    /// hitting `dst` mid-copy always sees either the old complete file or the
    /// new one, never a partial write, and a crash/OOM-kill/power-loss during
    /// the copy leaves the original `dst` (if any) untouched instead of
    /// truncated. On any mismatch the (corrupt) temp file is removed and an
    /// error is returned — so a caller performing a move never deletes the
    /// source unless a byte-identical copy is confirmed on disk. Source
    /// permission bits, ownership (uid/gid) and mtime are preserved.
    fn copy_file_verified(src: &Path, dst: &Path) -> Result<u64, BrowserError> {
        let src_meta = std::fs::symlink_metadata(src)?;

        // Written next to `dst` (not to a shared temp dir) so the final
        // rename below is guaranteed to be same-filesystem, and therefore
        // atomic.
        let tmp_name = format!(
            ".{}.hvtmp-{}",
            dst.file_name().and_then(|n| n.to_str()).unwrap_or("copy"),
            uuid::Uuid::new_v4()
        );
        let tmp_path = dst.with_file_name(tmp_name);

        let (total, src_hash) = match Self::stream_copy_hashed(src, &tmp_path) {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e);
            }
        };

        let dst_hash_result = std::fs::File::open(&tmp_path)
            .and_then(|f| Self::sha256_reader(std::io::BufReader::new(f)));
        let dst_hash = match dst_hash_result {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(BrowserError::Io(e));
            }
        };

        if src_hash != dst_hash {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(BrowserError::IntegrityMismatch(format!(
                "SHA-256 mismatch after copying {} -> {}",
                src.display(),
                dst.display()
            )));
        }

        // Preserve metadata on the temp file before the atomic swap, so
        // `dst` never has a window with the wrong permissions/ownership.
        Self::preserve_metadata(&src_meta, &tmp_path);

        if let Err(e) = std::fs::rename(&tmp_path, dst) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(BrowserError::Io(e));
        }
        // Make the rename itself durable: without fsync'ing the parent
        // directory, a crash/power-loss right after the rename could lose the
        // new directory entry even though the file's own blocks are on disk.
        Self::fsync_parent_dir(dst);

        Ok(total)
    }

    /// Best-effort `fsync` of a path's parent directory so a completed atomic
    /// rename is actually durable. Failures are logged at debug level and
    /// otherwise ignored — some filesystems don't support opening a
    /// directory, and a copy that otherwise succeeded must not be reported as
    /// failed because the durability hint couldn't be applied.
    pub fn fsync_parent_dir(path: &Path) {
        let Some(parent) = path.parent() else {
            return;
        };
        match std::fs::File::open(parent).and_then(|d| d.sync_all()) {
            Ok(()) => {}
            Err(e) => tracing::debug!(
                "fsync of parent dir for '{}' failed (best-effort): {}",
                parent.display(),
                e
            ),
        }
    }

    /// Streams `src` into a fresh file at `tmp_dst`, hashing as it goes, then
    /// flushes and `fsync`'s it before returning. Returns the byte count and
    /// source SHA-256 on success; `tmp_dst` is left in place either way (the
    /// caller owns cleanup) so the hash can be verified against it.
    fn stream_copy_hashed(src: &Path, tmp_dst: &Path) -> Result<(u64, [u8; 32]), BrowserError> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};

        let mut reader = std::io::BufReader::new(std::fs::File::open(src)?);
        let mut writer = std::io::BufWriter::new(std::fs::File::create(tmp_dst)?);

        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut total: u64 = 0;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            total += n as u64;
        }
        writer.flush()?;
        let file = writer
            .into_inner()
            .map_err(|e| BrowserError::Io(e.into_error()))?;
        // Force the bytes to stable storage before we trust the copy.
        file.sync_all()?;
        drop(file);

        Ok((total, hasher.finalize().into()))
    }

    /// Recreates a symlink at `dst` pointing to the same target as `src`,
    /// instead of dereferencing it (which would copy the target's contents,
    /// risk infinite recursion on symlink cycles, and lose the link itself).
    fn copy_symlink(src: &Path, dst: &Path) -> Result<(), BrowserError> {
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        {
            if std::fs::symlink_metadata(dst).is_ok() {
                let _ = std::fs::remove_file(dst);
            }
            std::os::unix::fs::symlink(&target, dst)?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (target, dst);
            Err(BrowserError::Io(std::io::Error::other(
                "symlinks are not supported on this platform",
            )))
        }
    }

    /// Best-effort preservation of a source entry's permission bits, ownership
    /// (uid/gid) and access/modification times onto `dst`. Ownership changes
    /// require privilege and are silently ignored when unavailable, so an
    /// unprivileged copy still succeeds (just owned by the running user).
    #[cfg(unix)]
    fn preserve_metadata(src_meta: &std::fs::Metadata, dst: &Path) {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;

        let mode = src_meta.permissions().mode();
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode));

        if let Ok(c_path) = std::ffi::CString::new(dst.as_os_str().as_bytes()) {
            unsafe {
                libc::chown(c_path.as_ptr(), src_meta.uid(), src_meta.gid());
                let times = [
                    libc::timespec {
                        tv_sec: src_meta.atime() as libc::time_t,
                        tv_nsec: src_meta.atime_nsec() as _,
                    },
                    libc::timespec {
                        tv_sec: src_meta.mtime() as libc::time_t,
                        tv_nsec: src_meta.mtime_nsec() as _,
                    },
                ];
                libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0);
            }
        }
    }

    #[cfg(not(unix))]
    fn preserve_metadata(_src_meta: &std::fs::Metadata, _dst: &Path) {}

    pub fn format_permissions(_metadata: &std::fs::Metadata) -> String {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = _metadata.permissions().mode();
            let r = if mode & 0o400 != 0 { 'r' } else { '-' };
            let w = if mode & 0o200 != 0 { 'w' } else { '-' };
            let x = if mode & 0o100 != 0 { 'x' } else { '-' };
            format!("{}{}{}", r, w, x)
        }

        #[cfg(not(unix))]
        {
            "rwx".to_string()
        }
    }

    fn format_time(time: Option<std::time::SystemTime>) -> String {
        time.map(|t| {
            let datetime: chrono::DateTime<chrono::Local> = t.into();
            datetime.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_else(|| "Unknown".to_string())
    }

    #[cfg(unix)]
    fn get_owner(metadata: &std::fs::Metadata, passwd: &HashMap<u32, String>) -> String {
        use std::os::unix::fs::MetadataExt;
        let uid = metadata.uid();
        passwd.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
    }

    #[cfg(not(unix))]
    fn get_owner(_metadata: &std::fs::Metadata, _passwd: &HashMap<u32, String>) -> String {
        "unknown".to_string()
    }

    /// Parses `/etc/passwd` into a `uid -> username` map. Falls back to an
    /// empty map (so callers just see the numeric uid) if it can't be read —
    /// this should never happen on a real Linux host, but LDAP/NSS-only
    /// users won't be in this file either, hence the numeric fallback rather
    /// than a hard error.
    fn load_passwd_map() -> HashMap<u32, String> {
        let mut map = HashMap::new();
        let Ok(content) = std::fs::read_to_string("/etc/passwd") else {
            return map;
        };
        for line in content.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 3 {
                if let Ok(uid) = fields[2].parse::<u32>() {
                    map.insert(uid, fields[0].to_string());
                }
            }
        }
        map
    }

    /// Detects the filesystem type for a given path using `df` command.
    /// Note: Call detect_filesystem_sync for synchronous use inside spawn_blocking.
    pub fn detect_filesystem(path: &str) -> Option<String> {
        Self::detect_filesystem_sync(path)
    }

    fn guess_mime_type(path: &Path) -> String {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match extension.as_str() {
            "txt" => "text/plain",
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "js" => "application/javascript",
            "json" => "application/json",
            "xml" => "application/xml",
            "pdf" => "application/pdf",
            "zip" => "application/zip",
            "tar" => "application/x-tar",
            "gz" | "gzip" => "application/gzip",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "mp4" => "video/mp4",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            _ => "application/octet-stream",
        }
        .to_string()
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use std::io::Write;

    /// A verified single-file copy must produce a byte-identical file and
    /// preserve the source's permission bits.
    #[test]
    fn copy_item_file_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");

        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 256) as u8).collect();
        std::fs::File::create(&src)
            .unwrap()
            .write_all(&data)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o640)).unwrap();
        }

        FileBrowser::copy_item(src.to_str().unwrap(), dst.to_str().unwrap()).unwrap();

        assert_eq!(std::fs::read(&dst).unwrap(), data);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o640);
        }
    }

    /// A verified copy must fail loudly if the destination cannot be written
    /// as an exact copy — here we prove the happy path returns the exact size.
    #[test]
    fn copy_file_verified_returns_byte_count() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a");
        let dst = dir.path().join("b");
        std::fs::write(&src, b"hello world").unwrap();
        let n = FileBrowser::copy_file_verified(&src, &dst).unwrap();
        assert_eq!(n, 11);
        assert_eq!(std::fs::read(&dst).unwrap(), b"hello world");
    }

    /// Copying a directory tree must recurse, copy file contents exactly, and
    /// recreate symlinks as symlinks (not dereference them).
    #[cfg(unix)]
    #[test]
    fn copy_dir_preserves_symlinks_and_contents() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("tree");
        let nested = src.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("file.txt"), b"nested data").unwrap();
        std::fs::write(src.join("top.txt"), b"top data").unwrap();
        std::os::unix::fs::symlink("top.txt", src.join("link.txt")).unwrap();

        let dst = dir.path().join("tree_copy");
        FileBrowser::copy_item(src.to_str().unwrap(), dst.to_str().unwrap()).unwrap();

        assert_eq!(std::fs::read(dst.join("top.txt")).unwrap(), b"top data");
        assert_eq!(
            std::fs::read(dst.join("sub/file.txt")).unwrap(),
            b"nested data"
        );
        let link_meta = std::fs::symlink_metadata(dst.join("link.txt")).unwrap();
        assert!(
            link_meta.file_type().is_symlink(),
            "symlink must stay a symlink"
        );
        assert_eq!(
            std::fs::read_link(dst.join("link.txt")).unwrap(),
            Path::new("top.txt")
        );
    }

    /// Overwriting an existing destination must go through the atomic
    /// temp-file + rename path: the final content must be the new data (not
    /// a mix of old and new), and no `.hvtmp-*` temp file may be left behind
    /// in the destination directory afterwards.
    #[test]
    fn copy_file_verified_overwrite_is_atomic_and_leaves_no_temp_debris() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        let dst = dir.path().join("dst.bin");

        std::fs::write(&dst, b"old content that will be replaced").unwrap();
        std::fs::write(&src, b"new content").unwrap();

        let n = FileBrowser::copy_file_verified(&src, &dst).unwrap();
        assert_eq!(n, 11);
        assert_eq!(std::fs::read(&dst).unwrap(), b"new content");

        let leftover: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("hvtmp"))
            .collect();
        assert!(
            leftover.is_empty(),
            "temp file(s) left behind: {:?}",
            leftover
        );
    }

    /// `copy_dir_all_tracked` records exactly the paths it created (never a
    /// pre-existing destination) so a failed run can clean up only its own
    /// partial output.
    #[test]
    fn copy_dir_all_tracked_records_only_newly_created_paths() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("tree");
        let sub = src.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(src.join("top.txt"), b"top").unwrap();
        std::fs::write(sub.join("nested.txt"), b"nested").unwrap();

        // Pre-create the destination directory with a file that must survive.
        let dst = dir.path().join("existing_dst");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("precious.txt"), b"keep me").unwrap();

        let copied = std::sync::atomic::AtomicU64::new(0);
        let mut created = Vec::new();
        FileBrowser::copy_dir_all_tracked(&src, &dst, &copied, &mut created).unwrap();

        assert_eq!(copied.load(std::sync::atomic::Ordering::Relaxed), 2);
        // The pre-existing destination dir and its file are *not* recorded.
        assert!(
            !created.iter().any(|p| p.ends_with("precious.txt")),
            "pre-existing file must not be recorded for cleanup: {created:?}"
        );
        assert!(created.iter().any(|p| p.ends_with("top.txt")));
        assert!(created.iter().any(|p| p.ends_with("nested.txt")));
        assert!(std::fs::read(dst.join("precious.txt")).unwrap() == b"keep me");
    }
}

#[cfg(test)]
mod fuzzy_tests {
    use super::*;

    #[test]
    fn test_fuzzy_score_matches_exact_and_prefix() {
        assert!(fuzzy_score("readme", "readme.md") > 0);
        assert!(fuzzy_score("read", "readme.md") > 0);
    }

    #[test]
    fn test_fuzzy_score_matches_non_contiguous_subsequence() {
        assert!(fuzzy_score("rdm", "readme.md") > 0);
        assert!(fuzzy_score("psm", "photo_summer.png") > 0);
    }

    #[test]
    fn test_fuzzy_score_rejects_non_subsequence_and_empty() {
        assert_eq!(fuzzy_score("", "readme.md"), 0);
        assert_eq!(fuzzy_score("xyz", "readme.md"), 0);
        assert_eq!(fuzzy_score("readme.md.bak!", "readme.md"), 0);
    }

    #[test]
    fn test_fuzzy_score_prefers_contiguous_and_short() {
        let contiguous = fuzzy_score("read", "readme.md");
        let scattered = fuzzy_score("read", "r-e-a-d-notes.txt");
        assert!(contiguous > scattered);
        let short = fuzzy_score("doc", "doc.txt");
        let long = fuzzy_score("doc", "documentation_final_draft_v2.txt");
        assert!(short > long);
    }

    #[test]
    fn test_fuzzy_score_prefers_word_boundary_start() {
        let boundary = fuzzy_score("ph", "my-photo.png");
        let middle = fuzzy_score("ph", "alphabet-soup.txt");
        assert!(boundary > middle);
    }
}

#[cfg(test)]
mod collision_tests {
    use super::*;

    #[test]
    fn test_collision_strategy_parse_defaults_to_overwrite() {
        // None and anything unrecognised must stay Overwrite so existing
        // callers that pass no `?strategy=` are unaffected.
        assert_eq!(CollisionStrategy::parse(None), CollisionStrategy::Overwrite);
        assert_eq!(
            CollisionStrategy::parse(Some("")),
            CollisionStrategy::Overwrite
        );
        assert_eq!(
            CollisionStrategy::parse(Some("garbage")),
            CollisionStrategy::Overwrite
        );
    }

    #[test]
    fn test_collision_strategy_parse_accepts_the_known_values_case_and_dash_insensitive() {
        for s in [
            "keep_both",
            "keepboth",
            "keep-both",
            "KEEP_BOTH",
            " Keep-Both ",
        ] {
            assert_eq!(
                CollisionStrategy::parse(Some(s)),
                CollisionStrategy::KeepBoth,
                "{s:?}"
            );
        }
        for s in ["skip", "SKIP", " skip "] {
            assert_eq!(
                CollisionStrategy::parse(Some(s)),
                CollisionStrategy::Skip,
                "{s:?}"
            );
        }
    }

    #[test]
    fn test_non_colliding_path_returns_a_free_path_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let free = dir.path().join("brand-new.txt");
        assert_eq!(non_colliding_path(&free), free);
    }

    #[test]
    fn test_non_colliding_path_inserts_a_counter_before_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("report.txt");
        std::fs::write(&taken, b"x").unwrap();
        assert_eq!(
            non_colliding_path(&taken),
            dir.path().join("report (1).txt")
        );

        // With `(1)` also taken it must move on to `(2)`.
        std::fs::write(dir.path().join("report (1).txt"), b"x").unwrap();
        assert_eq!(
            non_colliding_path(&taken),
            dir.path().join("report (2).txt")
        );
    }

    #[test]
    fn test_non_colliding_path_handles_a_dotfile_with_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join(".env");
        std::fs::write(&taken, b"x").unwrap();
        assert_eq!(non_colliding_path(&taken), dir.path().join(".env (1)"));
    }

    #[test]
    fn test_non_colliding_path_only_splits_the_last_extension() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("archive.tar.gz");
        std::fs::write(&taken, b"x").unwrap();
        assert_eq!(
            non_colliding_path(&taken),
            dir.path().join("archive.tar (1).gz")
        );
    }
}
