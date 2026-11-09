/* =============================================================================
 * File          : atomic.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Atomic file writer: writes to a temporary file in the same directory
 *   then renames over the target, so a crash can never leave a
 *   half-written settings file behind. The target is replaced by a single
 *   `rename`, never deleted first, and both the file and its parent directory
 *   are fsync'd before the write is reported as durable.
 *
 * Dependencies:
 *   - thiserror (>= 2)
 *
 * Usage:
 *   Used by the JSON settings persistence layer and by the system-config
 *   writer in `src/shares.rs`.
 * =============================================================================
 */

//! Atomic file writes (temp file + rename) so crashes never leave
//! half-written files.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Errors from atomic file writing operations.
#[derive(Error, Debug)]
pub enum AtomicWriteError {
    #[error("Failed to write to temporary file: {0}")]
    WriteError(#[from] std::io::Error),
    #[error("Failed to rename temporary file: {0}")]
    RenameError(#[source] std::io::Error),
    #[error("Failed to delete temporary file: {0}")]
    CleanupError(#[source] std::io::Error),
}

impl AtomicWriteError {
    /// Whether the failure was a permission problem, so a caller that has a
    /// privileged fallback (`sudo tee`) can use it instead of failing the
    /// whole request.
    pub fn is_permission_denied(&self) -> bool {
        match self {
            Self::WriteError(e) | Self::RenameError(e) | Self::CleanupError(e) => {
                e.kind() == std::io::ErrorKind::PermissionDenied
            }
        }
    }
}

/// Monotonic suffix so two writes in the same process cannot collide on a
/// temporary name, even within the same nanosecond.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a temporary path **in the target's own directory**, so the final
/// `rename` is guaranteed to stay on the same filesystem.
fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{file_name}.tmp.{pid}.{nanos}.{seq}"))
}

/// Create the temporary file, retrying once on the (astronomically unlikely)
/// name collision instead of truncating a file another writer owns.
fn create_temp(path: &Path) -> Result<(PathBuf, File), AtomicWriteError> {
    for _ in 0..3 {
        let candidate = temp_path_for(path);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(AtomicWriteError::WriteError(e)),
        }
    }
    Err(AtomicWriteError::WriteError(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    )))
}

/// fsync the directory entry itself, not just the file contents. Without
/// this, a crash right after `rename` can lose the new name even though the
/// bytes were flushed.
fn fsync_parent_dir(path: &Path) -> Result<(), AtomicWriteError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(AtomicWriteError::WriteError)
}

/// Writer that replaces files atomically (write-temp → sync → rename).
/// Prevents partial/corrupt writes on crash or power loss.
pub struct AtomicWriter;

impl AtomicWriter {
    /// Write `data` to `path` using an atomic temp-file-then-rename sequence.
    ///
    /// The old file is **never removed first**: POSIX `rename` replaces the
    /// target atomically, so a crash between the two steps cannot leave the
    /// path missing. The mode of an existing target is preserved.
    pub fn write<P: AsRef<Path>>(path: P, data: &str) -> Result<(), AtomicWriteError> {
        Self::write_bytes(path, data.as_bytes())
    }

    /// Atomic write of **binary** data (the configuration-backup payload and its
    /// staged database). The text `write` above would corrupt anything that is
    /// not UTF-8, and a SQLite snapshot is not.
    pub fn write_bytes<P: AsRef<Path>>(path: P, data: &[u8]) -> Result<(), AtomicWriteError> {
        let path = path.as_ref();
        let old_permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
        let (temp_path, mut file) = create_temp(path)?;

        let result = (|| {
            file.write_all(data)?;
            file.sync_all()?;
            if let Some(permissions) = old_permissions {
                // Keep the file's mode (0600 secrets, 0644 configs, …) across
                // the replacement. Ownership is unchanged because the writer
                // creates the temporary file as the same user.
                fs::set_permissions(&temp_path, permissions)?;
            }
            fs::rename(&temp_path, path).map_err(AtomicWriteError::RenameError)?;
            fsync_parent_dir(path)?;
            Ok(())
        })();

        if result.is_err() {
            // The target is untouched: only the temporary file may remain.
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    /// Read the full content of `path` as a string. Returns empty if missing.
    pub fn read<P: AsRef<Path>>(path: P) -> Result<String, AtomicWriteError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(String::new());
        }
        Ok(fs::read_to_string(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn write_replaces_an_existing_file_without_leaving_a_temporary() {
        let dir = temp_dir();
        let path = dir.path().join("settings.json");

        AtomicWriter::write(&path, "old").expect("first write");
        AtomicWriter::write(&path, "new").expect("second write");

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name != "settings.json")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn write_creates_a_missing_file() {
        let dir = temp_dir();
        let path = dir.path().join("nested").join("new.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();

        AtomicWriter::write(&path, "{\"ok\":true}").expect("write");

        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"ok\":true}");
    }

    #[test]
    fn write_preserves_the_existing_mode() {
        let dir = temp_dir();
        let path = dir.path().join("secret.json");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        AtomicWriter::write(&path, "new").expect("write");

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn write_bytes_round_trips_non_utf8_payloads() {
        let dir = temp_dir();
        let path = dir.path().join("snapshot.db");
        let payload = [0u8, 159, 146, 150, 255];

        AtomicWriter::write_bytes(&path, &payload).expect("write");
        assert_eq!(fs::read(&path).unwrap(), payload);
    }

    #[test]
    fn read_of_a_missing_file_is_empty_and_read_of_a_written_file_is_exact() {
        let dir = temp_dir();
        let path = dir.path().join("missing.json");
        assert_eq!(AtomicWriter::read(&path).unwrap(), "");

        AtomicWriter::write(&path, "content").unwrap();
        assert_eq!(AtomicWriter::read(&path).unwrap(), "content");
    }

    #[test]
    fn temporary_names_are_unique() {
        let dir = temp_dir();
        let path = dir.path().join("file.json");
        let a = temp_path_for(&path);
        let b = temp_path_for(&path);
        assert_ne!(a, b);
        assert_eq!(a.parent(), path.parent());
        assert_eq!(b.parent(), path.parent());
    }
}
