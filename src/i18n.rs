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

    /// Tranche T1 (2026-10-29) added the reachability surfaces: certificate
    /// automation, dynamic DNS, the reachability checklist, trusted devices and
    /// the rescue address. Every one of them is a whole card rendered from the
    /// translation map, so a key missing from one locale would show a raw key in
    /// exactly that locale. Pin the prefix families instead of listing ~80 keys
    /// one by one: any key that exists in English must exist and be non-empty in
    /// the other three.
    #[test]
    fn test_the_t1_surfaces_are_translated_in_every_locale() {
        let sources = embedded_translation_sources();
        let mut maps = std::collections::HashMap::new();
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            maps.insert(*code, flatten_toml(&parsed, ""));
        }

        let english = maps.get("en").expect("en is embedded");
        let prefixes = [
            "settings.acme_",
            "settings.ddns_",
            "settings.reachability_",
            "settings.tls_mode_acme",
            "account.section_devices",
            "account.devices_",
            "account.section_rescue",
            "account.rescue_",
            "login.email_recovery_",
            "login.remember_device",
        ];
        let expected: Vec<&String> = english
            .keys()
            .filter(|key| prefixes.iter().any(|p| key.starts_with(p)))
            .collect();
        assert!(
            expected.len() >= 60,
            "the T1 surfaces should carry their own strings, found {}",
            expected.len()
        );

        for (code, map) in &maps {
            for key in &expected {
                let value = map
                    .get(*key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {code}.toml — a blank label is worse than a raw key"
                );
            }
        }

        // The ACME-mode radio is useless without its explanation: the label and
        // the hint are one control, and the hint must name the automation card.
        for (code, map) in &maps {
            let hint = map
                .get("settings.tls_mode_acme_hint")
                .unwrap_or_else(|| panic!("settings.tls_mode_acme_hint missing from {code}.toml"));
            assert!(
                hint.to_lowercase().contains("automat") || hint.contains("Automat"),
                "the ACME mode hint in {code}.toml must point at certificate automation: {hint}"
            );
        }
    }

    /// QW-13 added the Explorer's transfer tray, whose whole chrome — the
    /// three zone labels, the badges, the empty state, the per-row cancel, the
    /// state lines and the history section — is rendered from the translation
    /// map, and whose strings use the `%name%`/`%count%` placeholders the
    /// frontend substitutes. A key present in English and missing elsewhere
    /// would show a raw `explorer.transfer_*` key in exactly that locale, so
    /// the family is pinned across all four files.
    #[test]
    fn explorer_transfer_keys_are_at_parity_in_every_locale() {
        let sources = embedded_translation_sources();
        assert_eq!(sources.len(), 4, "four locales ship");
        let mut by_locale = std::collections::HashMap::new();
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            by_locale.insert(*code, flatten_toml(&parsed, ""));
        }

        let english = by_locale.get("en").expect("en is embedded");
        let prefixes = ["explorer.transfer", "explorer.transfers"];
        let expected: Vec<String> = english
            .keys()
            .filter(|key| prefixes.iter().any(|p| key.starts_with(p)))
            .cloned()
            .collect();
        assert!(
            expected.len() >= 20,
            "the transfer tray carries its own strings, found {}",
            expected.len()
        );

        for (code, map) in &by_locale {
            for key in &expected {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {code}.toml — a blank label is worse than a raw key"
                );
            }
        }

        // The cancel label and the zone badge are formatted, not concatenated:
        // the placeholder must be `%name%`/`%count%` in every locale, because
        // that is what the frontend's `translate()` replaces.
        for (code, map) in &by_locale {
            let cancel = map
                .get("explorer.transfer_cancel")
                .unwrap_or_else(|| panic!("explorer.transfer_cancel missing from {code}.toml"));
            assert!(cancel.contains("%name%"), "{code}: {cancel}");
            let count = map
                .get("explorer.transfer_zone_count")
                .unwrap_or_else(|| panic!("explorer.transfer_zone_count missing from {code}.toml"));
            assert!(count.contains("%count%"), "{code}: {count}");
        }
    }

    /// QW-10 added the Security Advisor card, whose whole chrome (title,
    /// severities, summary, the two placeholders, the per-app deep-link
    /// labels) is rendered from the translation map. A key missing from one
    /// locale would show a raw `health.advisor_*` string in exactly that
    /// locale, and the placeholder names must match — the frontend replaces
    /// `%name%`, while a TOML interpolation written `{name}` would survive into
    /// the UI verbatim (the QW batch-B defect). Both are pinned here.
    #[test]
    fn security_advisor_keys_are_at_parity_in_every_locale() {
        let sources = embedded_translation_sources();
        assert_eq!(sources.len(), 4, "four locales ship");
        let mut by_locale = std::collections::HashMap::new();
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            by_locale.insert(*code, flatten_toml(&parsed, ""));
        }

        let english = by_locale.get("en").expect("en is embedded");
        let prefix = "health.advisor_";
        let expected: Vec<String> = english
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        assert!(
            expected.len() >= 20,
            "the advisor card should carry its own strings, found {}",
            expected.len()
        );

        for (code, map) in &by_locale {
            for key in &expected {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {code}.toml — a blank label is worse than a raw key"
                );
                // A TOML `{name}` interpolation is dead text in the frontend.
                assert!(
                    !value.contains('{'),
                    "{key} in {code}.toml uses a TOML interpolation the frontend cannot                      replace (expects %name%): {value}"
                );
            }
        }

        // The two placeholders are data, not prose: their names must match what
        // the card passes, in every locale.
        for (code, map) in &by_locale {
            for (key, placeholder) in [
                ("health.advisor_score_value", "%score%"),
                ("health.advisor_summary", "%pass%"),
            ] {
                let value = map.get(key).unwrap();
                assert!(
                    value.contains(placeholder),
                    "{key} in {code}.toml must interpolate {placeholder}: {value}"
                );
            }
            let all_checks = map.get("health.advisor_all_checks").unwrap();
            assert!(
                all_checks.contains("%count%"),
                "health.advisor_all_checks in {code}.toml must interpolate %count%: {all_checks}"
            );
        }

        // Every app the report can deep-link to has its own label; the card
        // falls back to the generic "Open" when one is missing, so a gap here
        // is a quiet degradation rather than a raw key — still worth pinning.
        for code in ["en", "fr", "de", "es"] {
            let map = by_locale.get(code).unwrap();
            for app in ["settings", "users", "shares", "account", "health"] {
                let key = format!("health.advisor_open_{app}");
                assert!(
                    map.contains_key(&key),
                    "{key} missing from {code}.toml (the card would fall back to a generic label)"
                );
            }
        }
    }

    /// QW-12 added the cross-root file provider to the ⌘K overlay: a result
    /// row's type badge, the "still looking" line, and the two honesty notes
    /// (a partial answer, a cut answer). A locale missing one would show a raw
    /// `search.*` key in the overlay, and `files_partial` must keep its
    /// `%count%` placeholder — the frontend replaces `%name%`, never `{name}`.
    #[test]
    fn file_search_keys_are_at_parity_in_every_locale() {
        let sources = embedded_translation_sources();
        assert_eq!(sources.len(), 4, "four locales ship");
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            let map = flatten_toml(&parsed, "");
            for key in [
                "search.type_file",
                "search.files_searching",
                "search.files_partial",
                "search.files_truncated",
                "search.files_unavailable",
            ] {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {code}.toml — a blank label is worse than a raw key"
                );
                assert!(
                    !value.contains('{'),
                    "{key} in {code}.toml uses a TOML interpolation the frontend cannot                      replace (expects %name%): {value}"
                );
            }
            let partial = map.get("search.files_partial").unwrap();
            assert!(
                partial.contains("%count%"),
                "search.files_partial in {code}.toml must interpolate %count%: {partial}"
            );
        }
    }

    /// ROADMAP P3.1 stage 3 added the third single sign-on tab. Its whole panel
    /// is rendered from `saml.*`, so a key present in one locale and missing
    /// from another would show a raw key in exactly that locale. Pin the family
    /// by prefix: any `saml.*` key in English must exist and be non-empty
    /// everywhere, and the counted footer string must keep `%total%`/`%local%`.
    #[test]
    fn saml_keys_are_at_parity_in_every_locale() {
        let sources = embedded_translation_sources();
        let mut maps = std::collections::HashMap::new();
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            maps.insert(*code, flatten_toml(&parsed, ""));
        }

        let english = maps.get("en").expect("en is embedded");
        let keys: Vec<String> = english
            .keys()
            .filter(|key| key.starts_with("saml."))
            .cloned()
            .collect();
        assert!(keys.len() >= 49, "the SAML panel ships 49 strings: {keys:?}");

        for (code, map) in &maps {
            for key in &keys {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {code}.toml — a blank label is worse than a raw key"
                );
            }
        }

        for (code, map) in &maps {
            for (key, placeholders) in [("saml.footer_counts", ["%total%", "%local%"])] {
                let value = map.get(key).unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                for placeholder in placeholders {
                    assert!(
                        value.contains(placeholder),
                        "{key} in {code}.toml must interpolate {placeholder}: {value}"
                    );
                }
            }
        }
    }

    /// Phase 1 P1.3 added passkeys: the Account card's chrome and the login
    /// screen's passkey button. A locale missing one would show a raw
    /// `account.passkeys_*` / `login.passkey_*` key exactly there, and a TOML
    /// `{name}` placeholder would survive into the UI verbatim.
    #[test]
    fn passkey_keys_are_at_parity_in_every_locale() {
        let sources = embedded_translation_sources();
        assert_eq!(sources.len(), 4, "four locales ship");
        let mut by_locale = std::collections::HashMap::new();
        for (code, toml_src) in &sources {
            let parsed: toml::Value = toml::from_str(toml_src).expect("parse");
            by_locale.insert(*code, flatten_toml(&parsed, ""));
        }

        let english = by_locale.get("en").expect("en is embedded");
        let account_keys: Vec<String> = english
            .keys()
            .filter(|key| {
                key.starts_with("account.passkeys_") || *key == "account.section_passkeys"
            })
            .cloned()
            .collect();
        let login_keys: Vec<String> = english
            .keys()
            .filter(|key| key.starts_with("login.passkey_"))
            .cloned()
            .collect();
        assert!(
            account_keys.len() >= 20,
            "the passkey card should carry its own strings, found {}",
            account_keys.len()
        );
        assert!(
            login_keys.len() >= 5,
            "the login button too, found {}",
            login_keys.len()
        );

        for (code, map) in &by_locale {
            for key in account_keys.iter().chain(login_keys.iter()) {
                let value = map
                    .get(key)
                    .unwrap_or_else(|| panic!("{key} missing from {code}.toml"));
                assert!(
                    !value.trim().is_empty(),
                    "{key} is empty in {code}.toml — a blank label is worse than a raw key"
                );
                assert!(
                    !value.contains('{'),
                    "{key} in {code}.toml uses a TOML interpolation the frontend cannot \
                     replace (expects %name%): {value}"
                );
            }
            // The two placeholders are data, not prose: their names must match
            // what the components pass, in every locale.
            for (key, placeholder) in [
                ("account.passkeys_revoke_message", "%label%"),
                ("account.passkeys_max", "%max%"),
                ("account.passkeys_origin", "%origin%"),
            ] {
                let value = map.get(key).unwrap();
                assert!(
                    value.contains(placeholder),
                    "{key} in {code}.toml must interpolate {placeholder}: {value}"
                );
            }
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
