/* =============================================================================
 * File          : docker_catalog.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.1
 *
 * Description:
 *   One-click app catalog on top of Docker Compose: browse the built-in
 *   manifest-driven catalog plus admin-added community stores, install and
 *   uninstall apps, check and apply updates (individually or all at once),
 *   inspect install/update progress and logs, and manage the proxy config
 *   that exposes an app under its own port. Browsing is open to any
 *   authenticated user; installing, updating and store management are
 *   admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/catalog and /api/stores in src/web/mod.rs; installs and
 *   updates run through ComposeManager, manifests come from crate::app_catalog.
 * =============================================================================
 */

//! HTTP handlers for the one-click Docker app catalog.
//!
//! The catalog is manifest-driven (see `crate::app_catalog`): apps are
//! packages read from the built-in `catalog/` directory embedded in the
//! binary, plus any community app stores (git repos) the admin has added.
//! Install / uninstall still go through `ComposeManager` unchanged.

use crate::app_catalog::{self, store as app_store};
use crate::compose::ComposeManager;
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::storage::AppStoreRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Deadline on `docker compose logs` (a log read, not a build).
///
/// The read streams from the daemon; a daemon that stopped answering must not
/// hold the blocking thread that serves the app's log panel (ROADMAP X.9).
const COMPOSE_LOGS_TIMEOUT: Duration = Duration::from_secs(30);

/// The catalog view sent to the frontend: manifest metadata (localized)
/// plus live install/run state.
#[derive(Debug, Serialize)]
struct CatalogAppView {
    /// `<store>:<id>` - the id every other catalog endpoint expects.
    id: String,
    /// Bare id, used by the frontend only for the Lucide-icon fallback
    /// lookup and legacy `catalog_desc_*` keys.
    app_id: String,
    name: String,
    description: String,
    tagline: Option<String>,
    category: String,
    icon: Option<String>,
    /// True when the package ships its own `icon.svg` (served from
    /// `/api/docker/catalog/{id}/icon`); the frontend prefers it over the
    /// Lucide fallback.
    has_icon: bool,
    gallery_count: usize,
    default_port: u16,
    version: String,
    developer: Option<String>,
    website: Option<String>,
    repo: Option<String>,
    release_notes: Option<String>,
    dependencies: Vec<String>,
    /// Canonical ids this app can stand in for (`implements`).
    implements: Vec<String>,
    /// Advanced settings the install dialog offers.
    environment: Vec<crate::app_catalog::manifest::EnvVarDecl>,
    /// Dependencies (same store) that are not currently installed.
    missing_dependencies: Vec<String>,
    /// `None` when this server can install the app, else the reason why.
    incompatibility_reason: Option<String>,
    store_id: String,
    store_name: String,
    installed: bool,
    running: bool,
    installed_port: Option<u16>,
    /// Per-app reverse-proxy port (`None` when the app isn't installed or
    /// its proxy isn't up). The "Open" button points here.
    proxy_port: Option<u16>,
    /// Whether the proxy currently requires a HomeVault session.
    proxy_auth_required: bool,
    /// True when the app's images don't cover the host's CPU architecture.
    unsupported_on_host: bool,
    /// Container path the app mounts an admin-chosen share on; `None` when
    /// the app does not consume media files.
    share_mount: Option<String>,
    /// True when the app can use `/dev/dri` (hardware transcoding).
    gpu: bool,
    /// True when this host actually exposes `/dev/dri`.
    gpu_available: bool,
    /// The share this install is bound to (`None` = its own folder).
    installed_share: Option<String>,
}

/// Reads back the host port an app was installed on, from the `.port`
/// marker file. Filesystem-derived, like all install state in this module.
fn read_installed_port(install_key: &str) -> Option<u16> {
    std::fs::read_to_string(
        crate::config::paths::apps_dir()
            .join(install_key)
            .join(".port"),
    )
    .ok()
    .and_then(|s| s.trim().parse().ok())
}

/// Async wrapper around [`read_installed_port`] (blocking filesystem read).
async fn read_installed_port_async(install_key: &str) -> Option<u16> {
    let key = install_key.to_string();
    tokio::task::spawn_blocking(move || read_installed_port(&key))
        .await
        .ok()
        .flatten()
}

/// The `.install.toml` marker written next to an installed app's data,
/// recording which catalog entry (and template version) it came from, plus
/// its per-app reverse-proxy state.
///
/// It also records **every admin choice that changes the rendered compose
/// file** (declared settings, media share, GPU access). An update re-renders
/// from the marker, so it can no longer silently drop a chosen share or a
/// hardware-transcoding opt-in and restart the app with defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallMarker {
    store_id: String,
    app_id: String,
    namespaced_id: String,
    package_version: String,
    host_port: u16,
    #[serde(default)]
    proxy_port: Option<u16>,
    #[serde(default = "default_true")]
    proxy_auth_required: bool,
    /// Values given for the manifest's declared `environment` settings.
    #[serde(default)]
    environment: std::collections::HashMap<String, String>,
    /// The HomeVault share the app's library is mounted from, if any.
    #[serde(default)]
    share_name: Option<String>,
    /// Protocol of that share (`smb`, `nfs`, …), as `ShareType::as_str`.
    #[serde(default)]
    share_type: Option<String>,
    /// Host path recorded at install time. Used only when the share itself
    /// has since been renamed or removed, so an update still renders.
    #[serde(default)]
    share_path: Option<String>,
    /// `/dev/dri` is passed to the container.
    #[serde(default)]
    gpu: bool,
}

impl InstallMarker {
    /// The render options this app was installed with.
    fn render_options(&self) -> crate::app_catalog::RenderOptions {
        crate::app_catalog::RenderOptions {
            env: self.environment.clone(),
            share_dir: self.media_share_dir(),
            gpu: self.gpu,
        }
    }

    /// The share's host directory, re-resolved from the share list so a share
    /// whose path changed keeps working. Blocking (reads the share configs):
    /// call it from the blocking pool.
    fn media_share_dir(&self) -> Option<std::path::PathBuf> {
        let (Some(name), Some(kind)) = (self.share_name.as_deref(), self.share_type.as_deref())
        else {
            return self.share_path.clone().map(std::path::PathBuf::from);
        };
        let resolved = crate::shares::ShareType::parse(kind).and_then(|parsed| {
            match crate::shares::SharesService::get_share(name, parsed) {
                Ok(share) => Some(std::path::PathBuf::from(share.path)),
                Err(e) => {
                    tracing::warn!(
                        "media share '{name}' ({kind}) is no longer available ({e}); \
                         keeping the recorded path for the next update"
                    );
                    None
                }
            }
        });
        resolved.or_else(|| self.share_path.clone().map(std::path::PathBuf::from))
    }
}

fn default_true() -> bool {
    true
}

/// Directory holding an installed app's own data and its `.install.toml`.
fn install_dir(install_key: &str) -> std::path::PathBuf {
    crate::config::paths::apps_dir().join(install_key)
}

/// Give a data directory to the numeric owner declared by `[security] user`.
///
/// Docker mounts a bind-mount source as-is, so a container running as
/// `1000:1000` cannot write a directory HomeVault created as `root:root`.
/// The chown is best effort: HomeVault may run unprivileged on a development
/// machine, and failing to change ownership must not abort an install.
fn apply_data_ownership(
    dir: &std::path::Path,
    policy: &crate::app_catalog::manifest::SecurityPolicy,
) {
    let Some((uid, gid)) = policy.data_owner() else {
        return;
    };
    if std::os::unix::fs::chown(dir, Some(uid), gid).is_ok() {
        return;
    }

    // The optional non-root deployment owns the directory but cannot change
    // its uid; the sudoers grant confines this to `apps/<app>/*`.
    let owner = match gid {
        Some(gid) => format!("{uid}:{gid}"),
        None => uid.to_string(),
    };
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg("chown").arg(&owner).arg(dir);
    match crate::utils::process::run_timed(&mut cmd, std::time::Duration::from_secs(15)) {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::warn!(
            "could not give {} to {}: {}",
            dir.display(),
            owner,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => tracing::warn!("could not give {} to {}: {}", dir.display(), owner, e),
    }
}

/// Writes `.install.toml` into `dir` and **reports the failure**.
///
/// The result used to be discarded, so an install on a read-only/full apps
/// directory looked successful while the marker was never persisted: the app
/// then disappeared from the list after a restart and its reverse proxy was
/// no longer restored. Blocking filesystem work — callers on the async path go
/// through [`write_install_marker`].
fn write_install_marker_sync(dir: &std::path::Path, marker: &InstallMarker) -> Result<(), String> {
    let path = dir.join(".install.toml");
    let body = toml::to_string_pretty(marker)
        .map_err(|e| format!("cannot serialise install marker: {e}"))?;
    std::fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Async wrapper: the marker write is blocking filesystem work, so it never
/// runs on a runtime worker.
async fn write_install_marker(install_key: &str, marker: InstallMarker) -> Result<(), String> {
    let dir = install_dir(install_key);
    tokio::task::spawn_blocking(move || write_install_marker_sync(&dir, &marker))
        .await
        .map_err(|e| format!("install marker task failed: {e}"))?
}

fn read_install_marker_sync(dir: &std::path::Path) -> Option<InstallMarker> {
    let raw = std::fs::read_to_string(dir.join(".install.toml")).ok()?;
    toml::from_str(&raw).ok()
}

/// Async wrapper around [`read_install_marker_sync`].
async fn read_install_marker(install_key: &str) -> Option<InstallMarker> {
    let dir = install_dir(install_key);
    tokio::task::spawn_blocking(move || read_install_marker_sync(&dir))
        .await
        .ok()
        .flatten()
}

/// Every installed app's proxy config, for `app_proxy::restore_all` at
/// startup. Scans `apps_dir()` for `.install.toml` markers.
pub async fn proxy_restore_entries() -> Vec<crate::app_proxy::RestoreEntry> {
    tokio::task::spawn_blocking(|| {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(crate::config::paths::apps_dir()) else {
            return out;
        };
        for entry in entries.flatten() {
            let key = entry.file_name().to_string_lossy().to_string();
            let Some(m) = read_install_marker_sync(&install_dir(&key)) else {
                continue;
            };
            // Only restore for apps whose compose project still exists.
            if ComposeManager::get_project(&key).is_err() {
                continue;
            }
            out.push(crate::app_proxy::RestoreEntry {
                ns_id: m.namespaced_id,
                target_port: m.host_port,
                auth_required: m.proxy_auth_required,
                proxy_port: m.proxy_port,
            });
        }
        out
    })
    .await
    .unwrap_or_default()
}

/// Load the enabled community stores as `(id, name)` pairs for
/// `app_catalog::list_apps` / `find`. Shared with the MCP handler.
pub(crate) async fn enabled_stores(state: &Arc<AppState>) -> Vec<(String, String)> {
    let repo = AppStoreRepository::new(state.db.pool());
    repo.list()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id, s.name))
        .collect()
}

/// A media share the admin picked for a catalog app's library.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaShareChoice {
    pub name: String,
    /// Protocol, as `ShareType::as_str` (`smb`, `nfs`, `ftp`, `webdav`, `sftp`).
    pub share_type: String,
}

/// The resolved media library of an install: the configured share bound to
/// the app's container mount, empty when the app uses its own folder.
#[derive(Debug, Default)]
struct MediaBinding {
    /// Share name, `None` when no share was chosen.
    name: Option<String>,
    /// `ShareType::as_str` of that share.
    share_type: Option<String>,
    /// Host path recorded in the install marker.
    path: Option<String>,
    /// Host path actually mounted for this install.
    dir: Option<std::path::PathBuf>,
    /// Container path from the manifest (`/media`, `/mnt/media`, …).
    mount: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct InstallCatalogAppRequest {
    pub port: Option<u16>,
    /// `dependency -> namespaced provider id` overrides. Each override must
    /// really provide the dependency or the install is a 400.
    #[serde(default)]
    pub alternatives: std::collections::HashMap<String, String>,
    /// Values for the manifest's declared `environment` settings.
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    /// Mount one of the **configured** shares as the app's media library.
    /// Refused for a manifest that does not declare `share_mount`; the path is
    /// always resolved server-side, never taken from the request.
    #[serde(default)]
    pub media_share: Option<MediaShareChoice>,
    /// Hand `/dev/dri` to the container. Refused unless the manifest sets
    /// `gpu = true` and the host really exposes the device.
    #[serde(default)]
    pub gpu: bool,
}

/// Lists every catalog app (built-in + enabled stores) with its live
/// install / run state.
pub async fn handle_catalog_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    // Members only see the apps an admin granted them (`resource_shares`,
    // `kind = app`); admins get `None` and see everything. The filter is
    // server-side: a grant is not a UI preference, it is the only thing that
    // decides whether the app is listed at all.
    let visible = match crate::handlers::resource_shares::visible_app_ids(&state, &username).await {
        Ok(visible) => visible,
        Err(e) => return ResponseHelper::json_error(format!("Failed to resolve app access: {e}")),
    };

    let stores = enabled_stores(&state).await;
    let lang = crate::i18n::I18n::get_locale();
    let host_arch = std::env::consts::ARCH;
    let proxies = crate::app_proxy::APP_PROXY.snapshot().await;

    match tokio::task::spawn_blocking(move || {
        let apps = app_catalog::list_apps(&stores);

        // Which install keys currently have a Compose project (installed).
        // Computed over *every* app so a member's dependency check is not
        // fooled by a provider they simply cannot see.
        let installed_keys: std::collections::HashSet<String> = apps
            .iter()
            .filter(|a| ComposeManager::get_project(&a.install_key()).is_ok())
            .map(|a| a.bare_id().to_string())
            .collect();

        let may_see = |app: &app_catalog::CatalogApp| match &visible {
            None => true,
            Some(ids) => ids
                .iter()
                .any(|id| id == app.bare_id() || id == &app.namespaced_id()),
        };

        apps.iter()
            .filter(|app| may_see(app))
            .map(|app| {
                let key = app.install_key();
                let project = ComposeManager::get_project(&key).ok();
                let installed = project.is_some();
                let running = project
                    .as_ref()
                    .map(|p| p.status == "running")
                    .unwrap_or(false);

                let missing_dependencies = app
                    .manifest
                    .dependencies
                    .iter()
                    .filter(|d| !installed_keys.contains(*d))
                    .cloned()
                    .collect();

                let proxy = proxies.get(&app.namespaced_id());

                CatalogAppView {
                    id: app.namespaced_id(),
                    app_id: app.bare_id().to_string(),
                    name: app.manifest.name.clone(),
                    description: app.manifest.localized_description(&lang).to_string(),
                    tagline: app.manifest.tagline.clone(),
                    category: app.manifest.category.clone(),
                    icon: app.manifest.icon.clone(),
                    has_icon: app.icon_svg().is_some(),
                    gallery_count: app.gallery_count(),
                    default_port: app.manifest.host_port(),
                    version: app.manifest.version.clone(),
                    developer: app.manifest.developer.clone(),
                    website: app.manifest.website.clone(),
                    repo: app.manifest.repo.clone(),
                    release_notes: app.manifest.release_notes.clone(),
                    dependencies: app.manifest.dependencies.clone(),
                    implements: app.manifest.implements.clone(),
                    environment: app.manifest.environment.clone(),
                    missing_dependencies,
                    incompatibility_reason: app_catalog::compatibility(app),
                    store_id: app.store_id.clone(),
                    store_name: app.store_name.clone(),
                    installed,
                    running,
                    installed_port: if installed {
                        read_installed_port(&key)
                    } else {
                        None
                    },
                    proxy_port: proxy.map(|p| p.proxy_port),
                    proxy_auth_required: proxy
                        .map(|p| p.auth_required)
                        .or_else(|| {
                            read_install_marker_sync(&install_dir(&key))
                                .map(|m| m.proxy_auth_required)
                        })
                        .unwrap_or(true),
                    unsupported_on_host: !app.manifest.supports_host_arch(host_arch),
                    share_mount: app.manifest.share_mount().map(str::to_string),
                    gpu: app.manifest.gpu,
                    gpu_available: app_catalog::gpu_available(),
                    installed_share: if installed {
                        read_install_marker_sync(&install_dir(&key)).and_then(|m| m.share_name)
                    } else {
                        None
                    },
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    {
        Ok(views) => ResponseHelper::json_success(views),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Serves an app package's own `icon.svg` (204 when it ships none - the
/// frontend then falls back to the Lucide icon named in `icon`).
pub async fn handle_catalog_icon(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let stores = enabled_stores(&state).await;
    match tokio::task::spawn_blocking(move || {
        app_catalog::find(&stores, &id).and_then(|a| a.icon_svg())
    })
    .await
    {
        Ok(Some(bytes)) => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            bytes,
        )
            .into_response(),
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

/// Serves screenshot `n` (1-based) from an app package's `gallery/`.
pub async fn handle_catalog_gallery(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, n)): Path<(String, usize)>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let stores = enabled_stores(&state).await;
    match tokio::task::spawn_blocking(move || {
        app_catalog::find(&stores, &id).and_then(|a| a.gallery_file(n))
    })
    .await
    {
        Ok(Some((bytes, ct))) => (
            [
                (header::CONTENT_TYPE, ct),
                (header::CACHE_CONTROL, "public, max-age=3600"),
            ],
            bytes,
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Renders the chosen catalog app's compose template and starts it.
pub async fn handle_catalog_install(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<InstallCatalogAppRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match install_catalog_app(&state, &id, &req, &format!("Admin '{admin_username}'")).await {
        Ok(message) => ResponseHelper::json_success(message),
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// Shared install core behind the HTTP handler and the MCP `apps.install`
/// tool: validate, render, create + start the compose project, bring up the
/// per-app proxy, write the install marker. `actor` labels the event-log
/// line (`Admin 'x'` for HTTP, `MCP token <id>` for agents).
pub async fn install_catalog_app(
    state: &Arc<AppState>,
    id: &str,
    req: &InstallCatalogAppRequest,
    actor: &str,
) -> Result<String, String> {
    let stores = enabled_stores(state).await;
    let Some(app) = app_catalog::find(&stores, id) else {
        return Err(format!("Unknown catalog app: {}", id));
    };

    let install_key = app.install_key();
    // `get_project` shells out to `docker compose`, so it never runs on a
    // runtime worker.
    let already_installed = {
        let key = install_key.clone();
        tokio::task::spawn_blocking(move || ComposeManager::get_project(&key).is_ok())
            .await
            .unwrap_or(false)
    };
    if already_installed {
        return Err(format!("{} is already installed", app.manifest.name));
    }

    // Compatibility gate (arch + min_homevault_version) — same reason the
    // catalog list shows, enforced server-side so the UI can't bypass it.
    if let Some(reason) = app_catalog::compatibility(&app) {
        return Err(format!("Cannot install {}: {}", app.manifest.name, reason));
    }

    if let Err(e) = app_catalog::validate_env_values(&app, &req.environment) {
        return Err(format!("Invalid settings for {}: {}", app.manifest.name, e));
    }

    // Media integration: bind a **configured** share into the app, never an
    // arbitrary host path. The manifest decides whether the app consumes a
    // share at all and where inside the container it lands.
    let mut media = MediaBinding::default();
    if let Some(choice) = &req.media_share {
        let Some(mount) = app.manifest.share_mount() else {
            return Err(format!("{} does not use a media share", app.manifest.name));
        };
        let Some(kind) = crate::shares::ShareType::parse(&choice.share_type) else {
            return Err(format!("Unknown share type: {}", choice.share_type));
        };
        let name = choice.name.clone();
        let lookup = kind.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            crate::shares::SharesService::get_share(&name, lookup)
        })
        .await
        .map_err(|e| format!("share lookup task failed: {e}"))?;
        let share = match resolved {
            Ok(share) => share,
            Err(e) => return Err(format!("Cannot use share '{}': {e}", choice.name)),
        };
        // The share's own path is what gets mounted; anything else (relative,
        // missing, not a directory) would make Docker create an empty
        // directory and the app would scan nothing.
        let dir = std::path::PathBuf::from(&share.path);
        if !dir.is_absolute() {
            return Err(format!(
                "Share '{}' has a relative path ({}), which cannot be mounted",
                share.name, share.path
            ));
        }
        if !dir.is_dir() {
            return Err(format!(
                "Share '{}' is not available at {} on this server",
                share.name, share.path
            ));
        }
        media = MediaBinding {
            name: Some(share.name.clone()),
            share_type: Some(kind.as_str().to_string()),
            path: Some(share.path.clone()),
            dir: Some(dir),
            mount: mount.to_string(),
        };
    } else if let Some(mount) = app.manifest.share_mount() {
        // No share chosen: the app still gets a working (empty) library in
        // its own data directory, so it starts without an error.
        media.mount = mount.to_string();
    }

    if req.gpu {
        if !app.manifest.gpu {
            return Err(format!(
                "{} does not support hardware acceleration",
                app.manifest.name
            ));
        }
        if !app_catalog::gpu_available() {
            return Err(format!(
                "Hardware acceleration needs {} on this server, and it is missing",
                app_catalog::GPU_DEVICE_DIR
            ));
        }
    }

    let render_options = app_catalog::RenderOptions {
        env: req.environment.clone(),
        share_dir: media.dir.clone(),
        gpu: req.gpu,
    };

    let port = req.port.unwrap_or(app.manifest.host_port());

    // Port-conflict guard (same check the create-container form uses).
    let port_status = crate::docker::DockerService::check_port(port);
    if !port_status.available {
        let reason = match port_status.used_by.as_deref() {
            Some("container") => format!(
                "port {} is already used by container '{}'",
                port,
                port_status.detail.as_deref().unwrap_or("?")
            ),
            Some("homevault") => format!("port {} is reserved by the HomeVault server", port),
            _ => format!("port {} is already in use on the host", port),
        };
        return Err(format!("Cannot install {}: {}", app.manifest.name, reason));
    }

    // Dependency gate: every declared dependency must already be satisfied
    // by an installed app (canonical id or an `implements` alternative), or
    // map to a catalog provider via `alternatives`/default selection.
    // Missing providers are an error naming what to install first —
    // dependency installs stay an explicit admin decision, never an
    // implicit pull.
    //
    // Listing the stores reads the filesystem and `get_project` shells out to
    // `docker compose` once per catalog entry, so that half runs on the
    // blocking pool; the plan itself is pure.
    let (all_apps, installed) = {
        let stores = stores.clone();
        tokio::task::spawn_blocking(move || {
            let all_apps = app_catalog::list_apps(&stores);
            let installed: std::collections::HashSet<String> = all_apps
                .iter()
                .filter(|a| ComposeManager::get_project(&a.install_key()).is_ok())
                .map(|a| a.bare_id().to_string())
                .collect();
            (all_apps, installed)
        })
        .await
        .map_err(|e| format!("install plan task failed: {e}"))?
    };
    {
        for (dep, want) in &req.alternatives {
            if !app.manifest.dependencies.contains(dep) {
                return Err(format!("{} does not depend on {dep:?}", app.manifest.name));
            }
            let provides = app_catalog::providers_of(&all_apps, dep)
                .iter()
                .any(|a| &a.namespaced_id() == want);
            if !provides {
                return Err(format!("{want:?} does not provide {dep:?}"));
            }
        }
        let plan =
            app_catalog::resolve_install_plan(&all_apps, &app, &installed, &req.alternatives);
        let missing: Vec<String> = plan
            .iter()
            .filter(|n| n.satisfied_by.is_empty() && n.selected.is_none())
            .map(|n| {
                if n.providers.is_empty() {
                    format!(
                        "{} (no known provider — add a store that ships it)",
                        n.dependency
                    )
                } else {
                    format!("{} (try {})", n.dependency, n.providers.join(" or "))
                }
            })
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "Cannot install {}: missing dependencies: {}",
                app.manifest.name,
                missing.join("; ")
            ));
        }
    }

    let (rendered, data_dir) = match app_catalog::render_with_options(&app, port, &render_options) {
        Ok(v) => v,
        Err(e) => {
            return Err(format!("Failed to render {}: {}", app.manifest.name, e));
        }
    };

    if let Err(e) = tokio::fs::create_dir_all(&data_dir).await {
        return Err(format!("Failed to create data directory: {}", e));
    }

    // Hand the app's data to the account declared by `[security] user`
    // *before* compose starts. Docker creates a missing bind-mount source as
    // `root:root`, which would lock out a container started as a non-root uid,
    // so the sub-directories the template mounts are created here as well.
    apply_data_ownership(&data_dir, &app.manifest.security);
    for volume_dir in app_catalog::data_volume_paths(&rendered, &data_dir) {
        if let Err(e) = tokio::fs::create_dir_all(&volume_dir).await {
            return Err(format!(
                "Failed to create data directory {}: {}",
                volume_dir.display(),
                e
            ));
        }
        apply_data_ownership(&volume_dir, &app.manifest.security);
    }

    let key = install_key.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || ComposeManager::create_project(&key, &rendered))
            .await
            .map_err(|e| format!("create_project task failed: {e}"))
            .and_then(|r| r.map_err(|e| e.to_string()))
    {
        return Err(format!("Failed to create project: {}", e));
    }

    // Persist the install marker **before** anything runs: a marker that
    // cannot be written (read-only or full apps directory) must abort the
    // install instead of producing an app that vanishes after a restart. The
    // proxy port is filled in below, once the proxy is actually up.
    let marker = InstallMarker {
        store_id: app.store_id.clone(),
        app_id: app.bare_id().to_string(),
        namespaced_id: app.namespaced_id(),
        package_version: app.manifest.version.clone(),
        host_port: port,
        proxy_port: None,
        proxy_auth_required: true,
        environment: req.environment.clone(),
        share_name: media.name.clone(),
        share_type: media.share_type.clone(),
        share_path: media.path.clone(),
        gpu: req.gpu,
    };
    if let Err(e) = write_install_marker(&install_key, marker.clone()).await {
        return Err(format!(
            "Failed to record the install of {}: {}",
            app.manifest.name, e
        ));
    }

    // `start_project` runs `docker compose up -d`, which pulls the images —
    // can take minutes. It is synchronous, so it runs on the blocking pool; a
    // first install on the runtime would pin a worker for its whole duration.
    // Surface it in the desktop activity taskbar as an indeterminate job (no
    // percentage available from compose).
    let job_id = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("docker_install".to_string()),
        &format!("Installing {}", app.manifest.name),
    );

    let key = install_key.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || ComposeManager::start_project(&key))
        .await
        .map_err(|e| format!("start_project task failed: {e}"))
        .and_then(|r| r.map_err(|e| e.to_string()))
    {
        crate::sse::PROGRESS_MANAGER.fail_job(&job_id, &e);
        return Err(format!("Failed to start {}: {}", app.manifest.name, e));
    }

    // Home Assistant uses `network_mode: host` - no compose-managed port.
    if app.bare_id() != "home-assistant" {
        if let Err(e) = tokio::fs::write(data_dir.join(".port"), port.to_string()).await {
            tracing::warn!(
                "could not record the published port of {}: {}",
                app.manifest.name,
                e
            );
        }
    }

    // Bring up a per-app reverse proxy (auth-gated by default). Best effort:
    // a proxy failure doesn't fail the install — the app is still reachable
    // on its own published port.
    let proxy_port = crate::app_proxy::APP_PROXY
        .start(&app.namespaced_id(), port, true, None)
        .await
        .map_err(|e| tracing::warn!("app proxy for {} not started: {}", app.namespaced_id(), e))
        .ok();

    // Record the proxy port so the proxy is restored on the next start. A
    // failure here is not silent either: the app is stopped again rather than
    // left running in a state the UI cannot describe.
    let mut marker = marker;
    marker.proxy_port = proxy_port;
    if let Err(e) = write_install_marker(&install_key, marker).await {
        let key = install_key.clone();
        let _ = tokio::task::spawn_blocking(move || ComposeManager::stop_project(&key)).await;
        crate::sse::PROGRESS_MANAGER.fail_job(&job_id, &e);
        return Err(format!(
            "Failed to record the proxy of {}: {}",
            app.manifest.name, e
        ));
    }

    crate::sse::PROGRESS_MANAGER.complete_job(&job_id, &format!("{} installed", app.manifest.name));

    crate::events::log_event(format!(
        "{} installed catalog app '{}' from store '{}' (port: {}){}{}",
        actor,
        app.manifest.name,
        app.store_id,
        port,
        media
            .name
            .as_ref()
            .map(|name| format!(", library: share '{name}' on {}", media.mount))
            .unwrap_or_default(),
        if req.gpu { ", GPU: /dev/dri" } else { "" }
    ));
    Ok(format!("{} installed and started", app.manifest.name))
}

/// Stops and removes a previously installed catalog app.
pub async fn handle_catalog_uninstall(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match uninstall_catalog_app(&state, &id, &format!("Admin '{admin_username}'")).await {
        Ok(message) => ResponseHelper::json_success(message),
        Err(e) => ResponseHelper::json_error(e),
    }
}

/// Shared uninstall core behind the HTTP handler and the MCP
/// `apps.uninstall` tool.
pub async fn uninstall_catalog_app(
    state: &Arc<AppState>,
    id: &str,
    actor: &str,
) -> Result<String, String> {
    let stores = enabled_stores(state).await;
    let Some(app) = app_catalog::find(&stores, id) else {
        return Err(format!("Unknown catalog app: {}", id));
    };
    let install_key = app.install_key();

    crate::app_proxy::APP_PROXY.stop(&app.namespaced_id()).await;
    // Both compose calls are synchronous CLI invocations (stop can wait on
    // container shutdown), so they run on the blocking pool.
    let key = install_key.clone();
    let _ = tokio::task::spawn_blocking(move || ComposeManager::stop_project(&key)).await;

    let key = install_key.clone();
    let removed = tokio::task::spawn_blocking(move || ComposeManager::delete_project(&key)).await;
    match removed {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "{} uninstalled catalog app '{}'",
                actor, app.manifest.name
            ));
            Ok(format!("{} uninstalled", app.manifest.name))
        }
        Ok(Err(e)) => Err(format!("Failed to uninstall {}: {}", app.manifest.name, e)),
        Err(e) => Err(format!("Failed to uninstall {}: {}", app.manifest.name, e)),
    }
}

// ───────────────────────────── App updates ──────────────────────────────────

/// Lists installed catalog apps that have an image or package update
/// available. Read-only registry digest checks; no image is pulled.
pub async fn handle_catalog_updates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    let stores = enabled_stores(&state).await;
    let updates = crate::app_catalog::updates::check_installed_apps(&stores).await;
    ResponseHelper::json_success(updates)
}

/// Pulls newer images (and re-renders the template) for one installed app,
/// then recreates its stack.
pub async fn handle_catalog_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let stores = enabled_stores(&state).await;
    let Some(app) = crate::app_catalog::find(&stores, &id) else {
        return ResponseHelper::json_error(format!("Unknown catalog app: {}", id));
    };
    let install_key = app.install_key();
    let check_key = install_key.clone();
    let installed =
        tokio::task::spawn_blocking(move || ComposeManager::get_project(&check_key).is_ok())
            .await
            .unwrap_or(false);
    if !installed {
        return ResponseHelper::json_error(format!("{} is not installed", app.manifest.name));
    }
    let host_port = read_installed_port_async(&install_key)
        .await
        .unwrap_or(app.manifest.host_port());

    let name = app.manifest.name.clone();
    let prev = read_install_marker(&install_key).await;
    let marker = InstallMarker {
        store_id: app.store_id.clone(),
        app_id: app.bare_id().to_string(),
        namespaced_id: app.namespaced_id(),
        package_version: app.manifest.version.clone(),
        host_port,
        proxy_port: prev.as_ref().and_then(|m| m.proxy_port),
        proxy_auth_required: prev.as_ref().map(|m| m.proxy_auth_required).unwrap_or(true),
        // Carry the admin's install choices through the update: dropping
        // them would remount the app's library as an empty folder and
        // quietly turn off hardware transcoding.
        environment: prev
            .as_ref()
            .map(|m| m.environment.clone())
            .unwrap_or_default(),
        share_name: prev.as_ref().and_then(|m| m.share_name.clone()),
        share_type: prev.as_ref().and_then(|m| m.share_type.clone()),
        share_path: prev.as_ref().and_then(|m| m.share_path.clone()),
        gpu: prev.as_ref().map(|m| m.gpu).unwrap_or(false),
    };
    // `apply_update` = git pull (store) + `docker compose pull` + `up -d` —
    // minutes-long, no progress signal. Show it as an indeterminate job in the
    // desktop activity taskbar.
    let job_id = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("docker_update".to_string()),
        &format!("Updating {}", name),
    );
    let options_marker = marker.clone();
    match tokio::task::spawn_blocking(move || {
        crate::app_catalog::updates::apply_update(&app, host_port, &options_marker.render_options())
    })
    .await
    {
        Ok(Ok(())) => match write_install_marker(&install_key, marker).await {
            Ok(()) => {
                crate::sse::PROGRESS_MANAGER.complete_job(&job_id, &format!("{} updated", name));
                crate::events::log_event(format!(
                    "Admin '{}' updated catalog app '{}'",
                    admin, name
                ));
                ResponseHelper::json_success(format!("{} updated", name))
            }
            // The images are updated but the marker could not be rewritten:
            // report it instead of claiming a clean update.
            Err(e) => {
                crate::sse::PROGRESS_MANAGER.fail_job(&job_id, &e);
                ResponseHelper::json_error(format!(
                    "{} updated, but its install marker could not be recorded: {}",
                    name, e
                ))
            }
        },
        Ok(Err(e)) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job_id, &e.to_string());
            ResponseHelper::json_error(format!("Failed to update {}: {}", name, e))
        }
        Err(e) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job_id, &e.to_string());
            ResponseHelper::json_error(format!("Update task failed: {}", e))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfigRequest {
    /// Whether the per-app proxy requires a HomeVault session.
    pub auth_required: bool,
}

/// Pre-install review: dependencies with
/// their installed satisfiers and alternative providers, declared
/// environment settings, and the server-side compatibility verdict — so the
/// install dialog can offer choices before the long image pull starts.
pub async fn handle_catalog_install_review(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let stores = enabled_stores(&state).await;
    let Some(app) = app_catalog::find(&stores, &id) else {
        return ResponseHelper::json_error(format!("Unknown catalog app: {}", id));
    };
    let review = tokio::task::spawn_blocking(move || {
        let apps = app_catalog::list_apps(&stores);
        let installed: std::collections::HashSet<String> = apps
            .iter()
            .filter(|a| ComposeManager::get_project(&a.install_key()).is_ok())
            .map(|a| a.bare_id().to_string())
            .collect();
        let needs = app_catalog::resolve_install_plan(
            &apps,
            &app,
            &installed,
            &std::collections::HashMap::new(),
        );
        serde_json::json!({
            "id": app.namespaced_id(),
            "name": app.manifest.name,
            "version": app.manifest.version,
            "incompatibility_reason": app_catalog::compatibility(&app),
            "dependencies": needs.iter().map(|n| serde_json::json!({
                "dependency": n.dependency,
                "satisfied_by": n.satisfied_by,
                "providers": n.providers,
                "selected": n.selected,
            })).collect::<Vec<_>>(),
            "environment": app.manifest.environment,
            "default_port": app.manifest.host_port(),
            // Media integration: only offered for an app that mounts a share.
            "share_mount": app.manifest.share_mount(),
            "gpu": app.manifest.gpu,
            "gpu_available": app_catalog::gpu_available(),
            "shares": if app.manifest.share_mount().is_some() {
                crate::shares::SharesService::get_shares()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|s| s.enabled)
                    .map(|s| serde_json::json!({
                        "name": s.name,
                        "share_type": s.share_type.as_str(),
                        "path": s.path,
                    }))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            },
        })
    })
    .await;
    match review {
        Ok(body) => ResponseHelper::json_success(body),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Per-app outcome of an update-all pass.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CatalogUpdateReport {
    pub updated: Vec<String>,
    pub failed: Vec<serde_json::Value>,
}

/// Updates every installed catalog app with a pending update. Shared by the
/// admin "update all" action and the scheduled updater (P1 Docker updates), so
/// both paths behave identically and a failure never aborts the queue.
pub(crate) async fn update_all_pending(state: &Arc<AppState>, actor: &str) -> CatalogUpdateReport {
    let stores = enabled_stores(state).await;
    let pending = crate::app_catalog::updates::check_installed_apps(&stores).await;
    if pending.is_empty() {
        return CatalogUpdateReport::default();
    }
    let job_id = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("docker_update_all".to_string()),
        &format!("Updating {} apps", pending.len()),
    );
    let mut updated = Vec::new();
    let mut failed = Vec::new();
    for item in &pending {
        let Some(app) = app_catalog::find(&stores, &item.id) else {
            failed.push(serde_json::json!({"id": item.id, "error": "catalog entry vanished"}));
            continue;
        };
        let install_key = app.install_key();
        let host_port = read_installed_port_async(&install_key)
            .await
            .unwrap_or(app.manifest.host_port());
        let version = app.manifest.version.clone();
        let store_id = app.store_id.clone();
        let bare_id = app.bare_id().to_string();
        let ns_id = app.namespaced_id();
        let prev = read_install_marker(&install_key).await;
        let carry = prev.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let options = carry
                .as_ref()
                .map(InstallMarker::render_options)
                .unwrap_or_default();
            crate::app_catalog::updates::apply_update(&app, host_port, &options)
        })
        .await;
        match outcome {
            Ok(Ok(())) => {
                let marker = InstallMarker {
                    store_id,
                    app_id: bare_id,
                    namespaced_id: ns_id,
                    package_version: version,
                    host_port,
                    proxy_port: prev.as_ref().and_then(|m| m.proxy_port),
                    proxy_auth_required: prev
                        .as_ref()
                        .map(|m| m.proxy_auth_required)
                        .unwrap_or(true),
                    environment: prev
                        .as_ref()
                        .map(|m| m.environment.clone())
                        .unwrap_or_default(),
                    share_name: prev.as_ref().and_then(|m| m.share_name.clone()),
                    share_type: prev.as_ref().and_then(|m| m.share_type.clone()),
                    share_path: prev.as_ref().and_then(|m| m.share_path.clone()),
                    gpu: prev.as_ref().map(|m| m.gpu).unwrap_or(false),
                };
                match write_install_marker(&install_key, marker).await {
                    Ok(()) => updated.push(item.id.clone()),
                    Err(e) => failed.push(
                        serde_json::json!({"id": item.id, "error": format!("updated, but its install marker could not be recorded: {e}")}),
                    ),
                }
            }
            Ok(Err(e)) => failed.push(serde_json::json!({"id": item.id, "error": e.to_string()})),
            Err(e) => failed.push(serde_json::json!({"id": item.id, "error": e.to_string()})),
        }
    }
    if failed.is_empty() {
        crate::sse::PROGRESS_MANAGER
            .complete_job(&job_id, &format!("{} apps updated", updated.len()));
    } else {
        crate::sse::PROGRESS_MANAGER.fail_job(
            &job_id,
            &format!("{} updated, {} failed", updated.len(), failed.len()),
        );
    }
    if actor == "scheduler" {
        crate::events::log_event(format!(
            "Scheduled Docker update: {} updated, {} failed",
            updated.len(),
            failed.len()
        ));
    } else {
        crate::events::log_event(format!(
            "Admin '{}' ran update-all on catalog apps: {} updated, {} failed",
            actor,
            updated.len(),
            failed.len()
        ));
    }
    CatalogUpdateReport { updated, failed }
}

/// One-shot update queue for every installed app with a pending update
/// ("update all"): runs the same `apply_update` path as the single-app
/// endpoint sequentially and reports a per-app outcome, so one failure never
/// aborts the rest of the queue.
pub async fn handle_catalog_update_all(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let report = update_all_pending(&state, &admin).await;
    ResponseHelper::json_success(report)
}

/// Tail an installed app's compose logs (per-app logs viewer).
/// Query: `?tail=200` (default 200, max 2000). Read-only, any session.
pub async fn handle_catalog_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let stores = enabled_stores(&state).await;
    let Some(app) = app_catalog::find(&stores, &id) else {
        return ResponseHelper::json_error(format!("Unknown catalog app: {}", id));
    };
    let install_key = app.install_key();
    if ComposeManager::get_project(&install_key).is_err() {
        return ResponseHelper::json_error(format!("{} is not installed", app.manifest.name));
    }
    let tail: usize = query
        .get("tail")
        .and_then(|t| t.parse().ok())
        .unwrap_or(200)
        .clamp(1, 2000);
    let output = tokio::task::spawn_blocking(move || {
        // Bounded: `docker compose logs` streams from the daemon, and a daemon
        // that stopped answering used to hold this blocking thread for ever
        // (ROADMAP X.9).
        let mut cmd = std::process::Command::new("docker");
        cmd.args([
            "compose",
            "-p",
            &install_key,
            "logs",
            "--no-color",
            "--tail",
            &tail.to_string(),
        ]);
        crate::utils::process::run_timed(&mut cmd, COMPOSE_LOGS_TIMEOUT)
            .map_err(|e| format!("failed to run docker compose logs: {e}"))
            .and_then(|o| {
                if o.status.success() {
                    Ok(String::from_utf8_lossy(&o.stdout).to_string())
                } else {
                    Err(format!(
                        "docker compose logs: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ))
                }
            })
    })
    .await;
    match output {
        Ok(Ok(logs)) => ResponseHelper::json_success(serde_json::json!({ "logs": logs })),
        Ok(Err(e)) => ResponseHelper::json_error(e),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Toggle the auth gate on an installed app's reverse proxy.
pub async fn handle_catalog_proxy_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Json(req): axum::extract::Json<ProxyConfigRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let stores = enabled_stores(&state).await;
    let Some(app) = app_catalog::find(&stores, &id) else {
        return ResponseHelper::json_error(format!("Unknown catalog app: {}", id));
    };
    let install_key = app.install_key();
    let ns_id = app.namespaced_id();

    // Apply live if the proxy is running, and persist the preference either
    // way (it is picked up on the next proxy start / server restart). The two
    // outcomes are reported separately: a live flag that could not be stored
    // must not be answered as a plain success.
    let applied = crate::app_proxy::APP_PROXY
        .set_auth_required(&ns_id, req.auth_required)
        .await;
    let existing = read_install_marker(&install_key).await;
    let mut marker = match existing {
        Some(mut m) => {
            m.proxy_auth_required = req.auth_required;
            m
        }
        // No marker (installed before markers existed, or the file was
        // truncated): rebuild it from the catalog entry and live state rather
        // than failing the toggle.
        None => InstallMarker {
            store_id: app.store_id.clone(),
            app_id: app.bare_id().to_string(),
            namespaced_id: ns_id.clone(),
            package_version: app.manifest.version.clone(),
            host_port: read_installed_port_async(&install_key)
                .await
                .unwrap_or(app.manifest.host_port()),
            proxy_port: crate::app_proxy::APP_PROXY
                .info(&ns_id)
                .await
                .map(|i| i.proxy_port),
            proxy_auth_required: req.auth_required,
            environment: std::collections::HashMap::new(),
            share_name: None,
            share_type: None,
            share_path: None,
            gpu: false,
        },
    };
    marker.proxy_auth_required = req.auth_required;
    if let Err(e) = write_install_marker(&install_key, marker).await {
        return ResponseHelper::json_error(format!(
            "Could not save the proxy setting for {}: {}",
            app.manifest.name, e
        ));
    }

    crate::events::log_event(format!(
        "Admin '{}' set proxy auth for '{}' to {}",
        admin, app.manifest.name, req.auth_required
    ));
    ResponseHelper::json_success(serde_json::json!({
        "auth_required": req.auth_required,
        // `applied` is false when the app's proxy is not running: the value is
        // saved and takes effect when it next starts.
        "applied": applied,
    }))
}

// ─────────────────────────── Community app stores ───────────────────────────

#[derive(Debug, Serialize)]
struct StoreView {
    id: String,
    name: String,
    git_url: String,
    enabled: bool,
    last_synced_at: Option<i64>,
    last_error: Option<String>,
    app_count: usize,
    /// P1-27: `verified`, `unconfigured` or `invalid`.
    signature: String,
    /// Why an `invalid` store is refused (bounded, no key material).
    signature_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddStoreRequest {
    pub git_url: String,
}

pub async fn handle_stores_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let repo = AppStoreRepository::new(state.db.pool());
    let stores = match repo.list().await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to list stores: {}", e)),
    };

    let views = tokio::task::spawn_blocking(move || {
        stores
            .into_iter()
            .map(|s| {
                let dir = app_store::store_dir(&s.id);
                let signature = crate::catalog_signature::state_for_store(&dir);
                let app_count = if s.enabled && signature.as_str() != "invalid" {
                    app_store::app_dirs(&dir).len()
                } else {
                    0
                };
                let signature_error = match &signature {
                    crate::catalog_signature::SignatureState::Invalid(reason) => {
                        Some(reason.clone())
                    }
                    _ => None,
                };
                StoreView {
                    id: s.id,
                    name: s.name,
                    git_url: s.git_url,
                    enabled: s.enabled,
                    last_synced_at: s.last_synced_at,
                    last_error: s.last_error,
                    app_count,
                    signature: signature.as_str().to_string(),
                    signature_error,
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    ResponseHelper::json_success(views)
}

pub async fn handle_store_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<AddStoreRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let git_url = req.git_url.trim().to_string();
    if let Err(e) = app_store::validate_git_url(&git_url) {
        return ResponseHelper::json_error(e);
    }
    let store_id = app_store::slug_from_url(&git_url);
    if store_id == app_catalog::BUILTIN_STORE_ID {
        return ResponseHelper::json_error(
            "that store id is reserved for the built-in catalog".to_string(),
        );
    }

    // Clone/sync first so a bad URL never leaves a dangling DB row.
    let dest = app_store::store_dir(&store_id);
    let sync_url = git_url.clone();
    let desc = match tokio::task::spawn_blocking(move || app_store::sync(&sync_url, &dest)).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => return ResponseHelper::json_error(format!("Could not sync store: {}", e)),
        Err(e) => return ResponseHelper::json_error(format!("Sync task failed: {}", e)),
    };

    let repo = AppStoreRepository::new(state.db.pool());
    if let Err(e) = repo.add(&store_id, &desc.name, &git_url, &admin).await {
        let _ = std::fs::remove_dir_all(app_store::store_dir(&store_id));
        return ResponseHelper::json_error(format!("This store is already added ({})", e));
    }
    let now = chrono::Utc::now().timestamp();
    let _ = repo.record_sync(&store_id, Some(now), None).await;

    crate::events::log_event(format!(
        "Admin '{}' added app store '{}' ({})",
        admin, desc.name, git_url
    ));
    ResponseHelper::json_success(serde_json::json!({ "id": store_id, "name": desc.name }))
}

pub async fn handle_store_sync(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    let repo = AppStoreRepository::new(state.db.pool());
    let Ok(Some(store)) = repo.get(&id).await else {
        return ResponseHelper::json_error(format!("Unknown store: {}", id));
    };

    let dest = app_store::store_dir(&store.id);
    let url = store.git_url.clone();
    let result = tokio::task::spawn_blocking(move || app_store::sync(&url, &dest)).await;

    match result {
        Ok(Ok(_)) => {
            let now = chrono::Utc::now().timestamp();
            let _ = repo.record_sync(&store.id, Some(now), None).await;
            ResponseHelper::json_success(format!("{} synced", store.name))
        }
        Ok(Err(e)) => {
            let _ = repo.record_sync(&store.id, None, Some(&e)).await;
            ResponseHelper::json_error(format!("Sync failed: {}", e))
        }
        Err(e) => ResponseHelper::json_error(format!("Sync task failed: {}", e)),
    }
}

pub async fn handle_store_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let repo = AppStoreRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => {
            let _ = std::fs::remove_dir_all(app_store::store_dir(&id));
            crate::events::log_event(format!("Admin '{}' removed app store '{}'", admin, id));
            ResponseHelper::json_success(format!("Store '{}' removed", id))
        }
        Ok(false) => ResponseHelper::json_error(format!("Unknown store: {}", id)),
        Err(e) => ResponseHelper::json_error(format!("Failed to remove store: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(proxy_port: Option<u16>) -> InstallMarker {
        InstallMarker {
            store_id: "official".to_string(),
            app_id: "joplin".to_string(),
            namespaced_id: "official:joplin".to_string(),
            package_version: "1.2.3".to_string(),
            host_port: 22300,
            proxy_port,
            proxy_auth_required: true,
            environment: std::collections::HashMap::new(),
            share_name: None,
            share_type: None,
            share_path: None,
            gpu: false,
        }
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hv-catalog-marker-{}-{}",
            name,
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn test_install_marker_round_trips_through_the_filesystem() {
        let dir = scratch_dir("round-trip");
        write_install_marker_sync(&dir, &marker(Some(47001))).expect("marker written");
        let read = read_install_marker_sync(&dir).expect("marker read back");
        assert_eq!(read.namespaced_id, "official:joplin");
        assert_eq!(read.proxy_port, Some(47001));
        assert!(read.proxy_auth_required);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A marker written before the proxy starts carries no proxy port; the
    /// auth gate still defaults to on.
    #[test]
    fn test_install_marker_without_proxy_port_keeps_auth_on() {
        let dir = scratch_dir("no-proxy");
        write_install_marker_sync(&dir, &marker(None)).expect("marker written");
        let read = read_install_marker_sync(&dir).expect("marker read back");
        assert_eq!(read.proxy_port, None);
        assert!(read.proxy_auth_required);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ROADMAP 6.1: an unwritable apps directory must be *reported*. The old
    /// `write_install_marker` discarded the write result, so the install looked
    /// successful and the app vanished after a restart.
    #[test]
    fn test_install_marker_write_failure_is_reported() {
        // A plain file where the app directory is expected: the write fails
        // whatever the process uid, unlike a chmod-based test under root.
        let parent = scratch_dir("not-a-dir");
        let not_a_dir = parent.join("app");
        std::fs::write(&not_a_dir, b"not a directory").expect("file created");

        let err = write_install_marker_sync(&not_a_dir, &marker(None)).expect_err("must fail");
        assert!(err.contains("cannot write"), "got: {err}");
        assert!(read_install_marker_sync(&not_a_dir).is_none());
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn test_missing_marker_reads_as_none() {
        let dir = scratch_dir("missing");
        assert!(read_install_marker_sync(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P1-34: the marker carries every admin choice that changes the rendered
    /// compose file, so an update rebuilds the same stack instead of defaults.
    #[test]
    fn test_install_marker_round_trips_media_choices_and_settings() {
        let dir = scratch_dir("media");
        let mut m = marker(None);
        m.environment
            .insert("TZ".to_string(), "Europe/Paris".to_string());
        m.share_name = Some("hv-missing-test-share".to_string());
        m.share_type = Some("smb".to_string());
        m.share_path = Some("/srv/media".to_string());
        m.gpu = true;
        write_install_marker_sync(&dir, &m).expect("marker written");

        let read = read_install_marker_sync(&dir).expect("marker read back");
        assert_eq!(
            read.environment.get("TZ").map(String::as_str),
            Some("Europe/Paris")
        );
        assert_eq!(read.share_name.as_deref(), Some("hv-missing-test-share"));
        assert_eq!(read.share_type.as_deref(), Some("smb"));
        assert!(read.gpu);

        let options = read.render_options();
        assert_eq!(
            options.env.get("TZ").map(String::as_str),
            Some("Europe/Paris")
        );
        assert!(options.gpu);
        // The share is re-resolved by name; one that no longer exists falls
        // back to the recorded path, so the update still renders.
        assert_eq!(
            options.share_dir.as_deref(),
            Some(std::path::Path::new("/srv/media"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A marker written before media integration existed has none of the new
    /// keys: it must still load, with defaults rather than a parse error.
    #[test]
    fn test_legacy_marker_without_media_fields_loads() {
        let dir = scratch_dir("legacy");
        std::fs::write(
            dir.join(".install.toml"),
            "store_id = \"homevault\"\n\
             app_id = \"jellyfin\"\n\
             namespaced_id = \"homevault:jellyfin\"\n\
             package_version = \"1.0.0\"\n\
             host_port = 8096\n",
        )
        .expect("legacy marker written");
        let read = read_install_marker_sync(&dir).expect("legacy marker must parse");
        assert!(read.environment.is_empty());
        assert!(!read.gpu);
        assert!(read.share_name.is_none());
        assert!(read.proxy_auth_required, "absent flag still means gated");
        assert!(read.render_options().share_dir.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
