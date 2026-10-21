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

use serde::{Deserialize, Serialize};
use serde_json::Error as JsonError;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;
use tracing::warn;

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
}

/// Fields that can be updated on an existing share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareUpdate {
    pub path: Option<String>,
    pub enabled: Option<bool>,
    pub comment: Option<String>,
    pub options: Option<Vec<String>>,
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
    pub protocol_version: String,
    pub allow_guest: bool,
    pub create_mask: String,
    pub directory_mask: String,
    pub additional_params: Vec<String>,
}

impl Default for SmbConfig {
    fn default() -> Self {
        Self {
            workgroup: "WORKGROUP".to_string(),
            security_mode: "user".to_string(),
            protocol_version: "SMB3".to_string(),
            allow_guest: false,
            create_mask: "0755".to_string(),
            directory_mask: "0755".to_string(),
            additional_params: Vec::new(),
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
    pub custom_options: Vec<String>,
}

impl Default for NfsConfig {
    fn default() -> Self {
        Self {
            networks: vec!["*".to_string()],
            default_access: "rw".to_string(),
            version: "4".to_string(),
            root_squash: "root_squash".to_string(),
            custom_options: Vec::new(),
        }
    }
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

fn get_smb_config_path() -> PathBuf {
    get_config_dir().join("smb.json")
}

fn get_nfs_config_path() -> PathBuf {
    get_config_dir().join("nfs.json")
}

fn get_ftp_config_path() -> PathBuf {
    get_config_dir().join("ftp.json")
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
    let mut child = Command::new("sudo")
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(content.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(())
}

/// Appends to a root-owned system config file (`/etc/samba/smb.conf`,
/// `/etc/exports`) via `sudo tee -a`, for call sites that only need to add a
/// new share section/export line rather than rewrite the whole file.
pub(crate) fn append_file_as_root(path: &str, content: &str) -> std::io::Result<()> {
    let mut child = Command::new("sudo")
        .args(["tee", "-a", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(content.as_bytes())?;
    let output = child.wait_with_output()?;
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
    let output = Command::new("sudo")
        .args(["mkdir", "-p", &dir.to_string_lossy()])
        .output()?;
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
/// `additional_params` is only persisted to `/etc/homevault/smb.json` today —
/// it is validated single-line anyway so that any future consumer of the
/// stored array can never emit multi-line values.
pub(crate) fn validate_smb_config(config: &SmbConfig) -> Result<(), String> {
    check_config_value("workgroup", &config.workgroup, 15, true)?;
    let security = config.security_mode.to_lowercase();
    match security.as_str() {
        "user" | "share" | "domain" | "server" | "ads" => {}
        other => {
            return Err(format!(
                "security_mode '{other}' is not valid (use user, share, domain, server or ads)"
            ))
        }
    }
    if !config
        .protocol_version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return Err("protocol_version may only contain letters, digits, '_' and '.'".to_string());
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
    }
    Ok(())
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
    Ok(())
}

/// Build the appended smb.conf section for a share (single line per field;
/// inputs are validated by the caller).
fn smb_share_section(name: &str, comment: &str, path: &str) -> String {
    format!(
        "\n[{name}]\n  comment = {comment}\n  path = {path}\n  browseable = yes\n  read only = no\n  create mask = 0755\n"
    )
}

/// Build the `/etc/exports` line for a share: `path *(options)`. An empty
/// options list keeps the historical safe default; the options token is
/// guarded by [`validate_nfs_options`].
fn nfs_export_line(path: &str, options: &[String]) -> String {
    let opts = if options.is_empty() {
        "rw,sync,no_subtree_check".to_string()
    } else {
        options.join(",")
    };
    format!("{path} *({opts})\n")
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

    // `*(rw,sync,no_subtree_check)` → ["rw", "sync", "no_subtree_check"], so
    // the parsed option list round-trips through `nfs_export_line` exactly.
    let mut options: Vec<String> = Vec::new();
    for client_token in &parts[1..] {
        let inner = client_token
            .strip_prefix('*')
            .unwrap_or(client_token)
            .trim_start_matches('(')
            .trim_end_matches(')');
        options.extend(
            inner
                .split(',')
                .filter(|opt| !opt.is_empty())
                .map(|opt| opt.to_string()),
        );
    }

    Some(Share {
        name,
        share_type: ShareType::Nfs,
        path: path.to_string(),
        enabled,
        comment: String::new(),
        options,
    })
}

/// Service for managing SMB, NFS, FTP, and WebDAV shares.
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
                let config_path = "/etc/samba/smb.conf";
                if !std::path::Path::new(config_path).exists() {
                    warn!("Samba config file not found: {}", config_path);
                    return Err(ShareError::ConfigNotFound(config_path.to_string()));
                }
                let config = std::fs::read_to_string(config_path)?;
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

                    write_file_as_root("/etc/samba/smb.conf", &new_config)?;
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

    /// Return the SMB service config, or a default if no config file exists.
    pub fn get_smb_config() -> Result<SmbConfig, ShareError> {
        let path = get_smb_config_path();
        if path.exists() {
            let data = fs::read_to_string(path)?;
            Ok(serde_json::from_str(&data)?)
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
        let smb_conf = "/etc/samba/smb.conf";
        if !std::path::Path::new(smb_conf).exists() {
            warn!("Samba config file not found: {}", smb_conf);
            let content = format!(
                "[global]\n  workgroup = {}\n  security = {}\n  server min protocol = {}\n  guest ok = {}\n  create mask = {}\n  directory mask = {}\n",
                config.workgroup, config.security_mode, config.protocol_version,
                if config.allow_guest { "yes" } else { "no" },
                config.create_mask, config.directory_mask
            );
            write_file_as_root(smb_conf, &content)?;
        } else {
            let mut lines: Vec<String> = fs::read_to_string(smb_conf)?
                .lines()
                .map(String::from)
                .collect();
            let mut in_global = false;
            let mut global_end = 0;

            for (i, line) in lines.clone().iter().enumerate() {
                let trimmed = line.trim();
                if trimmed.eq_ignore_ascii_case("[global]") {
                    in_global = true;
                    continue;
                }
                if in_global
                    && trimmed.starts_with('[')
                    && !trimmed.eq_ignore_ascii_case("[global]")
                {
                    global_end = i;
                    break;
                }
                if in_global {
                    global_end = i + 1;
                }
            }

            if in_global {
                let mut insert_after = global_end;
                // Remove existing global params we manage
                let managed_keys = [
                    "workgroup",
                    "security",
                    "server min protocol",
                    "server max protocol",
                    "guest ok",
                    "create mask",
                    "directory mask",
                ];
                for key in &managed_keys {
                    lines.retain(|l| {
                        let t = l.trim();
                        !t.starts_with(key) || t.starts_with('#') || t.starts_with(';')
                    });
                }
                // Find new global_end after removals
                let mut new_global_end: usize = 0;
                let mut found_global = false;
                for (i, line) in lines.iter().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.eq_ignore_ascii_case("[global]") {
                        found_global = true;
                        continue;
                    }
                    if found_global && trimmed.starts_with('[') {
                        new_global_end = i;
                        break;
                    }
                    if found_global {
                        new_global_end = i + 1;
                    }
                }
                insert_after = if found_global {
                    new_global_end
                } else {
                    insert_after
                };

                let new_params = [
                    format!("  workgroup = {}", config.workgroup),
                    format!("  security = {}", config.security_mode),
                    format!("  server min protocol = {}", config.protocol_version),
                    format!(
                        "  guest ok = {}",
                        if config.allow_guest { "yes" } else { "no" }
                    ),
                    format!("  create mask = {}", config.create_mask),
                    format!("  directory mask = {}", config.directory_mask),
                ];
                for (offset, param) in new_params.iter().enumerate() {
                    lines.insert(insert_after + offset, param.clone());
                }
            }
            write_file_as_root(smb_conf, &lines.join("\n"))?;
        }

        if let Err(e) = Command::new("sudo")
            .arg("systemctl")
            .args(["reload", "smbd"])
            .output()
        {
            warn!("Failed to reload smbd after applying SMB config: {}", e);
        }
        Ok(())
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

    /// Persist the NFS service config to disk.
    pub fn save_nfs_config(config: &NfsConfig) -> Result<(), ShareError> {
        let path = get_nfs_config_path();
        if let Some(parent) = path.parent() {
            mkdir_as_root(parent)?;
        }
        write_file_as_root(
            &path.to_string_lossy(),
            &serde_json::to_string_pretty(config)?,
        )?;
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
        let output = Command::new("sudo")
            .arg("systemctl")
            .args(["start", service_name])
            .output()
            .map_err(|e| {
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
        let output = Command::new("sudo")
            .arg("systemctl")
            .args(["stop", service_name])
            .output()
            .map_err(|e| {
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
        let output = Command::new("sudo")
            .arg("systemctl")
            .args(["restart", service_name])
            .output()
            .map_err(|e| {
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
        // Defence in depth: `ftpd_banner=` is a single-line directive, and the
        // banner is already validated single-line in `save_ftp_config`; strip
        // any line break anyway so no caller path can ever append directives.
        let banner = config
            .welcome_message
            .replace(['\r', '\n'], " ")
            .replace('\t', " ");
        let content = format!(
            r#"listen=YES
listen_port={port}
anonymous_enable={anon}
local_enable=YES
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
chroot_local_user={chroot}
allow_writeable_chroot=YES
ftpd_banner={banner}
"#,
            port = config.port,
            anon = if config.allow_anonymous { "YES" } else { "NO" },
            pasv_min = config.passive_ports.split('-').next().unwrap_or("30000"),
            pasv_max = config.passive_ports.split('-').nth(1).unwrap_or("31000"),
            max_clients = config.max_clients,
            chroot = if config.chroot { "YES" } else { "NO" },
            banner = banner,
        );

        if config.force_tls {
            let tls_conf = r#"ssl_enable=YES
allow_anon_ssl=NO
force_local_data_ssl=YES
force_local_logins_ssl=YES
ssl_tlsv1=YES
ssl_sslv2=NO
ssl_sslv3=NO
rsa_cert_file=/etc/ssl/certs/ssl-cert-snakeoil.pem
rsa_private_key_file=/etc/ssl/private/ssl-cert-snakeoil.key
"#
            .to_string();
            write_file_as_root(vsftpd_conf, &format!("{}\n{}", content, tls_conf))?;
        } else {
            write_file_as_root(vsftpd_conf, &content)?;
        }

        if let Err(e) = Command::new("sudo")
            .arg("systemctl")
            .args(["reload", "vsftpd"])
            .output()
        {
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
        let config_path = "/etc/samba/smb.conf";
        let share_section = smb_share_section(&share.name, &share.comment, &share.path);

        append_file_as_root(config_path, &share_section)?;

        if let Err(e) = Command::new("sudo")
            .arg("systemctl")
            .args(["reload", "smbd"])
            .output()
            .map(|o| o.status.success())
        {
            warn!("Failed to reload smbd: {}", e);
        }

        Ok(())
    }

    fn create_nfs_share(share: &Share) -> Result<(), ShareError> {
        let exports_path = "/etc/exports";
        let export_line = nfs_export_line(&share.path, &share.options);

        append_file_as_root(exports_path, &export_line)?;

        if let Err(e) = Command::new("sudo")
            .arg("exportfs")
            .args(["-ra"])
            .output()
            .map(|o| o.status.success())
        {
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
        });

        shares.insert(key, entry);
        write_file_as_root(
            &shares_path.to_string_lossy(),
            &serde_json::to_string_pretty(&shares)?,
        )?;
        Ok(())
    }

    fn parse_smb_config() -> Result<Vec<Share>, ShareError> {
        let config_path = "/etc/samba/smb.conf";
        if !std::path::Path::new(config_path).exists() {
            warn!("Samba config file not found: {}", config_path);
            return Ok(Vec::new());
        }

        let config = std::fs::read_to_string(config_path).map_err(|e| {
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
                    } else if !trimmed.is_empty() && !trimmed.starts_with(';') {
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
                });
            }
        }

        Ok(shares)
    }

    fn delete_smb_share(name: &str) -> Result<(), ShareError> {
        let config_path = "/etc/samba/smb.conf";
        if !std::path::Path::new(config_path).exists() {
            warn!("Samba config file not found: {}", config_path);
            return Err(ShareError::ConfigNotFound(config_path.to_string()));
        }
        let config = std::fs::read_to_string(config_path)?;
        let share_section = format!("[{}]", name);

        if let Some(start) = config.find(&share_section) {
            let after_section = &config[start..];
            let end = after_section.find("\n[").unwrap_or(after_section.len());

            let mut new_config = config[..start].to_string();
            new_config.push_str(&after_section[end..]);

            write_file_as_root("/etc/samba/smb.conf", &new_config)?;

            if let Err(e) = Command::new("sudo")
                .arg("systemctl")
                .args(["reload", "smbd"])
                .output()
                .map(|o| o.status.success())
            {
                warn!("Failed to reload smbd after deleting share {}: {}", name, e);
            }
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
        match Command::new("sudo").arg("exportfs").args(["-ra"]).output() {
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
        let config_path = "/etc/samba/smb.conf";
        if !std::path::Path::new(config_path).exists() {
            return Err(ShareError::ConfigNotFound(config_path.to_string()));
        }

        let config = std::fs::read_to_string(config_path)?;
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

            let mut new_config = config[..start].to_string();
            new_config.push_str(&new_content);
            new_config.push_str(&after_section[end..]);

            write_file_as_root("/etc/samba/smb.conf", &new_config)?;

            if let Err(e) = Command::new("sudo")
                .arg("systemctl")
                .args(["reload", "smbd"])
                .output()
                .map(|o| o.status.success())
            {
                warn!("Failed to reload smbd after updating share {}: {}", name, e);
            }
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
        let replacement = if existing.enabled {
            nfs_export_line(&new_path, &new_options)
        } else {
            nfs_disabled_line(name, &nfs_export_line(&new_path, &new_options))
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

        let mut new_exports = String::new();
        for (idx, line) in raw.lines().enumerate() {
            if idx == target {
                let parsed = parse_nfs_export_line(line)
                    .ok_or_else(|| ShareError::NotFound(name.to_string()))?;
                let plain = nfs_export_line(&parsed.path, &parsed.options);
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
        assert_eq!(d.version, "4");
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
        let section = smb_share_section("media", "Family media", "/mnt/pool/media");
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

    #[test]
    fn test_nfs_export_line_defaults_and_round_trip() {
        assert_eq!(
            nfs_export_line("/mnt/pool/media", &[]),
            "/mnt/pool/media *(rw,sync,no_subtree_check)\n"
        );

        let parsed = parse_nfs_export_line("/mnt/pool/media *(rw,sync,no_subtree_check)").unwrap();
        assert_eq!(parsed.name, "media");
        assert_eq!(parsed.path, "/mnt/pool/media");
        assert!(parsed.enabled);
        assert_eq!(parsed.options, vec!["rw", "sync", "no_subtree_check"]);
        // Round-trip: re-serialising the parsed options reproduces the line.
        assert_eq!(
            nfs_export_line(&parsed.path, &parsed.options),
            "/mnt/pool/media *(rw,sync,no_subtree_check)\n"
        );
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
        let plain = nfs_export_line("/mnt/pool/media", &["rw".to_string()]);
        let tagged = nfs_disabled_line("media", &plain);
        assert!(tagged.starts_with(NFS_DISABLED_TAG), "{tagged:?}");

        let parsed = parse_nfs_export_line(&tagged).unwrap();
        assert_eq!(parsed.name, "media");
        assert_eq!(parsed.path, "/mnt/pool/media");
        assert!(!parsed.enabled, "a tagged line is a disabled share");
        assert_eq!(parsed.options, vec!["rw"]);
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
        assert!(validate_smb_config(&with(|c| {
            c.additional_params = vec!["\n[evil]".to_string()]
        }))
        .is_err());
    }

    fn ftp_config_fixture() -> FtpConfig {
        FtpConfig {
            port: 21,
            allowed_users: vec![],
            passive_ports: "30000-31000".to_string(),
            max_clients: 10,
            force_tls: false,
            allow_anonymous: false,
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
}
