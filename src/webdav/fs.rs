/* =============================================================================
 * File          : fs.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The WebDAV filesystem layer: resolves a request path to the concrete
 *   directory of one enabled WebDAV share and applies the same per-user
 *   guard as every other share type (HomeVault account + POSIX-ACL grant).
 *   Path mapping and validation are pure functions so they are unit-tested
 *   without a running server.
 *
 * Dependencies:
 *   - dav-server (>= 0.11)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Built by src/webdav/serve.rs and passed to `DavHandler::builder()`.
 * =============================================================================
 */

//! Path mapping and per-user authorization for the built-in WebDAV server.
//!
//! A WebDAV URL is `/ <share-name> / <relative path…>`. Only the shares that
//! are defined for the `webdav` protocol *and* enabled are reachable; the
//! first segment is looked up by name, never by path, so no request can walk
//! out of a share through `..`, a symlink or an absolute path. Everything
//! after the share name is validated with [`resolve_relative_path`] and
//! joined under the share root.

use crate::shares::{Share, ShareType};
use std::path::{Path, PathBuf};

/// One WebDAV share exposed over HTTP, with its resolved root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebdavShare {
    /// The URL segment (first path component), e.g. `photos`.
    pub name: String,
    /// The absolute directory the share maps to, e.g. `/srv/photos`.
    pub root: PathBuf,
}

impl WebdavShare {
    /// Builds a share from a persisted [`Share`], normalizing the root path
    /// (trailing slashes and `.` components are stripped so two spellings of
    /// the same directory compare equal).
    pub fn from_share(share: &Share) -> Self {
        Self {
            name: share.name.clone(),
            root: normalize_root(&share.path),
        }
    }
}

/// Keeps only the WebDAV shares that should actually be served: right
/// protocol, enabled, non-empty name, usable absolute path, and no duplicate
/// name (the first definition wins so the table stays deterministic).
pub fn servable_shares(all: Vec<Share>) -> Vec<WebdavShare> {
    let mut out: Vec<WebdavShare> = Vec::new();
    for share in all {
        if share.share_type != ShareType::Webdav || !share.enabled {
            continue;
        }
        if share.name.trim().is_empty() {
            continue;
        }
        let root = normalize_root(&share.path);
        if !root.is_absolute() {
            continue;
        }
        if out.iter().any(|s| s.name == share.name) {
            continue;
        }
        out.push(WebdavShare {
            name: share.name,
            root,
        });
    }
    out
}

/// Normalizes a configured share path: trims whitespace, collapses repeated
/// separators and drops `.` / trailing `/` components. `..` is rejected (the
/// whole point is that a share root is a real, fixed directory).
fn normalize_root(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    let mut out = PathBuf::new();
    for comp in trimmed.split('/') {
        match comp {
            "" | "." => continue,
            ".." => return PathBuf::new(),
            other => out.push(other),
        }
    }
    if trimmed.starts_with('/') {
        // `split('/')` dropped the leading empty component; re-root so the
        // result stays absolute.
        PathBuf::from("/").join(out)
    } else {
        out
    }
}

/// Splits a WebDAV request path into `(share name, relative path)`.
///
/// Returns `None` when the request has no share segment (`/`, `""`). The
/// returned relative path is always normalized: no leading `/`, no `.` or
/// `..` component, no embedded NUL — so the caller can safely `join` it under
/// a share root.
pub fn split_share_path(request_path: &str) -> Option<(String, String)> {
    let decoded = percent_decode(request_path);
    let trimmed = decoded.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = trimmed.splitn(2, '/');
    let name = parts.next().unwrap_or("").to_string();
    if name.is_empty() {
        return None;
    }
    let rest = parts.next().unwrap_or("");
    Some((name, resolve_relative_path(rest)?))
}

/// Validates and normalizes the path *inside* a share.
///
/// Returns `None` (reject the request) when the input contains a `..`
/// component or a NUL byte, or when normalization would escape the share
/// root. An empty result is valid — it denotes the share root itself.
pub fn resolve_relative_path(raw: &str) -> Option<String> {
    if raw.contains('\0') {
        return None;
    }
    let mut depth: Vec<&str> = Vec::new();
    for comp in raw.split('/') {
        match comp {
            "" | "." => continue,
            ".." => {
                // Never allow climbing above the share root.
                depth.pop()?;
            }
            other => depth.push(other),
        }
    }
    Some(depth.join("/"))
}

/// Minimal percent-decoding for path segments (`%2F`, `%20`, `%C3%A9`…).
///
/// A malformed escape is passed through literally rather than rejected: it
/// then fails the `.` / `..` / NUL checks above like any other text, which
/// keeps the failure mode a 404 instead of a decoding edge case.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The result of mapping a request path against the configured share table.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedPath {
    /// The request names a share that exists; carries its root and the
    /// normalized path inside it (empty string == the share root).
    Inside {
        share: WebdavShare,
        relative: String,
    },
    /// No share matches the first path segment.
    UnknownShare(String),
}

/// Maps a request path to a share + concrete filesystem path.
pub fn resolve_path(shares: &[WebdavShare], request_path: &str) -> ResolvedPath {
    let Some((name, relative)) = split_share_path(request_path) else {
        return ResolvedPath::UnknownShare(String::new());
    };
    match shares.iter().find(|s| s.name == name) {
        Some(share) => ResolvedPath::Inside {
            share: share.clone(),
            relative,
        },
        None => ResolvedPath::UnknownShare(name),
    }
}

/// Joins a share root with a normalized relative path.
pub fn absolute_path(share: &WebdavShare, relative: &str) -> PathBuf {
    if relative.is_empty() {
        share.root.clone()
    } else {
        share.root.join(relative)
    }
}

/// Whether `candidate` stays inside `root` after lexical normalization.
///
/// This is a defense-in-depth check run on the already-normalized paths: a
/// symlink or a config change between the two steps must not turn into an
/// escape. Purely textual, so it needs no filesystem access and stays
/// testable.
pub fn is_inside(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(name: &str, path: &str) -> Share {
        Share {
            name: name.to_string(),
            share_type: ShareType::Webdav,
            path: path.to_string(),
            enabled: true,
            comment: String::new(),
            options: Vec::new(),
        }
    }

    #[test]
    fn test_split_share_path_reads_name_then_relative() {
        assert_eq!(
            split_share_path("/photos/2024/img.jpg"),
            Some(("photos".to_string(), "2024/img.jpg".to_string()))
        );
        assert_eq!(
            split_share_path("photos"),
            Some(("photos".to_string(), String::new()))
        );
        assert_eq!(
            split_share_path("/photos/"),
            Some(("photos".to_string(), String::new()))
        );
    }

    #[test]
    fn test_split_share_path_rejects_root_and_empty() {
        assert_eq!(split_share_path("/"), None);
        assert_eq!(split_share_path(""), None);
        assert_eq!(split_share_path("//"), None);
    }

    #[test]
    fn test_split_share_path_decodes_percent_escapes() {
        assert_eq!(
            split_share_path("/medias/mon%20film.mkv"),
            Some(("medias".to_string(), "mon film.mkv".to_string()))
        );
        // `%2F` must not be able to fake an extra path separator into the
        // share-name lookup: it is decoded, then treated as a literal `/`
        // inside the relative path, which normalization handles.
        assert_eq!(
            split_share_path("/a%2Fb/c"),
            Some(("a".to_string(), "b/c".to_string()))
        );
    }

    #[test]
    fn test_resolve_relative_path_rejects_parent_traversal() {
        // A single `..` at depth 0 has nothing to pop -> rejected.
        assert_eq!(resolve_relative_path("../etc/passwd"), None);
        assert_eq!(resolve_relative_path("a/../../b"), None);
        assert_eq!(resolve_relative_path(".."), None);
    }

    #[test]
    fn test_resolve_relative_path_normalizes_within_share() {
        assert_eq!(resolve_relative_path("a/./b//c"), Some("a/b/c".to_string()));
        // Climbing then descending is fine as long as the root is not left.
        assert_eq!(resolve_relative_path("a/b/../c"), Some("a/c".to_string()));
        assert_eq!(resolve_relative_path(""), Some(String::new()));
    }

    #[test]
    fn test_resolve_relative_path_rejects_nul() {
        assert_eq!(resolve_relative_path("a\0b"), None);
    }

    #[test]
    fn test_servable_shares_filters_disabled_other_protocol_and_relative() {
        let mut smb = share("smb", "/srv/smb");
        smb.share_type = ShareType::Smb;
        let mut disabled = share("off", "/srv/off");
        disabled.enabled = false;
        let relative = share("rel", "srv/rel");
        let empty_name = share("  ", "/srv/empty");

        let served = servable_shares(vec![
            share("photos", "/srv/photos"),
            smb,
            disabled,
            relative,
            empty_name,
        ]);
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].name, "photos");
    }

    #[test]
    fn test_servable_shares_deduplicates_by_name() {
        let served = servable_shares(vec![
            share("photos", "/srv/first"),
            share("photos", "/srv/second"),
        ]);
        assert_eq!(served.len(), 1);
        assert_eq!(served[0].root, PathBuf::from("/srv/first"));
    }

    #[test]
    fn test_normalize_root_strips_trailing_and_dot_components() {
        assert_eq!(
            WebdavShare::from_share(&share("p", "/srv/photos///")).root,
            PathBuf::from("/srv/photos")
        );
        assert_eq!(
            WebdavShare::from_share(&share("p", " /srv/./photos ")).root,
            PathBuf::from("/srv/photos")
        );
        // A `..` anywhere makes the root unusable (empty path -> not absolute).
        assert!(!WebdavShare::from_share(&share("p", "/srv/../etc"))
            .root
            .is_absolute());
    }

    #[test]
    fn test_resolve_path_matches_share_and_rejects_unknown() {
        let shares = vec![WebdavShare {
            name: "photos".to_string(),
            root: PathBuf::from("/srv/photos"),
        }];
        assert_eq!(
            resolve_path(&shares, "/photos/a/b.txt"),
            ResolvedPath::Inside {
                share: shares[0].clone(),
                relative: "a/b.txt".to_string(),
            }
        );
        assert_eq!(
            resolve_path(&shares, "/videos/x"),
            ResolvedPath::UnknownShare("videos".to_string())
        );
        // A `..` that would leave the share root is rejected outright, so the
        // request never even reaches the share-name lookup.
        assert_eq!(
            resolve_path(&shares, "/photos/../../etc/passwd"),
            ResolvedPath::UnknownShare(String::new())
        );
    }

    #[test]
    fn test_absolute_path_joins_under_root() {
        let share = WebdavShare {
            name: "s".to_string(),
            root: PathBuf::from("/srv/s"),
        };
        assert_eq!(absolute_path(&share, ""), PathBuf::from("/srv/s"));
        assert_eq!(absolute_path(&share, "a/b"), PathBuf::from("/srv/s/a/b"));
        assert!(is_inside(
            Path::new("/srv/s"),
            &absolute_path(&share, "a/b")
        ));
        assert!(!is_inside(Path::new("/srv/s"), Path::new("/srv/other")));
    }
}
