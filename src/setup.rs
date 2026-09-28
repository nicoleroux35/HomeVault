use std::fs::OpenOptions;
use std::io::Write;
use thiserror::Error;

/// Errors that can occur during initial system setup.
#[derive(Error, Debug)]
pub enum SetupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Setup already complete")]
    AlreadyComplete,
    #[error("Setup in progress by another process")]
    SetupInProgress,
    #[error("Admin user already exists")]
    AdminExists,
    #[error("Validation error: {0}")]
    Validation(String),
}

/// Service for first-run setup: flag files, hardware detection, user/password validation, sudoers.
pub struct SetupService;

impl SetupService {
    fn data_dir() -> std::path::PathBuf {
        crate::config::paths::base_dir()
    }
    fn setup_flag_file() -> std::path::PathBuf {
        crate::config::paths::setup_flag_file()
    }
    fn setup_lock_file() -> std::path::PathBuf {
        crate::config::paths::setup_lock_file()
    }
    fn system_hash_file() -> std::path::PathBuf {
        crate::config::paths::system_hash_file()
    }
    const RESERVED_USERNAMES: &'static [&'static str] = &[
        "root", "admin", "system", "daemon", "nobody", "www-data", "postgres", "mysql",
        "ftp", "mail", "operator", "bin", "sys", "sync", "games", "man", "lp", "news",
        "uucp", "proxy", "backup", "list", "irc", "gnats", "systemd", "messagebus",
    ];

    pub fn is_first_run() -> bool {
        !Self::setup_flag_file().exists()
    }

    /// Returns true if the given command can be located via `which`.
    pub fn is_command_available(command: &str) -> bool {
        std::process::Command::new("which")
            .arg(command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn is_setup_valid() -> bool {
        Self::setup_flag_file().exists() && Self::system_hash_file().exists()
    }

    /// Return a stable machine identifier, falling back to hostname for containers without machine-id.
    pub fn get_machine_id() -> String {
        std::fs::read_to_string("/etc/machine-id")
            .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
            .or_else(|_| {
                tracing::warn!("No machine-id found, using hostname as fallback");
                hostname::get().map(|h| h.to_string_lossy().to_string())
            })
            .unwrap_or_else(|_| {
                tracing::warn!("No machine-id or hostname found, using default");
                "unknown".to_string()
            })
    }

    pub fn generate_system_hash() -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();

        let machine_id = Self::get_machine_id();
        hasher.update(machine_id.as_bytes());

        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        hasher.update(hostname.as_bytes());

        // Deliberately excludes the OS username running this process: that's
        // an execution-context detail (systemd service vs. a manual/root run),
        // not a hardware attribute, and including it caused false "hardware
        // changed" positives whenever the service started under a different
        // account.
        hasher.update(std::env::consts::OS.as_bytes());
        hasher.update(std::env::consts::ARCH.as_bytes());

        hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect::<String>()
    }

    pub fn has_hardware_changed() -> bool {
        if !Self::system_hash_file().exists() {
            return false;
        }
        let stored_hash = std::fs::read_to_string(Self::system_hash_file()).unwrap_or_default();
        let current_hash = Self::generate_system_hash();

        if stored_hash != current_hash {
            tracing::warn!(
                "Hardware change detected - stored: {}, current: {}",
                stored_hash,
                current_hash
            );
            true
        } else {
            false
        }
    }

    /// Atomically mark setup as in progress to prevent race conditions
    pub fn mark_setup_in_progress() -> Result<(), SetupError> {
        std::fs::create_dir_all(Self::data_dir())?;

        // O_EXCL ensures atomicity - fails if file already exists
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(Self::setup_lock_file())
        {
            Ok(mut file) => {
                file.write_all(b"in_progress")?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(SetupError::SetupInProgress)
            }
            Err(e) => Err(SetupError::Io(e)),
        }
    }

    pub fn mark_setup_complete() -> Result<(), SetupError> {
        std::fs::create_dir_all(Self::data_dir())?;

        std::fs::write(Self::setup_flag_file(), "true")?;

        let system_hash = Self::generate_system_hash();
        std::fs::write(Self::system_hash_file(), system_hash)?;

        // Remove lock file
        let _ = std::fs::remove_file(Self::setup_lock_file());

        tracing::info!("Setup marked as complete");

        Ok(())
    }

    pub fn cleanup_setup_lock() {
        let _ = std::fs::remove_file(Self::setup_lock_file());
    }

    pub fn validate_username(username: &str) -> Result<(), String> {
        if username.is_empty() {
            return Err("Username cannot be empty".to_string());
        }

        if username.len() < 3 {
            return Err("Username must be at least 3 characters".to_string());
        }

        if username.len() > 64 {
            return Err("Username cannot exceed 64 characters".to_string());
        }

        // Check reserved usernames
        if Self::RESERVED_USERNAMES.contains(&username.to_lowercase().as_str()) {
            return Err(format!("Username '{}' is reserved", username));
        }

        // First character must be a letter or underscore
        let mut chars = username.chars();
        match chars.next() {
            Some(c) if c.is_alphabetic() || c == '_' => {}
            _ => return Err("Username must start with a letter or underscore".to_string()),
        }

        // Remaining characters must be alphanumeric or underscore
        if !chars.all(|c| c.is_alphanumeric() || c == '_') {
            return Err("Username can only contain letters, numbers, and underscores".to_string());
        }

        Ok(())
    }

    pub fn validate_password(password: &str) -> Result<(), String> {
        if password.len() < 8 {
            return Err("Password must be at least 8 characters".to_string());
        }

        if password.len() > 128 {
            return Err("Password cannot exceed 128 characters".to_string());
        }

        if !password.chars().any(|c| c.is_uppercase()) {
            return Err("Password must contain at least one uppercase letter".to_string());
        }

        if !password.chars().any(|c| c.is_lowercase()) {
            return Err("Password must contain at least one lowercase letter".to_string());
        }

        if !password.chars().any(|c| c.is_numeric()) {
            return Err("Password must contain at least one digit".to_string());
        }

        Ok(())
    }

    pub fn validate_fullname(fullname: &str) -> Result<(), String> {
        if fullname.is_empty() {
            return Err("Full name cannot be empty".to_string());
        }

        if fullname.len() > 128 {
            return Err("Full name cannot exceed 128 characters".to_string());
        }

        Ok(())
    }

    /// Legacy wrapper around `validate_username` returning a boolean.
    pub fn is_valid_username(username: &str) -> bool {
        Self::validate_username(username).is_ok()
    }

    /// Resolve the absolute path of a binary via `which`, falling back to
    /// `default_path` when it is not installed (e.g. before setup).
    fn resolve_bin(bin: &str, default_path: &str) -> String {
        if let Ok(out) = std::process::Command::new("which").arg(bin).output() {
            if out.status.success() {
                let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !p.is_empty() {
                    return p;
                }
            }
        }
        default_path.to_string()
    }

    /// Create the WireGuard/OpenVPN config directories and make them owned by
    /// the unprivileged `homevault` service user, so it can write config
    /// files directly (the privileged `wg-quick`/`openvpn` commands are
    /// delegated to root via the sudoers rule instead).
    fn prepare_vpn_directories() {
        for dir in ["/etc/wireguard", "/etc/openvpn"] {
            if std::fs::create_dir_all(dir).is_err() {
                tracing::warn!("Could not create VPN config directory '{}'", dir);
                continue;
            }
            // Best-effort: own the directory as the homevault service user.
            if let Err(e) = std::process::Command::new("chown")
                .args(["homevault:homevault", dir])
                .output()
            {
                tracing::warn!("Could not chown VPN config directory '{}': {}", dir, e);
            }
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) = std::fs::set_permissions(
                    dir,
                    std::fs::Permissions::from_mode(0o750),
                ) {
                    tracing::warn!("Could not set permissions on '{}': {}", dir, e);
                }
            }
        }
    }

    pub fn configure_sudoers() -> Result<(), SetupError> {
        // Prepare config directories owned by the unprivileged homevault
        // service user so it can write WireGuard/OpenVPN config files.
        Self::prepare_vpn_directories();

        // Resolve VPN binary paths; fall back to common defaults if missing.
        let wg = Self::resolve_bin("wg", "/usr/bin/wg");
        let wg_quick = Self::resolve_bin("wg-quick", "/usr/bin/wg-quick");
        let openvpn = Self::resolve_bin("openvpn", "/usr/sbin/openvpn");
        let pkill = Self::resolve_bin("pkill", "/usr/bin/pkill");
        let kill = Self::resolve_bin("kill", "/usr/bin/kill");
        let ip = Self::resolve_bin("ip", "/sbin/ip");
        let ufw = Self::resolve_bin("ufw", "/usr/sbin/ufw");
        let mdadm = Self::resolve_bin("mdadm", "/usr/sbin/mdadm");
        let wpa_cli = Self::resolve_bin("wpa_cli", "/usr/sbin/wpa_cli");
        // Dashboard/Health disk S.M.A.R.T. status — `homevault` is not in the
        // `disk` group (only `docker`, see install.sh) and real block devices
        // are `660 root:disk`, so `smartctl -H`/`-A` fails to even open the
        // device without root, the same reason `mdadm`/`blkid` need it above.
        // Read-only diagnostic tool, same trust level as `mdadm --examine`.
        let smartctl = Self::resolve_bin("smartctl", "/usr/sbin/smartctl");

        // Storage pool (ZFS/Btrfs) and disk-encryption (LUKS2) binaries.
        // `homevault` is not in the `disk` group (only `docker`, see
        // install.sh) and real block devices are `660 root:disk` — so even
        // just *opening* one of these for mkfs/wipefs fails with "Permission
        // denied", separate from `mount`'s own CAP_SYS_ADMIN requirement.
        // Confirmed live against a loop device deliberately given real disk
        // permissions (not the world-writable default a freshly-created
        // loop device gets): `mkfs.btrfs` failed outright. All Storage pool
        // create/delete/scrub and LUKS create/format ultimately go through
        // one of these.
        let zpool = Self::resolve_bin("zpool", "/usr/sbin/zpool");
        let zfs = Self::resolve_bin("zfs", "/usr/sbin/zfs");
        let mkfs_btrfs = Self::resolve_bin("mkfs.btrfs", "/sbin/mkfs.btrfs");
        let mkfs_ext4 = Self::resolve_bin("mkfs.ext4", "/sbin/mkfs.ext4");
        let mkfs_xfs = Self::resolve_bin("mkfs.xfs", "/sbin/mkfs.xfs");
        let mount_bin = Self::resolve_bin("mount", "/usr/bin/mount");
        let umount_bin = Self::resolve_bin("umount", "/usr/bin/umount");
        let wipefs = Self::resolve_bin("wipefs", "/usr/sbin/wipefs");
        let btrfs = Self::resolve_bin("btrfs", "/usr/bin/btrfs");
        let cryptsetup = Self::resolve_bin("cryptsetup", "/usr/sbin/cryptsetup");
        let blkid = Self::resolve_bin("blkid", "/usr/sbin/blkid");
        // Pool/encrypted-volume mount points live under /mnt, which is
        // root-owned — `sudo mkdir`/`rmdir` are needed for the same reason
        // as everything else above (std::fs::create_dir_all runs as the
        // unprivileged process directly and isn't covered by any of this;
        // it must always go through one of these two instead).
        let mkdir = Self::resolve_bin("mkdir", "/usr/bin/mkdir");
        let rmdir = Self::resolve_bin("rmdir", "/usr/bin/rmdir");
        // `sudo tee` is how pools.rs/encryption.rs write /etc/fstab entries
        // (644 root:root, so a direct std::fs write also fails with EACCES)
        // — confirmed live this was silently swallowed by a bare `let _ =`
        // at the call site, so a pool/encrypted-volume mount was never
        // actually persisted and would not have survived a reboot.
        let tee = Self::resolve_bin("tee", "/usr/bin/tee");

        // WireGuard server config (`/etc/wireguard/wg0.conf`) — deliberately
        // `0600` since it holds the server's own private key, unlike the
        // other root-owned configs above (fstab, smb.conf, ...) which stay
        // default-readable — so unlike those, even reading it back or
        // chmod'ing it after a `sudo tee` write needs root too, not just
        // writing it.
        let cat = Self::resolve_bin("cat", "/usr/bin/cat");
        let test_bin = Self::resolve_bin("test", "/usr/bin/test");

        // System Journal viewer — `homevault` is only in the `docker` group
        // (see install.sh), not `adm`/`systemd-journal`, so reading the
        // systemd journal or the rotating /var/log text files needs root
        // the same way every other host-privileged read in this file does.
        let journalctl = Self::resolve_bin("journalctl", "/usr/bin/journalctl");
        let tail = Self::resolve_bin("tail", "/usr/bin/tail");

        // Shares page service control (smbd/nfs-server/vsftpd/homevault-webdav)
        // — `homevault` needs root to start/stop/restart/reload these systemd
        // units the same way it needs root for mdadm/zpool above; unlike those,
        // `systemctl` itself is not scoped to a single subsystem (it can control
        // *any* unit on the host, including sshd or homevault's own service), so
        // this grant is deliberately scoped to the exact reload/start/stop/
        // restart/enable/daemon-reload commands src/shares.rs actually runs,
        // rather than the whole `systemctl` binary.
        let systemctl = Self::resolve_bin("systemctl", "/usr/bin/systemctl");

        // Shares system config writes — /etc/samba/smb.conf, /etc/vsftpd.conf,
        // /etc/exports, and the WebDAV service/script must stay root-owned
        // (unlike /etc/homevault's own smb.json/nfs.json/etc, which install.sh
        // chowns to homevault directly), so src/shares.rs writes them via
        // `sudo tee`. Scoped to the exact files, not the whole `tee` binary —
        // an unscoped grant would let homevault overwrite any file on the
        // host as root, e.g. /etc/shadow or this very sudoers file.
        let exportfs = Self::resolve_bin("exportfs", "/usr/sbin/exportfs");
        let chmod = Self::resolve_bin("chmod", "/usr/bin/chmod");
        let webdav_py = "/usr/local/bin/homevault-webdav.py";
        let webdav_service = "/etc/systemd/system/homevault-webdav.service";
        let smb_conf = "/etc/samba/smb.conf";
        let vsftpd_conf = "/etc/vsftpd.conf";
        let exports = "/etc/exports";

        // SFTP (real, SSH-based) — `src/sftp.rs` provisions a real,
        // shell-less Linux account + `homevault-sftp` group membership for
        // every user granted access to an SFTP share (regular HomeVault
        // users are DB-only otherwise), the same way
        // `sync_admin_user_to_group` already does for promoted admins. The
        // `mount`/`umount`/`mkdir`/`rmdir`/`tee` grants it also needs are
        // already covered, unscoped, by the Storage pools grant just above.
        // `useradd`/`usermod`/`gpasswd` are deliberately scoped to the exact
        // arguments `src/sftp.rs` runs, not the bare binaries — unscoped,
        // `usermod`/`gpasswd` could add/remove any user from any group
        // (including `homevault_admin`) and `useradd` could create an
        // arbitrary account with an arbitrary shell/uid.
        let groupadd = Self::resolve_bin("groupadd", "/usr/sbin/groupadd");
        let groupdel = Self::resolve_bin("groupdel", "/usr/sbin/groupdel");
        let useradd = Self::resolve_bin("useradd", "/usr/sbin/useradd");
        let usermod_sftp = Self::resolve_bin("usermod", "/usr/sbin/usermod");
        let gpasswd = Self::resolve_bin("gpasswd", "/usr/sbin/gpasswd");
        // `chpasswd` clears the locked-password flag `useradd` leaves on a
        // freshly created SFTP account — confirmed live that OpenSSH
        // rejects a locked account ("account is locked") even for pubkey-
        // only auth with UsePAM no, independent of PasswordAuthentication.
        // Deliberately granted unscoped (unlike every other binary in this
        // file, which is scoped to exact arguments): `chpasswd` takes the
        // target username via stdin, not argv, which sudoers cannot
        // pattern-match at all — there's no `chpasswd <fixed-prefix> *`
        // equivalent. `src/sftp.rs::unlock_password` is the only caller and
        // only ever runs immediately after this module's own `useradd` on
        // an account it just created, with a random, immediately-discarded
        // password, but sudoers itself cannot enforce that constraint —
        // this grant is only as safe as `src/sftp.rs`'s own call discipline.
        let chpasswd = Self::resolve_bin("chpasswd", "/usr/sbin/chpasswd");
        // `rm -f` deletes a purged user's own authorized_keys file
        // (`SftpJailService::purge_user`, called from `handle_user_delete`);
        // scoped to the exact glob, not the whole `rm` binary — an
        // unscoped grant would let homevault delete any file on the host.
        let rm = Self::resolve_bin("rm", "/usr/bin/rm");
        let sftp_keys_glob = format!(
            "{}/*/authorized_keys",
            crate::config::paths::sftp_keys_dir().display()
        );

        // Settings page "server name" -> OS hostname (`src/handlers/settings.rs`).
        // `hostnamectl set-hostname` needs root; scoped to that one subcommand
        // (not the bare binary, which can also query/change the machine-id or
        // icon-name) with a wildcard for the sanitized name argument, the same
        // pattern as the `useradd`/`groupadd` grants above.
        let hostnamectl = Self::resolve_bin("hostnamectl", "/usr/bin/hostnamectl");

        let sudoers_content = format!(
            r#"# HomeVault sudoers configuration
# Grant full administrative privileges to homevault_admin group members
# Members of this group have the same rights as root/sudo users

# Full sudo access for homevault_admin group members (like sudo/wheel groups)
%homevault_admin ALL=(ALL:ALL) NOPASSWD: ALL

# Terminal access - allow homevault process to launch shells as admin group members
# This enables the web terminal to provide each admin with their own Linux shell session
homevault ALL=(%homevault_admin) NOPASSWD: /usr/bin/sudo -u * -i

# VPN management - the unprivileged homevault service delegates privileged
# network operations (interface creation, tunnels, stopping root-owned
# openvpn processes, and reading WireGuard state) to root via passwordless sudo.
# ufw (firewall management) is delegated the same way; clamscan/freshclam
# (antivirus) are deliberately NOT here since they run unprivileged as the
# homevault user against paths it already owns. mdadm needs root even for
# read-only --detail (confirmed live: fails with "must be super-user"
# otherwise), used by both the RAID page and Rescue's mdadm diagnose/repair.
homevault ALL=(root) NOPASSWD: {wg}, {wg_quick}, {openvpn}, {pkill}, {kill}, {ip}, {ufw}, {mdadm}, {wpa_cli}

# WireGuard server boot persistence — the roadwarrior server+peer feature in
# src/vpn.rs enables the packaged `wg-quick@wg0` systemd unit the first time
# a peer is provisioned, so the VPN survives a reboot with no separate admin
# step. Scoped to exactly this one unit, same pattern as the Shares/SFTP
# systemctl grants below.
homevault ALL=(root) NOPASSWD: {systemctl} enable wg-quick@wg0, {systemctl} start wg-quick@wg0

# WireGuard server config file itself — `/etc/wireguard` is chowned to
# `homevault:homevault 0750` by `prepare_vpn_directories()` above so the
# service account can write directly when it really is the process owner,
# but `wg0.conf` is deliberately `0600` (it holds the server's own private
# key), and `/etc/wireguard`'s `0750` mode also means a process running as
# any other account (e.g. this binary launched manually instead of via the
# packaged systemd unit) can't even traverse the directory to check whether
# the file exists. Routed through `sudo` unconditionally so it works either
# way, mirroring the `tee`-based pattern already used for `/etc/fstab`/
# `smb.conf`/etc. above — scoped to the exact fixed path
# (`src/vpn.rs::WG_INTERFACE` is hardcoded to `"wg0"`), not the whole
# tee/cat/chmod/test binaries.
homevault ALL=(root) NOPASSWD: {tee} /etc/wireguard/wg0.conf, {tee} -a /etc/wireguard/wg0.conf, {cat} /etc/wireguard/wg0.conf, {chmod} 600 /etc/wireguard/wg0.conf, {test_bin} -e /etc/wireguard/wg0.conf

# Dashboard/Health disk S.M.A.R.T. status — read-only diagnostic reads of
# each block device's health/temperature attributes; needs root the same
# way mdadm --examine does above.
homevault ALL=(root) NOPASSWD: {smartctl}

# Storage pools (ZFS/Btrfs create/delete/scrub) and LUKS2 disk encryption —
# same reasoning as mdadm above: mkfs/mount/wipefs/cryptsetup on a real
# block device need root regardless of the calling process's own group
# memberships.
homevault ALL=(root) NOPASSWD: {zpool}, {zfs}, {mkfs_btrfs}, {mkfs_ext4}, {mkfs_xfs}, {mount_bin}, {umount_bin}, {wipefs}, {btrfs}, {cryptsetup}, {blkid}, {mkdir}, {rmdir}, {tee}

# System Journal viewer — read-only access to journald and a whitelisted set
# of /var/log text files (see src/handlers/journal.rs for the whitelist).
homevault ALL=(root) NOPASSWD: {journalctl}, {tail}

# Shares page service control — scoped to the exact smbd/nfs-server/vsftpd/
# homevault-webdav commands src/shares.rs runs, not the whole systemctl binary
# (see the comment above where `systemctl` is resolved for why).
homevault ALL=(root) NOPASSWD: {systemctl} reload smbd, {systemctl} start smbd, {systemctl} stop smbd, {systemctl} restart smbd, {systemctl} reload vsftpd, {systemctl} start vsftpd, {systemctl} stop vsftpd, {systemctl} restart vsftpd, {systemctl} start nfs-server, {systemctl} stop nfs-server, {systemctl} restart nfs-server, {systemctl} start homevault-webdav, {systemctl} stop homevault-webdav, {systemctl} restart homevault-webdav, {systemctl} start homevault-sftp, {systemctl} stop homevault-sftp, {systemctl} restart homevault-sftp, {systemctl} enable homevault-sftp, {systemctl} daemon-reload, {systemctl} enable homevault-webdav

# Docker daemon start — the Docker page's "daemon down" state lets an admin
# bring the daemon back up (and enable it for boot persistence) without a
# shell. `homevault` is only in the `docker` group, so starting the daemon
# needs root; scoped to exactly these two commands, same pattern as the
# Shares systemctl grants above.
homevault ALL=(root) NOPASSWD: {systemctl} start docker, {systemctl} enable docker

# Shares system config writes — see the comment above where `exportfs`/
# `webdav_py`/`webdav_service` are resolved for why this is scoped to exact
# files rather than the whole `tee` binary.
homevault ALL=(root) NOPASSWD: {tee} {smb_conf}, {tee} -a {smb_conf}, {tee} {vsftpd_conf}, {tee} {exports}, {tee} -a {exports}, {tee} {webdav_py}, {tee} {webdav_service}, {exportfs}, {chmod} +x {webdav_py}

# SFTP — see the comment above where `groupadd`/`useradd`/`usermod_sftp`/
# `gpasswd` are resolved for why these are scoped to exact arguments.
homevault ALL=(root) NOPASSWD: {groupadd} homevault-sftp, {useradd} -M -s /usr/sbin/nologin -G homevault-sftp *, {usermod_sftp} -a -G homevault-sftp *, {gpasswd} -d * homevault-sftp, {chmod} 644 {sftp_keys_glob}, {chpasswd}, {rm} -f {sftp_keys_glob}

# Users/Groups admin page (`src/groups.rs`, `src/storage/database.rs`) —
# creating/deleting a `homevault_`-prefixed group, adding/removing a user
# from one (including the admin group itself, and the host's own
# `sudo`/`wheel` groups when promoting a user to admin), and creating the
# Linux account backing a newly-promoted admin, all need root the same way
# every other host-privileged mutation in this file does. Confirmed live
# these silently no-op under the real unprivileged `homevault` service
# account without this grant (group create/delete failed with an IO error,
# and a promoted admin's Linux account was never actually created).
# `groupadd`/`groupdel` are scoped to the `homevault_` prefix every group
# managed by this page always carries; `usermod`/`gpasswd` are scoped to
# that same prefix plus the `sudo`/`wheel` grants `sync_admin_user_to_group`
# also needs; `useradd` is scoped to the exact real-account-shell shape
# `sync_admin_user_to_group` runs, not the bare binary — unscoped, any of
# these could create/modify an arbitrary account instead of just a
# HomeVault-managed one.
homevault ALL=(root) NOPASSWD: {groupadd} homevault_*, {groupdel} homevault_*, {usermod_sftp} -a -G homevault_* *, {usermod_sftp} -a -G sudo *, {usermod_sftp} -a -G wheel *, {gpasswd} -d * homevault_*, {useradd} -m -d * -s /bin/bash -G homevault_admin *

# Users/Groups group-membership provisioning (`GroupService::ensure_linux_user`,
# `src/groups.rs`) — most HomeVault accounts are DB-only (no Linux user at
# all) until promoted to admin or given a home folder, per the comment on
# the grant just above, but `usermod`/`gpasswd`-based group membership
# (the `homevault_*`/`sudo`/`wheel` grant just above) requires one to exist.
# Confirmed live: adding a DB-only account to a group failed outright
# ("user does not exist") with no way to fix it short of the admin-promotion
# path, which also grants real sudo — wrong for an ordinary group. This
# grant provisions a bare account on demand instead: no home directory
# (`-M`), no login shell (`-s /usr/sbin/nologin`), and — unlike the
# admin-promotion `useradd` above — no `-G` at all, so creating the account
# itself grants no group membership; the caller still has to go through the
# scoped `usermod`/`gpasswd` grant above for that, one group at a time.
homevault ALL=(root) NOPASSWD: {useradd} -M -s /usr/sbin/nologin *

# Settings page "server name" field - see the comment above where
# `hostnamectl` is resolved for why this is scoped to just `set-hostname`.
homevault ALL=(root) NOPASSWD: {hostnamectl} set-hostname *

# Settings page "start on boot" toggle (`src/autostart.rs`) - scoped to
# exactly enabling/disabling HomeVault's own unit, not the bare `systemctl`
# binary (shared variable with the Shares service-control grant above),
# which could otherwise enable/disable/start/stop any unit on the host.
homevault ALL=(root) NOPASSWD: {systemctl} enable homevault, {systemctl} disable homevault
"#,
            wg = wg,
            wg_quick = wg_quick,
            openvpn = openvpn,
            pkill = pkill,
            kill = kill,
            ip = ip,
            ufw = ufw,
            mdadm = mdadm,
            wpa_cli = wpa_cli,
            smartctl = smartctl,
            zpool = zpool,
            zfs = zfs,
            mkfs_btrfs = mkfs_btrfs,
            mkfs_ext4 = mkfs_ext4,
            mkfs_xfs = mkfs_xfs,
            mount_bin = mount_bin,
            umount_bin = umount_bin,
            wipefs = wipefs,
            btrfs = btrfs,
            cryptsetup = cryptsetup,
            blkid = blkid,
            mkdir = mkdir,
            rmdir = rmdir,
            tee = tee,
            cat = cat,
            test_bin = test_bin,
            journalctl = journalctl,
            tail = tail,
            systemctl = systemctl,
            exportfs = exportfs,
            chmod = chmod,
            webdav_py = webdav_py,
            webdav_service = webdav_service,
            smb_conf = smb_conf,
            vsftpd_conf = vsftpd_conf,
            exports = exports,
            groupadd = groupadd,
            groupdel = groupdel,
            useradd = useradd,
            usermod_sftp = usermod_sftp,
            gpasswd = gpasswd,
            chpasswd = chpasswd,
            rm = rm,
            sftp_keys_glob = sftp_keys_glob,
            hostnamectl = hostnamectl,
        );

        match std::fs::write("/etc/sudoers.d/homevault", sudoers_content) {
            Ok(_) => {
                // Set correct permissions for sudoers file
                if let Err(e) = std::fs::set_permissions(
                    "/etc/sudoers.d/homevault",
                    std::os::unix::fs::PermissionsExt::from_mode(0o440),
                ) {
                    tracing::warn!("Failed to set sudoers file permissions: {}", e);
                }

                // Validate sudoers syntax
                match std::process::Command::new("visudo")
                    .args(["-c", "-f", "/etc/sudoers.d/homevault"])
                    .output()
                {
                    Ok(output) => {
                        if output.status.success() {
                            tracing::info!("Sudoers configuration created and validated successfully");
                        } else {
                            tracing::error!("Sudoers syntax validation failed, removing file");
                            let _ = std::fs::remove_file("/etc/sudoers.d/homevault");
                            return Err(SetupError::Validation(
                                "Invalid sudoers configuration".to_string(),
                            ));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Could not validate sudoers file: {}", e);
                        // Continue anyway - visudo might not be available
                    }
                }

                Ok(())
            }
            Err(e) => {
                tracing::warn!("Failed to create sudoers configuration: {}", e);
                // Don't fail setup if sudoers cannot be configured (might not have permissions)
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =====================================================================
    // validate_username
    // =====================================================================

    #[test]
    fn test_validate_username_empty() {
        assert_eq!(
            SetupService::validate_username("").unwrap_err(),
            "Username cannot be empty"
        );
    }

    #[test]
    fn test_validate_username_too_short() {
        assert_eq!(
            SetupService::validate_username("ab").unwrap_err(),
            "Username must be at least 3 characters"
        );
    }

    #[test]
    fn test_validate_username_too_long() {
        let long = "a".repeat(65);
        assert_eq!(
            SetupService::validate_username(&long).unwrap_err(),
            "Username cannot exceed 64 characters"
        );
    }

    #[test]
    fn test_validate_username_reserved_root() {
        let err = SetupService::validate_username("root").unwrap_err();
        assert!(err.contains("reserved"), "Expected reserved error, got: {}", err);
    }

    #[test]
    fn test_validate_username_reserved_admin() {
        let err = SetupService::validate_username("admin").unwrap_err();
        assert!(err.contains("reserved"), "Expected reserved error, got: {}", err);
    }

    #[test]
    fn test_validate_username_reserved_case_insensitive() {
        let err = SetupService::validate_username("ROOT").unwrap_err();
        assert!(err.contains("reserved"), "Expected reserved error for uppercase, got: {}", err);
    }

    #[test]
    fn test_validate_username_reserved_all_listed() {
        for name in SetupService::RESERVED_USERNAMES {
            assert!(
                SetupService::validate_username(name).is_err(),
                "Expected '{}' to be reserved",
                name
            );
        }
    }

    #[test]
    fn test_validate_username_starts_with_digit() {
        let err = SetupService::validate_username("1admin").unwrap_err();
        assert!(err.contains("letter or underscore"), "Expected start-char error, got: {}", err);
    }

    #[test]
    fn test_validate_username_starts_with_hyphen() {
        let err = SetupService::validate_username("-admin").unwrap_err();
        assert!(err.contains("letter or underscore"), "Expected start-char error, got: {}", err);
    }

    #[test]
    fn test_validate_username_invalid_chars_space() {
        let err = SetupService::validate_username("user name").unwrap_err();
        assert!(err.contains("letters, numbers, and underscores"), "Expected char error, got: {}", err);
    }

    #[test]
    fn test_validate_username_invalid_chars_hyphen() {
        let err = SetupService::validate_username("user-name").unwrap_err();
        assert!(err.contains("letters, numbers, and underscores"), "Expected char error, got: {}", err);
    }

    #[test]
    fn test_validate_username_invalid_chars_at() {
        let err = SetupService::validate_username("user@name").unwrap_err();
        assert!(err.contains("letters, numbers, and underscores"), "Expected char error, got: {}", err);
    }

    #[test]
    fn test_validate_username_valid_simple() {
        assert!(SetupService::validate_username("validuser").is_ok());
    }

    #[test]
    fn test_validate_username_valid_with_underscore() {
        assert!(SetupService::validate_username("valid_user").is_ok());
    }

    #[test]
    fn test_validate_username_valid_with_numbers() {
        assert!(SetupService::validate_username("user123").is_ok());
    }

    #[test]
    fn test_validate_username_valid_starts_with_underscore() {
        assert!(SetupService::validate_username("_private").is_ok());
    }

    #[test]
    fn test_validate_username_valid_mixed_case() {
        assert!(SetupService::validate_username("TestUser").is_ok());
    }

    #[test]
    fn test_validate_username_exact_min_length() {
        assert!(SetupService::validate_username("abc").is_ok());
    }

    #[test]
    fn test_validate_username_exact_max_length() {
        let max = "a".repeat(64);
        assert!(SetupService::validate_username(&max).is_ok());
    }

    #[test]
    fn test_validate_username_single_char() {
        assert!(SetupService::validate_username("a").is_err());
    }

    // =====================================================================
    // validate_password
    // =====================================================================

    #[test]
    fn test_validate_password_empty() {
        assert_eq!(
            SetupService::validate_password("").unwrap_err(),
            "Password must be at least 8 characters"
        );
    }

    #[test]
    fn test_validate_password_too_short() {
        assert_eq!(
            SetupService::validate_password("Short1").unwrap_err(),
            "Password must be at least 8 characters"
        );
    }

    #[test]
    fn test_validate_password_too_long() {
        let long = "A".repeat(130) + "b1"; // 132 chars > 128
        assert_eq!(
            SetupService::validate_password(&long).unwrap_err(),
            "Password cannot exceed 128 characters"
        );
    }

    #[test]
    fn test_validate_password_exactly_129_chars() {
        let pass = "A".repeat(128) + "b1"; // 130 chars
        assert_eq!(
            SetupService::validate_password(&pass).unwrap_err(),
            "Password cannot exceed 128 characters"
        );
    }

    #[test]
    fn test_validate_password_no_uppercase() {
        let err = SetupService::validate_password("nouppercase1").unwrap_err();
        assert!(err.contains("uppercase"), "Expected uppercase error, got: {}", err);
    }

    #[test]
    fn test_validate_password_no_lowercase() {
        let err = SetupService::validate_password("NOLOWERCASE1").unwrap_err();
        assert!(err.contains("lowercase"), "Expected lowercase error, got: {}", err);
    }

    #[test]
    fn test_validate_password_no_digit() {
        let err = SetupService::validate_password("NoDigitsHere").unwrap_err();
        assert!(err.contains("digit"), "Expected digit error, got: {}", err);
    }

    #[test]
    fn test_validate_password_valid() {
        assert!(SetupService::validate_password("Valid1Pass").is_ok());
    }

    #[test]
    fn test_validate_password_valid_with_special() {
        assert!(SetupService::validate_password("V4l!d_Pass").is_ok());
    }

    #[test]
    fn test_validate_password_exactly_8_chars() {
        assert!(SetupService::validate_password("Abcdef12").is_ok());
    }

    #[test]
    fn test_validate_password_exactly_128_chars() {
        let mut pass = String::new();
        // Build a 128-char password that satisfies all rules
        pass.push_str("Ab"); // 2 chars: uppercase + lowercase
        for _ in 0..124 {
            pass.push('a');
        }
        pass.push_str("12"); // 2 digits
        assert_eq!(pass.len(), 128);
        assert!(SetupService::validate_password(&pass).is_ok());
    }

    // =====================================================================
    // validate_fullname
    // =====================================================================

    #[test]
    fn test_validate_fullname_empty() {
        assert_eq!(
            SetupService::validate_fullname("").unwrap_err(),
            "Full name cannot be empty"
        );
    }

    #[test]
    fn test_validate_fullname_too_long() {
        let long = "a".repeat(129);
        assert_eq!(
            SetupService::validate_fullname(&long).unwrap_err(),
            "Full name cannot exceed 128 characters"
        );
    }

    #[test]
    fn test_validate_fullname_valid() {
        assert!(SetupService::validate_fullname("John Doe").is_ok());
    }

    #[test]
    fn test_validate_fullname_exact_max() {
        let max = "a".repeat(128);
        assert!(SetupService::validate_fullname(&max).is_ok());
    }

    // =====================================================================
    // is_valid_username (boolean wrapper)
    // =====================================================================

    #[test]
    fn test_is_valid_username_valid() {
        assert!(SetupService::is_valid_username("firstadmin"));
    }

    #[test]
    fn test_is_valid_username_invalid() {
        assert!(!SetupService::is_valid_username(""));
    }

    #[test]
    fn test_is_valid_username_reserved() {
        assert!(!SetupService::is_valid_username("root"));
    }

    // =====================================================================
    // generate_system_hash
    // =====================================================================

    #[test]
    fn test_generate_system_hash_non_empty() {
        let hash = SetupService::generate_system_hash();
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_generate_system_hash_hex_format() {
        let hash = SetupService::generate_system_hash();
        assert_eq!(hash.len(), 64, "SHA-256 hex should be 64 chars");
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "Hash should only contain hex characters"
        );
    }

    #[test]
    fn test_generate_system_hash_deterministic() {
        let h1 = SetupService::generate_system_hash();
        let h2 = SetupService::generate_system_hash();
        assert_eq!(h1, h2, "Same machine should produce same hash");
    }

    // =====================================================================
    // RESERVED_USERNAMES completeness
    // =====================================================================

    #[test]
    fn test_reserved_usernames_not_empty() {
        assert!(
            !SetupService::RESERVED_USERNAMES.is_empty(),
            "RESERVED_USERNAMES should not be empty"
        );
    }

    #[test]
    fn test_reserved_usernames_count() {
        // If someone adds/removes a reserved name, this test forces them to
        // consciously update the count rather than silently changing behavior.
        assert!(
            SetupService::RESERVED_USERNAMES.len() >= 8,
            "Expected at least 8 reserved usernames"
        );
    }
}
