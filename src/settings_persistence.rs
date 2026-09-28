//! Settings persistence layer - saves settings to JSON files
//! 
//! This module provides automatic persistence for all HomeVault settings,
//! ensuring configuration survives server restarts.

use crate::handlers::settings::{
    APPEARANCE_SETTINGS, EMAIL_SETTINGS, GENERAL_SETTINGS, NOTIFICATION_SETTINGS, UPDATE_SETTINGS,
    AppearanceSettings, EmailSettings, GeneralSettings, NotificationSettings, UpdateFrequency, UpdateSettings,
};
use crate::storage::json::JsonStorage;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Global settings persistence manager
static SETTINGS_PERSISTENCE: Lazy<RwLock<Option<SettingsPersistence>>> = Lazy::new(|| RwLock::new(None));

/// Settings persistence data structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedSettings {
    pub general: Option<PersistedGeneral>,
    pub email: Option<PersistedEmail>,
    pub update: Option<PersistedUpdate>,
    pub notifications: Option<PersistedNotifications>,
    pub appearance: Option<PersistedAppearance>,
    pub ai: Option<PersistedAi>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAi {
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedGeneral {
    pub server_name: Option<String>,
    pub timezone: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            webhook_url: self.notifications.as_ref().and_then(|n| n.webhook_url.clone()),
            webhook_enabled: self.notifications.as_ref().and_then(|n| n.webhook_enabled),
        }
    }

    pub fn to_appearance(&self) -> AppearanceSettings {
        AppearanceSettings {
            theme: self.appearance.as_ref().and_then(|a| a.theme.clone()),
            wallpaper: self.appearance.as_ref().and_then(|a| a.wallpaper.clone()),
            accent_color: self.appearance.as_ref().and_then(|a| a.accent_color.clone()),
            density: self.appearance.as_ref().and_then(|a| a.density.clone()),
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
            general: Some(PersistedGeneral {
                server_name: general.server_name.clone(),
                timezone: general.timezone.clone(),
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
            }),
            appearance: Some(PersistedAppearance {
                theme: appearance.theme.clone(),
                wallpaper: appearance.wallpaper.clone(),
                accent_color: appearance.accent_color.clone(),
                density: appearance.density.clone(),
            }),
            ai: None,
        };

        self.storage.save("settings", &settings).map_err(|e| e.to_string())
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
    pub fn export(&self) -> Result<String, String> {
        let mut settings = self.load_all();
        if let Some(email) = settings.email.as_mut() {
            email.smtp_password = None;
        }
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())
    }

    /// Import settings from JSON string
    pub fn import(&self, json: &str) -> Result<PersistedSettings, String> {
        let settings: PersistedSettings = serde_json::from_str(json).map_err(|e| e.to_string())?;
        self.storage.save("settings", &settings).map_err(|e| e.to_string())?;
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
        // Built-in HTTPS/TLS config (data/tls.json) is separate from settings.json
        // but is still "a setting" from the admin's point of view — clear it too.
        crate::tls::clear_settings();
        Ok(())
    }
}

/// Initialize the settings persistence system
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
    }
    
    // Appearance settings
    if let Some(appearance) = &loaded.appearance {
        use crate::handlers::settings::APPEARANCE_SETTINGS;
        let mut settings = APPEARANCE_SETTINGS.write();
        if let Some(theme) = &appearance.theme {
            settings.theme = Some(theme.clone());
        }
        if let Some(wallpaper) = &appearance.wallpaper {
            settings.wallpaper = Some(wallpaper.clone());
        }
        if let Some(accent) = &appearance.accent_color {
            // Startup load *and* import funnel through here: rewrite the
            // legacy non-compliant default (see `handlers::settings::normalize_accent`).
            settings.accent_color = Some(crate::handlers::settings::normalize_accent(accent));
        }
        if let Some(density) = &appearance.density {
            settings.density = Some(density.clone());
        }
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
    persistence.as_ref().and_then(|p| {
        p.load_all().ai.and_then(|a| a.model)
    })
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
            general: Some(PersistedGeneral {
                server_name: general.server_name.clone(),
                timezone: general.timezone.clone(),
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
            }),
            appearance: Some(PersistedAppearance {
                theme: appearance.theme.clone(),
                wallpaper: appearance.wallpaper.clone(),
                accent_color: appearance.accent_color.clone(),
                density: appearance.density.clone(),
            }),
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
        save(&p, &email_with_password("on-disk-secret"), &AppearanceSettings::default());

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
            theme: Some("dark".to_string()),
            wallpaper: Some("/wallpapers/hv-07.webp".to_string()),
            accent_color: Some("#e2725b".to_string()),
            density: Some("compact".to_string()),
        };
        save(&p, &EmailSettings::default(), &appearance);

        let exported = p.export().expect("export");
        let (p2, _dir2) = persistence();
        let imported = p2.import(&exported).expect("import");

        let a = imported.to_appearance();
        assert_eq!(a.theme.as_deref(), Some("dark"));
        assert_eq!(a.wallpaper.as_deref(), Some("/wallpapers/hv-07.webp"));
        assert_eq!(a.accent_color.as_deref(), Some("#e2725b"));
        assert_eq!(a.density.as_deref(), Some("compact"));

        // …and it landed on p2's disk.
        assert_eq!(
            p2.load_all().appearance.and_then(|x| x.accent_color).as_deref(),
            Some("#e2725b")
        );
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
        assert!(appearance.theme.is_none() && appearance.accent_color.is_none());
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
}
