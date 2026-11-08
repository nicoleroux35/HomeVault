/* =============================================================================
 * File          : host_accounts.rs
 * Author        : Nicolas Le Roux
 * Created       : 21/10/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Lifecycle of the real Linux account that backs some HomeVault accounts:
 *   existence and system-account checks, admin-group grants and revocation
 *   (homevault_admin/sudo/wheel), password sync, password lock/unlock and
 *   account deletion. Every command is validated, time-bounded and runs off
 *   the async runtime; a host failure is reported, never silently ignored.
 *
 * Dependencies:
 *   - thiserror (>= 2)
 *   - tokio (>= 1.52) — process, io-util, time
 *   - tracing (>= 0.1)
 *
 * Usage:
 *   `HostAccountService` from src/handlers/users.rs (delete, lock, password),
 *   src/auth/service.rs (role change, password change, lock) and
 *   src/storage/database.rs (admin promotion sync).
 * =============================================================================
 */

//! Host (Linux) identity lifecycle for HomeVault accounts.
//!
//! A HomeVault account may or may not have a matching Linux account:
//!
//!   * a **database account** created by the Setup Wizard or the Users page is
//!     DB-only until it needs the host — an admin promotion
//!     (`sync_admin_user_to_group`) or a group membership
//!     (`GroupService::ensure_linux_user`) provisions one on demand;
//!   * a **host account** that signs in through PAM gets a mirrored `users`
//!     row with the [`crate::auth::PAM_PROVISIONED_HASH`] sentinel.
//!
//! Either way, the Linux account outlives the HomeVault row unless someone
//! removes it. Before 2026-10-21 nothing did: deleting a promoted admin left a
//! working Linux login (and, because `is_admin` derives admin rights from host
//! group membership, a working *HomeVault admin* login that reappeared on the
//! next PAM sign-in), demoting one left real `sudo`/`wheel` access in place,
//! and locking one left the host password usable. This module is the single
//! place that closes those gaps.
//!
//! Every command here is:
//!
//!   1. **validated** — the username must be a plain Linux login name, so a
//!      path/argv fragment can never reach `userdel`/`chpasswd`;
//!   2. **time-bounded** — a hung NSS lookup or a locked `/etc/passwd` must
//!      never pin an HTTP request (or a Tokio worker) forever;
//!   3. **off the async runtime** — `tokio::process` only, never
//!      `std::process::Command` from a handler;
//!   4. **reported** — callers get a typed error and log it, instead of
//!      swallowing a failure that left the host and the app disagreeing.

use crate::config::paths;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Hard ceiling for any host-account command. `userdel` on a busy account can
/// take a moment; a hung NSS backend takes forever.
pub const HOST_OP_TIMEOUT: Duration = Duration::from_secs(20);

/// The **application** admin group. Membership grants HomeVault admin rights
/// and is the only group `grant_admin` adds automatically. It does **not**
/// grant host root: the web terminal launches a shell *as the admin user*
/// through the `homevault` service's own sudoers rule.
pub const APP_ADMIN_GROUP: &str = crate::storage::database::ADMIN_GROUP_NAME;

/// Optional **host root** group. Never granted automatically; an operator who
/// needs an interactive root shell adds a user to it by hand and sets a Linux
/// password. The sudoers line for it asks for that password (DSM7's
/// `%administrators` model), instead of the old passwordless `ALL`.
pub const HOST_ADMIN_GROUP: &str = "homevault_hostadmin";

/// Every group a demotion must clear. It includes the application admin group
/// and the host-root groups (`homevault_hostadmin`, `sudo`, `wheel`) so a
/// demoted account cannot keep real root access. `grant_admin` only adds
/// [`APP_ADMIN_GROUP`]; the others are cleaned up here for accounts promoted
/// by older builds or by the operator.
pub const HOST_PRIVILEGED_GROUPS: &[&str] = &[APP_ADMIN_GROUP, HOST_ADMIN_GROUP, "sudo", "wheel"];

/// Linux login names are capped at 32 bytes (`LOGIN_NAME_MAX`), so a longer
/// HomeVault username can never be provisioned on the host.
const MAX_LINUX_USERNAME_LEN: usize = 32;

/// Errors returned by host-account operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum HostAccountError {
    #[error("Invalid username: {0}")]
    InvalidUsername(String),
    #[error("Refusing to modify the system account '{0}'")]
    SystemAccount(String),
    #[error("{what} timed out after {}s", HOST_OP_TIMEOUT.as_secs())]
    Timeout { what: &'static str },
    #[error("{what} could not be started: {message}")]
    Io { what: &'static str, message: String },
    #[error("{what} failed: {message}")]
    Failed { what: &'static str, message: String },
}

/// Whether this process should provision host/Samba identities on account
/// creation. The systemd unit sets `HV_HOST_SYNC=1`; a bare test run (or a
/// web-only developer build) leaves it unset and never shells out to
/// `useradd`/`smbpasswd` just because a DB row was created.
pub fn host_sync_enabled() -> bool {
    match std::env::var("HV_HOST_SYNC") {
        Ok(value) => !value.is_empty() && value != "0",
        // Opt-in: the systemd unit sets `HV_HOST_SYNC=1`. A bare test/dev
        // process (including a root CI container) must not shell out to
        // `useradd`/`smbpasswd` just because an in-memory account was created.
        Err(_) => false,
    }
}

/// Lifecycle operations on the Linux account backing a HomeVault account.
pub struct HostAccountService;

impl HostAccountService {
    /// Whether the process can already act as root (the shipped systemd unit
    /// runs HomeVault as root). When it cannot, commands are prefixed with
    /// `sudo`, matching the rest of the codebase.
    fn is_effective_root() -> bool {
        // `/proc/self/status` carries `Uid: real effective saved fs`; reading
        // it avoids a subprocess (and a `libc` dependency) just to learn this.
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find(|line| line.starts_with("Uid:"))
                    .and_then(|line| line.split_whitespace().nth(2))
                    .and_then(|effective| effective.parse::<u32>().ok())
            })
            .map(|effective| effective == 0)
            .unwrap_or(false)
    }

    fn command(bin: &str) -> Command {
        if Self::is_effective_root() {
            Command::new(bin)
        } else {
            let mut cmd = Command::new("sudo");
            cmd.arg(bin);
            cmd
        }
    }

    /// Validate a username before it reaches any host command.
    ///
    /// Deliberately stricter than `useradd` itself: a leading letter or
    /// underscore (never `-`, so nothing is ever parsed as an option), ASCII
    /// alphanumerics plus `_`/`-`, and at most 32 bytes.
    pub fn validate_username(username: &str) -> Result<(), HostAccountError> {
        let invalid = |reason: &str| {
            Err(HostAccountError::InvalidUsername(format!(
                "'{}' ({})",
                username, reason
            )))
        };

        if username.is_empty() {
            return invalid("empty");
        }
        if username.len() > MAX_LINUX_USERNAME_LEN {
            return invalid("longer than 32 characters");
        }
        if username == "." || username == ".." {
            return invalid("reserved");
        }

        let mut chars = username.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return invalid("must start with a letter or underscore"),
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return invalid("only ASCII letters, digits, '_' and '-' are allowed");
        }

        Ok(())
    }

    /// The default home directory `useradd -m` creates for a HomeVault account.
    pub fn home_dir(username: &str) -> PathBuf {
        paths::user_home_path(username)
    }

    /// Run one command to completion under [`HOST_OP_TIMEOUT`].
    async fn run(
        mut cmd: Command,
        what: &'static str,
    ) -> Result<std::process::Output, HostAccountError> {
        cmd.stdin(std::process::Stdio::null());
        cmd.kill_on_drop(true);
        match tokio::time::timeout(HOST_OP_TIMEOUT, cmd.output()).await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(e)) => Err(HostAccountError::Io {
                what,
                message: e.to_string(),
            }),
            Err(_) => Err(HostAccountError::Timeout { what }),
        }
    }

    /// Run one command and turn a non-zero exit into [`HostAccountError::Failed`].
    async fn run_checked(
        cmd: Command,
        what: &'static str,
    ) -> Result<std::process::Output, HostAccountError> {
        let output = Self::run(cmd, what).await?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(HostAccountError::Failed {
                what,
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// Whether a Linux account with this username exists.
    pub async fn exists(username: &str) -> bool {
        if Self::validate_username(username).is_err() {
            return false;
        }
        let mut cmd = Command::new("id");
        cmd.args(["-u", username]);
        matches!(Self::run(cmd, "id").await, Ok(o) if o.status.success())
    }

    /// The account's numeric UID, or `None` when it does not exist.
    pub async fn uid(username: &str) -> Option<u32> {
        if Self::validate_username(username).is_err() {
            return None;
        }
        let mut cmd = Command::new("id");
        cmd.args(["-u", username]);
        let output = Self::run(cmd, "id").await.ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }

    /// Whether `uid` belongs to the system range rather than a human account.
    ///
    /// `userdel`/`usermod -L` must never touch the service accounts the
    /// distribution ships (0–999 on Debian/Ubuntu, plus `nobody` at 65534).
    pub fn is_system_uid(uid: u32) -> bool {
        !(1000..65534).contains(&uid)
    }

    /// Whether this is a distribution/service account that must be left alone.
    pub async fn is_system_account(username: &str) -> bool {
        match Self::uid(username).await {
            Some(uid) => Self::is_system_uid(uid),
            // An account that does not exist cannot be a system account.
            None => false,
        }
    }

    /// The privileged groups from [`HOST_PRIVILEGED_GROUPS`] the account
    /// belongs to.
    pub async fn admin_groups(username: &str) -> Vec<String> {
        if Self::validate_username(username).is_err() {
            return Vec::new();
        }
        let mut cmd = Command::new("id");
        cmd.args(["-nG", username]);
        let Ok(output) = Self::run(cmd, "id").await else {
            return Vec::new();
        };
        if !output.status.success() {
            return Vec::new();
        }
        let current: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        HOST_PRIVILEGED_GROUPS
            .iter()
            .filter(|group| current.iter().any(|name| name == *group))
            .map(|group| group.to_string())
            .collect()
    }

    async fn group_exists(group: &str) -> bool {
        let mut cmd = Command::new("getent");
        cmd.args(["group", group]);
        matches!(Self::run(cmd, "getent group").await, Ok(o) if o.status.success())
    }

    /// Provision the Linux account if needed and grant the **application**
    /// admin group only.
    ///
    /// Returns the groups that were added. The account is created with a real
    /// shell and home directory (`useradd -m -d /home/homevault/<user> -s
    /// /bin/bash`) because it is the account the web terminal and SSH use;
    /// it is deliberately **not** added to `sudo`, `wheel` or
    /// `homevault_hostadmin` — host root is a separate, explicit grant.
    pub async fn grant_admin(username: &str) -> Result<Vec<String>, HostAccountError> {
        Self::validate_username(username)?;

        if !Self::exists(username).await {
            let home = Self::home_dir(username);
            let mut cmd = Self::command("useradd");
            cmd.args([
                "-m",
                "-d",
                &home.to_string_lossy(),
                "-s",
                "/bin/bash",
                username,
            ]);
            Self::run_checked(cmd, "useradd").await?;
            tracing::info!(
                "Provisioned Linux account '{}' at {} for admin access",
                username,
                home.display()
            );
        }

        let mut added = Vec::new();
        for group in [APP_ADMIN_GROUP] {
            if !Self::group_exists(group).await {
                continue;
            }
            let mut cmd = Self::command("usermod");
            cmd.args(["-a", "-G", group, username]);
            match Self::run_checked(cmd, "usermod").await {
                Ok(_) => added.push(group.to_string()),
                Err(e) => tracing::warn!(
                    "Could not add '{}' to host group '{}': {}",
                    username,
                    group,
                    e
                ),
            }
        }

        Ok(added)
    }

    /// Revoke every privileged group from the account (application admin,
    /// optional host root, and the legacy `sudo`/`wheel` grants).
    ///
    /// Returns the groups that were actually removed. An account that does not
    /// exist, or that is not a member of a group, is a no-op — the caller's
    /// intent ("this account must not be an admin") already holds. Demotion is
    /// deliberately conservative: a host-root grant must be re-added by the
    /// operator after an application demotion.
    pub async fn revoke_admin(username: &str) -> Result<Vec<String>, HostAccountError> {
        Self::validate_username(username)?;

        let current = Self::admin_groups(username).await;
        let mut removed = Vec::new();
        for group in current {
            let mut cmd = Self::command("gpasswd");
            cmd.args(["-d", username, &group]);
            match Self::run(cmd, "gpasswd").await {
                Ok(output) if output.status.success() => removed.push(group),
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    // "not a member" / "does not exist": nothing to revoke.
                    if !stderr.contains("not a member") && !stderr.contains("does not exist") {
                        return Err(HostAccountError::Failed {
                            what: "gpasswd",
                            message: stderr.trim().to_string(),
                        });
                    }
                }
                Err(e) => return Err(e),
            }
        }

        Ok(removed)
    }

    /// Provision a bare Linux account for a HomeVault user, without a home
    /// directory or a login shell.
    ///
    /// SMB, FTP and NFS all resolve a *Linux* identity (a Samba passdb entry
    /// is keyed by the Unix account, FTP authenticates through PAM, and NFS
    /// compares numeric UIDs). A HomeVault account is database-only by
    /// default, so the first use of one of those protocols must create the
    /// Unix half. `-M` keeps the user's real home folder an explicit decision
    /// (the Users page / `grant_admin` own it) and `nologin` keeps the
    /// account from becoming a shell login by accident.
    pub async fn ensure_linux_account(username: &str) -> Result<(), HostAccountError> {
        Self::validate_username(username)?;
        if Self::exists(username).await {
            return Ok(());
        }

        let mut cmd = Self::command("useradd");
        cmd.args(["-M", "-s", "/usr/sbin/nologin", username]);
        match Self::run(cmd, "useradd").await {
            Ok(output) if output.status.success() => {
                tracing::info!("Provisioned bare Linux account '{}'", username);
                Ok(())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // A concurrent provision or a pre-existing account is not a
                // failure: the caller asked for "the account exists".
                if stderr.contains("already exists") {
                    return Ok(());
                }
                Err(HostAccountError::Failed {
                    what: "useradd",
                    message: stderr.trim().to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// The two-line stdin body `smbpasswd -a -s` expects.
    ///
    /// Samba reads the new password and its confirmation from stdin. A
    /// newline in either copy would inject a second line, so it is rejected
    /// here — the same guard as [`Self::chpasswd_line`].
    pub fn smbpasswd_input(password: &str) -> Result<String, HostAccountError> {
        if password.is_empty() {
            return Err(HostAccountError::InvalidUsername(
                "password cannot be empty".to_string(),
            ));
        }
        if password.contains('\n') || password.contains('\r') {
            return Err(HostAccountError::InvalidUsername(
                "password cannot contain a line break".to_string(),
            ));
        }
        Ok(format!("{password}\n{password}\n"))
    }

    /// Create or update the Samba passdb credential for an account.
    ///
    /// HomeVault created DB-only users and never called `smbpasswd`, so a
    /// real SMB login was impossible for every account the appliance itself
    /// provisioned — the daemon had no password hash for it. This is the
    /// missing half of user creation / password change. Best-effort callers
    /// log a failure; the database write is still authoritative for the web
    /// login.
    pub async fn set_samba_password(
        username: &str,
        password: &str,
    ) -> Result<(), HostAccountError> {
        Self::validate_username(username)?;
        let input = Self::smbpasswd_input(password)?;

        let mut cmd = Self::command("smbpasswd");
        cmd.args(["-a", "-s", username]);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| HostAccountError::Io {
            what: "smbpasswd",
            message: e.to_string(),
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .await
                .map_err(|e| HostAccountError::Io {
                    what: "smbpasswd",
                    message: e.to_string(),
                })?;
        }

        let output = match tokio::time::timeout(HOST_OP_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(HostAccountError::Io {
                    what: "smbpasswd",
                    message: e.to_string(),
                })
            }
            Err(_) => return Err(HostAccountError::Timeout { what: "smbpasswd" }),
        };

        if output.status.success() {
            Ok(())
        } else {
            Err(HostAccountError::Failed {
                what: "smbpasswd",
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// Remove the Samba passdb credential before the Unix account is deleted.
    pub async fn delete_samba_account(username: &str) -> Result<(), HostAccountError> {
        Self::validate_username(username)?;
        let mut cmd = Self::command("smbpasswd");
        cmd.args(["-x", username]);
        match Self::run(cmd, "smbpasswd").await {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                // No passdb entry is the desired end state.
                if stderr.contains("Failed to find user")
                    || stderr.contains("does not exist")
                    || stderr.contains("No such user")
                {
                    return Ok(());
                }
                Err(HostAccountError::Failed {
                    what: "smbpasswd",
                    message: stderr.trim().to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Disable/enable the Samba credential (`smbpasswd -d/-e`).
    pub async fn set_samba_locked(
        username: &str,
        locked: bool,
    ) -> Result<(), HostAccountError> {
        Self::validate_username(username)?;
        let mut cmd = Self::command("smbpasswd");
        cmd.arg(if locked { "-d" } else { "-e" });
        cmd.arg(username);
        match Self::run(cmd, "smbpasswd").await {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("Failed to find user")
                    || stderr.contains("does not exist")
                    || stderr.contains("No such user")
                {
                    return Ok(());
                }
                Err(HostAccountError::Failed {
                    what: "smbpasswd",
                    message: stderr.trim().to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// Remove the Linux account, keeping its home directory.
    ///
    /// Home removal is the caller's decision (`DELETE /users/{u}?delete_home_folder`)
    /// and uses the folder HomeVault actually recorded, so `userdel -r` is
    /// deliberately not used: it would delete the passwd home only, which is
    /// not necessarily where the admin put the folder.
    pub async fn delete_account(username: &str) -> Result<(), HostAccountError> {
        Self::validate_username(username)?;

        let Some(uid) = Self::uid(username).await else {
            return Ok(());
        };
        if Self::is_system_uid(uid) {
            return Err(HostAccountError::SystemAccount(username.to_string()));
        }

        // Remove the Samba credential first: `userdel` would leave it behind
        // and a recreated account could inherit the old SMB password hash.
        if let Err(e) = Self::delete_samba_account(username).await {
            tracing::warn!(
                "Could not remove the Samba credential for '{}': {}",
                username,
                e
            );
        }

        let mut cmd = Self::command("userdel");
        cmd.arg(username);
        match Self::run(cmd, "userdel").await {
            Ok(output) if output.status.success() => {
                tracing::info!("Removed Linux account '{}'", username);
                Ok(())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("does not exist") {
                    return Ok(());
                }
                Err(HostAccountError::Failed {
                    what: "userdel",
                    message: stderr.trim().to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }

    /// The single `chpasswd` input line for `username:password`.
    ///
    /// `chpasswd` reads `name:password` per line, so a newline in either field
    /// would inject a second account's password. Both are validated here, in
    /// one testable place, instead of relying on every caller.
    pub fn chpasswd_line(username: &str, password: &str) -> Result<String, HostAccountError> {
        Self::validate_username(username)?;
        if password.is_empty() {
            return Err(HostAccountError::InvalidUsername(
                "password cannot be empty".to_string(),
            ));
        }
        if password.contains('\n') || password.contains('\r') {
            return Err(HostAccountError::InvalidUsername(
                "password cannot contain a line break".to_string(),
            ));
        }
        Ok(format!("{}:{}", username, password))
    }

    /// Set the Linux account's password, so the web terminal and SSH follow a
    /// HomeVault password change instead of keeping the old host credential.
    ///
    /// The password travels on stdin, never in argv: `ps` would otherwise
    /// expose it to every local user for the lifetime of the command.
    pub async fn set_password(username: &str, password: &str) -> Result<(), HostAccountError> {
        let line = Self::chpasswd_line(username, password)?;

        if !Self::exists(username).await {
            return Ok(());
        }

        let mut cmd = Self::command("chpasswd");
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| HostAccountError::Io {
            what: "chpasswd",
            message: e.to_string(),
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            // `take()` + drop closes the pipe, so `chpasswd` sees EOF and
            // exits instead of waiting for more input.
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| HostAccountError::Io {
                    what: "chpasswd",
                    message: e.to_string(),
                })?;
            stdin.write_all(b"\n").await.ok();
        }

        let output = match tokio::time::timeout(HOST_OP_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return Err(HostAccountError::Io {
                    what: "chpasswd",
                    message: e.to_string(),
                })
            }
            Err(_) => return Err(HostAccountError::Timeout { what: "chpasswd" }),
        };

        if output.status.success() {
            Ok(())
        } else {
            Err(HostAccountError::Failed {
                what: "chpasswd",
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// Lock or unlock the Linux account's password (`usermod -L` / `-U`).
    ///
    /// This is the host half of `PUT /users/{u}/locked`: the HomeVault lock
    /// already refuses a PAM sign-in, but without this the same password still
    /// opens an SSH session or the web terminal.
    pub async fn set_locked(username: &str, locked: bool) -> Result<(), HostAccountError> {
        Self::validate_username(username)?;

        let Some(uid) = Self::uid(username).await else {
            return Ok(());
        };
        if Self::is_system_uid(uid) {
            return Err(HostAccountError::SystemAccount(username.to_string()));
        }

        let mut cmd = Self::command("usermod");
        cmd.arg(if locked { "-L" } else { "-U" });
        cmd.arg(username);
        match Self::run(cmd, "usermod").await {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("does not exist") {
                    return Ok(());
                }
                Err(HostAccountError::Failed {
                    what: "usermod",
                    message: stderr.trim().to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_username_accepts_plain_login_names() {
        for name in ["alice", "a", "alice_2", "alice-2", "_svc", "Bob123"] {
            assert!(
                HostAccountService::validate_username(name).is_ok(),
                "'{}' should be a valid Linux login name",
                name
            );
        }
    }

    #[test]
    fn test_validate_username_rejects_option_like_names() {
        // A leading '-' would be parsed as an option by userdel/usermod.
        for name in ["-rf", "--help", "-", "-alice"] {
            assert!(
                HostAccountService::validate_username(name).is_err(),
                "{}",
                name
            );
        }
    }

    #[test]
    fn test_validate_username_rejects_shell_and_path_metacharacters() {
        for name in [
            "alice bob",
            "alice;rm",
            "alice|cat",
            "alice$(id)",
            "alice/../root",
            "../alice",
            "alice:bob",
            "alice\nroot",
        ] {
            assert!(
                HostAccountService::validate_username(name).is_err(),
                "{}",
                name
            );
        }
    }

    #[test]
    fn test_validate_username_rejects_empty_dots_and_overlong_names() {
        assert!(HostAccountService::validate_username("").is_err());
        assert!(HostAccountService::validate_username(".").is_err());
        assert!(HostAccountService::validate_username("..").is_err());
        assert!(HostAccountService::validate_username(&"a".repeat(32)).is_ok());
        assert!(HostAccountService::validate_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn test_validate_username_rejects_non_ascii() {
        // Linux login names are byte-oriented; a non-ASCII name would be
        // accepted by `useradd` on some systems but not by `id`/NSS here.
        assert!(HostAccountService::validate_username("émile").is_err());
    }

    #[test]
    fn test_is_system_uid_boundaries() {
        assert!(HostAccountService::is_system_uid(0));
        assert!(HostAccountService::is_system_uid(999));
        assert!(!HostAccountService::is_system_uid(1000));
        assert!(!HostAccountService::is_system_uid(65533));
        // `nobody` — never a HomeVault account.
        assert!(HostAccountService::is_system_uid(65534));
        assert!(HostAccountService::is_system_uid(65535));
    }

    #[test]
    fn test_privileged_groups_cover_app_admin_and_host_root() {
        // A demotion clears the application admin group *and* every host-root
        // group an older build (or the operator) may have added.
        assert_eq!(APP_ADMIN_GROUP, crate::storage::database::ADMIN_GROUP_NAME);
        assert!(HOST_PRIVILEGED_GROUPS.contains(&APP_ADMIN_GROUP));
        assert!(HOST_PRIVILEGED_GROUPS.contains(&HOST_ADMIN_GROUP));
        assert!(HOST_PRIVILEGED_GROUPS.contains(&"sudo"));
        assert!(HOST_PRIVILEGED_GROUPS.contains(&"wheel"));
    }

    #[test]
    fn test_chpasswd_line_formats_name_and_password() {
        assert_eq!(
            HostAccountService::chpasswd_line("alice", "S3cret-pass").unwrap(),
            "alice:S3cret-pass"
        );
    }

    #[test]
    fn test_chpasswd_line_rejects_line_break_injection() {
        // A newline would make chpasswd apply a second, attacker-chosen line.
        assert!(HostAccountService::chpasswd_line("alice", "pwd\nroot:evil").is_err());
        assert!(HostAccountService::chpasswd_line("alice", "pwd\rroot:evil").is_err());
    }

    #[test]
    fn test_chpasswd_line_rejects_empty_password_and_bad_username() {
        assert!(HostAccountService::chpasswd_line("alice", "").is_err());
        assert!(HostAccountService::chpasswd_line("-rf", "pwd").is_err());
    }

    #[test]
    fn test_home_dir_is_under_the_homevault_homes_root() {
        let home = HostAccountService::home_dir("alice");
        assert_eq!(home, PathBuf::from("/home/homevault/alice"));
        assert!(home.starts_with(paths::HOMES_BASE_DIR));
    }

    #[tokio::test]
    async fn test_exists_is_false_for_an_invalid_username() {
        // No command is spawned for a name that cannot be an account.
        assert!(!HostAccountService::exists("-rf").await);
        assert!(HostAccountService::uid("../root").await.is_none());
        assert!(HostAccountService::admin_groups("a b").await.is_empty());
    }

    #[tokio::test]
    async fn test_delete_account_is_a_noop_for_a_missing_account() {
        // A name that cannot exist: the operation must succeed (idempotent)
        // and must never reach `userdel`.
        let name = "hv-missing-account-xyz";
        assert!(!HostAccountService::exists(name).await);
        assert!(HostAccountService::delete_account(name).await.is_ok());
    }

    #[tokio::test]
    async fn test_set_password_is_a_noop_for_a_missing_account() {
        assert!(
            HostAccountService::set_password("hv-missing-account-xyz", "Str0ng-Pass")
                .await
                .is_ok()
        );
    }

    #[test]
    fn test_smbpasswd_input_is_the_two_confirmation_lines() {
        assert_eq!(
            HostAccountService::smbpasswd_input("S3cret-pass").unwrap(),
            "S3cret-pass\nS3cret-pass\n"
        );
    }

    #[test]
    fn test_smbpasswd_input_rejects_injection_and_empty_password() {
        assert!(HostAccountService::smbpasswd_input("").is_err());
        assert!(HostAccountService::smbpasswd_input("pwd\nroot:evil").is_err());
        assert!(HostAccountService::smbpasswd_input("pwd\rroot:evil").is_err());
    }

    #[tokio::test]
    async fn test_samba_sync_is_idempotent_when_samba_is_absent_or_user_missing() {
        // These must not panic or hang when `smbpasswd`/the account is absent:
        // the caller (user creation/password change) treats Samba as
        // best-effort and keeps the database write authoritative.
        let name = "hv-missing-account-xyz";
        assert!(HostAccountService::delete_samba_account(name).await.is_ok());
        assert!(HostAccountService::set_samba_locked(name, true).await.is_ok());
        assert!(HostAccountService::set_samba_locked(name, false).await.is_ok());
    }
}
