/* =============================================================================
 * File          : ftp.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Version       : 1.0.0
 *
 * Description:
 *   Real per-user FTP roots for the Shares app. vsftpd has no notion of a
 *   "share": it exposes a Linux account's home (or the whole filesystem when
 *   chroot is off). This module builds one chroot jail per grantee, bind-
 *   mounts exactly the FTP shares their ACL grants name, and writes the
 *   matching `user_config_dir` entry so `local_root` is that jail.
 *
 * Dependencies:
 *   - internal storage layer (share/permission repositories)
 *   - `mount`, `umount`, `mkdir`, `rmdir`, `tee` through sudo
 *
 * Usage:
 *   handlers/shares.rs and permissions.rs call `rebuild_jails` after an FTP
 *   share, grant or restriction change.
 * =============================================================================
 */

//! Per-user FTP chroot jails built from the share-permission grants.

use crate::config::paths;
use crate::groups::GroupService;
use crate::permissions::PermissionService;
use crate::shares::{
    validate_share_name, validate_share_path, write_file_as_root, Share, ShareType, SharesService,
};
use crate::utils::process::run_timed;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Hard ceiling for one full FTP jail reconciliation.
pub const FTP_REBUILD_TIMEOUT: Duration = Duration::from_secs(120);

/// Ceiling for one host command.
const FTP_OP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Error, Debug)]
pub enum FtpError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Invalid FTP field: {0}")]
    InvalidField(String),
}

/// One share mounted into a user's jail.
#[derive(Debug, Clone)]
pub struct FtpJailShare {
    pub name: String,
    pub path: String,
    pub writable: bool,
    pub browsable: bool,
    pub downloadable: bool,
}

/// The desired jail content for one user.
#[derive(Debug, Clone)]
pub struct FtpJail {
    pub username: String,
    pub shares: Vec<FtpJailShare>,
}

impl FtpJail {
    /// `write_enable`, `dirlist_enable` and `download_enable` are per-user in
    /// vsftpd. A user with one read-only share must not get write access just
    /// because another of their shares is writable, so the strictest value
    /// wins: a permission is only enabled when *every* mounted share allows
    /// it.
    pub fn write_enable(&self) -> bool {
        !self.shares.is_empty() && self.shares.iter().all(|share| share.writable)
    }

    pub fn dirlist_enable(&self) -> bool {
        !self.shares.is_empty() && self.shares.iter().all(|share| share.browsable)
    }

    pub fn download_enable(&self) -> bool {
        !self.shares.is_empty() && self.shares.iter().all(|share| share.downloadable)
    }

    /// Render the vsftpd per-user configuration. No value can contain a
    /// newline: share names/paths are validated before they reach this file.
    pub fn user_config(&self) -> String {
        format!(
            "local_root={jail}\nchroot_local_user=YES\nallow_writeable_chroot=YES\nwrite_enable={write}\ndirlist_enable={list}\ndownload_enable={download}\n",
            jail = paths::ftp_jails_dir().join(&self.username).display(),
            write = if self.write_enable() { "YES" } else { "NO" },
            list = if self.dirlist_enable() { "YES" } else { "NO" },
            download = if self.download_enable() { "YES" } else { "NO" },
        )
    }
}

pub struct FtpJailService;

impl FtpJailService {
    /// Resolve every enabled FTP share's grantees from `share_permissions`.
    /// The group membership is read from the live OS group so a member added
    /// to a group later inherits the jail without a stored row per person.
    async fn desired_jails(pool: Arc<sqlx::SqlitePool>) -> Result<Vec<FtpJail>, FtpError> {
        let shares = tokio::task::spawn_blocking(SharesService::get_shares)
            .await
            .map_err(|e| FtpError::CommandFailed(format!("share list worker failed: {e}")))?
            .map_err(|e| FtpError::CommandFailed(e.to_string()))?;

        let ftp_shares: Vec<Share> = shares
            .into_iter()
            .filter(|share| share.share_type == ShareType::Ftp && share.enabled)
            .collect();

        let mut desired: BTreeMap<String, Vec<FtpJailShare>> = BTreeMap::new();
        for share in &ftp_shares {
            let permissions =
                PermissionService::list_share_permissions(&share.name, "ftp", Arc::clone(&pool))
                    .await
                    .map_err(|e| FtpError::CommandFailed(e.to_string()))?;

            let mut usernames: HashSet<String> = HashSet::new();
            for permission in &permissions {
                if permission.grantee_type == "user" {
                    usernames.insert(permission.grantee_name.clone());
                } else if permission.grantee_type == "group" {
                    if let Ok(group) = GroupService::get_group(&permission.grantee_name) {
                        usernames.extend(group.members);
                    }
                }
            }

            for username in usernames {
                desired.entry(username).or_default().push(FtpJailShare {
                    name: share.name.clone(),
                    path: share.path.clone(),
                    writable: !share.read_only && !share.restrictions.disable_modification,
                    browsable: !share.restrictions.disable_browsing,
                    downloadable: !share.restrictions.disable_download,
                });
            }
        }

        Ok(desired
            .into_iter()
            .map(|(username, shares)| FtpJail { username, shares })
            .collect())
    }

    /// Reconcile the FTP jails against the current grants.
    pub async fn rebuild_jails(pool: Arc<sqlx::SqlitePool>) -> Result<(), FtpError> {
        let desired = Self::desired_jails(pool).await?;
        tokio::task::spawn_blocking(move || Self::reconcile(&desired))
            .await
            .map_err(|e| FtpError::CommandFailed(format!("FTP jail worker failed: {e}")))?
    }

    fn run(cmd: &mut Command) -> Result<std::process::Output, FtpError> {
        run_timed(cmd, FTP_OP_TIMEOUT).map_err(FtpError::CommandFailed)
    }

    fn mkdir_as_root(path: &str) -> Result<(), FtpError> {
        let output = Self::run(Command::new("sudo").args(["mkdir", "-p", path]))?;
        if !output.status.success() {
            return Err(FtpError::CommandFailed(format!(
                "mkdir -p {path} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    fn remove_dir(path: &str) {
        let _ = Self::run(Command::new("sudo").args(["umount", path]));
        let _ = Self::run(Command::new("sudo").args(["rmdir", path]));
    }

    fn ensure_account(username: &str) -> Result<(), FtpError> {
        GroupService::ensure_linux_user(username)
            .map_err(|e| FtpError::CommandFailed(e.to_string()))
    }

    fn ensure_bind_mount(share_path: &str, mountpoint: &str) -> Result<(), FtpError> {
        validate_share_path(share_path)
            .map_err(|e| FtpError::InvalidField(format!("share path: {e}")))?;
        validate_share_path(mountpoint)
            .map_err(|e| FtpError::InvalidField(format!("mount point: {e}")))?;
        Self::mkdir_as_root(mountpoint)?;
        let already = Self::run(Command::new("mountpoint").args(["-q", mountpoint]))
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !already {
            let output =
                Self::run(Command::new("sudo").args(["mount", "--bind", share_path, mountpoint]))?;
            if !output.status.success() {
                return Err(FtpError::CommandFailed(format!(
                    "mount --bind '{share_path}' -> '{mountpoint}' failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }
        Ok(())
    }

    fn reconcile(desired: &[FtpJail]) -> Result<(), FtpError> {
        Self::mkdir_as_root(&paths::ftp_jails_dir().to_string_lossy())?;
        Self::mkdir_as_root(&paths::ftp_user_conf_dir().to_string_lossy())?;

        let desired_users: HashSet<&str> =
            desired.iter().map(|jail| jail.username.as_str()).collect();

        // Remove jails and configs for users that no longer hold an FTP grant.
        if let Ok(entries) = fs::read_dir(paths::ftp_jails_dir()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if desired_users.contains(name.as_str()) {
                    continue;
                }
                if let Ok(children) = fs::read_dir(entry.path()) {
                    for child in children.flatten() {
                        Self::remove_dir(&child.path().to_string_lossy());
                    }
                }
                Self::remove_dir(&entry.path().to_string_lossy());
                let _ = Self::run(Command::new("sudo").args([
                    "rm",
                    "-f",
                    &paths::ftp_user_conf_dir().join(&name).to_string_lossy(),
                ]));
            }
        }

        for jail in desired {
            validate_share_name(&jail.username)
                .map_err(|e| FtpError::InvalidField(format!("username: {e}")))?;
            Self::ensure_account(&jail.username)?;
            let root = paths::ftp_jails_dir().join(&jail.username);
            Self::mkdir_as_root(&root.to_string_lossy())?;

            let desired_mounts: HashSet<String> =
                jail.shares.iter().map(|share| share.name.clone()).collect();
            if let Ok(entries) = fs::read_dir(&root) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !desired_mounts.contains(&name) {
                        Self::remove_dir(&entry.path().to_string_lossy());
                    }
                }
            }

            for share in &jail.shares {
                validate_share_name(&share.name)
                    .map_err(|e| FtpError::InvalidField(format!("share name: {e}")))?;
                let mountpoint = root.join(&share.name);
                Self::ensure_bind_mount(&share.path, &mountpoint.to_string_lossy())?;
            }

            let conf = paths::ftp_user_conf_dir().join(&jail.username);
            write_file_as_root(&conf.to_string_lossy(), &jail.user_config())?;
        }

        Ok(())
    }

    /// Remove everything this feature provisioned for one account. Called by
    /// the user-delete path so a recreated same-name account cannot inherit a
    /// stale jail.
    pub fn purge_user(username: &str) {
        if validate_share_name(username).is_err() {
            return;
        }
        let root = paths::ftp_jails_dir().join(username);
        if !root.exists() && !paths::ftp_user_conf_dir().join(username).exists() {
            return;
        }
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                Self::remove_dir(&entry.path().to_string_lossy());
            }
        }
        Self::remove_dir(&root.to_string_lossy());
        let _ = Self::run(Command::new("sudo").args([
            "rm",
            "-f",
            &paths::ftp_user_conf_dir().join(username).to_string_lossy(),
        ]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jail(shares: Vec<FtpJailShare>) -> FtpJail {
        FtpJail {
            username: "alice".to_string(),
            shares,
        }
    }

    fn share(writable: bool, browsable: bool, downloadable: bool) -> FtpJailShare {
        FtpJailShare {
            name: "media".to_string(),
            path: "/srv/media".to_string(),
            writable,
            browsable,
            downloadable,
        }
    }

    #[test]
    fn test_permissions_are_the_strictest_of_every_mounted_share() {
        let mixed = jail(vec![share(true, true, true), share(false, true, true)]);
        assert!(!mixed.write_enable());
        assert!(mixed.dirlist_enable());
        assert!(mixed.download_enable());

        let none = jail(vec![share(false, false, false)]);
        assert!(!none.write_enable());
        assert!(!none.dirlist_enable());
        assert!(!none.download_enable());
    }

    #[test]
    fn test_user_config_renders_the_jail_root_and_switches() {
        let text = jail(vec![share(true, false, true)]).user_config();
        assert!(text.contains("chroot_local_user=YES\n"), "{text}");
        assert!(text.contains("allow_writeable_chroot=YES\n"), "{text}");
        assert!(text.contains("write_enable=YES\n"), "{text}");
        assert!(text.contains("dirlist_enable=NO\n"), "{text}");
        assert!(text.contains("download_enable=YES\n"), "{text}");
        assert!(
            text.contains(&format!(
                "local_root={}\n",
                paths::ftp_jails_dir().join("alice").display()
            )),
            "{text}"
        );
    }

    #[test]
    fn test_purge_rejects_path_fragments() {
        // No panic and no command for an invalid name.
        FtpJailService::purge_user("../../etc");
    }
}
