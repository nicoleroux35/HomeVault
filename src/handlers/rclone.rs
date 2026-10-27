/* =============================================================================
 * File          : rclone.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Cloud-sync endpoints backed by rclone: remote and provider listing,
 *   configuration read/save (including adding and deleting remotes),
 *   mount start/stop, one-shot copy/sync operations, and scheduled sync
 *   jobs persisted in the database. Reads use session auth; every mutation
 *   — and reading the raw config, which contains every remote's
 *   credentials — requires admin. The rclone binary runs locally, spawned
 *   per operation.
 *
 *   Every subprocess is bounded by a deadline (ROADMAP 16.1), the mounts this
 *   process starts are tracked and unmounted at shutdown (16.2), and the
 *   config file carries the credentials so it is written mode 0600 (16.3).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/rclone in src/web/mod.rs; the Cloud Sync desktop app and
 *   the backup flows consume them.
 * =============================================================================
 */

//! Cloud-sync endpoints backed by rclone (remotes, mounts, sync jobs,
//! copy/sync operations).

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path as FsPath};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, Command};

use crate::config::paths::RCLONE_MOUNT_BASE;
use crate::handlers::AppStateArc;
use crate::storage::sync_job_repo::{NewSyncJob, ScheduleKind, SyncJob, SyncJobRepository};
use crate::utils::process::run_timed_async_result;

// ---------------------------------------------------------------------------
// Deadlines (ROADMAP 16.1)
// ---------------------------------------------------------------------------

/// One-shot rclone probe: `rclone version`, `rclone listremotes`,
/// `fusermount -V`. rclone answers these before it touches the network.
pub const RCLONE_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `fusermount -u` / `umount -f` may take. A wedged FUSE server is
/// exactly the case this deadline exists for: waiting forever is what left the
/// handler task (and the mount) alive.
pub const RCLONE_UMOUNT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a started mount is given to appear in the kernel's mount table.
/// rclone authenticates with the remote first, so this is not instant on a
/// slow provider — but it is a start, not a transfer.
pub const RCLONE_MOUNT_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Ceiling on one `rclone sync` / `rclone copy`.
///
/// Deliberately generous: a first sync of a large library legitimately takes
/// hours, and a shorter ceiling would turn a working transfer into a failure.
/// It exists so a stalled remote can no longer hold the HTTP request and the
/// activity job open *forever* — see `docs/guides/cloud-sync.md` for the
/// trade-off and for the "Run now" path that shares it.
pub const RCLONE_TRANSFER_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

/// Poll interval while waiting for a mount to become active.
const MOUNT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Mode every write of the config — and of its `.backup.*` sibling — is
/// pinned to. The file holds every remote's credentials (rclone only
/// "obscures" them), so it must never be group- or world-readable.
pub const CONFIG_FILE_MODE: u32 = 0o600;

/// Longest accepted remote name. rclone section names are short in practice;
/// the bound keeps one config section from being absurd.
const MAX_REMOTE_NAME_LEN: usize = 64;

/// Longest accepted provider type (`type = …` in the config file).
const MAX_PROVIDER_LEN: usize = 32;

/// Longest accepted mount point.
const MAX_MOUNT_POINT_LEN: usize = 200;

/// Longest accepted sub-path inside a remote.
const MAX_REMOTE_PATH_LEN: usize = 200;

/// How much of rclone's stderr is kept for the "the mount did not start"
/// message. The tail only — the beginning of a mount log is always the same
/// preamble.
const STDERR_TAIL_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// Mount registry (ROADMAP 16.2 / 16.3)
// ---------------------------------------------------------------------------

/// One mount this process started and therefore owns.
///
/// `handle_mount_start` used to call `spawn()` and drop the handle, so nothing
/// could report a failure after the fact, nothing could unmount it on
/// shutdown, and a second "Start mount" click stacked a second rclone on the
/// same directory. The registry is process-wide for the same reason the
/// downloader's state is: one process owns the mounts it started.
pub struct MountedRemote {
    pub child: Child,
    pub remote: String,
    pub mount_point: String,
    /// Last bytes rclone wrote on stderr, quoted in a startup failure.
    pub log_tail: Arc<Mutex<String>>,
}

/// Every mount this process owns, keyed by mount point.
static MOUNTS: LazyLock<tokio::sync::Mutex<HashMap<String, MountedRemote>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

/// Response wrapper for API responses
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: &str) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }
    }
}

/// Returns the authenticated username, or a human-readable error if the
/// session is missing/invalid. Every handler in this module returns
/// `Json<ApiResponse<T>>` directly (not `axum::response::Response`, which
/// is what `handlers::admin_guard::require_auth`/`require_admin` return) —
/// duplicated here as a plain `Result<String, String>` so each handler can
/// fold the error straight into its own `ApiResponse<T>` without changing
/// every return type. Same session-token-header convention used throughout
/// the rest of the codebase (e.g. `handlers/explorer.rs`).
async fn check_session(state: &AppStateArc, headers: &HeaderMap) -> Result<String, String> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Err("No session token provided".to_string());
    }

    match state.auth.get_username_from_token(session_token).await {
        Some(username) => Ok(username),
        None => Err("Invalid or expired session".to_string()),
    }
}

/// Same as `check_session`, but also requires the session to be an admin —
/// used for every mutating rclone endpoint and for reading the raw config
/// (which contains every remote's credentials).
async fn check_admin(state: &AppStateArc, headers: &HeaderMap) -> Result<String, String> {
    let username = check_session(state, headers).await?;
    if state.auth.is_admin(&username).await {
        Ok(username)
    } else {
        Err("Admin privileges required for this operation".to_string())
    }
}

// ---------------------------------------------------------------------------
// Input validation (ROADMAP 16.3)
//
// Every value below reaches an `rclone` argv or the INI file the credentials
// live in. They are pure functions so the matrices are unit-testable without
// a process, a file or a network.
// ---------------------------------------------------------------------------

/// Validate a remote name — an rclone config *section* name.
///
/// Rejects `:` (rclone's own `remote:path` separator), `[`/`]` (the section
/// delimiters, i.e. INI injection), `/` and `\` (a name is not a path), any
/// control character, whitespace (a name is never quoted, so a space would
/// silently truncate it in a hand-edited section) and a leading `-` (which an
/// argv parser could read as a flag).
pub fn validate_remote_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("Remote name is required".to_string());
    }
    if name.len() > MAX_REMOTE_NAME_LEN {
        return Err(format!(
            "Remote name is longer than {} characters",
            MAX_REMOTE_NAME_LEN
        ));
    }
    if name.starts_with('-') {
        return Err("Remote name must not start with '-'".to_string());
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '+');
    if !name.chars().all(allowed) {
        return Err(
            "Remote name contains invalid characters (letters, digits, '-', '_', '.', '@' and '+' only)"
                .to_string(),
        );
    }
    Ok(name.to_string())
}

/// Validate a provider type — the value written as `type = …` in the config.
///
/// rclone type names are lower-case slugs (`drive`, `s3`, `webdav`, …). The
/// field is free text from the API, so an unvalidated value containing a
/// newline would append attacker-chosen lines to the INI section — the same
/// class of defect as the `[section]` injection guard on the `config` field.
pub fn validate_provider_slug(raw: &str) -> Result<String, String> {
    let provider = raw.trim().to_ascii_lowercase();
    if provider.is_empty() {
        return Err("Provider type is required".to_string());
    }
    if provider.len() > MAX_PROVIDER_LEN {
        return Err(format!(
            "Provider type is longer than {} characters",
            MAX_PROVIDER_LEN
        ));
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    if !provider.chars().all(allowed) {
        return Err(
            "Provider type contains invalid characters (letters, digits, '-' and '_' only)"
                .to_string(),
        );
    }
    Ok(provider)
}

/// Validate a mount point and return it normalised.
///
/// A mount point is turned into an `rclone mount` argv entry and a
/// `fusermount -u` target, so it must be an absolute path **strictly inside**
/// [`RCLONE_MOUNT_BASE`], with no `.`/`..` component, no control character and
/// no leading `-` on any component. The server — not the browser — is the
/// authority here: `CloudSync.svelte` used to build `/mnt/rclone/<name>`
/// itself, and any admin-gated client could post anything.
pub fn validate_mount_point(raw: &str) -> Result<String, String> {
    // Checked on the **raw** input: `trim()` below silently removes a trailing
    // newline, so the check used to run after the evidence was gone.
    if raw.chars().any(|c| c.is_control()) {
        return Err("Mount point contains control characters".to_string());
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Mount point is required".to_string());
    }
    if trimmed.len() > MAX_MOUNT_POINT_LEN {
        return Err(format!(
            "Mount point is longer than {} characters",
            MAX_MOUNT_POINT_LEN
        ));
    }
    if !trimmed.starts_with('/') {
        return Err(format!(
            "Mount point must be an absolute path under {}",
            RCLONE_MOUNT_BASE
        ));
    }
    // `.` and `..` segments: `Path::components()` skips `CurDir` in the middle
    // of a path, so the containment check below would accept
    // `/mnt/rclone/./x` and `/mnt/rclone/..` while `mount` resolved them
    // somewhere else.
    if trimmed.split('/').any(|seg| seg == "." || seg == "..") {
        return Err("Mount point must not contain '.' or '..'".to_string());
    }
    let path = FsPath::new(trimmed);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err("Mount point must not contain '.' or '..'".to_string());
    }

    let base = FsPath::new(RCLONE_MOUNT_BASE);
    let relative = path.strip_prefix(base).map_err(|_| {
        format!(
            "Mount point must live under {} (one folder per remote)",
            RCLONE_MOUNT_BASE
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Err(format!(
            "Mount point must be a sub-folder of {}, not the shared root",
            RCLONE_MOUNT_BASE
        ));
    }
    if relative.components().any(|component| match component {
        Component::Normal(name) => name.to_string_lossy().starts_with('-'),
        _ => false,
    }) {
        return Err("Mount point folder must not start with '-'".to_string());
    }

    Ok(base.join(relative).to_string_lossy().into_owned())
}

/// Validate the optional sub-path inside a remote (`remote:sub/path`).
///
/// rclone's own paths are relative to the remote root, so an absolute path
/// (`/etc`) or a `..` component is meaningless at best. A backslash is
/// refused because it is a path separator on the provider side and a client
/// sending one is describing something else than it thinks.
pub fn validate_remote_subpath(raw: &Option<String>) -> Result<Option<String>, String> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let trimmed = value.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > MAX_REMOTE_PATH_LEN {
        return Err(format!(
            "Remote path is longer than {} characters",
            MAX_REMOTE_PATH_LEN
        ));
    }
    if value.trim().starts_with('/') {
        return Err("Remote path must be relative to the remote root".to_string());
    }
    if trimmed.contains('\\') || trimmed.chars().any(|c| c.is_control()) {
        return Err("Remote path contains invalid characters".to_string());
    }
    if trimmed
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return Err("Remote path must not contain '.' or '..'".to_string());
    }
    Ok(Some(trimmed.to_string()))
}

/// `true` when the config holds a section named exactly `name`.
///
/// The previous check was `content.contains("[name]")`, which matched
/// `[my]` inside `[mydrive]` — so adding `my` was refused as a duplicate, and
/// deleting `my` could delete `mydrive`.
pub fn config_has_section(content: &str, name: &str) -> bool {
    let header = format!("[{}]", name);
    content.lines().any(|line| line.trim() == header)
}

/// Remove the `[name]` section from an INI-style config, keeping every other
/// byte.
///
/// The previous implementation searched for the first `[` after the header,
/// which both matched a *prefix* (`[my]` → `[mydrive]`) and could cut a
/// section in half when a value contained a `[`. Here the header is matched
/// on a whole trimmed line and the section ends at the next line that starts
/// with `[` — a comment (`# [x]`) or a bracketed value (`patterns = [a-z]`)
/// does not end it.
pub fn remove_section(content: &str, name: &str) -> Result<String, String> {
    let header = format!("[{}]", name);
    let lines: Vec<&str> = content.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == header)
        .ok_or_else(|| format!("Remote '{}' not found", name))?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .map(|offset| start + 1 + offset)
        .unwrap_or(lines.len());

    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..start]);
    kept.extend_from_slice(&lines[end..]);

    // Collapse the blank line the removal may leave at the seam, then keep a
    // single trailing newline (rclone and every INI reader accept it).
    while kept.first() == Some(&"") {
        kept.remove(0);
    }
    while kept.last() == Some(&"") {
        kept.pop();
    }
    if kept.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{}\n", kept.join("\n")))
}

/// Parse the text of an rclone config into its remotes (pure).
///
/// Sections without a `type` are skipped: rclone itself ignores them, and the
/// screen cannot offer a remote it does not know how to describe.
pub fn parse_rclone_config_text(content: &str) -> Option<Vec<Remote>> {
    let mut remotes = Vec::new();
    let mut current_section = String::new();
    let mut current_type = String::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse section headers like [section_name]
        if line.starts_with('[') && line.ends_with(']') {
            // Save previous remote if it has a type
            if !current_section.is_empty() && !current_type.is_empty() {
                let description = get_provider_description(&current_type);
                remotes.push(Remote {
                    name: current_section.clone(),
                    remote_type: description,
                });
            }
            current_section = line[1..line.len() - 1].to_string();
            current_type.clear();
            continue;
        }

        // Parse key=value pairs
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_lowercase();
            let value = value.trim();

            if key == "type" {
                current_type = value.to_string();
            }
        }
    }

    // Don't forget the last remote
    if !current_section.is_empty() && !current_type.is_empty() {
        let description = get_provider_description(&current_type);
        remotes.push(Remote {
            name: current_section,
            remote_type: description,
        });
    }

    if remotes.is_empty() {
        return None;
    }

    remotes.sort_by(|a, b| a.name.cmp(&b.name));
    Some(remotes)
}

/// Read and parse the config off the async runtime (ROADMAP 16.1's sibling:
/// the blocking `std::fs` call left the handler's async body).
async fn load_remotes() -> Option<Vec<Remote>> {
    let path = rclone_config_path();
    tokio::task::spawn_blocking(move || {
        let content = read_rclone_config(&path)?;
        parse_rclone_config_text(&content)
    })
    .await
    .ok()
    .flatten()
}

/// Write the config (or one of its backups) with mode 0600.
///
/// `OpenOptions::mode` only applies on creation, so the mode is pinned again
/// afterwards — that is what repairs a file an older build left at 0644.
async fn write_private_config(path: &FsPath, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(CONFIG_FILE_MODE)
        .open(path)
        .await
        .map_err(|e| format!("Failed to save config: {}", e))?;
    file.write_all(content.as_bytes())
        .await
        .map_err(|e| format!("Failed to save config: {}", e))?;
    file.flush()
        .await
        .map_err(|e| format!("Failed to save config: {}", e))?;

    let mut permissions = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("Failed to read config metadata: {}", e))?
        .permissions();
    permissions.set_mode(CONFIG_FILE_MODE);
    tokio::fs::set_permissions(path, permissions)
        .await
        .map_err(|e| format!("Failed to restrict config permissions: {}", e))?;
    Ok(())
}

/// `true` when `mount_point` appears as a mount point in `/proc/self/mounts`.
///
/// The kernel's own view, not the registry: a mount started before a restart
/// is still mounted, and a registry entry is a claim about a child process,
/// not about the filesystem. The mount point is matched as a whole field and
/// octal escapes (`\040` for a space) are decoded, so `/mnt/rclone/vault2`
/// can never satisfy `/mnt/rclone/vault`.
fn parse_mounts_for(content: &str, mount_point: &str) -> bool {
    content.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _device = fields.next();
        match fields.next() {
            Some(field) => decode_mount_escape(field) == mount_point,
            None => false,
        }
    })
}

/// Decode the `\040`-style escapes `/proc/*/mounts` uses for space, tab,
/// newline and backslash in a path.
fn decode_mount_escape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        if digits.len() == 3 && digits.chars().all(|d| d.is_ascii_digit()) {
            if let Ok(byte) = u8::from_str_radix(&digits, 8) {
                out.push(byte as char);
                for _ in 0..3 {
                    chars.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

async fn mount_is_active(mount_point: &str) -> bool {
    match tokio::fs::read_to_string("/proc/self/mounts").await {
        Ok(content) => parse_mounts_for(&content, mount_point),
        Err(_) => false,
    }
}

/// `true` while a mount this process started is still alive. A child that
/// exited behind our back is reaped here, exactly like the downloader's
/// `aria2_is_running`, so a dead mount is never reported as active.
async fn mount_is_tracked(mount_point: &str) -> bool {
    let mut mounts = MOUNTS.lock().await;
    let Some(entry) = mounts.get_mut(mount_point) else {
        return false;
    };
    match entry.child.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            tracing::warn!(
                "rclone mount of '{}' at {} exited unexpectedly ({})",
                entry.remote,
                mount_point,
                status
            );
            mounts.remove(mount_point);
            false
        }
        Err(e) => {
            tracing::warn!("could not poll the rclone mount at {}: {}", mount_point, e);
            mounts.remove(mount_point);
            false
        }
    }
}

/// Kill the child this process owns for `mount_point` and forget it.
async fn release_mount(mount_point: &str) {
    let entry = MOUNTS.lock().await.remove(mount_point);
    if let Some(mut entry) = entry {
        if let Ok(Some(_)) = entry.child.try_wait() {
            return;
        }
        let _ = entry.child.kill().await;
        let _ = entry.child.wait().await;
    }
}

/// Unmount `mount_point`, trying `fusermount -u` first and `umount -f` after.
///
/// Both calls are bounded and the failure names both reasons: "fusermount is
/// not installed" and "the server is wedged" are different problems, and the
/// old code reported only the second one's absence.
async fn unmount_checked(mount_point: &str) -> Result<(), String> {
    let fusermount = run_timed_async_result(
        Command::new("fusermount").arg("-u").arg(mount_point),
        RCLONE_UMOUNT_TIMEOUT,
    )
    .await;
    match &fusermount {
        Ok(output) if output.status.success() => return Ok(()),
        _ => {}
    }

    let umount = run_timed_async_result(
        Command::new("umount").arg("-f").arg(mount_point),
        RCLONE_UMOUNT_TIMEOUT,
    )
    .await;
    match &umount {
        Ok(output) if output.status.success() => return Ok(()),
        _ => {}
    }

    Err(format!(
        "Failed to unmount {}: fusermount {}; umount {}",
        mount_point,
        describe_attempt(&fusermount),
        describe_attempt(&umount)
    ))
}

/// One line describing what one unmount attempt did, for the error above.
fn describe_attempt(
    attempt: &Result<std::process::Output, crate::utils::process::ProcessFailure>,
) -> String {
    match attempt {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr.trim();
            if reason.is_empty() {
                format!("exited with {}", output.status)
            } else {
                format!("refused: {}", reason)
            }
        }
        Err(failure) => failure.to_string(),
    }
}

/// Unmount every mount this process started, then kill its child.
///
/// Called from the shutdown hook (`src/web/mod.rs`) so a restart does not
/// leave stale FUSE mounts and orphaned `rclone` processes behind —
/// the state the old fire-and-forget `spawn()` guaranteed (ROADMAP 16.2).
pub async fn shutdown_mounts() {
    let mount_points: Vec<String> = {
        let mounts = MOUNTS.lock().await;
        mounts.keys().cloned().collect()
    };
    if mount_points.is_empty() {
        return;
    }

    tracing::info!(
        "Unmounting {} rclone mount(s) before shutdown",
        mount_points.len()
    );
    for mount_point in mount_points {
        if let Err(e) = unmount_checked(&mount_point).await {
            tracing::warn!("{}", e);
        }
        release_mount(&mount_point).await;
    }
}

/// Message for a mount that never appeared in the kernel's mount table.
fn mount_failure_message(
    mount_point: &str,
    status: Option<std::process::ExitStatus>,
    log_tail: &Arc<Mutex<String>>,
) -> String {
    let base = match status {
        Some(status) => format!(
            "rclone exited before {} became a mount point ({})",
            mount_point, status
        ),
        None => format!(
            "rclone did not mount {} within {} seconds",
            mount_point,
            RCLONE_MOUNT_READY_TIMEOUT.as_secs()
        ),
    };
    let tail = log_tail
        .lock()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if tail.is_empty() {
        base
    } else {
        format!("{}: {}", base, tail)
    }
}

/// Poll until `mount_point` is in the kernel's mount table, the child dies, or
/// the deadline passes. A mount that "started" but is not in
/// `/proc/self/mounts` is a failure, which is exactly what the old
/// `spawn()`-and-report-success path could not tell.
async fn wait_until_mounted(
    mount_point: &str,
    child: &mut Child,
    log_tail: &Arc<Mutex<String>>,
) -> Result<(), String> {
    let deadline = Instant::now() + RCLONE_MOUNT_READY_TIMEOUT;
    loop {
        if mount_is_active(mount_point).await {
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(mount_failure_message(mount_point, Some(status), log_tail))
            }
            Ok(None) => {}
            Err(e) => return Err(format!("Could not poll the rclone mount: {}", e)),
        }
        if Instant::now() >= deadline {
            return Err(mount_failure_message(mount_point, None, log_tail));
        }
        tokio::time::sleep(MOUNT_POLL_INTERVAL).await;
    }
}

/// Remote information from configured remotes
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Remote {
    pub name: String,
    #[serde(rename = "type")]
    pub remote_type: String,
}

/// Provider tier information
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub description: String,
    pub tier: String,
}

/// Mount information
#[derive(Serialize, Debug, Clone)]
pub struct Mount {
    pub remote: String,
    pub mount_point: String,
    pub path: Option<String>,
    pub is_active: bool,
    pub pid: Option<u32>,
}

/// `GET /rclone/status` payload.
///
/// `mount_base` is included so the screen derives a mount point from the
/// server's own rule instead of hardcoding `/mnt/rclone/<name>` client-side.
#[derive(Serialize, Debug, Clone)]
pub struct RcloneStatus {
    pub available: bool,
    pub fusermount: bool,
    pub mount_base: String,
}

/// Request for sync operation
#[derive(Deserialize)]
pub struct SyncRequest {
    pub source_type: Option<String>, // "cloud" or "local"
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: Option<String>, // "cloud" or "local"
    pub destination: String,
    pub dest_path: Option<String>,
    pub delete: bool,
    pub dry_run: bool,
    pub update: bool,
    pub exclude: Option<String>,
}

/// Request for copy operation
#[derive(Deserialize)]
pub struct CopyRequest {
    pub source_type: Option<String>, // "cloud" or "local"
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: Option<String>, // "cloud" or "local"
    pub destination: String,
    pub dest_path: Option<String>,
    pub delete: bool,
    pub dry_run: bool,
    pub interactive: bool,
}

/// Request for mount operation. `read_only`/`allow_other`/`daemon` default
/// to `false` when omitted — a real bug found during release verification
/// had these as required fields with no default, but the frontend
/// (`api.startMount()`) only ever sends `{ remote, mount_point, path }`, so
/// every real mount request failed JSON deserialization before the handler
/// even ran. `daemon` is kept for API compatibility but changes nothing:
/// this process always owns the mount child itself (no `--daemon`), which is
/// what lets it report a startup failure and unmount on shutdown
/// (ROADMAP 16.2/16.3).
#[derive(Deserialize)]
pub struct MountRequest {
    pub remote: String,
    pub mount_point: String,
    pub path: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub allow_other: bool,
    #[serde(default)]
    pub daemon: bool,
}

/// Request for unmounting. Only the mount point matters — a dedicated body
/// because `MountRequest` requires `remote`, and `api.unmountCloudRemote()`
/// sends `{ mount_point }` alone: every real "Stop mount" click used to be
/// rejected by the JSON extractor (a `422`) before the handler ran.
#[derive(Deserialize)]
pub struct MountStopRequest {
    pub mount_point: String,
}

/// Build a source or destination path for rclone
/// For cloud: "remote:path" or "remote:"
/// For local: "/path" or "/path/subpath"
fn build_rclone_path(path_type: &str, remote: &str, sub_path: &Option<String>) -> String {
    let clean_subpath = sub_path
        .as_ref()
        .map(|p| p.trim_start_matches('/'))
        .unwrap_or("");

    if path_type == "local" {
        // Local path: /path or /path/subpath
        if clean_subpath.is_empty() {
            remote.to_string()
        } else {
            format!("{}/{}", remote.trim_end_matches('/'), clean_subpath)
        }
    } else {
        // Cloud path: remote:path or remote:
        if clean_subpath.is_empty() {
            format!("{}:", remote)
        } else {
            format!("{}:{}", remote, clean_subpath)
        }
    }
}

/// Canonical config location when nothing else exists — also what a fresh
/// install writes.
const DEFAULT_RCLONE_CONFIG: &str = "/etc/rclone.conf";

/// The locations rclone itself falls back to, in order, when `RCLONE_CONFIG`
/// is not set.
fn rclone_config_fallback_paths() -> Vec<std::path::PathBuf> {
    let mut paths = vec![std::path::PathBuf::from(DEFAULT_RCLONE_CONFIG)];
    paths.push(std::path::PathBuf::from("/root/.config/rclone/rclone.conf"));
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            paths.push(std::path::PathBuf::from(home).join(".config/rclone/rclone.conf"));
        }
    }
    paths
}

/// The **one** config file this module reads *and* writes.
///
/// Every handler used to hardcode `/etc/rclone.conf` while the reader honoured
/// the `RCLONE_CONFIG` override — so the Settings editor could read and write a
/// different file from the one the
/// remote list was parsed from, and an admin who moved the config (the
/// supported way to run a second, non-root rclone) saw an empty list next to
/// editor contents that did not match it. Both sides now resolve through this
/// single function, so they cannot drift apart again.
///
/// Order: an explicit non-empty `RCLONE_CONFIG` wins outright — existing or
/// not, since that is how a fresh config gets created outside `/etc` — then
/// the first of rclone's fallback locations that exists, then
/// [`DEFAULT_RCLONE_CONFIG`].
pub fn rclone_config_path() -> std::path::PathBuf {
    if let Ok(overridden) = std::env::var("RCLONE_CONFIG") {
        if !overridden.is_empty() {
            return std::path::PathBuf::from(overridden);
        }
    }

    rclone_config_fallback_paths()
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_RCLONE_CONFIG))
}

/// Constructs a [`tokio::process::Command`] preconfigured with `rclone` and
/// `--config <rclone_config_path()>`.
///
/// Subprocess invocations that read or write remotes must use this helper so
/// that the CLI subprocess uses the exact same config file that the backend
/// reads and writes (honouring `RCLONE_CONFIG` overrides and the
/// `/etc/rclone.conf` system default). `$HOME/.config/rclone/rclone.conf` was
/// the fallback rclone itself picked, so a remote the backend had just written
/// to `/etc/rclone.conf` was invisible to `rclone mount`/`sync`/`copy`.
pub fn rclone_command() -> Command {
    let mut cmd = Command::new("rclone");
    cmd.arg("--config").arg(rclone_config_path());
    cmd
}

/// Parse rclone config file to get remotes with their types
fn read_rclone_config(path: &FsPath) -> Option<String> {
    // One resolver, shared with every handler that writes the file (see
    // `rclone_config_path`). Previously this looped over the candidates on its
    // own, which is exactly how it ended up disagreeing with them.
    //
    // Blocking `std::fs` on purpose, and only ever reached from
    // `spawn_blocking` (see `load_remotes`): reading a config file on the
    // async runtime is what ROADMAP X.1 is about.
    std::fs::read_to_string(path).ok()
}

/// Get human-readable description for provider type
fn get_provider_description(provider_type: &str) -> String {
    match provider_type.to_lowercase().as_str() {
        // Tier 1 - Major Cloud Providers
        "drive" => "Google Drive".to_string(),
        "dropbox" => "Dropbox".to_string(),
        "onedrive" => "Microsoft OneDrive".to_string(),
        "box" => "Box".to_string(),
        "pcloud" => "pCloud".to_string(),
        "yandex" => "Yandex Disk".to_string(),
        "filen" => "Filen".to_string(),
        "hidrive" => "HiDrive".to_string(),
        "mailru" => "Mail.ru Cloud".to_string(),
        "opendrive" => "Opendrive".to_string(),
        "pixeldrain" => "Pixeldrain".to_string(),
        "pikpak" => "PikPak".to_string(),
        "gofile" => "Gofile".to_string(),
        "netstorage" => "Akamai NetStorage".to_string(),
        "memory" => "Memory".to_string(),
        "drime" => "Drime".to_string(),
        "filescom" => "Files.com".to_string(),
        "imagekit" => "ImageKit".to_string(),
        "shade" => "Shade".to_string(),
        // Tier 1 - Object Storage
        "s3" => "Amazon S3".to_string(),
        "b2" | "backblaze" => "Backblaze B2".to_string(),
        "azureblob" => "Azure Blob Storage".to_string(),
        "googlecloudstorage" => "Google Cloud Storage".to_string(),
        "oracleobjectstorage" => "Oracle Object Storage".to_string(),
        "storj" => "Storj".to_string(),
        "swift" => "OpenStack Swift".to_string(),
        // Tier 1 - Protocols
        "ftp" => "FTP Server".to_string(),
        "sftp" => "SFTP / SSH".to_string(),
        "webdav" => "WebDAV".to_string(),
        "alias" => "Alias".to_string(),
        "combine" => "Combine".to_string(),
        "union" => "Union".to_string(),
        "file" => "Local".to_string(),
        // Tier 1 - Crypt
        "crypt" => "Encrypted (Crypt)".to_string(),
        // Tier 2 - Stable
        "mega" => "MEGA".to_string(),
        "jottacloud" => "Jottacloud".to_string(),
        "koofr" => "Koofr".to_string(),
        "putio" => "Put.io".to_string(),
        "internxt" => "Internxt".to_string(),
        "azurefiles" => "Azure Files".to_string(),
        "wasabi" => "Wasabi".to_string(),
        "digitalocean" => "DigitalOcean Spaces".to_string(),
        "smb" | "cifs" => "SMB/CIFS".to_string(),
        "hdfs" => "HDFS".to_string(),
        "doi" => "DOI".to_string(),
        // Tier 3 - Supported
        "iclouddrive" => "iCloud Drive".to_string(),
        "premiumizeme" => "Premiumize.me".to_string(),
        "huaweidrive" => "Huawei Cloud".to_string(),
        "quatrix" => "Quatrix".to_string(),
        "ulozto" => "Uloz.to".to_string(),
        "fichier" => "Fichier".to_string(),
        "qingstor" => "QingStor".to_string(),
        "http" => "HTTP Server".to_string(),
        "seafile" => "Seafile".to_string(),
        "internetarchive" => "Internet Archive".to_string(),
        "sugarsync" => "SugarSync".to_string(),
        "zoho" => "Zoho".to_string(),
        "cloudinary" => "Cloudinary".to_string(),
        // Tier 4 - Experimental
        "nfs" => "NFS".to_string(),
        "chunker" => "Chunker".to_string(),
        "hasher" => "Hasher".to_string(),
        "compress" => "Compress".to_string(),
        "filefabric" => "FileFabric".to_string(),
        "sia" => "Sia".to_string(),
        "protondrive" => "Proton Drive".to_string(),
        "telegram" => "Telegram".to_string(),
        "hubic" => "HubiC".to_string(),
        // Tier 5 - Deprecated
        "gphotos" => "Google Photos (deprecated)".to_string(),
        "cache" => "Cache (deprecated)".to_string(),
        "sharefile" => "Sharefile (deprecated)".to_string(),
        "linkbox" => "Linkbox (deprecated)".to_string(),
        _ => format!("{} (custom)", provider_type),
    }
}

/// List all configured remotes
pub async fn handle_list_remotes(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Remote>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Check if rclone is available. Bounded: the old bare `.output()` hung
    // here forever when `rclone` was replaced by something slow (a wrapper
    // script waiting on a network mount), so the remotes tab never loaded and
    // gave no reason (ROADMAP 16.1).
    if run_timed_async_result(Command::new("rclone").arg("version"), RCLONE_PROBE_TIMEOUT)
        .await
        .is_err()
    {
        return Json(ApiResponse::error(
            "rclone is not installed on this system (or did not answer in time)",
        ));
    }

    // Try to parse config file directly for better type detection
    if let Some(remotes) = load_remotes().await {
        return Json(ApiResponse::success(remotes));
    }

    // Fallback: use rclone listremotes command
    let output =
        run_timed_async_result(rclone_command().args(["listremotes"]), RCLONE_PROBE_TIMEOUT).await;

    match output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Json(ApiResponse::error(&format!(
                    "Failed to list remotes: {}",
                    stderr
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut remotes = Vec::new();

            for line in stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                // Parse output like: gdrive:
                if let Some(name) = line.strip_suffix(':') {
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        remotes.push(Remote {
                            name,
                            remote_type: "Remote".to_string(),
                        });
                    }
                }
            }

            Json(ApiResponse::success(remotes))
        }
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to list remotes: {}",
            e
        ))),
    }
}

/// List available provider types for adding new remotes
/// Based on rclone tiers: https://rclone.org/tiers/
pub async fn handle_list_providers(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Provider>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Tier 1: Core (Production-grade, first-class)
    // Tier 2: Stable (Well-supported, minor gaps)
    // Tier 3: Supported (Works for many uses; known caveats)
    // Tier 4: Experimental (Use with care; expect gaps/changes)
    let providers = vec![
        // Tier 1 - Cloud Storage
        Provider {
            name: "drive".to_string(),
            description: "Google Drive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "onedrive".to_string(),
            description: "Microsoft OneDrive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "dropbox".to_string(),
            description: "Dropbox".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "box".to_string(),
            description: "Box".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "pcloud".to_string(),
            description: "pCloud".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "mega".to_string(),
            description: "MEGA".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "yandex".to_string(),
            description: "Yandex Disk".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "iclouddrive".to_string(),
            description: "iCloud Drive".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "gphotos".to_string(),
            description: "Google Photos".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "filen".to_string(),
            description: "Filen".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "hidrive".to_string(),
            description: "HiDrive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "mailru".to_string(),
            description: "Mail.ru Cloud".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "opendrive".to_string(),
            description: "Opendrive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "pixeldrain".to_string(),
            description: "Pixeldrain".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "pikpak".to_string(),
            description: "PikPak".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "gofile".to_string(),
            description: "Gofile".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "jottacloud".to_string(),
            description: "Jottacloud".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "premiumizeme".to_string(),
            description: "Premiumize.me".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "koofr".to_string(),
            description: "Koofr".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "putio".to_string(),
            description: "Put.io".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "internxt".to_string(),
            description: "Internxt".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "huaweidrive".to_string(),
            description: "Huawei Cloud".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "quatrix".to_string(),
            description: "Quatrix".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "ulozto".to_string(),
            description: "Uloz.to".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "fichier".to_string(),
            description: "Fichier".to_string(),
            tier: "Tier 3".to_string(),
        },
        // Tier 1 - Object Storage / S3-compatible
        Provider {
            name: "s3".to_string(),
            description: "Amazon S3".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "b2".to_string(),
            description: "Backblaze B2".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "azureblob".to_string(),
            description: "Azure Blob Storage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "azurefiles".to_string(),
            description: "Azure Files".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "googlecloudstorage".to_string(),
            description: "Google Cloud Storage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "oracleobjectstorage".to_string(),
            description: "Oracle Object Storage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "wasabi".to_string(),
            description: "Wasabi".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "digitalocean".to_string(),
            description: "DigitalOcean Spaces".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "storj".to_string(),
            description: "Storj".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "qingstor".to_string(),
            description: "QingStor".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "swift".to_string(),
            description: "OpenStack Swift".to_string(),
            tier: "Tier 1".to_string(),
        },
        // Tier 1 - Protocol-based
        Provider {
            name: "ftp".to_string(),
            description: "FTP Server".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "sftp".to_string(),
            description: "SFTP / SSH".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "webdav".to_string(),
            description: "WebDAV".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "http".to_string(),
            description: "HTTP Server".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "smb".to_string(),
            description: "SMB/CIFS".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "nfs".to_string(),
            description: "NFS".to_string(),
            tier: "Tier 4".to_string(),
        },
        // Tier 1 - Local & Virtual
        Provider {
            name: "file".to_string(),
            description: "Local".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "alias".to_string(),
            description: "Alias".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "crypt".to_string(),
            description: "Encrypted (Crypt)".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "cache".to_string(),
            description: "Cache".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "combine".to_string(),
            description: "Combine".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "union".to_string(),
            description: "Union".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "chunker".to_string(),
            description: "Chunker".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "hasher".to_string(),
            description: "Hasher".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "compress".to_string(),
            description: "Compress".to_string(),
            tier: "Tier 4".to_string(),
        },
        // Tier 3 - Other services
        Provider {
            name: "seafile".to_string(),
            description: "Seafile".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "internetarchive".to_string(),
            description: "Internet Archive".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "sugarsync".to_string(),
            description: "SugarSync".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "hubic".to_string(),
            description: "HubiC".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "zoho".to_string(),
            description: "Zoho".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "cloudinary".to_string(),
            description: "Cloudinary".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "sharefile".to_string(),
            description: "Sharefile".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "linkbox".to_string(),
            description: "Linkbox".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "netstorage".to_string(),
            description: "Akamai NetStorage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "memory".to_string(),
            description: "Memory".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "hdfs".to_string(),
            description: "HDFS".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "drime".to_string(),
            description: "Drime".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "filescom".to_string(),
            description: "Files.com".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "doi".to_string(),
            description: "DOI".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "imagekit".to_string(),
            description: "ImageKit".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "shade".to_string(),
            description: "Shade".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "sia".to_string(),
            description: "Sia".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "filefabric".to_string(),
            description: "FileFabric".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "protondrive".to_string(),
            description: "Proton Drive".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "telegram".to_string(),
            description: "Telegram".to_string(),
            tier: "Tier 4".to_string(),
        },
    ];

    Json(ApiResponse::success(providers))
}

/// List active mounts.
///
/// The kernel's mount table is the source of truth for *what is mounted*; the
/// process registry only contributes what the kernel cannot know — the remote
/// a mount came from and the pid of the child we own, so the UI can offer a
/// real "Stop".
pub async fn handle_list_mounts(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Mount>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Snapshot the mounts this process owns (and reap the dead ones).
    let tracked: HashMap<String, (String, Option<u32>)> = {
        let mut registry = MOUNTS.lock().await;
        let points: Vec<String> = registry.keys().cloned().collect();
        let mut owned = HashMap::new();
        for point in points {
            let alive = registry
                .get_mut(&point)
                .and_then(|entry| match entry.child.try_wait() {
                    Ok(None) => Some((entry.remote.clone(), entry.child.id())),
                    Ok(Some(status)) => {
                        tracing::warn!(
                            "rclone mount of '{}' at {} exited ({})",
                            entry.remote,
                            point,
                            status
                        );
                        None
                    }
                    Err(_) => None,
                });
            match alive {
                Some(value) => {
                    owned.insert(point, value);
                }
                None => {
                    registry.remove(&point);
                }
            }
        }
        owned
    };

    let content = match tokio::fs::read_to_string("/proc/mounts").await {
        Ok(content) => content,
        // A process that cannot read its own mount table has nothing truthful
        // to report; the old code did the same and returned an empty list.
        Err(_) => return Json(ApiResponse::success(Vec::new())),
    };

    let mut mounts = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for line in content.lines() {
        let mut fields = line.split_whitespace();
        let _device = fields.next();
        let Some(mount_point_field) = fields.next() else {
            continue;
        };
        let Some(fs_type) = fields.next() else {
            continue;
        };

        let mount_point = decode_mount_escape(mount_point_field);
        let is_rclone = fs_type == "fuse.rclone"
            || fs_type == "rclone"
            || mount_point.starts_with(crate::config::paths::RCLONE_MOUNT_BASE)
            || mount_point.starts_with("/run/rclone");
        if !is_rclone {
            continue;
        }

        let (remote, pid) = match tracked.get(&mount_point) {
            Some((remote, pid)) => (remote.clone(), *pid),
            None => (
                mount_point
                    .split('/')
                    .next_back()
                    .unwrap_or("unknown")
                    .to_string(),
                None,
            ),
        };

        seen.push(mount_point.clone());
        mounts.push(Mount {
            remote,
            mount_point,
            path: None,
            is_active: true,
            pid,
        });
    }

    // A child we own whose mount is gone is a broken mount, not a healthy one:
    // report it inactive so the screen offers "Start" (or "Stop", which kills
    // the child) instead of a mount that silently reads an empty directory.
    for (mount_point, (remote, pid)) in tracked {
        if seen.contains(&mount_point) {
            continue;
        }
        mounts.push(Mount {
            remote,
            mount_point,
            path: None,
            is_active: false,
            pid,
        });
    }

    mounts.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    Json(ApiResponse::success(mounts))
}

/// Start a mount and report success only once the kernel agrees.
///
/// The old version appended `--daemon`, called `spawn()` and dropped the
/// handle: rclone forked, the child exited, and every outcome — a bad token,
/// a missing fuse device, a mount that never appeared — looked identical to
/// the caller (ROADMAP 16.2/16.3). Here the child is ours, `--daemon` is
/// gone, and the call returns only after the mount point shows up in
/// `/proc/self/mounts` or the child dies (see `wait_until_mounted`).
pub async fn handle_mount_start(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<MountRequest>,
) -> Json<ApiResponse<Mount>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // Validate every value before it reaches an argv or a filesystem call.
    let mount_point = match validate_mount_point(&req.mount_point) {
        Ok(point) => point,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let remote = match validate_remote_name(&req.remote) {
        Ok(remote) => remote,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let sub_path = match validate_remote_subpath(&req.path) {
        Ok(path) => path,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // The remote must exist in the config the server itself resolved: `mount`
    // with an unknown name would still spawn rclone and fail asynchronously,
    // and the argv would be built from a name nobody validated against the
    // file that holds the credentials.
    let configured = load_remotes()
        .await
        .map(|remotes| remotes.iter().any(|candidate| candidate.name == remote))
        .unwrap_or(false);
    if !configured {
        return Json(ApiResponse::error(&format!(
            "Remote '{}' is not configured in the rclone config file",
            remote
        )));
    }

    // One mount per directory: a second rclone on the same mount point would
    // stack two FUSE servers and neither could be stopped cleanly.
    if mount_is_tracked(&mount_point).await || mount_is_active(&mount_point).await {
        return Json(ApiResponse::error(&format!(
            "Something is already mounted at {} — unmount it first",
            mount_point
        )));
    }

    // Create the mount point directory before spawning rclone. Unlike other
    // mount types, rclone's FUSE mount can create the dir itself, but we create
    // it explicitly for consistency with RAID/pools/encrypted. Off the async
    // runtime: this is a privileged call on a possibly slow filesystem.
    let directory = mount_point.clone();
    match tokio::task::spawn_blocking(move || std::fs::create_dir_all(&directory)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            return Json(ApiResponse::error(&format!(
                "Failed to create mount point: {}",
                e
            )))
        }
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to create mount point: {}",
                e
            )))
        }
    }

    // Build `rclone mount remote:sub/path mount_point …`.
    let source = match &sub_path {
        Some(path) => format!("{}:{}", remote, path),
        None => format!("{}:", remote),
    };

    let mut command = rclone_command();
    command
        .arg("mount")
        .arg(&source)
        .arg(&mount_point)
        .arg("--vfs-cache-mode")
        .arg("writes")
        .arg("--log-level")
        .arg("ERROR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        // We own the child from here on: it must not be killed by dropping a
        // future, only by `release_mount`/`shutdown_mounts`.
        .kill_on_drop(false);

    if req.read_only {
        command.arg("--read-only");
    }
    if req.allow_other {
        command.arg("--allow-other");
    }
    // `req.daemon` is deliberately ignored: a forked rclone is a mount nobody
    // can report on, wait for, or unmount.

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Json(ApiResponse::error(&format!("Failed to start mount: {}", e))),
    };
    let log_tail = capture_stderr_tail(&mut child);

    let pid = match wait_until_mounted(&mount_point, &mut child, &log_tail).await {
        Ok(()) => child.id(),
        Err(e) => {
            // Never leave a half-started mount behind on the failure path.
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Json(ApiResponse::error(&e));
        }
    };

    MOUNTS.lock().await.insert(
        mount_point.clone(),
        MountedRemote {
            child,
            remote: remote.clone(),
            mount_point: mount_point.clone(),
            log_tail,
        },
    );

    let mount = Mount {
        remote: remote.clone(),
        mount_point: mount_point.clone(),
        path: sub_path,
        is_active: true,
        pid,
    };
    crate::events::log_event(format!(
        "Admin '{}' mounted cloud remote '{}' at '{}'",
        admin_username, remote, mount_point
    ));
    Json(ApiResponse::success(mount))
}

/// Keep the last few KiB of a child's stderr for a startup-failure message.
///
/// rclone's `.output()` is not usable for a mount (it never exits), so its
/// stderr is drained by a task instead — a full pipe would otherwise block the
/// child. The buffer is bounded and only the tail is kept: the head of a mount
/// log is always the same banner.
fn capture_stderr_tail(child: &mut Child) -> Arc<Mutex<String>> {
    let log_tail = Arc::new(Mutex::new(String::new()));
    let Some(mut stderr) = child.stderr.take() else {
        return log_tail;
    };
    let sink = Arc::clone(&log_tail);
    tokio::spawn(async move {
        let mut buffer = [0u8; 1024];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut text) = sink.lock() {
                        text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                        if text.len() > STDERR_TAIL_BYTES {
                            let excess = text.len() - STDERR_TAIL_BYTES;
                            let cut = text
                                .char_indices()
                                .map(|(index, _)| index)
                                .find(|index| *index >= excess)
                                .unwrap_or(0);
                            text.drain(..cut);
                        }
                    }
                }
            }
        }
    });
    log_tail
}

/// Stop a mount: `fusermount -u`, then `umount -f`, then kill our child.
///
/// Takes a [`MountStopRequest`] because the UI sends `{ mount_point }` alone —
/// with the old `MountRequest` the JSON extractor rejected every real unmount
/// before this code ran.
pub async fn handle_mount_stop(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<MountStopRequest>,
) -> Json<ApiResponse<bool>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let mount_point = match validate_mount_point(&req.mount_point) {
        Ok(point) => point,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    if let Err(e) = unmount_checked(&mount_point).await {
        return Json(ApiResponse::error(&e));
    }

    // The mount is gone; make sure the process we own is gone too, otherwise a
    // live rclone keeps retrying against an unmounted FUSE path.
    release_mount(&mount_point).await;

    crate::events::log_event(format!(
        "Admin '{}' unmounted cloud mount at '{}'",
        admin_username, mount_point
    ));
    Json(ApiResponse::success(true))
}

/// Run sync operation
pub async fn handle_sync(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<SyncRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    execute_sync(&admin_username, &req).await
}

/// What one `rclone sync` run actually did.
///
/// Separate from the HTTP envelope so the **scheduler** can run the same work
/// and record its outcome without an `axum::response::Response` — ROADMAP 16.4
/// requires the transfer to leave the request, so the core cannot be shaped
/// around a reply.
#[derive(Debug)]
struct SyncOutcome {
    ok: bool,
    /// stdout on success, the stderr tail or the transport error on failure.
    message: String,
}

impl SyncOutcome {
    fn ok(stdout: &str) -> Self {
        Self {
            ok: true,
            message: if stdout.trim().is_empty() {
                "Sync complete".to_string()
            } else {
                stdout.trim().to_string()
            },
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
        }
    }
}

/// Shared core of [`handle_sync`], [`handle_run_sync_job`] and the scheduler:
/// validates the pair of paths and runs the `rclone sync` subprocess.
async fn run_sync_core(admin_username: &str, req: &SyncRequest) -> SyncOutcome {
    let mut args = vec!["sync".to_string()];

    // Determine path types
    let source_type = req.source_type.as_deref().unwrap_or("cloud");
    let dest_type = req.dest_type.as_deref().unwrap_or("cloud");

    // Build source and destination paths
    let source = build_rclone_path(source_type, &req.source, &req.source_path);
    let dest = build_rclone_path(dest_type, &req.destination, &req.dest_path);

    // Validate
    if source.is_empty() || source == ":" {
        return SyncOutcome::failed("Source is required");
    }
    if dest.is_empty() || dest == ":" {
        return SyncOutcome::failed("Destination is required");
    }

    args.push(source);
    args.push(dest);

    if req.delete {
        args.push("--delete".to_string());
    }

    if req.dry_run {
        args.push("--dry-run".to_string());
    }

    if req.update {
        args.push("--update".to_string());
    }

    if let Some(exclude) = &req.exclude {
        if !exclude.is_empty() {
            args.push("--exclude".to_string());
            args.push(exclude.clone());
        }
    }

    // Add progress output
    args.push("--progress".to_string());

    // Show the transfer in the desktop activity taskbar (indeterminate — the
    // blocking call gives no incremental stats). The deadline is what closes
    // the job if the remote stalls (ROADMAP 16.1): before it, a stuck `rclone
    // sync` kept the HTTP request *and* this job open forever.
    let job = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("cloud_sync".to_string()),
        &format!("Sync {} -> {}", req.source, req.destination),
    );

    let output = run_timed_async_result(rclone_command().args(&args), RCLONE_TRANSFER_TIMEOUT).await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                crate::sse::PROGRESS_MANAGER.complete_job(&job, "Sync complete");
                crate::events::log_event(format!(
                    "Admin '{}' ran a sync from '{}' to '{}'",
                    admin_username, req.source, req.destination
                ));
                SyncOutcome::ok(&stdout)
            } else {
                crate::sse::PROGRESS_MANAGER.fail_job(&job, &stderr);
                SyncOutcome::failed(format!("Sync failed: {}", stderr.trim()))
            }
        }
        Err(e) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job, &e.to_string());
            crate::events::log_event(format!(
                "Admin '{}' — sync from '{}' to '{}' aborted: {}",
                admin_username, req.source, req.destination, e
            ));
            SyncOutcome::failed(format!("Sync failed: {e}"))
        }
    }
}

/// The `SyncRequest` a stored job executes.
fn request_for(job: &SyncJob) -> SyncRequest {
    SyncRequest {
        source_type: Some(job.source_type.clone()),
        source: job.source.clone(),
        source_path: job.source_path.clone(),
        dest_type: Some(job.dest_type.clone()),
        destination: job.destination.clone(),
        dest_path: job.dest_path.clone(),
        delete: false,
        dry_run: false,
        update: true,
        exclude: None,
    }
}

/// [`execute_sync`]'s HTTP shell: the shared core plus the JSON envelope.
async fn execute_sync(admin_username: &str, req: &SyncRequest) -> Json<ApiResponse<String>> {
    let outcome = run_sync_core(admin_username, req).await;
    if outcome.ok {
        Json(ApiResponse::success(outcome.message))
    } else {
        Json(ApiResponse::error(&outcome.message))
    }
}

// ---------------------------------------------------------------------------
// Sync jobs (persisted server-side; the frontend stopped using localStorage)
// ---------------------------------------------------------------------------

/// Body for creating a sync job.
#[derive(Deserialize)]
pub struct CreateSyncJobRequest {
    pub name: String,
    #[serde(default = "default_cloud")]
    pub source_type: String,
    pub source: String,
    pub source_path: Option<String>,
    #[serde(default = "default_cloud")]
    pub dest_type: String,
    pub destination: String,
    pub dest_path: Option<String>,
    /// `none` (default — a job that only runs on demand), `interval` or `cron`.
    /// ROADMAP 16.4: the server needs a cadence to schedule anything, and a
    /// nightly sync is the whole reason this exists.
    #[serde(default)]
    pub schedule_kind: Option<String>,
    /// Cadence in minutes, when `schedule_kind = "interval"`.
    #[serde(default)]
    pub interval_minutes: Option<i64>,
    /// 5-field cron, when `schedule_kind = "cron"` (evaluated in the appliance
    /// timezone, exactly like a Scheduled Task).
    #[serde(default)]
    pub cron_expr: Option<String>,
}

/// Body for editing a job's cadence.
#[derive(Deserialize)]
pub struct UpdateSyncJobScheduleRequest {
    #[serde(default)]
    pub schedule_kind: Option<String>,
    #[serde(default)]
    pub interval_minutes: Option<i64>,
    #[serde(default)]
    pub cron_expr: Option<String>,
}

fn default_cloud() -> String {
    "cloud".to_string()
}

/// Response body for listing jobs. Mirrors the frontend's `SyncJob` shape so
/// the UI can render `id`, `source`, `destination` and a derived `direction`
/// without a second mapping layer.
#[derive(Serialize)]
pub struct SyncJobView {
    pub id: i64,
    pub name: String,
    pub source: String,
    pub destination: String,
    pub direction: String,
    pub created_at: String,
    /// `none` | `interval` | `cron`.
    pub schedule_kind: String,
    pub interval_minutes: Option<i64>,
    pub cron_expr: Option<String>,
    /// Server-written run state (ROADMAP 16.4): the screen renders these
    /// instead of remembering the response to its own click.
    pub last_run_at: Option<String>,
    /// `ok` | `failed` | `None` (never run).
    pub last_status: Option<String>,
    pub last_message: Option<String>,
    /// The next time the scheduler will run this job.
    pub next_run_at: Option<String>,
}

impl From<SyncJob> for SyncJobView {
    fn from(job: SyncJob) -> Self {
        let direction = if job.source_type == "local" {
            "local-to-cloud"
        } else {
            "cloud-to-local"
        };
        Self {
            id: job.id,
            name: job.name,
            source: job.source,
            destination: job.destination,
            direction: direction.to_string(),
            created_at: job.created_at,
            schedule_kind: ScheduleKind::parse(&job.schedule_kind).as_str().to_string(),
            interval_minutes: job.interval_minutes,
            cron_expr: job.cron_expr,
            last_run_at: job.last_run_at,
            last_status: job.last_status,
            last_message: job.last_message,
            next_run_at: job.next_run_at,
        }
    }
}

/// List saved sync jobs (any authenticated session).
pub async fn handle_list_sync_jobs(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<SyncJobView>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    let jobs = match repo.list_all().await {
        Ok(jobs) => jobs,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to list sync jobs: {}",
                e
            )))
        }
    };
    let views: Vec<SyncJobView> = jobs.into_iter().map(SyncJobView::from).collect();
    Json(ApiResponse::success(views))
}

/// Save a new sync job (admin).
pub async fn handle_create_sync_job(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<CreateSyncJobRequest>,
) -> Json<ApiResponse<SyncJobView>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let schedule_kind = ScheduleKind::parse(req.schedule_kind.as_deref().unwrap_or("none"));
    let job = NewSyncJob {
        name: req.name,
        source_type: req.source_type,
        source: req.source,
        source_path: req.source_path,
        dest_type: req.dest_type,
        destination: req.destination,
        dest_path: req.dest_path,
        schedule_kind,
        interval_minutes: req.interval_minutes,
        cron_expr: req.cron_expr,
    };
    if let Err(msg) = job.validate() {
        return Json(ApiResponse::error(&msg));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    let id = match repo.insert(&job).await {
        Ok(id) => id,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to save sync job: {}",
                e
            )))
        }
    };
    let created = match repo.get_by_id(id).await {
        Ok(Some(job)) => job,
        Ok(None) => {
            return Json(ApiResponse::error("Sync job was not persisted"));
        }
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to load sync job: {}",
                e
            )))
        }
    };
    Json(ApiResponse::success(SyncJobView::from(created)))
}

/// Delete a saved sync job (admin).
pub async fn handle_delete_sync_job(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Json<ApiResponse<bool>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    match repo.delete(id).await {
        Ok(true) => Json(ApiResponse::success(true)),
        Ok(false) => Json(ApiResponse::error("Sync job not found")),
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to delete sync job: {}",
            e
        ))),
    }
}

/// Runs one persisted job end to end and records its outcome.
///
/// Shared by the "Run now" button and the scheduler so the two cannot disagree
/// about what a run *is*: the same request, the same core, the same recorded
/// state (ROADMAP 16.4). `trigger` only labels the event log.
async fn run_saved_job(
    state: &AppStateArc,
    job: &SyncJob,
    actor: &str,
    trigger: &str,
) -> SyncOutcome {
    crate::events::log_event(format!(
        "Admin '{}' — {} sync job '{}' ({} -> {})",
        actor, trigger, job.name, job.source, job.destination
    ));
    let outcome = run_sync_core(actor, &request_for(job)).await;

    // The state is written **by the server**, whichever path started the run,
    // and the next due time is recomputed from the moment the run finished.
    let finished_at = chrono::Utc::now();
    let next = crate::storage::sync_job_repo::next_run_after(job, finished_at);
    let repo = SyncJobRepository::new(state.db.pool());
    if let Err(e) = repo
        .record_run(
            job.id,
            if outcome.ok { "ok" } else { "failed" },
            &outcome.message,
            finished_at,
            next,
        )
        .await
    {
        // A run that happened but could not be recorded must say so: the screen
        // would otherwise show the previous outcome as if it were this one.
        tracing::warn!(
            "Sync job '{}' ran but its state could not be saved: {}",
            job.name,
            e
        );
    }
    outcome
}

/// POST /api/rclone/jobs/{id}/run — run a saved sync job now (admin).
///
/// The run happens **inside** this request (the `rclone` subprocess is awaited
/// under `RCLONE_TRANSFER_TIMEOUT`), which is what a "Run now" button means: the
/// admin wants to watch it. The *scheduled* path is the one that must not be
/// request-shaped, and it is not — see [`run_scheduler_loop`].
pub async fn handle_run_sync_job(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Json<ApiResponse<SyncJobView>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let repo = SyncJobRepository::new(state.db.pool());
    let job = match repo.get_by_id(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return Json(ApiResponse::error("Sync job not found")),
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to load sync job: {}",
                e
            )))
        }
    };

    // ROADMAP 16.4: two paths can start a run (the button and the scheduler),
    // so a manual click while one is in flight would run the same `rclone`
    // twice over the same pair of directories.
    let _guard = match JobRunGuard::acquire(id, &job.name) {
        Ok(guard) => guard,
        Err(reason) => return Json(ApiResponse::error(&reason)),
    };

    let outcome = run_saved_job(&state, &job, &admin_username, "ran").await;
    let updated = repo.get_by_id(id).await.ok().flatten().unwrap_or(job);
    if outcome.ok {
        Json(ApiResponse::success(SyncJobView::from(updated)))
    } else {
        // The state is recorded either way, so the screen shows the failure;
        // the envelope still reports it.
        Json(ApiResponse {
            success: false,
            data: Some(SyncJobView::from(updated)),
            error: Some(outcome.message),
        })
    }
}

/// PUT /api/rclone/jobs/{id}/schedule — set (or clear) a job's cadence (admin).
pub async fn handle_update_sync_job_schedule(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSyncJobScheduleRequest>,
) -> Json<ApiResponse<SyncJobView>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    let existing = match repo.get_by_id(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return Json(ApiResponse::error("Sync job not found")),
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to load sync job: {}",
                e
            )))
        }
    };

    let kind = ScheduleKind::parse(req.schedule_kind.as_deref().unwrap_or("none"));
    // Validate through the same `NewSyncJob` rules the create path uses: one
    // definition of "a cadence that can actually be computed".
    let probe = NewSyncJob {
        name: existing.name.clone(),
        source_type: existing.source_type.clone(),
        source: existing.source.clone(),
        source_path: existing.source_path.clone(),
        dest_type: existing.dest_type.clone(),
        destination: existing.destination.clone(),
        dest_path: existing.dest_path.clone(),
        schedule_kind: kind,
        interval_minutes: req.interval_minutes,
        cron_expr: req.cron_expr.clone(),
    };
    if let Err(msg) = probe.validate_schedule() {
        return Json(ApiResponse::error(&msg));
    }

    let now = chrono::Utc::now();
    match repo
        .set_schedule(
            id,
            kind,
            req.interval_minutes,
            req.cron_expr.as_deref(),
            now,
        )
        .await
    {
        Ok(true) => {
            crate::events::log_event(format!(
                "Sync job '{}' cadence set to {}",
                existing.name,
                kind.as_str()
            ));
            let updated = repo
                .get_by_id(id)
                .await
                .ok()
                .flatten()
                .map(SyncJobView::from);
            match updated {
                Some(view) => Json(ApiResponse::success(view)),
                None => Json(ApiResponse::error("Sync job not found")),
            }
        }
        Ok(false) => Json(ApiResponse::error("Sync job not found")),
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to save the schedule: {}",
            e
        ))),
    }
}

/// One run per job at a time, process-wide.
///
/// The scheduler tick and the "Run now" button share it, for the same reason
/// `scheduled_tasks_scheduler::RUNNING` exists: a static is the only way two
/// entry points cannot each believe they own the id.
static RUNNING_JOBS: LazyLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[derive(Debug)]
struct JobRunGuard {
    id: i64,
}

impl JobRunGuard {
    fn acquire(id: i64, name: &str) -> Result<Self, String> {
        let mut running = RUNNING_JOBS.lock().unwrap_or_else(|e| e.into_inner());
        if !running.insert(id) {
            return Err(format!("Sync job '{name}' is already running"));
        }
        Ok(Self { id })
    }
}

impl Drop for JobRunGuard {
    fn drop(&mut self) {
        let mut running = RUNNING_JOBS.lock().unwrap_or_else(|e| e.into_inner());
        running.remove(&self.id);
    }
}

/// How often the scheduler looks for a due job.
///
/// A sync is a minutes-scale operation, so a one-minute tick is plenty and
/// keeps the query (one indexed `next_run_at <= ?`) trivial.
pub const SYNC_SCHEDULER_POLL_SECS: u64 = 60;

/// Runs forever; `tokio::spawn`ed once at startup (ROADMAP 16.4).
///
/// This is what makes a nightly sync possible at all: until it existed,
/// `POST /rclone/jobs/{id}/run` was the only way a persisted job ever ran.
/// The transfer happens **outside** any HTTP request — the tick spawns the run
/// and moves on, so a six-hour sync does not hold a connection open (the
/// request-shaped-transfer debt recorded in `docs/guides/cloud-sync.md`).
pub async fn run_scheduler_loop(state: AppStateArc) {
    let mut interval = tokio::time::interval(Duration::from_secs(SYNC_SCHEDULER_POLL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = tick_due(&state).await {
            tracing::warn!("Cloud-sync scheduler tick failed: {}", e);
        }
    }
}

/// Starts every job that is due at this instant.
async fn tick_due(state: &AppStateArc) -> Result<(), String> {
    let repo = SyncJobRepository::new(state.db.pool());
    let now = chrono::Utc::now();
    let due = repo.due_now(now).await.map_err(|e| e.to_string())?;

    for job in due {
        // Claim the run first: a job that is still going (from the previous
        // tick, or started by hand) is skipped, and its `next_run_at` is left
        // alone so the claim is retried on the next tick.
        let guard = match JobRunGuard::acquire(job.id, &job.name) {
            Ok(guard) => guard,
            Err(_) => {
                tracing::info!("Sync job '{}' still running; skipping tick", job.name);
                continue;
            }
        };
        // The scheduler is not a user; the event log names the trigger instead.
        let state_for_run = state.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let outcome = run_saved_job(&state_for_run, &job, "scheduler", "scheduled").await;
            if !outcome.ok {
                tracing::warn!(
                    "Scheduled sync job '{}' failed: {}",
                    job.name,
                    outcome.message
                );
            }
        });
    }
    Ok(())
}

/// Run copy operation/// Run copy operation
pub async fn handle_copy(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<CopyRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let mut args = vec!["copy".to_string()];

    // Determine path types
    let source_type = req.source_type.as_deref().unwrap_or("cloud");
    let dest_type = req.dest_type.as_deref().unwrap_or("cloud");

    // Build source and destination paths
    let source = build_rclone_path(source_type, &req.source, &req.source_path);
    let dest = build_rclone_path(dest_type, &req.destination, &req.dest_path);

    // Validate
    if source.is_empty() || source == ":" {
        return Json(ApiResponse::error("Source is required"));
    }
    if dest.is_empty() || dest == ":" {
        return Json(ApiResponse::error("Destination is required"));
    }

    args.push(source);
    args.push(dest);

    if req.delete {
        args.push("--delete".to_string());
    }

    if req.dry_run {
        args.push("--dry-run".to_string());
    }

    if req.interactive {
        args.push("--interactive".to_string());
    }

    // Add progress output
    args.push("--progress".to_string());

    let job = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("cloud_sync".to_string()),
        &format!("Copy {} -> {}", req.source, req.destination),
    );

    let output = run_timed_async_result(rclone_command().args(&args), RCLONE_TRANSFER_TIMEOUT).await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                crate::sse::PROGRESS_MANAGER.complete_job(&job, "Copy complete");
                crate::events::log_event(format!(
                    "Admin '{}' ran a copy from '{}' to '{}'",
                    admin_username, req.source, req.destination
                ));
                Json(ApiResponse::success(stdout.to_string()))
            } else {
                crate::sse::PROGRESS_MANAGER.fail_job(&job, &stderr);
                Json(ApiResponse::error(&format!("Copy failed: {}", stderr)))
            }
        }
        Err(e) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job, &e.to_string());
            crate::events::log_event(format!(
                "Admin '{}' — copy from '{}' to '{}' aborted: {}",
                admin_username, req.source, req.destination, e
            ));
            Json(ApiResponse::error(&format!("Copy failed: {}", e)))
        }
    }
}

/// Check if rclone is available.
///
/// Returns the resolved mount base too, so the screen stops building
/// `/mnt/rclone/<name>` itself — the server is the only side that knows
/// where mounts are allowed (`docs/guides/cloud-sync.md`).
pub async fn handle_rclone_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<RcloneStatus>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Both probes are bounded: a wrapper script on `$PATH` that never answers
    // must not hang the status tile (ROADMAP 16.1).
    let available =
        run_timed_async_result(Command::new("rclone").arg("version"), RCLONE_PROBE_TIMEOUT)
            .await
            .map(|output| output.status.success())
            .unwrap_or(false);

    // `fusermount` is what unmounts a FUSE mount; the screen warns when it is
    // missing instead of letting "Stop mount" fail.
    let fusermount =
        run_timed_async_result(Command::new("fusermount").arg("-V"), RCLONE_PROBE_TIMEOUT)
            .await
            .map(|output| output.status.success())
            .unwrap_or(false);

    Json(ApiResponse::success(RcloneStatus {
        available,
        fusermount,
        mount_base: RCLONE_MOUNT_BASE.to_string(),
    }))
}

/// Get the current rclone.conf content. Admin-only: this file contains
/// every configured remote's credentials — rclone "obscures" secrets with
/// a trivially-reversible encoding, not real encryption, so exposing this
/// to a non-admin session would be a plaintext credential leak.
pub async fn handle_get_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let config_path = rclone_config_path();

    match tokio::fs::read_to_string(&config_path).await {
        Ok(content) => Json(ApiResponse::success(content)),
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to read config {}: {}",
            config_path.display(),
            e
        ))),
    }
}

/// Request for saving config
#[derive(Deserialize)]
pub struct SaveConfigRequest {
    pub config: String,
}

/// Save the rclone.conf content.
///
/// The file holds every remote's credentials, so both it and its dated backup
/// are written mode 0600 (ROADMAP 16.3): `tokio::fs::write` used to create it
/// with the process umask (0644 on a default Debian install) and
/// `tokio::fs::copy` recreated the backup the same way.
pub async fn handle_save_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<SaveConfigRequest>,
) -> Json<ApiResponse<String>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let config_path = rclone_config_path();
    let backup_path = std::path::PathBuf::from(format!(
        "{}.backup.{}",
        config_path.to_string_lossy(),
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
    ));

    // Back up the existing content by re-reading it, not by `copy`: a copy
    // keeps the source's mode and the point is to write both files 0600.
    if let Ok(previous) = tokio::fs::read_to_string(&config_path).await {
        if let Err(e) = write_private_config(&backup_path, &previous).await {
            return Json(ApiResponse::error(&format!(
                "Failed to create backup: {}",
                e
            )));
        }
    }

    match write_private_config(&config_path, &req.config).await {
        Ok(()) => Json(ApiResponse::success(format!(
            "Config saved successfully. Backup: {}",
            backup_path.display()
        ))),
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Request for adding a remote
#[derive(Deserialize)]
pub struct AddRemoteRequest {
    pub name: String,
    pub provider: String,
    pub config: Option<String>,
}

/// Add a new remote
pub async fn handle_add_remote(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<AddRemoteRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let config_path = rclone_config_path();

    // Validate name and provider. The provider is written as `type = …`, so an
    // unvalidated value containing a newline would append arbitrary INI lines
    // to the section — the same injection class as the `[section]` guard below.
    let name = match validate_remote_name(&req.name) {
        Ok(name) => name,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let provider = match validate_provider_slug(&req.provider) {
        Ok(provider) => provider,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // Reject a `config` value containing an embedded `[section]` line —
    // otherwise this free-text field, appended verbatim into the INI-style
    // config file below, could silently inject or override an unrelated,
    // already-configured remote's section.
    if let Some(config) = &req.config {
        if config.lines().any(|l| l.trim_start().starts_with('[')) {
            return Json(ApiResponse::error(
                "Config value cannot contain a line starting with '[' (would inject another config section)",
            ));
        }
    }

    // Read existing config
    let existing_config: String = tokio::fs::read_to_string(&config_path)
        .await
        .unwrap_or_default();

    // Exact section match: `contains("[my]")` also matched `[mydrive]`, which
    // refused a legitimate `my` and made the delete path ambiguous.
    if config_has_section(&existing_config, &name) {
        return Json(ApiResponse::error(&format!(
            "Remote '{}' already exists",
            name
        )));
    }

    // Build new remote section
    let remote_section = format!("[{}]", name);
    let new_section = if let Some(config) = &req.config {
        format!("\n{}\ntype = {}\n{}\n", remote_section, provider, config)
    } else {
        format!("\n{}\ntype = {}\n", remote_section, provider)
    };

    // Append to config
    let new_config = format!("{}{}", existing_config.trim_end(), new_section);

    match write_private_config(&config_path, &new_config).await {
        Ok(()) => {
            crate::events::log_event(format!(
                "Admin '{}' added cloud remote '{}' ({})",
                admin_username, name, provider
            ));
            Json(ApiResponse::success(format!(
                "Remote '{}' added successfully",
                name
            )))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Delete a remote.
///
/// The old implementation searched the file for `[name]` and then for the next
/// `[` anywhere after it, so deleting `my` matched inside `[mydrive]` and a
/// value like `patterns = [a-z]` could cut a section in half. The edit is now
/// a parsed one, on whole lines, and the result is written 0600 like every
/// other config write.
pub async fn handle_delete_remote(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let config_path = rclone_config_path();

    // Read existing config
    let existing_config = match tokio::fs::read_to_string(&config_path).await {
        Ok(content) => content,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to read config {}: {}",
                config_path.display(),
                e
            )))
        }
    };

    let new_config = match remove_section(&existing_config, &name) {
        Ok(content) => content,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    match write_private_config(&config_path, &new_config).await {
        Ok(()) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted cloud remote '{}'",
                admin_username, name
            ));
            Json(ApiResponse::success(format!(
                "Remote '{}' deleted successfully",
                name
            )))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for the Cloud Sync (rclone) handler module.
    //!
    //! Covers the deterministic helpers: `build_rclone_path` (how a
    //! source/dest becomes an `rclone` argument — `remote:` vs `remote:sub`
    //! vs a local path), `get_provider_description` (the provider-type →
    //! label table, incl. the `<type> (custom)` fallback), the `rclone.conf`
    //! INI parser, the input validators that guard an argv and the config file
    //! (ROADMAP 16.3), the parsed section removal, the `/proc/mounts` matcher
    //! (16.2) and the `ApiResponse` envelope shape the frontend checks.
    //!
    //! No network, no `rclone` process, no FUSE. The config-file tests point
    //! `RCLONE_CONFIG` at a tempfile (serialised behind a mutex).
    //!
    //! Run: `cargo test handlers::rclone::tests`

    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -- build_rclone_path -------------------------------------------------

    #[test]
    fn test_build_rclone_path_cloud_root() {
        assert_eq!(build_rclone_path("cloud", "gdrive", &None), "gdrive:");
        assert_eq!(
            build_rclone_path("cloud", "gdrive", &Some(String::new())),
            "gdrive:"
        );
    }

    #[test]
    fn test_build_rclone_path_cloud_with_subpath() {
        assert_eq!(
            build_rclone_path("cloud", "gdrive", &Some("backups/2026".to_string())),
            "gdrive:backups/2026"
        );
        // a leading slash on the subpath is stripped (rclone remote paths
        // are not absolute)
        assert_eq!(
            build_rclone_path("cloud", "gdrive", &Some("/backups".to_string())),
            "gdrive:backups"
        );
    }

    #[test]
    fn test_build_rclone_path_local() {
        assert_eq!(build_rclone_path("local", "/srv/data", &None), "/srv/data");
        assert_eq!(
            build_rclone_path("local", "/srv/data", &Some("photos".to_string())),
            "/srv/data/photos"
        );
        // trailing slash on the base + leading slash on the subpath collapse
        assert_eq!(
            build_rclone_path("local", "/srv/data/", &Some("/photos".to_string())),
            "/srv/data/photos"
        );
    }

    #[test]
    fn test_build_rclone_path_anything_not_local_is_treated_as_cloud() {
        assert_eq!(build_rclone_path("", "r", &None), "r:");
        assert_eq!(build_rclone_path("cloud", "r", &None), "r:");
    }

    // -- get_provider_description ---------------------------------------

    #[test]
    fn test_provider_description_known_types() {
        assert_eq!(get_provider_description("drive"), "Google Drive");
        assert_eq!(get_provider_description("s3"), "Amazon S3");
        assert_eq!(get_provider_description("webdav"), "WebDAV");
        assert_eq!(get_provider_description("crypt"), "Encrypted (Crypt)");
    }

    #[test]
    fn test_provider_description_is_case_insensitive_and_has_aliases() {
        assert_eq!(get_provider_description("DRIVE"), "Google Drive");
        assert_eq!(get_provider_description("b2"), "Backblaze B2");
        assert_eq!(get_provider_description("backblaze"), "Backblaze B2");
        assert_eq!(get_provider_description("smb"), "SMB/CIFS");
        assert_eq!(get_provider_description("cifs"), "SMB/CIFS");
    }

    #[test]
    fn test_provider_description_unknown_type_falls_back_to_custom() {
        assert_eq!(
            get_provider_description("weirdcloud"),
            "weirdcloud (custom)"
        );
    }

    // -- rclone_config_path (one resolver for reads AND writes) -------

    /// Run `f` with `RCLONE_CONFIG` either set to `value` or removed, holding
    /// `ENV_LOCK` and always restoring the process environment.
    fn with_env_override<T>(value: Option<&std::path::Path>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var("RCLONE_CONFIG").ok();
        match value {
            Some(path) => std::env::set_var("RCLONE_CONFIG", path),
            None => std::env::remove_var("RCLONE_CONFIG"),
        }
        let out = f();
        match previous {
            Some(p) => std::env::set_var("RCLONE_CONFIG", p),
            None => std::env::remove_var("RCLONE_CONFIG"),
        }
        out
    }

    #[test]
    fn test_rclone_config_path_honours_the_env_override() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.conf");
        // Not created on disk: an override must win anyway, because that is how
        // a fresh config gets created outside /etc.
        assert!(!path.exists());
        let resolved = with_env_override(Some(&path), rclone_config_path);
        assert_eq!(resolved, path);
    }

    #[test]
    fn test_rclone_config_path_ignores_an_empty_env_override() {
        // An exported-but-empty `RCLONE_CONFIG` must not resolve to "" — that
        // would make every read fail and every write land in the cwd.
        let resolved = with_env_override(Some(std::path::Path::new("")), || {
            let p = rclone_config_path();
            assert!(!p.as_os_str().is_empty());
            p
        });
        assert!(resolved.is_absolute());
    }

    #[test]
    fn test_rclone_config_path_falls_back_to_the_default_when_none_exists() {
        // No override, and (in CI) no /etc/rclone.conf either. Whatever the
        // host has, the answer must be an absolute path — never a relative one
        // built from an unset HOME.
        let resolved = with_env_override(None, rclone_config_path);
        assert!(resolved.is_absolute(), "got {}", resolved.display());
    }

    #[test]
    fn test_rclone_command_passes_explicit_config_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.conf");
        with_env_override(Some(&path), || {
            let cmd = rclone_command();
            let std_cmd = cmd.as_std();
            let args: Vec<&std::ffi::OsStr> = std_cmd.get_args().collect();
            assert_eq!(
                args,
                &[std::ffi::OsStr::new("--config"), path.as_os_str(),],
                "rclone_command must pass `--config <rclone_config_path()>` as its first arguments"
            );
        });
    }

    #[test]
    fn test_parse_and_the_handlers_resolve_the_same_file() {
        // The regression this guards: the parser honoured `RCLONE_CONFIG`
        // while `handle_get_config`/`_save_config`/`add_remote`/`delete_remote`
        // hardcoded `/etc/rclone.conf`, so the editor could read and write a
        // different file than the one the remote list came from. Both sides now
        // go through `rclone_config_path` — if one of them grows its own
        // hardcoded path again, this fails.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rclone.conf");
        std::fs::write(&path, "[probe]\ntype = s3\n").unwrap();

        let (parsed, resolved) = with_env_override(Some(&path), || {
            let config_path = rclone_config_path();
            let parsed = read_rclone_config(&config_path)
                .and_then(|content| parse_rclone_config_text(&content));
            (parsed, config_path)
        });

        assert_eq!(resolved, path, "the writer's target must be the override");
        let remotes = parsed.expect("the parser must read that same file");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "probe");
        assert_eq!(remotes[0].remote_type, "Amazon S3");
    }

    // -- parse_rclone_config_text (INI) -------------------------------
    //
    // The parser is pure now: the file read lives in `read_rclone_config`
    // (called only from `spawn_blocking`), so the matrix below needs no
    // tempdir and no process environment.

    #[test]
    fn test_parse_rclone_config_reads_sections_sorted() {
        let cfg = "\
# a comment
[zeta]
type = dropbox
token = xxx

[alpha]
type = drive
scope = drive
";
        let remotes = parse_rclone_config_text(cfg).expect("some remotes");
        let names: Vec<_> = remotes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]); // sorted by name
        assert_eq!(remotes[0].remote_type, "Google Drive");
        assert_eq!(remotes[1].remote_type, "Dropbox");
    }

    #[test]
    fn test_parse_rclone_config_skips_sections_without_a_type() {
        let cfg = "\
[good]
type = s3

[no_type]
provider = whatever
region = us-east-1
";
        let remotes = parse_rclone_config_text(cfg).expect("some remotes");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "good");
    }

    #[test]
    fn test_parse_rclone_config_text_returns_none_for_an_empty_file() {
        assert!(parse_rclone_config_text("").is_none());
        assert!(parse_rclone_config_text("# only a comment\n").is_none());
    }

    // -- validate_remote_name (16.3) ----------------------------------

    #[test]
    fn test_validate_remote_name_keeps_a_plain_name() {
        assert_eq!(validate_remote_name("gdrive").unwrap(), "gdrive");
        assert_eq!(
            validate_remote_name("  my.drive-2  ").unwrap(),
            "my.drive-2"
        );
        assert_eq!(validate_remote_name("s3@home+x").unwrap(), "s3@home+x");
    }

    #[test]
    fn test_validate_remote_name_rejects_an_empty_name() {
        assert_eq!(
            validate_remote_name("   ").unwrap_err(),
            "Remote name is required"
        );
    }

    #[test]
    fn test_validate_remote_name_rejects_arg_and_ini_metacharacters() {
        for raw in [
            "a:b",
            "has[bracket",
            "has]bracket",
            "a/b",
            "a\\b",
            "-leading",
        ] {
            let err = validate_remote_name(raw).unwrap_err().to_lowercase();
            assert!(
                err.contains("invalid characters") || err.contains("must not start with"),
                "{raw:?} must be refused, got {err:?}"
            );
        }
        // A space is refused too: a name is never quoted in the section header,
        // so it would silently truncate in a hand-edited config.
        assert!(validate_remote_name("two words").is_err());
    }

    #[test]
    fn test_validate_remote_name_rejects_a_name_that_is_too_long() {
        let long = "a".repeat(MAX_REMOTE_NAME_LEN + 1);
        assert!(validate_remote_name(&long).is_err());
        assert!(validate_remote_name(&"a".repeat(MAX_REMOTE_NAME_LEN)).is_ok());
    }

    // -- validate_provider_slug (16.3, INI injection) -----------------

    #[test]
    fn test_validate_provider_slug_accepts_known_slugs() {
        assert_eq!(validate_provider_slug("drive").unwrap(), "drive");
        assert_eq!(validate_provider_slug(" S3 ").unwrap(), "s3");
        assert_eq!(validate_provider_slug("webdav").unwrap(), "webdav");
        assert_eq!(validate_provider_slug("local").unwrap(), "local");
    }

    #[test]
    fn test_validate_provider_slug_refuses_to_inject_ini_lines() {
        // `type = {provider}` is written verbatim into the config section.
        let err = validate_provider_slug("s3\ntoken = attacker").unwrap_err();
        assert!(err.contains("invalid characters"), "got {err:?}");
        assert!(validate_provider_slug("[evil]").is_err());
        assert!(validate_provider_slug("drive\n[evil]").is_err());
    }

    #[test]
    fn test_validate_provider_slug_rejects_empty_and_overlong_values() {
        assert_eq!(
            validate_provider_slug("").unwrap_err(),
            "Provider type is required"
        );
        assert!(validate_provider_slug(&"a".repeat(MAX_PROVIDER_LEN + 1)).is_err());
    }

    // -- validate_mount_point (16.3) ----------------------------------

    #[test]
    fn test_validate_mount_point_accepts_a_child_of_the_base() {
        let base = crate::config::paths::RCLONE_MOUNT_BASE;
        assert_eq!(
            validate_mount_point(&format!("{}/gdrive", base)).unwrap(),
            format!("{}/gdrive", base)
        );
        // A trailing slash is normalised away, and a nested folder is allowed.
        assert_eq!(
            validate_mount_point(&format!("{}/gdrive/", base)).unwrap(),
            format!("{}/gdrive", base)
        );
        assert_eq!(
            validate_mount_point(&format!("{}/team/a", base)).unwrap(),
            format!("{}/team/a", base)
        );
    }

    #[test]
    fn test_validate_mount_point_requires_one() {
        // The exact sentence the E2E suite (and the screen) shows.
        assert_eq!(
            validate_mount_point("").unwrap_err(),
            "Mount point is required"
        );
    }

    #[test]
    fn test_validate_mount_point_refuses_anything_outside_the_base() {
        let base = crate::config::paths::RCLONE_MOUNT_BASE;
        for raw in [
            "/tmp/evil".to_string(),
            "/mnt".to_string(),
            "/".to_string(),
            base.to_string(),
            format!("{}/../etc", base),
            format!("{}/./x", base),
            format!("{}/ok/../../etc", base),
            "relative/dir".to_string(),
        ] {
            assert!(
                validate_mount_point(&raw).is_err(),
                "{raw:?} must not be accepted as a mount point"
            );
        }
    }

    #[test]
    fn test_validate_mount_point_refuses_control_characters_and_leading_dash() {
        let base = crate::config::paths::RCLONE_MOUNT_BASE;
        assert!(validate_mount_point(&format!("{}/ok\n", base)).is_err());
        assert!(validate_mount_point(&format!("{}/-rf", base)).is_err());
    }

    // -- validate_remote_subpath --------------------------------------

    #[test]
    fn test_validate_remote_subpath_normalises_and_rejects_traversal() {
        assert_eq!(validate_remote_subpath(&None).unwrap(), None);
        assert_eq!(
            validate_remote_subpath(&Some("  ".to_string())).unwrap(),
            None
        );
        assert_eq!(
            validate_remote_subpath(&Some("/backups/2026/".to_string())).unwrap_err(),
            "Remote path must be relative to the remote root"
        );
        assert_eq!(
            validate_remote_subpath(&Some("backups/2026/".to_string())).unwrap(),
            Some("backups/2026".to_string())
        );
        assert!(validate_remote_subpath(&Some("../etc".to_string())).is_err());
        assert!(validate_remote_subpath(&Some("a/../b".to_string())).is_err());
        assert!(validate_remote_subpath(&Some("a\\b".to_string())).is_err());
    }

    // -- config_has_section / remove_section (16.3, prefix bug) -------

    #[test]
    fn test_config_has_section_matches_whole_headers_only() {
        let config = "[mydrive]\ntype = drive\n[my]\ntype = s3\n";
        assert!(config_has_section(config, "my"));
        assert!(config_has_section(config, "mydrive"));
        assert!(!config_has_section(config, "myd"));
        // A commented head is not a section.
        assert!(!config_has_section("# [ghost]\ntype = s3\n", "ghost"));
    }

    #[test]
    fn test_config_has_section_does_not_match_a_prefix() {
        // The exact bug: `contains("[my]")` was true for `[mydrive]`.
        let config = "[mydrive]\ntype = drive\n";
        assert!(!config_has_section(config, "my"));
    }

    #[test]
    fn test_remove_section_removes_only_the_named_section() {
        let config = "# header comment\n[mydrive]\ntype = drive\n\n[my]\ntype = s3\nregion = eu\n";
        let result = remove_section(config, "my").unwrap();
        assert!(!result.contains("[my]"), "got {result:?}");
        assert!(
            result.contains("[mydrive]") && result.contains("type = drive"),
            "the other section must survive, got {result:?}"
        );
        assert!(
            result.contains("# header comment"),
            "comments must survive, got {result:?}"
        );
    }

    #[test]
    fn test_remove_section_does_not_cut_on_a_bracketed_value() {
        // The old code ended the section at the first `[` *anywhere*, so this
        // value truncated the section and left `z]` behind.
        let config = "[a]\ntype = crypt\npatterns = [a-z]\n\n[b]\ntype = s3\n";
        let result = remove_section(config, "a").unwrap();
        assert_eq!(result, "[b]\ntype = s3\n");
    }

    #[test]
    fn test_remove_section_handles_the_first_and_last_section() {
        let first = remove_section("[a]\ntype = s3\n\n[b]\ntype = drive\n", "a").unwrap();
        assert_eq!(first, "[b]\ntype = drive\n");
        let last = remove_section("[a]\ntype = s3\n\n[b]\ntype = drive\n", "b").unwrap();
        assert_eq!(last, "[a]\ntype = s3\n");
    }

    #[test]
    fn test_remove_section_reports_a_missing_section() {
        assert_eq!(
            remove_section("[a]\ntype = s3\n", "b").unwrap_err(),
            "Remote 'b' not found"
        );
        // A prefix must not be deleted by accident.
        assert!(remove_section("[mydrive]\ntype = drive\n", "my").is_err());
    }

    // -- parse_mounts_for / decode_mount_escape (16.2) ----------------

    #[test]
    fn test_parse_mounts_for_matches_the_whole_field() {
        let mounts = "\
/dev/sda1 / ext4 rw 0 0
gdrive: /mnt/rclone/vault fuse.rclone rw 0 0
gdrive: /mnt/rclone/vault2 fuse.rclone rw 0 0
";
        assert!(parse_mounts_for(mounts, "/mnt/rclone/vault"));
        assert!(!parse_mounts_for(mounts, "/mnt/rclone"));
        assert!(!parse_mounts_for(mounts, "/mnt/rclone/vau"));
    }

    #[test]
    fn test_parse_mounts_for_decodes_octal_escapes() {
        // The kernel writes a space as \040, so a mount point with a space
        // would never match the raw text.
        let mounts = "remote: /mnt/rclone/my\\040disk fuse.rclone rw 0 0\n";
        assert!(parse_mounts_for(mounts, "/mnt/rclone/my disk"));
        assert_eq!(decode_mount_escape("/a\\040b"), "/a b");
        assert_eq!(decode_mount_escape("/a\\011b"), "/a\tb");
        assert_eq!(decode_mount_escape("/plain"), "/plain");
    }

    // -- deadlines (16.1) ---------------------------------------------

    #[test]
    fn test_rclone_deadlines_are_bounded_and_ordered() {
        // A probe must answer faster than a mount may take to appear, which
        // must itself be faster than a tolerant unmount, which is far below the
        // transfer ceiling.
        assert!(RCLONE_PROBE_TIMEOUT <= Duration::from_secs(30));
        assert!(RCLONE_PROBE_TIMEOUT < RCLONE_MOUNT_READY_TIMEOUT);
        assert!(RCLONE_MOUNT_READY_TIMEOUT < RCLONE_UMOUNT_TIMEOUT);
        assert!(RCLONE_UMOUNT_TIMEOUT < RCLONE_TRANSFER_TIMEOUT);
        // Long enough for a real first sync, short enough to be a ceiling.
        assert!(RCLONE_TRANSFER_TIMEOUT >= Duration::from_secs(60 * 60));
        assert!(RCLONE_TRANSFER_TIMEOUT <= Duration::from_secs(24 * 60 * 60));
    }

    #[test]
    fn test_config_write_mode_is_private() {
        // 0600: owner read/write only. The file holds every remote's
        // credentials (rclone "obscures", it does not encrypt).
        assert_eq!(CONFIG_FILE_MODE, 0o600);
    }

    // -- status payload (mount base comes from the server) ------------

    #[test]
    fn test_rclone_status_carries_the_server_mount_base() {
        let status = RcloneStatus {
            available: true,
            fusermount: false,
            mount_base: crate::config::paths::RCLONE_MOUNT_BASE.to_string(),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["available"], true);
        assert_eq!(json["fusermount"], false);
        assert_eq!(json["mount_base"], crate::config::paths::RCLONE_MOUNT_BASE);
    }

    // -- MountRequest defaults (the release-verification bug) -------

    #[test]
    fn test_api_response_shapes() {
        let ok = serde_json::to_value(ApiResponse::success("hi".to_string())).unwrap();
        assert_eq!(ok["success"], true);
        assert_eq!(ok["data"], "hi");
        assert_eq!(ok["error"], serde_json::Value::Null);

        let err = serde_json::to_value(ApiResponse::<String>::error("boom")).unwrap();
        assert_eq!(err["success"], false);
        assert_eq!(err["data"], serde_json::Value::Null);
        assert_eq!(err["error"], "boom");
    }

    // -- MountRequest defaults (the release-verification bug) -------

    #[test]
    fn test_mount_request_bool_fields_default_to_false() {
        // The frontend only sends { remote, mount_point, path } — the 3 bool
        // fields must be `#[serde(default)]` or every real mount request
        // fails deserialization before the handler runs.
        let req: MountRequest =
            serde_json::from_str(r#"{"remote":"gd","mount_point":"/mnt/gd","path":"backups"}"#)
                .expect("must deserialize without the bool fields");
        assert!(!req.read_only && !req.allow_other && !req.daemon);
        assert_eq!(req.path.as_deref(), Some("backups"));
    }

    // -----------------------------------------------------------------
    // ROADMAP 16.4 — a saved job can be scheduled, and the run leaves the request
    // -----------------------------------------------------------------

    /// The cadence a persisted job runs on. A scheduled run must not be
    /// request-shaped, so the loop exists and is spawned at startup; an
    /// unscheduled job must never be picked up.
    #[test]
    fn the_scheduler_loop_exists_and_is_wired_at_startup() {
        let src = include_str!("rclone.rs");
        assert!(
            src.contains("pub async fn run_scheduler_loop"),
            "the sync scheduler must exist (ROADMAP 16.4)"
        );
        assert!(
            src.contains("SYNC_SCHEDULER_POLL_SECS"),
            "the tick interval must be a named constant"
        );
        let boot = include_str!("../web/mod.rs");
        assert!(
            boot.contains("rclone::run_scheduler_loop"),
            "the loop must be spawned at startup, or a nightly sync never happens"
        );
    }

    /// The run core is shared by the button and the scheduler: the scheduler
    /// cannot use an HTTP envelope, which is exactly why it was extracted.
    #[test]
    fn the_run_core_returns_a_plain_outcome() {
        let src = include_str!("rclone.rs");
        assert!(
            src.contains("struct SyncOutcome") && src.contains("async fn run_sync_core"),
            "the transfer must be callable without an axum response"
        );
        assert!(
            src.contains("record_run"),
            "the run path must write the server-side state"
        );
    }

    /// Two entry points (button + scheduler) must not run the same job twice
    /// over the same pair of directories.
    #[test]
    fn a_job_cannot_run_twice_at_once() {
        let _first = JobRunGuard::acquire(4242, "guard test").expect("first claim wins");
        let second = JobRunGuard::acquire(4242, "guard test");
        assert!(second.is_err(), "a second concurrent run must be refused");
        let err = second.unwrap_err();
        assert!(err.contains("already running"), "{err}");
        drop(_first);
        assert!(
            JobRunGuard::acquire(4242, "guard test").is_ok(),
            "the claim must be released when the run ends"
        );
    }

    /// The stored job is replayed through the same request shape the ad-hoc
    /// endpoint uses, and the recorded state keeps the run's own options (a
    /// scheduled sync must never gain `--delete` from nowhere).
    #[test]
    fn a_saved_job_replays_as_a_non_destructive_sync() {
        let job = SyncJob {
            id: 7,
            name: "nightly".into(),
            source_type: "cloud".into(),
            source: "gdrive:photos".into(),
            source_path: Some("2024".into()),
            dest_type: "local".into(),
            destination: "/srv/photos".into(),
            dest_path: None,
            created_at: "2026-10-24T00:00:00Z".into(),
            schedule_kind: "cron".into(),
            interval_minutes: None,
            cron_expr: Some("0 3 * * *".into()),
            last_run_at: None,
            last_status: None,
            last_message: None,
            next_run_at: None,
        };
        let req = request_for(&job);
        assert_eq!(req.source, "gdrive:photos");
        assert_eq!(req.source_path.as_deref(), Some("2024"));
        assert_eq!(req.destination, "/srv/photos");
        assert_eq!(req.source_type.as_deref(), Some("cloud"));
        assert_eq!(req.dest_type.as_deref(), Some("local"));
        assert!(!req.delete, "a scheduled sync must never delete by default");
        assert!(!req.dry_run);
        assert!(req.update);
        assert!(req.exclude.is_none());
    }
}
