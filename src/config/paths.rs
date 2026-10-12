//! Centralized filesystem paths for HomeVault.
//!
//! All persistent data lives under a single base directory
//! (`/var/lib/homevault` by default, overridable via the `DATA_DIRECTORY`
//! environment variable). Service configuration lives under `/etc/homevault`
//! and personal user homes under `/home/homevault`.
//!
//! Keeping every path in one place makes backups, migration and permission
//! management straightforward and avoids paths scattered across the codebase.

use std::path::PathBuf;

/// Default base directory for all HomeVault data.
pub const DEFAULT_BASE_DIR: &str = "/var/lib/homevault";

/// Directory holding HomeVault service configuration files.
pub const CONFIG_DIR: &str = "/etc/homevault";

/// Base directory for personal user home folders (`/home/homevault/{user}`).
pub const HOMES_BASE_DIR: &str = "/home/homevault";

/// Default mount point prefix for RAID arrays (`/mnt/raid/{name}`).
pub const RAID_MOUNT_BASE: &str = "/mnt/raid";

/// Default mount point prefix for storage pools (`/mnt/pools/{name}`).
pub const POOLS_MOUNT_BASE: &str = "/mnt/pools";

/// Default mount point prefix for encrypted volumes (`/mnt/encrypted/{label}`).
pub const ENCRYPTED_MOUNT_BASE: &str = "/mnt/encrypted";

/// Default mount point prefix for rclone cloud remotes (`/mnt/rclone/{remote}`).
pub const RCLONE_MOUNT_BASE: &str = "/mnt/rclone";

/// Returns the HomeVault base data directory.
/// Overridable with the `DATA_DIRECTORY` environment variable.
pub fn base_dir() -> PathBuf {
    std::env::var("DATA_DIRECTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_BASE_DIR))
}

/// Directory holding the SQLite database and JSON settings storage.
pub fn data_dir() -> PathBuf {
    base_dir().join("data")
}

/// Absolute path of the SQLite database file.
pub fn database_path() -> PathBuf {
    data_dir().join("homevault.db")
}

/// Root directory of Docker Compose projects (one sub-directory per app).
pub fn compose_dir() -> PathBuf {
    base_dir().join("compose")
}

/// Directory for persistent application data of installed Docker apps.
pub fn apps_dir() -> PathBuf {
    base_dir().join("apps")
}

/// Directory holding file-recovery working data.
pub fn rescue_dir() -> PathBuf {
    base_dir().join("rescue")
}

/// Directory holding antivirus scan quarantine files.
pub fn quarantine_dir() -> PathBuf {
    base_dir().join("quarantine")
}

/// Directory holding HomeVault's own rotating application log files, as
/// read/cleared by the System Journal app's "HomeVault Application Log"
/// source (see `src/handlers/journal.rs`).
pub fn logs_dir() -> PathBuf {
    base_dir().join("logs")
}

/// Directory holding the Syncthing configuration.
pub fn syncthing_dir() -> PathBuf {
    base_dir().join("syncthing")
}

/// Directory where the Virtual Machines app keeps ISOs it downloaded (install
/// media, VirtIO drivers). World-readable so the unprivileged qemu can use
/// them without a per-file chmod dance.
pub fn vm_iso_dir() -> PathBuf {
    base_dir().join("vm").join("isos")
}

/// Absolute path of the Syncthing configuration file.
pub fn syncthing_config_path() -> PathBuf {
    syncthing_dir().join("config.xml")
}

/// Lock file preventing concurrent first-run setups.
pub fn setup_lock_file() -> PathBuf {
    base_dir().join(".setup_lock")
}

/// Absolute path of a user's personal home folder (`/home/homevault/{user}`).
pub fn user_home_path(username: &str) -> PathBuf {
    PathBuf::from(HOMES_BASE_DIR).join(username)
}

/// Base directory for SFTP chroot jails and authorized-keys files.
/// Deliberately NOT under `base_dir()`/`DATA_DIRECTORY` — that directory is
/// owned by the unprivileged `homevault` account so it can write to it
/// directly, but OpenSSH's `ChrootDirectory` requires every path component
/// up to `/` to be root-owned and non-group/other-writable, which a
/// homevault-owned directory can never satisfy. `/var/lib` itself is a
/// standard root:root 755 system directory, so a fresh root-owned
/// subdirectory under it (created via `sudo mkdir`, never a direct
/// `std::fs` call) satisfies the strict-mode check.
pub const SFTP_BASE_DIR: &str = "/var/lib/homevault-sftp";

/// Per-user chroot jail roots (`<jail>/<username>/<share_name>` is a bind
/// mount of that share's real path) for the SFTP app's dedicated sshd
/// instance.
pub fn sftp_jails_dir() -> PathBuf {
    PathBuf::from(SFTP_BASE_DIR).join("jails")
}

/// Per-user `authorized_keys` files, read by the dedicated SFTP sshd
/// instance via `AuthorizedKeysFile` — kept outside any chroot jail since
/// OpenSSH resolves authorized keys against the real filesystem, before
/// chrooting the session.
pub fn sftp_keys_dir() -> PathBuf {
    PathBuf::from(SFTP_BASE_DIR).join("keys")
}
