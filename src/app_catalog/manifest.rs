/* =============================================================================
 * File          : manifest.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The homevault-app.toml manifest: the static metadata half of an app
 *   package (the sibling docker-compose.yml template carries the
 *   {{DATA_DIR}}/{{PORT}} placeholders). Parsed with the toml crate already
 *   used for translations — no new dependency.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   app_catalog/mod.rs loads manifests from the embedded catalog and the
 *   community stores; install validates them.
 * =============================================================================
 */

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
    /// Bare ids of other apps in the same store this one needs. A
    /// dependency is satisfied by an installed app whose bare id equals the
    /// dependency **or** whose `implements` list contains it (alternatives:
    /// e.g. any `postgres`-implementing app satisfies a
    /// `postgres` dependency). `resolve_install_plan` in `mod.rs` computes
    /// the concrete choice; the install endpoint auto-installs missing
    /// same-store providers.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Canonical dependency ids this app can stand in for. An app with
    /// `implements = ["postgres"]` is offered as an alternative whenever
    /// another app declares `dependencies = ["postgres"]`.
    #[serde(default)]
    pub implements: Vec<String>,
    /// Advanced environment variables the admin may set at install time
    /// (the `environment` block). Declared values render into the
    /// compose template as `{{VAR_NAME}}` placeholders; undeclared `{{X}}`
    /// placeholders are left untouched.
    #[serde(default)]
    pub environment: Vec<EnvVarDecl>,
    /// Minimum HomeVault server version (semver `major.minor.patch`) required
    /// to install this app. `None` = no constraint. Compared against
    /// `CARGO_PKG_VERSION` in `is_compatible_with_server`.
    #[serde(default)]
    pub min_homevault_version: Option<String>,
}

/// One advanced environment variable an app package declares.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EnvVarDecl {
    /// `UPPER_SNAKE_CASE` name, also the `{{NAME}}` template placeholder.
    pub name: String,
    /// Compose services the variable applies to (informational; the value
    /// is substituted everywhere its placeholder appears).
    #[serde(default)]
    pub services: Vec<String>,
    /// Default used when the admin provides no value.
    #[serde(default)]
    pub default: Option<String>,
    /// Allowed values when present; an install value outside this list is a
    /// 400. Empty = free text.
    #[serde(default)]
    pub options: Vec<String>,
    /// Whether the admin must provide a value (or the manifest a default).
    #[serde(default)]
    pub required: bool,
    /// Short help text shown in the install dialog.
    #[serde(default)]
    pub note: Option<String>,
}

/// `UPPER_SNAKE_CASE`, 1-64 chars — keeps a variable name from breaking out
/// of a `{{NAME}}` placeholder into template syntax.
pub fn is_valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
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
        for dep in m.dependencies.iter().chain(m.implements.iter()) {
            if dep.is_empty()
                || !dep
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(format!("invalid dependency id: {dep:?}"));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for var in &m.environment {
            if !is_valid_env_name(&var.name) {
                return Err(format!("invalid environment variable name: {:?}", var.name));
            }
            if !seen.insert(var.name.clone()) {
                return Err(format!("duplicate environment variable: {:?}", var.name));
            }
            if !var.options.is_empty()
                && var
                    .default
                    .as_ref()
                    .is_some_and(|d| !var.options.contains(d))
            {
                return Err(format!(
                    "environment variable {:?} default is not in its options list",
                    var.name
                ));
            }
        }
        if let Some(min) = &m.min_homevault_version {
            parse_version(min).ok_or_else(|| {
                format!("invalid min_homevault_version {min:?} (want major.minor.patch)")
            })?;
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

    /// Whether this server (arch + version) can install the app. Returns
    /// `None` when compatible, else a short human-readable reason (surfaced
    /// in the catalog as `incompatibility_reason` and enforced by install).
    pub fn incompatibility_reason(&self, host_arch: &str, server_version: &str) -> Option<String> {
        if !self.supports_host_arch(host_arch) {
            return Some(format!("requires one of [{}]", self.arch.join(", ")));
        }
        if let Some(min) = &self.min_homevault_version {
            match (parse_version(min), parse_version(server_version)) {
                (Some(need), Some(have)) if have >= need => {}
                (Some(need), Some(have)) => {
                    return Some(format!(
                        "requires HomeVault {} or newer (this server is {})",
                        fmt_version(&need),
                        fmt_version(&have)
                    ));
                }
                _ => {
                    return Some(format!("declares an unreadable minimum version ({min})"));
                }
            }
        }
        None
    }
}

/// Parse `major.minor.patch` (missing parts default to 0, pre-release/build
/// suffixes ignored) for the `min_homevault_version` gate.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut parts = core.split('.');
    let major: u64 = parts.next()?.trim().parse().ok()?;
    let minor: u64 = match parts.next() {
        None => 0,
        Some(p) => p.trim().parse().ok()?,
    };
    let patch: u64 = match parts.next() {
        None => 0,
        Some(p) => p.trim().parse().ok()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn fmt_version(v: &(u64, u64, u64)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
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
        assert!(
            Manifest::parse(&MINIMAL.replace("manifest_version = 1", "manifest_version = 2"))
                .is_err()
        );
        assert!(Manifest::parse(&MINIMAL.replace(r#"id = "demo""#, r#"id = "../evil""#)).is_err());
    }

    #[test]
    fn test_parse_implements_and_environment() {
        let src = r#"
            manifest_version = 1
            id = "db"
            name = "Db"
            category = "db"
            app_port = 5432
            description = "d"
            implements = ["postgres"]
            min_homevault_version = "1.2.0"
            [[environment]]
            name = "POSTGRES_PASSWORD"
            services = ["db"]
            required = true
            [[environment]]
            name = "TZ"
            default = "UTC"
            options = ["UTC", "Europe/Paris"]
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.implements, vec!["postgres"]);
        assert_eq!(m.environment.len(), 2);
        assert!(m.environment[0].required);
        assert_eq!(m.min_homevault_version.as_deref(), Some("1.2.0"));
        assert!(m.incompatibility_reason("x86_64", "1.2.0").is_none());
        assert!(m
            .incompatibility_reason("x86_64", "1.1.9")
            .unwrap()
            .contains("1.2.0"));
    }

    #[test]
    fn test_reject_bad_environment_and_min_version() {
        // Lowercase env name.
        assert!(Manifest::parse(
            "manifest_version = 1\nid = \"a\"\nname = \"A\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\n[[environment]]\nname = \"lower\"\n"
        )
        .is_err());
        // Duplicate env name.
        assert!(Manifest::parse(
            "manifest_version = 1\nid = \"a\"\nname = \"A\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\n[[environment]]\nname = \"A\"\n[[environment]]\nname = \"A\"\n"
        )
        .is_err());
        // Default outside options.
        assert!(Manifest::parse(
            "manifest_version = 1\nid = \"a\"\nname = \"A\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\n[[environment]]\nname = \"A\"\ndefault = \"x\"\noptions = [\"y\"]\n"
        )
        .is_err());
        // Bad min version and bad dependency id.
        assert!(Manifest::parse(&MINIMAL.replace(
            r#"description = "A demo app.""#,
            "description = \"d\"\nmin_homevault_version = \"soon\""
        ))
        .is_err());
        assert!(Manifest::parse(&MINIMAL.replace(
            r#"description = "A demo app.""#,
            "description = \"d\"\ndependencies = [\"../evil\"]"
        ))
        .is_err());
    }

    #[test]
    fn test_parse_version_shapes() {
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("2"), Some((2, 0, 0)));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert!(parse_version("soon").is_none());
        assert!(parse_version("1.2.3.4").is_none());
        assert!(parse_version("").is_none());
    }

    #[test]
    fn test_incompatibility_arch_reason() {
        let m = Manifest::parse(
            "manifest_version = 1\nid = \"a\"\nname = \"A\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\narch = [\"amd64\"]\n",
        )
        .unwrap();
        assert!(m.incompatibility_reason("x86_64", "9.9.9").is_none());
        assert!(m
            .incompatibility_reason("arm", "9.9.9")
            .unwrap()
            .contains("amd64"));
    }
}
