/* =============================================================================
 * File          : remote_mounts.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Remote file shares (SMB/CIFS and NFS) mounted under one directory the
 *   appliance owns, so their contents are browsable in the Explorer like any
 *   other folder — DSM's "remote folder / VFS mount", without a new daemon.
 *
 *   Everything that decides *whether* a mount is allowed is pure and
 *   unit-tested: the name becomes a directory, the server/source become one
 *   argv entry, the options are an allowlist, and the mount point is always
 *   `<REMOTE_ROOT>/<name>`. Only `mount`/`umount` touch the host, through
 *   `sudo` (the scoped sudoers file already grants mount and umount over
 *   every path under /mnt), with bounded deadlines.
 *
 *   Credentials never leave the appliance: a CIFS password is written to
 *   `data/remote-mounts/<name>.cred` (mode 0600) and passed to `mount.cifs`
 *   as `credentials=<path>`, never on the command line, never in a log line
 *   and never in an API response.
 *
 * Dependencies:
 *   - serde (>= 1.0), crate::utils::process, crate::config::paths
 *
 * Usage:
 *   handlers::remote_mounts exposes the endpoints; see
 *   docs/guides/remote-mounts.md.
 * =============================================================================
 */

//! Remote SMB/CIFS and NFS mounts for the Explorer (see
//! docs/guides/remote-mounts.md).
//!
//! Opt-in and off until an admin adds a network location: no mount exists, no
//! credentials file exists and nothing runs until the first one is created.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::utils::process::run_timed;

/// Every remote share is mounted under this fixed directory. The target is
/// `<REMOTE_ROOT>/<name>` where `name` is a validated slug, so a request can
/// never choose a mount point (`/`, `/etc`, another pool, …).
pub const REMOTE_ROOT: &str = "/mnt/homevault-remote";

/// Deadline for `mount`. A CIFS/NFS mount is a network round trip: a server
/// that stopped answering must not hold the blocking thread that serves the
/// request (or the boot reconcile) for minutes.
pub const MOUNT_TIMEOUT: Duration = Duration::from_secs(45);

/// Deadline for `umount`. A hung NFS server makes `umount` wait; the caller
/// gets the timeout message instead of a wedged worker.
pub const UMOUNT_TIMEOUT: Duration = Duration::from_secs(20);

/// Deadline for the capability probe (`which mount.cifs`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest accepted name (it becomes a directory name).
pub const MAX_NAME_LEN: usize = 32;
/// Longest accepted server name (a DNS name is at most 253 characters).
pub const MAX_SERVER_LEN: usize = 253;
/// Longest accepted CIFS share name / NFS export path.
pub const MAX_SOURCE_LEN: usize = 1024;
/// Most option entries one mount may carry.
pub const MAX_OPTIONS: usize = 16;

/// Which remote protocol a mount speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteKind {
    Cifs,
    Nfs,
}

impl RemoteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cifs => "cifs",
            Self::Nfs => "nfs",
        }
    }

    /// Accepts the spellings an admin (or an older UI) may send.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cifs" | "smb" | "smb2" | "smb3" => Some(Self::Cifs),
            "nfs" | "nfs3" | "nfs4" => Some(Self::Nfs),
            _ => None,
        }
    }

    /// `mount -t <fs>` value.
    pub fn filesystem(self) -> &'static str {
        match self {
            Self::Cifs => "cifs",
            Self::Nfs => "nfs",
        }
    }

    /// The userspace helper `mount -t <fs>` needs; missing means `503`.
    pub fn helper(self) -> &'static str {
        match self {
            Self::Cifs => "mount.cifs",
            Self::Nfs => "mount.nfs",
        }
    }
}

impl Default for RemoteKind {
    fn default() -> Self {
        Self::Cifs
    }
}

fn default_true() -> bool {
    true
}

/// One network location, as persisted in `data/remote_mounts.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMount {
    /// Slug, also the folder under [`REMOTE_ROOT`].
    pub name: String,
    pub kind: RemoteKind,
    /// Host name or IPv4 address (IPv6 literals are refused: `server:/export`
    /// and `//server/share` are ambiguous without brackets).
    pub server: String,
    /// CIFS share name (`photos`) or NFS export path (`/volume1/photos`).
    pub source: String,
    /// CIFS only.
    #[serde(default)]
    pub username: Option<String>,
    /// CIFS only (`domain=` / `WORKGROUP`).
    #[serde(default)]
    pub domain: Option<String>,
    /// Extra mount options, from the per-protocol allowlist.
    #[serde(default)]
    pub options: Vec<String>,
    /// Mounted read-only. On by default: a media/backup source is far more
    /// often read than written, and a mistake should not reach the other NAS.
    #[serde(default = "default_true")]
    pub read_only: bool,
    /// Mount again automatically after a restart.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: String,
    /// Last failure, so the panel can explain a mount that is not up.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// A mount plus its live state, as the API reports it. Never carries the
/// password — only whether one is stored.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteMountView {
    #[serde(flatten)]
    pub mount: RemoteMount,
    /// Absolute host path the share is (or would be) mounted at.
    pub target: String,
    pub mounted: bool,
    pub has_credentials: bool,
}

/// The persisted store (one JSON file, rewritten atomically).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteMountStore {
    #[serde(default)]
    pub mounts: Vec<RemoteMount>,
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

/// A name becomes a directory under [`REMOTE_ROOT`], so it is a strict slug:
/// lowercase letters, digits and inner dashes. Anything else is refused rather
/// than sanitised — two different names must never collapse to one folder.
pub fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("A name is required".to_string());
    }
    if name.len() > MAX_NAME_LEN {
        return Err(format!("The name is longer than {MAX_NAME_LEN} characters"));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(
            "Use lowercase letters, digits and dashes only (for example 'photos-nas')".to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("The name must not start or end with a dash".to_string());
    }
    Ok(name.to_string())
}

/// A host name or IPv4 address. The value is spliced into `//server/share`
/// and `server:/export`, so `/`, `\`, whitespace, `:` and a leading `-` are
/// all refused.
pub fn validate_server(raw: &str) -> Result<String, String> {
    let server = raw.trim();
    if server.is_empty() {
        return Err("The server name or address is required".to_string());
    }
    if server.len() > MAX_SERVER_LEN {
        return Err("The server name is too long".to_string());
    }
    if server.chars().any(char::is_control) || server.chars().any(char::is_whitespace) {
        return Err("The server must not contain spaces or control characters".to_string());
    }
    if server.contains('/')
        || server.contains('\\')
        || server.contains(':')
        || server.starts_with('-')
    {
        return Err(
            "Use a host name or an IPv4 address (IPv6 is not supported for remote mounts)"
                .to_string(),
        );
    }
    if server.parse::<std::net::Ipv4Addr>().is_ok() {
        return Ok(server.to_string());
    }
    crate::ddns::validate_hostname(server)
}

/// The CIFS share name or the NFS export path.
pub fn validate_source(kind: RemoteKind, raw: &str) -> Result<String, String> {
    let source = raw.trim();
    if source.is_empty() {
        return Err(match kind {
            RemoteKind::Cifs => "The share name is required".to_string(),
            RemoteKind::Nfs => "The export path is required".to_string(),
        });
    }
    if source.len() > MAX_SOURCE_LEN {
        return Err("The share or export name is too long".to_string());
    }
    if source.chars().any(char::is_control) || source.chars().any(char::is_whitespace) {
        return Err("The share or export name must not contain spaces".to_string());
    }
    match kind {
        RemoteKind::Cifs => {
            // A share, not a path inside it: `//server/share/sub` is not a
            // mount source (`mount.cifs` wants the share, then a `dir=` option).
            if source.contains('/') || source.contains('\\') || source.starts_with('-') {
                return Err(
                    "Use the share name only (for example 'photos'), not a path inside it"
                        .to_string(),
                );
            }
            if source == "." || source == ".." {
                return Err("That is not a share name".to_string());
            }
            Ok(source.to_string())
        }
        RemoteKind::Nfs => {
            if !source.starts_with('/') {
                return Err(
                    "The export must be an absolute path (for example '/volume1/media')"
                        .to_string(),
                );
            }
            if source.split('/').any(|part| part == "." || part == "..") {
                return Err("The export must not contain '.' or '..'".to_string());
            }
            if source.split('/').skip(1).any(|part| part.is_empty()) {
                return Err("The export must not contain an empty path component".to_string());
            }
            Ok(source.to_string())
        }
    }
}

/// Option keys allowed per protocol. `ro`/`rw` are **not** here: the read-only
/// choice is its own field, so an option list cannot silently override it.
/// Credential-bearing and privilege-changing keys are absent by construction,
/// not by a blocklist.
pub fn allowed_option_keys(kind: RemoteKind) -> &'static [&'static str] {
    match kind {
        RemoteKind::Cifs => &[
            "vers",
            "uid",
            "gid",
            "file_mode",
            "dir_mode",
            "iocharset",
            "nobrl",
            "noperm",
            "soft",
            "noatime",
        ],
        RemoteKind::Nfs => &[
            "vers", "soft", "hard", "timeo", "retrans", "rsize", "wsize", "nolock", "noatime",
        ],
    }
}

/// Normalise and validate `key=value` option strings.
///
/// A key outside the allowlist, a value with a `,` (it would split into a
/// second option) or a control character is a `400` naming the key.
pub fn validate_options(kind: RemoteKind, raw: &[String]) -> Result<Vec<String>, String> {
    if raw.len() > MAX_OPTIONS {
        return Err(format!("At most {MAX_OPTIONS} options are allowed"));
    }
    let allowed = allowed_option_keys(kind);
    let mut out = Vec::with_capacity(raw.len());
    for item in raw {
        let option = item.trim();
        if option.is_empty() {
            continue;
        }
        if option.chars().any(char::is_control) || option.chars().any(char::is_whitespace) {
            return Err(format!("Option '{option}' must not contain spaces"));
        }
        if option.contains(',') {
            return Err(format!("Option '{option}' must not contain a comma"));
        }
        // The key is what the allowlist checks; the value only has to be a
        // token that cannot start a second option or a negative number.
        match option.split_once('=') {
            Some((key, value)) => {
                if value.is_empty() || value.starts_with('-') {
                    return Err(format!("Option '{option}' has no value"));
                }
                if !value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':'))
                {
                    return Err(format!("Option '{option}' has an invalid value"));
                }
                if !allowed.contains(&key) {
                    return Err(format!(
                        "Option '{key}' is not supported. Allowed: {}",
                        allowed.join(", ")
                    ));
                }
            }
            None => {
                if !allowed.contains(&option) {
                    return Err(format!(
                        "Option '{option}' is not supported. Allowed: {}",
                        allowed.join(", ")
                    ));
                }
            }
        }
        if !out.iter().any(|existing| existing == option) {
            out.push(option.to_string());
        }
    }
    Ok(out)
}

/// Validate an optional CIFS account. Samba user names may contain most
/// printable characters, but the value ends up in a credentials **file**, so a
/// newline would inject a second key.
pub fn validate_account(field: &str, raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 256 {
        return Err(format!("The {field} is too long"));
    }
    if value.chars().any(|c| c.is_control()) || value.contains('=') {
        return Err(format!(
            "The {field} must not contain '=' or control characters"
        ));
    }
    Ok(Some(value.to_string()))
}

// ---------------------------------------------------------------------------
// Paths and credentials
// ---------------------------------------------------------------------------

/// `data/remote_mounts.json` — the persisted list (no secrets).
pub fn config_path() -> PathBuf {
    crate::config::paths::data_dir().join("remote_mounts.json")
}

/// Directory holding one credentials file per CIFS mount.
pub fn credentials_dir() -> PathBuf {
    crate::config::paths::data_dir().join("remote-mounts")
}

/// The credentials file of one mount. Only its existence is ever reported.
pub fn credential_path(name: &str) -> PathBuf {
    credentials_dir().join(format!("{name}.cred"))
}

/// The mount point of a validated name. Never derived from a request path.
pub fn target_for(name: &str) -> PathBuf {
    Path::new(REMOTE_ROOT).join(name)
}

/// The `mount.cifs`/`mount.nfs` source string.
pub fn source_string(mount: &RemoteMount) -> String {
    match mount.kind {
        RemoteKind::Cifs => format!("//{}/{}", mount.server, mount.source),
        RemoteKind::Nfs => format!("{}:{}", mount.server, mount.source),
    }
}

/// The `-o` value. Order matters only for readability; the read-only choice
/// comes first so it is obvious in a support log.
pub fn mount_options(mount: &RemoteMount, credential: Option<&Path>) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(if mount.read_only {
        "ro".into()
    } else {
        "rw".into()
    });
    if mount.kind == RemoteKind::Cifs {
        if let Some(cred) = credential {
            parts.push(format!("credentials={}", cred.to_string_lossy()));
        }
    }
    parts.extend(mount.options.iter().cloned());
    parts.join(",")
}

/// The full `sudo` argv. Pure, so a test asserts the exact shape without
/// running `mount` (and without a network).
pub fn mount_argv(mount: &RemoteMount, target: &Path, credential: Option<&Path>) -> Vec<String> {
    vec![
        "mount".to_string(),
        "-t".to_string(),
        mount.kind.filesystem().to_string(),
        "-o".to_string(),
        mount_options(mount, credential),
        source_string(mount),
        target.to_string_lossy().into_owned(),
    ]
}

/// The `sudo umount` argv for a target.
pub fn umount_argv(target: &Path) -> Vec<String> {
    vec!["umount".to_string(), target.to_string_lossy().into_owned()]
}

/// Credentials file body (`mount.cifs` key=value format). A password is never
/// logged; this function only ever writes into a 0600 file.
pub fn credentials_body(username: &str, password: Option<&str>, domain: Option<&str>) -> String {
    let mut body = format!("username={username}\n");
    if let Some(password) = password {
        body.push_str(&format!("password={password}\n"));
    }
    if let Some(domain) = domain {
        body.push_str(&format!("domain={domain}\n"));
    }
    body
}

// ---------------------------------------------------------------------------
// Mount table parsing (pure)
// ---------------------------------------------------------------------------

/// Undo the kernel's octal escapes in `/proc/mounts` (`\040` = space, `\011`
/// = tab, `\134` = backslash).
pub fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = String::with_capacity(field.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let octal = &field[i + 1..i + 4];
            if let Ok(value) = u8::from_str_radix(octal, 8) {
                out.push(value as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// True when `target` is a mount point in `mounts` (the content of
/// `/proc/mounts`), whatever filesystem is there.
pub fn is_mounted(mounts: &str, target: &Path) -> bool {
    let wanted = target.to_string_lossy();
    mounts.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _device = fields.next();
        let Some(point) = fields.next() else {
            return false;
        };
        unescape_mount_field(point) == wanted
    })
}

/// The filesystem type of a mount point (`cifs`, `nfs`, `nfs4`, …), when the
/// table has it. Used to report a mount that is up but was created by hand.
pub fn filesystem_at(mounts: &str, target: &Path) -> Option<String> {
    let wanted = target.to_string_lossy();
    mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let _device = fields.next()?;
        let point = fields.next()?;
        let fstype = fields.next()?;
        (unescape_mount_field(point) == wanted).then(|| fstype.to_string())
    })
}

// ---------------------------------------------------------------------------
// Host operations (blocking)
// ---------------------------------------------------------------------------

fn read_mounts() -> String {
    std::fs::read_to_string("/proc/mounts").unwrap_or_default()
}

/// Load the persisted list. A corrupt file is reported and treated as empty
/// rather than crashing the subsystem.
pub fn load() -> Vec<RemoteMount> {
    let raw = match std::fs::read_to_string(config_path()) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_str::<RemoteMountStore>(&raw) {
        Ok(store) => store.mounts,
        Err(e) => {
            tracing::warn!(
                "remote mounts: {} is not valid JSON: {e}",
                config_path().display()
            );
            Vec::new()
        }
    }
}

/// Persist the list atomically (tmp + rename) with mode 0600.
pub fn save(mounts: &[RemoteMount]) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_vec_pretty(&RemoteMountStore {
        mounts: mounts.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    write_private(&path, &body)
}

/// Write a secret-bearing file: 0600 from the first byte, never a window where
/// another account could read a half-written credentials file.
fn write_private(path: &Path, body: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    file.write_all(body)
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    file.sync_all().ok();
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Store or clear the CIFS credentials of one mount.
pub fn write_credentials(
    name: &str,
    username: Option<&str>,
    password: Option<&str>,
    domain: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let path = credential_path(name);
    match username {
        Some(username) => {
            write_private(
                &path,
                credentials_body(username, password, domain).as_bytes(),
            )?;
            Ok(Some(path))
        }
        None => {
            let _ = std::fs::remove_file(&path);
            Ok(None)
        }
    }
}

/// Remove a mount's credentials file, ignoring a missing one.
pub fn remove_credentials(name: &str) {
    let _ = std::fs::remove_file(credential_path(name));
}

/// Whether a credentials file exists for a mount.
pub fn has_credentials(name: &str) -> bool {
    credential_path(name).is_file()
}

/// One `sudo` argv under a deadline; a non-zero exit becomes the tool's stderr
/// so the caller can act on the real reason.
fn run_sudo(args: &[String], deadline: Duration) -> Result<String, String> {
    let Some((program, rest)) = args.split_first() else {
        return Err("empty command".to_string());
    };
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg(program).args(rest);
    let output = run_timed(&mut cmd, deadline).map_err(|e| {
        format!(
            "{} did not answer within {}s: {e}",
            program,
            deadline.as_secs()
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("{program} failed with {}", output.status)
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether a protocol's helper is installed.
pub fn kind_available(kind: RemoteKind) -> bool {
    run_timed(
        std::process::Command::new("which").arg(kind.helper()),
        PROBE_TIMEOUT,
    )
    .map(|out| out.status.success())
    .unwrap_or(false)
}

/// The protocols this host can mount, for the UI's capability contract.
pub fn available_kinds() -> Vec<&'static str> {
    [RemoteKind::Cifs, RemoteKind::Nfs]
        .into_iter()
        .filter(|kind| kind_available(*kind))
        .map(RemoteKind::as_str)
        .collect()
}

/// Bring one configured mount up. Idempotent: an already-mounted target is a
/// success, not an error (the boot reconcile and the button share this path).
pub fn mount_one(name: &str) -> Result<RemoteMountView, String> {
    let mut mounts = load();
    let index = mounts
        .iter()
        .position(|m| m.name == name)
        .ok_or_else(|| format!("No network location named '{name}'"))?;
    if !kind_available(mounts[index].kind) {
        return Err(format!(
            "{} is not installed on this server",
            mounts[index].kind.helper()
        ));
    }

    let target = target_for(name);
    let mounts_text = read_mounts();
    if !is_mounted(&mounts_text, &target) {
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("cannot create {}: {e}", target.display()))?;
        let credential = has_credentials(name).then(|| credential_path(name));
        let argv = mount_argv(&mounts[index], &target, credential.as_deref());
        let result = run_sudo(&argv, MOUNT_TIMEOUT);
        if let Err(e) = result {
            mounts[index].last_error = Some(e.clone());
            let _ = save(&mounts);
            return Err(e);
        }
    }

    mounts[index].last_error = None;
    let _ = save(&mounts);
    Ok(view_of(&mounts[index], &read_mounts()))
}

/// Take one mount down. A target that is not mounted is already "done".
pub fn unmount_one(name: &str) -> Result<(), String> {
    let mut mounts = load();
    let index = mounts
        .iter()
        .position(|m| m.name == name)
        .ok_or_else(|| format!("No network location named '{name}'"))?;
    if mount_in_progress(&target_for(name)) {
        return Err(format!(
            "'{name}' is still being mounted; try again in a moment"
        ));
    }

    let target = target_for(name);
    if is_mounted(&read_mounts(), &target) {
        run_sudo(&umount_argv(&target), UMOUNT_TIMEOUT)?;
    }
    mounts[index].last_error = None;
    let _ = save(&mounts);
    Ok(())
}

/// A `mount`/`umount` for this target is running (a second one would stack or
/// race). Same liveness check the pool code uses: a lock held by a live pid.
fn mount_in_progress(target: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        // `/proc/<pid>/comm` is `mount` or `umount`; the target appears in the
        // command line. Bounded scan: only pids with a live process dir.
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return false;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
            let comm = comm.trim();
            if comm != "mount" && comm != "umount" {
                continue;
            }
            let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
            let cmdline = String::from_utf8_lossy(&cmdline);
            if cmdline.contains(&target.to_string_lossy().into_owned()) && pid > 0 {
                return true;
            }
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = target;
        false
    }
}

/// The API view of one mount.
fn view_of(mount: &RemoteMount, mounts_text: &str) -> RemoteMountView {
    let target = target_for(&mount.name);
    RemoteMountView {
        mount: mount.clone(),
        target: target.to_string_lossy().into_owned(),
        mounted: is_mounted(mounts_text, &target),
        has_credentials: mount.kind == RemoteKind::Cifs && has_credentials(&mount.name),
    }
}

/// Every configured mount with its live state.
pub fn list() -> Vec<RemoteMountView> {
    let mounts_text = read_mounts();
    load().iter().map(|m| view_of(m, &mounts_text)).collect()
}

/// Add or replace one mount. The password is only written when given, so a
/// later edit that leaves the field blank keeps the stored one.
/// Validate and normalise every field that can reach `mount`, without
/// touching the host or the store.
///
/// The create handler calls this **before** probing the protocol helper, so a
/// malformed request is always a `400` even on a host where
/// `mount.cifs`/`mount.nfs` is absent. `upsert` calls it again before writing,
/// so the invariant holds for every caller.
pub fn validate_fields(mount: &mut RemoteMount) -> Result<(), String> {
    mount.name = validate_name(&mount.name)?;
    mount.server = validate_server(&mount.server)?;
    mount.source = validate_source(mount.kind, &mount.source)?;
    mount.options = validate_options(mount.kind, &mount.options)?;
    mount.username = validate_account("user name", mount.username.as_deref())?;
    mount.domain = validate_account("domain", mount.domain.as_deref())?;
    if mount.kind != RemoteKind::Cifs {
        // NFS has no account fields; storing them would imply an auth that is
        // not happening.
        mount.username = None;
        mount.domain = None;
    }
    Ok(())
}

pub fn upsert(mut mount: RemoteMount, password: Option<&str>) -> Result<RemoteMountView, String> {
    validate_fields(&mut mount)?;
    if mount.created_at.is_empty() {
        mount.created_at = chrono::Utc::now().to_rfc3339();
    }

    // A user name without any stored password would make `mount.cifs` prompt
    // on a terminal it does not have: refuse it here, where the message can
    // say what to do.
    if mount.kind == RemoteKind::Cifs
        && mount.username.is_some()
        && password.is_none()
        && !has_credentials(&mount.name)
    {
        return Err("A password is required when a user name is set".to_string());
    }

    let mut mounts = load();
    let previous = mounts.iter().position(|m| m.name == mount.name);
    match previous {
        Some(index) => mounts[index] = mount.clone(),
        None => mounts.push(mount.clone()),
    }
    save(&mounts)?;

    // Credentials: only rewritten when an account or a password is supplied.
    if mount.kind == RemoteKind::Cifs {
        if mount.username.is_some() && (password.is_some() || previous.is_none()) {
            write_credentials(
                &mount.name,
                mount.username.as_deref(),
                password,
                mount.domain.as_deref(),
            )?;
        } else if mount.username.is_none() {
            remove_credentials(&mount.name);
        }
    }

    Ok(view_of(&mount, &read_mounts()))
}

/// Forget one mount: unmount first when it is up, then drop its credentials.
pub fn remove(name: &str) -> Result<bool, String> {
    let mut mounts = load();
    let Some(index) = mounts.iter().position(|m| m.name == name) else {
        return Ok(false);
    };
    let target = target_for(name);
    if is_mounted(&read_mounts(), &target) {
        run_sudo(&umount_argv(&target), UMOUNT_TIMEOUT)?;
    }
    remove_credentials(name);
    let _ = std::fs::remove_dir(&target);
    mounts.remove(index);
    save(&mounts)?;
    Ok(true)
}

/// Mount every enabled location that is not up yet (the boot pass). Returns
/// the names that failed, so the caller can log them once instead of per
/// request. A failure never stops the next location.
pub fn reconcile() -> Vec<(String, String)> {
    let mut failures = Vec::new();
    for mount in load().into_iter().filter(|m| m.enabled) {
        if let Err(e) = mount_one(&mount.name) {
            tracing::warn!("remote mount '{}' not restored: {e}", mount.name);
            failures.push((mount.name, e));
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cifs(name: &str) -> RemoteMount {
        RemoteMount {
            name: name.to_string(),
            kind: RemoteKind::Cifs,
            server: "nas.example.com".to_string(),
            source: "photos".to_string(),
            username: None,
            domain: None,
            options: Vec::new(),
            read_only: true,
            enabled: true,
            created_at: String::new(),
            last_error: None,
        }
    }

    fn nfs(name: &str) -> RemoteMount {
        RemoteMount {
            kind: RemoteKind::Nfs,
            server: "192.168.1.20".to_string(),
            source: "/volume1/media".to_string(),
            ..cifs(name)
        }
    }

    #[test]
    fn name_is_a_strict_slug() {
        assert_eq!(validate_name(" photos-nas ").unwrap(), "photos-nas");
        for bad in [
            "",
            "Photos",
            "with space",
            "../etc",
            "a/b",
            "-lead",
            "trail-",
            "ä",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(validate_name(&"a".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn server_accepts_dns_and_ipv4_only() {
        assert_eq!(
            validate_server("nas.example.com").unwrap(),
            "nas.example.com"
        );
        assert_eq!(validate_server("192.168.1.20").unwrap(), "192.168.1.20");
        // Injected into `//server/share` and `server:/export`: nothing that
        // could add an option or a fake second path.
        for bad in [
            "nas/../etc",
            "nas share",
            "-oProxyCommand=x",
            "nas:2049",
            "fe80::1",
            "",
        ] {
            assert!(validate_server(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn source_shape_depends_on_the_protocol() {
        assert_eq!(
            validate_source(RemoteKind::Cifs, "photos").unwrap(),
            "photos"
        );
        assert!(validate_source(RemoteKind::Cifs, "photos/2024").is_err());
        assert!(validate_source(RemoteKind::Cifs, "photos 2024").is_err());
        assert_eq!(
            validate_source(RemoteKind::Nfs, "/volume1/media").unwrap(),
            "/volume1/media"
        );
        for bad in ["volume1/media", "/volume1/../etc", "/volume1//media", ""] {
            assert!(
                validate_source(RemoteKind::Nfs, bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn options_are_an_allowlist_per_protocol() {
        let ok = validate_options(
            RemoteKind::Cifs,
            &[
                "vers=3.0".to_string(),
                "uid=1000".to_string(),
                "nobrl".to_string(),
            ],
        )
        .unwrap();
        assert_eq!(ok, vec!["vers=3.0", "uid=1000", "nobrl"]);

        // Duplicates collapse; order is preserved.
        let dedup = validate_options(
            RemoteKind::Cifs,
            &["vers=3.0".to_string(), "vers=3.0".to_string()],
        )
        .unwrap();
        assert_eq!(dedup, vec!["vers=3.0"]);

        // An NFS option is not a CIFS option, and credentials can never be set
        // through the option list.
        assert!(validate_options(RemoteKind::Cifs, &["nolock".to_string()]).is_err());
        for bad in [
            "credentials=/etc/passwd",
            "username=root",
            "password=x",
            "rw",
            "ro",
            "suid",
            "dev",
            "exec",
            "vers=3.0,rw",
            "vers=",
            "vers=-1",
        ] {
            assert!(
                validate_options(RemoteKind::Cifs, &[bad.to_string()]).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(validate_options(
            RemoteKind::Cifs,
            &vec!["nobrl".to_string(); MAX_OPTIONS + 1]
        )
        .is_err());
    }

    #[test]
    fn account_fields_cannot_inject_a_credential_key() {
        assert_eq!(
            validate_account("user name", Some(" alice "))
                .unwrap()
                .as_deref(),
            Some("alice")
        );
        assert_eq!(validate_account("domain", Some("  ")).unwrap(), None);
        for bad in ["alice\npassword=x", "a=b", "user\u{0}"] {
            assert!(
                validate_account("user name", Some(bad)).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn mount_argv_is_exactly_what_mount_expects() {
        let mount = cifs("photos-nas");
        let target = Path::new("/mnt/homevault-remote/photos-nas");
        let cred = Path::new("/srv/homevault/data/remote-mounts/photos-nas.cred");
        assert_eq!(
            mount_argv(&mount, target, Some(cred)),
            vec![
                "mount",
                "-t",
                "cifs",
                "-o",
                "ro,credentials=/srv/homevault/data/remote-mounts/photos-nas.cred",
                "//nas.example.com/photos",
                "/mnt/homevault-remote/photos-nas",
            ]
        );
        // Read-write is explicit, and NFS has no credentials entry.
        let mut rw = nfs("media");
        rw.read_only = false;
        rw.options = vec!["vers=4.1".to_string()];
        assert_eq!(
            mount_argv(&rw, Path::new("/mnt/homevault-remote/media"), None),
            vec![
                "mount",
                "-t",
                "nfs",
                "-o",
                "rw,vers=4.1",
                "192.168.1.20:/volume1/media",
                "/mnt/homevault-remote/media",
            ]
        );
        assert_eq!(
            umount_argv(target),
            vec!["umount", "/mnt/homevault-remote/photos-nas"]
        );
    }

    #[test]
    fn credentials_body_never_uses_the_command_line() {
        assert_eq!(
            credentials_body("alice", Some("s3cr3t"), Some("WORKGROUP")),
            "username=alice\npassword=s3cr3t\ndomain=WORKGROUP\n"
        );
        // No password supplied: the username alone (mount.cifs then prompts
        // is impossible under sudo, so this is only a keytab-like fallback).
        assert_eq!(credentials_body("alice", None, None), "username=alice\n");
    }

    #[test]
    fn mount_table_parsing_handles_escapes_and_boundaries() {
        let mounts = "//nas/share /mnt/homevault-remote/photos cifs ro,relatime 0 0\n\
                      //nas/two\\040words /mnt/homevault-remote/two\\040words cifs ro 0 0\n\
                      /dev/sda1 / ext4 rw 0 0\n";
        assert!(is_mounted(
            mounts,
            Path::new("/mnt/homevault-remote/photos")
        ));
        assert!(!is_mounted(
            mounts,
            Path::new("/mnt/homevault-remote/photos2")
        ));
        assert!(is_mounted(
            mounts,
            Path::new("/mnt/homevault-remote/two words")
        ));
        assert_eq!(
            filesystem_at(mounts, Path::new("/mnt/homevault-remote/photos")).as_deref(),
            Some("cifs")
        );
        assert_eq!(filesystem_at(mounts, Path::new("/nope")), None);
        assert_eq!(unescape_mount_field("two\\040words"), "two words");
        assert_eq!(unescape_mount_field("plain"), "plain");
    }

    #[test]
    fn target_is_derived_from_the_name_only() {
        assert_eq!(
            target_for("photos-nas"),
            PathBuf::from("/mnt/homevault-remote/photos-nas")
        );
        // A name can never escape the root because validation already refused
        // the characters that would do it.
        assert!(validate_name("../../etc").is_err());
    }

    #[test]
    fn store_round_trips_and_ignores_unknown_keys() {
        let store = RemoteMountStore {
            mounts: vec![cifs("photos-nas"), nfs("media")],
        };
        let json = serde_json::to_string(&store).unwrap();
        let back: RemoteMountStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mounts, store.mounts);

        // A file written by a newer HomeVault still loads.
        let future =
            r#"{"mounts":[{"name":"x","kind":"cifs","server":"h","source":"s","future_key":1}]}"#;
        let parsed: RemoteMountStore = serde_json::from_str(future).unwrap();
        assert_eq!(parsed.mounts.len(), 1);
        assert!(parsed.mounts[0].read_only, "read-only defaults on");
        assert!(parsed.mounts[0].enabled);
    }
}
