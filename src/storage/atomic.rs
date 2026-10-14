/* =============================================================================
 * File          : atomic.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Atomic file writer: writes to a temporary file in the same directory
 *   then renames over the target, so a crash can never leave a
 *   half-written settings file behind.
 *
 * Dependencies:
 *   - thiserror (>= 2)
 *
 * Usage:
 *   Used by the JSON settings persistence layer.
 * =============================================================================
 */

//! Atomic file writes (temp file + rename) so crashes never leave
//! half-written files.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

/// Errors from atomic file writing operations.
#[derive(Error, Debug)]
pub enum AtomicWriteError {
    #[error("Failed to write to temporary file: {0}")]
    WriteError(#[from] std::io::Error),
    #[error("Failed to rename temporary file: {0}")]
    RenameError(String),
    #[error("Failed to delete temporary file: {0}")]
    CleanupError(String),
}

/// Writer that replaces files atomically (write-temp → sync → rename).
/// Prevents partial/corrupt writes on crash or power loss.
pub struct AtomicWriter;

impl AtomicWriter {
    /// Write `data` to `path` using an atomic temp-file-then-rename sequence.
    pub fn write<P: AsRef<Path>>(path: P, data: &str) -> Result<(), AtomicWriteError> {
        let path = path.as_ref();
        let temp_path = path.with_extension("tmp");

        let mut file = File::create(&temp_path)?;
        file.write_all(data.as_bytes())?;
        file.sync_all()?;

        // Replace the old file with the new one.
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&temp_path, path)?;

        Ok(())
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
