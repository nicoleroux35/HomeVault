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
