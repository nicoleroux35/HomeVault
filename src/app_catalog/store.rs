/* =============================================================================
 * File          : store.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Community app stores: extra git repositories the admin adds as app
 *   sources, cloned locally and re-synced on demand. A store mirrors the
 *   built-in catalog shape (store manifest + one directory per app).
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   app_catalog/mod.rs merges store apps into the catalog; the store CRUD
 *   endpoints live in handlers/docker_catalog.rs.
 * =============================================================================
 */

//! Community app stores: extra git repositories the admin adds as sources
//! of app packages. A store repo has the same shape as the built-in
//! `catalog/` dir - a `homevault-store.toml` at the root (`id`, `name`)
//! plus one directory per app, each holding `homevault-app.toml` and
//! `docker-compose.yml`.
//!
//! Repos are cloned/updated into `apps_dir()/stores/<store-id>/`. `git` is
//! shelled out to (already a required host dependency elsewhere); all calls
//! are blocking and must be wrapped in `spawn_blocking` by the caller.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `apps_dir()/stores` - the parent of every synced community store.
pub fn stores_root() -> PathBuf {
    crate::config::paths::apps_dir().join("stores")
}

/// On-disk location of one store's checkout.
pub fn store_dir(store_id: &str) -> PathBuf {
    stores_root().join(store_id)
}

/// Root descriptor of a store repo.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct StoreDescriptor {
    pub id: String,
    pub name: String,
}

/// Derive a stable, filesystem-safe store id from a git URL - the last path
/// segment, minus a trailing `.git`, lowercased, non-alnum collapsed to `-`.
/// `https://github.com/acme/cool-apps.git` -> `cool-apps`.
pub fn slug_from_url(url: &str) -> String {
    let last = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(url)
        .trim_end_matches(".git");
    let slug: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "store".to_string()
    } else {
        slug
    }
}

/// Reject anything that isn't a plain `http(s)://` or `git://` URL, or an
/// `ssh`/`scp`-style remote - keeps a store URL from being, say, a local
/// `file://` path or a `--upload-pack` option smuggled in.
pub fn validate_git_url(url: &str) -> Result<(), String> {
    let ok =
        (url.starts_with("https://") || url.starts_with("http://") || url.starts_with("git://"))
            && !url.contains(char::is_whitespace)
            && !url.starts_with("http://-")
            && !url.starts_with("https://-");
    if ok {
        Ok(())
    } else {
        Err("store URL must be a plain http(s):// or git:// git repository URL".to_string())
    }
}

/// Clone the repo if `dest` doesn't exist yet, otherwise fast-forward it.
/// Returns the parsed root descriptor. Blocking.
pub fn sync(git_url: &str, dest: &Path) -> Result<StoreDescriptor, String> {
    validate_git_url(git_url)?;
    std::fs::create_dir_all(stores_root()).map_err(|e| e.to_string())?;

    let git_dir = dest.join(".git");
    let output = if git_dir.is_dir() {
        Command::new("git")
            .args(["-C"])
            .arg(dest)
            .args(["pull", "--ff-only", "--depth", "1"])
            .output()
    } else {
        // A stale non-git directory would make `clone` fail - clear it.
        if dest.exists() {
            let _ = std::fs::remove_dir_all(dest);
        }
        Command::new("git")
            .args(["clone", "--depth", "1", git_url])
            .arg(dest)
            .output()
    }
    .map_err(|e| format!("failed to run git: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    read_descriptor(dest)
}

/// Parse `<dir>/homevault-store.toml`.
pub fn read_descriptor(dir: &Path) -> Result<StoreDescriptor, String> {
    let path = dir.join("homevault-store.toml");
    let raw = std::fs::read_to_string(&path)
        .map_err(|_| "store repo is missing a homevault-store.toml at its root".to_string())?;
    let desc: StoreDescriptor =
        toml::from_str(&raw).map_err(|e| format!("invalid homevault-store.toml: {e}"))?;
    if desc.id.is_empty() {
        return Err("homevault-store.toml has an empty id".to_string());
    }
    Ok(desc)
}

/// List the app-package directories inside a store checkout (any immediate
/// subdirectory that contains a `homevault-app.toml`).
pub fn app_dirs(store_checkout: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(store_checkout) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("homevault-app.toml").is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slug_from_url() {
        assert_eq!(
            slug_from_url("https://github.com/acme/cool-apps.git"),
            "cool-apps"
        );
        assert_eq!(
            slug_from_url("https://git.example.com/x/MyStore/"),
            "mystore"
        );
        assert_eq!(slug_from_url("https://host/a/b/c_d"), "c-d");
    }

    #[test]
    fn test_validate_git_url() {
        assert!(validate_git_url("https://github.com/a/b.git").is_ok());
        assert!(validate_git_url("git://host/a/b").is_ok());
        assert!(validate_git_url("file:///etc/passwd").is_err());
        assert!(validate_git_url("https://host/a b").is_err());
        assert!(validate_git_url("ext::sh -c whoami").is_err());
    }

    #[test]
    fn test_app_dirs_and_descriptor() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("homevault-store.toml"),
            "id = \"demo\"\nname = \"Demo Store\"\n",
        )
        .unwrap();
        let app = tmp.path().join("thing");
        std::fs::create_dir(&app).unwrap();
        std::fs::write(app.join("homevault-app.toml"), "x").unwrap();
        std::fs::create_dir(tmp.path().join("not-an-app")).unwrap();

        let desc = read_descriptor(tmp.path()).unwrap();
        assert_eq!(desc.id, "demo");
        let dirs = app_dirs(tmp.path());
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("thing"));
    }
}
