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
use manifest::{Manifest, SecurityPolicy};
use std::path::{Path, PathBuf};

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
    render_with_options(app, port, &RenderOptions::default())
}

/// Like [`render`], but substitutes declared environment variables.
pub fn render_with_env(
    app: &CatalogApp,
    port: u16,
    env: &std::collections::HashMap<String, String>,
) -> Result<(String, PathBuf), String> {
    render_with_options(
        app,
        port,
        &RenderOptions {
            env: env.clone(),
            ..RenderOptions::default()
        },
    )
}

/// Where a container finds the host's hardware transcoder (Intel/AMD iGPU via
/// VAAPI/QuickSync). Mounting the whole directory gives the app the render
/// node it picks at runtime without HomeVault guessing the card model.
pub const GPU_DEVICE_DIR: &str = "/dev/dri";

/// True when the host exposes a render device to containers.
pub fn gpu_available() -> bool {
    gpu_available_in(std::path::Path::new(GPU_DEVICE_DIR))
}

/// Testable form of [`gpu_available`] (the real check is a `stat` on
/// `/dev/dri`, which a test cannot create).
pub fn gpu_available_in(dir: &std::path::Path) -> bool {
    dir.is_dir()
}

/// Everything an admin chooses at install time that changes the rendered
/// compose file.
///
/// Kept as one value so an update can rebuild the *same* stack from the
/// install marker: re-rendering with defaults would silently drop a chosen
/// media share, a GPU passthrough or a declared setting on the first update.
#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    /// Values for the manifest's declared `environment` settings.
    pub env: std::collections::HashMap<String, String>,
    /// Host directory bound to the manifest's `share_mount`. `None` uses the
    /// app's own folder inside its data directory, so a freshly installed
    /// media app still finds a library to scan.
    pub share_dir: Option<PathBuf>,
    /// Mount `/dev/dri` into the app. Only honoured when the manifest sets
    /// `gpu = true` and the host really has the device.
    pub gpu: bool,
}

/// Render the compose template with every admin choice applied.
///
/// Placeholders: `{{DATA_DIR}}`, `{{PORT}}`, `{{APP_ID}}`, the declared
/// `{{NAME}}` environment values, `{{SHARE_DIR}}` (the host side of the
/// manifest's `share_mount`) and `{{GPU_DEVICES}}` (the `devices:` block).
/// Security policy is inserted last, so it applies to the final file.
pub fn render_with_options(
    app: &CatalogApp,
    port: u16,
    options: &RenderOptions,
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
        let value = options
            .env
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

    // Media share: a configured HomeVault share, or the app's own folder.
    match app.manifest.share_mount() {
        Some(mount) => {
            let (share_dir, own_folder) = match &options.share_dir {
                Some(dir) => (dir.clone(), false),
                None => (
                    data_dir.join(
                        app.manifest
                            .default_share_folder()
                            .unwrap_or_else(|| "media".to_string()),
                    ),
                    true,
                ),
            };
            let raw = share_dir.to_string_lossy().to_string();
            if !share_dir.is_absolute() || raw.contains(':') || raw.chars().any(char::is_whitespace)
            {
                return Err(format!(
                    "invalid media share path {raw:?} for {}",
                    app.manifest.name
                ));
            }
            rendered = rendered.replace("{{SHARE_DIR}}", &raw);
            if own_folder {
                // Docker would create this as root:root; the installer's
                // `data_volume_paths` pass creates and owns it first.
                tracing::debug!(
                    "{}: no media share chosen, scanning {}",
                    app.manifest.name,
                    share_dir.display()
                );
            }
            // The container path is part of the trust boundary: it comes from
            // the package, never from the request.
            debug_assert!(manifest::validate_share_mount(mount).is_ok());
        }
        None => {
            if rendered.contains("{{SHARE_DIR}}") {
                return Err(format!(
                    "{} mounts {{SHARE_DIR}} but its manifest declares no share_mount",
                    app.manifest.name
                ));
            }
        }
    }

    // Hardware transcoding: opt-in, and only for an app that asks for it.
    // The explanation travels with the block so a disabled GPU leaves no
    // orphan comment in the rendered file.
    let gpu_block = (app.manifest.gpu && options.gpu && gpu_available())
        .then_some("devices:\n  # Hardware transcoding (VAAPI/QuickSync)\n  - /dev/dri:/dev/dri");
    rendered = replace_block(&rendered, "{{GPU_DEVICES}}", gpu_block);
    if app.manifest.gpu && options.gpu && !gpu_available() {
        tracing::warn!(
            "{} was asked for hardware transcoding but {} is missing: rendering software-only",
            app.manifest.name,
            GPU_DEVICE_DIR
        );
    }

    let rendered = apply_security_policy(&rendered, &app.manifest.security);
    Ok((rendered, data_dir))
}

/// Replace a whole-line placeholder, re-indenting the replacement's
/// continuation lines to the placeholder's own indentation.
///
/// An empty replacement removes the line entirely instead of leaving a blank
/// one, which is how optional blocks (the GPU `devices:` entry, absent on
/// most hosts) disappear from the file. A placeholder inside a line is
/// substituted in place without re-indentation.
fn replace_block(yaml: &str, placeholder: &str, value: Option<&str>) -> String {
    if !yaml.contains(placeholder) {
        return yaml.to_string();
    }
    let mut out = String::with_capacity(yaml.len());
    for line in yaml.split_inclusive('\n') {
        let (body, newline) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        let Some(index) = body.find(placeholder) else {
            out.push_str(line);
            continue;
        };
        let prefix = &body[..index];
        let suffix = &body[index + placeholder.len()..];
        match value.filter(|v| !v.is_empty()) {
            Some(value) if prefix.trim().is_empty() => {
                let mut first = true;
                for part in value.split('\n') {
                    if !first {
                        out.push_str(prefix);
                    }
                    out.push_str(part);
                    out.push('\n');
                    first = false;
                }
            }
            Some(value) => {
                out.push_str(prefix);
                out.push_str(value);
                out.push_str(suffix);
                out.push_str(newline);
            }
            // Dropped: skip the whole line, including its newline.
            None if prefix.trim().is_empty() => {}
            None => {
                out.push_str(prefix.trim_end());
                out.push_str(suffix);
                out.push_str(newline);
            }
        }
    }
    out
}

/// Host directories a rendered compose file bind-mounts from the app's data
/// directory, in template order and without duplicates.
///
/// Docker creates a missing bind-mount source as `root:root`, which locks out
/// a container started with `security.user`. The installer pre-creates these
/// paths (and hands them to the declared owner) before `docker compose up`.
pub fn data_volume_paths(rendered: &str, data_dir: &Path) -> Vec<PathBuf> {
    let prefix = data_dir.to_string_lossy();
    let mut paths: Vec<PathBuf> = Vec::new();
    for line in rendered.lines() {
        let Some(entry) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        let Some((host, _)) = entry.split_once(':') else {
            continue;
        };
        let host = host.trim();
        if host.is_empty() || !host.starts_with(prefix.as_ref()) {
            continue;
        }
        let path = PathBuf::from(host);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Insert the manifest's Docker security policy into every service block of a
/// rendered compose file.
///
/// This is deliberately line-based rather than a YAML rewrite: the catalog
/// templates are simple, and the function only *adds* keys at the service
/// level, never reorders or re-serialises the author's file. A key the
/// template already declares is left alone (the app author's explicit choice
/// wins), which also avoids duplicate YAML keys.
fn apply_security_policy(yaml: &str, policy: &SecurityPolicy) -> String {
    if policy.is_empty() {
        return yaml.to_string();
    }
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    let lines: Vec<&str> = yaml.lines().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_services = false;
    let mut services_indent = 0usize;

    while i < lines.len() {
        let line = lines[i];
        if !in_services {
            out.push_str(line);
            out.push('\n');
            if line.trim_end() == "services:" {
                in_services = true;
                services_indent = indent_of(line);
            }
            i += 1;
            continue;
        }
        // Leaving the services mapping (next top-level key).
        if !line.trim().is_empty() && indent_of(line) <= services_indent {
            in_services = false;
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }
        let service_indent = services_indent + 2;
        if !line.trim().is_empty()
            && indent_of(line) == service_indent
            && line.trim_end().ends_with(':')
        {
            out.push_str(line);
            out.push('\n');
            let mut j = i + 1;
            let mut block: Vec<&str> = Vec::new();
            while j < lines.len() {
                let l = lines[j];
                if !l.trim().is_empty() && indent_of(l) <= service_indent {
                    break;
                }
                block.push(l);
                j += 1;
            }
            let key_indent = service_indent + 2;
            let item_indent = service_indent + 4;
            let pad = " ".repeat(key_indent);
            let item = " ".repeat(item_indent);
            let has_key = |key: &str| {
                block.iter().any(|l| {
                    indent_of(l) == key_indent && l.trim_start().starts_with(&format!("{key}:"))
                })
            };
            if policy.no_new_privileges && !has_key("security_opt") {
                out.push_str(&format!(
                    "{pad}security_opt:\n{item}- no-new-privileges:true\n"
                ));
            }
            if !policy.cap_drop.is_empty() && !has_key("cap_drop") {
                out.push_str(&format!("{pad}cap_drop:\n"));
                for cap in &policy.cap_drop {
                    out.push_str(&format!("{item}- {cap}\n"));
                }
            }
            if !policy.cap_add.is_empty() && !has_key("cap_add") {
                out.push_str(&format!("{pad}cap_add:\n"));
                for cap in &policy.cap_add {
                    out.push_str(&format!("{item}- {cap}\n"));
                }
            }
            if let Some(user) = &policy.user {
                if !has_key("user") {
                    out.push_str(&format!("{pad}user: \"{user}\"\n"));
                }
            }
            if policy.read_only && !has_key("read_only") {
                out.push_str(&format!("{pad}read_only: true\n"));
            }
            for l in block {
                out.push_str(l);
                out.push('\n');
            }
            i = j;
            continue;
        }
        out.push_str(line);
        out.push('\n');
        i += 1;
    }
    out
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
    fn test_every_builtin_manifest_declares_security() {
        for dir in BUILTIN_DIR.dirs() {
            let name = dir.path().file_name().unwrap().to_str().unwrap();
            let raw = BUILTIN_DIR
                .get_file(format!("{name}/homevault-app.toml"))
                .and_then(|f| f.contents_utf8())
                .unwrap_or_else(|| panic!("{name} has no homevault-app.toml"));
            let manifest = Manifest::parse(raw)
                .unwrap_or_else(|e| panic!("{name} has an invalid manifest: {e}"));
            assert!(
                raw.lines().any(|l| l.trim() == "[security]"),
                "{name} must declare an explicit [security] block"
            );
            assert!(
                manifest.security.no_new_privileges,
                "{name} must keep no_new_privileges on"
            );
        }
    }

    #[test]
    fn test_data_volume_paths_only_app_data_mounts() {
        let dir = PathBuf::from("/srv/apps/demo");
        let yaml = "services:\n  demo:\n    volumes:\n      - /srv/apps/demo/files:/data\n      - /srv/apps/demo/db:/db:ro\n      - /etc/localtime:/etc/localtime:ro\n    ports:\n      - \"8080:80\"\n";
        let paths = data_volume_paths(yaml, &dir);
        assert_eq!(paths, vec![dir.join("files"), dir.join("db")]);
    }

    #[test]
    fn test_security_policy_is_rendered_into_every_service() {
        let mut app = scan_builtin()
            .into_iter()
            .find(|a| a.manifest.id == "vikunja")
            .unwrap();
        app.manifest.security = SecurityPolicy {
            no_new_privileges: true,
            cap_add: vec!["NET_BIND_SERVICE".to_string()],
            cap_drop: vec!["ALL".to_string()],
            user: Some("1000:1000".to_string()),
            read_only: true,
        };
        let (yaml, _) = render(&app, 12345).unwrap();
        assert_eq!(yaml.matches("no-new-privileges:true").count(), 1);
        assert!(yaml.contains("cap_drop:\n      - ALL"));
        assert!(yaml.contains("cap_add:\n      - NET_BIND_SERVICE"));
        assert!(yaml.contains("user: \"1000:1000\""));
        assert!(yaml.contains("read_only: true"));
    }

    #[test]
    fn test_security_policy_rejects_all_in_cap_add() {
        let toml = "manifest_version = 1\nid = \"demo\"\nname = \"Demo\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\n[security]\ncap_add = [\"ALL\"]\n";
        assert!(Manifest::parse(toml).is_err());
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

    // -- media integration (share binding + hardware transcoding) -----------

    fn builtin(id: &str) -> CatalogApp {
        scan_builtin()
            .into_iter()
            .find(|a| a.manifest.id == id)
            .unwrap_or_else(|| panic!("{id} is not in the built-in catalog"))
    }

    #[test]
    fn test_builtin_media_apps_declare_share_mount_and_gpu_support() {
        assert_eq!(builtin("jellyfin").manifest.share_mount(), Some("/media"));
        assert_eq!(builtin("immich").manifest.share_mount(), Some("/mnt/media"));
        assert_eq!(builtin("navidrome").manifest.share_mount(), Some("/music"));
        // Video apps can transcode on the GPU; a music server cannot.
        assert!(builtin("jellyfin").manifest.gpu);
        assert!(builtin("immich").manifest.gpu);
        assert!(!builtin("navidrome").manifest.gpu);
    }

    #[test]
    fn test_render_binds_the_chosen_share_read_only() {
        let app = builtin("jellyfin");
        let (yaml, data_dir) = render_with_options(
            &app,
            8096,
            &RenderOptions {
                share_dir: Some(PathBuf::from("/srv/films")),
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert!(yaml.contains("- /srv/films:/media:ro"), "{yaml}");
        assert!(!yaml.contains("{{"));
        // The user's share is not app data: it is never pre-created or
        // chowned by the installer.
        assert!(!data_volume_paths(&yaml, &data_dir)
            .iter()
            .any(|p| p.starts_with("/srv/films")));
    }

    #[test]
    fn test_render_defaults_to_a_folder_inside_the_app_data_dir() {
        let app = builtin("immich");
        let (yaml, data_dir) = render(&app, 2283).unwrap();
        let expected = data_dir.join("mnt-media");
        assert!(
            yaml.contains(&format!("{}:/mnt/media:ro", expected.display())),
            "{yaml}"
        );
        // And that folder *is* app data, so the installer creates it.
        assert!(data_volume_paths(&yaml, &data_dir).contains(&expected));
    }

    #[test]
    fn test_gpu_block_follows_the_device_and_the_opt_in() {
        // The check really inspects the filesystem (a temp dir exists, a
        // missing path does not) rather than returning a constant.
        assert!(gpu_available_in(&std::env::temp_dir()));
        assert!(!gpu_available_in(std::path::Path::new(
            "/nonexistent/hv-no-such-device"
        )));

        let app = builtin("jellyfin");
        // Not asked for: the placeholder disappears entirely (no blank line,
        // no dangling comment).
        let (off, _) = render(&app, 8096).unwrap();
        assert!(!off.contains("{{GPU_DEVICES}}"));
        assert!(!off.contains("/dev/dri"));
        assert!(!off.contains("Hardware transcoding"));

        let (asked, _) = render_with_options(
            &app,
            8096,
            &RenderOptions {
                gpu: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert!(!asked.contains("{{GPU_DEVICES}}"));
        if gpu_available() {
            assert!(
                asked.contains("devices:\n      - /dev/dri:/dev/dri"),
                "{asked}"
            );
        } else {
            assert!(!asked.contains("/dev/dri"), "{asked}");
        }

        // An app whose manifest does not declare `gpu` never receives the
        // device, even when the request asks for it.
        let (music, _) = render_with_options(
            &builtin("navidrome"),
            4533,
            &RenderOptions {
                gpu: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert!(!music.contains("/dev/dri"));
    }

    #[test]
    fn test_invalid_share_path_is_refused_by_the_renderer() {
        let app = builtin("jellyfin");
        for bad in ["relative/media", "/srv/two words", "/srv/a:b"] {
            let err = render_with_options(
                &app,
                8096,
                &RenderOptions {
                    share_dir: Some(PathBuf::from(bad)),
                    ..RenderOptions::default()
                },
            )
            .unwrap_err();
            assert!(err.contains("invalid media share path"), "{err}");
        }
    }

    #[test]
    fn test_share_placeholder_without_a_manifest_declaration_is_an_error() {
        let dir = std::env::temp_dir().join(format!(
            "hv-catalog-share-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("docker-compose.yml"),
            "services:\n  demo:\n    image: nginx\n    volumes:\n      - {{SHARE_DIR}}:/media:ro\n",
        )
        .unwrap();
        let manifest = Manifest::parse(
            "manifest_version = 1\nid = \"demo\"\nname = \"Demo\"\ncategory = \"x\"\napp_port = 1\ndescription = \"d\"\n",
        )
        .unwrap();
        let app = CatalogApp {
            store_id: "test".to_string(),
            store_name: "test".to_string(),
            manifest,
            source: AppSource::Store(dir.clone()),
        };
        let err = render(&app, 1).unwrap_err();
        assert!(err.contains("share_mount"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_replace_block_reindents_and_drops_whole_lines() {
        let yaml = "services:\n  a:\n    volumes:\n      - /x:/y\n    {{P}}\n    restart: always\n";
        let block = "devices:\n  - /dev/dri:/dev/dri";
        let rendered = replace_block(yaml, "{{P}}", Some(block));
        assert_eq!(
            rendered,
            "services:\n  a:\n    volumes:\n      - /x:/y\n    devices:\n      - /dev/dri:/dev/dri\n    restart: always\n"
        );
        // None removes the line itself, leaving no blank line behind.
        assert_eq!(
            replace_block(yaml, "{{P}}", None),
            "services:\n  a:\n    volumes:\n      - /x:/y\n    restart: always\n"
        );
    }
}
