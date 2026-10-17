/* =============================================================================
 * File          : vfs.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   `WebdavFs` — the dav-server filesystem for the built-in WebDAV server.
 *   It maps the first URL segment to an enabled WebDAV share, checks the
 *   caller's per-member grant, then delegates every operation to a `LocalFs`
 *   rooted at that share. Because each request gets its own LocalFs built
 *   from the validated share root, a `..` or a symlink can never reach
 *   outside the share the URL named, and a cross-share COPY/MOVE is refused.
 *
 * Dependencies:
 *   - dav-server (>= 0.11)
 *
 * Usage:
 *   Built (then rebuilt on every WebDAV config/share change) by
 *   src/webdav/mod.rs and handed to `DavHandler::builder().filesystem(...)`.
 * ============================================================================= */

//! Share-scoped, per-member-authorised filesystem on top of
//! `dav_server::localfs::LocalFs`.
//!
//! Every method first resolves the request path against the configured share
//! table and the caller's grants ([`WebdavCreds`]). An unknown share is a 404;
//! a share the member does not hold is a 403; anything inside an allowed share
//! is rebuilt as `/<relative>` and handed to a `LocalFs` based at that share's
//! root. `DavPath::new` re-normalizes the rebuilt path, so the library's own
//! traversal guard runs a second time on top of ours.

use crate::webdav::fs::{absolute_path, resolve_path, split_share_path, ResolvedPath, WebdavShare};
use dav_server::davpath::DavPath;
use dav_server::fs::{
    DavDirEntry, DavFile, DavMetaData, FsError, FsFuture, FsStream, GuardedFileSystem, OpenOptions,
    ReadDirMeta,
};
use dav_server::localfs::LocalFs;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use std::collections::HashSet;
use std::sync::Arc;

/// Everything except RFC 3986 unreserved characters and the path separator.
/// Matches the set `dav_server::davpath` uses internally, so a relative path
/// round-trips through `DavPath::new` unchanged.
const PATH_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// The authenticated caller, resolved once per request by `serve.rs`.
///
/// `granted` holds the names of the WebDAV shares the account may use. An
/// admin bypasses the set entirely (the appliance owner sees every share,
/// matching the role model everywhere else).
#[derive(Clone, Debug)]
pub struct WebdavCreds {
    pub username: String,
    pub is_admin: bool,
    pub granted: Arc<HashSet<String>>,
}

impl WebdavCreds {
    /// Whether this caller may reach `share`.
    pub fn can_access(&self, share: &str) -> bool {
        self.is_admin || self.granted.contains(share)
    }
}

/// A WebDAV filesystem restricted to the enabled WebDAV shares and the
/// caller's per-member grants.
#[derive(Clone)]
pub struct WebdavFs {
    shares: Arc<Vec<WebdavShare>>,
}

impl WebdavFs {
    pub fn new(shares: Vec<WebdavShare>) -> Self {
        Self {
            shares: Arc::new(shares),
        }
    }

    /// Snapshot of the shares this filesystem serves.
    pub fn shares(&self) -> &[WebdavShare] {
        &self.shares
    }

    /// Resolves a request path to the `LocalFs` of the named share plus the
    /// path relative to that share root, enforcing the caller's grant.
    ///
    /// `Err(NotFound)` for an unknown share, `Err(Forbidden)` for a share the
    /// member does not hold or a path that would escape it.
    fn local_for(
        &self,
        path: &DavPath,
        creds: &WebdavCreds,
    ) -> Result<(Box<LocalFs>, String), FsError> {
        let request = path.as_url_string();
        match resolve_path(&self.shares, &request) {
            ResolvedPath::UnknownShare(_) => Err(FsError::NotFound),
            ResolvedPath::Inside { share, relative } => {
                if !creds.can_access(&share.name) {
                    return Err(FsError::Forbidden);
                }
                let absolute = absolute_path(&share, &relative);
                if !crate::webdav::fs::is_inside(&share.root, &absolute) {
                    return Err(FsError::Forbidden);
                }
                Ok((LocalFs::new(&share.root, false, false, false), relative))
            }
        }
    }
}

/// Rebuilds a `DavPath` from a share-relative path (`"" == share root`).
///
/// The relative path is already percent-decoded, so it is re-encoded here:
/// `DavPath::new` rejects any raw byte outside printable ASCII, which would
/// otherwise send a name like `résumé.pdf` back to the share root.
fn davpath_for(relative: &str) -> DavPath {
    let url = if relative.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", utf8_percent_encode(relative, PATH_ENCODE_SET))
    };
    DavPath::new(&url).unwrap_or_else(|_| DavPath::new("/").expect("root path is valid"))
}

/// The share root a request path belongs to, if any.
fn share_root_of(shares: &[WebdavShare], path: &DavPath) -> Option<std::path::PathBuf> {
    match resolve_path(shares, &path.as_url_string()) {
        ResolvedPath::Inside { share, .. } => Some(share.root),
        ResolvedPath::UnknownShare(_) => None,
    }
}

/// The share name at the top of a request path, exposed for logging.
pub fn top_share(path: &DavPath) -> Option<String> {
    split_share_path(&path.as_url_string()).map(|(name, _)| name)
}

impl GuardedFileSystem<WebdavCreds> for WebdavFs {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.open(&davpath_for(&relative), options, &()).await
        })
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        meta: ReadDirMeta,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.read_dir(&davpath_for(&relative), meta, &()).await
        })
    }

    fn metadata<'a>(
        &'a self,
        path: &'a DavPath,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.metadata(&davpath_for(&relative), &()).await
        })
    }

    fn symlink_metadata<'a>(
        &'a self,
        path: &'a DavPath,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, Box<dyn DavMetaData>> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.symlink_metadata(&davpath_for(&relative), &()).await
        })
    }

    fn create_dir<'a>(&'a self, path: &'a DavPath, creds: &'a WebdavCreds) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.create_dir(&davpath_for(&relative), &()).await
        })
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath, creds: &'a WebdavCreds) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.remove_dir(&davpath_for(&relative), &()).await
        })
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath, creds: &'a WebdavCreds) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.remove_file(&davpath_for(&relative), &()).await
        })
    }

    fn rename<'a>(
        &'a self,
        from: &'a DavPath,
        to: &'a DavPath,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            // A cross-share MOVE cannot be one `rename`: resolve both sides
            // (each enforcing the grant) and refuse when they differ.
            let (from_fs, from_rel) = self.local_for(from, creds)?;
            let (_, to_rel) = self.local_for(to, creds)?;
            if share_root_of(&self.shares, from) != share_root_of(&self.shares, to) {
                return Err(FsError::Forbidden);
            }
            from_fs
                .rename(&davpath_for(&from_rel), &davpath_for(&to_rel), &())
                .await
        })
    }

    fn copy<'a>(
        &'a self,
        from: &'a DavPath,
        to: &'a DavPath,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let (from_fs, from_rel) = self.local_for(from, creds)?;
            let (_, to_rel) = self.local_for(to, creds)?;
            if share_root_of(&self.shares, from) != share_root_of(&self.shares, to) {
                return Err(FsError::Forbidden);
            }
            from_fs
                .copy(&davpath_for(&from_rel), &davpath_for(&to_rel), &())
                .await
        })
    }

    fn set_accessed<'a>(
        &'a self,
        path: &'a DavPath,
        tm: std::time::SystemTime,
        creds: &WebdavCreds,
    ) -> FsFuture<'a, ()> {
        // The trait ties this future to `'a` but leaves `credentials` with an
        // anonymous lifetime, so the resolution (the only step that reads
        // `creds`) happens eagerly and the future owns its result.
        let resolved = self.local_for(path, creds);
        Box::pin(async move {
            let (fs, relative) = resolved?;
            fs.set_accessed(&davpath_for(&relative), tm, &()).await
        })
    }

    fn set_modified<'a>(
        &'a self,
        path: &'a DavPath,
        tm: std::time::SystemTime,
        creds: &'a WebdavCreds,
    ) -> FsFuture<'a, ()> {
        Box::pin(async move {
            let (fs, relative) = self.local_for(path, creds)?;
            fs.set_modified(&davpath_for(&relative), tm, &()).await
        })
    }

    fn get_quota<'a>(&'a self, _creds: &WebdavCreds) -> FsFuture<'a, (u64, Option<u64>)> {
        Box::pin(async move { Ok((0, None)) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::{Share, ShareType};

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

    fn creds(admin: bool, granted: &[&str]) -> WebdavCreds {
        WebdavCreds {
            username: "alice".to_string(),
            is_admin: admin,
            granted: Arc::new(granted.iter().map(|s| s.to_string()).collect()),
        }
    }

    /// Builds a filesystem with one on-disk directory per named share.
    fn fs_with(shares: &[&str]) -> (WebdavFs, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut list = Vec::new();
        for name in shares {
            let root = dir.path().join(name);
            std::fs::create_dir_all(&root).expect("create share root");
            list.push(Share {
                path: root.to_string_lossy().into_owned(),
                ..share(name, "")
            });
        }
        let served = crate::webdav::fs::servable_shares(list);
        (WebdavFs::new(served), dir)
    }

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Runtime::new().unwrap().block_on(fut)
    }

    #[test]
    fn test_unknown_share_is_not_found() {
        let (fs, _dir) = fs_with(&["photos"]);
        let path = DavPath::new("/videos/x").unwrap();
        assert!(matches!(
            fs.local_for(&path, &creds(false, &["photos"])),
            Err(FsError::NotFound)
        ));
    }

    #[test]
    fn test_share_without_grant_is_forbidden() {
        let (fs, _dir) = fs_with(&["photos", "videos"]);
        let path = DavPath::new("/videos/x").unwrap();
        assert!(matches!(
            fs.local_for(&path, &creds(false, &["photos"])),
            Err(FsError::Forbidden)
        ));
    }

    #[test]
    fn test_admin_reaches_any_share() {
        let (fs, _dir) = fs_with(&["photos", "videos"]);
        let path = DavPath::new("/videos/").unwrap();
        assert!(fs.local_for(&path, &creds(true, &[])).is_ok());
    }

    #[test]
    fn test_davpath_for_round_trips_a_non_ascii_name() {
        // The re-encoded path must decode back to exactly the input, not to
        // the share root (the bug when `DavPath::new` gets raw UTF-8).
        let rebuilt = davpath_for("dossiers/résumé 2024.pdf");
        assert_eq!(
            rebuilt.as_url_string(),
            "/dossiers/r%C3%A9sum%C3%A9%202024.pdf"
        );
        assert_eq!(
            String::from_utf8_lossy(rebuilt.as_bytes()),
            "/dossiers/résumé 2024.pdf"
        );
        assert_eq!(davpath_for("").as_url_string(), "/");
    }

    #[test]
    fn test_read_dir_lists_a_known_share() {
        let (fs, dir) = fs_with(&["photos"]);
        std::fs::write(dir.path().join("photos/a.txt"), b"hi").unwrap();

        let names = block_on(async {
            use futures_util::StreamExt;
            let path = DavPath::new("/photos/").unwrap();
            let mut stream = fs
                .read_dir(&path, ReadDirMeta::None, &creds(false, &["photos"]))
                .await
                .unwrap();
            let mut names = Vec::new();
            while let Some(entry) = stream.next().await {
                names.push(String::from_utf8_lossy(&entry.unwrap().name()).into_owned());
            }
            names
        });
        assert!(names.contains(&"a.txt".to_string()));
    }

    #[test]
    fn test_read_dir_without_grant_is_forbidden() {
        let (fs, _dir) = fs_with(&["photos"]);
        let path = DavPath::new("/photos/").unwrap();
        let result = block_on(fs.read_dir(&path, ReadDirMeta::None, &creds(false, &[])));
        assert!(matches!(result, Err(FsError::Forbidden)));
    }

    #[test]
    fn test_metadata_of_share_root_is_a_directory() {
        let (fs, _dir) = fs_with(&["photos"]);
        let is_dir = block_on(async {
            let path = DavPath::new("/photos/").unwrap();
            fs.metadata(&path, &creds(false, &["photos"]))
                .await
                .unwrap()
                .is_dir()
        });
        assert!(is_dir);
    }

    #[test]
    fn test_open_reads_file_content_inside_share() {
        let (fs, dir) = fs_with(&["photos"]);
        std::fs::write(dir.path().join("photos/a.txt"), b"hello").unwrap();
        let content = block_on(async {
            let path = DavPath::new("/photos/a.txt").unwrap();
            let open = OpenOptions {
                read: true,
                ..OpenOptions::default()
            };
            let mut file = fs
                .open(&path, open, &creds(false, &["photos"]))
                .await
                .unwrap();
            file.read_bytes(1024).await.unwrap()
        });
        assert_eq!(&content[..], b"hello");
    }

    #[test]
    fn test_cross_share_rename_is_forbidden() {
        let (fs, dir) = fs_with(&["photos", "videos"]);
        std::fs::write(dir.path().join("photos/a.txt"), b"x").unwrap();
        let result = block_on(async {
            let from = DavPath::new("/photos/a.txt").unwrap();
            let to = DavPath::new("/videos/a.txt").unwrap();
            let c = creds(true, &[]);
            fs.rename(&from, &to, &c).await
        });
        assert!(matches!(result, Err(FsError::Forbidden)));
    }

    #[test]
    fn test_top_share_extracts_first_segment() {
        assert_eq!(
            top_share(&DavPath::new("/photos/2024/a.jpg").unwrap()),
            Some("photos".to_string())
        );
        assert_eq!(top_share(&DavPath::new("/").unwrap()), None);
    }

    #[test]
    fn test_share_root_of_reports_the_matching_root() {
        let (fs, dir) = fs_with(&["photos"]);
        let path = DavPath::new("/photos/x").unwrap();
        assert_eq!(
            share_root_of(fs.shares(), &path),
            Some(dir.path().join("photos"))
        );
    }
}
