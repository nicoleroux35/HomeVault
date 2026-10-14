/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Manifest-driven Docker app catalog: apps are packages (manifest +
 *   compose template) read from the embedded catalog/ directory plus any
 *   community stores, replacing the old hard-coded catalog array. Merges
 *   sources, exposes the install-ready app list and icon/gallery assets.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   handlers/docker_catalog.rs (Apps Center); install/uninstall go
 *   through ComposeManager (see docs/guides/apps-center.md).
 * =============================================================================
 */

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
    render_with_env(app, port, &std::collections::HashMap::new())
}

/// Like [`render`], but substitutes declared environment variables: every
/// `{{NAME}}` placeholder for a manifest-declared `NAME` is replaced by the
/// caller-supplied value, else the manifest default, else left in place when
/// the variable is optional. A missing required value is an error.
pub fn render_with_env(
    app: &CatalogApp,
    port: u16,
    env: &std::collections::HashMap<String, String>,
) -> Result<(String, PathBuf), String> {
    let tmpl = app
        .compose_template()
        .ok_or_else(|| "app package has no docker-compose.yml".to_string())?;
    let data_dir = crate::config::paths::apps_dir().join(app.install_key());
    let mut rendered = tmpl
        .replace("{{DATA_DIR}}", &data_dir.to_string_lossy())
        .replace("{{PORT}}", &port.to_string())
        .replace("{{APP_ID}}", app.bare_id());
    for decl in &app.manifest.environment {
        let value = env
            .get(&decl.name)
            .cloned()
            .or_else(|| decl.default.clone());
        match value {
            Some(v) => {
                rendered = rendered.replace(&format!("{{{{{}}}}}", decl.name), &v);
            }
            None if decl.required => {
                return Err(format!(
                    "missing required setting {} for {}",
                    decl.name, app.manifest.name
                ));
            }
            None => {}
        }
    }
    Ok((rendered, data_dir))
}

/// Validate caller-supplied environment values against the manifest's
/// declarations: unknown names, values outside `options`, and missing
/// required variables (with no default) are all errors.
pub fn validate_env_values(
    app: &CatalogApp,
    env: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    for key in env.keys() {
        let Some(decl) = app.manifest.environment.iter().find(|d| &d.name == key) else {
            return Err(format!(
                "{} declares no setting named {key:?}",
                app.manifest.name
            ));
        };
        if !decl.options.is_empty() && env.get(key).is_some_and(|v| !decl.options.contains(v)) {
            return Err(format!(
                "{key:?} must be one of [{}]",
                decl.options.join(", ")
            ));
        }
    }
    for decl in &app.manifest.environment {
        if decl.required && !env.contains_key(&decl.name) && decl.default.is_none() {
            return Err(format!("{} requires a value", decl.name));
        }
    }
    Ok(())
}

/// Every catalog app (same store first, then other stores) that can satisfy
/// `dependency`: the canonical app itself plus any app whose `implements`
/// list contains it. Used to offer alternative providers at install.
pub fn providers_of<'a>(apps: &'a [CatalogApp], dependency: &str) -> Vec<&'a CatalogApp> {
    let (mut canonical, mut alternatives): (Vec<_>, Vec<_>) = apps
        .iter()
        .filter(|a| {
            a.bare_id() == dependency || a.manifest.implements.contains(&dependency.to_string())
        })
        .partition(|a| a.bare_id() == dependency);
    // Prefer same-store providers deterministically by store then name.
    canonical.sort_by(|a, b| {
        a.store_id
            .cmp(&b.store_id)
            .then(a.manifest.name.cmp(&b.manifest.name))
    });
    alternatives.sort_by(|a, b| {
        a.store_id
            .cmp(&b.store_id)
            .then(a.manifest.name.cmp(&b.manifest.name))
    });
    canonical.into_iter().chain(alternatives).collect()
}

/// One dependency line of an install plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyNeed {
    /// The canonical id the app declares.
    pub dependency: String,
    /// Bare ids of installed apps that already satisfy it (empty = missing).
    pub satisfied_by: Vec<String>,
    /// Namespaced ids of catalog apps that could satisfy it, canonical first.
    pub providers: Vec<String>,
    /// The provider the plan selects (`alternatives[dep]`, else the first
    /// available provider, else `None` when nothing provides it).
    pub selected: Option<String>,
}

/// Resolve `app`'s dependencies against the catalog: which are already
/// satisfied by `installed_bare_ids`, which provider each missing one maps
/// to (honouring the caller's `alternatives` overrides, validated to really
/// provide the dependency), and which have no provider at all.
pub fn resolve_install_plan(
    apps: &[CatalogApp],
    app: &CatalogApp,
    installed_bare_ids: &std::collections::HashSet<String>,
    alternatives: &std::collections::HashMap<String, String>,
) -> Vec<DependencyNeed> {
    app.manifest
        .dependencies
        .iter()
        .map(|dep| {
            let satisfied_by: Vec<String> = apps
                .iter()
                .filter(|a| {
                    (a.bare_id() == *dep || a.manifest.implements.contains(dep))
                        && installed_bare_ids.contains(a.bare_id())
                })
                .map(|a| a.bare_id().to_string())
                .collect();
            let providers: Vec<String> = providers_of(apps, dep)
                .iter()
                .map(|a| a.namespaced_id())
                .collect();
            let selected = if !satisfied_by.is_empty() {
                None
            } else if let Some(want) = alternatives.get(dep) {
                providers.iter().find(|p| *p == want).cloned()
            } else {
                // Prefer a same-store provider so cross-store installs stay
                // explicit, else the first (canonical-first) provider.
                providers
                    .iter()
                    .find(|p| p.starts_with(&format!("{}:", app.store_id)))
                    .or_else(|| providers.first())
                    .cloned()
            };
            DependencyNeed {
                dependency: dep.clone(),
                satisfied_by,
                providers,
                selected,
            }
        })
        .collect()
}

/// Server-side compatibility for one catalog app: arch coverage plus the
/// manifest's `min_homevault_version` gate against this binary.
pub fn compatibility(app: &CatalogApp) -> Option<String> {
    app.manifest
        .incompatibility_reason(std::env::consts::ARCH, env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_catalog_loads() {
        let apps = scan_builtin();
        // The 15 migrated apps must all parse.
        assert_eq!(
            apps.len(),
            15,
            "expected 15 built-in apps, got {}",
            apps.len()
        );
        assert!(apps.iter().all(|a| a.store_id == BUILTIN_STORE_ID));
        assert!(apps.iter().any(|a| a.manifest.id == "jellyfin"));
    }

    #[test]
    fn test_builtin_apps_render() {
        for app in scan_builtin() {
            let (yaml, dir) = render(&app, 12345).unwrap();
            assert!(
                yaml.contains("services:"),
                "{} has no services",
                app.bare_id()
            );
            assert!(!yaml.contains("{{"), "{} left a placeholder", app.bare_id());
            assert!(dir.ends_with(app.bare_id()));
            // home-assistant uses host networking and publishes no port.
            if app.bare_id() != "home-assistant" {
                assert!(
                    yaml.contains("12345:"),
                    "{} did not bind the port",
                    app.bare_id()
                );
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

    fn test_app(store: &str, id: &str, deps: &[&str], implements: &[&str]) -> CatalogApp {
        let toml = format!(
            "manifest_version = 1\nid = \"{id}\"\nname = \"{id}\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\ndependencies = {deps:?}\nimplements = {impls:?}\n",
            deps = deps.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            impls = implements.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        CatalogApp {
            store_id: store.to_string(),
            store_name: store.to_string(),
            manifest: Manifest::parse(&toml).unwrap(),
            source: AppSource::Store(format!("/tmp/{store}-{id}").into()),
        }
    }

    #[test]
    fn test_providers_of_prefers_canonical() {
        let apps = vec![
            test_app("homevault", "alt-db", &[], &["postgres"]),
            test_app("homevault", "postgres", &[], &[]),
        ];
        let providers = providers_of(&apps, "postgres");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].bare_id(), "postgres");
        assert_eq!(providers[1].bare_id(), "alt-db");
        assert!(providers_of(&apps, "missing").is_empty());
    }

    #[test]
    fn test_resolve_install_plan_satisfied_missing_and_alternative() {
        let apps = vec![
            test_app("homevault", "wiki", &["postgres"], &[]),
            test_app("homevault", "postgres", &[], &[]),
            test_app("homevault", "alt-db", &[], &["postgres"]),
        ];
        let wiki = apps[0].clone();
        let installed: std::collections::HashSet<String> =
            ["alt-db".to_string()].into_iter().collect();
        // Satisfied via the alternative implementation.
        let plan =
            resolve_install_plan(&apps, &wiki, &installed, &std::collections::HashMap::new());
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].satisfied_by, vec!["alt-db"]);
        assert!(plan[0].selected.is_none());

        // Nothing installed: selects the canonical provider by default.
        let plan = resolve_install_plan(
            &apps,
            &wiki,
            &std::collections::HashSet::new(),
            &std::collections::HashMap::new(),
        );
        assert_eq!(plan[0].selected.as_deref(), Some("homevault:postgres"));

        // Explicit alternative override is honoured when it provides the dep.
        let mut alt = std::collections::HashMap::new();
        alt.insert("postgres".to_string(), "homevault:alt-db".to_string());
        let plan = resolve_install_plan(&apps, &wiki, &std::collections::HashSet::new(), &alt);
        assert_eq!(plan[0].selected.as_deref(), Some("homevault:alt-db"));

        // An override that does not provide the dep selects nothing.
        alt.insert("postgres".to_string(), "homevault:wiki".to_string());
        let plan = resolve_install_plan(&apps, &wiki, &std::collections::HashSet::new(), &alt);
        assert!(plan[0].selected.is_none());
    }

    #[test]
    fn test_render_with_env_and_validation() {
        let mut app = test_app("homevault", "demo", &[], &[]);
        app.manifest.environment = vec![
            manifest::EnvVarDecl {
                name: "SECRET".to_string(),
                services: vec![],
                default: None,
                options: vec![],
                required: true,
                note: None,
            },
            manifest::EnvVarDecl {
                name: "TZ".to_string(),
                services: vec![],
                default: Some("UTC".to_string()),
                options: vec!["UTC".to_string(), "Europe/Paris".to_string()],
                required: false,
                note: None,
            },
        ];
        let empty = std::collections::HashMap::new();
        assert!(validate_env_values(&app, &empty).is_err()); // SECRET missing
        let mut bad = std::collections::HashMap::new();
        bad.insert("NOPE".to_string(), "x".to_string());
        assert!(validate_env_values(&app, &bad).is_err()); // unknown
        let mut bad_opt = std::collections::HashMap::new();
        bad_opt.insert("SECRET".to_string(), "s".to_string());
        bad_opt.insert("TZ".to_string(), "Mars".to_string());
        assert!(validate_env_values(&app, &bad_opt).is_err()); // outside options
        let mut good = std::collections::HashMap::new();
        good.insert("SECRET".to_string(), "s3cr3t".to_string());
        assert!(validate_env_values(&app, &good).is_ok());
    }
}
