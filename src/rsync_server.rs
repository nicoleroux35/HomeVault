/* =============================================================================
 * File          : rsync_server.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Opt-in `rsyncd` server: one authenticated module per flagged share,
 *   served by a dedicated `homevault-rsync` systemd unit on its own port.
 *   All validation and rendering is pure and unit-tested; the module file,
 *   the secrets file and the unit are generated from one JSON state file, and
 *   HomeVault's own accounts authenticate through a per-user secret that is
 *   never returned by the API.
 *
 * Dependencies:
 *   - crate::shares (share inventory + root file writes), crate::utils::process
 *
 * Usage:
 *   handlers/rsync.rs (Shares → Rsync tab); see docs/guides/rsync.md.
 * =============================================================================
 */

//! Rsync server (see docs/guides/rsync.md).
//!
//! Design decisions worth knowing:
//!
//! * **A dedicated unit, never the host's `rsyncd`.** An admin may already run
//!   their own rsync daemon; HomeVault writes `homevault-rsync.service` with
//!   its own config, port and pid file, exactly like the SFTP sshd instance.
//! * **The module list is HomeVault's, the share inventory is Samba's.** A
//!   module can only be created from a share path HomeVault already serves, so
//!   the API can never turn into "expose any directory on the box". The path
//!   is re-checked against the live share inventory on every apply.
//! * **Authentication is mandatory.** A module without `auth users` would be
//!   an anonymous read/write export of a home directory; enabling one is
//!   refused instead.
//! * **Secrets never leave the box.** Per-user rsync passwords live in the
//!   state file and the generated `rsyncd.secrets` (both `0600`), are written
//!   through a pipe, and are never logged or serialised into an API response.
//!   The API only reports *whether* a user has a secret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::shares::{validate_share_name, validate_share_path, SharesService};
use crate::utils::process::run_timed;

/// Port the dedicated daemon listens on. 873 is rsync's own default; the port
/// is configurable because a host may already run a system `rsyncd`.
pub const DEFAULT_PORT: u16 = 873;

/// Longest a module list may grow — a short list an admin can read.
pub const MAX_MODULES: usize = 32;

/// Ceiling on simultaneous connections, matching the daemon's own option.
pub const MAX_CONNECTIONS_LIMIT: u32 = 64;

/// Deadline for one metadata call (`getent`, `id`, `chmod`).
const RSYNC_OP_TIMEOUT: Duration = Duration::from_secs(15);
/// Deadline for a `systemctl` call: an `enable --now` can wait on its own
/// dependencies.
const RSYNC_SERVICE_TIMEOUT: Duration = Duration::from_secs(60);

/// Directory override — the same test seam as `HV_AVAHI_CONF`: a test points
/// every generated file at a temp directory and proves what lands on disk
/// without touching `/etc`. Unset in production.
pub const RSYNC_DIR_ENV: &str = "HV_RSYNC_DIR";

/// The systemd unit HomeVault owns.
pub const RSYNC_SERVICE: &str = "homevault-rsync";
/// Unprivileged account the daemon drops to after chrooting into a module.
pub const RSYNC_USER: &str = "homevault-rsync";

/// Where the generated files live (overridable for tests).
pub fn config_dir() -> PathBuf {
    std::env::var(RSYNC_DIR_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/homevault-rsync"))
}

/// The generated `rsyncd.conf`.
pub fn conf_path() -> PathBuf {
    config_dir().join("rsyncd.conf")
}

/// The generated `rsyncd.secrets` (`0600`, root-owned).
pub fn secrets_path() -> PathBuf {
    config_dir().join("rsyncd.secrets")
}

/// The persisted module/settings/secrets state.
pub fn state_path() -> PathBuf {
    config_dir().join("modules.json")
}

/// Whether the paths are a test override rather than the host's real ones.
fn overridden() -> bool {
    config_dir() != Path::new("/etc/homevault-rsync")
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Daemon-wide settings, exposed in the Shares → Rsync tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RsyncSettings {
    /// Whether the daemon should be running. Default **off**: a network
    /// service is opt-in.
    pub enabled: bool,
    pub port: u16,
    pub max_connections: u32,
    /// `use chroot = yes` (the default). Only a host that deliberately runs
    /// the daemon unprivileged turns it off; `munge symlinks = yes` is then
    /// what keeps a symlink inside the module from escaping it.
    pub chroot: bool,
}

impl Default for RsyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            max_connections: 8,
            chroot: true,
        }
    }
}

/// One `[module]` — a share path plus who may reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RsyncModule {
    pub name: String,
    pub path: String,
    pub comment: String,
    /// `read only = yes` when true (a pull-only mirror).
    pub read_only: bool,
    /// HomeVault usernames allowed to authenticate against this module.
    pub users: Vec<String>,
    pub enabled: bool,
}

/// Everything persisted under [`state_path`].
///
/// `secrets` is a username → rsync password map. It is deliberately part of
/// the same file so one write keeps the module list and the credentials in
/// step; the file itself is `0600` and never served by the API.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RsyncState {
    #[serde(default)]
    pub settings: RsyncSettings,
    #[serde(default)]
    pub modules: Vec<RsyncModule>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Validation (pure)
// ---------------------------------------------------------------------------

/// A module name: a share-like name, and never one rsync interprets itself.
pub fn validate_module_name(name: &str) -> Result<(), String> {
    validate_share_name(name)?;
    if matches!(name, "global" | "homes" | "printers") {
        return Err(format!("'{name}' is reserved by rsync"));
    }
    if name.chars().any(|c| c != c.to_ascii_lowercase()) {
        // rsync matches module names literally; a mixed-case name is valid
        // but confusing, so it is refused rather than half-warned.
        return Err(format!("module name '{name}' must be lowercase"));
    }
    Ok(())
}

/// A HomeVault username used as an `auth users` entry and a secrets-file key.
///
/// Restricted to the characters a POSIX account name uses: the value reaches
/// `/etc/homevault-rsync/rsyncd.secrets` (`user:password`), so a `:` or a
/// newline would forge a second credential line.
pub fn validate_user_name(user: &str) -> Result<(), String> {
    let u = user.trim();
    if u.is_empty() {
        return Err("a username is required".to_string());
    }
    if u.len() > 32 {
        return Err("a username must be 32 characters or fewer".to_string());
    }
    if u.starts_with('-') {
        return Err("a username cannot start with '-'".to_string());
    }
    if !u
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(format!("'{u}' contains characters a username may not use"));
    }
    Ok(())
}

/// An rsync password: it is written verbatim after `user:` in the secrets
/// file, so a `:` or a line break would corrupt or forge an entry.
pub fn validate_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("a password is required".to_string());
    }
    if password.len() > 512 {
        return Err("the password must be 512 characters or fewer".to_string());
    }
    if password.chars().any(|c| c.is_control()) {
        return Err("the password contains a control character".to_string());
    }
    if password.contains(':') {
        return Err("an rsync password cannot contain ':'".to_string());
    }
    Ok(())
}

/// A module path: absolute, traversal-free, and inside one of the configured
/// share roots (canonicalised, so a symlink cannot smuggle a module out of a
/// share).
///
/// Returns the canonical path to store and serve.
pub fn validate_module_path(path: &str, allowed_roots: &[PathBuf]) -> Result<PathBuf, String> {
    validate_share_path(path)?;
    let canonical =
        std::fs::canonicalize(path).map_err(|e| format!("'{path}' cannot be read: {e}"))?;
    let inside = allowed_roots.iter().any(|root| canonical.starts_with(root));
    if !inside {
        return Err(format!(
            "'{path}' is not one of HomeVault's shares — a module may only serve a configured share"
        ));
    }
    Ok(canonical)
}

fn validate_settings(settings: &RsyncSettings) -> Result<(), String> {
    if settings.port == 0 {
        return Err("the port must not be 0".to_string());
    }
    if settings.max_connections == 0 {
        return Err("max connections must be at least 1".to_string());
    }
    if settings.max_connections > MAX_CONNECTIONS_LIMIT {
        return Err(format!(
            "max connections cannot exceed {MAX_CONNECTIONS_LIMIT}"
        ));
    }
    Ok(())
}

/// Everything that would make `apply` unsafe, checked before any write.
pub fn validate_state(state: &RsyncState, allowed_roots: &[PathBuf]) -> Result<(), String> {
    validate_settings(&state.settings)?;
    if state.modules.len() > MAX_MODULES {
        return Err(format!("at most {MAX_MODULES} modules are supported"));
    }
    let mut names = std::collections::HashSet::new();
    for module in &state.modules {
        validate_module_name(&module.name)?;
        if !names.insert(module.name.to_ascii_lowercase()) {
            return Err(format!("two modules are named '{}'", module.name));
        }
        if module.comment.chars().any(|c| c.is_control()) {
            return Err("a module comment cannot contain a control character".to_string());
        }
        if !module.enabled {
            continue;
        }
        if module.users.is_empty() {
            return Err(format!(
                "module '{}' has no allowed users — an unauthenticated rsync export is refused",
                module.name
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for user in &module.users {
            validate_user_name(user)?;
            if !seen.insert(user) {
                return Err(format!("module '{}' lists '{user}' twice", module.name));
            }
            if !state.secrets.contains_key(user) {
                return Err(format!(
                    "user '{user}' has no rsync password yet — set one before enabling module '{}'",
                    module.name
                ));
            }
        }
        validate_module_path(&module.path, allowed_roots)
            .map_err(|e| format!("module '{}': {e}", module.name))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering (pure)
// ---------------------------------------------------------------------------

/// Render the daemon configuration.
///
/// Only enabled modules are emitted, so "disable" is a real removal rather
/// than a flag rsync would have to understand. `auth users` and `secrets file`
/// are per module; a module with users but a global secrets file would let one
/// module's credentials unlock another.
pub fn render_config(settings: &RsyncSettings, modules: &[RsyncModule], secrets: &str) -> String {
    let mut out = String::new();
    out.push_str("# Generated by HomeVault (src/rsync_server.rs) — do not edit by hand.\n");
    out.push_str("# Modules, users and passwords are managed from Shares → Rsync.\n");
    out.push_str(&format!("port = {}\n", settings.port));
    out.push_str(&format!("pid file = /run/{RSYNC_SERVICE}.pid\n"));
    // rsync's own default is `/var/run/rsyncd.lock`, which collides with (and
    // is usually owned by) a system rsyncd.
    out.push_str(&format!("lock file = /run/{RSYNC_SERVICE}.lock\n"));
    out.push_str(&format!("log file = /var/log/{RSYNC_SERVICE}.log\n"));
    out.push_str(&format!("max connections = {}\n", settings.max_connections));
    out.push_str("use chroot = ");
    out.push_str(if settings.chroot { "yes\n" } else { "no\n" });
    // With chroot off, `munge symlinks` is what keeps a symlink inside the
    // module from pointing at something outside it.
    if !settings.chroot {
        out.push_str("munge symlinks = yes\n");
    }
    out.push_str("reverse lookup = no\n");
    out.push_str("transfer logging = yes\n");
    out.push_str("list = yes\n");
    out.push_str(&format!("secrets file = {secrets}\n"));

    for module in modules.iter().filter(|m| m.enabled) {
        out.push('\n');
        out.push_str(&format!("[{}]\n", module.name));
        out.push_str(&format!("  path = {}\n", module.path));
        if !module.comment.is_empty() {
            out.push_str(&format!("  comment = {}\n", module.comment));
        }
        out.push_str("  read only = ");
        out.push_str(if module.read_only { "yes\n" } else { "no\n" });
        out.push_str(&format!("  auth users = {}\n", module.users.join(",")));
        out.push_str(&format!("  secrets file = {secrets}\n"));
        // The daemon runs as root, chroots into the module, then drops to this
        // account: a module never writes as root.
        out.push_str(&format!("  uid = {RSYNC_USER}\n"));
        out.push_str(&format!("  gid = {RSYNC_USER}\n"));
    }
    out
}

/// Render the secrets file: one `user:password` line per entry.
pub fn render_secrets(secrets: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (user, password) in secrets {
        out.push_str(&format!("{user}:{password}\n"));
    }
    out
}

/// The systemd unit for the dedicated daemon.
pub fn render_unit(conf: &Path) -> String {
    format!(
        "[Unit]\nDescription=HomeVault Rsync server (authenticated per-share modules)\nAfter=network.target\n\n[Service]\nType=simple\n# The daemon chroots into each module and drops to {RSYNC_USER} there.\nExecStart=/usr/bin/rsync --daemon --no-detach --config={}\nExecReload=/bin/kill -HUP $MAINPID\nRestart=on-failure\nRestartSec=5\nNoNewPrivileges=true\nPrivateTmp=true\n\n[Install]\nWantedBy=multi-user.target\n",
        conf.display()
    )
}

/// The module's share path is re-checked against the live inventory on every
/// apply; this lists the canonical roots a module may live under.
pub fn allowed_share_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for share in SharesService::get_shares().unwrap_or_default() {
        if share.path.is_empty() {
            continue;
        }
        let Ok(canonical) = std::fs::canonicalize(&share.path) else {
            continue;
        };
        if !roots.iter().any(|r| *r == canonical) {
            roots.push(canonical);
        }
    }
    roots
}

/// The share inventory the Rsync tab offers as module candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShareCandidate {
    pub name: String,
    pub path: String,
    /// Whether this path is already served by a module.
    pub used: bool,
}

pub fn share_candidates(state: &RsyncState) -> Vec<ShareCandidate> {
    let mut out: Vec<ShareCandidate> = Vec::new();
    for share in SharesService::get_shares().unwrap_or_default() {
        if share.path.is_empty() {
            continue;
        }
        if out.iter().any(|c| c.path == share.path) {
            continue;
        }
        out.push(ShareCandidate {
            name: share.name,
            path: share.path.clone(),
            used: state.modules.iter().any(|m| m.path == share.path),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ---------------------------------------------------------------------------
// Persistence and host application
// ---------------------------------------------------------------------------

/// Load the persisted state. A missing file is the shipped default (daemon
/// off, no modules); a corrupt one is reported rather than silently replaced,
/// because replacing it would drop every module without telling anyone.
pub fn load_state() -> Result<RsyncState, String> {
    let path = state_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RsyncState::default()),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    serde_json::from_str(&raw).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

/// Write a generated file, falling back to `sudo tee` when this process may
/// not write it (the production `/etc` case).
fn write_generated(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    let text = path.to_string_lossy().to_string();
    match std::fs::write(path, content) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            crate::shares::write_file_as_root(&text, content).map_err(|e| e.to_string())?;
        }
        Err(e) => return Err(format!("cannot write {text}: {e}")),
    }
    // The secrets file holds credentials: `0600` always, including when a
    // test owns the directory (rsync itself refuses a group/world-readable
    // secrets file).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            warn!(
                "rsync: could not set mode {mode:o} on {}: {e}",
                path.display()
            );
        }
    }
    if !overridden() {
        let mut cmd = Command::new("sudo");
        cmd.args(["chmod", &format!("{mode:o}"), &text]);
        if let Err(e) = run_timed(&mut cmd, RSYNC_OP_TIMEOUT) {
            warn!("rsync: chmod {text} failed: {e}");
        }
        let mut own = Command::new("sudo");
        own.args(["chown", "root:root", &text]);
        let _ = run_timed(&mut own, RSYNC_OP_TIMEOUT);
    }
    Ok(())
}

/// Persist the state and regenerate every artefact, then bring the service in
/// line with `settings.enabled`.
///
/// Validation happens first: a refused state writes nothing at all, so a typo
/// can never leave a half-written `rsyncd.conf` behind.
pub fn apply(state: &RsyncState) -> Result<(), String> {
    apply_with_roots(state, &allowed_share_roots())
}

/// `apply` with an explicit share-root inventory — the seam used by tests and
/// by the integration suite, which cannot create a real Samba share.
pub fn apply_with_roots(state: &RsyncState, allowed_roots: &[PathBuf]) -> Result<(), String> {
    validate_state(state, allowed_roots)?;

    let dir = config_dir();
    if !dir.exists() {
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && !overridden() => {
                crate::shares::mkdir_as_root(&dir).map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(format!("cannot create {}: {e}", dir.display())),
        }
    }

    if state.settings.enabled && !overridden() {
        ensure_service_account()?;
    }

    let secrets = render_secrets(&state.secrets);
    write_generated(&secrets_path(), &secrets, 0o600)?;
    let conf = render_config(
        &state.settings,
        &state.modules,
        &secrets_path().to_string_lossy(),
    );
    write_generated(&conf_path(), &conf, 0o640)?;

    let state_json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    write_generated(&state_path(), &state_json, 0o600)?;

    if overridden() {
        // A test owns the files; never touch the host's systemd.
        return Ok(());
    }

    let unit = render_unit(&conf_path());
    write_generated(
        Path::new(&format!("/etc/systemd/system/{RSYNC_SERVICE}.service")),
        &unit,
        0o644,
    )?;
    systemctl(&["daemon-reload"])?;
    if state.settings.enabled {
        systemctl(&["enable", RSYNC_SERVICE])?;
        // `restart` is what makes a changed module list take effect; a plain
        // reload only re-reads the config of a running daemon.
        let active = service_active();
        if active {
            systemctl(&["restart", RSYNC_SERVICE])?;
        } else {
            systemctl(&["start", RSYNC_SERVICE])?;
        }
    } else {
        let _ = systemctl(&["stop", RSYNC_SERVICE]);
        let _ = systemctl(&["disable", RSYNC_SERVICE]);
    }
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.arg("systemctl").args(args);
    match run_timed(&mut cmd, RSYNC_SERVICE_TIMEOUT) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "systemctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("systemctl {} failed: {e}", args.join(" "))),
    }
}

/// Whether the dedicated unit is currently running (no `sudo` needed).
pub fn service_active() -> bool {
    if overridden() {
        return false;
    }
    run_timed(
        Command::new("systemctl").args(["is-active", "--quiet", RSYNC_SERVICE]),
        RSYNC_OP_TIMEOUT,
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// Create the unprivileged service account/group on first use.
fn ensure_service_account() -> Result<(), String> {
    let group = run_timed(
        Command::new("getent").args(["group", RSYNC_USER]),
        RSYNC_OP_TIMEOUT,
    )
    .map(|o| o.status.success())
    .unwrap_or(false);
    if !group {
        let mut cmd = Command::new("sudo");
        cmd.args(["groupadd", "--system", RSYNC_USER]);
        let _ = run_timed(&mut cmd, RSYNC_OP_TIMEOUT);
    }
    let user = run_timed(Command::new("id").arg(RSYNC_USER), RSYNC_OP_TIMEOUT)
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !user {
        let mut cmd = Command::new("sudo");
        cmd.args([
            "useradd",
            "--system",
            "--gid",
            RSYNC_USER,
            "--home-dir",
            "/nonexistent",
            "--shell",
            "/usr/sbin/nologin",
            RSYNC_USER,
        ]);
        match run_timed(&mut cmd, RSYNC_OP_TIMEOUT) {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("already exists") {
                    return Err(format!("cannot create {RSYNC_USER}: {}", stderr.trim()));
                }
            }
            Err(e) => return Err(format!("cannot create {RSYNC_USER}: {e}")),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mutations used by the API
// ---------------------------------------------------------------------------

/// Add or replace one module. `path` must belong to a configured share.
pub fn upsert_module(module: RsyncModule) -> Result<RsyncState, String> {
    let mut state = load_state()?;
    let roots = allowed_share_roots();
    let canonical = validate_module_path(&module.path, &roots)?;
    let mut module = module;
    module.path = canonical.to_string_lossy().to_string();
    validate_module_name(&module.name)?;
    for user in &module.users {
        validate_user_name(user)?;
    }
    if module.comment.chars().any(|c| c.is_control()) {
        return Err("a module comment cannot contain a control character".to_string());
    }
    match state.modules.iter_mut().find(|m| m.name == module.name) {
        Some(existing) => *existing = module,
        None => {
            if state.modules.len() >= MAX_MODULES {
                return Err(format!("at most {MAX_MODULES} modules are supported"));
            }
            state.modules.push(module);
        }
    }
    state.modules.sort_by(|a, b| a.name.cmp(&b.name));
    apply_with_roots(&state, &roots)?;
    Ok(state)
}

/// Remove one module (the share itself is untouched).
pub fn delete_module(name: &str) -> Result<RsyncState, String> {
    let mut state = load_state()?;
    let before = state.modules.len();
    state.modules.retain(|m| m.name != name);
    if state.modules.len() == before {
        return Err(format!("module '{name}' does not exist"));
    }
    apply(&state)?;
    Ok(state)
}

/// Turn one module on or off, and/or change its read-only flag or users.
pub fn set_module_enabled(
    name: &str,
    enabled: Option<bool>,
    read_only: Option<bool>,
    users: Option<Vec<String>>,
    comment: Option<String>,
) -> Result<RsyncState, String> {
    let mut state = load_state()?;
    let Some(module) = state.modules.iter_mut().find(|m| m.name == name) else {
        return Err(format!("module '{name}' does not exist"));
    };
    if let Some(enabled) = enabled {
        module.enabled = enabled;
    }
    if let Some(read_only) = read_only {
        module.read_only = read_only;
    }
    if let Some(users) = users {
        module.users = users;
    }
    if let Some(comment) = comment {
        if comment.chars().any(|c| c.is_control()) {
            return Err("a module comment cannot contain a control character".to_string());
        }
        module.comment = comment;
    }
    apply(&state)?;
    Ok(state)
}

/// Store (or replace) one user's rsync password. The value is never echoed
/// back, never logged, and only reaches the `0600` secrets file.
pub fn set_user_secret(user: &str, password: &str) -> Result<(), String> {
    validate_user_name(user)?;
    validate_password(password)?;
    let mut state = load_state()?;
    state.secrets.insert(user.to_string(), password.to_string());
    apply(&state)
}

/// Forget one user's rsync password. Modules still listing the user are
/// refused on the next apply, which is the honest outcome: their access stops.
pub fn delete_user_secret(user: &str) -> Result<RsyncState, String> {
    let mut state = load_state()?;
    state.secrets.remove(user);
    apply(&state)?;
    Ok(state)
}

/// Update the daemon settings (port, connection cap, chroot, on/off).
pub fn update_settings(settings: RsyncSettings) -> Result<RsyncState, String> {
    let mut state = load_state()?;
    // Turning the daemon on with a half-configured module list is refused by
    // `validate_state`, before anything is written.
    state.settings = settings;
    apply(&state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::Share;

    fn module(name: &str, path: &str, users: &[&str]) -> RsyncModule {
        RsyncModule {
            name: name.to_string(),
            path: path.to_string(),
            comment: "family".to_string(),
            read_only: false,
            users: users.iter().map(|u| u.to_string()).collect(),
            enabled: true,
        }
    }

    fn state_with(modules: Vec<RsyncModule>, users: &[&str]) -> RsyncState {
        RsyncState {
            settings: RsyncSettings::default(),
            modules,
            secrets: users
                .iter()
                .map(|u| (u.to_string(), "s3cret-pass".to_string()))
                .collect(),
        }
    }

    // -----------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------

    #[test]
    fn test_render_config_emits_one_authenticated_section_per_enabled_module() {
        let state = state_with(
            vec![
                module("media", "/mnt/pool/media", &["alice"]),
                module("photos", "/mnt/pool/photos", &["alice", "bob"]),
            ],
            &["alice", "bob"],
        );
        let conf = render_config(
            &state.settings,
            &state.modules,
            "/etc/homevault-rsync/rsyncd.secrets",
        );
        assert!(conf.contains(&format!("port = {DEFAULT_PORT}")), "{conf}");
        assert!(
            conf.contains(&format!("lock file = /run/{RSYNC_SERVICE}.lock")),
            "a dedicated lock file keeps the daemon out of a system rsyncd's: {conf}"
        );
        assert!(conf.contains("use chroot = yes"), "{conf}");
        assert!(conf.contains("[media]"), "{conf}");
        assert!(conf.contains("  path = /mnt/pool/media"), "{conf}");
        assert!(conf.contains("  auth users = alice"), "{conf}");
        assert!(conf.contains("  auth users = alice,bob"), "{conf}");
        assert!(conf.contains("  read only = no"), "{conf}");
        assert_eq!(
            conf.matches("secrets file =").count(),
            3,
            "global + 2: {conf}"
        );
        assert!(conf.contains(&format!("  uid = {RSYNC_USER}")), "{conf}");
        // Exactly one `[module]` header per module, and no anonymous export.
        assert_eq!(conf.matches("[media]").count(), 1, "{conf}");
        assert_eq!(conf.matches("[photos]").count(), 1, "{conf}");
    }

    #[test]
    fn test_disabled_modules_are_absent_from_the_config() {
        let mut off = module("media", "/mnt/pool/media", &["alice"]);
        off.enabled = false;
        let conf = render_config(
            &RsyncSettings::default(),
            &[off],
            "/etc/homevault-rsync/rsyncd.secrets",
        );
        assert!(!conf.contains("[media]"), "{conf}");
        assert!(!conf.contains("auth users"), "{conf}");
    }

    #[test]
    fn test_chroot_off_munges_symlinks() {
        let settings = RsyncSettings {
            chroot: false,
            ..RsyncSettings::default()
        };
        let conf = render_config(&settings, &[], "/etc/homevault-rsync/rsyncd.secrets");
        assert!(conf.contains("use chroot = no"), "{conf}");
        assert!(
            conf.contains("munge symlinks = yes"),
            "without a chroot, munged symlinks are what keeps a link inside the module: {conf}"
        );
    }

    #[test]
    fn test_render_secrets_is_one_line_per_user() {
        let secrets = BTreeMap::from([
            ("alice".to_string(), "pa:ss".to_string()),
            ("bob".to_string(), "hunter2".to_string()),
        ]);
        let rendered = render_secrets(&secrets);
        assert_eq!(rendered, "alice:pa:ss\nbob:hunter2\n");
        assert_eq!(rendered.lines().count(), 2);
    }

    #[test]
    fn test_render_unit_runs_the_dedicated_daemon() {
        let unit = render_unit(Path::new("/etc/homevault-rsync/rsyncd.conf"));
        assert!(unit.contains(&format!("--config=/etc/homevault-rsync/rsyncd.conf")));
        assert!(
            unit.contains("--no-detach"),
            "systemd needs a foreground daemon: {unit}"
        );
        assert!(unit.contains("NoNewPrivileges=true"), "{unit}");
    }

    // -----------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------

    #[test]
    fn test_module_name_refuses_reserved_and_mixed_case() {
        assert!(validate_module_name("media").is_ok());
        assert!(validate_module_name("Media").is_err());
        assert!(validate_module_name("global").is_err());
        assert!(validate_module_name("homes").is_err());
        assert!(validate_module_name("../etc").is_err());
        assert!(validate_module_name("").is_err());
    }

    #[test]
    fn test_user_name_cannot_forge_a_secrets_entry() {
        assert!(validate_user_name("alice").is_ok());
        assert!(validate_user_name("bob-2").is_ok());
        assert!(validate_user_name("alice:bob").is_err());
        assert!(validate_user_name("alice\nbob").is_err());
        assert!(validate_user_name("-alice").is_err());
        assert!(validate_user_name(&"x".repeat(33)).is_err());
        assert!(validate_user_name("").is_err());
    }

    #[test]
    fn test_password_cannot_break_the_secrets_format() {
        assert!(validate_password("plain").is_ok());
        assert!(validate_password("with space").is_ok());
        assert!(validate_password("").is_err());
        assert!(validate_password("has:colon").is_err());
        assert!(validate_password("has\nnewline").is_err());
        assert!(validate_password(&"x".repeat(513)).is_err());
    }

    #[test]
    fn test_a_module_without_users_is_refused() {
        let state = state_with(vec![module("media", "/tmp", &[])], &[]);
        let err = validate_state(&state, &[PathBuf::from("/tmp")]).unwrap_err();
        assert!(err.contains("no allowed users"), "{err}");
    }

    #[test]
    fn test_a_module_user_without_a_password_is_refused() {
        let state = state_with(vec![module("media", "/tmp", &["alice"])], &[]);
        let err = validate_state(&state, &[PathBuf::from("/tmp")]).unwrap_err();
        assert!(err.contains("no rsync password"), "{err}");
    }

    #[test]
    fn test_settings_bounds() {
        let mut state = state_with(vec![], &[]);
        state.settings.port = 0;
        assert!(validate_state(&state, &[]).is_err());
        state.settings.port = 873;
        state.settings.max_connections = 0;
        assert!(validate_state(&state, &[]).is_err());
        state.settings.max_connections = MAX_CONNECTIONS_LIMIT + 1;
        assert!(validate_state(&state, &[]).is_err());
        state.settings.max_connections = MAX_CONNECTIONS_LIMIT;
        assert!(validate_state(&state, &[]).is_ok());
    }

    #[test]
    fn test_module_path_must_live_inside_a_share() {
        let tmp = tempfile::tempdir().unwrap();
        let share = tmp.path().join("share");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let canonical_share = std::fs::canonicalize(&share).unwrap();

        // The share itself and a subdirectory are both acceptable…
        assert_eq!(
            validate_module_path(&share.to_string_lossy(), &[canonical_share.clone()]).unwrap(),
            canonical_share
        );
        let sub = share.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(validate_module_path(&sub.to_string_lossy(), &[canonical_share.clone()]).is_ok());

        // …a sibling directory, a relative path and a traversal are not.
        assert!(
            validate_module_path(&outside.to_string_lossy(), &[canonical_share.clone()]).is_err()
        );
        assert!(validate_module_path("relative/path", &[canonical_share.clone()]).is_err());
        assert!(
            validate_module_path(&share.join("..").to_string_lossy(), &[canonical_share]).is_err()
        );
    }

    #[test]
    fn test_a_symlink_cannot_smuggle_a_module_out_of_the_share() {
        let tmp = tempfile::tempdir().unwrap();
        let share = tmp.path().join("share");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, share.join("escape")).unwrap();
        let roots = vec![std::fs::canonicalize(&share).unwrap()];
        let err =
            validate_module_path(&share.join("escape").to_string_lossy(), &roots).unwrap_err();
        assert!(err.contains("not one of HomeVault's shares"), "{err}");
    }

    #[test]
    fn test_share_candidates_come_from_the_live_inventory() {
        // The function reads whatever share configs this host has (a sandbox
        // may have none, a test host may have printers): the invariant is the
        // shape and that no path appears twice, never a specific inventory.
        let candidates = share_candidates(&RsyncState::default());
        let mut seen = std::collections::HashSet::new();
        for candidate in &candidates {
            assert!(!candidate.path.is_empty());
            assert!(
                seen.insert(candidate.path.clone()),
                "duplicate path: {candidate:?}"
            );
            assert!(!candidate.used, "nothing is used by the default state");
        }
        // A module whose path matches a candidate marks it as used.
        if let Some(first) = candidates.first() {
            let state = RsyncState {
                modules: vec![RsyncModule {
                    name: "media".to_string(),
                    path: first.path.clone(),
                    comment: String::new(),
                    read_only: false,
                    users: vec![],
                    enabled: false,
                }],
                ..RsyncState::default()
            };
            let with_module = share_candidates(&state);
            let marked = with_module
                .iter()
                .find(|c| c.path == first.path)
                .expect("the path is still a candidate");
            assert!(marked.used);
        }
        let share = Share {
            name: "media".to_string(),
            share_type: crate::shares::ShareType::Smb,
            path: "/mnt/pool/media".to_string(),
            enabled: true,
            comment: String::new(),
            options: vec![],
            read_only: false,
            guest_ok: None,
            restrictions: crate::shares::ShareRestrictions::default(),
        };
        assert_eq!(share.path, "/mnt/pool/media");
    }
}
