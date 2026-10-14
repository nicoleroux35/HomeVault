/* =============================================================================
 * File          : updates.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   App-update detection for installed catalog apps, from two independent
 *   signals: the running image digest vs the registry's digest for the same
 *   tag, and the installed manifest version vs the catalog's. Feeds the
 *   update-check and update-all endpoints.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   handlers/docker_catalog.rs update endpoints.
 * =============================================================================
 */

//! App-update detection for installed catalog apps.
//!
//! Two independent signals:
//!  1. **Image update** — a service's running image digest differs from the
//!     digest the registry currently serves for that same tag (e.g.
//!     `:latest` moved). Checked read-only via the registry v2 API with an
//!     anonymous bearer token; no image is pulled.
//!  2. **Package update** — the store now ships a newer template `version`
//!     than the one recorded in the app's `.install.toml` marker.
//!
//! Applying an update is `docker compose pull` + `up -d` (re-rendering the
//! template first when it's a package update), via `ComposeManager`.

use crate::app_catalog::{self, CatalogApp};
use crate::compose::ComposeManager;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

/// One service's image-update status.
#[derive(Debug, Clone, Serialize)]
pub struct ImageUpdate {
    pub image: String,
    pub local_digest: Option<String>,
    pub remote_digest: Option<String>,
    pub update_available: bool,
}

/// Per-app update summary.
#[derive(Debug, Clone, Serialize)]
pub struct AppUpdateStatus {
    pub id: String,
    pub name: String,
    pub store_id: String,
    /// The template version currently installed (`.install.toml`).
    pub installed_version: Option<String>,
    /// The template version the store ships now.
    pub available_version: String,
    pub package_update_available: bool,
    pub image_update_available: bool,
    pub images: Vec<ImageUpdate>,
}

impl AppUpdateStatus {
    pub fn any_update(&self) -> bool {
        self.package_update_available || self.image_update_available
    }
}

/// Parsed OCI image reference.
#[derive(Debug, PartialEq)]
pub struct ImageRef {
    /// Registry host (`registry-1.docker.io`, `ghcr.io`, …).
    pub registry: String,
    /// Repository path, always including any implicit `library/`.
    pub repository: String,
    pub tag: String,
}

/// Parse `nginx`, `nginx:1.25`, `ghcr.io/immich-app/immich-server:release`,
/// `lscr.io/linuxserver/bookstack:latest`, `docker.io/library/redis:7`.
/// A `@sha256:…` digest suffix is stripped (we look the tag up fresh).
pub fn parse_image_ref(image: &str) -> Option<ImageRef> {
    let image = image.split('@').next().unwrap_or(image).trim();
    if image.is_empty() {
        return None;
    }

    let (head, rest) = match image.split_once('/') {
        // First segment looks like a registry host (has a dot or a port,
        // or is "localhost").
        Some((h, r)) if h.contains('.') || h.contains(':') || h == "localhost" => {
            (h.to_string(), r.to_string())
        }
        _ => ("registry-1.docker.io".to_string(), image.to_string()),
    };

    let (repo_part, tag) = match rest.rsplit_once(':') {
        // Guard against a port in a path (shouldn't happen post-split) —
        // a ':' after the last '/' is the tag separator.
        Some((r, t)) if !t.contains('/') => (r.to_string(), t.to_string()),
        _ => (rest.clone(), "latest".to_string()),
    };

    // Docker Hub official images live under `library/`.
    let repository = if head == "registry-1.docker.io" && !repo_part.contains('/') {
        format!("library/{repo_part}")
    } else {
        repo_part
    };

    Some(ImageRef {
        registry: normalize_registry(&head),
        repository,
        tag,
    })
}

fn normalize_registry(host: &str) -> String {
    match host {
        "docker.io" | "index.docker.io" => "registry-1.docker.io".to_string(),
        other => other.to_string(),
    }
}

/// Parse the `Www-Authenticate: Bearer realm="…",service="…",scope="…"`
/// challenge into `(realm, service, scope)`.
pub fn parse_bearer_challenge(header: &str) -> Option<(String, Option<String>, Option<String>)> {
    let rest = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;
    let mut realm = None;
    let mut service = None;
    let mut scope = None;
    for part in rest.split(',') {
        let (k, v) = part.split_once('=')?;
        let v = v.trim().trim_matches('"').to_string();
        match k.trim() {
            "realm" => realm = Some(v),
            "service" => service = Some(v),
            "scope" => scope = Some(v),
            _ => {}
        }
    }
    Some((realm?, service, scope))
}

/// The `sha256:…` digest of the image a container/image currently runs,
/// from `docker inspect`'s `RepoDigests`. Blocking.
pub fn local_image_digest(image: &str) -> Option<String> {
    let out = Command::new("docker")
        .args(["inspect", "--format", "{{index .RepoDigests 0}}", image])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    line.split('@').nth(1).map(|s| s.trim().to_string())
}

/// The digest the registry currently serves for `image`'s tag. `None` on
/// any failure (offline, private registry, rate-limited) — callers treat
/// "unknown" as "no update", never as an error.
pub async fn remote_image_digest(client: &reqwest::Client, image: &str) -> Option<String> {
    let r = parse_image_ref(image)?;
    let base = format!("https://{}/v2", r.registry);
    let manifest_url = format!("{base}/{}/manifests/{}", r.repository, r.tag);
    let accept = "application/vnd.oci.image.index.v1+json, \
                  application/vnd.docker.distribution.manifest.list.v2+json, \
                  application/vnd.oci.image.manifest.v1+json, \
                  application/vnd.docker.distribution.manifest.v2+json";

    // First try unauthenticated; on 401 follow the bearer challenge.
    let resp = client
        .head(&manifest_url)
        .header("Accept", accept)
        .send()
        .await
        .ok()?;
    let resp = if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let challenge = resp
            .headers()
            .get("www-authenticate")?
            .to_str()
            .ok()?
            .to_string();
        let (realm, service, _) = parse_bearer_challenge(&challenge)?;
        let mut token_url = url::Url::parse(&realm).ok()?;
        token_url
            .query_pairs_mut()
            .append_pair("scope", &format!("repository:{}:pull", r.repository));
        if let Some(svc) = service {
            token_url.query_pairs_mut().append_pair("service", &svc);
        }
        let token_json: serde_json::Value =
            client.get(token_url).send().await.ok()?.json().await.ok()?;
        let token = token_json
            .get("token")
            .or_else(|| token_json.get("access_token"))
            .and_then(|v| v.as_str())?
            .to_string();
        client
            .head(&manifest_url)
            .header("Accept", accept)
            .bearer_auth(token)
            .send()
            .await
            .ok()?
    } else {
        resp
    };

    if !resp.status().is_success() {
        return None;
    }
    resp.headers()
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("HomeVault/AppUpdates")
        .build()
        .unwrap_or_default()
}

/// Check every installed catalog app for image and package updates.
pub async fn check_installed_apps(
    enabled_stores: &[app_catalog::EnabledStore],
) -> Vec<AppUpdateStatus> {
    let apps = app_catalog::list_apps(enabled_stores);
    let client = http_client();
    let mut out = Vec::new();

    for app in &apps {
        let key = app.install_key();
        // Only installed apps have a compose project.
        let Ok(project) = ComposeManager::get_project(&key) else {
            continue;
        };

        let installed_version = read_installed_version(&key);
        let package_update_available = installed_version
            .as_deref()
            .map(|v| version_is_older(v, &app.manifest.version))
            .unwrap_or(false);

        // Images of the project's services.
        let mut images = Vec::new();
        for image in project_images(&project.name) {
            let local = local_image_digest(&image);
            let remote = remote_image_digest(&client, &image).await;
            let update_available = match (&local, &remote) {
                (Some(l), Some(r)) => l != r,
                _ => false,
            };
            images.push(ImageUpdate {
                image,
                local_digest: local,
                remote_digest: remote,
                update_available,
            });
        }
        let image_update_available = images.iter().any(|i| i.update_available);

        if package_update_available || image_update_available {
            out.push(AppUpdateStatus {
                id: app.namespaced_id(),
                name: app.manifest.name.clone(),
                store_id: app.store_id.clone(),
                installed_version,
                available_version: app.manifest.version.clone(),
                package_update_available,
                image_update_available,
                images,
            });
        }
    }
    out
}

/// The `image:` of every service in a compose project, via
/// `docker compose ps`. Run from the project directory (no `-p`), matching
/// how `ComposeManager` creates and starts projects. Short subprocess.
fn project_images(project: &str) -> Vec<String> {
    let dir = crate::config::paths::compose_dir().join(project);
    let out = Command::new("docker")
        .args(["compose", "ps", "-a", "--format", "{{.Image}}"])
        .current_dir(&dir)
        .output();
    let mut images: Vec<String> = out
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    images.sort();
    images.dedup();
    images
}

fn read_installed_version(install_key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(
        crate::config::paths::apps_dir()
            .join(install_key)
            .join(".install.toml"),
    )
    .ok()?;
    let v: toml::Value = raw.parse().ok()?;
    v.get("package_version")?.as_str().map(|s| s.to_string())
}

/// True when `installed` looks strictly older than `available`. Both are
/// compared as dotted numeric components; a non-numeric component falls
/// back to a plain string compare of the whole thing.
pub fn version_is_older(installed: &str, available: &str) -> bool {
    fn parts(s: &str) -> Option<Vec<u64>> {
        s.trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|p| p.parse::<u64>().ok())
            .collect()
    }
    match (parts(installed), parts(available)) {
        (Some(a), Some(b)) => a < b,
        _ => installed != available && installed < available,
    }
}

/// Re-render the template (picks up a newer package version), pull newer
/// images, then recreate the stack. Blocking; wrap in `spawn_blocking`.
pub fn apply_update(app: &CatalogApp, host_port: u16) -> Result<(), String> {
    let key = app.install_key();

    let (rendered, _dir) = app_catalog::render(app, host_port)?;
    ComposeManager::update_project(&key, &rendered).map_err(|e| e.to_string())?;

    let dir = crate::config::paths::compose_dir().join(&key);
    let pull = Command::new("docker")
        .args(["compose", "pull"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("docker compose pull: {e}"))?;
    if !pull.status.success() {
        return Err(format!(
            "docker compose pull failed: {}",
            String::from_utf8_lossy(&pull.stderr).trim()
        ));
    }

    // `up -d` recreates only the services whose image/config changed.
    ComposeManager::start_project(&key).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image_ref_docker_hub() {
        let r = parse_image_ref("nginx").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "library/nginx");
        assert_eq!(r.tag, "latest");

        let r = parse_image_ref("louislam/uptime-kuma:1").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "louislam/uptime-kuma");
        assert_eq!(r.tag, "1");
    }

    #[test]
    fn test_parse_image_ref_other_registries() {
        let r = parse_image_ref("ghcr.io/immich-app/immich-server:release").unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "immich-app/immich-server");
        assert_eq!(r.tag, "release");

        let r = parse_image_ref("lscr.io/linuxserver/bookstack:latest").unwrap();
        assert_eq!(r.registry, "lscr.io");
        assert_eq!(r.repository, "linuxserver/bookstack");

        let r = parse_image_ref("docker.io/library/redis:7@sha256:abc").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repository, "library/redis");
        assert_eq!(r.tag, "7");
    }

    #[test]
    fn test_parse_bearer_challenge() {
        let h = r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/nginx:pull""#;
        let (realm, service, scope) = parse_bearer_challenge(h).unwrap();
        assert_eq!(realm, "https://auth.docker.io/token");
        assert_eq!(service.as_deref(), Some("registry.docker.io"));
        assert_eq!(scope.as_deref(), Some("repository:library/nginx:pull"));

        let (realm, service, _) =
            parse_bearer_challenge(r#"Bearer realm="https://ghcr.io/token""#).unwrap();
        assert_eq!(realm, "https://ghcr.io/token");
        assert_eq!(service, None);
    }

    #[test]
    fn test_version_is_older() {
        assert!(version_is_older("1.0.0", "1.0.1"));
        assert!(version_is_older("1.0.0", "1.1.0"));
        assert!(version_is_older("v1.2.0", "1.2.1"));
        assert!(!version_is_older("1.2.0", "1.2.0"));
        assert!(!version_is_older("2.0.0", "1.9.9"));
    }
}
