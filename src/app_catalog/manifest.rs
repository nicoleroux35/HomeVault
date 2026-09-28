//! The `homevault-app.toml` manifest: the static metadata half of an app
//! package (the other half is a sibling `docker-compose.yml` template with
//! `{{DATA_DIR}}` / `{{PORT}}` placeholders). Parsed with the `toml` crate
//! already used for i18n - no new dependency.

use serde::{Deserialize, Serialize};

fn default_version() -> String {
    "1.0.0".to_string()
}

/// A parsed `homevault-app.toml`. Unknown keys are ignored so a manifest
/// written for a newer HomeVault still loads.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub manifest_version: u32,
    pub id: String,
    pub name: String,
    /// Package version - the version of *this template*, not the upstream
    /// image tag. Bumped by a store when the compose template changes;
    /// compared against the installed marker to offer a "template update".
    #[serde(default = "default_version")]
    pub version: String,
    pub category: String,
    /// Lucide icon name, used when the package ships no `icon.svg`.
    #[serde(default)]
    pub icon: Option<String>,
    /// The port the app listens on *inside* its main container.
    pub app_port: u16,
    /// The host port to publish by default. Falls back to `app_port`.
    #[serde(default)]
    pub default_port: Option<u16>,
    /// CPU architectures the app's images are published for
    /// (`amd64` / `arm64` / `armv7`). Empty = assume all.
    #[serde(default)]
    pub arch: Vec<String>,
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub tagline: Option<String>,
    pub description: String,
    #[serde(default)]
    pub description_fr: Option<String>,
    #[serde(default)]
    pub description_de: Option<String>,
    #[serde(default)]
    pub description_es: Option<String>,
    #[serde(default)]
    pub developer: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub release_notes: Option<String>,
    /// Bare ids of other apps in the same store this one needs. Phase A
    /// only surfaces a warning when a dependency isn't installed - it does
    /// not auto-install them.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

impl Manifest {
    pub fn parse(s: &str) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(s).map_err(|e| format!("invalid manifest: {e}"))?;
        if m.manifest_version != 1 {
            return Err(format!(
                "unsupported manifest_version {} (this HomeVault understands version 1)",
                m.manifest_version
            ));
        }
        if m.id.is_empty()
            || !m
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("invalid app id: {:?}", m.id));
        }
        Ok(m)
    }

    /// The host port to publish (chosen default, else the internal port).
    pub fn host_port(&self) -> u16 {
        self.default_port.unwrap_or(self.app_port)
    }

    /// The description in the requested UI language, falling back to English.
    /// `lang` is the 2-letter code from the i18n store (`en`/`fr`/`de`/`es`).
    pub fn localized_description(&self, lang: &str) -> &str {
        let localized = match lang {
            "fr" => self.description_fr.as_deref(),
            "de" => self.description_de.as_deref(),
            "es" => self.description_es.as_deref(),
            _ => None,
        };
        localized.unwrap_or(&self.description)
    }

    /// Whether the app's images cover `host_arch` (`std::env::consts::ARCH`
    /// values: `x86_64` / `aarch64` / `arm`). An empty `arch` list means
    /// "no constraint declared" -> assume supported.
    pub fn supports_host_arch(&self, host_arch: &str) -> bool {
        if self.arch.is_empty() {
            return true;
        }
        let wanted = match host_arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "arm" => "armv7",
            other => other,
        };
        self.arch.iter().any(|a| a == wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        manifest_version = 1
        id = "demo"
        name = "Demo"
        category = "misc"
        app_port = 8080
        description = "A demo app."
    "#;

    #[test]
    fn test_parse_minimal() {
        let m = Manifest::parse(MINIMAL).unwrap();
        assert_eq!(m.id, "demo");
        assert_eq!(m.version, "1.0.0");
        assert_eq!(m.host_port(), 8080);
        assert!(m.arch.is_empty());
        assert!(m.dependencies.is_empty());
    }

    #[test]
    fn test_parse_full_and_localized_description() {
        let src = r#"
            manifest_version = 1
            id = "jellyfin"
            name = "Jellyfin"
            version = "2.3.0"
            category = "media"
            icon = "movie"
            app_port = 8096
            default_port = 9000
            arch = ["amd64", "arm64"]
            gpu = true
            description = "Media server."
            description_fr = "Serveur multimédia."
            dependencies = ["postgres"]
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.version, "2.3.0");
        assert_eq!(m.host_port(), 9000);
        assert!(m.gpu);
        assert_eq!(m.localized_description("fr"), "Serveur multimédia.");
        assert_eq!(m.localized_description("de"), "Media server."); // fallback
        assert_eq!(m.localized_description("en"), "Media server.");
        assert_eq!(m.dependencies, vec!["postgres"]);
    }

    #[test]
    fn test_supports_host_arch() {
        let m = Manifest::parse(
            r#"manifest_version = 1
               id = "a"
               name = "A"
               category = "x"
               app_port = 1
               description = "d"
               arch = ["amd64", "arm64"]"#,
        )
        .unwrap();
        assert!(m.supports_host_arch("x86_64"));
        assert!(m.supports_host_arch("aarch64"));
        assert!(!m.supports_host_arch("arm")); // armv7 not listed
    }

    #[test]
    fn test_reject_bad_version_and_id() {
        assert!(Manifest::parse(&MINIMAL.replace("manifest_version = 1", "manifest_version = 2")).is_err());
        assert!(Manifest::parse(&MINIMAL.replace(r#"id = "demo""#, r#"id = "../evil""#)).is_err());
    }
}
