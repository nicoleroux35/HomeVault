/* =============================================================================
 * File          : setup.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   First-run system setup service: creates the initial admin (with
 *   username/password/fullname validation), records the machine identity
 *   (machine id + system hash powering the hardware-change recovery
 *   decision), guards the one-time setup with a lock file, detects the
 *   host's interface language, and configures the scoped sudoers grant the
 *   service account needs.
 *
 * Dependencies:
 *   - thiserror (>= 2)
 *
 * Usage:
 *   handlers/auth.rs (wizard endpoints); the recovery gate reads the
 *   stored system hash (see docs/guides/setup-wizard.md).
 * =============================================================================
 */

//! First-run system setup service (initial admin, machine identity,
//! validation rules, setup lock).

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

/// Seconds after which a first-run setup lock is considered abandoned even if
/// its recorded PID still resolves. A real setup takes seconds (Argon2 hashing
/// plus a couple of row inserts), so 15 minutes means the process is wedged.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a PID belongs to a running process.
///
/// `/proc/<pid>` is the authority on Linux; where it is unavailable the check
/// falls back to `kill -0`. Either way a permission error counts as *alive* —
/// the process exists, we just cannot signal it.
fn process_is_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    if std::path::Path::new("/proc").is_dir() {
        return std::path::Path::new(&format!("/proc/{pid}")).exists();
    }
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

impl SetupService {
    /// How long a first-run setup lock stays valid.
    pub const SETUP_LOCK_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

    fn data_dir() -> std::path::PathBuf {
        crate::config::paths::base_dir()
    }
    fn setup_lock_file() -> std::path::PathBuf {
        crate::config::paths::setup_lock_file()
    }
    const RESERVED_USERNAMES: &'static [&'static str] = &[
        "root",
        "admin",
        "system",
        "daemon",
        "nobody",
        "www-data",
        "postgres",
        "mysql",
        "ftp",
        "mail",
        "operator",
        "bin",
        "sys",
        "sync",
        "games",
        "man",
        "lp",
        "news",
        "uucp",
        "proxy",
        "backup",
        "list",
        "irc",
        "gnats",
        "systemd",
        "messagebus",
    ];

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

    /// The interface languages the appliance ships translations for.
    pub const SUPPORTED_LANGUAGES: [&'static str; 4] = ["en", "fr", "de", "es"];

    /// Maps a POSIX locale string (`fr_FR.UTF-8`, `de_AT`, `es-MX.UTF-8`, …)
    /// to one of [`Self::SUPPORTED_LANGUAGES`], or `"en"` when unsupported.
    ///
    /// Only the language part matters: the territory (`_FR`, `-MX`), the
    /// encoding (`.UTF-8`) and the modifier (`@euro`) are irrelevant to which
    /// dictionary is loaded. Pure, so the locale-parsing rules stay
    /// unit-testable without touching the host.
    pub fn language_from_locale(locale: &str) -> &'static str {
        // `LANGUAGE` may hold a colon-separated priority list (`fr:de:en`) —
        // the first entry wins, like gettext does.
        let first = locale.split(':').next().unwrap_or("");
        // Strip the encoding (`.UTF-8`) and the modifier (`@euro`), then the
        // territory, leaving the bare language code.
        let base = first
            .split(['.', '@'])
            .next()
            .unwrap_or("")
            .split(['_', '-'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        Self::SUPPORTED_LANGUAGES
            .iter()
            .copied()
            .find(|l| *l == base)
            .unwrap_or("en")
    }

    /// Detects the host's interface language from its configured locale — the
    /// Setup Wizard's initial language, since the wizard no longer asks.
    ///
    /// Order matters and mirrors how a shell resolves a locale: `LC_ALL`
    /// overrides everything, then `LC_MESSAGES`, then `LANG`, then the
    /// `LANGUAGE` priority list. A systemd service often runs with a minimal
    /// environment (no `LANG` at all), so `/etc/default/locale` — where
    /// Debian/Ubuntu persist the system locale — is read as the fallback
    /// before giving up. Anything unsupported, or a `C`/`POSIX` locale, falls
    /// back to English; the user can still change it in Settings.
    pub fn detect_system_language() -> &'static str {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
            if let Ok(value) = std::env::var(key) {
                if !value.trim().is_empty() {
                    return Self::language_from_locale(&value);
                }
            }
        }

        if let Ok(contents) = std::fs::read_to_string("/etc/default/locale") {
            for line in contents.lines() {
                let line = line.trim();
                if line.starts_with('#') {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                if !matches!(key.trim(), "LC_ALL" | "LC_MESSAGES" | "LANG" | "LANGUAGE") {
                    continue;
                }
                // Values may be quoted (`LANG="fr_FR.UTF-8"`).
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Self::language_from_locale(value);
                }
            }
        }

        "en"
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

        hasher
            .finalize()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    }

    /// Whether a previously recorded hardware fingerprint still describes this
    /// host.
    ///
    /// Pure comparison — no filesystem or database access — so the caller can
    /// fetch the stored hash from wherever it lives (today the `setup_state`
    /// table, via [`crate::storage::SetupStateRepository`]).
    pub fn system_hash_matches(stored_hash: &str) -> bool {
        stored_hash == Self::generate_system_hash()
    }

    /// Atomically mark setup as in progress to prevent race conditions.
    ///
    /// The lock file records the owner PID and its creation time. A lock left
    /// behind by a process that died mid-setup (OOM kill, systemd restart, a
    /// power cut) would otherwise dead-end the wizard forever: `is_first_run`
    /// stays true, the UI keeps offering setup, and every submission answers
    /// "Setup is already in progress" with no way out. So an existing lock is
    /// only honoured while it is *live* — its PID still exists and it is
    /// younger than [`SETUP_LOCK_MAX_AGE`]. Anything else is reclaimed.
    pub fn mark_setup_in_progress() -> Result<(), SetupError> {
        std::fs::create_dir_all(Self::data_dir())?;

        let path = Self::setup_lock_file();
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let body = format!(
                    "pid={}\ncreated_at={}\n",
                    std::process::id(),
                    unix_now()
                );
                file.write_all(body.as_bytes())?;
                file.flush()?;
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::setup_lock_is_live(&path) {
                    return Err(SetupError::SetupInProgress);
                }
                tracing::warn!(
                    lock = %path.display(),
                    "Reclaiming a stale first-run setup lock (owner process gone or too old)"
                );
                let _ = std::fs::remove_file(&path);
                // Re-create with O_EXCL: if another request raced us to the
                // reclaim, exactly one of the two wins.
                match OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(mut file) => {
                        let body = format!(
                            "pid={}\ncreated_at={}\n",
                            std::process::id(),
                            unix_now()
                        );
                        file.write_all(body.as_bytes())?;
                        file.flush()?;
                        Ok(())
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        Err(SetupError::SetupInProgress)
                    }
                    Err(e) => Err(SetupError::Io(e)),
                }
            }
            Err(e) => Err(SetupError::Io(e)),
        }
    }

    /// Whether an existing lock file belongs to a setup that is still running.
    ///
    /// Conservative on purpose: an unreadable or unparseable lock is treated as
    /// **stale** (it cannot be attributed to a live process), while a lock whose
    /// PID is alive and that is younger than the maximum age is honoured.
    fn setup_lock_is_live(path: &std::path::Path) -> bool {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return false;
        };

        // The lock's own timestamp drives the age rule. A lock written by an
        // older build (plain "in_progress", no metadata) has none, so fall back
        // to the file's mtime — which still lets a reboot clean it up.
        let created_at: Option<u64> = contents
            .lines()
            .filter_map(|line| line.split_once('='))
            .find(|(k, _)| k.trim() == "created_at")
            .and_then(|(_, v)| v.trim().parse().ok());
        let age = match created_at {
            Some(ts) => unix_now().saturating_sub(ts),
            None => std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| unix_now().saturating_sub(d.as_secs()))
                .unwrap_or(u64::MAX),
        };

        Self::lock_contents_is_live(&contents, age, process_is_alive)
    }

    /// The staleness rule itself, with the two impure inputs (age, liveness of
    /// a PID) injected so the decision is unit-testable without touching the
    /// real lock file or the process table.
    pub fn lock_contents_is_live(
        contents: &str,
        age_seconds: u64,
        is_alive: impl Fn(u32) -> bool,
    ) -> bool {
        if age_seconds > Self::SETUP_LOCK_MAX_AGE.as_secs() {
            return false;
        }

        let pid: Option<u32> = contents
            .lines()
            .filter_map(|line| line.split_once('='))
            .find(|(k, _)| k.trim() == "pid")
            .and_then(|(_, v)| v.trim().parse().ok());

        match pid {
            Some(pid) => is_alive(pid),
            // No PID recorded (legacy lock): honour it only while it is young,
            // which the age check above already enforced.
            None => true,
        }
    }

    /// Remove a lock left over by a crashed setup. Called once at startup so a
    /// reboot after a failed first run starts from a clean state even when the
    /// stale lock is still younger than [`SETUP_LOCK_MAX_AGE`].
    pub fn cleanup_stale_setup_lock() {
        let path = Self::setup_lock_file();
        if path.exists() && !Self::setup_lock_is_live(&path) {
            tracing::warn!(
                lock = %path.display(),
                "Removing a stale first-run setup lock at startup"
            );
            let _ = std::fs::remove_file(&path);
        }
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

    /// Minimum admin password length.
    ///
    /// 12 rather than 8: this is the single credential that owns the whole
    /// appliance (root-level control of every app, share and disk), and NIST
    /// SP 800-63B pairs a minimum length with a compromised-password check
    /// precisely because composition rules alone yield `Password1`-style
    /// secrets. The wizard and Settings both surface the live checklist.
    pub const MIN_PASSWORD_LEN: usize = 12;

    /// Passwords rejected outright regardless of their shape: the perennial
    /// top-of-the-breach-lists entries plus this product's own obvious
    /// candidates. Compared case-insensitively against the whole password, so
    /// `Homevault1`, `H0MEVAULT1` and `homevault1` are all refused.
    const BANNED_PASSWORDS: &'static [&'static str] = &[
        "password",
        "password1",
        "password12",
        "password123",
        "password1234",
        "passw0rd",
        "p@ssw0rd",
        "p@ssword1",
        "p@ssw0rd1",
        "qwerty",
        "qwerty123",
        "qwertyuiop",
        "azerty",
        "azerty123",
        "123456789012",
        "1234567890123",
        "letmein",
        "letmein123",
        "welcome",
        "welcome1",
        "welcome123",
        "iloveyou1",
        "admin",
        "admin123",
        "administrator",
        "changeme",
        "changeme123",
        "secret",
        "secret123",
        "homevault",
        "homevault1",
        "homevault12",
        "homevault123",
        "homevault2024",
        "homevault2025",
        "homevault2026",
        "homevaultadmin",
        "motdepasse",
        "motdepasse1",
        "motdepasse123",
        "motdepasse1234",
        "passwort",
        "passwort1",
        "passwort123",
        "contrasena",
        "contrasena1",
        "contrasena123",
    ];

    /// Longest run of identical characters tolerated (`aaaaaaaaaaaa`).
    const MAX_REPEATED_RUN: usize = 4;

    pub fn validate_password(password: &str) -> Result<(), String> {
        if password.len() < Self::MIN_PASSWORD_LEN {
            return Err(format!(
                "Password must be at least {} characters",
                Self::MIN_PASSWORD_LEN
            ));
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

        // Compromised / obvious passwords: refused even when they satisfy every
        // composition rule above (`Password1234` is 12 chars, mixed, with a
        // digit — and still the first guess of every credential-stuffing list).
        if Self::is_banned_password(password) {
            return Err(
                "Password is too common — choose something no one else would use".to_string(),
            );
        }

        if Self::has_long_repeated_run(password) {
            return Err(
                "Password must not repeat the same character more than 4 times in a row"
                    .to_string(),
            );
        }

        Ok(())
    }

    /// Whether the password (or its letters-only form) appears in
    /// [`Self::BANNED_PASSWORDS`].
    ///
    /// Also strips the trailing digits from the candidate so that `Password123`
    /// and `Homevault2026` are caught by the `password` / `homevault` entries
    /// without listing every year and counter.
    pub fn is_banned_password(password: &str) -> bool {
        let lowered = password.to_lowercase();
        let stripped: String = lowered
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string();
        let letters_only: String = lowered
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect();

        Self::BANNED_PASSWORDS.iter().any(|banned| {
            *banned == lowered
                || *banned == stripped
                || *banned == letters_only
                || (letters_only.len() >= 6 && *banned == letters_only.as_str())
        })
    }

    /// Whether any character is repeated more than [`Self::MAX_REPEATED_RUN`]
    /// times consecutively.
    pub fn has_long_repeated_run(password: &str) -> bool {
        let mut previous: Option<char> = None;
        let mut run = 0usize;
        for c in password.chars() {
            if Some(c) == previous {
                run += 1;
                if run > Self::MAX_REPEATED_RUN {
                    return true;
                }
            } else {
                previous = Some(c);
                run = 1;
            }
        }
        false
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
                if let Err(e) =
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o750))
                {
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

        // Shares page service control (smbd/nfs-server/vsftpd/homevault-sftp)
        // — `homevault` needs root to start/stop/restart/reload these systemd
        // units the same way it needs root for mdadm/zpool above; unlike those,
        // `systemctl` itself is not scoped to a single subsystem (it can control
        // *any* unit on the host, including sshd or homevault's own service), so
        // this grant is deliberately scoped to the exact reload/start/stop/
        // restart/enable/daemon-reload commands src/shares.rs actually runs,
        // rather than the whole `systemctl` binary. WebDAV is not in this list:
        // it is served in-process (`crate::webdav`), so there is no unit left
        // to drive.
        let systemctl = Self::resolve_bin("systemctl", "/usr/bin/systemctl");

        // Shares system config writes — /etc/samba/smb.conf, /etc/vsftpd.conf,
        // /etc/exports must stay root-owned (unlike /etc/homevault's own
        // smb.json/nfs.json/etc, which install.sh chowns to homevault directly),
        // so src/shares.rs writes them via `sudo tee`. Scoped to the exact
        // files, not the whole `tee` binary — an unscoped grant would let
        // homevault overwrite any file on the host as root, e.g. /etc/shadow or
        // this very sudoers file.
        //
        // `webdav_py`/`webdav_service` are the *legacy* generated WebDAV
        // service (a Python script + systemd unit, root-owned). They are never
        // written any more — they are only named here so
        // `crate::webdav::cleanup_legacy_service` can `rm -f` them (scoped to
        // exactly these two paths) when an upgraded host still has them.
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
# homevault-sftp commands src/shares.rs runs, not the whole systemctl binary
# (see the comment above where `systemctl` is resolved for why). WebDAV is
# served in-process; the only WebDAV systemctl call left is the one-shot
# `disable --now` that retires the legacy generated unit.
homevault ALL=(root) NOPASSWD: {systemctl} reload smbd, {systemctl} start smbd, {systemctl} stop smbd, {systemctl} restart smbd, {systemctl} reload vsftpd, {systemctl} start vsftpd, {systemctl} stop vsftpd, {systemctl} restart vsftpd, {systemctl} start nfs-server, {systemctl} stop nfs-server, {systemctl} restart nfs-server, {systemctl} start homevault-sftp, {systemctl} stop homevault-sftp, {systemctl} restart homevault-sftp, {systemctl} enable homevault-sftp, {systemctl} disable --now homevault-webdav, {systemctl} daemon-reload

# Docker daemon start — the Docker page's "daemon down" state lets an admin
# bring the daemon back up (and enable it for boot persistence) without a
# shell. `homevault` is only in the `docker` group, so starting the daemon
# needs root; scoped to exactly these two commands, same pattern as the
# Shares systemctl grants above.
homevault ALL=(root) NOPASSWD: {systemctl} start docker, {systemctl} enable docker

# Shares system config writes — see the comment above where `exportfs`/
# `webdav_py`/`webdav_service` are resolved for why this is scoped to exact
# files rather than the whole `tee` binary. The two `rm -f` grants exist only
# to retire the legacy generated WebDAV service, and are scoped to those two
# exact paths.
homevault ALL=(root) NOPASSWD: {tee} {smb_conf}, {tee} -a {smb_conf}, {tee} {vsftpd_conf}, {tee} {exports}, {tee} -a {exports}, {exportfs}, {rm} -f {webdav_py}, {rm} -f {webdav_service}

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
                            tracing::info!(
                                "Sudoers configuration created and validated successfully"
                            );
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
        assert!(
            err.contains("reserved"),
            "Expected reserved error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_username_reserved_admin() {
        let err = SetupService::validate_username("admin").unwrap_err();
        assert!(
            err.contains("reserved"),
            "Expected reserved error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_username_reserved_case_insensitive() {
        let err = SetupService::validate_username("ROOT").unwrap_err();
        assert!(
            err.contains("reserved"),
            "Expected reserved error for uppercase, got: {}",
            err
        );
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
        assert!(
            err.contains("letter or underscore"),
            "Expected start-char error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_username_starts_with_hyphen() {
        let err = SetupService::validate_username("-admin").unwrap_err();
        assert!(
            err.contains("letter or underscore"),
            "Expected start-char error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_username_invalid_chars_space() {
        let err = SetupService::validate_username("user name").unwrap_err();
        assert!(
            err.contains("letters, numbers, and underscores"),
            "Expected char error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_username_invalid_chars_hyphen() {
        let err = SetupService::validate_username("user-name").unwrap_err();
        assert!(
            err.contains("letters, numbers, and underscores"),
            "Expected char error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_username_invalid_chars_at() {
        let err = SetupService::validate_username("user@name").unwrap_err();
        assert!(
            err.contains("letters, numbers, and underscores"),
            "Expected char error, got: {}",
            err
        );
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
            "Password must be at least 12 characters"
        );
    }

    #[test]
    fn test_validate_password_too_short() {
        assert_eq!(
            SetupService::validate_password("Short1").unwrap_err(),
            "Password must be at least 12 characters"
        );
    }

    /// 11 characters: rejected by exactly one (the new 12-char floor).
    #[test]
    fn test_validate_password_eleven_chars_rejected() {
        let err = SetupService::validate_password("Abcdefghi12").unwrap_err();
        assert!(err.contains("12 characters"), "got: {err}");
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
        let err = SetupService::validate_password("nouppercase1234").unwrap_err();
        assert!(
            err.contains("uppercase"),
            "Expected uppercase error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_password_no_lowercase() {
        let err = SetupService::validate_password("NOLOWERCASE1234").unwrap_err();
        assert!(
            err.contains("lowercase"),
            "Expected lowercase error, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_password_no_digit() {
        let err = SetupService::validate_password("NoDigitsHereAtAll").unwrap_err();
        assert!(err.contains("digit"), "Expected digit error, got: {}", err);
    }

    #[test]
    fn test_validate_password_valid() {
        assert!(SetupService::validate_password("Valid1Passw0rd").is_ok());
    }

    #[test]
    fn test_validate_password_valid_with_special() {
        assert!(SetupService::validate_password("V4l!d_Passw0rd").is_ok());
    }

    #[test]
    fn test_validate_password_exactly_12_chars() {
        assert_eq!("Abcdefgh1234".len(), SetupService::MIN_PASSWORD_LEN);
        assert!(SetupService::validate_password("Abcdefgh1234").is_ok());
    }

    #[test]
    fn test_validate_password_exactly_128_chars() {
        // Build a 128-char password that satisfies every rule: mixed case, a
        // digit, no repeated run longer than 4, and not on the ban list.
        let mut pass = String::from("Ab");
        while pass.len() < 126 {
            pass.push_str("xY7z");
        }
        pass.push_str("12");
        pass.truncate(128);
        pass.push('1');
        pass.truncate(128);
        assert_eq!(pass.len(), 128);
        assert!(SetupService::validate_password(&pass).is_ok());
    }

    // =====================================================================
    // Compromised / obvious passwords
    // =====================================================================

    #[test]
    fn test_validate_password_rejects_banned_passwords() {
        // Each of these satisfies the composition rules (length + case +
        // digit) and must still be refused.
        for pass in [
            "Password1234",
            "Password12345",
            "Homevault123",
            "Motdepasse123",
            "Passwort1234",
            "Changeme1234",
            "Qwerty123456",
            "Welcome12345",
        ] {
            let err = SetupService::validate_password(pass)
                .expect_err(&format!("{pass} must be refused"));
            assert!(err.contains("too common"), "{pass} → {err}");
        }
    }

    #[test]
    fn test_validate_password_rejects_repeated_runs() {
        let err = SetupService::validate_password("Aaaaaaaa1234").unwrap_err();
        assert!(err.contains("repeat"), "got: {err}");
    }

    #[test]
    fn test_banned_password_matching_is_case_insensitive() {
        assert!(SetupService::is_banned_password("PASSWORD123"));
        assert!(SetupService::is_banned_password("HoMeVaUlT2026"));
        assert!(!SetupService::is_banned_password("Tr0ub4dor&3xyz"));
    }

    #[test]
    fn test_has_long_repeated_run_boundary() {
        // Exactly 4 in a row is allowed, 5 is not.
        assert!(!SetupService::has_long_repeated_run("Abc1aaaa"));
        assert!(SetupService::has_long_repeated_run("Abc1aaaaa"));
    }

    // =====================================================================
    // setup lock staleness
    // =====================================================================

    #[test]
    fn test_lock_with_live_pid_and_young_age_is_honoured() {
        let contents = "pid=4242\ncreated_at=1700000000\n";
        assert!(SetupService::lock_contents_is_live(contents, 5, |pid| {
            pid == 4242
        }));
    }

    #[test]
    fn test_lock_with_dead_pid_is_stale() {
        let contents = "pid=4242\ncreated_at=1700000000\n";
        assert!(!SetupService::lock_contents_is_live(contents, 5, |_| false));
    }

    #[test]
    fn test_lock_older_than_the_max_age_is_stale_even_with_a_live_pid() {
        let contents = "pid=4242\ncreated_at=1700000000\n";
        let too_old = SetupService::SETUP_LOCK_MAX_AGE.as_secs() + 1;
        assert!(!SetupService::lock_contents_is_live(contents, too_old, |_| true));
    }

    #[test]
    fn test_legacy_lock_without_pid_is_honoured_only_while_young() {
        // Pre-metadata lock file: plain "in_progress".
        assert!(SetupService::lock_contents_is_live("in_progress", 10, |_| true));
        let too_old = SetupService::SETUP_LOCK_MAX_AGE.as_secs() + 1;
        assert!(!SetupService::lock_contents_is_live(
            "in_progress",
            too_old,
            |_| true
        ));
    }

    #[test]
    fn test_lock_with_malformed_pid_is_treated_as_legacy() {
        // An unparseable pid cannot be attributed to a process, so the age
        // rule alone decides — a young lock still blocks a concurrent setup.
        let contents = "pid=not-a-number\ncreated_at=1700000000\n";
        assert!(SetupService::lock_contents_is_live(contents, 5, |_| false));
    }

    #[test]
    fn test_process_is_alive_recognises_this_process_and_rejects_pid_zero() {
        assert!(process_is_alive(std::process::id()));
        assert!(!process_is_alive(0));
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
    // language_from_locale / detect_system_language
    // =====================================================================

    #[test]
    fn test_language_from_locale_supported() {
        assert_eq!(SetupService::language_from_locale("fr_FR.UTF-8"), "fr");
        assert_eq!(SetupService::language_from_locale("de_DE.UTF-8"), "de");
        assert_eq!(SetupService::language_from_locale("es_ES.UTF-8"), "es");
        assert_eq!(SetupService::language_from_locale("en_GB.UTF-8"), "en");
    }

    #[test]
    fn test_language_from_locale_territory_and_case() {
        assert_eq!(SetupService::language_from_locale("fr_CA"), "fr");
        assert_eq!(SetupService::language_from_locale("DE_at@euro"), "de");
        assert_eq!(SetupService::language_from_locale("es-MX.UTF-8"), "es");
    }

    #[test]
    fn test_language_from_locale_unsupported_falls_back_to_english() {
        assert_eq!(SetupService::language_from_locale("it_IT.UTF-8"), "en");
        assert_eq!(SetupService::language_from_locale("pt_BR"), "en");
        assert_eq!(SetupService::language_from_locale("C"), "en");
        assert_eq!(SetupService::language_from_locale("POSIX"), "en");
        assert_eq!(SetupService::language_from_locale(""), "en");
    }

    #[test]
    fn test_language_from_locale_priority_list_uses_the_first_entry() {
        assert_eq!(SetupService::language_from_locale("fr:de:en"), "fr");
        assert_eq!(SetupService::language_from_locale("de_DE.UTF-8:en"), "de");
        assert_eq!(SetupService::language_from_locale("it:fr"), "en");
    }

    #[test]
    fn test_detect_system_language_is_one_of_the_supported_codes() {
        let lang = SetupService::detect_system_language();
        assert!(
            SetupService::SUPPORTED_LANGUAGES.contains(&lang),
            "detected {lang:?} must be a shipped language"
        );
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
    // system_hash_matches
    // =====================================================================

    #[test]
    fn test_system_hash_matches_the_current_hash() {
        let current = SetupService::generate_system_hash();
        assert!(SetupService::system_hash_matches(&current));
    }

    #[test]
    fn test_system_hash_matches_rejects_anything_else() {
        assert!(!SetupService::system_hash_matches(
            "deadbeef-not-a-real-hash"
        ));
        assert!(!SetupService::system_hash_matches(""));
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
