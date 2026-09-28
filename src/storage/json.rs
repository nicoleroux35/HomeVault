use crate::storage::atomic::{AtomicWriteError, AtomicWriter};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Errors from JSON-based storage operations.
#[derive(Error, Debug)]
pub enum JsonStorageError {
    #[error("Atomic write error: {0}")]
    AtomicError(#[from] AtomicWriteError),
    #[error("JSON serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Key-value JSON file storage with atomic writes.
#[derive(Clone)]
pub struct JsonStorage {
    data_directory: String,
}

impl JsonStorage {
    pub fn new(data_directory: String) -> Self {
        Self { data_directory }
    }

    /// Create the data root directory if it does not exist.
    pub fn ensure_directory(&self) -> Result<(), JsonStorageError> {
        let path = Path::new(&self.data_directory);
        if !path.exists() {
            fs::create_dir_all(path)?;
        }
        Ok(())
    }

    /// Persist `data` as a pretty-printed JSON file under `key`.
    pub fn save<T: Serialize>(&self, key: &str, data: &T) -> Result<(), JsonStorageError> {
        self.ensure_directory()?;

        let json_string = serde_json::to_string_pretty(data)?;
        let file_path = format!("{}/data/{}.json", self.data_directory, key);

        // Ensure the nested data/ subdirectory exists.
        let data_dir = format!("{}/data", self.data_directory);
        fs::create_dir_all(&data_dir)?;

        AtomicWriter::write(&file_path, &json_string)?;
        Ok(())
    }

    /// Deserialize and return the value stored under `key`, or `None` if absent.
    pub fn load<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>, JsonStorageError> {
        let file_path = format!("{}/data/{}.json", self.data_directory, key);
        let content = AtomicWriter::read(&file_path)?;

        if content.is_empty() {
            return Ok(None);
        }

        let data: T = serde_json::from_str(&content)?;
        Ok(Some(data))
    }

    /// Remove the JSON file identified by `key`.
    pub fn delete(&self, key: &str) -> Result<(), JsonStorageError> {
        let file_path = format!("{}/data/{}.json", self.data_directory, key);
        if Path::new(&file_path).exists() {
            fs::remove_file(&file_path)?;
        }
        Ok(())
    }

    /// Returns `true` if a file exists for the given key.
    pub fn exists(&self, key: &str) -> bool {
        let file_path = format!("{}/data/{}.json", self.data_directory, key);
        Path::new(&file_path).exists()
    }
}
