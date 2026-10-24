/* =============================================================================
 * File          : settings_persistence.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Settings persistence layer: automatic JSON-file storage for every
 *   settings family (keyed load/save with defaults) so configuration
 *   survives server restarts, independent of the SQLite database.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   SnmpService and other settings-backed services load/save through it.
 * =============================================================================
 */

//! Settings persistence layer - saves settings to JSON files
//!
//! This module provides automatic persistence for all HomeVault settings,
//! ensuring configuration survives server restarts.

use crate::handlers::settings::{
    AppearanceSettings, EmailSettings, GeneralSettings, NotificationSettings, UpdateFrequency,
    UpdateSettings, APPEARANCE_SETTINGS, EMAIL_SETTINGS, GENERAL_SETTINGS, NOTIFICATION_SETTINGS,
    UPDATE_SETTINGS,
};
use crate::storage::json::JsonStorage;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Global settings persistence manager
static SETTINGS_PERSISTENCE: Lazy<RwLock<Option<SettingsPersistence>>> =
    Lazy::new(|| RwLock::new(None));

/// Schema version written into every export and expected on import.
///
/// ROADMAP 13.5: exports were a bare blob with no version, so an import from a
/// different release either silently dropped fields the importer did not know
/// or failed with a serde message that named no field. The number is bumped
/// whenever a persisted key changes shape; an import from the future is refused
/// with that number in the message instead of being half-applied.
pub const CURRENT_SETTINGS_SCHEMA: u32 = 1;

/// Settings persistence data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSettings {
    /// `1` for this release. Absent (`0`) is a pre-versioning export, accepted
    /// because those blobs are exactly what this release still reads.
    #[serde(default)]
    pub schema_version: u32,
    pub general: Option<PersistedGeneral>,
    pub email: Option<PersistedEmail>,
    pub update: Option<PersistedUpdate>,
    pub notifications: Option<PersistedNotifications>,
    pub appearance: Option<PersistedAppearance>,
    pub ai: Option<PersistedAi>,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SETTINGS_SCHEMA,
            general: None,
            email: None,
            update: None,
            notifications: None,
            appearance: None,
            ai: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAi {
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedGeneral {
    pub server_name: Option<String>,
    pub timezone: Option<String>,
    /// Server-wide default UI language (ROADMAP 13.4).
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedEmail {
    pub smtp_server: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub from_address: Option<String>,
    pub use_tls: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedUpdate {
    pub check_frequency: Option<UpdateFrequency>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedNotifications {
    pub email: Option<String>,
    pub enable_alerts: Option<bool>,
    pub alert_threshold: Option<u8>,
    pub smtp_enabled: Option<bool>,
    pub webhook_url: Option<String>,
    pub webhook_enabled: Option<bool>,
    #[serde(default)]
    pub event_toggles: Option<std::collections::HashMap<String, bool>>,
    #[serde(default)]
    pub webhook_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// The on-disk appearance block (settings.json).
///
/// ROADMAP 26.2: `theme` and `density` were persisted no-ops and are no
/// longer written. The two fields stay in the **reader** so that a
/// `data/settings.json` left by an older release still loads (serde ignores
/// any key without a field, and keeps theirs), and they are dropped from the
/// live model (`AppearanceSettings`) and from every response.
pub struct PersistedAppearance {
    pub theme: Option<String>,
    pub wallpaper: Option<String>,
    pub accent_color: Option<String>,
    pub density: Option<String>,
}

impl PersistedSettings {
    pub fn to_general(&self) -> GeneralSettings {
        GeneralSettings {
            server_name: self.general.as_ref().and_then(|g| g.server_name.clone()),
            timezone: self.general.as_ref().and_then(|g| g.timezone.clone()),
            language: self.general.as_ref().and_then(|g| g.language.clone()),
        }
    }

    pub fn to_email(&self) -> EmailSettings {
        EmailSettings {
            smtp_server: self.email.as_ref().and_then(|e| e.smtp_server.clone()),
            smtp_port: self.email.as_ref().and_then(|e| e.smtp_port),
            smtp_username: self.email.as_ref().and_then(|e| e.smtp_username.clone()),
            smtp_password: self.email.as_ref().and_then(|e| e.smtp_password.clone()),
            from_address: self.email.as_ref().and_then(|e| e.from_address.clone()),
            use_tls: self.email.as_ref().and_then(|e| e.use_tls),
        }
    }

    pub fn to_update(&self) -> UpdateSettings {
        UpdateSettings {
            check_frequency: self.update.as_ref().and_then(|u| u.check_frequency),
        }
    }

    pub fn to_notifications(&self) -> NotificationSettings {
        NotificationSettings {
            email: self.notifications.as_ref().and_then(|n| n.email.clone()),
            enable_alerts: self.notifications.as_ref().and_then(|n| n.enable_alerts),
            alert_threshold: self.notifications.as_ref().and_then(|n| n.alert_threshold),
            smtp_enabled: self.notifications.as_ref().and_then(|n| n.smtp_enabled),
            webhook_url: self
                .notifications
                .as_ref()
                .and_then(|n| n.webhook_url.clone()),
            webhook_enabled: self.notifications.as_ref().and_then(|n| n.webhook_enabled),
            event_toggles: self
                .notifications
                .as_ref()
                .and_then(|n| n.event_toggles.clone()),
            webhook_format: self
                .notifications
                .as_ref()
                .and_then(|n| n.webhook_format.clone()),
        }
    }

    pub fn to_appearance(&self) -> AppearanceSettings {
        AppearanceSettings {
            wallpaper: self.appearance.as_ref().and_then(|a| a.wallpaper.clone()),
            accent_color: self
                .appearance
                .as_ref()
                .and_then(|a| a.accent_color.clone()),
        }
    }
}

/// Settings persistence manager
pub struct SettingsPersistence {
    storage: JsonStorage,
}

impl SettingsPersistence {
    /// Initialize the settings persistence with a data directory
    pub fn new(data_directory: String) -> Self {
        let storage = JsonStorage::new(data_directory);
        Self { storage }
    }

    /// Save all settings to disk
    pub fn save_all(
        &self,
        general: &GeneralSettings,
        email: &EmailSettings,
        update: &UpdateSettings,
        notifications: &NotificationSettings,
        appearance: &AppearanceSettings,
    ) -> Result<(), String> {
        let settings = PersistedSettings {
            schema_version: CURRENT_SETTINGS_SCHEMA,
            general: Some(PersistedGeneral {
                server_name: general.server_name.clone(),
                timezone: general.timezone.clone(),
                language: general.language.clone(),
            }),
            email: Some(PersistedEmail {
                smtp_server: email.smtp_server.clone(),
                smtp_port: email.smtp_port,
                smtp_username: email.smtp_username.clone(),
                smtp_password: email.smtp_password.clone(),
                from_address: email.from_address.clone(),
                use_tls: email.use_tls,
            }),
            update: Some(PersistedUpdate {
                check_frequency: update.check_frequency,
            }),
            notifications: Some(PersistedNotifications {
                email: notifications.email.clone(),
                enable_alerts: notifications.enable_alerts,
                alert_threshold: notifications.alert_threshold,
                smtp_enabled: notifications.smtp_enabled,
                webhook_url: notifications.webhook_url.clone(),
                webhook_enabled: notifications.webhook_enabled,
                event_toggles: notifications.event_toggles.clone(),
                webhook_format: notifications.webhook_format.clone(),
            }),
            appearance: Some(PersistedAppearance {
                // Never written again (ROADMAP 26.2); the reader keeps the
                // fields so an older settings.json still loads.
                theme: None,
                wallpaper: appearance.wallpaper.clone(),
                accent_color: appearance.accent_color.clone(),
                density: None,
            }),
            ai: None,
        };

        self.storage
            .save("settings", &settings)
            .map_err(|e| e.to_string())
    }

    /// Load all settings from disk
    pub fn load_all(&self) -> PersistedSettings {
        self.storage
            .load::<PersistedSettings>("settings")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Export settings as JSON string. The SMTP password is redacted — it's
    /// never shown in the UI either (GET /settings/email omits it), so an
    /// export downloaded as a file shouldn't leak it in the clear. Re-importing
    /// an export is still safe: `apply_loaded_settings` only overwrites fields
    /// that are `Some(..)`, so a redacted `None` password leaves the currently
    /// configured one untouched.
    ///
    /// The blob carries `schema_version` (ROADMAP 13.5) so a later release can
    /// tell an old export from a foreign file before applying anything.
    pub fn export(&self) -> Result<String, String> {
        let mut settings = self.load_all();
        settings.schema_version = CURRENT_SETTINGS_SCHEMA;
        if let Some(email) = settings.email.as_mut() {
            email.smtp_password = None;
        }
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())
    }

    /// Import settings from JSON string.
    ///
    /// ROADMAP 13.5: the file is validated *before* it replaces the stored
    /// blob — schema version first, then every field whose bad value would
    /// otherwise be persisted and only fail much later (a port of 0, an
    /// unknown timezone, a webhook URL that is not a URL). The error names the
    /// offending field, which the old bare `serde_json` error never did.
    pub fn import(&self, json: &str) -> Result<PersistedSettings, String> {
        let mut settings: PersistedSettings = serde_json::from_str(json).map_err(|e| {
            format!("Settings file is not valid for this HomeVault version: {e}")
        })?;
        validate_import(&settings)?;
        settings.schema_version = CURRENT_SETTINGS_SCHEMA;
        self.storage
            .save("settings", &settings)
            .map_err(|e| e.to_string())?;
        Ok(settings)
    }

    /// Reset all settings to defaults. Deleting the persisted file alone
    /// isn't enough: every GET handler and every real use of a setting
    /// (sending email, choosing the AI model, etc.) reads from the in-memory
    /// statics below, not from disk — those must be reset too, or the
    /// running process keeps serving the old values until its next restart.
    pub fn reset(&self) -> Result<(), String> {
        self.storage.delete("settings").map_err(|e| e.to_string())?;
        *GENERAL_SETTINGS.write() = GeneralSettings::default();
        *EMAIL_SETTINGS.write() = EmailSettings::default();
        *UPDATE_SETTINGS.write() = UpdateSettings::default();
        *NOTIFICATION_SETTINGS.write() = NotificationSettings::default();
        *APPEARANCE_SETTINGS.write() = AppearanceSettings::default();
        crate::handlers::chat::reset_ai_model();
        crate::handlers::snmp::reset_snmp_settings();
        // ROADMAP 13.4/13.5: the language is a setting too, so resetting must
        // put the running process back on English instead of keeping the old
        // locale until the next restart.
        crate::i18n::I18n::set_locale(crate::handlers::settings::DEFAULT_LANGUAGE);
        // Built-in HTTPS/TLS config (data/tls.json) is separate from settings.json
        // but is still "a setting" from the admin's point of view — clear it too.
        crate::tls::clear_settings();
        Ok(())
    }
}

/// Validate an imported settings blob before it is persisted.
///
/// Kept separate from `import` so it is unit-testable without touching disk and
/// so a future caller (e.g. a migration) can reuse it. Every refusal names the
/// field: "invalid settings file" alone was the reason ROADMAP 13.5 listed the
/// import as half-wired.
pub fn validate_import(settings: &PersistedSettings) -> Result<(), String> {
    if settings.schema_version > CURRENT_SETTINGS_SCHEMA {
        return Err(format!(
            "This settings file was exported by a newer HomeVault (schema {}; this build supports up to {}). Update HomeVault first.",
            settings.schema_version, CURRENT_SETTINGS_SCHEMA
        ));
    }

    if let Some(general) = &settings.general {
        if let Some(name) = &general.server_name {
            if name.chars().count() > 64 {
                return Err("general.server_name is longer than 64 characters".to_string());
            }
            if name.chars().any(|c| c.is_control()) {
                return Err("general.server_name contains control characters".to_string());
            }
        }
        if let Some(tz) = &general.timezone {
            let tz = tz.trim();
            if tz.is_empty() || chrono_tz::Tz::from_str(tz).is_err() {
                return Err(format!(
                    "general.timezone '{}' is not a known IANA timezone",
                    tz
                ));
            }
        }
        if let Some(language) = &general.language {
            crate::handlers::settings::normalize_language(language)
                .map_err(|e| format!("general.language: {e}"))?;
        }
    }

    if let Some(email) = &settings.email {
        if email.smtp_port == Some(0) {
            return Err("email.smtp_port must be 1-65535".to_string());
        }
        if let Some(from) = &email.from_address {
            let from = from.trim();
            if !from.is_empty() && !crate::handlers::settings::is_valid_email_address(from) {
                return Err(format!("email.from_address '{}' is not a valid address", from));
            }
        }
        // The relay fields become lines of the generated msmtp configuration,
        // so they go through the same validator that endpoint uses.
        let probe = crate::email::EmailConfig {
            smtp_host: email.smtp_server.clone().unwrap_or_default(),
            smtp_port: email.smtp_port.unwrap_or(587),
            smtp_username: email.smtp_username.clone().unwrap_or_default(),
            smtp_password: email.smtp_password.clone().unwrap_or_default(),
            from_address: email
                .from_address
                .clone()
                .unwrap_or_else(|| "homevault@localhost".to_string()),
            from_name: "HomeVault".to_string(),
            use_tls: email.use_tls.unwrap_or(true),
        };
        if probe.is_explicit() {
            probe
                .validate()
                .map_err(|e| format!("email: {}", e))?;
        }
    }

    if let Some(notifications) = &settings.notifications {
        if let Some(threshold) = notifications.alert_threshold {
            if threshold > 100 {
                return Err("notifications.alert_threshold must be 0-100".to_string());
            }
        }
        if let Some(email) = &notifications.email {
            let email = email.trim();
            if !email.is_empty() && !crate::handlers::settings::is_valid_email_address(email) {
                return Err(format!(
                    "notifications.email '{}' is not a valid address",
                    email
                ));
            }
        }
        if let Some(url) = &notifications.webhook_url {
            crate::handlers::settings::validate_webhook_url(url)
                .map_err(|e| format!("notifications.webhook_url: {e}"))?;
        }
        if let Some(format) = &notifications.webhook_format {
            if crate::notification_events::WebhookFormat::parse(format).is_none() {
                return Err(format!(
                    "notifications.webhook_format '{}' is not generic, slack or discord",
                    format
                ));
            }
        }
    }

    if let Some(appearance) = &settings.appearance {
        for (field, value, max) in [
            ("appearance.theme", &appearance.theme, 32usize),
            ("appearance.wallpaper", &appearance.wallpaper, 512),
            ("appearance.accent_color", &appearance.accent_color, 32),
            ("appearance.density", &appearance.density, 32),
        ] {
            if let Some(value) = value {
                if value.chars().count() > max {
                    return Err(format!("{field} is longer than {max} characters"));
                }
                if value.chars().any(|c| c.is_control()) {
                    return Err(format!("{field} contains control characters"));
                }
            }
        }
    }

    if let Some(ai) = &settings.ai {
        if let Some(model) = &ai.model {
            if model.chars().count() > 200 || model.chars().any(|c| c.is_control()) {
                return Err("ai.model is not a valid model id".to_string());
            }
        }
    }

    Ok(())
}

/// Initialize the settings persistence system.
pub fn init_persistence(data_directory: String) {
    let loaded_settings;
    let ai_model_option;

    {
        let mut persistence = SETTINGS_PERSISTENCE.write();
        *persistence = Some(SettingsPersistence::new(data_directory));

        // Try to load persisted settings and apply them
        if let Some(ref p) = *persistence {
            loaded_settings = p.load_all();
            ai_model_option = loaded_settings.ai.as_ref().and_then(|a| a.model.clone());
            tracing::info!("Settings loaded from persistence layer");

            // Apply loaded settings to the static handlers
            apply_loaded_settings(&loaded_settings);

            // ROADMAP 13.4: a fresh install has no persisted timezone, and the
            // compiled-in default ("UTC") would then contradict the host's own
            // zone — and the scheduled-task engine, which evaluates every cron
            // against this value. Seed it from the host once; from then on the
            // setting is authoritative (the settings screen applies it with
            // `timedatectl`, so the two agree).
            if loaded_settings
                .general
                .as_ref()
                .and_then(|g| g.timezone.as_deref())
                .map(str::trim)
                .filter(|tz| !tz.is_empty())
                .is_none()
            {
                let host_zone = crate::handlers::settings::system_timezone();
                if chrono_tz::Tz::from_str(&host_zone).is_ok() {
                    crate::handlers::settings::GENERAL_SETTINGS.write().timezone = Some(host_zone);
                }
            }
        } else {
            return;
        }
    }
    // Drop the lock before accessing AI_MODEL

    // Load AI model from persistence
    if let Some(model) = ai_model_option {
        let mut ai_model = crate::handlers::chat::AI_MODEL.write();
        *ai_model = model;
    }
}

/// Apply loaded settings to the global handler state
fn apply_loaded_settings(loaded: &PersistedSettings) {
    // General settings
    if let Some(general) = &loaded.general {
        use crate::handlers::settings::GENERAL_SETTINGS;
        let mut settings = GENERAL_SETTINGS.write();
        if let Some(name) = &general.server_name {
            settings.server_name = Some(name.clone());
        }
        if let Some(tz) = &general.timezone {
            settings.timezone = Some(tz.clone());
        }
        if let Some(language) = &general.language {
            // ROADMAP 13.4: the stored language is applied here, before the
            // HTTP server starts accepting requests, so the very first page
            // the admin sees is already in the language they chose. A value
            // from an older/hand-edited file that this build does not ship is
            // ignored rather than served as a broken locale.
            if let Ok(language) = crate::handlers::settings::normalize_language(language) {
                settings.language = Some(language.clone());
                crate::i18n::I18n::set_locale(&language);
            }
        }
    }

    // Email settings
    if let Some(email) = &loaded.email {
        use crate::handlers::settings::EMAIL_SETTINGS;
        let mut settings = EMAIL_SETTINGS.write();
        if let Some(server) = &email.smtp_server {
            settings.smtp_server = Some(server.clone());
        }
        if let Some(port) = email.smtp_port {
            settings.smtp_port = Some(port);
        }
        if let Some(user) = &email.smtp_username {
            settings.smtp_username = Some(user.clone());
        }
        if let Some(pwd) = &email.smtp_password {
            settings.smtp_password = Some(pwd.clone());
        }
        if let Some(from) = &email.from_address {
            settings.from_address = Some(from.clone());
        }
        if let Some(tls) = email.use_tls {
            settings.use_tls = Some(tls);
        }
    }

    // Update settings
    if let Some(update) = &loaded.update {
        use crate::handlers::settings::UPDATE_SETTINGS;
        let mut settings = UPDATE_SETTINGS.write();
        if let Some(freq) = update.check_frequency {
            settings.check_frequency = Some(freq);
        }
    }

    // Notification settings
    if let Some(notif) = &loaded.notifications {
        use crate::handlers::settings::NOTIFICATION_SETTINGS;
        let mut settings = NOTIFICATION_SETTINGS.write();
        if let Some(email) = &notif.email {
            settings.email = Some(email.clone());
        }
        if let Some(enable) = notif.enable_alerts {
            settings.enable_alerts = Some(enable);
        }
        if let Some(threshold) = notif.alert_threshold {
            settings.alert_threshold = Some(threshold);
        }
        if let Some(smtp) = notif.smtp_enabled {
            settings.smtp_enabled = Some(smtp);
        }
        if let Some(webhook_url) = &notif.webhook_url {
            settings.webhook_url = Some(webhook_url.clone());
        }
        if let Some(webhook_enabled) = notif.webhook_enabled {
            settings.webhook_enabled = Some(webhook_enabled);
        }
        settings.event_toggles = notif.event_toggles.clone();
        settings.webhook_format = notif.webhook_format.clone();
    }

    // Appearance settings
    if let Some(appearance) = loaded.appearance.as_ref() {
        use crate::handlers::settings::APPEARANCE_SETTINGS;
        let mut settings = APPEARANCE_SETTINGS.write();
        if let Some(wallpaper) = &appearance.wallpaper {
            // ROADMAP 26.1: a value that does not name a bundled photo (a
            // legacy custom upload, a gradient, a hand-edited file) is
            // dropped to "never picked" instead of being handed to the
            // desktop, where it used to reset silently on a failed JSON
            // parse.
            settings.wallpaper = Some(
                crate::handlers::settings::validate_wallpaper_field(wallpaper).unwrap_or_default(),
            );
        }
        if let Some(accent) = &appearance.accent_color {
            // Startup load *and* import funnel through here: rewrite the
            // legacy non-compliant default (see `handlers::settings::normalize_accent`).
            settings.accent_color = Some(crate::handlers::settings::normalize_accent(accent));
        }
        // `appearance.theme` / `appearance.density` are read for
        // backward compatibility and deliberately ignored (ROADMAP 26.2).
    }
}

/// Save the current AI model setting
pub fn save_ai_model(model: &str) {
    let persistence = SETTINGS_PERSISTENCE.read();
    if let Some(ref p) = *persistence {
        let mut loaded = p.load_all();
        loaded.ai = Some(PersistedAi {
            model: Some(model.to_string()),
        });
        if let Err(e) = p.storage.save("settings", &loaded) {
            tracing::error!("Failed to persist AI model: {}", e);
        }
    }
}

/// Load the AI model setting from persistence
pub fn load_ai_model() -> Option<String> {
    let persistence = SETTINGS_PERSISTENCE.read();
    persistence
        .as_ref()
        .and_then(|p| p.load_all().ai.and_then(|a| a.model))
}

/// Save all current settings (called after any setting update)
pub fn save_all_settings() {
    let persistence = SETTINGS_PERSISTENCE.read();
    if let Some(ref p) = *persistence {
        let general = GENERAL_SETTINGS.read();
        let email = EMAIL_SETTINGS.read();
        let update = UPDATE_SETTINGS.read();
        let notifications = NOTIFICATION_SETTINGS.read();
        let appearance = APPEARANCE_SETTINGS.read();

        let settings = PersistedSettings {
            schema_version: CURRENT_SETTINGS_SCHEMA,
            general: Some(PersistedGeneral {
                server_name: general.server_name.clone(),
                timezone: general.timezone.clone(),
                language: general.language.clone(),
            }),
            email: Some(PersistedEmail {
                smtp_server: email.smtp_server.clone(),
                smtp_port: email.smtp_port,
                smtp_username: email.smtp_username.clone(),
                smtp_password: email.smtp_password.clone(),
                from_address: email.from_address.clone(),
                use_tls: email.use_tls,
            }),
            update: Some(PersistedUpdate {
                check_frequency: update.check_frequency,
            }),
            notifications: Some(PersistedNotifications {
                email: notifications.email.clone(),
                enable_alerts: notifications.enable_alerts,
                alert_threshold: notifications.alert_threshold,
                smtp_enabled: notifications.smtp_enabled,
                webhook_url: notifications.webhook_url.clone(),
                webhook_enabled: notifications.webhook_enabled,
                event_toggles: notifications.event_toggles.clone(),
                webhook_format: notifications.webhook_format.clone(),
            }),
            appearance: Some(PersistedAppearance {
                // Never written again (ROADMAP 26.2); the reader keeps the
                // fields so an older settings.json still loads.
                theme: None,
                wallpaper: appearance.wallpaper.clone(),
                accent_color: appearance.accent_color.clone(),
                density: None,
            }),
            // Filled below from `AI_MODEL` (`settings_with_ai`): the model is
            // written by `save_ai_model`, not held in a settings static.
            ai: None,
        };

        drop(general);
        drop(email);
        drop(update);
        drop(notifications);
        drop(appearance);

        let ai_model = crate::handlers::chat::AI_MODEL.read().clone();
        let settings_with_ai = PersistedSettings {
            ai: Some(PersistedAi {
                model: Some(ai_model),
            }),
            ..settings
        };

        if let Err(e) = p.storage.save("settings", &settings_with_ai) {
            tracing::error!("Failed to persist settings: {}", e);
        } else {
            tracing::debug!("Settings persisted successfully");
        }
    }
}

/// Export settings as JSON
pub fn export_settings() -> Option<String> {
    let persistence = SETTINGS_PERSISTENCE.read();
    persistence.as_ref().and_then(|p| p.export().ok())
}

/// Import settings from JSON
pub fn import_settings(json: &str) -> Result<PersistedSettings, String> {
    let persistence = SETTINGS_PERSISTENCE.read();
    match &*persistence {
        Some(p) => {
            let settings = p.import(json)?;
            apply_loaded_settings(&settings);
            save_all_settings();
            Ok(settings)
        }
        None => Err("Settings persistence not initialized".to_string()),
    }
}

/// Reset settings to defaults
pub fn reset_settings() -> Result<(), String> {
    let persistence = SETTINGS_PERSISTENCE.read();
    match &*persistence {
        Some(p) => {
            p.reset()?;
            Ok(())
        }
        None => Err("Settings persistence not initialized".to_string()),
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the settings persistence layer. The security-relevant
    //! guarantee here is that `export()` never leaks the SMTP password and
    //! that re-importing a (redacted) export does not wipe the configured
    //! one — see `SettingsPersistence::export`'s own doc comment.
    //!
    //! Every test uses its own `tempfile::tempdir()` for the on-disk store,
    //! so nothing touches a real deployment's `data/settings.json`. The one
    //! test that exercises `apply_loaded_settings` mutates the process-global
    //! `*_SETTINGS` statics, so it serialises behind a mutex and restores
    //! them.
    //!
    //! Run: `cargo test settings_persistence::tests`

    use super::*;
    use std::sync::Mutex;

    static GLOBAL_STATE_LOCK: Mutex<()> = Mutex::new(());

    fn persistence() -> (SettingsPersistence, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let p = SettingsPersistence::new(dir.path().to_string_lossy().to_string());
        (p, dir)
    }

    fn email_with_password(pw: &str) -> EmailSettings {
        EmailSettings {
            smtp_server: Some("mail.example.com".to_string()),
            smtp_port: Some(587),
            smtp_username: Some("bot@example.com".to_string()),
            smtp_password: Some(pw.to_string()),
            from_address: Some("bot@example.com".to_string()),
            use_tls: Some(true),
        }
    }

    fn save(p: &SettingsPersistence, email: &EmailSettings, appearance: &AppearanceSettings) {
        p.save_all(
            &GeneralSettings::default(),
            email,
            &UpdateSettings::default(),
            &NotificationSettings::default(),
            appearance,
        )
        .expect("save_all");
    }

    // -- export() redaction ------------------------------------------

    #[test]
    fn test_export_redacts_the_smtp_password() {
        let (p, _dir) = persistence();
        save(
            &p,
            &email_with_password("hunter2-super-secret"),
            &AppearanceSettings::default(),
        );

        let json = p.export().expect("export");
        assert!(
            !json.contains("hunter2-super-secret"),
            "the plaintext SMTP password must never appear in an export: {json}"
        );

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["email"]["smtp_password"], serde_json::Value::Null);
        // The rest of the email block is still exported.
        assert_eq!(value["email"]["smtp_server"], "mail.example.com");
        assert_eq!(value["email"]["smtp_username"], "bot@example.com");
    }

    #[test]
    fn test_export_does_not_mutate_the_on_disk_password() {
        let (p, _dir) = persistence();
        save(
            &p,
            &email_with_password("on-disk-secret"),
            &AppearanceSettings::default(),
        );

        let _ = p.export().expect("export");

        // The redaction happens on a clone — the stored file still has it,
        // so the running process keeps working after an export.
        let reloaded = p.load_all();
        assert_eq!(
            reloaded.email.and_then(|e| e.smtp_password).as_deref(),
            Some("on-disk-secret")
        );
    }

    #[test]
    fn test_export_handles_a_store_with_no_email_block() {
        let (p, _dir) = persistence();
        // Nothing saved yet — load_all() returns defaults, export must still
        // produce valid JSON rather than panic.
        let json = p.export().expect("export of an empty store");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.is_object());
    }

    // -- import() round-trip ---------------------------------------

    #[test]
    fn test_import_round_trips_the_appearance_block() {
        let (p, _dir) = persistence();
        let appearance = AppearanceSettings {
            wallpaper: Some("/wallpapers/hv-07.webp".to_string()),
            accent_color: Some("#e2725b".to_string()),
        };
        save(&p, &EmailSettings::default(), &appearance);

        let exported = p.export().expect("export");
        let (p2, _dir2) = persistence();
        let imported = p2.import(&exported).expect("import");

        let a = imported.to_appearance();
        assert_eq!(a.wallpaper.as_deref(), Some("/wallpapers/hv-07.webp"));
        assert_eq!(a.accent_color.as_deref(), Some("#e2725b"));

        // …and it landed on p2's disk.
        assert_eq!(
            p2.load_all()
                .appearance
                .and_then(|x| x.accent_color)
                .as_deref(),
            Some("#e2725b")
        );
    }

    /// ROADMAP 26.2: `theme` and `density` are no longer written, and a
    /// `settings.json` left by an older release (which still carries them)
    /// must keep loading — they are simply ignored, never applied.
    #[test]
    fn test_legacy_theme_and_density_on_disk_load_and_are_ignored() {
        let (p, _dir) = persistence();
        let legacy = serde_json::json!({
            "schema_version": 1,
            "appearance": {
                "theme": "dark",
                "density": "compact",
                "wallpaper": "/wallpapers/hv-05.webp",
                "accent_color": "#123456"
            }
        });
        p.import(&legacy.to_string())
            .expect("a legacy blob must import");

        let loaded = p.load_all().appearance.expect("appearance block");
        // Read for compatibility…
        assert_eq!(loaded.theme.as_deref(), Some("dark"));
        assert_eq!(loaded.density.as_deref(), Some("compact"));
        // …but the live model has no such field, and a re-export stops
        // writing them.
        let reapplied = p.export().expect("export");
        let value: serde_json::Value = serde_json::from_str(&reapplied).unwrap();
        assert!(value["appearance"].get("theme").is_none(), "{value}");
        assert!(value["appearance"].get("density").is_none(), "{value}");
        assert_eq!(value["appearance"]["wallpaper"], "/wallpapers/hv-05.webp");
        assert_eq!(value["appearance"]["accent_color"], "#123456");
    }

    /// ROADMAP 26.1: a stored wallpaper that does not name a bundled photo
    /// is dropped on load rather than handed to a desktop that would silently
    /// reset it. The accent is untouched.
    #[test]
    fn test_apply_loaded_settings_drops_an_off_list_wallpaper() {
        let saved = APPEARANCE_SETTINGS.read().clone();

        apply_loaded_settings(&PersistedSettings {
            appearance: Some(PersistedAppearance {
                theme: None,
                wallpaper: Some("https://evil.example/x.webp".to_string()),
                accent_color: Some("#123456".to_string()),
                density: None,
            }),
            ..Default::default()
        });

        let appearance = APPEARANCE_SETTINGS.read().clone();
        assert_eq!(appearance.wallpaper.as_deref(), Some(""));
        assert_eq!(appearance.accent_color.as_deref(), Some("#123456"));

        // A valid stored value survives, normalised to the bare URL.
        apply_loaded_settings(&PersistedSettings {
            appearance: Some(PersistedAppearance {
                theme: None,
                wallpaper: Some("  /wallpapers/hv-09.webp ".to_string()),
                accent_color: None,
                density: None,
            }),
            ..Default::default()
        });
        assert_eq!(
            APPEARANCE_SETTINGS.read().wallpaper.as_deref(),
            Some("/wallpapers/hv-09.webp")
        );

        *APPEARANCE_SETTINGS.write() = saved;
    }

    #[test]
    fn test_import_rejects_malformed_json() {
        let (p, _dir) = persistence();
        assert!(p.import("{ not json").is_err());
        assert!(p.import("[]").is_err()); // wrong shape
    }

    // -- PersistedSettings mapping --------------------------------

    #[test]
    fn test_persisted_settings_mapping_is_none_when_a_block_is_absent() {
        let empty = PersistedSettings::default();
        let email = empty.to_email();
        assert!(email.smtp_server.is_none() && email.smtp_password.is_none());
        let appearance = empty.to_appearance();
        assert!(appearance.wallpaper.is_none() && appearance.accent_color.is_none());
    }

    // -- apply_loaded_settings preserves a redacted password -------

    #[test]
    fn test_reimporting_a_redacted_export_keeps_the_configured_password() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Snapshot the globals we touch so the test is self-contained.
        let saved_email = EMAIL_SETTINGS.read().clone();
        let saved_appearance = APPEARANCE_SETTINGS.read().clone();

        *EMAIL_SETTINGS.write() = email_with_password("live-password");

        // A redacted export: password is None, everything else is Some.
        let redacted = PersistedSettings {
            email: Some(PersistedEmail {
                smtp_server: Some("new.mail.example.com".to_string()),
                smtp_port: Some(465),
                smtp_username: Some("bot@example.com".to_string()),
                smtp_password: None,
                from_address: Some("bot@example.com".to_string()),
                use_tls: Some(false),
            }),
            appearance: Some(PersistedAppearance {
                theme: Some("dark".to_string()),
                wallpaper: None,
                accent_color: Some("#123456".to_string()),
                density: None,
            }),
            ..Default::default()
        };

        apply_loaded_settings(&redacted);

        let email = EMAIL_SETTINGS.read().clone();
        // The redacted `None` password left the live one untouched…
        assert_eq!(email.smtp_password.as_deref(), Some("live-password"));
        // …while the fields that were `Some` were applied.
        assert_eq!(email.smtp_server.as_deref(), Some("new.mail.example.com"));
        assert_eq!(email.use_tls, Some(false));

        let appearance = APPEARANCE_SETTINGS.read().clone();
        assert_eq!(appearance.accent_color.as_deref(), Some("#123456"));

        // Restore the globals for any other test in this binary.
        *EMAIL_SETTINGS.write() = saved_email;
        *APPEARANCE_SETTINGS.write() = saved_appearance;
    }

    // -- ROADMAP 13.4/13.5: language, schema version, import validation ---

    #[test]
    fn test_the_general_block_round_trips_the_language() {
        let (p, _dir) = persistence();
        let mut general = GeneralSettings::default();
        general.language = Some("fr".to_string());
        p.save_all(
            &general,
            &EmailSettings::default(),
            &UpdateSettings::default(),
            &NotificationSettings::default(),
            &AppearanceSettings::default(),
        )
        .unwrap();

        let loaded = p.load_all();
        assert_eq!(
            loaded.to_general().language.as_deref(),
            Some("fr"),
            "the language is a persisted setting, not process state"
        );
    }

    #[test]
    fn test_an_export_carries_the_current_schema_version() {
        let (p, _dir) = persistence();
        save(&p, &email_with_password("pw"), &AppearanceSettings::default());
        let json: serde_json::Value = serde_json::from_str(&p.export().unwrap()).unwrap();
        assert_eq!(
            json["schema_version"], CURRENT_SETTINGS_SCHEMA,
            "an export must say which schema it is"
        );
    }

    #[test]
    fn test_an_import_from_a_newer_schema_is_refused() {
        let (p, _dir) = persistence();
        let mut newer = PersistedSettings::default();
        newer.schema_version = CURRENT_SETTINGS_SCHEMA + 1;
        let json = serde_json::to_string(&newer).unwrap();
        let err = p.import(&json).unwrap_err();
        assert!(
            err.contains("newer HomeVault"),
            "the refusal must name the version problem: {err}"
        );
    }

    #[test]
    fn test_a_pre_versioning_import_is_still_accepted() {
        // Blobs written before the field existed have no `schema_version`.
        let (p, _dir) = persistence();
        let json = r#"{"general":{"server_name":"NAS","timezone":"Europe/Paris"}}"#;
        let imported = p.import(json).expect("legacy blob is accepted");
        assert_eq!(imported.schema_version, CURRENT_SETTINGS_SCHEMA);
        assert_eq!(imported.to_general().server_name.as_deref(), Some("NAS"));
    }

    #[test]
    fn test_import_validation_names_the_offending_field() {
        let mut bad_port = PersistedSettings::default();
        bad_port.email = Some(PersistedEmail {
            smtp_server: Some("mail.example.com".to_string()),
            smtp_port: Some(0),
            smtp_username: None,
            smtp_password: None,
            from_address: None,
            use_tls: Some(true),
        });
        let err = validate_import(&bad_port).unwrap_err();
        assert!(err.contains("smtp_port"), "{err}");

        let mut bad_tz = PersistedSettings::default();
        bad_tz.general = Some(PersistedGeneral {
            server_name: None,
            timezone: Some("Mars/Olympus".to_string()),
            language: None,
        });
        let err = validate_import(&bad_tz).unwrap_err();
        assert!(err.contains("timezone"), "{err}");

        let mut bad_url = PersistedSettings::default();
        bad_url.notifications = Some(PersistedNotifications {
            email: None,
            enable_alerts: None,
            alert_threshold: Some(80),
            smtp_enabled: None,
            webhook_url: Some("ftp://example.com/hook".to_string()),
            webhook_enabled: Some(true),
            event_toggles: None,
            webhook_format: None,
        });
        let err = validate_import(&bad_url).unwrap_err();
        assert!(err.contains("webhook_url"), "{err}");

        let mut bad_threshold = PersistedSettings::default();
        bad_threshold.notifications = Some(PersistedNotifications {
            email: None,
            enable_alerts: None,
            alert_threshold: Some(200),
            smtp_enabled: None,
            webhook_url: None,
            webhook_enabled: None,
            event_toggles: None,
            webhook_format: None,
        });
        assert!(validate_import(&bad_threshold)
            .unwrap_err()
            .contains("alert_threshold"));

        let mut bad_language = PersistedSettings::default();
        bad_language.general = Some(PersistedGeneral {
            server_name: None,
            timezone: None,
            language: Some("it".to_string()),
        });
        assert!(validate_import(&bad_language)
            .unwrap_err()
            .contains("language"));
    }

    #[test]
    fn test_a_valid_import_passes_validation() {
        let settings = PersistedSettings {
            schema_version: CURRENT_SETTINGS_SCHEMA,
            general: Some(PersistedGeneral {
                server_name: Some("NAS".to_string()),
                timezone: Some("Europe/Paris".to_string()),
                language: Some("fr".to_string()),
            }),
            ..PersistedSettings::default()
        };
        assert!(validate_import(&settings).is_ok());
    }
}
