/* =============================================================================
 * File          : sftp.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Real SFTP support for the Shares app: because SFTP is an SSH subsystem
 *   rather than a standalone daemon, this runs a second dedicated sshd on
 *   its own port, with per-user chroot jails (bind mounts of the granted
 *   shares) and authorized-keys management — rebuilt whenever share or key
 *   state changes.
 *
 * Dependencies:
 *   - internal storage layer (share/key repositories)
 *
 * Usage:
 *   handlers/shares.rs rebuilds jails best-effort after SFTP-affecting
 *   changes (see docs/guides/shares.md).
 * =============================================================================
 */

//! Real SFTP (SSH File Transfer Protocol) support for the Shares app.
//!
//! Unlike SMB/NFS/FTP/WebDAV, SFTP is not a standalone protocol daemon —
//! it's an SSH subsystem. Serving it correctly means running a *second*,
//! dedicated `sshd` instance (its own config file, its own port, its own
//! systemd unit) rather than touching the host's real system sshd, since
//! that would risk affecting a real admin's own shell/SSH access to the
//! box. Each HomeVault user granted SFTP access to a share gets a real
//! (but shell-less, key-only) Linux account, chrooted via OpenSSH's
//! `ChrootDirectory` into a per-user jail that bind-mounts exactly the
//! shares they've been granted — reusing the existing, already-audited
//! per-share ACL permission system (`crate::permissions::PermissionService`)
//! rather than inventing a second, parallel access-control list.
//!
//! Access changes (`PermissionService::grant_permission`/`revoke_permission`,
//! in the separately frozen `src/handlers/permissions.rs`) are not hooked
//! automatically — `rebuild_jails` is called after SFTP share/config
//! changes and via an explicit "Refresh SFTP Access" action, so this module
//! never has to modify that frozen file.

use crate::config::paths;
use crate::groups::GroupService;
use crate::permissions::PermissionService;
use crate::shares::{
    append_file_as_root, write_file_as_root, SftpConfig, Share, ShareType, SharesService,
};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::process::Command;
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;

#[derive(Error, Debug)]
pub enum SftpError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Invalid SSH public key")]
    InvalidKey,
}

const SFTP_GROUP: &str = "homevault-sftp";
const SFTP_SERVICE: &str = "homevault-sftp";
const SFTP_CONFIG_DIR: &str = "/etc/homevault-sftp";
const SFTP_CONFIG_FILE: &str = "/etc/homevault-sftp/sshd_config";
const SFTP_SERVICE_FILE: &str = "/etc/systemd/system/homevault-sftp.service";
const FSTAB_PATH: &str = "/etc/fstab";
const FSTAB_TAG_PREFIX: &str = "# homevault-sftp:";

/// A user with SFTP access to at least one share, for the "SFTP Users" key
/// management panel.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SftpGrantee {
    pub username: String,
    pub shares: Vec<String>,
    pub has_key: bool,
}

pub struct SftpJailService;

impl SftpJailService {
    fn ensure_group() -> Result<(), SftpError> {
        let check = Command::new("getent")
            .args(["group", SFTP_GROUP])
            .output()?;
        if check.status.success() {
            return Ok(());
        }
        let output = Command::new("sudo")
            .args(["groupadd", SFTP_GROUP])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("already exists") {
                return Err(SftpError::CommandFailed(format!(
                    "groupadd failed: {}",
                    stderr
                )));
            }
        }
        Ok(())
    }

    /// Ensure `username` exists as a real, shell-less Linux account and is
    /// a member of `homevault-sftp`. Regular HomeVault users are DB-only
    /// until now — see `src/storage/database.rs::sync_admin_user_to_group`
    /// for the only other place this codebase creates a Linux account. If
    /// the account already exists (an admin, or a user with a home folder),
    /// this never touches its shell or home directory, only group
    /// membership — same conservative approach `sync_admin_user_to_group`
    /// already takes.
    fn ensure_linux_user(username: &str) -> Result<(), SftpError> {
        let exists = Command::new("id")
            .arg(username)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !exists {
            let output = Command::new("sudo")
                .args([
                    "useradd",
                    "-M",
                    "-s",
                    "/usr/sbin/nologin",
                    "-G",
                    SFTP_GROUP,
                    username,
                ])
                .output()?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(SftpError::CommandFailed(format!(
                    "useradd failed for '{}': {}",
                    username, stderr
                )));
            }

            // `useradd` leaves a freshly created account's password field
            // locked (`!` in /etc/shadow). Confirmed live: OpenSSH rejects a
            // locked account outright — "User %s not allowed because account
            // is locked" — even with PasswordAuthentication no and a valid
            // key, since this check happens in sshd itself (not PAM, which
            // is already disabled via UsePAM no in this dedicated instance's
            // config). Set a random, never-revealed password hash purely to
            // clear the locked flag; PasswordAuthentication no means it can
            // never actually be used to log in, so it doesn't need to be
            // retrievable or memorable — it's discarded immediately.
            Self::unlock_password(username)?;
            return Ok(());
        }

        let output = Command::new("sudo")
            .args(["usermod", "-a", "-G", SFTP_GROUP, username])
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SftpError::CommandFailed(format!(
                "usermod failed for '{}': {}",
                username, stderr
            )));
        }
        Ok(())
    }

    /// Clear a freshly created account's locked-password flag by piping a
    /// random, immediately-discarded password to `chpasswd` — never stored,
    /// never logged, never returned. Only ever called right after `useradd`
    /// on an account this module just created; an account that already
    /// existed before this feature touched it (e.g. a real admin's account)
    /// must never have its password field touched here.
    fn unlock_password(username: &str) -> Result<(), SftpError> {
        let mut random_bytes = [0u8; 32];
        rand::Rng::fill_bytes(&mut rand::rng(), &mut random_bytes);
        let random_password = random_bytes
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        let mut child = Command::new("sudo")
            .arg("chpasswd")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{}:{}\n", username, random_password).as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(SftpError::CommandFailed(format!(
                "chpasswd failed to unlock '{}': {}",
                username,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    fn mkdir_as_root(path: &str) -> Result<(), SftpError> {
        let output = Command::new("sudo").args(["mkdir", "-p", path]).output()?;
        if !output.status.success() {
            return Err(SftpError::CommandFailed(format!(
                "mkdir -p {} failed: {}",
                path,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    /// Loosely validate a pasted SSH public key: single line, recognized
    /// key-type prefix, at least a type + key-material field. Rejects
    /// embedded newlines so a pasted value can never inject a second
    /// `authorized_keys` entry or option.
    fn validate_public_key(key: &str) -> Result<String, SftpError> {
        let trimmed = key.trim();
        if trimmed.contains('\n') || trimmed.contains('\r') || trimmed.is_empty() {
            return Err(SftpError::InvalidKey);
        }
        let valid_prefix = trimmed.starts_with("ssh-rsa ")
            || trimmed.starts_with("ssh-ed25519 ")
            || trimmed.starts_with("ecdsa-sha2-")
            || trimmed.starts_with("sk-ssh-ed25519")
            || trimmed.starts_with("sk-ecdsa-sha2-");
        if !valid_prefix || trimmed.split_whitespace().count() < 2 {
            return Err(SftpError::InvalidKey);
        }
        Ok(trimmed.to_string())
    }

    /// Set (overwrite) a user's SFTP public key, ensuring their Linux
    /// account and group membership exist first. The key is stored outside
    /// any chroot jail (`sftp_keys_dir()`), since OpenSSH resolves
    /// `AuthorizedKeysFile` against the real filesystem before chrooting.
    pub fn set_authorized_key(username: &str, public_key: &str) -> Result<(), SftpError> {
        let key = Self::validate_public_key(public_key)?;
        Self::ensure_group()?;
        Self::ensure_linux_user(username)?;

        let dir = paths::sftp_keys_dir().join(username);
        Self::mkdir_as_root(&dir.to_string_lossy())?;

        let file = dir.join("authorized_keys");
        write_file_as_root(&file.to_string_lossy(), &format!("{}\n", key))?;
        // 644, not 600: confirmed live that OpenSSH's unprivileged pre-auth
        // worker (the `sshd` system account, not root) opens this file
        // directly before authentication completes, so it must itself be
        // able to read it — "Could not open ... Permission denied" with 600.
        // This is safe: an SSH *public* key is not a secret (the private
        // key never leaves the client), unlike the WireGuard/OpenVPN
        // key/credential files elsewhere in this codebase that are
        // deliberately 0600 because they hold real secrets.
        let _ = Command::new("sudo")
            .args(["chmod", "644", &file.to_string_lossy()])
            .output();
        Ok(())
    }

    fn detect_host_keys() -> Vec<String> {
        [
            "/etc/ssh/ssh_host_ed25519_key",
            "/etc/ssh/ssh_host_rsa_key",
            "/etc/ssh/ssh_host_ecdsa_key",
        ]
        .iter()
        .filter(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
        .collect()
    }

    /// Regenerate the dedicated SFTP sshd instance's config file and
    /// systemd unit. Only restarts the service if it was already running —
    /// this applies config, it doesn't decide whether SFTP should be on.
    pub fn apply_config(config: &SftpConfig) -> Result<(), SftpError> {
        Self::ensure_group()?;
        Self::mkdir_as_root(SFTP_CONFIG_DIR)?;
        Self::mkdir_as_root(&paths::sftp_jails_dir().to_string_lossy())?;
        Self::mkdir_as_root(&paths::sftp_keys_dir().to_string_lossy())?;

        let host_keys = Self::detect_host_keys();
        if host_keys.is_empty() {
            return Err(SftpError::CommandFailed(
                "No SSH host keys found under /etc/ssh — is openssh-server installed?".to_string(),
            ));
        }
        let host_key_lines: String = host_keys
            .iter()
            .map(|k| format!("HostKey {}\n", k))
            .collect();

        let sshd_config = format!(
            r#"Port {port}
ListenAddress 0.0.0.0
Protocol 2
{host_keys}PidFile /run/homevault-sftp.pid
PermitRootLogin no
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
UsePAM no
AuthorizedKeysFile {keys_dir}/%u/authorized_keys
AllowGroups {group}
X11Forwarding no
AllowTcpForwarding no
AllowAgentForwarding no
PermitTunnel no
ChrootDirectory {jails_dir}/%u
ForceCommand internal-sftp
Subsystem sftp internal-sftp
"#,
            port = config.port,
            host_keys = host_key_lines,
            keys_dir = paths::sftp_keys_dir().display(),
            group = SFTP_GROUP,
            jails_dir = paths::sftp_jails_dir().display(),
        );
        write_file_as_root(SFTP_CONFIG_FILE, &sshd_config)?;

        let service = format!(
            "[Unit]\nDescription=HomeVault SFTP (dedicated, chrooted, key-auth only)\nAfter=network.target\n\n[Service]\nType=simple\nExecStart=/usr/sbin/sshd -D -f {conf}\nExecReload=/bin/kill -HUP $MAINPID\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=multi-user.target\n",
            conf = SFTP_CONFIG_FILE,
        );
        write_file_as_root(SFTP_SERVICE_FILE, &service)?;

        let _ = Command::new("sudo")
            .arg("systemctl")
            .args(["daemon-reload"])
            .output();
        let _ = Command::new("sudo")
            .arg("systemctl")
            .args(["enable", SFTP_SERVICE])
            .output();

        let is_active = Command::new("systemctl")
            .args(["is-active", "--quiet", SFTP_SERVICE])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if is_active {
            let _ = Command::new("sudo")
                .arg("systemctl")
                .args(["restart", SFTP_SERVICE])
                .output();
        }

        Ok(())
    }

    fn ensure_fstab_entry(
        share_path: &str,
        mountpoint: &str,
        username: &str,
        share_name: &str,
    ) -> Result<(), SftpError> {
        let tag = format!("{}{}:{}", FSTAB_TAG_PREFIX, username, share_name);
        let fstab = fs::read_to_string(FSTAB_PATH).unwrap_or_default();
        if fstab.contains(&tag) {
            return Ok(());
        }
        let entry = format!("{}\n{} {} none bind 0 0\n", tag, share_path, mountpoint);
        append_file_as_root(FSTAB_PATH, &entry)?;
        Ok(())
    }

    fn remove_fstab_entry(username: &str, share_name: &str) {
        let tag = format!("{}{}:{}", FSTAB_TAG_PREFIX, username, share_name);
        let Ok(fstab) = fs::read_to_string(FSTAB_PATH) else {
            return;
        };
        if !fstab.contains(&tag) {
            return;
        }
        let mut new_fstab = String::new();
        let mut skip_next = false;
        for line in fstab.lines() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if line.trim() == tag {
                skip_next = true;
                continue;
            }
            new_fstab.push_str(line);
            new_fstab.push('\n');
        }
        if let Err(e) = write_file_as_root(FSTAB_PATH, &new_fstab) {
            warn!(
                "SFTP: failed to remove fstab entry for {}:{}: {}",
                username, share_name, e
            );
        }
    }

    /// Unconditionally tear down everything this feature ever provisioned
    /// for `username` — every bind mount, its fstab entries, the jail
    /// directory, the authorized_keys file, and `homevault-sftp` group
    /// membership. Unlike `rebuild_jails`, this never consults the
    /// permission-grant tables at all, so it isn't affected by a deleted
    /// user's stale grant rows (see `handle_user_delete` in the separately
    /// frozen `src/handlers/users.rs`, which logs that it revokes a
    /// deleted user's direct share permissions but never actually calls
    /// `PermissionService::revoke_permission` — a real, pre-existing bug,
    /// reported rather than fixed here since that file is frozen). Called
    /// once, directly, from `handle_user_delete`'s success branch, the same
    /// way that handler already directly deletes the user's transfer-quota
    /// row rather than waiting for some later reconciliation pass. Never
    /// deletes the Linux account itself — same reasoning as
    /// `ensure_linux_user`: it may pre-date or outlive this feature.
    pub fn purge_user(username: &str) {
        let jail = paths::sftp_jails_dir().join(username);
        if let Ok(mounts) = fs::read_dir(&jail) {
            for entry in mounts.flatten() {
                let share_name = entry.file_name().to_string_lossy().to_string();
                Self::remove_bind_mount(&entry.path(), username, &share_name);
            }
        }
        let _ = Command::new("sudo")
            .args(["rmdir", &jail.to_string_lossy()])
            .output();

        let keys_dir = paths::sftp_keys_dir().join(username);
        let keys_file = keys_dir.join("authorized_keys");
        let _ = Command::new("sudo")
            .args(["rm", "-f", &keys_file.to_string_lossy()])
            .output();
        let _ = Command::new("sudo")
            .args(["rmdir", &keys_dir.to_string_lossy()])
            .output();

        let _ = Command::new("sudo")
            .args(["gpasswd", "-d", username, SFTP_GROUP])
            .output();
    }

    fn ensure_bind_mount(
        share_path: &str,
        mountpoint: &std::path::Path,
        username: &str,
        share_name: &str,
    ) -> Result<(), SftpError> {
        let mp = mountpoint.to_string_lossy().to_string();
        Self::mkdir_as_root(&mp)?;

        let already_mounted = Command::new("mountpoint")
            .args(["-q", &mp])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !already_mounted {
            let output = Command::new("sudo")
                .args(["mount", "--bind", share_path, &mp])
                .output()?;
            if !output.status.success() {
                return Err(SftpError::CommandFailed(format!(
                    "mount --bind '{}' -> '{}' failed: {}",
                    share_path,
                    mp,
                    String::from_utf8_lossy(&output.stderr)
                )));
            }
        }

        Self::ensure_fstab_entry(share_path, &mp, username, share_name)
    }

    fn remove_bind_mount(mountpoint: &std::path::Path, username: &str, share_name: &str) {
        let mp = mountpoint.to_string_lossy().to_string();
        let _ = Command::new("sudo").args(["umount", &mp]).output();
        let _ = Command::new("sudo").args(["rmdir", &mp]).output();
        Self::remove_fstab_entry(username, share_name);
    }

    /// Reconcile every user's chroot jail against real ACL-based
    /// share-permission grants on enabled SFTP-type shares. Bind-mounts
    /// every granted share into the grantee's jail, provisions their Linux
    /// account if needed, and unmounts/removes anything no longer granted.
    /// Never deletes a Linux account outright — only this feature's own
    /// group membership and jail contents, since the account may pre-date
    /// or outlive this feature (e.g. a promoted admin's real shell account).
    pub async fn rebuild_jails(pool: Arc<sqlx::SqlitePool>) -> Result<(), SftpError> {
        Self::ensure_group()?;

        let shares =
            SharesService::get_shares().map_err(|e| SftpError::CommandFailed(e.to_string()))?;
        let sftp_shares: Vec<Share> = shares
            .into_iter()
            .filter(|s| s.share_type == ShareType::Sftp && s.enabled)
            .collect();

        let mut desired: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();

        for share in &sftp_shares {
            let perms =
                PermissionService::list_share_permissions(&share.name, "sftp", Arc::clone(&pool))
                    .await
                    .map_err(|e| SftpError::CommandFailed(e.to_string()))?;

            let mut usernames: HashSet<String> = HashSet::new();
            for perm in &perms {
                if perm.grantee_type == "user" {
                    usernames.insert(perm.grantee_name.clone());
                } else if perm.grantee_type == "group" {
                    if let Ok(group) = GroupService::get_group(&perm.grantee_name) {
                        usernames.extend(group.members);
                    }
                }
            }

            for username in usernames {
                desired
                    .entry(username)
                    .or_default()
                    .push((share.name.clone(), share.path.clone()));
            }
        }

        for (username, grants) in &desired {
            if let Err(e) = Self::ensure_linux_user(username) {
                warn!(
                    "SFTP: failed to ensure Linux account for '{}': {}",
                    username, e
                );
                continue;
            }
            let jail = paths::sftp_jails_dir().join(username);
            if let Err(e) = Self::mkdir_as_root(&jail.to_string_lossy()) {
                warn!("SFTP: failed to create jail for '{}': {}", username, e);
                continue;
            }
            for (share_name, share_path) in grants {
                let mountpoint = jail.join(share_name);
                if let Err(e) =
                    Self::ensure_bind_mount(share_path, &mountpoint, username, share_name)
                {
                    warn!(
                        "SFTP: failed to bind-mount '{}' for '{}': {}",
                        share_name, username, e
                    );
                }
            }
        }

        let jails_root = paths::sftp_jails_dir();
        if jails_root.exists() {
            if let Ok(entries) = fs::read_dir(&jails_root) {
                for entry in entries.flatten() {
                    let username = entry.file_name().to_string_lossy().to_string();
                    let user_jail = entry.path();
                    if !user_jail.is_dir() {
                        continue;
                    }
                    let desired_shares: HashSet<String> = desired
                        .get(&username)
                        .map(|v| v.iter().map(|(n, _)| n.clone()).collect())
                        .unwrap_or_default();

                    if let Ok(mounts) = fs::read_dir(&user_jail) {
                        for mount_entry in mounts.flatten() {
                            let share_name = mount_entry.file_name().to_string_lossy().to_string();
                            if !desired_shares.contains(&share_name) {
                                Self::remove_bind_mount(
                                    &mount_entry.path(),
                                    &username,
                                    &share_name,
                                );
                            }
                        }
                    }

                    if !desired.contains_key(&username) {
                        let _ = Command::new("sudo")
                            .args(["gpasswd", "-d", &username, SFTP_GROUP])
                            .output();
                        let _ = Command::new("sudo")
                            .args(["rmdir", &user_jail.to_string_lossy()])
                            .output();
                    }
                }
            }
        }

        Ok(())
    }

    /// List every user with SFTP access to at least one share, for the
    /// "SFTP Users" key-management panel.
    pub async fn list_grantees(pool: Arc<sqlx::SqlitePool>) -> Result<Vec<SftpGrantee>, SftpError> {
        let shares =
            SharesService::get_shares().map_err(|e| SftpError::CommandFailed(e.to_string()))?;
        let sftp_shares: Vec<Share> = shares
            .into_iter()
            .filter(|s| s.share_type == ShareType::Sftp)
            .collect();

        let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for share in &sftp_shares {
            let perms =
                PermissionService::list_share_permissions(&share.name, "sftp", Arc::clone(&pool))
                    .await
                    .map_err(|e| SftpError::CommandFailed(e.to_string()))?;

            let mut usernames: HashSet<String> = HashSet::new();
            for perm in &perms {
                if perm.grantee_type == "user" {
                    usernames.insert(perm.grantee_name.clone());
                } else if perm.grantee_type == "group" {
                    if let Ok(group) = GroupService::get_group(&perm.grantee_name) {
                        usernames.extend(group.members);
                    }
                }
            }
            for username in usernames {
                by_user
                    .entry(username)
                    .or_default()
                    .push(share.name.clone());
            }
        }

        // Also include users who already have an SFTP key provisioned but no
        // share grant yet — `setfacl` (the frozen ACL grant this list feeds
        // into via the share's own "Manage Access") requires the grantee to
        // already exist as a real Linux account, so a user's SFTP key must
        // be set *before* they can be granted a share, not after. Without
        // this, a freshly key-provisioned user would be invisible here until
        // a grant already existed — a chicken-and-egg an admin can't resolve
        // through the UI alone.
        if let Ok(entries) = fs::read_dir(paths::sftp_keys_dir()) {
            for entry in entries.flatten() {
                if entry.path().join("authorized_keys").exists() {
                    let username = entry.file_name().to_string_lossy().to_string();
                    by_user.entry(username).or_default();
                }
            }
        }

        Ok(by_user
            .into_iter()
            .map(|(username, shares)| {
                let has_key = paths::sftp_keys_dir()
                    .join(&username)
                    .join("authorized_keys")
                    .exists();
                SftpGrantee {
                    username,
                    shares,
                    has_key,
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for the pasted-SSH-public-key validator. Everything
    //! else in this module shells out via `sudo` to `useradd`/`usermod`/
    //! `mount --bind`/`sshd` and needs the real host — covered by
    //! `tests/e2e/shares/shares.spec.ts`.

    use super::*;

    #[test]
    fn test_validate_public_key_accepts_common_key_types() {
        let material = "AAAAC3NzaC1lZDI1NTE5AAAAIJ1abcdefghijklmnopqrstuvwxyz0123456789ABCD";
        for line in [
            format!("ssh-ed25519 {material}"),
            format!("ssh-ed25519 {material} user@host"),
            format!("ssh-rsa {material}"),
            format!("ecdsa-sha2-nistp256 {material}"),
            format!("sk-ssh-ed25519@openssh.com {material}"),
            format!("sk-ecdsa-sha2-nistp256@openssh.com {material}"),
        ] {
            assert!(
                SftpJailService::validate_public_key(&line).is_ok(),
                "should accept: {line}"
            );
        }
    }

    #[test]
    fn test_validate_public_key_trims_surrounding_whitespace() {
        let key = "  ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc key\n";
        let out = SftpJailService::validate_public_key(key).unwrap();
        assert_eq!(out, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIabc key");
        assert!(!out.contains('\n'));
    }

    #[test]
    fn test_validate_public_key_rejects_empty_and_blank() {
        for s in ["", "   ", "\n", "\t"] {
            assert!(SftpJailService::validate_public_key(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn test_validate_public_key_rejects_embedded_newline_injection() {
        // The whole point of this validator: a pasted value must never be
        // able to smuggle a second authorized_keys line or a forced-command
        // option past it.
        let injection =
            "ssh-ed25519 AAAAvalidlooking key\ncommand=\"rm -rf /\" ssh-ed25519 AAAAevil";
        assert!(SftpJailService::validate_public_key(injection).is_err());
        let cr = "ssh-ed25519 AAAAvalidlooking\rssh-rsa AAAAother";
        assert!(SftpJailService::validate_public_key(cr).is_err());
    }

    #[test]
    fn test_validate_public_key_rejects_unknown_prefix() {
        for s in [
            "ssh-dss AAAAB3NzaC1kc3M",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "not-a-key at all",
            "AAAAC3NzaC1lZDI1NTE5 justmaterial",
        ] {
            assert!(SftpJailService::validate_public_key(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn test_validate_public_key_rejects_type_only_no_material() {
        // A recognized prefix but only one whitespace-separated field.
        assert!(SftpJailService::validate_public_key("ssh-ed25519").is_err());
        assert!(SftpJailService::validate_public_key("ssh-rsa ").is_err());
    }
}
