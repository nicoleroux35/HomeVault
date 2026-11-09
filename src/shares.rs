/* =============================================================================
 * File          : shares.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Network share service: share CRUD over the SQLite store, plus
 *   generation and application of the per-protocol service configuration
 *   (SMB/NFS/FTP/WebDAV config files) and service start/stop/restart.
 *
 * Dependencies:
 *   - serde (>= 1.0), internal storage layer
 *
 * Usage:
 *   handlers/shares.rs, handlers/permissions.rs (share lookup), SFTP jail
 *   rebuilds (see docs/guides/shares.md).
 * =============================================================================
 */

//! Network share service (CRUD + per-protocol config generation and
//! service control).

use crate::storage::atomic::AtomicWriter;
use crate::utils::process::{run_timed, run_timed_with_stdin};
use serde::{Deserialize, Serialize};
use serde_json::Error as JsonError;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::warn;

/// Serializes read-modify-write cycles on the system configuration files
/// (`smb.conf`, `/etc/exports`). Appending a share used to be a bare
/// `tee -a`; replacing the whole file now needs the same mutual exclusion or
/// two concurrent saves could lose one another's section.
static CONFIG_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Names the throwaway file `testparm` validates, so two saves racing on the
/// same box never read each other's candidate.
static VALIDATION_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Deadlines (ROADMAP X.2)
//
// Every host command of the shares module used to be a bare `Command::output`
// (or a hand-rolled `.spawn` + `wait_with_output` for `sudo tee`). A
// `systemctl reload smbd` blocked on a wedged SMB stack, an `exportfs -ra`
// parked on an unresponsive NFS export or a `sudo tee` on a full/hung console
// pinned the calling thread with no ceiling. Each value below is a ceiling for
// the slowest honest case, not an estimate.
// ---------------------------------------------------------------------------

/// A root-owned config write: `sudo tee`/`sudo tee -a`, `sudo mkdir -p`.
pub const SHARES_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// A service control: `systemctl start/stop/restart/reload` and
/// `exportfs -ra`. Longer than a write because a real service start can wait
/// on its dependencies, while staying finite.
pub const SHARES_SERVICE_TIMEOUT: Duration = Duration::from_secs(60);

/// Run one host command under a deadline, keeping the reason it failed.
///
/// The shares module is synchronous by design, so this is the blocking
/// helper. The timeout is a parameter rather than a constant baked into each
/// call site so the kill path is unit-testable, exactly like
/// `disk::bounded` and `cache::run_command`.
fn run_command(cmd: &mut Command, timeout: Duration) -> Result<std::process::Output, String> {
    run_timed(cmd, timeout)
}

/// Errors that can occur when managing file shares.
#[derive(Error, Debug)]
pub enum ShareError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] JsonError),
    #[error("Share not found: {0}")]
    NotFound(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Config file not found: {0}")]
    ConfigNotFound(String),
    #[error("{0}")]
    InvalidField(String),
    #[error("{0}")]
    Unavailable(String),
}

/// A file share definition, applicable across SMB, NFS, FTP, and WebDAV.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    pub name: String,
    pub share_type: ShareType,
    pub path: String,
    pub enabled: bool,
    pub comment: String,
    pub options: Vec<String>,
    /// SMB only: the share is exported read-only regardless of the POSIX
    /// write bits. `false` for every other protocol.
    #[serde(default)]
    pub read_only: bool,
    /// SMB only: explicit per-share guest access. `None` means "inherit the
    /// global `allow_guest` setting"; `Some(false)` is a deliberate per-share
    /// opt-out, which matters because the global setting otherwise opens
    /// every share at once.
    #[serde(default)]
    pub guest_ok: Option<bool>,
    /// Per-share advanced restrictions (browse / modify / download). They are
    /// enforced by the web Explorer, WebDAV and FTP; SMB has no portable
    /// equivalent, so the SMB client only sees the POSIX/ACL result.
    #[serde(default)]
    pub restrictions: ShareRestrictions,
}

/// Optional per-share restrictions DSM calls "Advanced Share Permissions".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRestrictions {
    /// Refuse directory listing (Explorer/WebDAV/FTP `dirlist_enable=NO`).
    #[serde(default)]
    pub disable_browsing: bool,
    /// Refuse PUT/DELETE/MOVE/COPY and FTP writes.
    #[serde(default)]
    pub disable_modification: bool,
    /// Refuse file downloads (Explorer/WebDAV GET, FTP `download_enable=NO`).
    #[serde(default)]
    pub disable_download: bool,
}

/// Fields that can be updated on an existing share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareUpdate {
    pub path: Option<String>,
    pub enabled: Option<bool>,
    pub comment: Option<String>,
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub read_only: Option<bool>,
    /// `Some(Some(value))` sets an explicit per-share value, `Some(None)`
    /// clears it back to "inherit the global setting", and the field being
    /// absent leaves it untouched.
    #[serde(default)]
    pub guest_ok: Option<Option<bool>>,
    #[serde(default)]
    pub restrictions: Option<ShareRestrictions>,
}

/// A share with additional metadata (creation date, usage, access count).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareWithMeta {
    #[serde(flatten)]
    pub share: Share,
    pub created_at: Option<String>,
    pub disk_usage: Option<u64>,
    pub access_count: Option<u32>,
}

/// Supported network file-sharing protocols.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ShareType {
    Smb,
    Nfs,
    Ftp,
    Webdav,
    /// Real SSH File Transfer Protocol, served by a dedicated `sshd`
    /// instance with per-user chroot jails — distinct from `Ftp` (vsftpd),
    /// which only ever spoke FTP/FTPS, never the SSH-based SFTP protocol.
    Sftp,
}

impl ShareType {
    /// Return the protocol string (`"smb"`, `"nfs"`, `"ftp"`, `"webdav"`, `"sftp"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ShareType::Smb => "smb",
            ShareType::Nfs => "nfs",
            ShareType::Ftp => "ftp",
            ShareType::Webdav => "webdav",
            ShareType::Sftp => "sftp",
        }
    }

    /// Parse a `ShareType` from a string. Accepts common aliases (e.g. `"cifs"` → SMB).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "smb" | "cifs" => Some(ShareType::Smb),
            "nfs" => Some(ShareType::Nfs),
            "ftp" => Some(ShareType::Ftp),
            "webdav" => Some(ShareType::Webdav),
            "sftp" => Some(ShareType::Sftp),
            _ => None,
        }
    }
}

impl std::fmt::Display for ShareType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Global SMB/CIFS service configuration persisted to `/etc/homevault/smb.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmbConfig {
    pub workgroup: String,
    pub security_mode: String,
    /// AD/Kerberos realm (`EXAMPLE.LAN`) used by `security = ads`.
    #[serde(default)]
    pub realm: String,
    /// `idmap config * : range` for winbind.
    #[serde(default = "default_idmap_range")]
    pub idmap_range: String,
    pub protocol_version: String,
    pub allow_guest: bool,
    pub create_mask: String,
    pub directory_mask: String,
    pub additional_params: Vec<String>,
    /// Samba `server multi channel support`. Off by default: it only helps
    /// when the client, the switch and at least two NICs/queues agree, and it
    /// is harmless to leave it off on a single-link appliance.
    #[serde(default)]
    pub multichannel: bool,
    /// Advertise this appliance to Windows Explorer through WS-Discovery
    /// (`wsdd2`). Not a Samba directive: it is applied by
    /// `crate::ws_discovery`, so `smb_global_params` deliberately ignores it.
    /// On by default, like DSM's Windows network discovery; harmless when
    /// `wsdd2` is not installed (the status card says so).
    #[serde(default = "default_ws_discovery")]
    pub ws_discovery: bool,
    /// SMB2 durable handles: a client that loses its TCP connection can
    /// reconnect and keep its open files. Off by default because enabling it
    /// disables cross-protocol file locking (AFP/SMB arbitration).
    #[serde(default)]
    pub durable_handles: bool,
    /// `server smb encrypt = off | desired | required` (SMB3 transport
    /// encryption). `desired` protects clients that support it without
    /// excluding older ones; `required` excludes clients that do not.
    #[serde(default = "default_smb_encrypt")]
    pub smb_encrypt: String,
    /// `server signing = off | desired | mandatory`. Signing protects packet
    /// integrity; `mandatory` is the only value the Security Advisor accepts.
    #[serde(default = "default_server_signing")]
    pub server_signing: String,
    /// `strict sync = yes`: flush every SMB write to disk before acknowledging
    /// it. Safer for databases, slower for bulk copies.
    #[serde(default)]
    pub strict_sync: bool,
    /// `aio read size` in bytes (`0` = leave Samba's default). `1` makes every
    /// read asynchronous, which is what DSM's "Enable asynchronous read" does.
    #[serde(default)]
    pub aio_read_size: u32,
    /// `aio write size` in bytes (`0` = leave Samba's default).
    #[serde(default)]
    pub aio_write_size: u32,
    /// `use sendfile = yes` (Samba's default). Disabling it can help on
    /// filesystems where `sendfile` is slower than a buffered copy.
    #[serde(default = "default_true")]
    pub use_sendfile: bool,
    /// `oplocks = yes` (Samba's default): client-side caching.
    #[serde(default = "default_true")]
    pub oplocks: bool,
    /// `kernel oplocks = yes` (Samba's default): kernel-assisted leases.
    #[serde(default = "default_true")]
    pub kernel_oplocks: bool,
    /// `level2 oplocks = yes` (Samba's default): read-only shared caching.
    #[serde(default = "default_true")]
    pub level2_oplocks: bool,
}

fn default_idmap_range() -> String {
    "10000-999999".to_string()
}

fn default_ws_discovery() -> bool {
    true
}

fn default_smb_encrypt() -> String {
    "off".to_string()
}

fn default_server_signing() -> String {
    "off".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for SmbConfig {
    fn default() -> Self {
        Self {
            workgroup: "WORKGROUP".to_string(),
            security_mode: "user".to_string(),
            realm: String::new(),
            idmap_range: default_idmap_range(),
            protocol_version: "SMB3".to_string(),
            allow_guest: false,
            create_mask: "0755".to_string(),
            directory_mask: "0755".to_string(),
            additional_params: Vec::new(),
            multichannel: false,
            ws_discovery: true,
            durable_handles: false,
            smb_encrypt: default_smb_encrypt(),
            server_signing: default_server_signing(),
            strict_sync: false,
            aio_read_size: 0,
            aio_write_size: 0,
            use_sendfile: true,
            oplocks: true,
            kernel_oplocks: true,
            level2_oplocks: true,
        }
    }
}

/// Global NFS service configuration persisted to `/etc/homevault/nfs.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NfsConfig {
    pub networks: Vec<String>,
    pub default_access: String,
    pub version: String,
    pub root_squash: String,
    /// `sys` (AUTH_SYS), or a Kerberos flavor (`krb5`, `krb5i`, `krb5p`).
    /// A non-`sys` value is rendered as `sec=…` and only accepted with an
    /// explicit NFSv4 version, because `auto` can still negotiate v3.
    #[serde(default = "default_nfs_security_flavor")]
    pub security_flavor: String,
    pub custom_options: Vec<String>,
}

fn default_nfs_security_flavor() -> String {
    "sys".to_string()
}

impl Default for NfsConfig {
    fn default() -> Self {
        Self {
            networks: vec!["*".to_string()],
            default_access: "rw".to_string(),
            // `auto` keeps the pre-existing kernel-negotiated behaviour for
            // installs that never touched the field; an explicit 3/4/4.2 is
            // written as `vers=` and therefore enforced.
            version: "auto".to_string(),
            root_squash: "root_squash".to_string(),
            security_flavor: default_nfs_security_flavor(),
            custom_options: Vec::new(),
        }
    }
}

/// One NFS client rule attached to a share (DSM's per-share NFS permission).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NfsClientRule {
    /// `*`, a CIDR, an IP or a host name. Validated as an `/etc/exports`
    /// client clause before it reaches the file.
    pub client: String,
    /// `rw` or `ro`. It overrides the share/service default for that client.
    pub access: String,
    /// Extra export options for this client only (`async`, `crossmnt`,
    /// `insecure`, `sec=krb5`, …).
    #[serde(default)]
    pub options: Vec<String>,
}

/// Global FTP service configuration persisted to `/etc/homevault/ftp.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FtpConfig {
    pub port: u16,
    pub allowed_users: Vec<String>,
    pub passive_ports: String,
    pub max_clients: u32,
    pub force_tls: bool,
    pub allow_anonymous: bool,
    /// Directory the anonymous user is chrooted to (DSM's "anonymous root").
    /// `None` keeps the historical vsftpd default (the `ftp` account's home).
    #[serde(default)]
    pub anonymous_root: Option<String>,
    pub chroot: bool,
    pub welcome_message: String,
}

/// Global WebDAV service configuration persisted to `/etc/homevault/webdav.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebdavConfig {
    pub port: u16,
    pub path_prefix: String,
    pub require_ssl: bool,
    pub require_auth: bool,
    pub compression: bool,
    pub locking: bool,
    pub allowed_methods: Vec<String>,
}

impl Default for WebdavConfig {
    /// A usable, **closed** default: a real port (the old derived default was
    /// `0`, which cannot be bound) and authentication ON — an appliance must
    /// never expose its shares because the config file happened to be absent.
    fn default() -> Self {
        Self {
            port: 8082,
            path_prefix: "/".to_string(),
            require_ssl: false,
            require_auth: true,
            compression: false,
            locking: true,
            allowed_methods: Vec::new(),
        }
    }
}

/// Global SFTP service configuration persisted to `/etc/homevault/sftp.json`.
/// Applying it regenerates the dedicated `homevault-sftp` sshd instance's
/// config file (see `crate::sftp`) — deliberately a *separate* sshd process
/// on its own port, never the host's own system sshd, so this feature can
/// never affect a real admin's shell/SSH access to the box.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpConfig {
    pub port: u16,
}

impl Default for SftpConfig {
    fn default() -> Self {
        Self { port: 2222 }
    }
}

fn get_config_dir() -> PathBuf {
    PathBuf::from(crate::config::paths::CONFIG_DIR)
}

/// Environment override for the Samba configuration file.
///
/// Same seam as `HV_AVAHI_CONF` (`src/mdns.rs`): a test can point the
/// generation/parse path at a temp file and prove what lands on disk without
/// touching `/etc`. Unset in production, where the compiled-in path is the
/// only correct one. Setting it also tells the mutating helpers not to bounce
/// a real service — see [`smb_conf_is_overridden`].
pub const SMB_CONF_ENV: &str = "HV_SMB_CONF";

/// The `smb.conf` this process reads and edits.
pub fn smb_conf_path() -> String {
    std::env::var(SMB_CONF_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/etc/samba/smb.conf".to_string())
}

/// Whether the config path is a test override (never the host's real file).
fn smb_conf_is_overridden() -> bool {
    smb_conf_path() != "/etc/samba/smb.conf"
}

fn get_smb_config_path() -> PathBuf {
    get_config_dir().join("smb.json")
}

fn get_nfs_config_path() -> PathBuf {
    get_config_dir().join("nfs.json")
}

fn get_ftp_config_path() -> PathBuf {
    get_config_dir().join("ftp.json")
}

fn get_nfs_rules_path() -> PathBuf {
    get_config_dir().join("nfs_rules.json")
}

fn get_webdav_config_path() -> PathBuf {
    get_config_dir().join("webdav.json")
}

fn get_ftp_shares_path() -> PathBuf {
    get_config_dir().join("ftp_shares.json")
}

fn get_webdav_shares_path() -> PathBuf {
    get_config_dir().join("webdav_shares.json")
}

fn get_sftp_config_path() -> PathBuf {
    get_config_dir().join("sftp.json")
}

fn get_sftp_shares_path() -> PathBuf {
    get_config_dir().join("sftp_shares.json")
}

/// Overwrites a root-owned system config file (`/etc/samba/smb.conf`,
/// `/etc/vsftpd.conf`, `/etc/exports`, the WebDAV service/script) via
/// `sudo tee`, since the unprivileged homevault account can't write these
/// directly. Same pattern as `pools.rs`/`encryption.rs`'s `write_file_as_root`.
pub(crate) fn write_file_as_root(path: &str, content: &str) -> std::io::Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.args(["tee", path]);
    // `run_timed_with_stdin` feeds the content from its own thread and closes
    // stdin afterwards (which is what `tee` waits for); a `tee` that never
    // reads must still be killed at the deadline, so this cannot stay a bare
    // `.spawn` + `wait_with_output`.
    let output = run_timed_with_stdin(&mut cmd, SHARES_WRITE_TIMEOUT, content.as_bytes())
        .map_err(std::io::Error::other)?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(())
}

/// Write a configuration file that this process may already be allowed to
/// write.
///
/// Production writes atomically (temporary file in the same directory,
/// `fsync`, `rename`, parent `fsync`). When the file is root-owned and the
/// process is not root, the write falls back to the existing `sudo tee` grant
/// — the same privileged path as before, but only as a fallback.
fn write_config_file(path: &str, content: &str) -> std::io::Result<()> {
    let _guard = CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    write_config_file_unlocked(path, content)
}

/// The body of [`write_config_file`], with the lock already held. Kept
/// separate so [`append_config_file`] can do its read-modify-write under the
/// same guard without deadlocking on a non-reentrant mutex.
fn write_config_file_unlocked(path: &str, content: &str) -> std::io::Result<()> {
    // Keep the last good version next to the file. Best effort: a read-only
    // parent or a test path must not turn a successful write into a failure.
    if let Ok(previous) = fs::read_to_string(path) {
        if !previous.is_empty() {
            let backup = format!("{path}.bak");
            if AtomicWriter::write(&backup, &previous).is_err() {
                let _ = write_file_as_root(&backup, &previous);
            }
        }
    }

    match AtomicWriter::write(path, content) {
        Ok(()) => Ok(()),
        Err(e) if e.is_permission_denied() => write_file_as_root(path, content),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    }
}

/// Append to a configuration file this process may already be allowed to
/// write — the append counterpart of [`write_config_file`].
///
/// Appending in place cannot be atomic: a crash between two `write` calls
/// leaves a truncated line. The file is therefore read, extended, and
/// replaced atomically as a whole, under the same lock as a full rewrite.
fn append_config_file(path: &str, content: &str) -> std::io::Result<()> {
    let _guard = CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut current = fs::read_to_string(path).unwrap_or_default();
    current.push_str(content);
    write_config_file_unlocked(path, &current)
}

/// Ask `smbd` to re-read its configuration.
///
/// Skipped when [`SMB_CONF_ENV`] points somewhere other than the host's real
/// file: an overridden path means the caller is editing a copy, and bouncing
/// the production service from a test would be both surprising and
/// unnecessary.
fn reload_smbd(context: &str) {
    if smb_conf_is_overridden() {
        return;
    }
    let mut cmd = Command::new("sudo");
    cmd.arg("systemctl");
    cmd.args(["reload", "smbd"]);
    if let Err(e) = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT).map(|o| o.status.success()) {
        warn!("Failed to reload smbd after {}: {}", context, e);
    }
}

/// Appends to a root-owned system config file (`/etc/samba/smb.conf`,
/// `/etc/exports`) via `sudo tee -a`, for call sites that only need to add a
/// new share section/export line rather than rewrite the whole file.
pub(crate) fn append_file_as_root(path: &str, content: &str) -> std::io::Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.args(["tee", "-a", path]);
    let output = run_timed_with_stdin(&mut cmd, SHARES_WRITE_TIMEOUT, content.as_bytes())
        .map_err(std::io::Error::other)?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(())
}

/// Create a root-owned directory (and parents) via `sudo mkdir -p`.
///
/// The HomeVault config/state dir (`CONFIG_DIR`, e.g. `/etc/homevault`) is
/// created root-owned by the installer; the SFTP/FTP/WebDAV share-state
/// JSON files under it were being written with a plain `fs::write`, which
/// fails outright (`EPERM`) whenever the service isn't running as root —
/// inconsistent with every `/etc` write elsewhere in this module.
pub(crate) fn mkdir_as_root(dir: &std::path::Path) -> std::io::Result<()> {
    let mut cmd = Command::new("sudo");
    cmd.args(["mkdir", "-p", &dir.to_string_lossy()]);
    let output = run_command(&mut cmd, SHARES_WRITE_TIMEOUT).map_err(std::io::Error::other)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Input validation — pure helpers
// ---------------------------------------------------------------------------
//
// Every value that reaches a root-owned config file (`smb.conf`, `/etc/exports`,
// `vsftpd.conf`, `/etc/fstab`) is validated *before* it is formatted into that
// file. These are format-injection guards: a share name with a newline could
// forge extra Samba sections, an NFS option containing `)` could fabricate
// extra client lists in `/etc/exports`, a path with whitespace could inject
// extra columns into `/etc/exports` or `/etc/fstab`. Validators return
// `Result<(), String>` so `sftp.rs` can reuse them for its fstab/jail paths.

/// Samba-reserved section names that may never be used as a share name —
/// deleting or rewriting such a section would corrupt `smb.conf` itself.
const RESERVED_SMB_SECTION_NAMES: [&str; 4] = ["global", "homes", "printers", "admin$"];

/// Tag written in `/etc/exports` in front of a disabled NFS export line:
/// `# homevault-nfs-disabled:<name> /path *(options)`. Unlike the old
/// `exportfs -u` toggle (an in-memory-only change that silently came back on
/// reboot), disabling rewrites the file, so the state survives restarts.
pub(crate) const NFS_DISABLED_TAG: &str = "# homevault-nfs-disabled:";

/// Maximum accepted length for a share name.
const SHARE_NAME_MAX: usize = 64;

/// Validate a share name: single non-empty token of ASCII letters, digits,
/// `-`, `_` and `.`; no leading `.`; no `..` segment (a jail mount point is
/// built as `jail/<name>`, so `.`-only names could traverse out of it); not a
/// Samba-reserved section name; this also makes the name safe to embed in
/// `smb.conf` section headers and in `/etc/homevault/*.json` keys.
pub fn validate_share_name(name: &str) -> Result<(), String> {
    if name.trim() != name {
        return Err("share name must not start or end with whitespace".to_string());
    }
    if name.is_empty() {
        return Err("share name must not be empty".to_string());
    }
    if name.chars().count() > SHARE_NAME_MAX {
        return Err(format!(
            "share name must be at most {SHARE_NAME_MAX} characters"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("share name may only contain letters, digits, '-', '_' and '.' (no spaces, brackets, slashes or newlines — those could forge extra smb.conf sections or /etc/exports columns)".to_string());
    }
    if name.starts_with('.') || name.contains("..") {
        return Err("share name must not start with '.' or contain '..'".to_string());
    }
    if RESERVED_SMB_SECTION_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
    {
        return Err(format!("share name '{name}' is reserved by Samba"));
    }
    Ok(())
}

/// Validate a share comment / FTP banner: a single line, no control
/// characters (tab allowed), max length. Both smb.conf (`comment = …`) and
/// vsftpd.conf (`ftpd_banner=…`) put this value on exactly one line.
pub fn validate_share_comment(comment: &str) -> Result<(), String> {
    if comment.chars().count() > 200 {
        return Err("comment must be at most 200 characters".to_string());
    }
    if comment.chars().any(|c| c.is_control() && c != '\t') {
        return Err(
            "comment must be a single line (no newlines or control characters)".to_string(),
        );
    }
    Ok(())
}

/// Validate a share path: absolute Unix path, no whitespace, no control
/// characters and none of `( ) , # ;` — the path is embedded verbatim as the
/// first column of `/etc/exports` and in `/etc/fstab`, where spaces inject
/// extra columns and `)`/`,`/`#` inject or terminate the export options and
/// comments. `..` segments are rejected (no intentional traversal).
pub fn validate_share_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err("share path must be absolute (start with '/')".to_string());
    }
    if path.chars().count() > 200 {
        return Err("share path must be at most 200 characters".to_string());
    }
    if path
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '(' | ')' | ',' | '#' | ';'))
    {
        return Err("share path must not contain spaces or any of ( ) , # ; — it is embedded verbatim in /etc/exports and /etc/fstab".to_string());
    }
    if path.split('/').skip(1).any(|seg| seg == "..") {
        return Err("share path must not contain '..' segments".to_string());
    }
    Ok(())
}

/// Validate NFS export options: each single token without whitespace,
/// control characters, or any of `( ) , # " '`. Options are written as
/// `path *(opt1,opt2)`, so a `)` in an option would close the client list
/// and fabricate an extra client clause (`rw) /etc *(no_root_squash`).
pub fn validate_nfs_options(options: &[String]) -> Result<(), String> {
    if options.len() > 16 {
        return Err("too many NFS options (max 16)".to_string());
    }
    for opt in options {
        if opt.is_empty() {
            return Err("NFS options must not be empty strings".to_string());
        }
        if opt.chars().count() > 60 {
            return Err(format!("NFS option '{opt}' is too long"));
        }
        if opt.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '(' | ')' | ',' | '#' | '"' | '\'')
        }) {
            return Err(format!("NFS option '{opt}' contains a forbidden character (whitespace, control character or one of ( ) , # \" ') — options are embedded in /etc/exports as 'path *(opts)'"));
        }
    }
    Ok(())
}

/// Single-line + max-length check shared by the per-protocol config
/// validators. `forbid_section_chars` additionally rejects `[`, `]` and `;`
/// (values formatted into smb.conf, where those characters could forge
/// section boundaries or commented-out directives).
fn check_config_value(
    field: &str,
    value: &str,
    max: usize,
    forbid_section_chars: bool,
) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{field} must be at most {max} characters"));
    }
    if value.chars().any(|c| c.is_control() && c != '\t') {
        return Err(format!(
            "{field} must be a single line (no newlines or control characters)"
        ));
    }
    if forbid_section_chars && value.contains(['[', ']', ';']) {
        return Err(format!(
            "{field} must not contain '[', ']' or ';' (config-file section characters)"
        ));
    }
    Ok(())
}

/// Validate the global SMB service config before it is formatted into
/// `smb.conf` (`workgroup = …`, `security = …`, … are one-line directives).
/// `additional_params` are real global directives: each one must be
/// `key = value` with a key from [`SMB_EXTRA_KEYS`], so a request can never
/// inject a section header or an `include` into the root-owned file.
/// Validate an AD realm: a DNS name, upper/lower case accepted, no spaces or
/// config-file delimiters.
pub(crate) fn validate_realm(realm: &str) -> Result<(), String> {
    if realm.len() > 255
        || realm.is_empty()
        || !realm.contains('.')
        || realm
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
    {
        return Err("realm must be a DNS name such as 'EXAMPLE.LAN'".to_string());
    }
    if realm.starts_with('.') || realm.ends_with('.') || realm.contains("..") {
        return Err("realm must be a DNS name such as 'EXAMPLE.LAN'".to_string());
    }
    Ok(())
}

/// `idmap config * : range` is two positive integers, low < high.
pub(crate) fn validate_idmap_range(range: &str) -> Result<(), String> {
    let (low, high) = range
        .split_once('-')
        .ok_or_else(|| "idmap_range must look like '10000-99999'".to_string())?;
    let low: u32 = low
        .trim()
        .parse()
        .map_err(|_| "idmap_range must look like '10000-99999'".to_string())?;
    let high: u32 = high
        .trim()
        .parse()
        .map_err(|_| "idmap_range must look like '10000-99999'".to_string())?;
    if low >= high {
        return Err("idmap_range must have a lower bound below its upper bound".to_string());
    }
    Ok(())
}

pub(crate) fn validate_smb_config(config: &SmbConfig) -> Result<(), String> {
    check_config_value("workgroup", &config.workgroup, 15, true)?;
    let security = config.security_mode.to_lowercase();
    match security.as_str() {
        "user" | "share" => {}
        // ADS/domain use winbind + Kerberos; the realm and idmap range must be
        // explicit. `server` (trusted-domain member) still needs a domain
        // controller account and is not offered.
        "ads" | "domain" => {
            let realm = config.realm.trim();
            if realm.is_empty() {
                return Err(format!("realm is required when security_mode is '{security}'"));
            }
            validate_realm(realm)?;
            validate_idmap_range(&config.idmap_range)?;
        }
        "server" => {
            return Err(
                "security_mode 'server' is not implemented; use 'ads' for an Active Directory member or 'user' for local accounts"
                    .to_string(),
            )
        }
        other => {
            return Err(format!(
                "security_mode '{other}' is not valid (use user, share or ads)"
            ))
        }
    }
    match config.protocol_version.trim() {
        "SMB2" | "SMB3" => {}
        // SMB1/NT1 is broken and deprecated; the UI used to offer it.
        "NT1" | "SMB1" | "CIFS" => {
            return Err("protocol_version NT1/SMB1 is not supported — use SMB2 or SMB3".to_string())
        }
        other => {
            return Err(format!(
                "protocol_version '{other}' is not valid (use SMB2 or SMB3)"
            ))
        }
    }
    match config.smb_encrypt.trim() {
        "off" | "desired" | "required" => {}
        other => {
            return Err(format!(
                "smb_encrypt '{other}' is not valid (use off, desired or required)"
            ))
        }
    }
    match config.server_signing.trim() {
        "off" | "desired" | "mandatory" => {}
        other => {
            return Err(format!(
                "server_signing '{other}' is not valid (use off, desired or mandatory)"
            ))
        }
    }
    const MAX_AIO_BYTES: u32 = 16 * 1024 * 1024;
    for (name, value) in [
        ("aio_read_size", config.aio_read_size),
        ("aio_write_size", config.aio_write_size),
    ] {
        if value > MAX_AIO_BYTES {
            return Err(format!(
                "{name} must be 0 (auto) or at most {MAX_AIO_BYTES} bytes"
            ));
        }
    }
    for (name, mask) in [
        ("create_mask", &config.create_mask),
        ("directory_mask", &config.directory_mask),
    ] {
        if !(3..=4).contains(&mask.chars().count())
            || mask.chars().any(|c| !('0'..='7').contains(&c))
        {
            return Err(format!(
                "{name} must be 3-4 octal digits (e.g. 0755), got '{mask}'"
            ));
        }
    }
    for param in &config.additional_params {
        check_config_value("additional_params entries", param, 200, true)?;
        smb_extra_param(param)?;
    }
    Ok(())
}

/// Global Samba directives an operator may add through `additional_params`.
/// A short allowlist is safer than writing arbitrary `key = value` lines into
/// the root-owned `smb.conf`: `include`, section headers and `%` macros are
/// deliberately absent.
const SMB_EXTRA_KEYS: &[&str] = &[
    "server signing",
    "server smb encrypt",
    "smb encrypt",
    "min receivefile size",
    "socket options",
    "deadtime",
    "keepalive",
    "log level",
    "max stat cache size",
    "aio read size",
    "aio write size",
    "use sendfile",
    "durable handles",
    "strict sync",
    "oplocks",
    "kernel oplocks",
    "level2 oplocks",
    "server multi channel support",
];

/// Parse and validate one `key = value` extra global parameter.
fn smb_extra_param(param: &str) -> Result<(String, String), String> {
    let (key, value) = param
        .split_once('=')
        .ok_or_else(|| format!("additional parameter '{param}' must be 'key = value'"))?;
    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    if !SMB_EXTRA_KEYS.contains(&key.as_str()) {
        return Err(format!(
            "additional parameter '{key}' is not allowed (allowed: {})",
            SMB_EXTRA_KEYS.join(", ")
        ));
    }
    if value.is_empty() {
        return Err(format!("additional parameter '{key}' needs a value"));
    }
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '[' | ']' | ';'))
    {
        return Err(format!(
            "additional parameter '{key}' contains a forbidden character"
        ));
    }
    Ok((key, value.to_string()))
}

/// Render the `[global]` block of `smb.conf`. The destructuring is exhaustive
/// on purpose: adding a field to `SmbConfig` without rendering it here is a
/// compile error, which is the contract P0‑4 asks for.
fn smb_global_params(config: &SmbConfig) -> Vec<String> {
    let SmbConfig {
        workgroup,
        security_mode,
        realm,
        idmap_range,
        protocol_version,
        allow_guest,
        create_mask,
        directory_mask,
        additional_params,
        multichannel,
        // Deliberately not a `smb.conf` key: it is applied by `ws_discovery`.
        ws_discovery: _,
        durable_handles,
        smb_encrypt,
        server_signing,
        strict_sync,
        aio_read_size,
        aio_write_size,
        use_sendfile,
        oplocks,
        kernel_oplocks,
        level2_oplocks,
    } = config;

    let yes_no = |value: bool| if value { "yes" } else { "no" };
    let mut params = vec![
        format!("  workgroup = {workgroup}"),
        format!("  security = {security_mode}"),
    ];
    if matches!(security_mode.to_lowercase().as_str(), "ads" | "domain") {
        params.push(format!("  realm = {}", realm.trim()));
        params.push("  idmap config * : backend = tdb".to_string());
        params.push(format!("  idmap config * : range = {}", idmap_range.trim()));
        params.push("  winbind use default domain = yes".to_string());
        params.push("  winbind offline logon = yes".to_string());
        // Windows ACLs, stored in the filesystem's extended attributes.
        params.push("  vfs objects = acl_xattr".to_string());
        params.push("  map acl inherit = yes".to_string());
        params.push("  store dos attributes = yes".to_string());
    }
    params.extend([
        format!("  server min protocol = {protocol_version}"),
        "  server max protocol = SMB3".to_string(),
        format!("  server multi channel support = {}", yes_no(*multichannel)),
        format!("  server smb encrypt = {smb_encrypt}"),
        format!("  server signing = {server_signing}"),
        format!("  durable handles = {}", yes_no(*durable_handles)),
        format!("  strict sync = {}", yes_no(*strict_sync)),
        format!("  use sendfile = {}", yes_no(*use_sendfile)),
        format!("  oplocks = {}", yes_no(*oplocks)),
        format!("  kernel oplocks = {}", yes_no(*kernel_oplocks)),
        format!("  level2 oplocks = {}", yes_no(*level2_oplocks)),
        format!("  guest ok = {}", yes_no(*allow_guest)),
        format!("  create mask = {create_mask}"),
        format!("  directory mask = {directory_mask}"),
    ]);
    if *aio_read_size > 0 {
        params.push(format!("  aio read size = {aio_read_size}"));
    }
    if *aio_write_size > 0 {
        params.push(format!("  aio write size = {aio_write_size}"));
    }
    let managed_keys: Vec<String> = params
        .iter()
        .filter_map(|param| {
            param
                .trim()
                .split_once('=')
                .map(|(key, _)| key.trim().to_ascii_lowercase())
        })
        .collect();
    for param in additional_params {
        if let Ok((key, value)) = smb_extra_param(param) {
            // A managed field wins over the same key passed as an extra, so a
            // stale `server multi channel support` cannot fight the toggle.
            if managed_keys.contains(&key) {
                continue;
            }
            params.push(format!("  {key} = {value}"));
        }
    }
    params
}

/// Bring a config written by an older build back into the supported set
/// before it reaches the UI. A stored `ads`/`domain`/`server` would otherwise
/// render as an empty select (the option no longer exists) and fail on save.
fn normalize_smb_config(config: &mut SmbConfig) {
    let security = config.security_mode.to_lowercase();
    let has_realm = !config.realm.trim().is_empty();
    if !matches!(security.as_str(), "user" | "share" | "ads" | "domain")
        || (matches!(security.as_str(), "ads" | "domain") && !has_realm)
    {
        warn!(
            "stored SMB security_mode '{}' is not usable; falling back to 'user'",
            config.security_mode
        );
        config.security_mode = "user".to_string();
    }
    if validate_idmap_range(&config.idmap_range).is_err() {
        config.idmap_range = default_idmap_range();
    }
    if !matches!(config.protocol_version.trim(), "SMB2" | "SMB3") {
        warn!(
            "stored SMB protocol_version '{}' is not supported; falling back to 'SMB3'",
            config.protocol_version
        );
        config.protocol_version = "SMB3".to_string();
    }
    if !matches!(config.smb_encrypt.trim(), "off" | "desired" | "required") {
        warn!(
            "stored SMB smb_encrypt '{}' is not valid; falling back to 'off'",
            config.smb_encrypt
        );
        config.smb_encrypt = default_smb_encrypt();
    }
    if !matches!(
        config.server_signing.trim(),
        "off" | "desired" | "mandatory"
    ) {
        warn!(
            "stored SMB server_signing '{}' is not valid; falling back to 'off'",
            config.server_signing
        );
        config.server_signing = default_server_signing();
    }
    const MAX_AIO_BYTES: u32 = 16 * 1024 * 1024;
    if config.aio_read_size > MAX_AIO_BYTES {
        warn!(
            "stored aio_read_size {} is out of range; falling back to 0",
            config.aio_read_size
        );
        config.aio_read_size = 0;
    }
    if config.aio_write_size > MAX_AIO_BYTES {
        warn!(
            "stored aio_write_size {} is out of range; falling back to 0",
            config.aio_write_size
        );
        config.aio_write_size = 0;
    }
}

/// The directive key of an `smb.conf` line, lower-cased. Comments, blank
/// lines and section headers return `None` and are never removed.
fn smb_line_key(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with(';')
        || trimmed.starts_with('[')
    {
        return None;
    }
    trimmed
        .split_once('=')
        .map(|(key, _)| key.trim().to_ascii_lowercase())
}

/// Validate the global FTP service config before it is formatted into
/// `vsftpd.conf`: the banner must be single-line (it lands in
/// `ftpd_banner=…`), `passive_ports` must be `"min"` or `"min-max"`, both
/// `port` and the passive bounds must be non-zero u16s.
pub(crate) fn validate_ftp_config(config: &FtpConfig) -> Result<(), String> {
    check_config_value("welcome_message", &config.welcome_message, 120, true)?;
    if config.port == 0 {
        return Err("FTP port must not be 0".to_string());
    }
    if config.max_clients == 0 {
        return Err("max_clients must be at least 1".to_string());
    }
    let mut parts = config.passive_ports.split('-');
    let parse_port = |s: &str| -> Result<u16, String> {
        s.trim()
            .parse::<u16>()
            .map_err(|_| format!("passive_ports '{s}' is not a valid port number"))
            .and_then(|p| {
                if p == 0 {
                    Err(format!("passive_ports '{s}' must not be 0"))
                } else {
                    Ok(p)
                }
            })
    };
    match (parts.next(), parts.next(), parts.next()) {
        (Some(min), None, _) => {
            parse_port(min)?;
        }
        (Some(min), Some(max), None) => {
            if parse_port(min)? >= parse_port(max)? {
                return Err(format!(
                    "passive_ports range '{min}-{max}' must have min below max"
                ));
            }
        }
        _ => {
            return Err("passive_ports must be 'min' or 'min-max' (e.g. '30000-31000')".to_string())
        }
    }
    for user in &config.allowed_users {
        validate_ftp_user(user)?;
    }
    if let Some(root) = config
        .anonymous_root
        .as_deref()
        .map(str::trim)
        .filter(|root| !root.is_empty())
    {
        validate_share_path(root).map_err(|e| format!("anonymous_root: {e}"))?;
    }
    Ok(())
}

/// One line in `/etc/vsftpd.user_list`: no whitespace, `/`, `\` or `:`, so a
/// value can neither split the line nor escape the file vsftpd reads.
fn validate_ftp_user(user: &str) -> Result<(), String> {
    check_config_value("allowed_users entries", user, 32, true)?;
    if user.is_empty() {
        return Err("allowed_users entries must not be empty".to_string());
    }
    if user
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(format!(
            "allowed_users entry '{user}' contains a forbidden character (whitespace, /, \\ or :)"
        ));
    }
    Ok(())
}

/// `/etc/vsftpd.user_list`, one account per line. Empty when the whitelist is
/// off (vsftpd then accepts every local account, the historical behaviour).
fn ftp_user_list_text(users: &[String]) -> String {
    if users.is_empty() {
        String::new()
    } else {
        let mut text = users.join("\n");
        text.push('\n');
        text
    }
}

/// Render the whole `vsftpd.conf`. Exhaustive destructuring: a new
/// `FtpConfig` field that is not rendered here is a compile error.
fn ftp_config_text(config: &FtpConfig) -> String {
    let FtpConfig {
        port,
        allowed_users,
        passive_ports,
        max_clients,
        force_tls,
        allow_anonymous,
        anonymous_root,
        // Retained for old `ftp.json` files; the per-user jail is always
        // chrooted now, because a `local_root` without chroot would expose
        // the whole host filesystem.
        chroot: _chroot,
        welcome_message,
    } = config;

    // Defence in depth: `ftpd_banner=` is a single-line directive; the value
    // is already validated, but strip line breaks so no caller can append
    // directives through the banner.
    let banner = welcome_message
        .replace(['\r', '\n'], " ")
        .replace('\t', " ");
    let (pasv_min, pasv_max) = match passive_ports.split_once('-') {
        Some((min, max)) => (min, max),
        None => (passive_ports.as_str(), "31000"),
    };

    let mut text = format!(
        r#"listen=YES
listen_port={port}
anonymous_enable={anon}
local_enable=YES
check_shell=NO
write_enable=YES
local_umask=022
dirmessage_enable=YES
xferlog_enable=YES
connect_from_port_20=YES
seccomp_sandbox=NO
pasv_enable=YES
pasv_min_port={pasv_min}
pasv_max_port={pasv_max}
max_clients={max_clients}
chroot_local_user=YES
allow_writeable_chroot=YES
user_config_dir={user_config_dir}
userlist_enable={userlist_enable}
userlist_deny=NO
userlist_file=/etc/vsftpd.user_list
ftpd_banner={banner}
"#,
        anon = if *allow_anonymous { "YES" } else { "NO" },
        user_config_dir = crate::config::paths::ftp_user_conf_dir().display(),
        userlist_enable = if allowed_users.is_empty() {
            "NO"
        } else {
            "YES"
        },
    );

    if *allow_anonymous {
        if let Some(root) = anonymous_root.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            text.push_str(&format!("anon_root={root}\n"));
        }
    }

    if *force_tls {
        text.push_str(
            r#"ssl_enable=YES
allow_anon_ssl=NO
force_local_data_ssl=YES
force_local_logins_ssl=YES
ssl_tlsv1=YES
ssl_sslv2=NO
ssl_sslv3=NO
rsa_cert_file=/etc/ssl/certs/ssl-cert-snakeoil.pem
rsa_private_key_file=/etc/ssl/private/ssl-cert-snakeoil.key
"#,
        );
    }
    text
}

/// Per-share SMB access generated from the share's grants.
///
/// Samba's `valid users` is the share-level gate DSM expresses as RO/RW/NA;
/// POSIX ACLs remain the file-level gate underneath. The application admin
/// group is always included so an admin can repair a share whose grants were
/// all removed, and an empty list is rendered as admin-only rather than as
/// `valid users =` (which Samba parses as "nobody").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SmbShareAccess {
    pub valid_users: Vec<String>,
    pub read_only: bool,
    pub guest_ok: Option<bool>,
}

impl SmbShareAccess {
    pub fn admin_only() -> Self {
        Self {
            valid_users: vec![format!("@{}", crate::storage::database::ADMIN_GROUP_NAME)],
            read_only: false,
            guest_ok: None,
        }
    }

    /// The managed directives, in a stable order. `guest ok` is only emitted
    /// when the share carries an explicit value, so a global `allow_guest`
    /// keeps working for shares that never opted in or out.
    pub fn render(&self) -> Vec<String> {
        let mut users = if self.valid_users.is_empty() {
            vec![format!("@{}", crate::storage::database::ADMIN_GROUP_NAME)]
        } else {
            self.valid_users.clone()
        };
        // Samba's guest account on Debian/Ubuntu is `nobody`; a per-share
        // `guest ok = yes` must also let that account pass `valid users`.
        if self.guest_ok == Some(true) {
            users.push("nobody".to_string());
        }
        users.sort();
        users.dedup();
        let mut lines = vec![
            format!("  valid users = {}", users.join(" ")),
            format!(
                "  read only = {}",
                if self.read_only { "yes" } else { "no" }
            ),
        ];
        if let Some(guest) = self.guest_ok {
            lines.push(format!("  guest ok = {}", if guest { "yes" } else { "no" }));
        }
        lines
    }
}

/// Build the SMB share access list from the protocol's grants.
pub fn smb_valid_users(
    permissions: &[crate::storage::share_perm_repo::SharePermission],
) -> Vec<String> {
    let mut users = vec![format!("@{}", crate::storage::database::ADMIN_GROUP_NAME)];
    for permission in permissions {
        if permission.grantee_type == "group" {
            users.push(format!(
                "@{}",
                crate::groups::GroupService::get_full_group_name(&permission.grantee_name)
            ));
        } else if permission.grantee_type == "user" {
            users.push(permission.grantee_name.clone());
        }
    }
    users.sort();
    users.dedup();
    users
}

/// Build the appended smb.conf section for a share (single line per field;
/// inputs are validated by the caller).
fn smb_share_section(name: &str, comment: &str, path: &str, access: &SmbShareAccess) -> String {
    let mut section = format!(
        "\n[{name}]\n  comment = {comment}\n  path = {path}\n  browseable = yes\n"
    );
    for line in access.render() {
        section.push_str(&line);
        section.push('\n');
    }
    section
}

/// Set or remove one `key = value` line in a section, leaving every other
/// line byte-for-byte untouched.
fn set_smb_managed_line(section: &str, key: &str, value: Option<&str>) -> String {
    let prefix = format!("{key} =");
    let mut kept: Vec<String> = section
        .lines()
        .filter(|line| !line.trim().starts_with(&prefix))
        .map(str::to_string)
        .collect();
    if let Some(value) = value {
        kept.push(format!("  {key} = {value}"));
    }
    let mut out = kept.join("\n");
    if section.ends_with('\n') || !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Replace the managed access directives inside one existing section while
/// preserving every line the admin added by hand (and the `# homevault-smb-vfs`
/// block, which is rewritten by its own helper).
pub fn apply_smb_access_to_section(section: &str, access: &SmbShareAccess) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in section.lines() {
        let trimmed = line.trim();
        let is_managed = trimmed.starts_with("valid users =")
            || trimmed.starts_with("read only =")
            || trimmed.starts_with("guest ok =");
        if !is_managed {
            kept.push(line);
        }
    }
    let mut out = kept.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    for line in access.render() {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Rewrite the `[name]` section of `smb.conf` with a new access list and
/// reload `smbd`. The caller resolves the grants; this function touches only
/// that one section, so two shares never rewrite each other.
pub fn rewrite_smb_share_access(
    name: &str,
    access: &SmbShareAccess,
) -> Result<(), ShareError> {
    validate_share_name(name).map_err(ShareError::InvalidField)?;
    let config_path = smb_conf_path();
    if !std::path::Path::new(&config_path).exists() {
        return Err(ShareError::ConfigNotFound(config_path));
    }
    let config = std::fs::read_to_string(&config_path)?;
    let header = format!("[{name}]");
    let mut current_start: Option<usize> = None;
    let mut start = 0usize;
    let mut end = config.len();
    let mut offset = 0usize;
    for line in config.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(s) = current_start {
                end = offset;
                start = s;
                break;
            }
            if trimmed == header {
                current_start = Some(offset);
            }
        }
        offset += line.len();
    }
    if let Some(s) = current_start {
        if end == config.len() {
            start = s;
        }
    } else {
        return Err(ShareError::NotFound(name.to_string()));
    }
    let section = &config[start..end];
    let replaced = apply_smb_access_to_section(section, access);
    let mut new_config = String::with_capacity(config.len() + replaced.len());
    new_config.push_str(&config[..start]);
    new_config.push_str(&replaced);
    new_config.push_str(&config[end..]);
    write_config_file(&config_path, &new_config)?;
    reload_smbd(&format!("rewriting access for share {}", name));
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-share Samba VFS features (recycle bin, Time Machine)
// ---------------------------------------------------------------------------
//
// Two independent per-share features share Samba's single `vfs objects`
// line: the recycle bin (a network delete lands in a recoverable folder) and
// Time Machine (`vfs_fruit`, so macOS can use the share as a backup target).
// Because `vfs objects` may only be set once per share section, both toggles
// are rendered into **one** HomeVault-managed block. A marked block keeps
// each toggle idempotent and leaves every line an admin added by hand
// untouched (the same idea as [`NFS_DISABLED_TAG`]).

/// Folder a share's recycle bin writes into. Deliberately the *same* name the
/// Explorer Trash reads (see [`crate::browser::TRASH_DIR_NAME`]), so a file
/// deleted from a network client and a file deleted from the web Explorer end
/// up in one place.
pub const RECYCLE_DIR_NAME: &str = crate::browser::TRASH_DIR_NAME;

/// Marker lines around the directives HomeVault manages inside a share's
/// `smb.conf` section. `# homevault-recycle:begin/end` is the historical
/// marker of the recycle-only block; it is still recognised so an existing
/// install upgrades in place instead of stacking a second block.
const SMB_VFS_BEGIN: &str = "# homevault-smb-vfs:begin";
const SMB_VFS_END: &str = "# homevault-smb-vfs:end";
const SMB_LEGACY_BEGIN: &str = "# homevault-recycle:begin";
const SMB_LEGACY_END: &str = "# homevault-recycle:end";

/// Largest Time Machine quota accepted, in GiB (1 PiB): a typo must not turn
/// into a `fruit:time machine max size` Samba reads as something else.
pub const MAX_TIME_MACHINE_QUOTA_GB: u32 = 1_048_576;

/// The VFS features currently enabled on one share, derived from its managed
/// `smb.conf` block. Serialised as-is for the Shares → SMB tab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmbVfsFlags {
    pub recycle: bool,
    pub time_machine: bool,
    /// SMB "Previous Versions" (`vfs_shadow_copy2`). The snapshot directory
    /// and name format are the ZFS defaults; a Btrfs host can edit the
    /// managed block and the values are preserved by the next toggle.
    pub previous_versions: bool,
    /// `shadow:snapdir` (ZFS default `.zfs/snapshot`).
    pub snapdir: String,
    /// `shadow:format` (ZFS default `%Y-%m-%d_%H:%M:%S`).
    pub format: String,
}

/// Time Machine state of one SMB share, as the UI and the mDNS publisher read
/// it: the toggle plus the optional maximum size macOS is told to respect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmbTimeMachine {
    pub enabled: bool,
    /// Quota in GiB; `0` means "no quota" (macOS grows until the share fills).
    pub quota_gb: u32,
}

/// Validate a Time Machine quota in GiB (`0` = unlimited).
pub fn validate_time_machine_quota(quota_gb: u32) -> Result<(), String> {
    if quota_gb > MAX_TIME_MACHINE_QUOTA_GB {
        return Err(format!(
            "a Time Machine quota above {MAX_TIME_MACHINE_QUOTA_GB} GiB is not supported (0 = unlimited)"
        ));
    }
    Ok(())
}

/// Render the managed block for one share section. Returns an empty string
/// when neither feature is on — the caller then removes the block entirely.
///
/// `vfs objects` lists `fruit` before `streams_xattr` (the order `vfs_fruit`
/// documents) and `recycle` last, so a delete is offered the fruit-handled
/// stream before the recycle module captures it.
fn smb_vfs_block(flags: SmbVfsFlags, quota_gb: u32) -> String {
    if !flags.recycle && !flags.time_machine && !flags.previous_versions {
        return String::new();
    }
    let mut modules: Vec<&str> = Vec::new();
    if flags.time_machine {
        modules.push("fruit");
        modules.push("streams_xattr");
    }
    if flags.recycle {
        modules.push("recycle");
    }
    if flags.previous_versions {
        modules.push("shadow_copy2");
    }

    let mut block = format!("{SMB_VFS_BEGIN}\n  vfs objects = {}\n", modules.join(" "));
    if flags.time_machine {
        // `fruit:time machine = yes` is the switch macOS' Time Machine looks
        // for; `fruit:metadata = stream` and the model/rename/wiping options
        // are what vfs_fruit's Time Machine recipe requires for a reliable
        // first backup and incremental backups afterwards.
        block.push_str("  fruit:time machine = yes\n");
        block.push_str("  fruit:time machine max size = ");
        if quota_gb == 0 {
            block.push_str("0\n");
        } else {
            block.push_str(&format!("{quota_gb}G\n"));
        }
        block.push_str("  fruit:metadata = stream\n");
        block.push_str("  fruit:model = MacSamba\n");
        block.push_str("  fruit:posix_rename = yes\n");
        block.push_str("  fruit:veto_appledouble = no\n");
        block.push_str("  fruit:wipe_intentionally_left_blank_rfork = yes\n");
        block.push_str("  fruit:delete_empty_adfiles = yes\n");
    }
    if flags.recycle {
        block.push_str(&format!(
            "  recycle:repository = {RECYCLE_DIR_NAME}\n  recycle:keeptree = yes\n  recycle:versions = yes\n  recycle:touch = yes\n  recycle:maxsize = 0\n  recycle:exclude = *.tmp, *.temp, *.swp, .DS_Store, Thumbs.db\n  recycle:directory_mode = 0700\n"
        ));
    }
    if flags.previous_versions {
        let snapdir = if flags.snapdir.trim().is_empty() {
            ".zfs/snapshot"
        } else {
            flags.snapdir.trim()
        };
        let format = if flags.format.trim().is_empty() {
            "%Y-%m-%d_%H:%M:%S"
        } else {
            flags.format.trim()
        };
        block.push_str(&format!(
            "  shadow:snapdir = {snapdir}\n  shadow:sort = desc\n  shadow:format = {format}\n  shadow:localtime = yes\n  shadow:snapprefix = .*\n  shadow:delimiter = _\n"
        ));
    }
    block.push_str(SMB_VFS_END);
    block
}

/// Remove every HomeVault-managed block (current or legacy recycle marker)
/// from one `smb.conf` share section, then append a fresh block when `flags`
/// enables something. Pure — the caller writes the result back — and
/// idempotent. Lines the admin added by hand are preserved verbatim.
fn apply_vfs_to_section(section: &str, flags: SmbVfsFlags, quota_gb: u32) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed == SMB_VFS_BEGIN || trimmed == SMB_LEGACY_BEGIN {
            skipping = true;
            continue;
        }
        if skipping {
            if trimmed == SMB_VFS_END || trimmed == SMB_LEGACY_END {
                skipping = false;
            }
            continue;
        }
        kept.push(line);
    }

    let mut out = kept.join("\n");
    let block = smb_vfs_block(flags, quota_gb);
    if !block.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block);
    }
    // Preserve the original trailing newline so a disable on a section that
    // never had a block round-trips byte-for-byte.
    if section.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The features a share section currently enables, parsed from its managed
/// block. A section without a block has every feature off.
fn section_vfs_flags(section: &str) -> SmbVfsFlags {
    let mut flags = SmbVfsFlags::default();
    for line in section.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("shadow:snapdir =") {
            flags.snapdir = value.trim().to_string();
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("shadow:format =") {
            flags.format = value.trim().to_string();
            continue;
        }
        let Some(modules) = trimmed
            .strip_prefix("vfs objects =")
            .or_else(|| trimmed.strip_prefix("vfs objects = "))
        else {
            continue;
        };
        flags.recycle |= modules.split_whitespace().any(|m| m == "recycle");
        flags.previous_versions |= modules.split_whitespace().any(|m| m == "shadow_copy2");
        flags.time_machine |= modules.split_whitespace().any(|m| m == "fruit")
            && section
                .lines()
                .any(|l| l.trim().eq_ignore_ascii_case("fruit:time machine = yes"));
    }
    flags
}

/// The Time Machine quota (GiB) a share section declares, `0` when absent.
fn section_time_machine_quota(section: &str) -> u32 {
    for line in section.lines() {
        let trimmed = line.trim();
        let Some(value) = trimmed.strip_prefix("fruit:time machine max size =") else {
            continue;
        };
        let value = value.trim();
        let Some(digits) = value.strip_suffix('G').or_else(|| value.strip_suffix('g')) else {
            continue;
        };
        return digits.trim().parse::<u32>().unwrap_or(0);
    }
    0
}

/// The legacy recycle-only block, kept for tests and for the recycle toggle's
/// helpers.
#[cfg(test)]
fn smb_recycle_block() -> String {
    smb_vfs_block(
        SmbVfsFlags {
            recycle: true,
            time_machine: false,
            previous_versions: false,
            ..SmbVfsFlags::default()
        },
        0,
    )
}

/// Toggle only the recycle feature, leaving Time Machine untouched.
fn apply_recycle_to_section(section: &str, enabled: bool) -> String {
    let mut flags = section_vfs_flags(section);
    flags.recycle = enabled;
    apply_vfs_to_section(section, flags, section_time_machine_quota(section))
}

/// Whether `section` (one `smb.conf` share section) carries the recycle module.
fn section_has_recycle(section: &str) -> bool {
    section_vfs_flags(section).recycle
}

/// Toggle only the Previous Versions (`shadow_copy2`) feature, leaving the
/// recycle/Time Machine flags untouched.
fn apply_previous_versions_to_section(section: &str, enabled: bool) -> String {
    let mut flags = section_vfs_flags(section);
    flags.previous_versions = enabled;
    apply_vfs_to_section(section, flags, section_time_machine_quota(section))
}

/// Parse the live `smb.conf` into `share name → (VFS flags, Time Machine
/// quota)`.
///
/// One parser feeding both per-share maps, so the recycle and Time Machine
/// views can never disagree about which block a section owns. Shares are
/// delimited by their `[name]` line; `global`/`homes` are not shares. A
/// missing or unreadable file yields an empty map, which is what "no SMB
/// share is configured" looks like.
fn smb_section_states() -> HashMap<String, (SmbVfsFlags, u32)> {
    let mut map = HashMap::new();
    let Ok(config) = std::fs::read_to_string(smb_conf_path()) else {
        return map;
    };
    let mut current: Option<String> = None;
    let mut body = String::new();
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(name) = current.take() {
                map.insert(
                    name,
                    (section_vfs_flags(&body), section_time_machine_quota(&body)),
                );
            }
            body.clear();
            let name = trimmed[1..trimmed.len() - 1].to_string();
            if name != "global" && name != "homes" {
                current = Some(name);
            }
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    if let Some(name) = current {
        map.insert(
            name,
            (section_vfs_flags(&body), section_time_machine_quota(&body)),
        );
    }
    map
}

/// Validate the global NFS service configuration before it is rendered into
/// `/etc/exports`. The network list is a list of client clauses (`*`, a CIDR
/// or a host name), never free-form text: whitespace or `()` there would forge
/// an extra client clause.
pub fn validate_nfs_config(config: &NfsConfig) -> Result<(), String> {
    match config.default_access.trim() {
        "rw" | "ro" => {}
        other => {
            return Err(format!(
                "default_access '{other}' is not valid (use rw or ro)"
            ))
        }
    }
    match config.root_squash.trim() {
        "root_squash" | "no_root_squash" | "all_squash" => {}
        other => {
            return Err(format!(
                "root_squash '{other}' is not valid (use root_squash, no_root_squash or all_squash)"
            ))
        }
    }
    nfs_version_option(&config.version)?;
    match config.security_flavor.trim() {
        "sys" | "" => {}
        "krb5" | "krb5i" | "krb5p" => {
            if !matches!(config.version.trim(), "4" | "4.1" | "4.2") {
                return Err(
                    "NFS Kerberos requires an explicit NFSv4 version (4, 4.1 or 4.2)"
                        .to_string(),
                );
            }
        }
        other => {
            return Err(format!(
                "NFS security flavor '{other}' is not valid (use sys, krb5, krb5i or krb5p)"
            ))
        }
    }
    if config.networks.is_empty() {
        return Err("at least one NFS network/client is required (use * for everyone)".to_string());
    }
    for network in &config.networks {
        validate_nfs_network(network)?;
    }
    validate_nfs_options(&config.custom_options)
}

/// A single `/etc/exports` client clause: `*`, a CIDR, an IP or a host name.
/// Rejects the characters that would end the clause or inject another one.
fn validate_nfs_network(network: &str) -> Result<(), String> {
    let network = network.trim();
    if network.is_empty() {
        return Err("NFS network entries must not be empty".to_string());
    }
    if network.chars().count() > 64 {
        return Err(format!("NFS network '{network}' is too long"));
    }
    if network != "*"
        && network.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(
                    c,
                    '(' | ')' | ',' | '#' | '"' | '\'' | '[' | ']' | ';' | '\\'
                )
        })
    {
        return Err(format!(
            "NFS network '{network}' contains a forbidden character (whitespace, control character or one of ( ) , # \" ' [ ] ; \\)"
        ));
    }
    Ok(())
}

/// Validate per-client NFS rules before any line reaches `/etc/exports`.
pub fn validate_nfs_rules(rules: &[NfsClientRule]) -> Result<(), String> {
    if rules.len() > 64 {
        return Err("at most 64 NFS client rules are supported per share".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for rule in rules {
        validate_nfs_network(&rule.client)?;
        if !seen.insert(rule.client.trim().to_string()) {
            return Err(format!("NFS client '{}' is listed twice", rule.client));
        }
        match rule.access.trim() {
            "rw" | "ro" => {}
            other => return Err(format!("NFS rule access '{other}' is not valid (use rw or ro)")),
        }
        validate_nfs_options(&rule.options)?;
    }
    Ok(())
}

/// Translate the UI's version choice into a `vers=` export option.
/// `auto` (and an empty value) lets the kernel negotiate; this keeps existing
/// installs working when the field was never touched.
fn nfs_version_option(version: &str) -> Result<Option<String>, String> {
    match version.trim() {
        "" | "auto" => Ok(None),
        "3" | "4" | "4.1" | "4.2" => Ok(Some(format!("vers={}", version.trim()))),
        other => Err(format!(
            "NFS version '{other}' is not valid (use 3, 4, 4.1, 4.2 or auto)"
        )),
    }
}

/// Option classes used to merge service defaults with per-share overrides.
/// A later option replaces the earlier one of the same class, so a share can
/// still say `ro` while the service default is `rw`.
fn nfs_option_class(option: &str) -> &'static str {
    match option {
        "rw" | "ro" => "access",
        "sync" | "async" => "sync",
        "subtree_check" | "no_subtree_check" => "subtree",
        "root_squash" | "no_root_squash" | "all_squash" => "squash",
        _ if option.starts_with("vers=") => "vers",
        _ if option.starts_with("sec=") => "sec",
        _ => "other",
    }
}

fn nfs_merge_options(defaults: &[String], overrides: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for option in defaults.iter().chain(overrides.iter()) {
        let class = nfs_option_class(option);
        if class == "other" {
            if merged.iter().any(|existing| existing == option) {
                continue;
            }
            merged.push(option.clone());
        } else if let Some(position) = merged
            .iter()
            .position(|existing| nfs_option_class(existing) == class)
        {
            // Replace in place: a share override must not reorder the line,
            // so the golden tests and a human reading /etc/exports both see
            // the same stable option order.
            merged[position] = option.clone();
        } else {
            merged.push(option.clone());
        }
    }
    merged
}

/// Build the `/etc/exports` line for a share:
/// `path client1(opts) client2(opts)`. The service configuration provides the
/// defaults; the share's own options override them by class (access, sync,
/// subtree, squash, vers, sec). Every token is guarded by
/// [`validate_nfs_options`] / [`validate_nfs_config`].
fn nfs_export_line(
    path: &str,
    share_options: &[String],
    config: &NfsConfig,
    rules: &[NfsClientRule],
) -> String {
    // Exhaustive destructuring is the NFS half of the P0-4 contract: adding a
    // field to `NfsConfig` without rendering it here is a compile error.
    let NfsConfig {
        networks,
        default_access,
        version,
        root_squash,
        security_flavor,
        custom_options,
    } = config;

    let mut defaults = vec![
        default_access.trim().to_string(),
        "sync".to_string(),
        "no_subtree_check".to_string(),
        root_squash.trim().to_string(),
    ];
    if let Ok(Some(version)) = nfs_version_option(version) {
        defaults.push(version);
    }
    if !matches!(security_flavor.trim(), "" | "sys") {
        defaults.push(format!("sec={}", security_flavor.trim()));
    }
    defaults.extend(custom_options.iter().cloned());

    // A share with explicit client rules uses one clause per rule; a share
    // with none keeps the historical "all configured networks, same options"
    // behaviour. Per-client access overrides the global/list default, and
    // per-rule options are appended after the merged defaults.
    let clauses: Vec<String> = if rules.is_empty() {
        let opts = nfs_merge_options(&defaults, share_options).join(",");
        networks
            .iter()
            .map(|network| format!("{}({opts})", network.trim()))
            .collect()
    } else {
        rules
            .iter()
            .map(|rule| {
                let mut options = nfs_merge_options(&defaults, share_options);
                options = nfs_merge_options(
                    &options,
                    &[rule.access.trim().to_string()]
                        .into_iter()
                        .chain(rule.options.iter().cloned())
                        .collect::<Vec<_>>(),
                );
                format!("{}({})", rule.client.trim(), options.join(","))
            })
            .collect()
    };
    format!("{path} {}\n", clauses.join(" "))
}

/// Build the tagged (disabled) form of an `/etc/exports` line:
/// `# homevault-nfs-disabled:<name> path *(options)`.
fn nfs_disabled_line(name: &str, export_line: &str) -> String {
    format!("{NFS_DISABLED_TAG}{name} {}\n", export_line.trim_end())
}

/// Parse a single `/etc/exports` line into a share.
///
/// Returns `None` for blank lines and plain comments. Lines carrying the
/// [`NFS_DISABLED_TAG`] prefix are parsed with `enabled: false` (the tag's
/// share name is ignored — the parsed name is the path's basename, the same
/// convention enabled lines use, so list/toggle/delete all key on it).
fn parse_nfs_export_line(raw_line: &str) -> Option<Share> {
    let trimmed = raw_line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let body = match trimmed.strip_prefix(NFS_DISABLED_TAG) {
        Some(rest) => {
            // "<name> <path> *(opts)" → the share body after the tag name.
            match rest.split_once(' ') {
                Some((_, line)) => line.trim(),
                None => rest,
            }
        }
        None => {
            if trimmed.starts_with('#') {
                return None;
            }
            trimmed
        }
    };
    let enabled = trimmed.strip_prefix(NFS_DISABLED_TAG).is_none();

    let parts: Vec<&str> = body.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let path = parts[0];
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    // `*(rw,sync)` or `192.168.1.0/24(rw) 10.0.0.0/8(rw)` → the option list,
    // so it round-trips through `nfs_export_line` exactly. A client clause is
    // `<client>(<options>)`; the parentheses are the only safe delimiter
    // because host names never contain them.
    let mut options: Vec<String> = Vec::new();
    for client_token in &parts[1..] {
        let Some(open) = client_token.find('(') else {
            continue;
        };
        let inner = &client_token[open + 1..];
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        for option in inner.split(',').filter(|opt| !opt.is_empty()) {
            // Several clients carry the same option list; keep one copy so the
            // parsed share round-trips through `nfs_export_line` exactly.
            if !options.iter().any(|existing| existing == option) {
                options.push(option.to_string());
            }
        }
    }

    Some(Share {
        name,
        share_type: ShareType::Nfs,
        path: path.to_string(),
        enabled,
        comment: String::new(),
        options,
        read_only: false,
        guest_ok: None,
        restrictions: ShareRestrictions::default(),
    })
}

/// Join an Active Directory domain with a domain account. The password goes
/// on stdin, never in argv (where `ps` would expose it).
pub fn join_smb_domain(username: &str, password: &str) -> Result<(), ShareError> {
    if username.trim().is_empty() || password.is_empty() || password.contains('\n') {
        return Err(ShareError::InvalidField(
            "a domain account and a single-line password are required".to_string(),
        ));
    }
    let mut cmd = Command::new("sudo");
    cmd.args(["net", "ads", "join", "-U", username.trim()]);
    let output = run_timed_with_stdin(
        &mut cmd,
        SHARES_SERVICE_TIMEOUT,
        format!("{password}\n").as_bytes(),
    )
    .map_err(ShareError::CommandFailed)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ShareError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

/// Leave the currently joined AD domain.
pub fn leave_smb_domain() -> Result<(), ShareError> {
    let mut cmd = Command::new("sudo");
    cmd.args(["net", "ads", "leave"]);
    let output = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT)
        .map_err(ShareError::CommandFailed)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ShareError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

/// Service for managing SMB, NFS, FTP, and WebDAV shares.
/// Binaries a working winbind/AD stack needs.
pub const WINBIND_BINARIES: &[&str] = &["/usr/sbin/winbindd", "/usr/bin/wbinfo"];

/// Whether the host has the winbind binaries. Split from the live check so a
/// test can pass a controlled list instead of touching the host.
pub fn winbind_available_at(paths: &[&str]) -> bool {
    paths.iter().any(|path| std::path::Path::new(path).exists())
}

pub fn winbind_available() -> bool {
    winbind_available_at(WINBIND_BINARIES)
}

fn validate_domain_username(username: &str) -> Result<(), String> {
    let username = username.trim();
    if username.is_empty()
        || username.len() > 256
        || username
            .chars()
            .any(|c| c.is_control() || c == '\n' || c == '\r')
    {
        return Err("a domain account name is required".to_string());
    }
    Ok(())
}

/// `net ads join` argv. The password is **not** an argument: it is written to
/// stdin so it never appears in `ps` or the audit log.
pub fn domain_join_argv(username: &str) -> Result<Vec<String>, String> {
    validate_domain_username(username)?;
    Ok(vec![
        "sudo".to_string(),
        "net".to_string(),
        "ads".to_string(),
        "join".to_string(),
        "-U".to_string(),
        username.trim().to_string(),
    ])
}

pub fn domain_leave_argv(username: &str) -> Result<Vec<String>, String> {
    validate_domain_username(username)?;
    Ok(vec![
        "sudo".to_string(),
        "net".to_string(),
        "ads".to_string(),
        "leave".to_string(),
        "-U".to_string(),
        username.trim().to_string(),
    ])
}

/// Join an AD domain. Requires winbind to be installed; a missing stack is a
/// named refusal, not a half-configured `security = ads`.
pub fn join_domain(username: &str, password: &str) -> Result<String, ShareError> {
    if !winbind_available() {
        return Err(ShareError::Unavailable(
            "winbind is not installed: install winbind/libnss-winbind before joining a domain"
                .to_string(),
        ));
    }
    let argv = domain_join_argv(username).map_err(ShareError::InvalidField)?;
    run_net_ads(&argv, password)
}

pub fn leave_domain(username: &str, password: &str) -> Result<String, ShareError> {
    let argv = domain_leave_argv(username).map_err(ShareError::InvalidField)?;
    run_net_ads(&argv, password)
}

fn run_net_ads(argv: &[String], password: &str) -> Result<String, ShareError> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    let output = run_timed_with_stdin(
        &mut cmd,
        SHARES_SERVICE_TIMEOUT,
        format!("{password}\n").as_bytes(),
    )
    .map_err(ShareError::CommandFailed)?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(if text.is_empty() {
            "domain operation succeeded".to_string()
        } else {
            text
        })
    } else {
        Err(ShareError::CommandFailed(format!(
            "{} failed: {}",
            argv.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Files/services a Kerberos NFS export needs. Split for a controlled test.
pub const NFS_KERBEROS_PATHS: &[&str] = &["/etc/krb5.keytab", "/usr/sbin/rpc.svcgssd"];

pub fn nfs_kerberos_available_at(paths: &[&str]) -> bool {
    paths.iter().all(|path| std::path::Path::new(path).exists())
}

pub fn nfs_kerberos_available() -> bool {
    nfs_kerberos_available_at(NFS_KERBEROS_PATHS)
        || std::path::Path::new("/usr/sbin/gssproxy").exists()
}

pub struct SharesService;

impl SharesService {
    /// List all shares across all configured protocols.
    pub fn get_shares() -> Result<Vec<Share>, ShareError> {
        let mut shares = Vec::new();

        if let Ok(smb_shares) = Self::parse_smb_config() {
            shares.extend(smb_shares);
        }
        if let Ok(nfs_shares) = Self::parse_nfs_exports() {
            shares.extend(nfs_shares);
        }
        if let Ok(ftp_shares) = Self::parse_ftp_shares() {
            shares.extend(ftp_shares);
        }
        if let Ok(webdav_shares) = Self::parse_webdav_shares() {
            shares.extend(webdav_shares);
        }
        if let Ok(sftp_shares) = Self::parse_sftp_shares() {
            shares.extend(sftp_shares);
        }

        Ok(shares)
    }

    /// Return a single share matching both name and protocol type.
    pub fn get_share(name: &str, share_type: ShareType) -> Result<Share, ShareError> {
        let shares = Self::get_shares()?;
        shares
            .into_iter()
            .find(|s| s.name == name && s.share_type == share_type)
            .ok_or_else(|| ShareError::NotFound(name.to_string()))
    }

    /// Update fields on an existing share. Dispatches to the protocol-specific handler.
    pub fn update_share(
        name: &str,
        share_type: ShareType,
        update: ShareUpdate,
    ) -> Result<(), ShareError> {
        // Validate every field that will be formatted into a root-owned
        // config file before dispatching (see the validators' docs).
        validate_share_name(name).map_err(ShareError::InvalidField)?;
        if let Some(path) = &update.path {
            validate_share_path(path).map_err(ShareError::InvalidField)?;
        }
        if let Some(comment) = &update.comment {
            validate_share_comment(comment).map_err(ShareError::InvalidField)?;
        }
        if let Some(options) = &update.options {
            validate_nfs_options(options).map_err(ShareError::InvalidField)?;
        }
        if share_type == ShareType::Smb
            && update
                .restrictions
                .is_some_and(|restrictions| restrictions != ShareRestrictions::default())
        {
            return Err(ShareError::InvalidField(
                "advanced restrictions are not supported for SMB; use POSIX/ACL permissions"
                    .to_string(),
            ));
        }
        match share_type {
            ShareType::Smb => Self::update_smb_share(name, update),
            ShareType::Nfs => Self::update_nfs_share(name, update),
            ShareType::Ftp => Self::update_ftp_share(name, update),
            ShareType::Webdav => Self::update_webdav_share(name, update),
            ShareType::Sftp => Self::update_sftp_share(name, update),
        }
    }

    /// Create a new share. Dispatches to the protocol-specific handler.
    pub fn create_share(share: &Share) -> Result<(), ShareError> {
        // Validate before any privileged side effect — these values end up
        // verbatim in `smb.conf` / `/etc/exports` / `vsftpd.conf`.
        validate_share_name(&share.name).map_err(ShareError::InvalidField)?;
        validate_share_path(&share.path).map_err(ShareError::InvalidField)?;
        validate_share_comment(&share.comment).map_err(ShareError::InvalidField)?;
        validate_nfs_options(&share.options).map_err(ShareError::InvalidField)?;
        if share.share_type == ShareType::Smb
            && share.restrictions != ShareRestrictions::default()
        {
            return Err(ShareError::InvalidField(
                "advanced restrictions are not supported for SMB; use POSIX/ACL permissions"
                    .to_string(),
            ));
        }
        match share.share_type {
            ShareType::Smb => Self::create_smb_share(share),
            ShareType::Nfs => Self::create_nfs_share(share),
            ShareType::Ftp => Self::create_ftp_share(share),
            ShareType::Webdav => Self::create_webdav_share(share),
            ShareType::Sftp => Self::create_sftp_share(share),
        }
    }

    /// Delete a share. Dispatches to the protocol-specific handler.
    pub fn delete_share(name: &str, share_type: ShareType) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;
        match share_type {
            ShareType::Smb => Self::delete_smb_share(name),
            ShareType::Nfs => Self::delete_nfs_share(name),
            ShareType::Ftp => Self::delete_ftp_share(name),
            ShareType::Webdav => Self::delete_webdav_share(name),
            ShareType::Sftp => Self::delete_sftp_share(name),
        }
    }

    /// Enable or disable a share. For SMB, toggles `browseable`; for NFS,
    /// rewrites the `/etc/exports` line (tagged-comment form, so the state
    /// survives a reboot) and reloads exports.
    pub fn set_share_enabled(
        name: &str,
        share_type: ShareType,
        enabled: bool,
    ) -> Result<(), ShareError> {
        match share_type {
            ShareType::Smb => {
                validate_share_name(name).map_err(ShareError::InvalidField)?;
                let config_path = smb_conf_path();
                if !std::path::Path::new(&config_path).exists() {
                    warn!("Samba config file not found: {}", config_path);
                    return Err(ShareError::ConfigNotFound(config_path.clone()));
                }
                let config = std::fs::read_to_string(&config_path)?;
                let share_section = format!("[{}]", name);

                if let Some(start) = config.find(&share_section) {
                    let after_section = &config[start..];
                    // The section ends at the next `\n[` — or, when this is
                    // the last section in the file (the common case for a
                    // freshly HomeVault-created share, since they're appended),
                    // at end-of-string. Without the EOF fallback the toggle
                    // was a silent no-op for that share.
                    let end = after_section.find("\n[").unwrap_or(after_section.len());
                    let section_content = &after_section[..end];

                    let new_content = if enabled {
                        section_content.replace("browseable = no", "browseable = yes")
                    } else {
                        section_content.replace("browseable = yes", "browseable = no")
                    };

                    let mut new_config = config[..start].to_string();
                    new_config.push_str(&new_content);
                    new_config.push_str(&after_section[end..]);

                    write_config_file(&config_path, &new_config)?;
                }
            }
            ShareType::Nfs => Self::set_nfs_share_enabled(name, enabled)?,
            ShareType::Ftp | ShareType::Webdav | ShareType::Sftp => {
                let filename = format!("{}.json", name);
                let shares_path = match share_type {
                    ShareType::Ftp => get_ftp_shares_path(),
                    ShareType::Sftp => get_sftp_shares_path(),
                    _ => get_webdav_shares_path(),
                };

                if shares_path.exists() {
                    let mut shares: HashMap<String, serde_json::Value> =
                        serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
                    if let Some(entry) = shares.get_mut(&filename) {
                        entry["enabled"] = serde_json::json!(enabled);
                    }
                    write_file_as_root(
                        &shares_path.to_string_lossy(),
                        &serde_json::to_string_pretty(&shares)?,
                    )?;
                }
            }
        }

        Ok(())
    }

    /// Map of SMB share name → recycle-bin enabled, derived from the live
    /// `smb.conf` (a share is recycling when its managed block lists the
    /// `recycle` module). The UI reads this instead of re-deriving the state
    /// from raw config lines.
    pub fn get_smb_recycle_map() -> HashMap<String, bool> {
        smb_section_states()
            .into_iter()
            .map(|(name, (flags, _))| (name, flags.recycle))
            .collect()
    }

    /// Enable or disable the recycle bin for one SMB share. Rewrites only that
    /// share's section in `smb.conf` — preserving every other section and
    /// every hand-added line — then asks `smbd` to reload. The change reaches
    /// the Trash screen too, because the repository folder is the one the
    /// Explorer already reads.
    pub fn set_smb_recycle(name: &str, enabled: bool) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;

        let config_path = smb_conf_path();
        if !std::path::Path::new(&config_path).exists() {
            warn!("Samba config file not found: {}", config_path);
            return Err(ShareError::ConfigNotFound(config_path));
        }
        let config = std::fs::read_to_string(&config_path)?;
        let share_section = format!("[{name}]");

        let Some(start) = config.find(&share_section) else {
            return Err(ShareError::NotFound(name.to_string()));
        };
        let after_section = &config[start..];
        let end = after_section.find("\n[").unwrap_or(after_section.len());
        let section = &config[start..start + end];

        if section_has_recycle(section) == enabled {
            // Already in the requested state — do not rewrite the file or
            // bounce the service for a no-op.
            return Ok(());
        }

        let new_section = apply_recycle_to_section(section, enabled);
        let mut new_config = config[..start].to_string();
        new_config.push_str(&new_section);
        new_config.push_str(&config[start + end..]);
        write_config_file(&config_path, &new_config)?;
        reload_smbd(&format!(
            "{} the recycle bin on share {}",
            if enabled { "enabling" } else { "disabling" },
            name
        ));

        Ok(())
    }

    /// Map of SMB share name → Previous Versions (`shadow_copy2`) enabled.
    pub fn get_smb_previous_versions_map() -> HashMap<String, bool> {
        smb_section_states()
            .into_iter()
            .map(|(name, (flags, _))| (name, flags.previous_versions))
            .collect()
    }

    /// Enable/disable `vfs_shadow_copy2` on one share so Windows Explorer's
    /// "Previous Versions" tab lists the host's snapshots.
    pub fn set_smb_previous_versions(name: &str, enabled: bool) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;
        let config_path = smb_conf_path();
        if !std::path::Path::new(&config_path).exists() {
            return Err(ShareError::ConfigNotFound(config_path));
        }
        let config = std::fs::read_to_string(&config_path)?;
        let share_section = format!("[{name}]");
        let Some(start) = config.find(&share_section) else {
            return Err(ShareError::NotFound(name.to_string()));
        };
        let after_section = &config[start..];
        let end = after_section.find("\n[").unwrap_or(after_section.len());
        let section = &config[start..start + end];
        if section_vfs_flags(section).previous_versions == enabled {
            return Ok(());
        }
        let new_section = apply_previous_versions_to_section(section, enabled);
        let mut new_config = config[..start].to_string();
        new_config.push_str(&new_section);
        new_config.push_str(&config[start + end..]);
        write_config_file(&config_path, &new_config)?;
        reload_smbd(&format!(
            "{} Previous Versions on share {}",
            if enabled { "enabling" } else { "disabling" },
            name
        ));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Time Machine (per-share `vfs_fruit` profile)
    // -----------------------------------------------------------------------

    /// Every SMB share's Time Machine state, derived from the live config —
    /// the same "the file is the source of truth" rule as the recycle map. A
    /// share with no config file simply has no entry.
    pub fn get_smb_time_machine_map() -> HashMap<String, SmbTimeMachine> {
        smb_section_states()
            .into_iter()
            .map(|(name, (flags, quota_gb))| {
                (
                    name,
                    SmbTimeMachine {
                        enabled: flags.time_machine,
                        quota_gb: if flags.time_machine { quota_gb } else { 0 },
                    },
                )
            })
            .collect()
    }

    /// Enable or disable Time Machine on one SMB share, optionally with a
    /// maximum size macOS must respect (`quota_gb = 0` means unlimited).
    ///
    /// Rewrites only this share's managed block, preserving every other
    /// section, the recycle block's own directives and every hand-added line.
    /// The mDNS publisher picks the change up on its next tick, which is what
    /// makes the volume appear in macOS' Time Machine preferences.
    pub fn set_smb_time_machine(
        name: &str,
        enabled: bool,
        quota_gb: u32,
    ) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;
        validate_time_machine_quota(quota_gb).map_err(ShareError::InvalidField)?;

        let config_path = smb_conf_path();
        if !std::path::Path::new(&config_path).exists() {
            warn!("Samba config file not found: {}", config_path);
            return Err(ShareError::ConfigNotFound(config_path));
        }
        let config = std::fs::read_to_string(&config_path)?;
        let share_section = format!("[{name}]");

        let Some(start) = config.find(&share_section) else {
            return Err(ShareError::NotFound(name.to_string()));
        };
        let after_section = &config[start..];
        let end = after_section.find("\n[").unwrap_or(after_section.len());
        let section = &config[start..start + end];

        let mut flags = section_vfs_flags(section);
        let quota = if enabled { quota_gb } else { 0 };
        if flags.time_machine == enabled
            && (!enabled || section_time_machine_quota(section) == quota)
        {
            // A no-op must not rewrite the file or bounce smbd.
            return Ok(());
        }
        flags.time_machine = enabled;
        let new_section = apply_vfs_to_section(section, flags, quota);
        let mut new_config = config[..start].to_string();
        new_config.push_str(&new_section);
        new_config.push_str(&config[start + end..]);
        write_config_file(&config_path, &new_config)?;
        reload_smbd(&format!(
            "{} Time Machine on share {}",
            if enabled { "enabling" } else { "disabling" },
            name
        ));
        Ok(())
    }

    /// Distinct filesystem paths of every configured SMB share. The Explorer
    /// uses these to also surface each share's own recycle folder in the
    /// Trash screen, so a file deleted over the network is visible and
    /// restorable next to a file deleted from the web UI.
    pub fn smb_share_paths() -> Vec<std::path::PathBuf> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        if let Ok(shares) = Self::parse_smb_config() {
            for share in shares {
                if !share.path.is_empty() && seen.insert(share.path.clone()) {
                    out.push(std::path::PathBuf::from(share.path));
                }
            }
        }
        out
    }

    /// Return the SMB service config, or a default if no config file exists.
    pub fn get_smb_config() -> Result<SmbConfig, ShareError> {
        let path = get_smb_config_path();
        if path.exists() {
            let data = fs::read_to_string(path)?;
            let mut config: SmbConfig = serde_json::from_str(&data)?;
            normalize_smb_config(&mut config);
            Ok(config)
        } else {
            Ok(SmbConfig::default())
        }
    }

    /// Persist the SMB service config to disk and apply it to smb.conf.
    pub fn save_smb_config(config: &SmbConfig) -> Result<(), ShareError> {
        validate_smb_config(config).map_err(ShareError::InvalidField)?;
        let path = get_smb_config_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(config)?,
        )?;
        Self::apply_smb_config(config)?;
        Ok(())
    }

    fn apply_smb_config(config: &SmbConfig) -> Result<(), ShareError> {
        let smb_conf = smb_conf_path();
        let smb_conf = smb_conf.as_str();
        let new_params = smb_global_params(config);

        let content = if !std::path::Path::new(smb_conf).exists() {
            warn!("Samba config file not found: {}", smb_conf);
            format!("[global]\n{}\n", new_params.join("\n"))
        } else {
            let mut lines: Vec<String> = fs::read_to_string(smb_conf)?
                .lines()
                .map(String::from)
                .collect();

            let found_global = lines
                .iter()
                .any(|line| line.trim().eq_ignore_ascii_case("[global]"));
            if !found_global {
                lines.insert(0, "[global]".to_string());
            }

            // Remove every key we manage (exact key match, comments kept).
            let managed: Vec<String> = new_params
                .iter()
                .filter_map(|param| {
                    param
                        .trim()
                        .split_once('=')
                        .map(|(key, _)| key.trim().to_ascii_lowercase())
                })
                .collect();
            lines.retain(|line| match smb_line_key(line) {
                Some(key) => !managed.contains(&key),
                None => true,
            });

            // Recompute the insertion point after removals.
            let mut found = false;
            let mut insert_after = 0;
            for (i, line) in lines.iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.eq_ignore_ascii_case("[global]") {
                    found = true;
                    continue;
                }
                if found && trimmed.starts_with('[') {
                    insert_after = i;
                    break;
                }
                if found {
                    insert_after = i + 1;
                }
            }
            for (offset, param) in new_params.iter().enumerate() {
                lines.insert(insert_after + offset, param.clone());
            }
            lines.join("\n")
        };

        // Validate the candidate *before* replacing the file: a `testparm`
        // rejection must leave the working configuration untouched.
        Self::validate_smb_content(&content).map_err(ShareError::InvalidField)?;
        write_config_file(smb_conf, &content)?;

        let mut cmd = Command::new("sudo");
        cmd.arg("systemctl");
        cmd.args(["reload", "smbd"]);
        if let Err(e) = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT) {
            warn!("Failed to reload smbd after applying SMB config: {}", e);
        }
        Ok(())
    }

    /// Validate a candidate `smb.conf` with Samba's own parser.
    ///
    /// `testparm -s <file>` reads exactly the syntax `smbd` will read. A
    /// missing `testparm` (Samba not installed) is not a reason to refuse a
    /// save: the caller only runs this when SMB is being configured, and the
    /// service will report the real error at start. Any *rejection* is
    /// authoritative and aborts the write.
    fn validate_smb_content(content: &str) -> Result<(), String> {
        let seq = VALIDATION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let candidate = std::env::temp_dir().join(format!(
            ".homevault-smb-{}-{nanos}-{seq}.conf",
            std::process::id()
        ));
        fs::write(&candidate, content)
            .map_err(|e| format!("could not stage smb.conf for validation: {e}"))?;

        let mut cmd = Command::new("testparm");
        cmd.args(["-s", &candidate.to_string_lossy()]);
        let result = run_command(&mut cmd, SHARES_WRITE_TIMEOUT);
        let _ = fs::remove_file(&candidate);

        match result {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let detail = if stderr.trim().is_empty() {
                    stdout.trim()
                } else {
                    stderr.trim()
                };
                Err(format!("testparm rejected smb.conf: {detail}"))
            }
            Err(e)
                if e.to_ascii_lowercase().contains("no such file")
                    || e.to_ascii_lowercase().contains("not found") =>
            {
                warn!("testparm is not installed; SMB config validation skipped");
                Ok(())
            }
            Err(e) => Err(format!("could not run testparm: {e}")),
        }
    }

    /// Repair an empty or invalid `smb.conf` at startup.
    ///
    /// The service must not stay down because a previous write was truncated
    /// (power loss, full disk). A `.bak` written by [`write_config_file`] is
    /// preferred; otherwise the HomeVault defaults are rebuilt. A valid file
    /// is never touched, so hand-edited configurations survive.
    pub fn self_heal_smb_config() -> Result<bool, ShareError> {
        let path = smb_conf_path();
        let file = std::path::Path::new(&path);
        if !file.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(file)?;
        if !content.trim().is_empty() && Self::validate_smb_content(&content).is_ok() {
            return Ok(false);
        }

        let reason = if content.trim().is_empty() {
            "the file is empty"
        } else {
            "testparm rejected the file"
        };
        Self::restore_smb_config(&path, reason)
    }

    fn restore_smb_config(path: &str, reason: &str) -> Result<bool, ShareError> {
        let backup = format!("{path}.bak");
        if let Ok(previous) = fs::read_to_string(&backup) {
            if !previous.trim().is_empty() && Self::validate_smb_content(&previous).is_ok() {
                write_config_file(path, &previous)?;
                warn!("Restored {} from {} ({})", path, backup, reason);
                return Ok(true);
            }
        }

        let defaults = format!(
            "[global]\n{}\n",
            smb_global_params(&SmbConfig::default()).join("\n")
        );
        if Self::validate_smb_content(&defaults).is_ok() {
            write_config_file(path, &defaults)?;
            warn!("Restored {} from defaults ({})", path, reason);
            return Ok(true);
        }

        Err(ShareError::InvalidField(format!(
            "{path} is invalid ({reason}) and no valid backup or default could be written"
        )))
    }

    /// Return the NFS service config, or a default if no config file exists.
    pub fn get_nfs_config() -> Result<NfsConfig, ShareError> {
        let path = get_nfs_config_path();
        if path.exists() {
            let data = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(NfsConfig::default())
        }
    }

    /// Per-share NFS client rules, keyed by share name.
    pub fn get_all_nfs_rules() -> HashMap<String, Vec<NfsClientRule>> {
        let path = get_nfs_rules_path();
        let Ok(data) = fs::read_to_string(path) else {
            return HashMap::new();
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    pub fn get_nfs_rules(name: &str) -> Vec<NfsClientRule> {
        Self::get_all_nfs_rules()
            .remove(name)
            .unwrap_or_default()
    }

    pub fn save_nfs_rules(name: &str, rules: &[NfsClientRule]) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;
        validate_nfs_rules(rules).map_err(ShareError::InvalidField)?;
        let path = get_nfs_rules_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        let mut all = Self::get_all_nfs_rules();
        if rules.is_empty() {
            all.remove(name);
        } else {
            all.insert(name.to_string(), rules.to_vec());
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(&all)?,
        )?;

        // Re-render the one export line so the NFS server sees the rules now.
        let config = Self::get_nfs_config().unwrap_or_default();
        let exports = "/etc/exports";
        if std::path::Path::new(exports).exists() {
            let raw = std::fs::read_to_string(exports)?;
            let mut out = String::new();
            for line in raw.lines() {
                match parse_nfs_export_line(line) {
                    Some(share) if share.name == name => {
                        let plain = nfs_export_line(
                            &share.path,
                            &share.options,
                            &config,
                            &Self::get_nfs_rules(name),
                        );
                        if share.enabled {
                            out.push_str(plain.trim_end());
                        } else {
                            out.push_str(nfs_disabled_line(&share.name, &plain).trim_end());
                        }
                    }
                    _ => out.push_str(line),
                }
                out.push('\n');
            }
            write_file_as_root(exports, &out)?;
        }
        Self::reload_nfs_exports(name, "save NFS rules");
        Ok(())
    }

    /// Persist the NFS service config to disk and re-render every export line
    /// so the saved defaults actually apply. Before this, the fields were
    /// persisted and ignored: `/etc/exports` kept `*(rw,sync,no_subtree_check)`
    /// whatever the user chose.
    pub fn save_nfs_config(config: &NfsConfig) -> Result<(), ShareError> {
        validate_nfs_config(config).map_err(ShareError::InvalidField)?;
        if config.security_flavor.trim() != "sys" && !nfs_kerberos_available() {
            return Err(ShareError::Unavailable(
                "NFS Kerberos needs /etc/krb5.keytab and rpc.svcgssd (or gssproxy): configure the KDC and keytab before selecting krb5/krb5i/krb5p"
                    .to_string(),
            ));
        }
        let path = get_nfs_config_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(config)?,
        )?;
        Self::rewrite_nfs_exports(config)?;
        Self::reload_nfs_exports("all shares", "apply NFS service config");
        Ok(())
    }

    /// Re-render every export line in `/etc/exports` with `config`. The file
    /// is HomeVault-owned; per-share options are preserved and override the
    /// service defaults by class, while blank lines and plain comments are
    /// left untouched.
    fn rewrite_nfs_exports(config: &NfsConfig) -> Result<(), ShareError> {
        let exports_path = "/etc/exports";
        if !std::path::Path::new(exports_path).exists() {
            return Ok(());
        }
        let raw = std::fs::read_to_string(exports_path)?;
        let all_rules = Self::get_all_nfs_rules();
        let mut out = String::new();
        for line in raw.lines() {
            if let Some(share) = parse_nfs_export_line(line) {
                let rules = all_rules.get(&share.name).map(Vec::as_slice).unwrap_or(&[]);
                let plain = nfs_export_line(&share.path, &share.options, config, rules);
                if share.enabled {
                    out.push_str(plain.trim_end());
                } else {
                    out.push_str(nfs_disabled_line(&share.name, &plain).trim_end());
                }
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        write_file_as_root(exports_path, &out)?;
        Ok(())
    }

    /// Start a network service via systemctl.
    pub fn start_service(share_type: ShareType) -> Result<(), ShareError> {
        // WebDAV has no systemd unit any more — it is the in-process listener
        // in `crate::webdav`. Callers must go through `WEBDAV.start/stop/reload`.
        if share_type == ShareType::Webdav {
            return Err(ShareError::CommandFailed(
                "WebDAV is served in-process (crate::webdav), not by systemctl".to_string(),
            ));
        }
        let service_name = match share_type {
            ShareType::Smb => "smbd",
            ShareType::Nfs => "nfs-server",
            ShareType::Ftp => "vsftpd",
            ShareType::Webdav => "homevault-webdav",
            ShareType::Sftp => "homevault-sftp",
        };
        // A truncated or hand-broken smb.conf must not keep the service down
        // forever: restore the last good version (or the HomeVault default)
        // before asking systemd to start smbd.
        if share_type == ShareType::Smb {
            if let Err(e) = Self::self_heal_smb_config() {
                warn!("SMB self-heal before start failed: {}", e);
            }
        }
        let mut cmd = Command::new("sudo");
        cmd.arg("systemctl");
        cmd.args(["start", service_name]);
        let output = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT).map_err(|e| {
            ShareError::CommandFailed(format!("Failed to start {}: {}", service_name, e))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ShareError::CommandFailed(format!(
                "Failed to start {}: {}",
                service_name, stderr
            )));
        }
        Ok(())
    }

    /// Stop a network service via systemctl.
    pub fn stop_service(share_type: ShareType) -> Result<(), ShareError> {
        // WebDAV has no systemd unit any more — it is the in-process listener
        // in `crate::webdav`. Callers must go through `WEBDAV.start/stop/reload`.
        if share_type == ShareType::Webdav {
            return Err(ShareError::CommandFailed(
                "WebDAV is served in-process (crate::webdav), not by systemctl".to_string(),
            ));
        }
        let service_name = match share_type {
            ShareType::Smb => "smbd",
            ShareType::Nfs => "nfs-server",
            ShareType::Ftp => "vsftpd",
            ShareType::Webdav => "homevault-webdav",
            ShareType::Sftp => "homevault-sftp",
        };
        let mut cmd = Command::new("sudo");
        cmd.arg("systemctl");
        cmd.args(["stop", service_name]);
        let output = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT).map_err(|e| {
            ShareError::CommandFailed(format!("Failed to stop {}: {}", service_name, e))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ShareError::CommandFailed(format!(
                "Failed to stop {}: {}",
                service_name, stderr
            )));
        }
        Ok(())
    }

    /// Restart a network service via systemctl.
    pub fn restart_service(share_type: ShareType) -> Result<(), ShareError> {
        // WebDAV has no systemd unit any more — it is the in-process listener
        // in `crate::webdav`. Callers must go through `WEBDAV.start/stop/reload`.
        if share_type == ShareType::Webdav {
            return Err(ShareError::CommandFailed(
                "WebDAV is served in-process (crate::webdav), not by systemctl".to_string(),
            ));
        }
        let service_name = match share_type {
            ShareType::Smb => "smbd",
            ShareType::Nfs => "nfs-server",
            ShareType::Ftp => "vsftpd",
            ShareType::Webdav => "homevault-webdav",
            ShareType::Sftp => "homevault-sftp",
        };
        let mut cmd = Command::new("sudo");
        cmd.arg("systemctl");
        cmd.args(["restart", service_name]);
        let output = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT).map_err(|e| {
            ShareError::CommandFailed(format!("Failed to restart {}: {}", service_name, e))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ShareError::CommandFailed(format!(
                "Failed to restart {}: {}",
                service_name, stderr
            )));
        }
        Ok(())
    }

    /// Return the FTP service config, or a default if no config file exists.
    pub fn get_ftp_config() -> Result<FtpConfig, ShareError> {
        let path = get_ftp_config_path();
        if path.exists() {
            let data = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(FtpConfig::default())
        }
    }

    /// Persist the FTP service config to disk and apply it to vsftpd.conf.
    pub fn save_ftp_config(config: &FtpConfig) -> Result<(), ShareError> {
        validate_ftp_config(config).map_err(ShareError::InvalidField)?;
        let path = get_ftp_config_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(config)?,
        )?;
        Self::apply_ftp_config(config)?;
        Ok(())
    }

    fn apply_ftp_config(config: &FtpConfig) -> Result<(), ShareError> {
        let vsftpd_conf = "/etc/vsftpd.conf";
        // The whitelist lives in its own root-owned file, read by vsftpd
        // through `userlist_file`. Writing it before the config means a
        // reload never sees a half-applied pair.
        write_file_as_root(
            "/etc/vsftpd.user_list",
            &ftp_user_list_text(&config.allowed_users),
        )?;
        write_file_as_root(vsftpd_conf, &ftp_config_text(config))?;

        let mut cmd = Command::new("sudo");
        cmd.arg("systemctl");
        cmd.args(["reload", "vsftpd"]);
        if let Err(e) = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT) {
            warn!("Failed to reload vsftpd: {}", e);
        }
        Ok(())
    }

    /// Return the WebDAV service config, or a default if no config file exists.
    pub fn get_webdav_config() -> Result<WebdavConfig, ShareError> {
        let path = get_webdav_config_path();
        if path.exists() {
            let data = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(WebdavConfig::default())
        }
    }

    /// Persist the WebDAV service config to disk.
    ///
    /// The server itself is the in-process listener in [`crate::webdav`]: it
    /// is (re)built from this file and the live share table, so there is no
    /// generated script, no systemd unit and no root-owned process to manage.
    pub fn save_webdav_config(config: &WebdavConfig) -> Result<(), ShareError> {
        let path = get_webdav_config_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(config)?,
        )?;
        Ok(())
    }

    /// Return the SFTP service config, or a default if no config file exists.
    pub fn get_sftp_config() -> Result<SftpConfig, ShareError> {
        let path = get_sftp_config_path();
        if path.exists() {
            let data = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(SftpConfig::default())
        }
    }

    /// Persist the SFTP service config to disk and regenerate the dedicated
    /// sshd instance's config file + systemd unit.
    pub fn save_sftp_config(config: &SftpConfig) -> Result<(), ShareError> {
        let path = get_sftp_config_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(config)?,
        )?;
        crate::sftp::SftpJailService::apply_config(config)
            .map_err(|e| ShareError::CommandFailed(e.to_string()))?;
        Ok(())
    }

    fn create_smb_share(share: &Share) -> Result<(), ShareError> {
        let config_path = smb_conf_path();
        let access = SmbShareAccess {
            valid_users: SmbShareAccess::admin_only().valid_users,
            read_only: share.read_only,
            guest_ok: share.guest_ok,
        };
        let share_section = smb_share_section(&share.name, &share.comment, &share.path, &access);

        append_config_file(&config_path, &share_section)?;
        reload_smbd(&format!("creating share {}", share.name));

        Ok(())
    }

    fn create_nfs_share(share: &Share) -> Result<(), ShareError> {
        let exports_path = "/etc/exports";
        let nfs_config = Self::get_nfs_config().unwrap_or_default();
        let export_line = nfs_export_line(
            &share.path,
            &share.options,
            &nfs_config,
            &Self::get_nfs_rules(&share.name),
        );

        append_file_as_root(exports_path, &export_line)?;

        let mut cmd = Command::new("sudo");
        cmd.arg("exportfs");
        cmd.args(["-ra"]);
        if let Err(e) = run_command(&mut cmd, SHARES_SERVICE_TIMEOUT).map(|o| o.status.success()) {
            warn!("Failed to reload NFS exports: {}", e);
        }

        Ok(())
    }

    fn create_ftp_share(share: &Share) -> Result<(), ShareError> {
        let shares_path = get_ftp_shares_path();
        if let Some(parent) = shares_path.parent() {
            mkdir_as_root(parent)?;
        }

        let mut shares: HashMap<String, serde_json::Value> = if shares_path.exists() {
            let data = fs::read_to_string(&shares_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let key = format!("{}.json", share.name);
        let entry = serde_json::json!({
            "name": share.name,
            "path": share.path,
            "comment": share.comment,
            "enabled": share.enabled,
            "options": share.options,
            "read_only": share.read_only,
            "guest_ok": share.guest_ok,
            "restrictions": share.restrictions,
        });

        shares.insert(key, entry);
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn create_webdav_share(share: &Share) -> Result<(), ShareError> {
        let shares_path = get_webdav_shares_path();
        if let Some(parent) = shares_path.parent() {
            mkdir_as_root(parent)?;
        }

        let mut shares: HashMap<String, serde_json::Value> = if shares_path.exists() {
            let data = fs::read_to_string(&shares_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let key = format!("{}.json", share.name);
        let entry = serde_json::json!({
            "name": share.name,
            "path": share.path,
            "comment": share.comment,
            "enabled": share.enabled,
            "options": share.options,
            "read_only": share.read_only,
            "guest_ok": share.guest_ok,
            "restrictions": share.restrictions,
        });

        shares.insert(key, entry);
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn create_sftp_share(share: &Share) -> Result<(), ShareError> {
        let shares_path = get_sftp_shares_path();
        if let Some(parent) = shares_path.parent() {
            mkdir_as_root(parent)?;
        }

        let mut shares: HashMap<String, serde_json::Value> = if shares_path.exists() {
            let data = fs::read_to_string(&shares_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let key = format!("{}.json", share.name);
        let entry = serde_json::json!({
            "name": share.name,
            "path": share.path,
            "comment": share.comment,
            "enabled": share.enabled,
            "options": share.options,
            "read_only": share.read_only,
            "guest_ok": share.guest_ok,
            "restrictions": share.restrictions,
        });

        shares.insert(key, entry);
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn parse_smb_config() -> Result<Vec<Share>, ShareError> {
        let config_path = smb_conf_path();
        if !std::path::Path::new(&config_path).exists() {
            warn!("Samba config file not found: {}", config_path);
            return Ok(Vec::new());
        }

        let config = std::fs::read_to_string(&config_path).map_err(|e| {
            warn!("Failed to read Samba config: {}", e);
            e
        })?;

        let mut shares = Vec::new();
        let mut current_share: Option<Share> = None;
        let mut in_share = false;

        for line in config.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                if let Some(share) = current_share.take() {
                    shares.push(share);
                }

                let name = trimmed[1..trimmed.len() - 1].to_string();

                if name != "global" && name != "homes" {
                    current_share = Some(Share {
                        name,
                        share_type: ShareType::Smb,
                        path: String::new(),
                        enabled: true,
                        comment: String::new(),
                        options: Vec::new(),
                        read_only: false,
                        guest_ok: None,
                        restrictions: ShareRestrictions::default(),
                    });
                    in_share = true;
                } else {
                    in_share = false;
                }
            } else if in_share {
                if let Some(ref mut share) = current_share {
                    if let Some(stripped) = trimmed.strip_prefix("path = ") {
                        share.path = stripped.to_string();
                    } else if let Some(stripped) = trimmed.strip_prefix("comment = ") {
                        share.comment = stripped.to_string();
                    } else if trimmed == "browseable = no" {
                        share.enabled = false;
                    } else if trimmed == "read only = yes" {
                        share.read_only = true;
                    } else if trimmed == "read only = no" {
                        share.read_only = false;
                    } else if trimmed == "guest ok = yes" || trimmed == "public = yes" {
                        share.guest_ok = Some(true);
                    } else if trimmed == "guest ok = no" || trimmed == "public = no" {
                        share.guest_ok = Some(false);
                    } else if trimmed.starts_with("valid users =") {
                        // Managed from `share_permissions`; not a user-editable
                        // option, and rendering it in the UI would invite a
                        // stale list after a grant/revoke.
                    } else if !trimmed.is_empty()
                        && !trimmed.starts_with(';')
                        && !trimmed.starts_with('#')
                    {
                        // `#` lines are Samba comments — including the
                        // HomeVault recycle block's markers and directives —
                        // and are not user-visible options.
                        share.options.push(trimmed.to_string());
                    }
                }
            }
        }

        if let Some(share) = current_share {
            shares.push(share);
        }

        Ok(shares)
    }

    fn parse_nfs_exports() -> Result<Vec<Share>, ShareError> {
        let exports_path = "/etc/exports";
        if !std::path::Path::new(exports_path).exists() {
            warn!("NFS exports file not found: {}", exports_path);
            return Ok(Vec::new());
        }

        let exports = std::fs::read_to_string(exports_path).map_err(|e| {
            warn!("Failed to read NFS exports: {}", e);
            e
        })?;

        let mut shares = Vec::new();

        for line in exports.lines() {
            if let Some(share) = parse_nfs_export_line(line) {
                shares.push(share);
            }
        }

        Ok(shares)
    }

    /// Read the optional advanced-restriction object from a JSON share entry.
    /// A missing object means "no restriction", not an error: old files must
    /// keep loading after an upgrade.
    fn parse_restrictions(value: &serde_json::Value) -> ShareRestrictions {
        // (associated so the three protocol parsers share one implementation)
        let Some(object) = value.get("restrictions").and_then(|v| v.as_object()) else {
            return ShareRestrictions::default();
        };
        ShareRestrictions {
            disable_browsing: object
                .get("disable_browsing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            disable_modification: object
                .get("disable_modification")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            disable_download: object
                .get("disable_download")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    }

    fn parse_ftp_shares() -> Result<Vec<Share>, ShareError> {
        let shares_path = get_ftp_shares_path();
        if !shares_path.exists() {
            return Ok(Vec::new());
        }

        let data = fs::read_to_string(shares_path)?;
        let map: HashMap<String, serde_json::Value> = serde_json::from_str(&data)?;

        let mut shares = Vec::new();
        for (_, value) in map {
            if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                let path = value
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let comment = value
                    .get("comment")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let enabled = value
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let options = value
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                shares.push(Share {
                    name: name.to_string(),
                    share_type: ShareType::Ftp,
                    path,
                    enabled,
                    comment,
                    options,
                    read_only: value
                        .get("read_only")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    guest_ok: value.get("guest_ok").and_then(|v| v.as_bool()),
                    restrictions: Self::parse_restrictions(&value),
                });
            }
        }

        Ok(shares)
    }

    fn parse_webdav_shares() -> Result<Vec<Share>, ShareError> {
        let shares_path = get_webdav_shares_path();
        if !shares_path.exists() {
            return Ok(Vec::new());
        }

        let data = fs::read_to_string(shares_path)?;
        let map: HashMap<String, serde_json::Value> = serde_json::from_str(&data)?;

        let mut shares = Vec::new();
        for (_, value) in map {
            if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                let path = value
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let comment = value
                    .get("comment")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let enabled = value
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let options = value
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                shares.push(Share {
                    name: name.to_string(),
                    share_type: ShareType::Webdav,
                    path,
                    enabled,
                    comment,
                    options,
                    read_only: value
                        .get("read_only")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    guest_ok: value.get("guest_ok").and_then(|v| v.as_bool()),
                    restrictions: Self::parse_restrictions(&value),
                });
            }
        }

        Ok(shares)
    }

    fn parse_sftp_shares() -> Result<Vec<Share>, ShareError> {
        let shares_path = get_sftp_shares_path();
        if !shares_path.exists() {
            return Ok(Vec::new());
        }

        let data = fs::read_to_string(shares_path)?;
        let map: HashMap<String, serde_json::Value> = serde_json::from_str(&data)?;

        let mut shares = Vec::new();
        for (_, value) in map {
            if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                let path = value
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let comment = value
                    .get("comment")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let enabled = value
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let options = value
                    .get("options")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                shares.push(Share {
                    name: name.to_string(),
                    share_type: ShareType::Sftp,
                    path,
                    enabled,
                    comment,
                    options,
                    read_only: value
                        .get("read_only")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    guest_ok: value.get("guest_ok").and_then(|v| v.as_bool()),
                    restrictions: Self::parse_restrictions(&value),
                });
            }
        }

        Ok(shares)
    }

    fn delete_smb_share(name: &str) -> Result<(), ShareError> {
        let config_path = smb_conf_path();
        if !std::path::Path::new(&config_path).exists() {
            warn!("Samba config file not found: {}", config_path);
            return Err(ShareError::ConfigNotFound(config_path.clone()));
        }
        let config = std::fs::read_to_string(&config_path)?;
        let share_section = format!("[{}]", name);

        if let Some(start) = config.find(&share_section) {
            let after_section = &config[start..];
            let end = after_section.find("\n[").unwrap_or(after_section.len());

            let mut new_config = config[..start].to_string();
            new_config.push_str(&after_section[end..]);

            write_config_file(&config_path, &new_config)?;
            reload_smbd(&format!("deleting share {}", name));
        } else {
            return Err(ShareError::NotFound(name.to_string()));
        }

        Ok(())
    }

    fn delete_nfs_share(name: &str) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;

        let exports_path = "/etc/exports";
        if !std::path::Path::new(exports_path).exists() {
            warn!("NFS exports file not found: {}", exports_path);
            return Err(ShareError::ConfigNotFound(exports_path.to_string()));
        }
        let raw = std::fs::read_to_string(exports_path).map_err(|e| {
            warn!("Failed to read exports for deletion: {}", e);
            e
        })?;

        // Match by parsed share name, removing exactly the one matching line —
        // never by line-rewriting heuristics, and never more than one line.
        let mut matched: Vec<(usize, String)> = Vec::new();
        for (idx, line) in raw.lines().enumerate() {
            if let Some(share) = parse_nfs_export_line(line) {
                if share.name == name {
                    matched.push((idx, share.path));
                }
            }
        }

        match matched.len() {
            0 => return Err(ShareError::NotFound(name.to_string())),
            1 => {}
            _ => {
                let paths: Vec<&str> = matched.iter().map(|(_, p)| p.as_str()).collect();
                return Err(ShareError::InvalidField(format!(
                    "share name '{name}' is ambiguous in /etc/exports (matching paths: {}) — remove the other export first",
                    paths.join(", ")
                )));
            }
        }

        let drop_idx = matched[0].0;
        let mut new_exports = String::new();
        for (idx, line) in raw.lines().enumerate() {
            if idx == drop_idx {
                continue;
            }
            new_exports.push_str(line);
            new_exports.push('\n');
        }

        write_file_as_root(exports_path, &new_exports)?;
        Self::reload_nfs_exports(name, "deletion");
        Ok(())
    }

    /// Best-effort `exportfs -ra` after an `/etc/exports` rewrite: the file is
    /// the source of truth, so a reload failure is logged, not fatal (the
    /// export will be applied on the next reload/reboot either way).
    fn reload_nfs_exports(name: &str, context: &str) {
        let mut cmd = Command::new("sudo");
        cmd.arg("exportfs");
        cmd.args(["-ra"]);
        match run_command(&mut cmd, SHARES_SERVICE_TIMEOUT) {
            Ok(output) if output.status.success() => {}
            Ok(output) => warn!(
                "exportfs -ra failed after NFS share {} ({}): {}",
                context,
                name,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(e) => warn!(
                "Failed to reload NFS exports after {} for {}: {}",
                context, name, e
            ),
        }
    }

    fn delete_ftp_share(name: &str) -> Result<(), ShareError> {
        let shares_path = get_ftp_shares_path();
        if !shares_path.exists() {
            return Ok(());
        }

        let mut shares: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
        shares.remove(&format!("{}.json", name));
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn delete_webdav_share(name: &str) -> Result<(), ShareError> {
        let shares_path = get_webdav_shares_path();
        if !shares_path.exists() {
            return Ok(());
        }

        let mut shares: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
        shares.remove(&format!("{}.json", name));
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn delete_sftp_share(name: &str) -> Result<(), ShareError> {
        let shares_path = get_sftp_shares_path();
        if !shares_path.exists() {
            return Ok(());
        }

        let mut shares: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
        shares.remove(&format!("{}.json", name));
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn update_smb_share(name: &str, update: ShareUpdate) -> Result<(), ShareError> {
        let config_path = smb_conf_path();
        if !std::path::Path::new(&config_path).exists() {
            return Err(ShareError::ConfigNotFound(config_path.clone()));
        }

        let config = std::fs::read_to_string(&config_path)?;
        let share_section = format!("[{}]", name);

        if let Some(start) = config.find(&share_section) {
            let after_section = &config[start..];
            let end = after_section.find("\n[").unwrap_or(after_section.len());
            let section_content = &config[start..start + end];

            let mut new_content = section_content.to_string();

            if let Some(ref path) = update.path {
                if let Some(idx) = new_content.find("path = ") {
                    let line_end = new_content[idx..]
                        .find('\n')
                        .unwrap_or(new_content.len() - idx);
                    new_content = format!(
                        "{}path = {}{}",
                        &new_content[..idx],
                        path,
                        &new_content[idx + line_end..]
                    );
                }
            }

            if let Some(ref comment) = update.comment {
                if let Some(idx) = new_content.find("comment = ") {
                    let line_end = new_content[idx..]
                        .find('\n')
                        .unwrap_or(new_content.len() - idx);
                    new_content = format!(
                        "{}comment = {}{}",
                        &new_content[..idx],
                        comment,
                        &new_content[idx + line_end..]
                    );
                }
            }

            if let Some(enabled) = update.enabled {
                if enabled {
                    new_content = new_content.replace("browseable = no", "browseable = yes");
                } else {
                    new_content = new_content.replace("browseable = yes", "browseable = no");
                }
            }

            if let Some(read_only) = update.read_only {
                new_content = set_smb_managed_line(
                    &new_content,
                    "read only",
                    Some(if read_only { "yes" } else { "no" }),
                );
            }
            if let Some(guest_ok) = update.guest_ok {
                new_content = set_smb_managed_line(
                    &new_content,
                    "guest ok",
                    guest_ok.map(|v| if v { "yes" } else { "no" }),
                );
            }

            let mut new_config = config[..start].to_string();
            new_config.push_str(&new_content);
            new_config.push_str(&after_section[end..]);

            write_config_file(&config_path, &new_config)?;
            reload_smbd(&format!("updating share {}", name));
        } else {
            return Err(ShareError::NotFound(name.to_string()));
        }

        Ok(())
    }

    fn update_nfs_share(name: &str, update: ShareUpdate) -> Result<(), ShareError> {
        let exports_path = "/etc/exports";
        if !std::path::Path::new(exports_path).exists() {
            return Err(ShareError::ConfigNotFound(exports_path.to_string()));
        }

        let raw = std::fs::read_to_string(exports_path)?;

        // Resolve the single existing export line for `name` first, so a
        // basename collision across different parent directories is an
        // explicit error instead of a silent multi-line rewrite.
        let mut matched: Vec<(usize, Share)> = Vec::new();
        for (idx, line) in raw.lines().enumerate() {
            if let Some(share) = parse_nfs_export_line(line) {
                if share.name == name {
                    matched.push((idx, share));
                }
            }
        }
        let (line_idx, existing) = match matched.len() {
            0 => return Err(ShareError::NotFound(name.to_string())),
            1 => (matched[0].0, matched[0].1.clone()),
            _ => {
                let paths: Vec<&str> = matched.iter().map(|(_, s)| s.path.as_str()).collect();
                return Err(ShareError::InvalidField(format!(
                    "share name '{name}' is ambiguous in /etc/exports (matching paths: {}) — update the other export first",
                    paths.join(", ")
                )));
            }
        };

        let new_path = update.path.clone().unwrap_or_else(|| existing.path.clone());
        let new_options = update
            .options
            .clone()
            .unwrap_or_else(|| existing.options.clone());

        // Validation already ran in `update_share`; re-check here so a direct
        // caller can never write an unvalidated line.
        validate_share_path(&new_path).map_err(ShareError::InvalidField)?;
        validate_nfs_options(&new_options).map_err(ShareError::InvalidField)?;

        // Preserve the enable/disable state carried by the existing line.
        let nfs_config = Self::get_nfs_config().unwrap_or_default();
        let rules = Self::get_nfs_rules(name);
        let rendered = nfs_export_line(&new_path, &new_options, &nfs_config, &rules);
        let replacement = if existing.enabled {
            rendered
        } else {
            nfs_disabled_line(name, &rendered)
        };

        let mut new_exports = String::new();
        for (idx, line) in raw.lines().enumerate() {
            if idx == line_idx {
                new_exports.push_str(replacement.trim_end());
            } else {
                new_exports.push_str(line);
            }
            new_exports.push('\n');
        }

        write_file_as_root(exports_path, &new_exports)?;
        Self::reload_nfs_exports(name, "update");
        Ok(())
    }

    /// Rewrite `/etc/exports` so the single export line for `name` is
    /// disabled (prefixed with [`NFS_DISABLED_TAG`]) or re-enabled (tag
    /// removed), then reload. The tag makes the state survive a reboot, which
    /// the previous in-memory `exportfs -u` toggle did not.
    fn set_nfs_share_enabled(name: &str, enabled: bool) -> Result<(), ShareError> {
        validate_share_name(name).map_err(ShareError::InvalidField)?;

        let exports_path = "/etc/exports";
        if !std::path::Path::new(exports_path).exists() {
            return Err(ShareError::ConfigNotFound(exports_path.to_string()));
        }
        let raw = std::fs::read_to_string(exports_path)?;

        let mut active: Vec<(usize, String)> = Vec::new();
        let mut disabled: Vec<(usize, String)> = Vec::new();
        for (idx, line) in raw.lines().enumerate() {
            if let Some(share) = parse_nfs_export_line(line) {
                if share.name == name {
                    if share.enabled {
                        active.push((idx, share.path));
                    } else {
                        disabled.push((idx, share.path));
                    }
                }
            }
        }

        let candidates = if enabled { &disabled } else { &active };
        let already_in_state = if enabled { &active } else { &disabled };

        let target = match candidates.len() {
            0 => {
                if !already_in_state.is_empty() {
                    // Already in the requested state — idempotent no-op.
                    return Ok(());
                }
                return Err(ShareError::NotFound(name.to_string()));
            }
            1 => candidates[0].0,
            _ => {
                let paths: Vec<&str> = candidates.iter().map(|(_, p)| p.as_str()).collect();
                return Err(ShareError::InvalidField(format!(
                    "share name '{name}' is ambiguous in /etc/exports (matching paths: {}) — disambiguate by removing the duplicate export first",
                    paths.join(", ")
                )));
            }
        };

        let nfs_config = Self::get_nfs_config().unwrap_or_default();
        let mut new_exports = String::new();
        for (idx, line) in raw.lines().enumerate() {
            if idx == target {
                let parsed = parse_nfs_export_line(line)
                    .ok_or_else(|| ShareError::NotFound(name.to_string()))?;
                let plain = nfs_export_line(
                    &parsed.path,
                    &parsed.options,
                    &nfs_config,
                    &Self::get_nfs_rules(name),
                );
                if enabled {
                    // Un-tag: keep the export line as-is.
                    new_exports.push_str(plain.trim_end());
                } else {
                    new_exports.push_str(nfs_disabled_line(name, &plain).trim_end());
                }
            } else {
                new_exports.push_str(line);
            }
            new_exports.push('\n');
        }

        write_file_as_root(exports_path, &new_exports)?;
        Self::reload_nfs_exports(name, if enabled { "enable" } else { "disable" });
        Ok(())
    }

    fn update_ftp_share(name: &str, update: ShareUpdate) -> Result<(), ShareError> {
        let shares_path = get_ftp_shares_path();
        if !shares_path.exists() {
            return Err(ShareError::NotFound(name.to_string()));
        }

        let mut shares: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
        let key = format!("{}.json", name);

        if let Some(entry) = shares.get_mut(&key) {
            if let Some(path) = update.path {
                entry["path"] = serde_json::json!(path);
            }
            if let Some(comment) = update.comment {
                entry["comment"] = serde_json::json!(comment);
            }
            if let Some(enabled) = update.enabled {
                entry["enabled"] = serde_json::json!(enabled);
            }
            if let Some(options) = update.options {
                entry["options"] = serde_json::json!(options);
            }
            if let Some(read_only) = update.read_only {
                entry["read_only"] = serde_json::json!(read_only);
            }
            if let Some(guest_ok) = update.guest_ok {
                entry["guest_ok"] = match guest_ok {
                    Some(value) => serde_json::json!(value),
                    None => serde_json::Value::Null,
                };
            }
            if let Some(restrictions) = update.restrictions {
                entry["restrictions"] = serde_json::json!(restrictions);
            }
            write_file_as_root(
                &shares_path.to_string_lossy(),
                &serde_json::to_string_pretty(&shares)?,
            )?;
        } else {
            return Err(ShareError::NotFound(name.to_string()));
        }

        Ok(())
    }

    fn update_webdav_share(name: &str, update: ShareUpdate) -> Result<(), ShareError> {
        let shares_path = get_webdav_shares_path();
        if !shares_path.exists() {
            return Err(ShareError::NotFound(name.to_string()));
        }

        let mut shares: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
        let key = format!("{}.json", name);

        if let Some(entry) = shares.get_mut(&key) {
            if let Some(path) = update.path {
                entry["path"] = serde_json::json!(path);
            }
            if let Some(comment) = update.comment {
                entry["comment"] = serde_json::json!(comment);
            }
            if let Some(enabled) = update.enabled {
                entry["enabled"] = serde_json::json!(enabled);
            }
            if let Some(options) = update.options {
                entry["options"] = serde_json::json!(options);
            }
            if let Some(read_only) = update.read_only {
                entry["read_only"] = serde_json::json!(read_only);
            }
            if let Some(guest_ok) = update.guest_ok {
                entry["guest_ok"] = match guest_ok {
                    Some(value) => serde_json::json!(value),
                    None => serde_json::Value::Null,
                };
            }
            if let Some(restrictions) = update.restrictions {
                entry["restrictions"] = serde_json::json!(restrictions);
            }
            write_file_as_root(
                &shares_path.to_string_lossy(),
                &serde_json::to_string_pretty(&shares)?,
            )?;
        } else {
            return Err(ShareError::NotFound(name.to_string()));
        }

        Ok(())
    }

    fn update_sftp_share(name: &str, update: ShareUpdate) -> Result<(), ShareError> {
        let shares_path = get_sftp_shares_path();
        if !shares_path.exists() {
            return Err(ShareError::NotFound(name.to_string()));
        }

        let mut shares: HashMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(&shares_path)?).unwrap_or_default();
        let key = format!("{}.json", name);

        if let Some(entry) = shares.get_mut(&key) {
            if let Some(path) = update.path {
                entry["path"] = serde_json::json!(path);
            }
            if let Some(comment) = update.comment {
                entry["comment"] = serde_json::json!(comment);
            }
            if let Some(enabled) = update.enabled {
                entry["enabled"] = serde_json::json!(enabled);
            }
            if let Some(options) = update.options {
                entry["options"] = serde_json::json!(options);
            }
            if let Some(read_only) = update.read_only {
                entry["read_only"] = serde_json::json!(read_only);
            }
            if let Some(guest_ok) = update.guest_ok {
                entry["guest_ok"] = match guest_ok {
                    Some(value) => serde_json::json!(value),
                    None => serde_json::Value::Null,
                };
            }
            if let Some(restrictions) = update.restrictions {
                entry["restrictions"] = serde_json::json!(restrictions);
            }
            write_file_as_root(
                &shares_path.to_string_lossy(),
                &serde_json::to_string_pretty(&shares)?,
            )?;
        } else {
            return Err(ShareError::NotFound(name.to_string()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests: `ShareType` parsing/formatting and the default
    //! service configs. Everything that reads `/etc/samba/smb.conf`,
    //! `/etc/exports` or `/etc/homevault/*.json`, shells out via `sudo`, or
    //! talks to systemd needs the real host and is covered by
    //! `tests/e2e/shares/shares.spec.ts` / `tests/shares_integration.rs`.

    use super::*;

    /// Serializes the tests that point `HV_SMB_CONF` at a temporary file.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_sharetype_parse_canonical_names() {
        assert_eq!(ShareType::parse("smb"), Some(ShareType::Smb));
        assert_eq!(ShareType::parse("nfs"), Some(ShareType::Nfs));
        assert_eq!(ShareType::parse("ftp"), Some(ShareType::Ftp));
        assert_eq!(ShareType::parse("webdav"), Some(ShareType::Webdav));
        assert_eq!(ShareType::parse("sftp"), Some(ShareType::Sftp));
    }

    #[test]
    fn test_sharetype_parse_is_case_insensitive() {
        assert_eq!(ShareType::parse("SMB"), Some(ShareType::Smb));
        assert_eq!(ShareType::parse("Nfs"), Some(ShareType::Nfs));
        assert_eq!(ShareType::parse("WebDAV"), Some(ShareType::Webdav));
        assert_eq!(ShareType::parse("SFTP"), Some(ShareType::Sftp));
    }

    #[test]
    fn test_sharetype_parse_cifs_is_an_alias_for_smb() {
        // The only alias — used by clients that call SMB "CIFS".
        assert_eq!(ShareType::parse("cifs"), Some(ShareType::Smb));
        assert_eq!(ShareType::parse("CIFS"), Some(ShareType::Smb));
    }

    #[test]
    fn test_sharetype_parse_rejects_unknown() {
        for s in [
            "", "http", "s3", "webdavs", "ftps", "ssh", "scp", "nfsv4", " smb",
        ] {
            assert_eq!(ShareType::parse(s), None, "{s:?} must not parse");
        }
    }

    #[test]
    fn test_sharetype_as_str_matches_the_wire_protocol_strings() {
        assert_eq!(ShareType::Smb.as_str(), "smb");
        assert_eq!(ShareType::Nfs.as_str(), "nfs");
        assert_eq!(ShareType::Ftp.as_str(), "ftp");
        assert_eq!(ShareType::Webdav.as_str(), "webdav");
        assert_eq!(ShareType::Sftp.as_str(), "sftp");
    }

    #[test]
    fn test_sharetype_display_equals_as_str() {
        for t in [
            ShareType::Smb,
            ShareType::Nfs,
            ShareType::Ftp,
            ShareType::Webdav,
            ShareType::Sftp,
        ] {
            assert_eq!(format!("{t}"), t.as_str());
        }
    }

    #[test]
    fn test_sharetype_as_str_then_parse_round_trips() {
        for t in [
            ShareType::Smb,
            ShareType::Nfs,
            ShareType::Ftp,
            ShareType::Webdav,
            ShareType::Sftp,
        ] {
            assert_eq!(
                ShareType::parse(t.as_str()),
                Some(t.clone()),
                "round-trip for {t}"
            );
        }
    }

    #[test]
    fn test_sftp_config_default_port() {
        // The dedicated sshd instance must default to 2222, never 22 (the
        // host's own sshd) — a regression here would collide with real SSH.
        assert_eq!(SftpConfig::default().port, 2222);
        assert_ne!(SftpConfig::default().port, 22);
    }

    #[test]
    fn test_smb_config_default_is_not_guest_writable() {
        let d = SmbConfig::default();
        assert_eq!(d.workgroup, "WORKGROUP");
        assert_eq!(d.security_mode, "user");
        assert!(!d.allow_guest, "guest access must be off by default");
        assert_eq!(d.protocol_version, "SMB3");
    }

    #[test]
    fn test_nfs_config_default() {
        let d = NfsConfig::default();
        assert_eq!(d.networks, vec!["*".to_string()]);
        assert_eq!(
            d.root_squash, "root_squash",
            "root must be squashed by default"
        );
        assert_eq!(
            d.version, "auto",
            "an untouched install keeps kernel negotiation instead of forcing v4"
        );
    }

    #[test]
    fn test_share_json_round_trip() {
        let share = Share {
            name: "media".to_string(),
            share_type: ShareType::Sftp,
            path: "/mnt/pool/media".to_string(),
            enabled: true,
            comment: "family media".to_string(),
            options: vec!["ro".to_string()],
            read_only: false,
            guest_ok: None,
            restrictions: ShareRestrictions::default(),
        };
        let json = serde_json::to_string(&share).unwrap();
        let back: Share = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, share.name);
        assert_eq!(back.share_type, share.share_type);
        assert_eq!(back.path, share.path);
        assert_eq!(back.enabled, share.enabled);
        assert_eq!(back.options, share.options);
    }

    #[test]
    fn test_share_update_all_fields_optional_deserialization() {
        // The update handler builds a `ShareUpdate` from an all-optional
        // request body; an empty object must deserialize to all-None.
        let empty: ShareUpdate = serde_json::from_str("{}").unwrap();
        assert!(empty.path.is_none());
        assert!(empty.enabled.is_none());
        assert!(empty.comment.is_none());
        assert!(empty.options.is_none());

        let partial: ShareUpdate = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert_eq!(partial.enabled, Some(false));
        assert!(partial.path.is_none());
    }

    // -----------------------------------------------------------------------
    // Input validation — the format-injection guards
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_share_name_accepts_ordinary_names() {
        for name in ["media", "Media_2024", "a-b.c", "photos2", "x"] {
            assert!(validate_share_name(name).is_ok(), "{name:?} must be valid");
        }
    }

    #[test]
    fn test_validate_share_name_rejects_smb_section_forging() {
        // A newline or a bracket in a name would forge an extra Samba section
        // (or escape the section header) inside /etc/samba/smb.conf.
        for name in [
            "media\n[global]",
            "media\r[evil]",
            "media]",
            "[global]",
            "me dia",
            "a\tb",
            "a;b",
        ] {
            assert!(
                validate_share_name(name).is_err(),
                "{name:?} must be rejected"
            );
        }
    }

    #[test]
    fn test_validate_share_name_rejects_path_fragments_and_reserved_sections() {
        for name in [
            "media/pool",
            "media\\pool",
            "..",
            "a..b",
            ".hidden",
            "global",
            "GLOBAL",
            "homes",
            "printers",
            "admin$",
        ] {
            assert!(
                validate_share_name(name).is_err(),
                "{name:?} must be rejected"
            );
        }
    }

    #[test]
    fn test_validate_share_name_rejects_empty_whitespace_and_oversized() {
        assert!(validate_share_name("").is_err());
        assert!(validate_share_name(" media").is_err());
        assert!(validate_share_name("media ").is_err());
        assert!(validate_share_name(&"a".repeat(SHARE_NAME_MAX + 1)).is_err());
        assert!(validate_share_name(&"a".repeat(SHARE_NAME_MAX)).is_ok());
    }

    #[test]
    fn test_validate_share_comment_accepts_single_line_text() {
        for comment in ["", "Family media", "Café 2024", "a\tb"] {
            assert!(validate_share_comment(comment).is_ok(), "{comment:?}");
        }
    }

    #[test]
    fn test_validate_share_comment_rejects_newline_injection() {
        // `comment = <value>` is one smb.conf line: a newline would let the
        // value add directives to the section.
        for comment in [
            "ok\n  read only = no",
            "ok\rpath = /etc",
            "ok\u{0000}",
            "ok\u{0007}",
        ] {
            assert!(
                validate_share_comment(comment).is_err(),
                "{comment:?} must be rejected"
            );
        }
        assert!(validate_share_comment(&"c".repeat(201)).is_err());
    }

    #[test]
    fn test_validate_share_path_accepts_ordinary_absolute_paths() {
        for path in ["/mnt/pool/media", "/srv/data", "/", "/mnt/a-b/c.d"] {
            assert!(validate_share_path(path).is_ok(), "{path:?}");
        }
    }

    #[test]
    fn test_validate_share_path_rejects_relative_paths_and_traversal() {
        for path in ["mnt/pool", "", "/mnt/../etc", "/mnt/./a/../.."] {
            assert!(validate_share_path(path).is_err(), "{path:?} must fail");
        }
    }

    #[test]
    fn test_validate_share_path_rejects_exports_and_fstab_column_injection() {
        // A space splits extra columns in /etc/exports and /etc/fstab; `)`,
        // `,` and `#` terminate/fabricate export options and comments.
        for path in [
            "/mnt/po ol/media",
            "/mnt/media *(rw)",
            "/mnt/media #comment",
            "/mnt/media;rm -rf /",
            "/mnt/media\n/other *(rw)",
            "/mnt/media\u{0009}x",
        ] {
            assert!(
                validate_share_path(path).is_err(),
                "{path:?} must be rejected"
            );
        }
    }

    #[test]
    fn test_validate_nfs_options_accepts_normal_options() {
        for options in [
            vec![],
            vec!["rw".to_string()],
            vec![
                "rw".to_string(),
                "sync".to_string(),
                "no_subtree_check".to_string(),
                "root_squash".to_string(),
            ],
            vec!["anonuid=1000".to_string(), "fsid=0".to_string()],
        ] {
            assert!(
                validate_nfs_options(&options).is_ok(),
                "{options:?} must be valid"
            );
        }
    }

    #[test]
    fn test_validate_nfs_options_rejects_client_list_forging() {
        // `path *({opts})` — a `)` in an option closes the client list and a
        // space starts a new one, fabricating extra exports.
        for options in [
            vec!["rw) /etc *(no_root_squash".to_string()],
            vec!["rw)".to_string()],
            vec!["a b".to_string()],
            vec!["rw,sync".to_string()],
            vec!["rw\nno_root_squash".to_string()],
            vec!["".to_string()],
            vec![format!("{}\n#", "x".repeat(5))],
        ] {
            assert!(
                validate_nfs_options(&options).is_err(),
                "{options:?} must be rejected"
            );
        }
        let too_many: Vec<String> = (0..17).map(|i| format!("opt{i}")).collect();
        assert!(validate_nfs_options(&too_many).is_err());
    }

    #[test]
    fn test_smb_share_section_is_one_field_per_line() {
        let section = smb_share_section("media", "Family media", "/mnt/pool/media", &SmbShareAccess::admin_only());
        assert!(section.contains("\n[media]\n"), "{section:?}");
        assert!(
            section.contains("  comment = Family media\n"),
            "{section:?}"
        );
        assert!(
            section.contains("  path = /mnt/pool/media\n"),
            "{section:?}"
        );
        assert_eq!(
            section.lines().filter(|l| l.contains("path =")).count(),
            1,
            "exactly one path directive: {section:?}"
        );
    }

    // -----------------------------------------------------------------
    // Per-share recycle bin
    // -----------------------------------------------------------------

    #[test]
    fn test_smb_recycle_block_targets_the_explorer_trash_folder() {
        let block = smb_recycle_block();
        assert!(block.starts_with(SMB_VFS_BEGIN), "{block:?}");
        assert!(block.ends_with(SMB_VFS_END), "{block:?}");
        assert!(block.contains("vfs objects = recycle"), "{block:?}");
        assert!(
            block.contains(&format!("recycle:repository = {RECYCLE_DIR_NAME}")),
            "the repository must be the folder the Explorer Trash reads: {block:?}"
        );
        assert!(section_has_recycle(&block));
    }

    #[test]
    fn test_apply_recycle_enable_is_idempotent() {
        let base = smb_share_section("media", "Family media", "/mnt/pool/media", &SmbShareAccess::admin_only());
        let once = apply_recycle_to_section(&base, true);
        let twice = apply_recycle_to_section(&once, true);
        assert_eq!(once, twice, "enabling twice must not stack blocks");
        assert_eq!(once.matches(SMB_VFS_BEGIN).count(), 1, "{once:?}");
        assert!(section_has_recycle(&once));
    }

    #[test]
    fn test_apply_recycle_disable_removes_the_block_and_keeps_the_section() {
        let base = smb_share_section("media", "Family media", "/mnt/pool/media", &SmbShareAccess::admin_only());
        let enabled = apply_recycle_to_section(&base, true);
        let disabled = apply_recycle_to_section(&enabled, false);
        assert!(!section_has_recycle(&disabled));
        assert!(!disabled.contains(SMB_VFS_BEGIN), "{disabled:?}");
        // Every original directive survives the round-trip.
        assert!(disabled.contains("\n[media]\n"), "{disabled:?}");
        assert!(
            disabled.contains("  path = /mnt/pool/media"),
            "{disabled:?}"
        );
        // Disabling a share that never had a block is a no-op.
        assert_eq!(apply_recycle_to_section(&base, false), base);
    }

    #[test]
    fn test_apply_recycle_preserves_hand_added_lines() {
        let base = format!(
            "{}{}",
            smb_share_section("media", "Family media", "/mnt/pool/media", &SmbShareAccess::admin_only()),
            "  veto files = /lost+found/\n"
        );
        let enabled = apply_recycle_to_section(&base, true);
        assert!(enabled.contains("veto files = /lost+found/"), "{enabled:?}");
        let disabled = apply_recycle_to_section(&enabled, false);
        assert!(
            disabled.contains("veto files = /lost+found/"),
            "{disabled:?}"
        );
        assert!(!disabled.contains(SMB_VFS_BEGIN), "{disabled:?}");
    }

    // -----------------------------------------------------------------
    // Per-share Time Machine (vfs_fruit)
    // -----------------------------------------------------------------

    #[test]
    fn test_time_machine_block_enables_fruit_and_its_recipe() {
        let block = smb_vfs_block(
            SmbVfsFlags {
                recycle: false,
                time_machine: true,
                previous_versions: false,
                ..SmbVfsFlags::default()
            },
            500,
        );
        assert!(
            block.contains("vfs objects = fruit streams_xattr"),
            "{block:?}"
        );
        assert!(block.contains("fruit:time machine = yes"), "{block:?}");
        assert!(
            block.contains("fruit:time machine max size = 500G"),
            "the quota is part of the profile: {block:?}"
        );
        assert!(block.contains("fruit:metadata = stream"), "{block:?}");
        assert!(!block.contains("recycle:"), "{block:?}");
        let flags = section_vfs_flags(&block);
        assert!(flags.time_machine);
        assert!(!flags.recycle);
        assert_eq!(section_time_machine_quota(&block), 500);
    }

    #[test]
    fn test_time_machine_quota_zero_is_unlimited() {
        let block = smb_vfs_block(
            SmbVfsFlags {
                recycle: false,
                time_machine: true,
                previous_versions: false,
                ..SmbVfsFlags::default()
            },
            0,
        );
        assert!(
            block.contains("fruit:time machine max size = 0\n"),
            "{block:?}"
        );
        assert_eq!(section_time_machine_quota(&block), 0);
    }

    #[test]
    fn test_recycle_and_time_machine_share_one_vfs_objects_line() {
        let both = smb_vfs_block(
            SmbVfsFlags {
                recycle: true,
                time_machine: true,
                previous_versions: false,
                ..SmbVfsFlags::default()
            },
            0,
        );
        // Samba accepts only one `vfs objects` line per share: fruit must come
        // first, recycle last, and there must be exactly one line.
        assert_eq!(
            both.lines()
                .filter(|l| l.trim().starts_with("vfs objects ="))
                .count(),
            1,
            "{both:?}"
        );
        assert!(
            both.contains("vfs objects = fruit streams_xattr recycle"),
            "{both:?}"
        );
        assert!(both.contains("recycle:repository ="), "{both:?}");
        let flags = section_vfs_flags(&both);
        assert!(flags.time_machine && flags.recycle);
    }

    #[test]
    fn test_toggling_one_feature_keeps_the_other() {
        let base = smb_share_section("tm", "Mac backups", "/mnt/pool/tm", &SmbShareAccess::admin_only());
        let tm = apply_vfs_to_section(
            &base,
            SmbVfsFlags {
                recycle: false,
                time_machine: true,
                previous_versions: false,
                ..SmbVfsFlags::default()
            },
            120,
        );
        // Turning the recycle bin on must not drop the Time Machine profile.
        let both = apply_recycle_to_section(&tm, true);
        assert!(section_vfs_flags(&both).recycle);
        assert!(section_vfs_flags(&both).time_machine);
        assert_eq!(section_time_machine_quota(&both), 120);
        // …and turning recycle off keeps Time Machine and its quota.
        let tm_only = apply_recycle_to_section(&both, false);
        assert!(!section_vfs_flags(&tm_only).recycle);
        assert!(section_vfs_flags(&tm_only).time_machine);
        assert_eq!(section_time_machine_quota(&tm_only), 120);
        assert!(tm_only.contains("fruit:time machine max size = 120G"));
    }

    #[test]
    fn test_a_legacy_recycle_block_is_upgraded_not_stacked() {
        let base = smb_share_section("media", "Family media", "/mnt/pool/media", &SmbShareAccess::admin_only());
        let legacy = format!(
            "{base}{SMB_LEGACY_BEGIN}\n  vfs objects = recycle\n  recycle:repository = {RECYCLE_DIR_NAME}\n{SMB_LEGACY_END}\n"
        );
        assert!(section_has_recycle(&legacy));
        let upgraded = apply_vfs_to_section(
            &legacy,
            SmbVfsFlags {
                recycle: true,
                time_machine: true,
                previous_versions: false,
                ..SmbVfsFlags::default()
            },
            0,
        );
        assert!(!upgraded.contains(SMB_LEGACY_BEGIN), "{upgraded:?}");
        assert_eq!(upgraded.matches(SMB_VFS_BEGIN).count(), 1, "{upgraded:?}");
        assert!(section_has_recycle(&upgraded));
        assert!(section_vfs_flags(&upgraded).time_machine);
    }

    #[test]
    fn test_disabling_both_features_removes_the_managed_block() {
        let base = smb_share_section("tm", "Mac backups", "/mnt/pool/tm", &SmbShareAccess::admin_only());
        let both = apply_vfs_to_section(
            &base,
            SmbVfsFlags {
                recycle: true,
                time_machine: true,
                previous_versions: false,
                ..SmbVfsFlags::default()
            },
            0,
        );
        let off = apply_vfs_to_section(&both, SmbVfsFlags::default(), 0);
        assert_eq!(off, base);
        assert!(!off.contains(SMB_VFS_BEGIN), "{off:?}");
    }

    #[test]
    fn test_time_machine_quota_validation_bounds() {
        assert!(validate_time_machine_quota(0).is_ok());
        assert!(validate_time_machine_quota(1).is_ok());
        assert!(validate_time_machine_quota(MAX_TIME_MACHINE_QUOTA_GB).is_ok());
        assert!(validate_time_machine_quota(MAX_TIME_MACHINE_QUOTA_GB + 1).is_err());
    }

    #[test]
    fn test_section_parsers_ignore_a_quota_without_the_fruit_module() {
        // A hand-written quota alone does not make a share a Time Machine
        // target: the module and the explicit switch are what count.
        let section = format!(
            "{}\n  fruit:time machine max size = 100G\n",
            smb_share_section("x", "x", "/mnt/x", &SmbShareAccess::admin_only())
        );
        assert!(!section_vfs_flags(&section).time_machine);
        assert_eq!(section_time_machine_quota(&section), 100);
    }

    #[test]
    fn test_nfs_export_line_defaults_and_round_trip() {
        let config = NfsConfig::default();
        assert_eq!(
            nfs_export_line("/mnt/pool/media", &[], &config, &[]),
            "/mnt/pool/media *(rw,sync,no_subtree_check,root_squash)\n"
        );

        let line = "/mnt/pool/media *(rw,sync,no_subtree_check,root_squash,vers=4)";
        let parsed = parse_nfs_export_line(line).unwrap();
        assert_eq!(parsed.name, "media");
        assert_eq!(parsed.path, "/mnt/pool/media");
        assert!(parsed.enabled);
        assert_eq!(
            parsed.options,
            vec!["rw", "sync", "no_subtree_check", "root_squash", "vers=4"]
        );
        // Round-trip: re-serialising the parsed options reproduces the line.
        assert_eq!(
            nfs_export_line(&parsed.path, &parsed.options, &config, &[]),
            format!("{line}\n")
        );
    }

    #[test]
    fn test_nfs_export_line_applies_networks_and_share_overrides() {
        let config = NfsConfig {
            networks: vec!["192.168.1.0/24".to_string(), "10.0.0.0/8".to_string()],
            default_access: "ro".to_string(),
            version: "3".to_string(),
            root_squash: "all_squash".to_string(),
            security_flavor: "sys".to_string(),
            custom_options: vec!["no_wdelay".to_string()],
        };
        let overrides = vec![
            "rw".to_string(),
            "async".to_string(),
            "no_root_squash".to_string(),
            "vers=4.2".to_string(),
        ];
        let opts = "rw,async,no_subtree_check,no_root_squash,vers=4.2,no_wdelay";
        let expected = format!("/mnt/pool/media 192.168.1.0/24({opts}) 10.0.0.0/8({opts})\n");
        assert_eq!(
            nfs_export_line("/mnt/pool/media", &overrides, &config, &[]),
            expected
        );

        let parsed = parse_nfs_export_line(expected.trim_end()).unwrap();
        let expected_options: Vec<String> = opts.split(',').map(str::to_string).collect();
        assert_eq!(parsed.options, expected_options);
        assert_eq!(
            nfs_export_line(&parsed.path, &parsed.options, &config, &[]),
            expected,
            "the multi-client line must round-trip"
        );
    }

    #[test]
    fn test_nfs_kerberos_security_is_explicit_and_requires_v4() {
        let mut config = NfsConfig {
            version: "4.2".to_string(),
            security_flavor: "krb5p".to_string(),
            ..NfsConfig::default()
        };
        assert!(validate_nfs_config(&config).is_ok());
        let line = nfs_export_line("/mnt/pool/media", &[], &config, &[]);
        assert!(line.contains("vers=4.2"));
        assert!(line.contains("sec=krb5p"));

        config.version = "auto".to_string();
        assert!(validate_nfs_config(&config).is_err());
        config.version = "4.2".to_string();
        config.security_flavor = "none".to_string();
        assert!(validate_nfs_config(&config).is_err());
        config.security_flavor = "sys".to_string();
        let line = nfs_export_line("/mnt/pool/media", &[], &config, &[]);
        assert!(!line.contains("sec="));
        assert!(!nfs_kerberos_available_at(&["/nonexistent/keytab"]));
    }

    #[test]
    fn test_nfs_version_auto_omits_vers() {
        let config = NfsConfig {
            version: "auto".to_string(),
            ..NfsConfig::default()
        };
        let line = nfs_export_line("/mnt/pool/media", &[], &config, &[]);
        assert_eq!(
            line,
            "/mnt/pool/media *(rw,sync,no_subtree_check,root_squash)\n"
        );
    }

    #[test]
    fn test_validate_nfs_config_rejects_injection_and_bad_values() {
        let with = |f: fn(&mut NfsConfig)| {
            let mut cfg = NfsConfig::default();
            f(&mut cfg);
            cfg
        };

        assert!(validate_nfs_config(&NfsConfig::default()).is_ok());
        assert!(validate_nfs_config(&with(|c| c.networks = vec![])).is_err());
        assert!(
            validate_nfs_config(&with(|c| { c.networks = vec!["* (rw)".to_string()] })).is_err()
        );
        assert!(validate_nfs_config(&with(|c| c.networks = vec!["a\nb".to_string()])).is_err());
        assert!(validate_nfs_config(&with(|c| c.default_access = "rw) /etc".to_string())).is_err());
        assert!(validate_nfs_config(&with(|c| c.version = "5".to_string())).is_err());
        assert!(validate_nfs_config(&with(|c| c.root_squash = "nope".to_string())).is_err());
        assert!(validate_nfs_config(&with(|c| {
            c.custom_options = vec!["rw,sync".to_string()]
        }))
        .is_err());
    }

    #[test]
    fn test_parse_nfs_export_line_skips_blanks_and_plain_comments() {
        for line in [
            "",
            "   ",
            "# a comment",
            "#homevault-nfs-disabled-without-space",
        ] {
            assert!(parse_nfs_export_line(line).is_none(), "{line:?}");
        }
    }

    #[test]
    fn test_nfs_disabled_tag_round_trips_as_a_disabled_share() {
        let config = NfsConfig::default();
        let plain = nfs_export_line("/mnt/pool/media", &["rw".to_string()], &config, &[]);
        let tagged = nfs_disabled_line("media", &plain);
        assert!(tagged.starts_with(NFS_DISABLED_TAG), "{tagged:?}");

        let parsed = parse_nfs_export_line(&tagged).unwrap();
        assert_eq!(parsed.name, "media");
        assert_eq!(parsed.path, "/mnt/pool/media");
        assert!(!parsed.enabled, "a tagged line is a disabled share");
        assert_eq!(
            parsed.options,
            vec!["rw", "sync", "no_subtree_check", "root_squash"]
        );
    }

    #[test]
    fn test_validate_smb_config_accepts_the_default() {
        assert!(validate_smb_config(&SmbConfig::default()).is_ok());
    }

    #[test]
    fn test_validate_smb_config_rejects_multiline_and_bad_values() {
        let with = |f: fn(&mut SmbConfig)| {
            let mut cfg = SmbConfig::default();
            f(&mut cfg);
            cfg
        };

        assert!(validate_smb_config(&with(|c| c.workgroup = "WORK\nGROUP".to_string())).is_err());
        assert!(
            validate_smb_config(&with(|c| c.security_mode = "user\n[evil]".to_string())).is_err()
        );
        assert!(validate_smb_config(&with(|c| c.security_mode = "bogus".to_string())).is_err());
        assert!(
            validate_smb_config(&with(|c| c.protocol_version = "SMB3\n#".to_string())).is_err()
        );
        assert!(validate_smb_config(&with(|c| c.create_mask = "9999".to_string())).is_err());
        assert!(validate_smb_config(&with(|c| c.directory_mask = "0o755".to_string())).is_err());
        assert!(validate_smb_config(&with(|c| c.workgroup = "GRP]".to_string())).is_err());
        assert!(validate_smb_config(&with(|c| c.smb_encrypt = "sometimes".to_string())).is_err());
        assert!(validate_smb_config(&with(|c| c.server_signing = "maybe".to_string())).is_err());
        assert!(validate_smb_config(&with(|c| c.aio_read_size = 16 * 1024 * 1024 + 1)).is_err());
        assert!(validate_smb_config(&with(|c| c.aio_write_size = 16 * 1024 * 1024 + 1)).is_err());
        assert!(validate_smb_config(&with(|c| {
            c.additional_params = vec!["\n[evil]".to_string()]
        }))
        .is_err());
    }

    #[test]
    fn test_validate_smb_config_requires_a_realm_for_ads_and_rejects_server() {
        // P2-1: `ads`/`domain` are configurable, but only with the realm and
        // idmap Samba actually needs. A bare mode is still a 400 rather than a
        // silently broken daemon; `server` remains unimplemented.
        for mode in ["domain", "ads"] {
            let mut config = SmbConfig::default();
            config.security_mode = mode.to_string();
            let error = validate_smb_config(&config).expect_err("a realm is required");
            assert!(error.contains("realm is required"), "{error}");

            config.realm = "EXAMPLE.LAN".to_string();
            assert!(validate_smb_config(&config).is_ok(), "{mode} with a realm");
            let rendered = smb_global_params(&config).join("\n");
            assert!(rendered.contains(&format!("security = {mode}")), "{rendered}");
            assert!(rendered.contains("realm = EXAMPLE.LAN"), "{rendered}");
            assert!(
                rendered.contains(&format!("idmap config * : range = {}", config.idmap_range)),
                "{rendered}"
            );
            assert!(rendered.contains("vfs objects = acl_xattr"), "{rendered}");
            assert!(
                rendered.contains("winbind use default domain = yes"),
                "{rendered}"
            );

            for bad in ["EXAMPLE", "EX AMPLE.LAN", "EXAMPLE/LAN", "[EXAMPLE.LAN]"] {
                config.realm = bad.to_string();
                assert!(
                    validate_smb_config(&config).is_err(),
                    "{mode} must refuse realm {bad:?}"
                );
            }
            config.realm = "EXAMPLE.LAN".to_string();
            for bad in ["0-100", "100-100", "abc-def", "100"] {
                config.idmap_range = bad.to_string();
                assert!(
                    validate_smb_config(&config).is_err(),
                    "{mode} must refuse idmap {bad:?}"
                );
            }
        }

        let mut config = SmbConfig::default();
        config.security_mode = "server".to_string();
        let error = validate_smb_config(&config).expect_err("server mode is not implemented");
        assert!(error.contains("not implemented"), "{error}");

        for version in ["NT1", "SMB1", "CIFS"] {
            let mut config = SmbConfig::default();
            config.protocol_version = version.to_string();
            assert!(
                validate_smb_config(&config).is_err(),
                "{version} must be refused"
            );
        }
        let mut config = SmbConfig::default();
        config.protocol_version = "SMB2".to_string();
        assert!(validate_smb_config(&config).is_ok());
    }

    #[test]
    fn test_domain_join_argv_never_contains_the_password() {
        let argv = domain_join_argv("Administrator").unwrap();
        assert_eq!(
            argv,
            vec!["sudo", "net", "ads", "join", "-U", "Administrator"]
        );
        assert!(!argv.iter().any(|arg| arg.contains("secret")));
        assert!(domain_join_argv("bad\nname").is_err());
        assert!(domain_leave_argv("Administrator")
            .unwrap()
            .contains(&"leave".to_string()));
        assert!(!winbind_available_at(&["/nonexistent/winbindd"]));
    }

    #[test]
    fn test_smb_extra_params_are_allowlisted() {
        assert!(smb_extra_param("deadtime = 30").is_ok());
        assert!(smb_extra_param("server multi channel support = yes").is_ok());
        assert!(smb_extra_param("include = /etc/passwd").is_err());
        assert!(smb_extra_param("server max protocol = SMB1").is_err());
        assert!(smb_extra_param("deadtime").is_err());
        assert!(smb_extra_param("deadtime = ").is_err());
        assert!(smb_extra_param("deadtime = 30\n[evil]").is_err());
    }

    #[test]
    fn test_smb_global_params_renders_every_field() {
        let config = SmbConfig {
            workgroup: "HV".to_string(),
            security_mode: "user".to_string(),
            protocol_version: "SMB2".to_string(),
            allow_guest: true,
            create_mask: "0770".to_string(),
            directory_mask: "0700".to_string(),
            additional_params: vec![
                "server multi channel support = no".to_string(),
                "deadtime = 30".to_string(),
            ],
            multichannel: true,
            realm: String::new(),
            idmap_range: "10000-999999".to_string(),
            ws_discovery: true,
            durable_handles: true,
            smb_encrypt: "required".to_string(),
            server_signing: "mandatory".to_string(),
            strict_sync: true,
            aio_read_size: 1,
            aio_write_size: 1,
            use_sendfile: false,
            oplocks: false,
            kernel_oplocks: false,
            level2_oplocks: false,
        };
        let rendered = smb_global_params(&config).join("\n");
        for expected in [
            "workgroup = HV",
            "security = user",
            "server min protocol = SMB2",
            "server max protocol = SMB3",
            "server multi channel support = yes",
            "server smb encrypt = required",
            "server signing = mandatory",
            "durable handles = yes",
            "strict sync = yes",
            "use sendfile = no",
            "oplocks = no",
            "kernel oplocks = no",
            "level2 oplocks = no",
            "aio read size = 1",
            "aio write size = 1",
            "guest ok = yes",
            "create mask = 0770",
            "directory mask = 0700",
            "deadtime = 30",
        ] {
            assert!(
                rendered.contains(expected),
                "missing '{expected}' in:\n{rendered}"
            );
        }
        // 16 managed parameters + 2 AIO + 1 allowed extra (the duplicate
        // multichannel passed as an extra loses against the dedicated toggle).
        assert_eq!(rendered.lines().count(), 19);
        assert!(!rendered.contains("server multi channel support = no"));
    }

    #[test]
    fn test_smb_config_deserializes_without_multichannel() {
        // A pre-2026-11-05 /etc/homevault/smb.json has no `multichannel` key;
        // `#[serde(default)]` must keep it loadable (off).
        let json = r#"{
            "workgroup": "WORKGROUP",
            "security_mode": "user",
            "protocol_version": "SMB3",
            "allow_guest": false,
            "create_mask": "0755",
            "directory_mask": "0755",
            "additional_params": []
        }"#;
        let config: SmbConfig = serde_json::from_str(json).expect("old config must still load");
        assert!(!config.multichannel);
        // The reliability fields did not exist in that file; their defaults
        // must be Samba-compatible (off for encryption/signing/strict sync,
        // on for the performance switches).
        assert!(!config.durable_handles);
        assert_eq!(config.smb_encrypt, "off");
        assert_eq!(config.server_signing, "off");
        assert!(!config.strict_sync);
        assert_eq!(config.aio_read_size, 0);
        assert_eq!(config.aio_write_size, 0);
        assert!(config.use_sendfile);
        assert!(config.oplocks);
        assert!(config.kernel_oplocks);
        assert!(config.level2_oplocks);
    }

    #[test]
    fn test_smb_config_ws_discovery_defaults_on_for_a_legacy_file() {
        // A pre-2026-11-08 smb.json has no `ws_discovery` key. The default is
        // on (DSM's Windows network discovery); the daemon is only started
        // when wsdd2 is installed, and the Shares card reports the real state.
        let json = r#"{
            "workgroup": "WORKGROUP",
            "security_mode": "user",
            "protocol_version": "SMB3",
            "allow_guest": false,
            "create_mask": "0755",
            "directory_mask": "0755",
            "additional_params": []
        }"#;
        let config: SmbConfig = serde_json::from_str(json).expect("old config must still load");
        assert!(config.ws_discovery);
    }

    #[test]
    fn test_get_smb_config_normalizes_legacy_values() {
        let mut config = SmbConfig {
            security_mode: "ads".to_string(),
            protocol_version: "NT1".to_string(),
            smb_encrypt: "sometimes".to_string(),
            server_signing: "maybe".to_string(),
            aio_read_size: 32 * 1024 * 1024,
            aio_write_size: 32 * 1024 * 1024,
            ..SmbConfig::default()
        };
        normalize_smb_config(&mut config);
        assert_eq!(config.security_mode, "user");
        assert_eq!(config.protocol_version, "SMB3");
        assert_eq!(config.smb_encrypt, "off");
        assert_eq!(config.server_signing, "off");
        assert_eq!(config.aio_read_size, 0);
        assert_eq!(config.aio_write_size, 0);

        let mut valid = SmbConfig::default();
        normalize_smb_config(&mut valid);
        assert_eq!(valid.security_mode, "user");
        assert_eq!(valid.protocol_version, "SMB3");
    }

    #[test]
    fn test_ftp_config_text_renders_every_field() {
        let config = FtpConfig {
            port: 2121,
            allowed_users: vec!["alice".to_string(), "bob".to_string()],
            passive_ports: "40000-40100".to_string(),
            max_clients: 25,
            force_tls: true,
            allow_anonymous: true,
            anonymous_root: Some("/srv/public".to_string()),
            chroot: false,
            welcome_message: "Hello".to_string(),
        };
        let text = ftp_config_text(&config);
        for expected in [
            "listen_port=2121",
            "pasv_min_port=40000",
            "pasv_max_port=40100",
            "max_clients=25",
            "anonymous_enable=YES",
            "anon_root=/srv/public",
            "chroot_local_user=YES",
            "userlist_enable=YES",
            "userlist_deny=NO",
            "userlist_file=/etc/vsftpd.user_list",
            "ftpd_banner=Hello",
            "ssl_enable=YES",
        ] {
            assert!(text.contains(expected), "missing '{expected}' in:\n{text}");
        }
    }

    #[test]
    fn test_ftp_user_list_and_validation() {
        assert_eq!(ftp_user_list_text(&[]), "");
        assert_eq!(
            ftp_user_list_text(&["alice".to_string(), "bob".to_string()]),
            "alice\nbob\n"
        );
        assert!(validate_ftp_user("alice").is_ok());
        assert!(validate_ftp_user("alice smith").is_err());
        assert!(validate_ftp_user("a/b").is_err());
        assert!(validate_ftp_user("").is_err());

        let mut config = FtpConfig {
            port: 21,
            passive_ports: "30000-31000".to_string(),
            max_clients: 10,
            ..FtpConfig::default()
        };
        config.allowed_users = vec!["a b".to_string()];
        assert!(validate_ftp_config(&config).is_err());
    }

    #[test]
    fn test_smb_line_key_ignores_comments_and_sections() {
        assert_eq!(
            smb_line_key("  workgroup = X"),
            Some("workgroup".to_string())
        );
        assert_eq!(smb_line_key("[global]"), None);
        assert_eq!(smb_line_key("# security = ads"), None);
        assert_eq!(smb_line_key("; security = ads"), None);
        assert_eq!(smb_line_key(""), None);
        assert_eq!(smb_line_key("guest ok = yes"), Some("guest ok".to_string()));
    }

    fn ftp_config_fixture() -> FtpConfig {
        FtpConfig {
            port: 21,
            allowed_users: vec![],
            passive_ports: "30000-31000".to_string(),
            max_clients: 10,
            force_tls: false,
            allow_anonymous: false,
            anonymous_root: None,
            chroot: true,
            welcome_message: "Welcome to HomeVault FTP".to_string(),
        }
    }

    #[test]
    fn test_validate_ftp_config_accepts_a_normal_config() {
        assert!(validate_ftp_config(&ftp_config_fixture()).is_ok());

        let mut single = ftp_config_fixture();
        single.passive_ports = "30000".to_string();
        assert!(validate_ftp_config(&single).is_ok());
    }

    #[test]
    fn test_validate_ftp_config_rejects_banner_injection() {
        // `ftpd_banner=` is a single-line vsftpd.conf directive.
        let mut cfg = ftp_config_fixture();
        cfg.welcome_message = "hi\nlocal_enable=YES\nanonymous_enable=YES".to_string();
        assert!(validate_ftp_config(&cfg).is_err());

        let mut cfg = ftp_config_fixture();
        cfg.welcome_message = "hi\rwrite_enable=YES".to_string();
        assert!(validate_ftp_config(&cfg).is_err());
    }

    #[test]
    fn test_validate_ftp_config_rejects_bad_ports_and_client_counts() {
        let mut cfg = ftp_config_fixture();
        cfg.port = 0;
        assert!(validate_ftp_config(&cfg).is_err());

        let mut cfg = ftp_config_fixture();
        cfg.max_clients = 0;
        assert!(validate_ftp_config(&cfg).is_err());

        for passive in ["abc", "31000-30000", "30000-", "-31000", "0-100"] {
            let mut cfg = ftp_config_fixture();
            cfg.passive_ports = passive.to_string();
            assert!(
                validate_ftp_config(&cfg).is_err(),
                "{passive:?} must be rejected"
            );
        }
    }

    // -- subprocess deadlines (ROADMAP X.2) -----------------------------

    /// The defect this guards against is invisible to a behavioural test: a
    /// bare command call still answers correctly on an idle box and only pins
    /// the caller when `systemctl`/`exportfs`/`sudo tee` stops answering.
    ///
    /// `concat!` keeps each pattern out of this file's own source, so the
    /// assertion cannot count itself.
    #[test]
    fn test_no_subprocess_of_this_module_runs_without_a_deadline() {
        let src = include_str!("shares.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for unbounded in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!(".", "status()"),
            concat!("wait_with_", "output()"),
        ] {
            assert!(
                !production.contains(unbounded),
                "`{unbounded}` waits without a deadline — use `run_command`/`run_timed_with_stdin` (ROADMAP X.2)"
            );
        }
    }

    #[test]
    fn test_run_command_kills_a_hung_tool_and_names_the_deadline() {
        let started = std::time::Instant::now();
        let error = run_command(Command::new("sleep").arg("30"), Duration::from_millis(120))
            .expect_err("a hung tool must not succeed");
        assert!(error.contains("did not finish within"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must be enforced, not waited out"
        );
    }

    #[test]
    fn test_run_command_keeps_a_non_zero_exit_as_data() {
        // `systemctl is-active` on a stopped unit exits non-zero and the
        // caller reads the status; a non-zero exit must arrive as output.
        let out = run_command(&mut Command::new("false"), Duration::from_secs(5))
            .expect("`false` still runs to completion");
        assert_eq!(out.status.code(), Some(1));
    }

    #[test]
    fn test_run_command_reports_a_missing_tool() {
        let error = run_command(
            &mut Command::new("homevault-no-such-binary"),
            Duration::from_secs(1),
        )
        .expect_err("a missing tool is an error");
        assert!(error.contains("Failed to run"), "{error}");
    }

    #[test]
    fn test_shares_deadlines_are_ceilings_and_ordered() {
        // A config write is short; a real service start can wait on its
        // dependencies, so it gets the longer (still finite) budget.
        assert!(SHARES_WRITE_TIMEOUT >= Duration::from_secs(1));
        assert!(SHARES_SERVICE_TIMEOUT >= SHARES_WRITE_TIMEOUT);
        assert!(SHARES_SERVICE_TIMEOUT <= Duration::from_secs(180));
    }

    fn permission(grantee_type: &str, grantee_name: &str) -> crate::storage::share_perm_repo::SharePermission {
        crate::storage::share_perm_repo::SharePermission {
            id: 0,
            share_name: "media".to_string(),
            share_type: "smb".to_string(),
            grantee_type: grantee_type.to_string(),
            grantee_name: grantee_name.to_string(),
            permission: "rw".to_string(),
            recursive: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_smb_valid_users_always_keeps_the_admin_group_and_names_groups() {
        let users = smb_valid_users(&[
            permission("user", "alice"),
            permission("group", "family"),
            permission("user", "alice"),
        ]);
        assert_eq!(
            users,
            vec![
                "@homevault_admin".to_string(),
                "@homevault_family".to_string(),
                "alice".to_string()
            ]
        );
    }

    #[test]
    fn test_write_config_file_replaces_atomically_and_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb.conf");
        let text = path.to_string_lossy().to_string();

        write_config_file(&text, "[global]\n   workgroup = ONE\n").unwrap();
        write_config_file(&text, "[global]\n   workgroup = TWO\n").unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[global]\n   workgroup = TWO\n"
        );
        assert_eq!(
            fs::read_to_string(format!("{text}.bak")).unwrap(),
            "[global]\n   workgroup = ONE\n"
        );

        let leftovers: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name != "smb.conf" && name != "smb.conf.bak")
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn test_smb_share_access_renders_valid_users_read_only_and_guest() {
        let access = SmbShareAccess {
            valid_users: vec!["alice".to_string(), "@homevault_admin".to_string()],
            read_only: true,
            guest_ok: Some(false),
        };
        let rendered = access.render().join("\n");
        assert!(rendered.contains("valid users = @homevault_admin alice"), "{rendered}");
        assert!(rendered.contains("read only = yes"), "{rendered}");
        assert!(rendered.contains("guest ok = no"), "{rendered}");
    }

    #[test]
    fn test_apply_smb_access_replaces_only_managed_lines() {
        let section = "[media]\n  path = /srv/media\n  read only = no\n  veto files = /secret/\n";
        let access = SmbShareAccess {
            valid_users: vec!["alice".to_string()],
            read_only: true,
            guest_ok: None,
        };
        let out = apply_smb_access_to_section(section, &access);
        assert!(out.contains("path = /srv/media"), "{out}");
        assert!(out.contains("veto files = /secret/"), "{out}");
        assert!(!out.contains("read only = no"), "{out}");
        assert!(out.contains("read only = yes"), "{out}");
    }

    #[test]
    fn test_nfs_client_rules_override_per_client_access() {
        let config = NfsConfig::default();
        let rules = vec![
            NfsClientRule {
                client: "192.168.1.0/24".to_string(),
                access: "ro".to_string(),
                options: vec!["sync".to_string()],
            },
            NfsClientRule {
                client: "10.0.0.0/8".to_string(),
                access: "rw".to_string(),
                options: vec![],
            },
        ];
        let line = nfs_export_line("/srv/media", &[], &config, &rules);
        assert!(line.contains("192.168.1.0/24(ro,"), "{line}");
        assert!(line.contains("10.0.0.0/8(rw,"), "{line}");
        assert!(!line.contains("*(rw"), "{line}");
    }

    #[test]
    fn test_validate_nfs_rules_rejects_duplicate_and_injection() {
        let duplicate = vec![
            NfsClientRule {
                client: "192.168.1.0/24".to_string(),
                access: "rw".to_string(),
                options: vec![],
            },
            NfsClientRule {
                client: "192.168.1.0/24".to_string(),
                access: "ro".to_string(),
                options: vec![],
            },
        ];
        assert!(validate_nfs_rules(&duplicate).is_err());

        let injection = vec![NfsClientRule {
            client: "192.168.1.0/24)".to_string(),
            access: "rw".to_string(),
            options: vec![],
        }];
        assert!(validate_nfs_rules(&injection).is_err());
    }

    #[test]
    fn test_smb_ads_renders_realm_idmap_and_acl_mapping() {
        let config = SmbConfig {
            security_mode: "ads".to_string(),
            realm: "EXAMPLE.LAN".to_string(),
            idmap_range: "10000-999999".to_string(),
            ..SmbConfig::default()
        };
        validate_smb_config(&config).expect("a complete ADS config is valid");
        let rendered = smb_global_params(&config).join("\n");
        assert!(rendered.contains("security = ads"), "{rendered}");
        assert!(rendered.contains("realm = EXAMPLE.LAN"), "{rendered}");
        assert!(rendered.contains("idmap config * : range = 10000-999999"), "{rendered}");
        assert!(rendered.contains("map acl inherit = yes"), "{rendered}");
    }

    #[test]
    fn test_previous_versions_block_round_trips_and_shares_vfs_line() {
        let section = smb_share_section(
            "media",
            "media",
            "/srv/media",
            &SmbShareAccess::admin_only(),
        );
        let on = apply_previous_versions_to_section(&section, true);
        assert!(on.contains("shadow_copy2"), "{on}");
        assert!(section_vfs_flags(&on).previous_versions);
        let off = apply_previous_versions_to_section(&on, false);
        assert!(!section_vfs_flags(&off).previous_versions);
        assert!(!off.contains("shadow_copy2"), "{off}");
    }

    #[test]
    fn test_ftp_anonymous_root_is_rendered_only_when_enabled() {
        let mut config = ftp_config_fixture();
        config.allow_anonymous = true;
        config.anonymous_root = Some("/srv/public".to_string());
        let text = ftp_config_text(&config);
        assert!(text.contains("anon_root=/srv/public"), "{text}");
        config.allow_anonymous = false;
        let text = ftp_config_text(&config);
        assert!(!text.contains("anon_root="), "{text}");
    }

    #[test]
    fn test_nfs_kerberos_requires_an_explicit_v4_version() {
        let mut config = NfsConfig {
            version: "3".to_string(),
            security_flavor: "krb5".to_string(),
            ..NfsConfig::default()
        };
        assert!(validate_nfs_config(&config).is_err());
        config.version = "4.2".to_string();
        assert!(validate_nfs_config(&config).is_ok());
        let line = nfs_export_line("/srv/media", &[], &config, &[]);
        assert!(line.contains("sec=krb5"), "{line}");
    }

    #[test]
    fn test_append_config_file_preserves_existing_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exports");
        let text = path.to_string_lossy().to_string();

        write_config_file(&text, "/srv/data *(rw,sync)\n").unwrap();
        append_config_file(&text, "/srv/photos *(ro,sync)\n").unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "/srv/data *(rw,sync)\n/srv/photos *(ro,sync)\n"
        );
    }

    #[test]
    fn test_self_heal_restores_an_empty_smb_conf_from_its_backup() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb.conf");
        let text = path.to_string_lossy().to_string();
        std::env::set_var(SMB_CONF_ENV, &path);

        fs::write(&path, "").unwrap();
        fs::write(
            format!("{text}.bak"),
            "[global]\n   workgroup = RESTORED\n   security = user\n",
        )
        .unwrap();

        let healed = SharesService::self_heal_smb_config().unwrap();
        std::env::remove_var(SMB_CONF_ENV);

        assert!(healed);
        let restored = fs::read_to_string(&path).unwrap();
        assert!(restored.contains("workgroup = RESTORED"), "{restored}");
    }

    #[test]
    fn test_self_heal_leaves_a_valid_smb_conf_untouched() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("smb.conf");
        std::env::set_var(SMB_CONF_ENV, &path);
        let original = "[global]\n   workgroup = KEEP\n   security = user\n";
        fs::write(&path, original).unwrap();

        let healed = SharesService::self_heal_smb_config().unwrap();
        std::env::remove_var(SMB_CONF_ENV);

        assert!(!healed);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }
}
