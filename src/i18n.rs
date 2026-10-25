/* =============================================================================
 * File          : i18n.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Server-side translations: parses the bundled per-language TOML tables
 *   (src/i18n, one .toml per language), caches them in a process-wide lock,
 *   and serves them to the frontend plus the RAG document indexers. Shared
 *   by the translations endpoint and server-side message building.
 *
 * Dependencies:
 *   - once_cell, parking_lot (>= 0.12), serde (>= 1.0)
 *
 * Usage:
 *   handlers/settings.rs serves translations; rag_indexers.rs indexes the
 *   documents (see docs/guides/i18n.md).
 * =============================================================================
 */

//! Server-side translation tables (src/i18n/*.toml) served to the frontend
//! and indexed for the RAG knowledge base.

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

/// Get the translation for a key. Can be used as a filter in Askama templates.
/// Usage in template: {{ "nav.dashboard" | t }}
pub fn t(key: &str) -> String {
    let locale = CURRENT_LOCALE.read().clone();
    let i18n = I18n::new(&locale);
    i18n.translate(key)
}

/// Get the translation with variables. Can be used as a filter in Askama templates.
/// Usage in template: {{ "snapshot.rollback_confirm" | t_var("name", snapshot.name) }}
pub fn t_var<S: std::fmt::Display>(key: &str, name: &str, value: S) -> String {
    let locale = CURRENT_LOCALE.read().clone();
    let i18n = I18n::new(&locale);
    i18n.t_with_vars(key, &[(name, &value.to_string())])
}

/// Currently active locale (e.g. `"en"`, `"fr"`).
pub static CURRENT_LOCALE: Lazy<RwLock<String>> = Lazy::new(|| RwLock::new("en".to_string()));
/// Map of locale → (key → translated string).
pub static TRANSLATIONS: Lazy<RwLock<HashMap<String, HashMap<String, String>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Internationalization helper for looking up translations with optional variable substitution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct I18n {
    locale: String,
}

impl I18n {
    pub fn new(locale: &str) -> Self {
        Self {
            locale: locale.to_string(),
        }
    }

    pub fn t(&self, key: &str) -> String {
        self.translate(key)
    }

    pub fn t_with_vars(&self, key: &str, vars: &[(&str, &str)]) -> String {
        let mut text = self.translate(key);
        for (name, value) in vars {
            text = text.replace(&format!("%{}%", name), value);
        }
        text
    }

    pub fn translate(&self, key: &str) -> String {
        let translations = TRANSLATIONS.read();
        if let Some(locale_translations) = translations.get(&self.locale) {
            if let Some(value) = locale_translations.get(key) {
                return value.clone();
            }
        }

        if self.locale != "en" {
            if let Some(en_translations) = translations.get("en") {
                if let Some(value) = en_translations.get(key) {
                    return value.clone();
                }
            }
        }

        key.to_string()
    }

    pub fn locale(&self) -> &str {
        &self.locale
    }

    pub fn set_locale(locale: &str) {
        let mut current = CURRENT_LOCALE.write();
        *current = locale.to_string();
        info!("Locale changed to: {}", locale);
    }

    pub fn get_locale() -> String {
        CURRENT_LOCALE.read().clone()
    }

    pub fn available_locales() -> Vec<String> {
        let translations = TRANSLATIONS.read();
        translations.keys().cloned().collect()
    }

    /// Returns the flattened translation map for a locale, with English as fallback.
    /// Used by the frontend to load translations for the SPA.
    pub fn get_translations(locale: &str) -> HashMap<String, String> {
        let translations = TRANSLATIONS.read();
        let mut result: HashMap<String, String> = HashMap::new();

        if let Some(en) = translations.get("en") {
            result.extend(en.clone());
        }

        if locale != "en" {
            if let Some(loc) = translations.get(locale) {
                result.extend(loc.clone());
            }
        }

        result
    }
}

/// Initializes translations from the embedded `src/i18n/*.toml` content
/// (baked into the binary at compile time via `include_str!` — see
/// `load_embedded_translations`), so a bare copy of the binary needs no
/// external translation files to serve a fully translated UI.
///
/// `translations_path`, when given, additionally overlays translations read
/// from disk at that path, winning over the embedded defaults on any key
/// collision — for a developer iterating on `.toml` files without wanting
/// to recompile. Nothing in this codebase currently passes `Some(..)`; every
/// call site uses `None`, which now needs no filesystem access at all.
pub fn init_i18n(translations_path: Option<&Path>) {
    load_embedded_translations();

    let Some(base_path) = translations_path else {
        return;
    };

    let locales = ["en", "fr", "es", "de"];
    let mut translations = TRANSLATIONS.write();

    for locale in locales {
        let file_path = base_path.join(format!("{}.toml", locale));
        if file_path.exists() {
            match std::fs::read_to_string(&file_path) {
                Ok(content) => match toml::from_str::<toml::Value>(&content) {
                    Ok(parsed) => {
                        let flat = flatten_toml(&parsed, "");
                        translations
                            .entry(locale.to_string())
                            .or_default()
                            .extend(flat);
                        info!("Loaded translation override for locale: {}", locale);
                    }
                    Err(e) => {
                        warn!("Failed to parse {}: {}", file_path.display(), e);
                    }
                },
                Err(e) => {
                    warn!("Failed to read {}: {}", file_path.display(), e);
                }
            }
        }
    }
}

/// The real `src/i18n/*.toml` files, embedded into the binary at compile
/// time. Previously this fallback was a large, hand-maintained JSON blob
/// duplicating the `.toml` content — which had drifted badly out of sync
/// (e.g. it still had `nav.*` keys from an old, pre-SPA UI that no longer
/// exist in the real `.toml` files at all). Embedding the actual `.toml`
/// source directly means the embedded fallback can never drift again: it's
/// always exactly what was checked in at build time.
const EN_TOML: &str = include_str!("i18n/en.toml");
const FR_TOML: &str = include_str!("i18n/fr.toml");
const ES_TOML: &str = include_str!("i18n/es.toml");
const DE_TOML: &str = include_str!("i18n/de.toml");

/// The raw embedded `.toml` sources for every supported locale, as
/// `(locale, raw TOML)` pairs — the exact same content `init_i18n` loads.
///
/// Exposed so other subsystems (the RAG knowledge base's i18n indexer, for
/// one) can index or inspect translations from the binary alone, instead of
/// re-opening `src/i18n/*.toml` beside the process — a path that only exists
/// in a source checkout, never on a deployed host.
pub fn embedded_translation_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("en", EN_TOML),
        ("fr", FR_TOML),
        ("es", ES_TOML),
        ("de", DE_TOML),
    ]
}

fn load_embedded_translations() {
    let mut translations = TRANSLATIONS.write();

    for (locale, toml_src) in embedded_translation_sources() {
        match toml::from_str::<toml::Value>(toml_src) {
            Ok(parsed) => {
                let flat = flatten_toml(&parsed, "");
                translations
                    .entry(locale.to_string())
                    .or_default()
                    .extend(flat);
            }
            Err(e) => {
                warn!("Failed to parse embedded {}.toml: {}", locale, e);
            }
        }
    }
}

fn flatten_toml(value: &toml::Value, prefix: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();

    if let toml::Value::Table(table) = value {
        for (key, val) in table {
            let full_key = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };

            match val {
                toml::Value::String(s) => {
                    result.insert(full_key, s.clone());
                }
                toml::Value::Table(nested) => {
                    let nested_map = flatten_toml(&toml::Value::Table(nested.clone()), &full_key);
                    result.extend(nested_map);
                }
                _ => {}
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // These use `common.save`/`common.cancel` (stable, present in every
    // locale's `[common]` table) rather than the `nav.*` keys the previous
    // version of this test suite used — those belonged to an old, pre-SPA
    // UI and no longer exist in the real `src/i18n/*.toml` files at all,
    // which these tests now read from directly (embedded at compile time).

    #[test]
    fn test_translation() {
        init_i18n(None);
        let i18n = I18n::new("en");
        assert_eq!(i18n.t("common.save"), "Save");
    }

    #[test]
    fn test_french_translation() {
        init_i18n(None);
        let i18n = I18n::new("fr");
        assert_eq!(i18n.t("common.save"), "Enregistrer");
        assert_eq!(i18n.t("common.cancel"), "Annuler");
    }

    #[test]
    fn test_missing_key() {
        init_i18n(None);
        let i18n = I18n::new("en");
        assert_eq!(i18n.t("nonexistent.key"), "nonexistent.key");
    }

    #[test]
    fn test_spanish_translation() {
        init_i18n(None);
        let i18n = I18n::new("es");
        assert_eq!(i18n.t("common.save"), "Guardar");
        assert_eq!(i18n.t("common.cancel"), "Cancelar");
    }

    #[test]
    fn test_german_translation() {
        init_i18n(None);
        let i18n = I18n::new("de");
        assert_eq!(i18n.t("common.save"), "Speichern");
        assert_eq!(i18n.t("common.cancel"), "Abbrechen");
    }

    #[test]
    fn test_spanish_fallback_to_english() {
        init_i18n(None);
        let i18n = I18n::new("es");
        assert_eq!(i18n.t("nonexistent.key"), "nonexistent.key");
    }

    #[test]
    fn test_german_fallback_to_english() {
        init_i18n(None);
        let i18n = I18n::new("de");
        assert_eq!(i18n.t("nonexistent.key"), "nonexistent.key");
    }

    // The embedded sources are what the RAG i18n indexer now consumes instead
    // of reading `src/i18n/*.toml` off disk — a path that only exists in a
    // checkout, never on a deployed host. All four must be present and parse.
    #[test]
    fn test_embedded_translation_sources_are_complete() {
        let sources = embedded_translation_sources();
        assert_eq!(sources.len(), 4, "all four locales must be embedded");

        let mut codes: Vec<&str> = sources.iter().map(|(code, _)| *code).collect();
        codes.sort_unstable();
        assert_eq!(codes, vec!["de", "en", "es", "fr"]);

        for (code, toml_src) in &sources {
            assert!(
                !toml_src.is_empty(),
                "embedded {code}.toml must not be empty"
            );
            toml::from_str::<toml::Value>(toml_src)
                .unwrap_or_else(|e| panic!("embedded {code}.toml must parse: {e}"));
        }
    }

    /// ROADMAP 26.2 removed the `theme`/`density` appearance no-ops and ROADMAP
    /// 26.1 added two new strings. The four locales are at key parity, so a key
    /// added to (or dropped from) one file alone would surface as a raw key in
    /// the UI of the other three — pin it here.
    #[test]
    fn test_appearance_keys_are_at_parity_and_the_removed_one_is_gone() {
        let sources = embedded_translation_sources();
        let mut maps = std::collections::HashMap::new();
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            maps.insert(*code, flatten_toml(&parsed, ""));
        }

        // Present in every locale, non-empty.
        for key in [
            "settings.wallpaper_managed_by_admin",
            "settings.appearance_load_failed",
        ] {
            for (code, map) in &maps {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(!value.trim().is_empty(), "{key} is empty in {code}.toml");
            }
        }

        // Round 2 of 26.2: the dead label of the removed control is gone
        // everywhere, so nothing can resurrect the field by reusing it.
        for (code, map) in &maps {
            assert!(
                !map.contains_key("settings.density"),
                "settings.density must be gone from {code}.toml"
            );
        }
    }

    /// ROADMAP 28.3 added three activity `kind` slugs (a RAID resync, a pool
    /// scrub, a Syncthing folder catching up). The desktop renders a row's name
    /// as `desktop.activity_kind_<slug>`, falling back to the **raw slug** when
    /// the key is missing — so a locale that missed one would show
    /// `raid_resync` on screen. Pin the four locales here instead.
    #[test]
    fn activity_poller_kinds_are_at_parity_in_every_locale() {
        let sources = embedded_translation_sources();
        assert_eq!(sources.len(), 4, "four locales ship");
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            let map = flatten_toml(&parsed, "");
            for key in [
                "desktop.activity_kind_raid_resync",
                "desktop.activity_kind_pool_scrub",
                "desktop.activity_kind_syncthing_sync",
            ] {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(!value.trim().is_empty(), "{key} is empty in {code}.toml");
                // The label must not *be* the slug: `raid_resync` on screen is
                // the exact failure this key exists to prevent.
                assert!(
                    !value.contains("raid_resync")
                        && !value.contains("pool_scrub")
                        && !value.contains("syncthing_sync"),
                    "{key} in {code}.toml renders a raw slug: {value:?}"
                );
            }
        }
    }
}
