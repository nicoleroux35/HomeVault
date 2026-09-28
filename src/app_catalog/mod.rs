//! Manifest-driven Docker app catalog. Replaces the hard-coded
//! `src/docker_catalog.rs::CATALOG` array: apps are now packages (a
//! `homevault-app.toml` manifest + a `docker-compose.yml` template) read
//! from the built-in `catalog/` directory embedded in the binary, plus any
//! community app stores (git repos) the admin has added.
//!
//! Install / uninstall still go through [`crate::compose::ComposeManager`]
//! exactly as before - only the *source* of the templates changed.

pub mod manifest;
pub mod store;
pub mod updates;

use include_dir::{include_dir, Dir};
use manifest::Manifest;
use std::path::PathBuf;

/// The built-in catalog, baked into the binary at compile time (same
/// mechanism as `sqlx/migrations` and the frontend `static/` dir).
static BUILTIN_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/catalog");

/// Store id of the built-in catalog. Apps from this store keep bare
/// (un-namespaced) ids and install keys, so apps installed before the
/// manifest migration keep working.
pub const BUILTIN_STORE_ID: &str = "homevault";
pub const BUILTIN_STORE_NAME: &str = "HomeVault";

/// Where an app package's files live.
#[derive(Debug, Clone)]
pub enum AppSource {
    /// Read from the embedded `BUILTIN_DIR` under `<bare-id>/`.
    Builtin,
    /// Read from this directory on disk (a synced community store).
    Store(PathBuf),
}

/// One catalog entry: its parsed manifest plus where its files come from.
#[derive(Debug, Clone)]
pub struct CatalogApp {
    pub store_id: String,
    pub store_name: String,
    pub manifest: Manifest,
    pub source: AppSource,
}

impl CatalogApp {
    pub fn bare_id(&self) -> &str {
        &self.manifest.id
    }

    /// `<store>:<id>` - the id used in API paths.
    pub fn namespaced_id(&self) -> String {
        format!("{}:{}", self.store_id, self.manifest.id)
    }

    /// The compose project name and data-directory name. The built-in store
    /// keeps the bare id (backward compatibility); other stores are
    /// prefixed so two stores can each ship an app called "gitea" without
    /// colliding on disk.
    pub fn install_key(&self) -> String {
        if self.store_id == BUILTIN_STORE_ID {
            self.manifest.id.clone()
        } else {
            format!("{}-{}", self.store_id, self.manifest.id)
        }
    }

    fn read_rel(&self, rel: &str) -> Option<Vec<u8>> {
        match &self.source {
            AppSource::Builtin => BUILTIN_DIR
                .get_file(format!("{}/{}", self.manifest.id, rel))
                .map(|f| f.contents().to_vec()),
            AppSource::Store(dir) => std::fs::read(dir.join(rel)).ok(),
        }
    }

    /// The raw `docker-compose.yml` template (with `{{DATA_DIR}}` /
    /// `{{PORT}}` / `{{APP_ID}}` placeholders).
    pub fn compose_template(&self) -> Option<String> {
        self.read_rel("docker-compose.yml")
            .and_then(|b| String::from_utf8(b).ok())
    }

    /// The package's own `icon.svg`, if it ships one.
    pub fn icon_svg(&self) -> Option<Vec<u8>> {
        self.read_rel("icon.svg")
    }

    /// Number of `gallery/<n>.<ext>` screenshots the package ships
    /// (contiguous from 1).
    pub fn gallery_count(&self) -> usize {
        (1..=12)
            .take_while(|n| self.gallery_file(*n).is_some())
            .count()
    }

    /// Screenshot `n` (1-based) as `(bytes, content_type)`.
    pub fn gallery_file(&self, n: usize) -> Option<(Vec<u8>, &'static str)> {
        for (ext, ct) in [
            ("png", "image/png"),
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("webp", "image/webp"),
        ] {
            if let Some(bytes) = self.read_rel(&format!("gallery/{n}.{ext}")) {
                return Some((bytes, ct));
            }
        }
        None
    }
}

/// A store the caller has determined is enabled: `(id, display name)`.
pub type EnabledStore = (String, String);

/// Every app from the built-in catalog plus every enabled community store.
/// Malformed manifests are skipped (logged), not fatal.
pub fn list_apps(enabled_stores: &[EnabledStore]) -> Vec<CatalogApp> {
    let mut apps = scan_builtin();
    for (store_id, store_name) in enabled_stores {
        apps.extend(scan_store(store_id, store_name));
    }
    apps.sort_by(|a, b| {
        a.manifest
            .name
            .to_lowercase()
            .cmp(&b.manifest.name.to_lowercase())
    });
    apps
}

/// Resolve a `<store>:<id>` (or a bare `<id>`, assumed built-in) to its app.
pub fn find(enabled_stores: &[EnabledStore], id: &str) -> Option<CatalogApp> {
    let ns = if id.contains(':') {
        id.to_string()
    } else {
        format!("{BUILTIN_STORE_ID}:{id}")
    };
    list_apps(enabled_stores)
        .into_iter()
        .find(|a| a.namespaced_id() == ns)
}

fn scan_builtin() -> Vec<CatalogApp> {
    BUILTIN_DIR
        .dirs()
        .filter_map(|d| {
            let name = d.path().file_name()?.to_str()?;
            let raw = BUILTIN_DIR
                .get_file(format!("{name}/homevault-app.toml"))?
                .contents_utf8()?;
            match Manifest::parse(raw) {
                Ok(m) => Some(CatalogApp {
                    store_id: BUILTIN_STORE_ID.to_string(),
                    store_name: BUILTIN_STORE_NAME.to_string(),
                    manifest: m,
                    source: AppSource::Builtin,
                }),
                Err(e) => {
                    tracing::warn!("built-in catalog app {name} has a bad manifest: {e}");
                    None
                }
            }
        })
        .collect()
}

fn scan_store(store_id: &str, store_name: &str) -> Vec<CatalogApp> {
    let dir = store::store_dir(store_id);
    if !dir.is_dir() {
        return Vec::new();
    }
    store::app_dirs(&dir)
        .into_iter()
        .filter_map(|app_dir| {
            let raw = std::fs::read_to_string(app_dir.join("homevault-app.toml")).ok()?;
            match Manifest::parse(&raw) {
                Ok(m) => Some(CatalogApp {
                    store_id: store_id.to_string(),
                    store_name: store_name.to_string(),
                    manifest: m,
                    source: AppSource::Store(app_dir),
                }),
                Err(e) => {
                    tracing::warn!("store {store_id}: app at {app_dir:?} has a bad manifest: {e}");
                    None
                }
            }
        })
        .collect()
}

/// Render an app's compose template for the chosen host port. Returns the
/// rendered YAML and the app's dedicated data directory (created by the
/// caller). Mirrors the old `docker_catalog::render`.
pub fn render(app: &CatalogApp, port: u16) -> Result<(String, PathBuf), String> {
    let tmpl = app
        .compose_template()
        .ok_or_else(|| "app package has no docker-compose.yml".to_string())?;
    let data_dir = crate::config::paths::apps_dir().join(app.install_key());
    let rendered = tmpl
        .replace("{{DATA_DIR}}", &data_dir.to_string_lossy())
        .replace("{{PORT}}", &port.to_string())
        .replace("{{APP_ID}}", app.bare_id());
    Ok((rendered, data_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_catalog_loads() {
        let apps = scan_builtin();
        // The 15 migrated apps must all parse.
        assert_eq!(apps.len(), 15, "expected 15 built-in apps, got {}", apps.len());
        assert!(apps.iter().all(|a| a.store_id == BUILTIN_STORE_ID));
        assert!(apps.iter().any(|a| a.manifest.id == "jellyfin"));
    }

    #[test]
    fn test_builtin_apps_render() {
        for app in scan_builtin() {
            let (yaml, dir) = render(&app, 12345).unwrap();
            assert!(yaml.contains("services:"), "{} has no services", app.bare_id());
            assert!(!yaml.contains("{{"), "{} left a placeholder", app.bare_id());
            assert!(dir.ends_with(app.bare_id()));
            // home-assistant uses host networking and publishes no port.
            if app.bare_id() != "home-assistant" {
                assert!(yaml.contains("12345:"), "{} did not bind the port", app.bare_id());
            }
        }
    }

    #[test]
    fn test_namespacing_and_install_key() {
        let jf = &scan_builtin()
            .into_iter()
            .find(|a| a.manifest.id == "jellyfin")
            .unwrap();
        assert_eq!(jf.namespaced_id(), "homevault:jellyfin");
        assert_eq!(jf.install_key(), "jellyfin"); // built-in => bare
    }

    #[test]
    fn test_find_accepts_bare_and_namespaced() {
        assert!(find(&[], "jellyfin").is_some());
        assert!(find(&[], "homevault:jellyfin").is_some());
        assert!(find(&[], "nope:jellyfin").is_none());
    }
}
