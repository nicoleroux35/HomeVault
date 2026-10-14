/* =============================================================================
 * File          : user_config.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-user JSON configuration documents (display preferences synced
 *   server-side).
 *
 * Dependencies:
 *   - none (JSON storage via storage/json)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Per-user JSON configuration documents.

use crate::storage::json::{JsonStorage, JsonStorageError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from user configuration operations.
#[derive(Error, Debug)]
pub enum UserConfigError {
    #[error("Storage error: {0}")]
    Storage(#[from] JsonStorageError),
    #[error("Configuration not found for user: {0}")]
    NotFound(String),
}

/// Per-user application preferences stored as JSON files.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    /// UI theme (e.g. `"light"`, `"dark"`).
    pub theme: Option<String>,
    /// UI language code (e.g. `"en"`, `"fr"`).
    pub language: Option<String>,
    /// Session expiry in minutes.
    pub session_timeout: Option<u32>,
    /// Whether push/desktop notifications are enabled.
    pub notifications_enabled: Option<bool>,
    /// Whether email alerts are enabled.
    pub email_alerts: Option<bool>,
    /// Custom webhook URL for notifications.
    pub webhook_url: Option<String>,
    /// Free-form dashboard layout configuration.
    pub dashboard_layout: Option<serde_json::Value>,
    /// Arbitrary extension settings.
    pub custom_settings: Option<serde_json::Value>,
}

/// Manages per-user JSON configuration files on disk.
#[derive(Clone)]
pub struct UserConfigManager {
    storage: JsonStorage,
}

impl UserConfigManager {
    pub fn new(data_directory: String) -> Self {
        Self {
            storage: JsonStorage::new(data_directory),
        }
    }

    fn config_key(username: &str) -> String {
        format!("user_config_{}", username)
    }

    /// Persist a user's configuration to disk.
    pub fn save_user_config(
        &self,
        username: &str,
        config: &UserConfig,
    ) -> Result<(), UserConfigError> {
        self.storage
            .save(&Self::config_key(username), config)
            .map_err(UserConfigError::from)
    }

    /// Load a user's configuration, returning defaults if none exists.
    pub fn load_user_config(&self, username: &str) -> Result<UserConfig, UserConfigError> {
        match self
            .storage
            .load::<UserConfig>(&Self::config_key(username))?
        {
            Some(config) => Ok(config),
            None => Ok(UserConfig::default()),
        }
    }

    /// Remove a user's configuration file.
    pub fn delete_user_config(&self, username: &str) -> Result<(), UserConfigError> {
        self.storage
            .delete(&Self::config_key(username))
            .map_err(UserConfigError::from)
    }

    /// Check whether a user has a saved configuration.
    pub fn user_config_exists(&self, username: &str) -> bool {
        self.storage.exists(&Self::config_key(username))
    }

    /// Update just the theme preference (load-edit-save).
    pub fn update_theme(
        &self,
        username: &str,
        theme: String,
    ) -> Result<UserConfig, UserConfigError> {
        let mut config = self.load_user_config(username)?;
        config.theme = Some(theme);
        self.save_user_config(username, &config)?;
        Ok(config)
    }

    /// Update just the language preference.
    pub fn update_language(
        &self,
        username: &str,
        language: String,
    ) -> Result<UserConfig, UserConfigError> {
        let mut config = self.load_user_config(username)?;
        config.language = Some(language);
        self.save_user_config(username, &config)?;
        Ok(config)
    }

    /// Update just the session timeout.
    pub fn update_session_timeout(
        &self,
        username: &str,
        timeout_minutes: u32,
    ) -> Result<UserConfig, UserConfigError> {
        let mut config = self.load_user_config(username)?;
        config.session_timeout = Some(timeout_minutes);
        self.save_user_config(username, &config)?;
        Ok(config)
    }

    /// Update the notifications-enabled flag.
    pub fn update_notifications_enabled(
        &self,
        username: &str,
        enabled: bool,
    ) -> Result<UserConfig, UserConfigError> {
        let mut config = self.load_user_config(username)?;
        config.notifications_enabled = Some(enabled);
        self.save_user_config(username, &config)?;
        Ok(config)
    }

    /// Update the email alerts flag.
    pub fn update_email_alerts(
        &self,
        username: &str,
        enabled: bool,
    ) -> Result<UserConfig, UserConfigError> {
        let mut config = self.load_user_config(username)?;
        config.email_alerts = Some(enabled);
        self.save_user_config(username, &config)?;
        Ok(config)
    }

    /// Update the webhook URL for notifications.
    pub fn update_webhook_url(
        &self,
        username: &str,
        url: String,
    ) -> Result<UserConfig, UserConfigError> {
        let mut config = self.load_user_config(username)?;
        config.webhook_url = Some(url);
        self.save_user_config(username, &config)?;
        Ok(config)
    }
}
