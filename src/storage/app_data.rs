/* =============================================================================
 * File          : app_data.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-app persistent key/value data for installed Docker apps, stored
 *   as JSON documents.
 *
 * Dependencies:
 *   - none (JSON storage via storage/json)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Per-app persistent key/value data (JSON documents).

use crate::storage::json::{JsonStorage, JsonStorageError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from application-level data storage.
#[derive(Error, Debug)]
pub enum AppDataError {
    #[error("Storage error: {0}")]
    Storage(#[from] JsonStorageError),
}

/// Global application settings (not per-user).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: String,
    pub language: String,
    pub session_timeout_minutes: u32,
    pub notifications_enabled: bool,
    pub email_alerts_enabled: bool,
    pub webhook_url: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "light".to_string(),
            language: "en".to_string(),
            session_timeout_minutes: 30,
            notifications_enabled: true,
            email_alerts_enabled: false,
            webhook_url: String::new(),
        }
    }
}

/// Manages application-level JSON data storage (settings, etc.).
#[derive(Clone)]
pub struct AppDataManager {
    storage: JsonStorage,
}

impl AppDataManager {
    pub fn new(data_directory: String) -> Self {
        Self {
            storage: JsonStorage::new(data_directory),
        }
    }

    /// Save arbitrary JSON-serializable data under a key.
    pub fn save_app_data<T: Serialize>(&self, key: &str, data: &T) -> Result<(), AppDataError> {
        self.storage.save(key, data).map_err(AppDataError::from)
    }

    /// Load previously saved data, or return `None` if missing.
    pub fn load_app_data<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>, AppDataError> {
        self.storage.load(key).map_err(AppDataError::from)
    }

    /// Convenience method to persist the global app settings.
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), AppDataError> {
        self.save_app_data("app_settings", settings)
    }

    /// Load the global app settings, returning defaults if none saved.
    pub fn load_settings(&self) -> Result<AppSettings, AppDataError> {
        match self.load_app_data::<AppSettings>("app_settings")? {
            Some(settings) => Ok(settings),
            None => Ok(AppSettings::default()),
        }
    }
}
