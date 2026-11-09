/* =============================================================================
 * File          : downloader.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Download-manager endpoints backed by the aria2 daemon: service status
 *   and start/stop, global and per-task statistics, adding URLs and
 *   torrents, pause/resume/removal, and task details. Every endpoint is
 *   admin-only; aria2 state is shared through Aria2State in AppState.
 *
 *   Since ROADMAP 17 nothing the request says reaches the RPC unvalidated:
 *   links go through a scheme allowlist, `dir` is a relative sub-folder of
 *   `paths::downloader_dir()` and `out` is a bare file name, all checked
 *   before the daemon is spawned. The daemon itself runs with a per-process
 *   `rpc-secret` (kept in a 0600 config file, never on the command line),
 *   every RPC call is bounded, startup waits for a real health probe, and a
 *   crashed child is reaped instead of reported as running.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/downloader in src/web/mod.rs; the Downloader desktop app
 *   polls these endpoints.
 * =============================================================================
 */

//! Download-manager endpoints (aria2): status, tasks, add/pause/remove —
//! all admin-only.

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::handlers::AppStateArc;

const ARIA2_RPC_URL: &str = "http://localhost:6800/jsonrpc";

/// Hard bound on a single JSON-RPC round-trip. Without it a wedged aria2
/// leaves every handler awaiting forever (ROADMAP 17.2).
const ARIA2_RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on the startup health poll. Replaces the old fixed 500 ms sleep that
/// answered "started" before the RPC port was listening (ROADMAP 17.2).
const ARIA2_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Delay between two startup probes.
const ARIA2_START_PROBE: Duration = Duration::from_millis(200);

/// Longest accepted download link. aria2's own `--rpc-max-request-size` is 2 MB;
/// this is about a sane URL, not about what the wire can carry.
pub const MAX_URI_LEN: usize = 2048;

/// Longest accepted output file name.
pub const MAX_OUT_LEN: usize = 255;

/// Longest accepted sub-folder path (relative to the download root).
pub const MAX_SUBFOLDER_LEN: usize = 128;

/// Schemes a download may use. Everything else is refused before aria2 sees it:
/// `file://` (local read as the service user), `data:`, `javascript:`, and the
/// `ftp://`-adjacent proxy schemes that turn a download into SSRF (ROADMAP 17.1).
const ALLOWED_SCHEMES: &[&str] = &["http", "https", "ftp", "sftp", "magnet"];

/// The aria2 daemon handle plus the state its RPC port needs.
pub struct Aria2Process {
    pub child: Option<tokio::process::Child>,
    /// `rpc-secret` value for this process. Generated per start, written to a
    /// 0600 config file and never logged, echoed or returned by an endpoint.
    secret: String,
    /// Absolute download root. Every accepted `dir` is confined under it.
    pub dir: PathBuf,
}

impl Aria2Process {
    pub fn new() -> Self {
        Self {
            child: None,
            secret: new_rpc_secret(),
            dir: crate::config::paths::downloader_dir(),
        }
    }

    /// The RPC secret. Crate-visible only: it must never reach a response body.
    pub(crate) fn rpc_secret(&self) -> &str {
        &self.secret
    }
}

impl Default for Aria2Process {
    fn default() -> Self {
        Self::new()
    }
}

pub type Aria2State = Arc<Mutex<Aria2Process>>;

/// 128 bits of entropy, prefixed so a value seen in a process list or a config
/// file is recognisable as HomeVault's own.
fn new_rpc_secret() -> String {
    format!("hv_{}", uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// Input validation (ROADMAP 17.1)
//
// These are pure functions so the refusal matrix is unit-testable without an
// aria2 process. Every one of them is applied **before** `ensure_aria2_running`
// so a rejected request can never spawn the daemon.
// ---------------------------------------------------------------------------

/// Validates a download link and returns it trimmed.
///
/// Scheme allowlist only: `dir`/`out` confinement alone would still let
/// `file:///etc/shadow` be copied into the download folder.
pub fn validate_uri(raw: &str) -> Result<String, String> {
    let uri = raw.trim();
    if uri.is_empty() {
        return Err("A download link is required".to_string());
    }
    if uri.len() > MAX_URI_LEN {
        return Err(format!(
            "The link is too long (maximum {MAX_URI_LEN} characters)"
        ));
    }
    if uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("The link must not contain spaces or control characters".to_string());
    }

    let (scheme, rest) = uri.split_once(':').ok_or_else(|| {
        "The link must start with a supported scheme (http, https, ftp, sftp, magnet)".to_string()
    })?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return Err(
            "The link must start with a supported scheme (http, https, ftp, sftp, magnet)"
                .to_string(),
        );
    }
    let scheme = scheme.to_ascii_lowercase();
    if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
        return Err(format!(
            "Unsupported link scheme '{scheme}' — use http, https, ftp, sftp or magnet"
        ));
    }

    if scheme == "magnet" {
        if !rest.contains("xt=") {
            return Err("A magnet link must contain an 'xt=' parameter".to_string());
        }
        return Ok(uri.to_string());
    }

    // A plain link needs an authority: `http://host/path`.
    let after_scheme = rest.strip_prefix("//").ok_or_else(|| {
        "The link must include a host, for example https://example.com/file.iso".to_string()
    })?;
    let authority = after_scheme.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return Err(
            "The link must include a host, for example https://example.com/file.iso".to_string(),
        );
    }
    Ok(uri.to_string())
}

/// Validates an optional destination sub-folder, relative to the download root.
///
/// Absolute paths, `..`, `.`, backslashes and control characters are refused:
/// `root.join(sub)` can then only ever land inside the root.
pub fn validate_subfolder(raw: &str) -> Result<Option<String>, String> {
    let sub = raw.trim();
    if sub.is_empty() {
        return Ok(None);
    }
    if sub.len() > MAX_SUBFOLDER_LEN {
        return Err(format!(
            "The sub-folder is too long (maximum {MAX_SUBFOLDER_LEN} characters)"
        ));
    }
    if sub.starts_with('/') || sub.starts_with('\\') {
        return Err("Use a folder inside the download folder, not an absolute path".to_string());
    }
    if sub.contains('\\') {
        return Err("The sub-folder must use '/' as its separator".to_string());
    }
    if sub.chars().any(|c| c.is_control()) {
        return Err("The sub-folder must not contain control characters".to_string());
    }
    for part in sub.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err("The sub-folder must not contain empty, '.' or '..' segments".to_string());
        }
        if part.len() > MAX_OUT_LEN {
            return Err("A folder name in the sub-folder path is too long".to_string());
        }
    }
    Ok(Some(sub.to_string()))
}

/// Validates an optional output file name. It is a *name*, never a path.
pub fn validate_output_name(raw: &str) -> Result<Option<String>, String> {
    let out = raw.trim();
    if out.is_empty() {
        return Ok(None);
    }
    if out.len() > MAX_OUT_LEN {
        return Err(format!(
            "The file name is too long (maximum {MAX_OUT_LEN} characters)"
        ));
    }
    if out == "." || out == ".." {
        return Err("The file name must not be '.' or '..'".to_string());
    }
    if out.contains('/') || out.contains('\\') {
        return Err("The file name must not contain a path separator".to_string());
    }
    if out.chars().any(|c| c.is_control()) {
        return Err("The file name must not contain control characters".to_string());
    }
    Ok(Some(out.to_string()))
}

/// Resolves a validated sub-folder against the canonical download root.
///
/// The deepest existing ancestor of the result is canonicalised and must still
/// start with the root, so a symlink planted inside the download folder cannot
/// redirect a download outside it.
pub fn resolve_download_dir(root: &FsPath, subfolder: Option<&str>) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root)
        .map_err(|e| format!("The download folder {} is not usable: {e}", root.display()))?;

    let Some(sub) = validate_subfolder(subfolder.unwrap_or(""))? else {
        return Ok(root);
    };

    let candidate = root.join(&sub);
    // `validate_subfolder` already refused `..`, but re-check on the assembled
    // path so a future caller cannot skip that guarantee.
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err("The sub-folder must stay inside the download folder".to_string());
    }

    let mut probe = candidate.clone();
    loop {
        if probe.exists() {
            let real = std::fs::canonicalize(&probe)
                .map_err(|e| format!("The folder {} is not usable: {e}", probe.display()))?;
            if !real.starts_with(&root) {
                return Err("The sub-folder must stay inside the download folder".to_string());
            }
            break;
        }
        if !probe.pop() {
            return Err("The sub-folder must stay inside the download folder".to_string());
        }
    }

    Ok(candidate)
}

/// Strips `user:password@` from a link before it reaches the event log
/// (CLAUDE.md "Secrets never leak").
pub fn redact_userinfo(uri: &str) -> String {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return uri.to_string();
    };
    let (authority, tail) = match rest.split_once('/') {
        Some((a, t)) => (a, format!("/{t}")),
        None => (rest, String::new()),
    };
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}{tail}"),
        None => uri.to_string(),
    }
}

/// Validates an optional per-task download/upload rate limit (`500K`, `2M`,
/// `1024`, `0` = unlimited).
///
/// aria2 parses this string itself, so the shape is fixed here: digits and at
/// most one K/M/G suffix. Anything else is refused before the RPC call.
pub fn validate_rate_limit(field: &str, raw: &str) -> Result<Option<String>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let (digits, suffix) = match value.char_indices().last() {
        Some((index, c)) if c.is_ascii_alphabetic() => {
            (&value[..index], Some(c.to_ascii_uppercase()))
        }
        _ => (value, None),
    };
    if digits.is_empty() || digits.len() > 9 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "The {field} must be a number, optionally followed by K, M or G (for example 500K)"
        ));
    }
    match suffix {
        None => Ok(Some(digits.to_string())),
        Some('K' | 'M' | 'G') => Ok(Some(format!("{digits}{}", suffix.unwrap()))),
        Some(_) => Err(format!(
            "The {field} must be a number, optionally followed by K, M or G (for example 500K)"
        )),
    }
}

/// Validates an optional seed ratio (`0` disables seeding for the task,
/// `1.5` stops after 1.5 uploaded copies). aria2 accepts `0.0`–`100.0`.
pub fn validate_seed_ratio(raw: &str) -> Result<Option<String>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 6 || !value.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err("The seed ratio must be a number between 0 and 100".to_string());
    }
    let ratio: f64 = value
        .parse()
        .map_err(|_| "The seed ratio must be a number between 0 and 100".to_string())?;
    if !(0.0..=100.0).contains(&ratio) {
        return Err("The seed ratio must be a number between 0 and 100".to_string());
    }
    Ok(Some(value.to_string()))
}

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

// aria2's real JSON-RPC responses use camelCase field names (confirmed
// against a live daemon: `downloadSpeed`, `numActive`, `totalLength`,
// `completedLength`, `errorMessage`, ...), but these structs' field names
// are snake_case with no `rename_all`, so deserializing a real aria2
// response into them always failed silently (the call sites here discard
// the error via `if let Ok(...)`) — every stat/task lookup returned nothing,
// regardless of the `aria2.tellAll` RPC-method-name bug fixed above. Each
// field below gets an asymmetric `rename(deserialize = ...)` so incoming
// aria2 JSON parses correctly while the JSON this API still sends to the
// frontend (which expects snake_case, matching `Aria2Task` in
// `Downloader.svelte`) is unchanged.
#[derive(Serialize, Deserialize, Debug)]
pub struct Aria2GlobalStat {
    #[serde(rename(deserialize = "downloadSpeed"))]
    pub download_speed: String,
    #[serde(rename(deserialize = "uploadSpeed"))]
    pub upload_speed: String,
    #[serde(rename(deserialize = "numActive"))]
    pub num_active: String,
    #[serde(rename(deserialize = "numWaiting"))]
    pub num_waiting: String,
    #[serde(rename(deserialize = "numStopped"))]
    pub num_stopped: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Aria2Task {
    pub gid: String,
    pub status: String,
    #[serde(rename(deserialize = "totalLength"))]
    pub total_length: String,
    #[serde(rename(deserialize = "completedLength"))]
    pub completed_length: String,
    #[serde(rename(deserialize = "downloadSpeed"))]
    pub download_speed: String,
    #[serde(rename(deserialize = "uploadSpeed"))]
    pub upload_speed: String,
    #[serde(rename(deserialize = "errorMessage"))]
    pub error_message: Option<String>,
    pub bittorrent: Option<Aria2BtInfo>,
    pub files: Vec<Aria2File>,
    pub dir: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Aria2BtInfo {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Aria2File {
    pub index: String,
    pub path: String,
    pub length: String,
    #[serde(rename(deserialize = "completedLength"))]
    pub completed_length: String,
    pub selected: String,
    pub uris: Vec<Aria2Uri>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Aria2Uri {
    pub uri: String,
    pub status: String,
}

#[derive(Deserialize)]
pub struct AddUriRequest {
    pub uri: String,
    pub dir: Option<String>,
    pub out: Option<String>,
    /// Per-task speed cap (`500K`, `2M`, `0` = unlimited).
    #[serde(default)]
    pub max_download_limit: Option<String>,
    /// Per-task upload cap (torrents).
    #[serde(default)]
    pub max_upload_limit: Option<String>,
    /// Stop seeding after this ratio (`0` = no seeding).
    #[serde(default)]
    pub seed_ratio: Option<String>,
}

#[derive(Deserialize)]
pub struct GidRequest {
    pub gid: String,
}

/// One JSON-RPC call to the local aria2 daemon, with the shared secret.
///
/// The secret is read from the process handle and prepended as aria2's
/// `token:<secret>` first parameter, so the RPC port is no longer drivable by
/// any local process (ROADMAP 17.2).
async fn aria2_rpc(
    state: &Aria2State,
    method: &str,
    params: Vec<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let secret = state.lock().await.rpc_secret().to_string();
    aria2_rpc_with_secret(&secret, method, params, ARIA2_RPC_TIMEOUT).await
}

async fn aria2_rpc_with_secret(
    secret: &str,
    method: &str,
    params: Vec<serde_json::Value>,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let mut params = params;
    params.insert(0, serde_json::json!(format!("token:{secret}")));

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "homevault",
        "method": method,
        "params": params
    });

    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("aria2 client setup failed: {}", e))?;

    let resp = client
        .post(ARIA2_RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                format!("aria2 did not answer within {} s", timeout.as_secs().max(1))
            } else {
                format!("aria2 RPC connection failed: {}", e)
            }
        })?;

    let result: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("aria2 RPC parse failed: {}", e))?;

    if let Some(error) = result.get("error") {
        return Err(error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string());
    }

    Ok(result
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

/// `true` while the child we own is alive. A crashed child is reaped here
/// instead of being reported as "running" forever (ROADMAP 17.3).
pub(crate) async fn aria2_is_running(state: &Aria2State) -> bool {
    let mut proc = state.lock().await;
    let Some(child) = proc.child.as_mut() else {
        return false;
    };
    match child.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            tracing::warn!("aria2c exited unexpectedly ({status}); clearing the process handle");
            proc.child = None;
            false
        }
        Err(e) => {
            tracing::warn!("could not poll aria2c ({e}); clearing the process handle");
            proc.child = None;
            false
        }
    }
}

/// Read-only `aria2.tellActive` for the desktop activity taskbar. Shares the
/// authenticated RPC path with this module instead of re-implementing it, and
/// keeps its own short bound: the taskbar polls every few seconds.
pub(crate) async fn aria2_active_tasks(state: &Aria2State) -> Result<Vec<Aria2Task>, String> {
    let secret = state.lock().await.rpc_secret().to_string();
    let value = aria2_rpc_with_secret(
        &secret,
        "aria2.tellActive",
        vec![],
        Duration::from_millis(1500),
    )
    .await?;
    serde_json::from_value(value).map_err(|e| format!("aria2 task decode failed: {e}"))
}

async fn find_aria2() -> Option<String> {
    let candidates = ["aria2c", "/usr/bin/aria2c", "/usr/local/bin/aria2c"];
    for bin in &candidates {
        let check = if bin.starts_with('/') {
            tokio::fs::metadata(bin).await.is_ok()
        } else {
            // Bounded: a `PATH` entry on a dead automount makes `which` block
            // (ROADMAP X.9).
            crate::utils::process::run_timed_async(
                tokio::process::Command::new("which").arg(bin),
                std::time::Duration::from_secs(10),
            )
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        };
        if check {
            return Some(bin.to_string());
        }
    }
    None
}

/// Writes `rpc-secret=<secret>` to aria2's config file with mode 0600.
///
/// A command-line `--rpc-secret` would be readable by every local user through
/// `/proc/<pid>/cmdline`; a 0600 file is the narrowest place aria2 accepts it.
async fn write_rpc_config(secret: &str) -> Result<PathBuf, String> {
    let path = crate::config::paths::aria2_conf_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            format!(
                "Could not create aria2's configuration folder {}: {e}",
                parent.display()
            )
        })?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .await
        .map_err(|e| format!("Could not write aria2's configuration: {e}"))?;
    file.write_all(format!("rpc-secret={secret}\n").as_bytes())
        .await
        .map_err(|e| format!("Could not write aria2's configuration: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("Could not write aria2's configuration: {e}"))?;
    // `mode` only applies when the file is created, so pin it again in case an
    // older, looser file was already there.
    let mut perms = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("Could not read aria2's configuration: {e}"))?
        .permissions();
    perms.set_mode(0o600);
    tokio::fs::set_permissions(&path, perms)
        .await
        .map_err(|e| format!("Could not restrict aria2's configuration: {e}"))?;
    Ok(path)
}

/// Polls `aria2.getVersion` until the daemon answers, or the child dies, or the
/// deadline passes. A successful start now means the RPC port really is up.
async fn wait_until_ready(secret: &str, child: &mut tokio::process::Child) -> Result<(), String> {
    let deadline = Instant::now() + ARIA2_START_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("aria2c exited during startup ({status})"));
            }
            Ok(None) => {}
            Err(e) => return Err(format!("Could not poll aria2c: {e}")),
        }

        if aria2_rpc_with_secret(secret, "aria2.getVersion", vec![], ARIA2_START_PROBE)
            .await
            .is_ok()
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "aria2c did not answer its RPC port within {} s",
                ARIA2_START_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(ARIA2_START_PROBE).await;
    }
}

/// Starts aria2 if needed and returns the canonical download root.
///
/// Holds the process lock across the bounded startup poll so two concurrent
/// requests cannot spawn two daemons on the same port.
async fn ensure_aria2_running(state: &Aria2State) -> Result<PathBuf, String> {
    let mut proc = state.lock().await;

    if let Some(child) = proc.child.as_mut() {
        match child.try_wait() {
            Ok(None) => return Ok(proc.dir.clone()),
            Ok(Some(status)) => {
                tracing::warn!("aria2c exited unexpectedly ({status}); restarting it");
                proc.child = None;
            }
            Err(e) => {
                tracing::warn!("could not poll aria2c ({e}); restarting it");
                proc.child = None;
            }
        }
    }

    let aria2_path = find_aria2().await.ok_or_else(|| {
        "aria2c not found on system. Install with: sudo apt install aria2".to_string()
    })?;

    let root = crate::config::paths::downloader_dir();
    tokio::fs::create_dir_all(&root).await.map_err(|e| {
        format!(
            "Could not create the download folder {}: {e}",
            root.display()
        )
    })?;
    let root = tokio::fs::canonicalize(&root).await.map_err(|e| {
        format!(
            "Could not resolve the download folder {}: {e}",
            root.display()
        )
    })?;
    proc.dir = root.clone();

    let conf_path = write_rpc_config(proc.rpc_secret()).await?;

    let mut child = tokio::process::Command::new(&aria2_path)
        .args([
            format!("--conf-path={}", conf_path.display()),
            "--enable-rpc".to_string(),
            "--rpc-listen-all=false".to_string(),
            "--rpc-listen-port=6800".to_string(),
            "--rpc-max-request-size=2M".to_string(),
            format!("--dir={}", root.display()),
            "--continue=true".to_string(),
            "--max-connection-per-server=4".to_string(),
            "--split=4".to_string(),
            "--min-split-size=1M".to_string(),
            "--console-log-level=warn".to_string(),
            "--daemon=false".to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start aria2c: {}", e))?;

    if let Err(e) = wait_until_ready(proc.rpc_secret(), &mut child).await {
        // Never leave a half-started daemon behind: it would hold the RPC port
        // and the next start would silently talk to it.
        let _ = child.kill().await;
        let _ = child.wait().await;
        proc.child = None;
        return Err(e);
    }

    proc.child = Some(child);
    Ok(root)
}

pub async fn handle_downloader_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<serde_json::Value>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }

    let available = find_aria2().await.is_some();
    let running = aria2_is_running(&state.downloader).await;
    let dir = state.downloader.lock().await.dir.display().to_string();

    Json(ApiResponse::success(serde_json::json!({
        "available": available,
        "running": running,
        "dir": dir
    })))
}

pub async fn handle_downloader_start(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    match ensure_aria2_running(&state.downloader).await {
        Ok(dir) => Json(ApiResponse::success(dir.display().to_string())),
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_downloader_stop(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    let mut proc = state.downloader.lock().await;
    if let Some(mut child) = proc.child.take() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        Json(ApiResponse::success("aria2c stopped".to_string()))
    } else {
        Json(ApiResponse::error("aria2c is not running"))
    }
}

pub async fn handle_downloader_global_stat(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Aria2GlobalStat>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc(&state.downloader, "aria2.getGlobalStat", vec![]).await {
        Ok(val) => match serde_json::from_value::<Aria2GlobalStat>(val) {
            Ok(stat) => Json(ApiResponse::success(stat)),
            Err(e) => Json(ApiResponse::error(&format!("parse error: {}", e))),
        },
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_downloader_tasks(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Aria2Task>>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    // aria2's JSON-RPC API has no single "list everything" method — despite
    // its name, `aria2.tellAll` does not exist (confirmed against a real
    // aria2c via `system.listMethods`, and calling it returns "No such
    // method"), so this always failed and the task list was permanently
    // empty. The real API splits it into three calls by status. Track
    // whether any of them actually reached aria2 so a real RPC outage still
    // surfaces as an error instead of silently looking like "no downloads".
    let mut tasks: Vec<Aria2Task> = Vec::new();
    let mut last_error: Option<String> = None;
    let mut any_succeeded = false;

    match aria2_rpc(&state.downloader, "aria2.tellActive", vec![]).await {
        Ok(active) => {
            any_succeeded = true;
            if let Ok(mut active_tasks) = serde_json::from_value::<Vec<Aria2Task>>(active) {
                tasks.append(&mut active_tasks);
            }
        }
        Err(e) => last_error = Some(e),
    }

    match aria2_rpc(
        &state.downloader,
        "aria2.tellWaiting",
        vec![serde_json::json!(0), serde_json::json!(50)],
    )
    .await
    {
        Ok(waiting) => {
            any_succeeded = true;
            if let Ok(mut waiting_tasks) = serde_json::from_value::<Vec<Aria2Task>>(waiting) {
                tasks.append(&mut waiting_tasks);
            }
        }
        Err(e) => last_error = Some(e),
    }

    match aria2_rpc(
        &state.downloader,
        "aria2.tellStopped",
        vec![serde_json::json!(0), serde_json::json!(50)],
    )
    .await
    {
        Ok(stopped) => {
            any_succeeded = true;
            if let Ok(mut stopped_tasks) = serde_json::from_value::<Vec<Aria2Task>>(stopped) {
                tasks.append(&mut stopped_tasks);
            }
        }
        Err(e) => last_error = Some(e),
    }

    if !any_succeeded {
        if let Some(e) = last_error {
            return Json(ApiResponse::error(&e));
        }
    }

    Json(ApiResponse::success(tasks))
}

pub async fn handle_downloader_add(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<AddUriRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };

    // Validate everything before the daemon is touched: a refused request must
    // not spawn aria2 as a side effect (ROADMAP 17.1).
    let uri = match validate_uri(&req.uri) {
        Ok(uri) => uri,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let subfolder = match validate_subfolder(req.dir.as_deref().unwrap_or("")) {
        Ok(sub) => sub,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let out = match validate_output_name(req.out.as_deref().unwrap_or("")) {
        Ok(out) => out,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let max_download = match validate_rate_limit(
        "download limit",
        req.max_download_limit.as_deref().unwrap_or(""),
    ) {
        Ok(value) => value,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let max_upload = match validate_rate_limit(
        "upload limit",
        req.max_upload_limit.as_deref().unwrap_or(""),
    ) {
        Ok(value) => value,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let seed_ratio = match validate_seed_ratio(req.seed_ratio.as_deref().unwrap_or("")) {
        Ok(value) => value,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let root = match ensure_aria2_running(&state.downloader).await {
        Ok(dir) => dir,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // `dir` is always sent explicitly, so aria2 can never fall back to its own
    // working directory or to a path the request supplied.
    let dir = match resolve_download_dir(&root, subfolder.as_deref()) {
        Ok(dir) => dir,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let mut options = serde_json::json!({ "dir": dir.display().to_string() });
    if let Some(out) = &out {
        options["out"] = serde_json::json!(out);
    }
    if let Some(limit) = &max_download {
        options["max-download-limit"] = serde_json::json!(limit);
    }
    if let Some(limit) = &max_upload {
        options["max-upload-limit"] = serde_json::json!(limit);
    }
    if let Some(ratio) = &seed_ratio {
        options["seed-ratio"] = serde_json::json!(ratio);
    }

    let params = vec![serde_json::json!([uri]), options];

    match aria2_rpc(&state.downloader, "aria2.addUri", params).await {
        Ok(val) => {
            let gid = val.as_str().unwrap_or("unknown").to_string();
            crate::events::log_event(format!(
                "Admin '{}' added a download from '{}'",
                admin_username,
                redact_userinfo(&uri)
            ));
            Json(ApiResponse::success(gid))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_downloader_pause(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<GidRequest>,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc(
        &state.downloader,
        "aria2.pause",
        vec![serde_json::json!(req.gid)],
    )
    .await
    {
        Ok(val) => {
            let gid = val.as_str().unwrap_or("unknown").to_string();
            Json(ApiResponse::success(gid))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_downloader_unpause(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<GidRequest>,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc(
        &state.downloader,
        "aria2.unpause",
        vec![serde_json::json!(req.gid)],
    )
    .await
    {
        Ok(val) => {
            let gid = val.as_str().unwrap_or("unknown").to_string();
            Json(ApiResponse::success(gid))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_downloader_remove(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<GidRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }
    crate::events::log_event(format!(
        "Admin '{}' removed download '{}'",
        admin_username, req.gid
    ));

    // `aria2.remove`/`aria2.forceRemove` only operate on active or waiting
    // downloads — they fail with "Active Download not found" for anything
    // already in the stopped/complete/error list, which is the most common
    // case a user actually wants to clear (the "Remove" button on a
    // finished download in the UI). `aria2.removeDownloadResult` is the
    // separate RPC method aria2 requires for that case, so it's tried last.
    match aria2_rpc(
        &state.downloader,
        "aria2.remove",
        vec![serde_json::json!(req.gid)],
    )
    .await
    {
        Ok(val) => {
            let gid = val.as_str().unwrap_or("unknown").to_string();
            Json(ApiResponse::success(gid))
        }
        Err(_) => match aria2_rpc(
            &state.downloader,
            "aria2.forceRemove",
            vec![serde_json::json!(req.gid)],
        )
        .await
        {
            Ok(val) => {
                let gid = val.as_str().unwrap_or("unknown").to_string();
                Json(ApiResponse::success(gid))
            }
            Err(_) => match aria2_rpc(
                &state.downloader,
                "aria2.removeDownloadResult",
                vec![serde_json::json!(req.gid)],
            )
            .await
            {
                Ok(_) => Json(ApiResponse::success(req.gid.clone())),
                Err(e) => Json(ApiResponse::error(&e)),
            },
        },
    }
}

pub async fn handle_downloader_task(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(gid): Path<String>,
) -> Json<ApiResponse<Aria2Task>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc(
        &state.downloader,
        "aria2.tellStatus",
        vec![serde_json::json!(gid)],
    )
    .await
    {
        Ok(val) => match serde_json::from_value::<Aria2Task>(val) {
            Ok(task) => Json(ApiResponse::success(task)),
            Err(e) => Json(ApiResponse::error(&format!("parse error: {}", e))),
        },
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Guards an admin-only Downloader request.
///
/// The decision — token present, token valid, account admin, and the status
/// each failure carries — belongs to `admin_guard` (ROADMAP X.7). The module
/// answers a `Json<ApiResponse>` envelope, so the guard's status is relayed as
/// a number and its message re-derived from that status.
async fn guard_admin(state: &AppStateArc, headers: &HeaderMap) -> Result<String, (u16, String)> {
    match crate::handlers::admin_guard::check_admin(state, headers).await {
        Ok(username) => Ok(username),
        Err(reason) => Err((reason.status().as_u16(), reason.message().to_string())),
    }
}

// ---------------------------------------------------------------------------
// Per-task limits, automatic extraction and RSS subscriptions (P1, 2026-11-09)
// ---------------------------------------------------------------------------

/// Queue one already-validated link in the download root.
///
/// Shared by the RSS refresh and the scheduler, so both go through the same
/// `validate_uri` + `resolve_download_dir` gate the manual add uses.
async fn queue_uri(state: &AppStateArc, uri: &str) -> Result<String, String> {
    let uri = validate_uri(uri)?;
    let root = ensure_aria2_running(&state.downloader).await?;
    let dir = resolve_download_dir(&root, None)?;
    let options = serde_json::json!({ "dir": dir.display().to_string() });
    let params = vec![serde_json::json!([uri]), options];
    let value = aria2_rpc(&state.downloader, "aria2.addUri", params).await?;
    Ok(value.as_str().unwrap_or("unknown").to_string())
}

/// `GET /api/downloader/settings` — the extraction preference.
pub async fn handle_downloader_settings_get(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<crate::downloader_extract::DownloaderSettings>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    let settings = tokio::task::spawn_blocking(crate::downloader_extract::load_settings).await;
    match settings {
        Ok(settings) => Json(ApiResponse::success(settings)),
        Err(e) => Json(ApiResponse::error(&format!("Settings task failed: {e}"))),
    }
}

#[derive(Deserialize)]
pub struct DownloaderSettingsRequest {
    #[serde(default)]
    pub auto_extract: bool,
}

/// `PUT /api/downloader/settings` — turn automatic extraction on or off.
pub async fn handle_downloader_settings_update(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<DownloaderSettingsRequest>,
) -> Json<ApiResponse<crate::downloader_extract::DownloaderSettings>> {
    let admin = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    let settings = crate::downloader_extract::DownloaderSettings {
        auto_extract: req.auto_extract,
    };
    let saved = settings.clone();
    let result =
        tokio::task::spawn_blocking(move || crate::downloader_extract::save_settings(&saved)).await;
    match result {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{}' turned automatic extraction {}",
                admin,
                if settings.auto_extract { "on" } else { "off" }
            ));
            Json(ApiResponse::success(settings))
        }
        Ok(Err(e)) => Json(ApiResponse::error(&e)),
        Err(e) => Json(ApiResponse::error(&format!("Settings task failed: {e}"))),
    }
}

/// The folder a finished task wrote into: the parent of its first file, else
/// the task's own `dir`. Never a path from a request.
fn task_folder(task: &Aria2Task) -> PathBuf {
    task.files
        .first()
        .and_then(|file| FsPath::new(&file.path).parent().map(|p| p.to_path_buf()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from(&task.dir))
}

/// `POST /api/downloader/extract` — unpack the archives a task left behind.
pub async fn handle_downloader_extract(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<GidRequest>,
) -> Json<ApiResponse<Vec<String>>> {
    let admin = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    let gid = req.gid.trim().to_string();
    if gid.is_empty() || gid.len() > 64 || !gid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Json(ApiResponse::error("Invalid download id"));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }
    let task = match aria2_rpc(
        &state.downloader,
        "aria2.tellStatus",
        vec![serde_json::json!(gid)],
    )
    .await
    {
        Ok(value) => match serde_json::from_value::<Aria2Task>(value) {
            Ok(task) => task,
            Err(e) => return Json(ApiResponse::error(&format!("parse error: {e}"))),
        },
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    if task.status != "complete" {
        return Json(ApiResponse::error(
            "Only a finished download can be extracted",
        ));
    }
    let folder = task_folder(&task);
    let extracted = tokio::task::spawn_blocking(move || {
        crate::downloader_extract::extract_archives_in(&folder)
    })
    .await;
    match extracted {
        Ok(Ok(extracted)) => {
            if !extracted.is_empty() {
                crate::events::log_event(format!(
                    "Admin '{admin}' extracted {} archive(s) from download {gid}",
                    extracted.len()
                ));
            }
            Json(ApiResponse::success(extracted))
        }
        Ok(Err(e)) => Json(ApiResponse::error(&e)),
        Err(e) => Json(ApiResponse::error(&format!("Extract task failed: {e}"))),
    }
}

/// Unpack the archives of every finished download not handled yet.
///
/// Runs from [`run_extract_loop`] while `auto_extract` is on, and never
/// spawns aria2 — a daemon that is not running has no finished downloads.
async fn extract_finished(state: &AppStateArc) -> Result<usize, String> {
    let stopped = aria2_rpc(
        &state.downloader,
        "aria2.tellStopped",
        vec![serde_json::json!(0), serde_json::json!(50)],
    )
    .await?;
    let tasks: Vec<Aria2Task> =
        serde_json::from_value(stopped).map_err(|e| format!("parse error: {e}"))?;

    let mut processed = crate::downloader_extract::load_processed();
    let mut handled = 0usize;
    for task in tasks.iter().filter(|task| task.status == "complete") {
        if processed.gids.iter().any(|gid| gid == &task.gid) {
            continue;
        }
        let folder = task_folder(task);
        let gid = task.gid.clone();
        let extracted = tokio::task::spawn_blocking(move || {
            crate::downloader_extract::extract_archives_in(&folder)
        })
        .await
        .map_err(|e| format!("Extract task failed: {e}"))?;
        match extracted {
            Ok(names) if !names.is_empty() => {
                crate::events::log_event(format!(
                    "Downloader extracted {} archive(s) from download {gid}: {}",
                    names.len(),
                    names.join(", ")
                ));
            }
            Ok(_) => {}
            Err(e) => {
                // Recorded once instead of retried forever: a broken archive
                // would otherwise fail every 20 seconds.
                crate::events::log_event_warn(format!(
                    "Downloader could not extract download {gid}: {e}"
                ));
            }
        }
        processed.gids.push(gid);
        handled += 1;
    }
    if handled > 0 {
        crate::downloader_extract::save_processed(processed)?;
    }
    Ok(handled)
}

/// Background pass: while automatic extraction is on, unpack what finished.
pub async fn run_extract_loop(state: AppStateArc) {
    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
        if !crate::downloader_extract::load_settings().auto_extract {
            continue;
        }
        let running = { state.downloader.lock().await.child.is_some() };
        if !running {
            continue;
        }
        if let Err(e) = extract_finished(&state).await {
            tracing::debug!("downloader extraction pass: {e}");
        }
    }
}

// -- RSS subscriptions ------------------------------------------------------

/// `GET /api/downloader/feeds` — the subscribed feeds.
pub async fn handle_downloader_feeds_list(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<crate::downloader_rss::RssFeed>>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    match tokio::task::spawn_blocking(crate::downloader_rss::load).await {
        Ok(feeds) => Json(ApiResponse::success(feeds)),
        Err(e) => Json(ApiResponse::error(&format!("Feeds task failed: {e}"))),
    }
}

#[derive(Deserialize)]
pub struct AddFeedRequest {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub auto_download: bool,
}

/// `POST /api/downloader/feeds` — subscribe to a feed.
pub async fn handle_downloader_feed_add(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<AddFeedRequest>,
) -> Json<ApiResponse<crate::downloader_rss::RssFeed>> {
    let admin = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    let url = match crate::downloader_rss::validate_feed_url(&req.url) {
        Ok(url) => url,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let name = match req.name.as_deref().filter(|n| !n.trim().is_empty()) {
        Some(name) => match crate::downloader_rss::validate_feed_name(name) {
            Ok(name) => name,
            Err(e) => return Json(ApiResponse::error(&e)),
        },
        None => crate::downloader_rss::default_feed_name(&url),
    };

    let feed = crate::downloader_rss::RssFeed {
        id: uuid::Uuid::new_v4().simple().to_string(),
        name,
        url: url.clone(),
        enabled: true,
        auto_download: req.auto_download,
        last_checked_at: None,
        last_error: None,
        seen: Vec::new(),
    };
    let to_save = feed.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut feeds = crate::downloader_rss::load();
        if feeds.iter().any(|existing| existing.url == to_save.url) {
            return Err("This feed is already subscribed".to_string());
        }
        feeds.push(to_save);
        crate::downloader_rss::save(&feeds)
    })
    .await;
    match result {
        Ok(Ok(())) => {
            crate::events::log_event(format!(
                "Admin '{}' subscribed to the feed '{}' ({})",
                admin,
                feed.name,
                crate::downloader_rss::default_feed_name(&url)
            ));
            Json(ApiResponse::success(feed))
        }
        Ok(Err(e)) => Json(ApiResponse::error(&e)),
        Err(e) => Json(ApiResponse::error(&format!("Feeds task failed: {e}"))),
    }
}

#[derive(Deserialize)]
pub struct UpdateFeedRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub auto_download: Option<bool>,
}

/// `PUT /api/downloader/feeds/{id}` — rename, enable/disable, auto-download.
pub async fn handle_downloader_feed_update(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateFeedRequest>,
) -> Json<ApiResponse<crate::downloader_rss::RssFeed>> {
    let admin = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    let result = tokio::task::spawn_blocking(move || {
        let mut feeds = crate::downloader_rss::load();
        let Some(feed) = feeds.iter_mut().find(|feed| feed.id == id) else {
            return Err("No such feed".to_string());
        };
        if let Some(name) = req.name.as_deref().filter(|n| !n.trim().is_empty()) {
            feed.name = crate::downloader_rss::validate_feed_name(name)?;
        }
        if let Some(enabled) = req.enabled {
            feed.enabled = enabled;
        }
        if let Some(auto_download) = req.auto_download {
            feed.auto_download = auto_download;
        }
        let updated = feed.clone();
        crate::downloader_rss::save(&feeds)?;
        Ok(updated)
    })
    .await;
    match result {
        Ok(Ok(feed)) => {
            crate::events::log_event(format!(
                "Admin '{}' updated the feed '{}'",
                admin, feed.name
            ));
            Json(ApiResponse::success(feed))
        }
        Ok(Err(e)) => Json(ApiResponse::error(&e)),
        Err(e) => Json(ApiResponse::error(&format!("Feeds task failed: {e}"))),
    }
}

/// `DELETE /api/downloader/feeds/{id}` — unsubscribe.
pub async fn handle_downloader_feed_delete(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = guard_admin(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    let result = tokio::task::spawn_blocking(move || {
        let mut feeds = crate::downloader_rss::load();
        let before = feeds.len();
        feeds.retain(|feed| feed.id != id);
        if feeds.len() == before {
            return Err("No such feed".to_string());
        }
        crate::downloader_rss::save(&feeds)?;
        Ok(())
    })
    .await;
    match result {
        Ok(Ok(())) => Json(ApiResponse::success("Feed removed".to_string())),
        Ok(Err(e)) => Json(ApiResponse::error(&e)),
        Err(e) => Json(ApiResponse::error(&format!("Feeds task failed: {e}"))),
    }
}

#[derive(Serialize)]
pub struct FeedRefreshResult {
    pub feed_id: String,
    pub items: Vec<FeedItemView>,
    pub queued: usize,
    pub errors: Vec<String>,
}

#[derive(Serialize)]
pub struct FeedItemView {
    #[serde(flatten)]
    pub item: crate::downloader_rss::RssItem,
    /// Already queued for this feed (so the UI can say "already downloaded").
    pub seen: bool,
}

/// Fetch one feed, queue what is new when asked, and record the check.
///
/// One code path for the button, the "download all new" action and the
/// scheduled refresh. The seen marker only ever records what was **actually
/// queued**: listing a feed (or a failed queue) leaves the item eligible, so a
/// later scheduled refresh still picks it up.
pub async fn refresh_feed(
    state: &AppStateArc,
    id: &str,
    queue: bool,
) -> Result<FeedRefreshResult, String> {
    let id = id.to_string();
    let feeds = tokio::task::spawn_blocking(crate::downloader_rss::load)
        .await
        .map_err(|e| format!("Feeds task failed: {e}"))?;
    let feed = feeds
        .iter()
        .find(|feed| feed.id == id)
        .cloned()
        .ok_or_else(|| "No such feed".to_string())?;

    let fetched = crate::downloader_rss::fetch(&feed.url).await;
    let (items, error) = match fetched {
        Ok(items) => (items, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let unseen: Vec<crate::downloader_rss::RssItem> = crate::downloader_rss::unseen(&feed, &items)
        .into_iter()
        .cloned()
        .collect();

    let mut queued = 0usize;
    let mut queued_items: Vec<crate::downloader_rss::RssItem> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    if queue && error.is_none() {
        for item in &unseen {
            match queue_uri(state, &item.download_url).await {
                Ok(_) => {
                    queued += 1;
                    queued_items.push(item.clone());
                }
                Err(e) => errors.push(format!("{}: {e}", item.label())),
            }
        }
    }

    // The check time and the error are always recorded; only queued items are
    // marked seen.
    let update_id = feed.id.clone();
    let error_for_store = error.clone();
    let seen_for_store = queued_items.clone();
    tokio::task::spawn_blocking(move || {
        let mut feeds = crate::downloader_rss::load();
        if let Some(stored) = feeds.iter_mut().find(|feed| feed.id == update_id) {
            crate::downloader_rss::mark_seen(stored, &seen_for_store);
            stored.last_checked_at = Some(chrono::Utc::now().to_rfc3339());
            stored.last_error = error_for_store;
            crate::downloader_rss::save(&feeds)?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("Feeds task failed: {e}"))??;

    if let Some(error) = &error {
        return Err(error.clone());
    }

    Ok(FeedRefreshResult {
        feed_id: feed.id,
        items: items
            .into_iter()
            .map(|item| FeedItemView {
                seen: feed.seen.iter().any(|guid| guid == &item.guid)
                    || queued_items.iter().any(|queued| queued.guid == item.guid),
                item,
            })
            .collect(),
        queued,
        errors,
    })
}

/// `POST /api/downloader/feeds/{id}/refresh` — check now, optionally queue.
pub async fn handle_downloader_feed_refresh(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Json<ApiResponse<FeedRefreshResult>> {
    let admin = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    // Body is optional: the button sends nothing and queues nothing; "download
    // all new" sends `{"queue": true}`.
    let queue = if body.is_empty() {
        false
    } else {
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => value
                .get("queue")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            Err(e) => return Json(ApiResponse::error(&format!("Malformed request body: {e}"))),
        }
    };

    match refresh_feed(&state, &id, queue).await {
        Ok(result) => {
            if result.queued > 0 {
                crate::events::log_event(format!(
                    "Admin '{admin}' queued {} item(s) from feed '{}'",
                    result.queued, result.feed_id
                ));
            }
            Json(ApiResponse::success(result))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Refresh every enabled feed, queueing new items when the feed opted in.
/// Shared by the manual "refresh all" action and the scheduled task.
pub async fn refresh_all_feeds(state: &AppStateArc) -> Result<(usize, usize), String> {
    let feeds = tokio::task::spawn_blocking(crate::downloader_rss::load)
        .await
        .map_err(|e| format!("Feeds task failed: {e}"))?;
    let mut queued = 0usize;
    let mut failures = 0usize;
    for feed in feeds.into_iter().filter(|feed| feed.enabled) {
        let auto = feed.auto_download;
        match refresh_feed(state, &feed.id, auto).await {
            Ok(result) => queued += result.queued,
            Err(e) => {
                failures += 1;
                tracing::warn!("feed '{}' failed: {e}", feed.name);
            }
        }
    }
    Ok((queued, failures))
}

/// `POST /api/downloader/feeds/refresh` — check every enabled feed now.
pub async fn handle_downloader_feeds_refresh_all(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<serde_json::Value>> {
    let admin = match guard_admin(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    match refresh_all_feeds(&state).await {
        Ok((queued, failures)) => {
            crate::events::log_event(format!(
                "Admin '{admin}' refreshed all feeds: {queued} item(s) queued, {failures} feed(s) failed"
            ));
            Json(ApiResponse::success(serde_json::json!({
                "queued": queued,
                "failed": failures,
            })))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for the Downloader (aria2) handler module.
    //!
    //! Two groups:
    //!
    //! 1. The **asymmetric serde mapping** between aria2's real JSON-RPC wire
    //!    format (camelCase: `downloadSpeed`, `totalLength`, `errorMessage`,
    //!    ...) and the snake_case JSON this API re-emits to the Svelte
    //!    frontend (`Aria2Task` / `GlobalStat` in `Downloader.svelte`).
    //!    A regression here is exactly the bug the `#[serde(rename(deserialize =
    //!    ...))]` attributes were added to fix (see the module-level comment on
    //!    `Aria2GlobalStat`): a real aria2 response would silently fail to parse
    //!    and every stat/task lookup would come back empty.
    //! 2. The **input-confinement matrix** added for ROADMAP 17.1 — the scheme
    //!    allowlist, the sub-folder and output-name validators, the download-root
    //!    resolver (including a symlink escape), the event-log redaction, and the
    //!    per-process RPC secret. These are the assertions the feature shipped
    //!    without: `uri`, `dir` and `out` used to reach aria2 verbatim.
    //!
    //! No network and no aria2 process — just serde round-trips against fixtures
    //! captured verbatim from a live `aria2c` 1.37.0, and pure validators (the
    //! resolver uses a `tempfile` directory, which is a dev-dependency).
    //!
    //! Run: `cargo test downloader::tests`

    use super::*;

    // -- aria2.getGlobalStat -------------------------------------------------

    #[test]
    fn test_global_stat_deserializes_real_aria2_camelcase() {
        // Captured verbatim from `aria2.getGlobalStat` on a live daemon.
        let raw = r#"{"downloadSpeed":"8384","uploadSpeed":"0","numActive":"1",
            "numWaiting":"2","numStopped":"3","numStoppedTotal":"3"}"#;
        let stat: Aria2GlobalStat = serde_json::from_str(raw).expect("must parse real aria2 JSON");
        assert_eq!(stat.download_speed, "8384");
        assert_eq!(stat.upload_speed, "0");
        assert_eq!(stat.num_active, "1");
        assert_eq!(stat.num_waiting, "2");
        assert_eq!(stat.num_stopped, "3");
    }

    #[test]
    fn test_global_stat_ignores_unknown_aria2_fields() {
        // aria2 also sends `numStoppedTotal`; unknown fields must not break parsing.
        let raw = r#"{"downloadSpeed":"0","uploadSpeed":"0","numActive":"0",
            "numWaiting":"0","numStopped":"0","numStoppedTotal":"1","somethingNew":"x"}"#;
        assert!(serde_json::from_str::<Aria2GlobalStat>(raw).is_ok());
    }

    #[test]
    fn test_global_stat_reserializes_to_snake_case_for_frontend() {
        // The Svelte `GlobalStat` interface expects snake_case keys — the
        // `rename` is deserialize-only, so serialization must stay snake_case.
        let stat = Aria2GlobalStat {
            download_speed: "100".into(),
            upload_speed: "0".into(),
            num_active: "2".into(),
            num_waiting: "0".into(),
            num_stopped: "5".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&stat).unwrap();
        assert_eq!(v["download_speed"], "100");
        assert_eq!(v["num_active"], "2");
        assert_eq!(v["num_stopped"], "5");
        assert!(v.get("downloadSpeed").is_none(), "must not emit camelCase");
    }

    #[test]
    fn test_global_stat_rejects_snake_case_input() {
        // Guards the asymmetry: aria2 never sends snake_case, so snake_case
        // input (a hand-rolled/legacy payload) must fail rather than parse.
        let raw = r#"{"download_speed":"0","upload_speed":"0","num_active":"0",
            "num_waiting":"0","num_stopped":"0"}"#;
        assert!(serde_json::from_str::<Aria2GlobalStat>(raw).is_err());
    }

    // -- aria2.tellStatus / tellActive / tellStopped -----------------------

    fn real_task_json() -> &'static str {
        // Captured verbatim from `aria2.tellStopped` for a completed HTTP
        // download on a live daemon (paths/gid kept as-is).
        r#"{
            "gid":"da9eb6ed9a94ecc2",
            "status":"complete",
            "totalLength":"25966",
            "completedLength":"25966",
            "downloadSpeed":"0",
            "uploadSpeed":"0",
            "errorMessage":"",
            "bittorrent":null,
            "files":[{
                "index":"1",
                "path":"/tmp/hv-dl/hv-01-thumb.webp",
                "length":"25966",
                "completedLength":"25966",
                "selected":"true",
                "uris":[{"uri":"http://127.0.0.1:8080/wallpapers/hv-01-thumb.webp","status":"used"}]
            }],
            "dir":"/tmp/hv-dl"
        }"#
    }

    #[test]
    fn test_task_deserializes_real_aria2_camelcase() {
        let task: Aria2Task = serde_json::from_str(real_task_json()).expect("must parse");
        assert_eq!(task.gid, "da9eb6ed9a94ecc2");
        assert_eq!(task.status, "complete");
        assert_eq!(task.total_length, "25966");
        assert_eq!(task.completed_length, "25966");
        assert_eq!(task.download_speed, "0");
        assert_eq!(task.error_message.as_deref(), Some(""));
        assert!(task.bittorrent.is_none());
        assert_eq!(task.files.len(), 1);
        assert_eq!(task.files[0].completed_length, "25966");
        assert_eq!(task.files[0].uris.len(), 1);
        assert_eq!(task.files[0].uris[0].status, "used");
        assert_eq!(task.dir, "/tmp/hv-dl");
    }

    #[test]
    fn test_task_reserializes_to_snake_case_matching_svelte_interface() {
        // The Svelte `Aria2Task` interface keys: total_length, completed_length,
        // download_speed, upload_speed, error_message, bittorrent, files, dir.
        let task: Aria2Task = serde_json::from_str(real_task_json()).unwrap();
        let v = serde_json::to_value(&task).unwrap();
        for key in [
            "gid",
            "status",
            "total_length",
            "completed_length",
            "download_speed",
            "upload_speed",
            "error_message",
            "bittorrent",
            "files",
            "dir",
        ] {
            assert!(v.get(key).is_some(), "serialized task missing `{key}`");
        }
        assert!(
            v.get("totalLength").is_none(),
            "must not leak camelCase to the frontend"
        );
        assert_eq!(v["files"][0]["completed_length"], "25966");
        assert!(v["files"][0].get("completedLength").is_none());
    }

    #[test]
    fn test_task_with_bittorrent_parses_name() {
        // A magnet/torrent task carries a `bittorrent.name` used as the display
        // name in the UI (`fileName()` in Downloader.svelte).
        let raw = r#"{
            "gid":"abcdef0123456789","status":"active",
            "totalLength":"1000","completedLength":"500",
            "downloadSpeed":"250","uploadSpeed":"10","errorMessage":null,
            "bittorrent":{"name":"ubuntu-24.04.iso"},
            "files":[],"dir":"/downloads"
        }"#;
        let task: Aria2Task = serde_json::from_str(raw).expect("must parse a torrent task");
        assert_eq!(
            task.bittorrent.as_ref().map(|b| b.name.as_str()),
            Some("ubuntu-24.04.iso")
        );
        assert_eq!(task.error_message, None);
    }

    #[test]
    fn test_task_tolerates_missing_optional_error_message() {
        // `errorMessage` is absent entirely on healthy downloads — Option field.
        let raw = r#"{
            "gid":"0000000000000001","status":"active",
            "totalLength":"0","completedLength":"0",
            "downloadSpeed":"0","uploadSpeed":"0",
            "bittorrent":null,"files":[],"dir":"/x"
        }"#;
        let task: Aria2Task = serde_json::from_str(raw).expect("must parse without errorMessage");
        assert_eq!(task.error_message, None);
    }

    #[test]
    fn test_task_list_deserializes_as_vec() {
        // `tellActive`/`tellWaiting`/`tellStopped` each return a JSON array;
        // the handler appends `Vec<Aria2Task>` from all three.
        let raw = format!("[{0},{0}]", real_task_json());
        let tasks: Vec<Aria2Task> = serde_json::from_str(&raw).expect("must parse a task array");
        assert_eq!(tasks.len(), 2);
    }

    // -- request bodies ---------------------------------------------------

    #[test]
    fn test_add_uri_request_uri_only() {
        let req: AddUriRequest =
            serde_json::from_str(r#"{"uri":"https://example.com/f.bin"}"#).unwrap();
        assert_eq!(req.uri, "https://example.com/f.bin");
        assert!(req.dir.is_none() && req.out.is_none());
    }

    #[test]
    fn test_add_uri_request_with_dir_and_out() {
        let req: AddUriRequest = serde_json::from_str(
            r#"{"uri":"magnet:?xt=urn:btih:abc","dir":"/data/dl","out":"movie.mkv"}"#,
        )
        .unwrap();
        assert_eq!(req.dir.as_deref(), Some("/data/dl"));
        assert_eq!(req.out.as_deref(), Some("movie.mkv"));
    }

    #[test]
    fn test_add_uri_request_missing_uri_is_rejected() {
        assert!(serde_json::from_str::<AddUriRequest>(r#"{"dir":"/data/dl"}"#).is_err());
    }

    #[test]
    fn test_gid_request_roundtrip() {
        let req: GidRequest = serde_json::from_str(r#"{"gid":"da9eb6ed9a94ecc2"}"#).unwrap();
        assert_eq!(req.gid, "da9eb6ed9a94ecc2");
        assert!(serde_json::from_str::<GidRequest>(r#"{}"#).is_err());
    }

    // -- ROADMAP 17.1: link scheme allowlist ------------------------------

    #[test]
    fn test_validate_uri_accepts_the_supported_schemes() {
        for uri in [
            "https://example.com/file.iso",
            "http://192.168.1.10:8080/share/a.bin",
            "ftp://mirror.example.org/pub/img.img",
            "sftp://nas.example.org/export/a.bin",
            "magnet:?xt=urn:btih:0123456789abcdef&dn=ubuntu.iso",
        ] {
            assert_eq!(
                validate_uri(uri).as_deref(),
                Ok(uri),
                "`{uri}` must be accepted"
            );
        }
    }

    #[test]
    fn test_validate_uri_refuses_local_read_and_injection_schemes() {
        // `file://` is the local-read primitive ROADMAP 17.1 names, the rest are
        // the same class: a scheme aria2 would happily hand to a local handler.
        for uri in [
            "file:///etc/shadow",
            "FILE:///etc/shadow",
            "data:text/plain;base64,SGVsbG8=",
            "javascript:alert(1)",
            "vbscript:msgbox(1)",
            "smb://nas/secret",
            "ssh://host/etc",
            "/etc/passwd",
            "etc/passwd",
        ] {
            assert!(
                validate_uri(uri).is_err(),
                "`{uri}` must be refused, got {:?}",
                validate_uri(uri)
            );
        }
    }

    #[test]
    fn test_validate_uri_refuses_empty_whitespace_and_control_characters() {
        assert!(validate_uri("").is_err());
        assert!(validate_uri("   ").is_err());
        assert!(
            validate_uri("http://example.com/a b").is_err(),
            "a space splits the link"
        );
        assert!(validate_uri("http://example.com/a\nb").is_err());
        assert!(validate_uri("http://example.com/a\u{0}b").is_err());
    }

    #[test]
    fn test_validate_uri_refuses_an_over_long_link() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URI_LEN));
        assert!(long.len() > MAX_URI_LEN);
        assert!(validate_uri(&long).is_err());
    }

    #[test]
    fn test_validate_uri_requires_a_host_for_plain_schemes() {
        for uri in ["http://", "https://", "http:/example.com/a", "ftp://"] {
            assert!(validate_uri(uri).is_err(), "`{uri}` has no host");
        }
    }

    #[test]
    fn test_validate_uri_requires_xt_on_magnet_links() {
        assert!(validate_uri("magnet:?dn=ubuntu.iso").is_err());
        assert!(validate_uri("magnet:?xt=urn:btih:abc").is_ok());
    }

    #[test]
    fn test_validate_uri_trims_surrounding_whitespace() {
        assert_eq!(
            validate_uri("  https://example.com/a.bin \n").as_deref(),
            Ok("https://example.com/a.bin")
        );
    }

    // -- ROADMAP 17.1: destination confinement ----------------------------

    #[test]
    fn test_validate_subfolder_accepts_relative_paths() {
        assert_eq!(validate_subfolder("").unwrap(), None);
        assert_eq!(validate_subfolder("   ").unwrap(), None);
        assert_eq!(
            validate_subfolder("films").unwrap().as_deref(),
            Some("films")
        );
        assert_eq!(
            validate_subfolder("films/2026/octobre").unwrap().as_deref(),
            Some("films/2026/octobre")
        );
        assert_eq!(
            validate_subfolder("  films  ").unwrap().as_deref(),
            Some("films")
        );
    }

    #[test]
    fn test_validate_subfolder_refuses_absolute_and_traversal_paths() {
        for sub in [
            "/etc",
            "/var/lib/homevault/downloads/../../etc",
            "..",
            "../etc",
            "films/../../etc",
            "films/./x",
            "films//x",
            "films\\x",
            "C:\\Windows",
        ] {
            assert!(
                validate_subfolder(sub).is_err(),
                "`{sub}` must be refused, got {:?}",
                validate_subfolder(sub)
            );
        }
    }

    #[test]
    fn test_validate_subfolder_refuses_control_characters_and_oversize() {
        assert!(validate_subfolder("films\nx").is_err());
        assert!(validate_subfolder(&"a".repeat(MAX_SUBFOLDER_LEN + 1)).is_err());
    }

    #[test]
    fn test_validate_output_name_accepts_a_plain_file_name() {
        assert_eq!(validate_output_name("").unwrap(), None);
        assert_eq!(
            validate_output_name("movie.mkv").unwrap().as_deref(),
            Some("movie.mkv")
        );
        assert_eq!(
            validate_output_name("  movie.mkv  ").unwrap().as_deref(),
            Some("movie.mkv")
        );
    }

    #[test]
    fn test_validate_output_name_refuses_a_path() {
        for out in [
            "../etc/passwd",
            "a/b",
            "/etc/passwd",
            "a\\b",
            "..",
            ".",
            "a\u{0}b",
        ] {
            assert!(
                validate_output_name(out).is_err(),
                "`{out}` must be refused, got {:?}",
                validate_output_name(out)
            );
        }
        assert!(validate_output_name(&"a".repeat(MAX_OUT_LEN + 1)).is_err());
    }

    #[test]
    fn test_resolve_download_dir_confines_every_path_to_the_root() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let root = tmp.path().join("downloads");
        std::fs::create_dir_all(&root).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();

        // No sub-folder → the root itself.
        assert_eq!(resolve_download_dir(&root, None).unwrap(), canonical_root);
        assert_eq!(
            resolve_download_dir(&root, Some("")).unwrap(),
            canonical_root
        );

        // A relative sub-folder → under the root.
        assert_eq!(
            resolve_download_dir(&root, Some("films/2026")).unwrap(),
            canonical_root.join("films/2026")
        );

        // Absolute and traversal forms never resolve.
        for sub in ["/etc", "../etc", "films/../../etc"] {
            assert!(
                resolve_download_dir(&root, Some(sub)).is_err(),
                "`{sub}` must not resolve"
            );
        }
    }

    #[test]
    fn test_resolve_download_dir_refuses_a_symlink_that_leaves_the_root() {
        // A symlink planted inside the download folder must not redirect a
        // download outside it.
        let tmp = tempfile::tempdir().expect("temp dir");
        let root = tmp.path().join("downloads");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        let err = resolve_download_dir(&root, Some("escape")).expect_err("must refuse");
        assert!(
            err.contains("inside the download folder"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn test_resolve_download_dir_reports_a_missing_root() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let missing = tmp.path().join("does-not-exist");
        assert!(resolve_download_dir(&missing, None).is_err());
    }

    // -- ROADMAP 17.1: event-log redaction --------------------------------

    #[test]
    fn test_redact_userinfo_strips_embedded_credentials() {
        assert_eq!(
            redact_userinfo("https://alice:s3cr3t@example.com/a.bin?x=1"),
            "https://***@example.com/a.bin?x=1"
        );
        assert_eq!(
            redact_userinfo("ftp://user@mirror.example.org/pub"),
            "ftp://***@mirror.example.org/pub"
        );
        // Nothing to redact.
        assert_eq!(
            redact_userinfo("https://example.com/a.bin"),
            "https://example.com/a.bin"
        );
        assert_eq!(
            redact_userinfo("magnet:?xt=urn:btih:abc"),
            "magnet:?xt=urn:btih:abc"
        );
    }

    // -- ROADMAP 17.2: per-process RPC secret -----------------------------

    #[test]
    fn test_rpc_secret_is_generated_per_process_and_is_not_the_default() {
        let a = Aria2Process::new();
        let b = Aria2Process::new();
        assert!(a.rpc_secret().starts_with("hv_"), "secret shape changed");
        assert_eq!(a.rpc_secret().len(), 3 + 32, "128 bits of hex expected");
        assert_ne!(
            a.rpc_secret(),
            b.rpc_secret(),
            "each start needs its own secret"
        );
        assert!(a.child.is_none());
    }

    // -- ApiResponse envelope (the shape the frontend checks) ------------

    #[test]
    fn test_api_response_success_shape() {
        let v = serde_json::to_value(ApiResponse::success("gid123".to_string())).unwrap();
        assert_eq!(v["success"], true);
        assert_eq!(v["data"], "gid123");
        assert_eq!(v["error"], serde_json::Value::Null);
    }

    #[test]
    fn test_api_response_error_shape() {
        let v = serde_json::to_value(ApiResponse::<String>::error("boom")).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["data"], serde_json::Value::Null);
        assert_eq!(v["error"], "boom");
    }

    // -- per-task limits (P1) ------------------------------------------------

    #[test]
    fn test_rate_limit_accepts_aria2_syntax_only() {
        // Empty means "no per-task limit": the global setting applies.
        assert_eq!(validate_rate_limit("download limit", "").unwrap(), None);
        assert_eq!(
            validate_rate_limit("download limit", " 500k ").unwrap(),
            Some("500K".to_string())
        );
        assert_eq!(
            validate_rate_limit("download limit", "2M").unwrap(),
            Some("2M".to_string())
        );
        assert_eq!(
            validate_rate_limit("download limit", "0").unwrap(),
            Some("0".to_string())
        );
        // aria2 would silently ignore, or fail on, all of these.
        for bad in ["fast", "-5", "5T", "1.5M", "1 G", "1234567890K"] {
            assert!(
                validate_rate_limit("download limit", bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn test_seed_ratio_range() {
        assert_eq!(validate_seed_ratio("").unwrap(), None);
        assert_eq!(validate_seed_ratio("0").unwrap(), Some("0".to_string()));
        assert_eq!(validate_seed_ratio("1.5").unwrap(), Some("1.5".to_string()));
        assert_eq!(validate_seed_ratio("100").unwrap(), Some("100".to_string()));
        for bad in ["200", "-1", "abc", "1.5.0", "1000000"] {
            assert!(validate_seed_ratio(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
