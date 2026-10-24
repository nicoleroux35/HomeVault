/* =============================================================================
 * File          : syncthing.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Syncthing integration endpoints: service status, configuration read and
 *   update, device and folder management, connection state, rescan,
 *   pause/resume per device, restart, and error listing. Every endpoint is
 *   admin-only; calls are proxied to the local Syncthing REST API.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - reqwest
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/syncthing in src/web/mod.rs; the Cloud Sync desktop app
 *   consumes them.
 * =============================================================================
 */

//! Syncthing integration endpoints (proxying the local REST API) — admin-only.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue},
    Json,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::handlers::AppStateArc;

const SYNCTHING_ADDR: &str = "http://127.0.0.1:8384";

/// Bound on every request to the local daemon. Syncthing answers on loopback,
/// so a reply that takes longer than this means the daemon is wedged — there is
/// nothing to wait for.
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Prefix of every error produced while reading or applying the daemon's API
/// key. It lets a caller tell a *configuration* fault (fix `config.xml`) from a
/// *transport* fault (the daemon is not running) without parsing a free-form
/// sentence — and it is what the screen keys its banner on.
const KEY_ERROR: &str = "Syncthing API key";

/// Require a valid, authenticated **admin** session. Every handler in this
/// file had no auth check at all before this fix — not even a "header is
/// non-empty" check — so anyone who could reach the server could enumerate
/// every paired device's ID/IP address and every synced folder's path, and
/// could restart the Syncthing daemon or pause/resume any device outright.
/// Device topology and sync control are privileged, same reasoning as VPN.
async fn require_admin(state: &AppStateArc, headers: &HeaderMap) -> Result<String, String> {
    let token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if token.is_empty() {
        return Err("No session token provided".to_string());
    }

    let Some(username) = state.auth.get_username_from_token(token).await else {
        return Err("Invalid or expired session".to_string());
    };

    if !state.auth.is_admin(&username).await {
        return Err("Admin privileges required for this operation".to_string());
    }

    Ok(username)
}

/// Response wrapper
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }
    fn error(msg: &str) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Models matching Syncthing REST API
// ---------------------------------------------------------------------------

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingStatus {
    pub available: bool,
    pub version: Option<String>,
    pub my_id: Option<String>,
    pub uptime: Option<u64>,
    pub last_seen: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingSystemStatus {
    #[serde(alias = "myID")]
    pub my_id: String,
    #[serde(alias = "uptimeSeconds")]
    pub uptime_seconds: Option<u64>,
    #[serde(alias = "lastSeen")]
    pub last_seen: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingVersion {
    pub version: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingConfig {
    pub folders: Vec<SyncthingFolder>,
    pub devices: Vec<SyncthingDevice>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingFolder {
    pub id: String,
    pub label: String,
    pub path: String,
    #[serde(alias = "type")]
    pub folder_type: String,
    pub devices: Vec<FolderDevice>,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct FolderDevice {
    #[serde(alias = "deviceID")]
    pub device_id: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingDevice {
    #[serde(alias = "deviceID")]
    pub device_id: String,
    pub name: String,
    pub addresses: Vec<String>,
    pub compression: String,
    #[serde(alias = "certName")]
    pub cert_name: String,
    pub introducer: bool,
    pub paused: bool,
    #[serde(default)]
    pub introduced_by: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ConnectionsResponse {
    pub connections: HashMap<String, DeviceConnection>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DeviceConnection {
    pub address: String,
    #[serde(alias = "at")]
    pub connected_at: String,
    #[serde(alias = "clientVersion")]
    pub client_version: String,
    pub connected: bool,
    pub paused: bool,
    #[serde(alias = "inBytesTotal")]
    pub in_bytes_total: u64,
    #[serde(alias = "outBytesTotal")]
    pub out_bytes_total: u64,
    #[serde(alias = "completion")]
    pub completion: Option<u64>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingDeviceWithStatus {
    #[serde(flatten)]
    pub device: SyncthingDevice,
    pub connected: bool,
    pub address: Option<String>,
    pub client_version: Option<String>,
    pub completion: f64,
    pub in_bytes_total: u64,
    pub out_bytes_total: u64,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct FolderStatus {
    pub id: String,
    pub label: String,
    pub path: String,
    pub paused: bool,
    pub global_bytes: u64,
    pub global_files: u64,
    pub local_bytes: u64,
    pub local_files: u64,
    pub need_bytes: u64,
    pub need_files: u64,
    pub in_sync_files: u64,
    pub receive_only: bool,
    pub devices: Vec<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct FolderCompletion {
    pub completion: f64,
    #[serde(alias = "needBytes")]
    pub need_bytes: u64,
    #[serde(alias = "needItems")]
    pub need_items: u64,
    #[serde(alias = "globalBytes")]
    pub global_bytes: u64,
}

// `GET /rest/db/status` has no "list every folder" form — it requires a
// `folder` query parameter and returns stats for that one folder as a flat
// object (confirmed against a real syncthing instance: with no `folder`
// param it 404s with "no such folder"). The response also has no `id`,
// `label`, `path`, or `receiveOnly` fields at all (those come from
// `system/config`, which `handle_syncthing_folders` below already has from
// the outer loop) — this struct only models the stats fields that are
// actually present.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DbFolderStatus {
    #[serde(alias = "globalBytes")]
    pub global_bytes: u64,
    #[serde(alias = "globalFiles")]
    pub global_files: u64,
    #[serde(alias = "localBytes")]
    pub local_bytes: u64,
    #[serde(alias = "localFiles")]
    pub local_files: u64,
    #[serde(alias = "needBytes")]
    pub need_bytes: u64,
    #[serde(alias = "needFiles")]
    pub need_files: u64,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SyncthingErrors {
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_syncthing_config() -> Option<String> {
    let paths: Vec<String> = vec![
        crate::config::paths::syncthing_config_path()
            .to_string_lossy()
            .into_owned(),
        "/root/.config/syncthing/config.xml".to_string(),
        "/var/lib/syncthing/.config/syncthing/config.xml".to_string(),
        "/etc/syncthing/config.xml".to_string(),
    ];
    for p in &paths {
        if std::path::Path::new(p).exists() {
            return std::fs::read_to_string(p).ok();
        }
    }
    None
}

fn extract_api_key(config_xml: &str) -> Option<String> {
    // Look for <apikey>...</apikey>
    let start = config_xml.find("<apikey>")?;
    let value_start = start + "<apikey>".len();
    let end = config_xml[value_start..].find("</apikey>")?;
    Some(config_xml[value_start..value_start + end].to_string())
}

/// Read the daemon's API key off disk. Blocking I/O — callers must go through
/// [`read_api_key`], never straight from an async handler body.
fn get_api_key() -> Option<String> {
    let xml = find_syncthing_config()?;
    extract_api_key(&xml)
}

/// Turn the raw `<apikey>` text read from `config.xml` into an HTTP header
/// value.
///
/// The key comes from a file an administrator can edit by hand, so it is
/// untrusted input, and `HeaderValue::from_str(..).unwrap()` used to sit here:
/// a key holding a control character or a non-ASCII byte **panicked the request
/// task** instead of answering with an error. With `panic = "abort"` in the
/// release profile that is not a failed request, it is a dead appliance. Every
/// outcome is a `Result` now.
///
/// The value is trimmed first: XML pretty-printing indents the element
/// (`<apikey>\n  abc\n</apikey>`), and that leading newline alone is enough to
/// make a header value invalid — so the trim is not cosmetic, it is the fix for
/// the most likely way a hand-edited config breaks.
///
/// `Ok(None)` means "no key configured" (blank element) and sends no header,
/// which is what a daemon with the GUI API key disabled expects. The error
/// message never contains the key.
fn api_key_header(raw: &str) -> Result<Option<HeaderValue>, String> {
    let key = raw.trim();
    if key.is_empty() {
        return Ok(None);
    }
    HeaderValue::from_str(key).map(Some).map_err(|_| {
        format!(
            "{KEY_ERROR} in config.xml is not a valid HTTP header value (it contains \
             control characters or non-ASCII bytes); regenerate it in Syncthing's \
             GUI (Actions → Settings → GUI) or remove the <apikey> line"
        )
    })
}

/// Build the outgoing header set from an already-read key.
fn build_headers(key: Option<&str>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    if let Some(raw) = key {
        if let Some(value) = api_key_header(raw)? {
            headers.insert("X-API-Key", value);
        }
    }
    Ok(headers)
}

/// Read the daemon's API key on the blocking pool.
///
/// `find_syncthing_config` probes up to four paths and reads the first that
/// exists. That is blocking file I/O, and it used to run inside every handler's
/// async body — once per proxied request (ROADMAP X.1). Same shape as
/// `mdns.rs` / `transfer_quota.rs`.
async fn read_api_key() -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(get_api_key)
        .await
        .map_err(|e| format!("{KEY_ERROR}: the config read task failed ({e})"))
}

/// Resolve the headers for one request to the daemon.
async fn request_headers() -> Result<HeaderMap, String> {
    let key = read_api_key().await?;
    build_headers(key.as_deref())
}

/// Client for the local daemon.
///
/// Certificate verification used to be switched off unconditionally here, so
/// any impostor answering on the configured address over TLS was trusted. The
/// address is a loopback constant today ([`SYNCTHING_ADDR`]), which is why the
/// flag was inert — but it is one configuration knob away from a real
/// man-in-the-middle, and a security flag that only happens to be harmless is
/// still a bug. Verification is the default again; if the daemon address ever
/// becomes configurable and a self-signed certificate has to be accepted, that
/// must be an explicit, documented, admin-chosen opt-in — never a constant in
/// the client builder. A unit test in this module fails if the escape-hatch
/// flag comes back (reqwest exposes no read-back for it).
fn http_client() -> Client {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        // Unreachable: the builder carries a static timeout and nothing else —
        // no user input, no TLS backend options. Written as `expect` rather than
        // the former `.unwrap_or_default()`, which silently returned a client
        // with **no timeout** at all.
        .expect("static Syncthing HTTP client configuration is valid")
}

/// One call to the daemon, with an optional JSON body.
///
/// Every endpoint goes through here, including the three mutating config calls
/// added by ROADMAP 18.1 (`POST /rest/config/folders`, `POST
/// /rest/config/devices` and the two `DELETE`s) — so the 10 s deadline, the
/// certificate check and the API-key handling cannot be forgotten on a new
/// route.
async fn syncthing_send(
    method: reqwest::Method,
    endpoint: &str,
    body: Option<&serde_json::Value>,
) -> Result<String, String> {
    let url = format!("{}/rest/{}", SYNCTHING_ADDR, endpoint);
    // Resolve the key before the request: a broken `config.xml` is a
    // configuration error, reported as such, not a connection error.
    let headers = request_headers().await?;
    let mut request = http_client().request(method, &url).headers(headers);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Syncthing: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Syncthing API error ({}): {}", status, body));
    }

    response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))
}

async fn syncthing_request(method: reqwest::Method, endpoint: &str) -> Result<String, String> {
    syncthing_send(method, endpoint, None).await
}

async fn syncthing_get(endpoint: &str) -> Result<String, String> {
    syncthing_request(reqwest::Method::GET, endpoint).await
}

async fn syncthing_post(endpoint: &str) -> Result<String, String> {
    syncthing_request(reqwest::Method::POST, endpoint).await
}

/// Read `GET /rest/system/config` and parse it into the models above.
///
/// Used as the *existence* check and the *device allowlist* of a config edit:
/// `system/config` is what the daemon is actually running, so an ID that is
/// already there is a conflict, and a device that is not there cannot be made a
/// sharer of a new folder.
async fn load_config() -> Result<SyncthingConfig, String> {
    let raw = syncthing_get("system/config").await?;
    serde_json::from_str(&raw).map_err(|e| format!("Failed to parse config: {}", e))
}

/// The appliance's own device ID (`myID` of `GET /rest/system/status`).
///
/// A Syncthing folder is always shared with the device that owns it, so the ID
/// is injected into the `devices` array of a new folder. `Ok(None)` means the
/// daemon did not answer — the folder is then created with the devices the
/// admin picked, and Syncthing normalises the list itself.
async fn self_device_id() -> Result<Option<String>, String> {
    let raw = syncthing_get("system/status").await?;
    let status: SyncthingSystemStatus =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse system status: {}", e))?;
    Ok(Some(status.my_id).filter(|id| !id.is_empty()))
}

// ---------------------------------------------------------------------------
// Validation of the values a config edit writes into Syncthing (ROADMAP 18.1)
// ---------------------------------------------------------------------------
//
// Everything below is interpolated into the daemon's persisted configuration,
// so it is validated before it is sent — the same bar the rest of the tree
// applies to a value that leaves a handler (cf. `validate_quarantine_dir`).
// The functions are pure and unit-tested; each one returns the *normalised*
// value on success, which is what the handler sends and what the audit log
// names.

/// Upper bound on a folder ID. Syncthing treats the ID as the folder's
/// identity — it is what a peer's devices store — so it is bounded and
/// restricted to a conservative character set.
pub const MAX_FOLDER_ID_LEN: usize = 64;
/// Upper bound on a folder label (shown in the UI and on peers).
pub const MAX_FOLDER_LABEL_LEN: usize = 128;
/// Upper bound on the folder path.
pub const MAX_FOLDER_PATH_LEN: usize = 1024;
/// Upper bound on the number of devices one folder is shared with.
pub const MAX_FOLDER_DEVICES: usize = 64;
/// Upper bound on a device name.
pub const MAX_DEVICE_NAME_LEN: usize = 128;
/// A Syncthing device ID is 56 base-32 characters, printed in eight groups of
/// seven separated by dashes.
pub const DEVICE_ID_LEN: usize = 56;

/// Folder ID: non-empty, ≤ [`MAX_FOLDER_ID_LEN`], letters, digits, `-`, `_`
/// and `.` only, and not the `.`/`..` path aliases.
pub fn validate_folder_id(raw: &str) -> Result<String, String> {
    let id = raw.trim();
    if id.is_empty() {
        return Err("Folder ID must not be empty".to_string());
    }
    if id.len() > MAX_FOLDER_ID_LEN {
        return Err(format!(
            "Folder ID is too long ({} characters, {MAX_FOLDER_ID_LEN} maximum)",
            id.len()
        ));
    }
    if id == "." || id == ".." {
        return Err(format!("Folder ID '{id}' is not a usable identifier"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "Folder ID '{id}' may only contain letters, digits, '-', '_' and '.'"
        ));
    }
    Ok(id.to_string())
}

/// Folder label: optional, trimmed, ≤ [`MAX_FOLDER_LABEL_LEN`], no control
/// characters. A blank label is accepted and means "same as the ID".
pub fn validate_folder_label(raw: &str) -> Result<String, String> {
    let label = raw.trim();
    if label.chars().any(|c| c.is_control()) {
        return Err("Folder name must not contain control characters".to_string());
    }
    if label.chars().count() > MAX_FOLDER_LABEL_LEN {
        return Err(format!(
            "Folder name is too long ({MAX_FOLDER_LABEL_LEN} characters maximum)"
        ));
    }
    Ok(label.to_string())
}

/// Folder path: absolute, no `..`, no control characters, not the filesystem
/// root, and inside one of `allowed_roots` — the storage locations HomeVault
/// manages (`AppState::path_guard`).
///
/// The path is passed to a daemon that runs as its own system user, so a value
/// outside the managed roots (say `/etc`) would make HomeVault the vector for
/// reading or rewriting files it has no business touching. A folder that does
/// not exist yet is legitimate: Syncthing creates it.
pub fn validate_folder_path(
    raw: &str,
    allowed_roots: &[std::path::PathBuf],
) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Folder path must not be empty".to_string());
    }
    if value.len() > MAX_FOLDER_PATH_LEN {
        return Err(format!(
            "Folder path is too long ({MAX_FOLDER_PATH_LEN} characters maximum)"
        ));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err("Folder path must not contain control characters".to_string());
    }
    let path = std::path::Path::new(value);
    if !path.is_absolute() {
        return Err(format!("Folder path '{value}' must be an absolute path"));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Folder path must not contain '..'".to_string());
    }
    let normalized: std::path::PathBuf = path
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect();
    if normalized.as_os_str().is_empty() {
        return Err("Folder path must not be the filesystem root".to_string());
    }
    let inside = allowed_roots.iter().any(|root| {
        let root: std::path::PathBuf = root
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();
        !root.as_os_str().is_empty() && normalized.starts_with(&root)
    });
    if !inside {
        return Err(format!(
            "Folder path '{value}' must be inside a storage location HomeVault manages"
        ));
    }
    Ok(value.trim_end_matches('/').to_string())
}

/// Sync mode: the three modes a home user can be offered. `receiveencrypted`
/// is deliberately absent — it only applies to an untrusted device, and
/// offering it would create a folder that cannot sync.
pub fn validate_folder_type(raw: &str) -> Result<String, String> {
    match raw.trim() {
        "" | "sendreceive" => Ok("sendreceive".to_string()),
        "sendonly" => Ok("sendonly".to_string()),
        "receiveonly" => Ok("receiveonly".to_string()),
        other => Err(format!(
            "Sync mode '{other}' is not supported (two-way, send only or receive only)"
        )),
    }
}

/// Device ID: normalised to the canonical `XXXXXXX-XXXXXXX-…` form.
///
/// Dashes and whitespace are optional on input (Syncthing's own parser ignores
/// them, and users paste what the other device shows), lower-case is accepted,
/// and the result must be 56 characters of the base-32 alphabet the format
/// uses. The check digit is **not** verified here: the daemon is the authority
/// on it, and a wrong implementation of the Luhn variant would reject valid
/// IDs. A structurally-wrong ID is refused before the request, so a typo never
/// reaches the daemon's config file.
pub fn normalize_device_id(raw: &str) -> Result<String, String> {
    let compact: String = raw
        .chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if compact.is_empty() {
        return Err("Device ID must not be empty".to_string());
    }
    if compact.len() != DEVICE_ID_LEN {
        return Err(format!(
            "Device ID must be {DEVICE_ID_LEN} characters (dashes are optional), got {}",
            compact.len()
        ));
    }
    if !compact
        .chars()
        .all(|c| c.is_ascii_uppercase() || ('2'..='7').contains(&c))
    {
        return Err(
            "Device ID may only contain letters A-Z and digits 2-7 (it is the ID the other \
             device shows under Actions → Show ID)"
                .to_string(),
        );
    }
    Ok(compact
        .as_bytes()
        .chunks(7)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-"))
}

/// Device name: optional, trimmed, ≤ [`MAX_DEVICE_NAME_LEN`], no control
/// characters. A blank name becomes the first group of the ID, which is what
/// Syncthing shows for an unnamed device anyway.
pub fn validate_device_name(raw: &str, device_id: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.chars().any(|c| c.is_control()) {
        return Err("Device name must not contain control characters".to_string());
    }
    if name.chars().count() > MAX_DEVICE_NAME_LEN {
        return Err(format!(
            "Device name is too long ({MAX_DEVICE_NAME_LEN} characters maximum)"
        ));
    }
    if name.is_empty() {
        return Ok(device_id.split('-').next().unwrap_or(device_id).to_string());
    }
    Ok(name.to_string())
}

/// Refuse more sharer devices than [`MAX_FOLDER_DEVICES`], and deduplicate.
pub fn validate_folder_devices(devices: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in devices {
        let id = normalize_device_id(raw)?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > MAX_FOLDER_DEVICES {
        return Err(format!(
            "A folder can be shared with at most {MAX_FOLDER_DEVICES} devices"
        ));
    }
    Ok(out)
}

/// Does the daemon's config already declare this folder?
pub fn config_has_folder(config: &SyncthingConfig, id: &str) -> bool {
    config.folders.iter().any(|f| f.id == id)
}

/// Is this device already paired?
pub fn config_has_device(config: &SyncthingConfig, device_id: &str) -> bool {
    config
        .devices
        .iter()
        .any(|d| d.device_id.eq_ignore_ascii_case(device_id))
}

/// The `devices` array of a folder's config, in Syncthing's own shape.
///
/// The appliance's own ID is appended when it is known and absent: a folder
/// only syncs with the devices listed on it, so without this a newly created
/// folder would not even be owned by the machine that created it.
pub fn folder_devices_with_self(devices: &[String], self_id: Option<&str>) -> Vec<String> {
    let mut out = devices.to_vec();
    if let Some(self_id) = self_id {
        let self_id = self_id.trim();
        if !self_id.is_empty() && !out.iter().any(|d| d.eq_ignore_ascii_case(self_id)) {
            out.insert(0, self_id.to_string());
        }
    }
    out
}

/// Body of `POST /rest/config/folders`.
pub fn folder_config_body(
    id: &str,
    label: &str,
    path: &str,
    folder_type: &str,
    devices: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "label": label,
        "path": path,
        "type": folder_type,
        "paused": false,
        "devices": devices
            .iter()
            .map(|d| serde_json::json!({ "deviceID": d }))
            .collect::<Vec<_>>(),
    })
}

/// Body of `POST /rest/config/devices`: the defaults Syncthing's GUI uses for
/// a newly paired device (discovery address, metadata-only compression, no
/// introducer, not paused). They are not exposed in the dialog on purpose —
/// a home user has no reason to change them, and every one of them is editable
/// in Syncthing's own GUI.
pub fn device_config_body(device_id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "deviceID": device_id,
        "name": name,
        "addresses": ["dynamic"],
        "compression": "metadata",
        "introducer": false,
        "paused": false,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Check if Syncthing is installed and running
pub async fn handle_syncthing_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<SyncthingStatus>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    // First check if syncthing binary exists
    let binary_check = tokio::process::Command::new("syncthing")
        .arg("--version")
        .output()
        .await;

    if binary_check.is_err() || !binary_check.unwrap().status.success() {
        return Json(ApiResponse::success(SyncthingStatus {
            available: false,
            version: None,
            my_id: None,
            uptime: None,
            last_seen: None,
        }));
    }

    // Resolve the API key before pinging. A `config.xml` whose key cannot be
    // sent as a header is a configuration fault, and reporting it here keeps
    // the screen from claiming "Installed but not running" for a daemon that is
    // running perfectly well.
    if let Err(e) = request_headers().await {
        return Json(ApiResponse::error(&e));
    }

    // Try to ping the API
    match syncthing_get("system/ping").await {
        Ok(_) => {
            // Get version and status
            let version_str = syncthing_get("system/version").await.ok();
            let version: Option<SyncthingVersion> =
                version_str.and_then(|v| serde_json::from_str(&v).ok());

            let status_str = syncthing_get("system/status").await.ok();
            let status: Option<SyncthingSystemStatus> =
                status_str.and_then(|s| serde_json::from_str(&s).ok());

            Json(ApiResponse::success(SyncthingStatus {
                available: true,
                version: version.map(|v| v.version),
                my_id: status.as_ref().map(|s| s.my_id.clone()),
                uptime: status.as_ref().and_then(|s| s.uptime_seconds),
                last_seen: status.as_ref().and_then(|s| s.last_seen.clone()),
            }))
        }
        Err(_) => Json(ApiResponse::success(SyncthingStatus {
            available: false,
            version: Some("Installed but not running".to_string()),
            my_id: None,
            uptime: None,
            last_seen: None,
        })),
    }
}

/// Get full Syncthing config (folders + devices)
pub async fn handle_syncthing_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<SyncthingConfig>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    let raw = match syncthing_get("system/config").await {
        Ok(r) => r,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    match serde_json::from_str::<SyncthingConfig>(&raw) {
        Ok(config) => Json(ApiResponse::success(config)),
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to parse config: {}",
            e
        ))),
    }
}

/// Get devices with connection status
pub async fn handle_syncthing_devices(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<SyncthingDeviceWithStatus>>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    let config_raw = match syncthing_get("system/config").await {
        Ok(r) => r,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let config: SyncthingConfig = match serde_json::from_str(&config_raw) {
        Ok(c) => c,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to parse config: {}",
                e
            )))
        }
    };

    let connections_raw = syncthing_get("system/connections").await.ok();
    let connections: Option<ConnectionsResponse> =
        connections_raw.and_then(|r| serde_json::from_str(&r).ok());

    let mut devices_with_status = Vec::new();

    for device in config.devices {
        let conn_info = connections
            .as_ref()
            .and_then(|c| c.connections.get(&device.device_id));

        // Get completion for this device
        let completion = if let Some(conn) = conn_info {
            conn.completion.unwrap_or(0)
        } else {
            0
        };

        devices_with_status.push(SyncthingDeviceWithStatus {
            device,
            connected: conn_info.map(|c| c.connected).unwrap_or(false),
            address: conn_info.map(|c| c.address.clone()),
            client_version: conn_info.map(|c| c.client_version.clone()),
            completion: completion as f64,
            in_bytes_total: conn_info.map(|c| c.in_bytes_total).unwrap_or(0),
            out_bytes_total: conn_info.map(|c| c.out_bytes_total).unwrap_or(0),
        });
    }

    Json(ApiResponse::success(devices_with_status))
}

/// Get folders with sync status
pub async fn handle_syncthing_folders(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<FolderStatus>>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    let config_raw = match syncthing_get("system/config").await {
        Ok(r) => r,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let config: SyncthingConfig = match serde_json::from_str(&config_raw) {
        Ok(c) => c,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to parse config: {}",
                e
            )))
        }
    };

    let mut folder_statuses = Vec::new();

    // `db/status` must be called once per folder (see `DbFolderStatus`'s
    // doc comment) — there's no bulk endpoint, so this is one request per
    // configured folder. `receive_only` comes from the folder's own `type`
    // in system/config (`db/status` doesn't report it at all), matching how
    // `id`/`label`/`path` already have to come from `config`, not `db_info`.
    for folder in config.folders {
        let device_ids: Vec<String> = folder.devices.iter().map(|d| d.device_id.clone()).collect();
        let encoded_id: String =
            url::form_urlencoded::byte_serialize(folder.id.as_bytes()).collect();
        let db_info: Option<DbFolderStatus> =
            syncthing_get(&format!("db/status?folder={}", encoded_id))
                .await
                .ok()
                .and_then(|r| serde_json::from_str(&r).ok());

        folder_statuses.push(FolderStatus {
            id: folder.id.clone(),
            label: folder.label.clone(),
            path: folder.path.clone(),
            paused: folder.paused,
            global_bytes: db_info.as_ref().map(|f| f.global_bytes).unwrap_or(0),
            global_files: db_info.as_ref().map(|f| f.global_files).unwrap_or(0),
            local_bytes: db_info.as_ref().map(|f| f.local_bytes).unwrap_or(0),
            local_files: db_info.as_ref().map(|f| f.local_files).unwrap_or(0),
            need_bytes: db_info.as_ref().map(|f| f.need_bytes).unwrap_or(0),
            need_files: db_info.as_ref().map(|f| f.need_files).unwrap_or(0),
            in_sync_files: db_info
                .as_ref()
                .map(|f| f.local_files.saturating_sub(f.need_files))
                .unwrap_or(0),
            receive_only: folder.folder_type == "receiveonly",
            devices: device_ids,
        });
    }

    Json(ApiResponse::success(folder_statuses))
}

/// Get active connections
pub async fn handle_syncthing_connections(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<ConnectionsResponse>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    let raw = match syncthing_get("system/connections").await {
        Ok(r) => r,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    match serde_json::from_str::<ConnectionsResponse>(&raw) {
        Ok(conns) => Json(ApiResponse::success(conns)),
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to parse connections: {}",
            e
        ))),
    }
}

/// Restart Syncthing
pub async fn handle_syncthing_restart(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    match syncthing_post("system/restart").await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' restarted the Syncthing daemon",
                admin_username
            ));
            Json(ApiResponse::success("Syncthing restarting...".to_string()))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Pause a device
#[derive(Deserialize)]
pub struct PauseDeviceRequest {
    pub device: String,
}

pub async fn handle_syncthing_pause_device(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<PauseDeviceRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let device: String = url::form_urlencoded::byte_serialize(req.device.as_bytes()).collect();
    match syncthing_post(&format!("system/pause?device={}", device)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' paused Syncthing device '{}'",
                admin_username, req.device
            ));
            Json(ApiResponse::success("Device paused".to_string()))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Resume a device
pub async fn handle_syncthing_resume_device(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<PauseDeviceRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let device: String = url::form_urlencoded::byte_serialize(req.device.as_bytes()).collect();
    match syncthing_post(&format!("system/resume?device={}", device)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' resumed Syncthing device '{}'",
                admin_username, req.device
            ));
            Json(ApiResponse::success("Device resumed".to_string()))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Trigger folder rescan
#[derive(Deserialize)]
pub struct ScanFolderRequest {
    pub folder: String,
}

pub async fn handle_syncthing_scan_folder(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<ScanFolderRequest>,
) -> Json<ApiResponse<String>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    let folder: String = url::form_urlencoded::byte_serialize(req.folder.as_bytes()).collect();
    match syncthing_post(&format!("db/scan?folder={}", folder)).await {
        Ok(_) => Json(ApiResponse::success("Scan triggered".to_string())),
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Get errors
pub async fn handle_syncthing_errors(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<String>>> {
    if let Err(e) = require_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }
    let raw = syncthing_get("system/error").await;
    match raw {
        Ok(r) => {
            let errors: Vec<String> = serde_json::from_str(&r).unwrap_or_default();
            Json(ApiResponse::success(errors))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

// ---------------------------------------------------------------------------
// Folder and device management (ROADMAP 18.1)
// ---------------------------------------------------------------------------
//
// Until this landed the screen could only *read* the daemon's topology and
// pause/resume an already-paired device: there was no route and no control to
// add or remove a folder or a device, so a user could not onboard a share or a
// peer from HomeVault at all. These four handlers close that, with the same
// admin bar as the rest of the file and validation *before* anything reaches
// the daemon's persisted configuration.

/// `POST /syncthing/folders` — create a folder and, optionally, share it.
#[derive(Deserialize)]
pub struct CreateFolderRequest {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub path: String,
    #[serde(default, alias = "type")]
    pub folder_type: String,
    /// Device IDs to share the new folder with. Defaults to none, which leaves
    /// the folder private to this appliance.
    #[serde(default)]
    pub devices: Vec<String>,
}

/// `POST /syncthing/devices` — pair a device.
#[derive(Deserialize)]
pub struct CreateDeviceRequest {
    #[serde(alias = "deviceID")]
    pub device_id: String,
    #[serde(default)]
    pub name: String,
}

pub async fn handle_syncthing_create_folder(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<CreateFolderRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // 1. Validate every field on its own, so the screen can point at the one
    //    that is wrong instead of echoing a daemon error.
    let id = match validate_folder_id(&req.id) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let label = match validate_folder_label(&req.label) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let path = match validate_folder_path(&req.path, state.path_guard.allowed_roots()) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let folder_type = match validate_folder_type(&req.folder_type) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let devices = match validate_folder_devices(&req.devices) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // 2. Only then talk to the daemon: the running config is both the conflict
    //    check and the allowlist of devices the folder may be shared with.
    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    if config_has_folder(&config, &id) {
        return Json(ApiResponse::error(&format!(
            "A folder with ID '{id}' already exists"
        )));
    }
    for device in &devices {
        if !config_has_device(&config, device) {
            return Json(ApiResponse::error(&format!(
                "Device '{device}' is not paired with this appliance; pair it before sharing a folder with it"
            )));
        }
    }

    let self_id = self_device_id().await.ok().flatten();
    let sharers = folder_devices_with_self(&devices, self_id.as_deref());
    let body = folder_config_body(&id, &label, &path, &folder_type, &sharers);

    match syncthing_send(reqwest::Method::POST, "config/folders", Some(&body)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' added Syncthing folder '{}' ({}) shared with {} device(s)",
                admin_username,
                id,
                path,
                sharers.len().saturating_sub(1)
            ));
            Json(ApiResponse::success(serde_json::json!({ "id": id })))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// `DELETE /syncthing/folders/{id}` — stop syncing a folder.
///
/// Syncthing only removes the folder from its configuration; the files it
/// already synced stay on disk. The screen says so in its confirmation.
pub async fn handle_syncthing_delete_folder(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<ApiResponse<String>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    // Re-validated even though it came out of the URL: the value is interpolated
    // into the daemon's REST path, and a decoded `..` must never get there.
    let id = match validate_folder_id(&id) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let encoded: String = url::form_urlencoded::byte_serialize(id.as_bytes()).collect();

    match syncthing_send(
        reqwest::Method::DELETE,
        &format!("config/folders/{}", encoded),
        None,
    )
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' removed Syncthing folder '{}'",
                admin_username, id
            ));
            Json(ApiResponse::success("Folder removed".to_string()))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_syncthing_create_device(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<CreateDeviceRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let device_id = match normalize_device_id(&req.device_id) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let name = match validate_device_name(&req.name, &device_id) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let config = match load_config().await {
        Ok(c) => c,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    if config_has_device(&config, &device_id) {
        return Json(ApiResponse::error(&format!(
            "A device with ID '{device_id}' is already paired"
        )));
    }
    // Pairing the appliance with itself would corrupt its own topology, and
    // Syncthing refuses it later with a message about the local device.
    if let Ok(Some(self_id)) = self_device_id().await {
        if self_id.eq_ignore_ascii_case(&device_id) {
            return Json(ApiResponse::error(
                "This is HomeVault's own device ID; pair the other device instead",
            ));
        }
    }

    let body = device_config_body(&device_id, &name);
    match syncthing_send(reqwest::Method::POST, "config/devices", Some(&body)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' paired Syncthing device '{}' ({})",
                admin_username, name, device_id
            ));
            Json(ApiResponse::success(serde_json::json!({ "device_id": device_id })))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// `DELETE /syncthing/devices/{id}` — remove a pairing.
///
/// The folders stay on disk and can be re-shared with the same device later;
/// the screen says so in its confirmation.
pub async fn handle_syncthing_delete_device(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    axum::extract::Path(device_id): axum::extract::Path<String>,
) -> Json<ApiResponse<String>> {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    let device_id = match normalize_device_id(&device_id) {
        Ok(v) => v,
        Err(e) => return Json(ApiResponse::error(&e)),
    };
    if let Ok(Some(self_id)) = self_device_id().await {
        if self_id.eq_ignore_ascii_case(&device_id) {
            return Json(ApiResponse::error(
                "This is HomeVault's own device ID; it cannot be removed",
            ));
        }
    }
    let encoded: String = url::form_urlencoded::byte_serialize(device_id.as_bytes()).collect();

    match syncthing_send(
        reqwest::Method::DELETE,
        &format!("config/devices/{}", encoded),
        None,
    )
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' removed Syncthing device '{}'",
                admin_username, device_id
            ));
            Json(ApiResponse::success("Device removed".to_string()))
        }
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for the Syncthing handler module.
    //!
    //! Two things here are pure and bug-prone: `extract_api_key` (pull the
    //! REST API key out of syncthing's `config.xml`) and the serde mapping
    //! of syncthing's REST responses onto the structs this module re-emits.
    //! Several of those `#[serde(alias = ...)]` attributes exist because a
    //! missing one silently broke parsing against a real daemon (e.g.
    //! `certName` on the device list) — pin them with fixtures captured from
    //! syncthing v1.27/v1.29.
    //!
    //! `api_key_header` / `build_headers` (ROADMAP 18.2) turn that same
    //! user-editable file content into an outbound header, which is where a
    //! malformed key used to panic the request task; they are pure, so the
    //! whole matrix is unit-testable.
    //!
    //! No HTTP, no `syncthing` process.
    //!
    //! Run: `cargo test handlers::syncthing::tests`

    use super::*;

    // -- extract_api_key -------------------------------------------------

    #[test]
    fn test_extract_api_key_from_realistic_config() {
        let xml = r#"<configuration version="37">
  <gui enabled="true" tls="false">
    <address>127.0.0.1:8384</address>
    <apikey>abcDEF123456ghiJKL7890mnoPQR</apikey>
    <theme>default</theme>
  </gui>
</configuration>"#;
        assert_eq!(
            extract_api_key(xml).as_deref(),
            Some("abcDEF123456ghiJKL7890mnoPQR")
        );
    }

    #[test]
    fn test_extract_api_key_missing_or_malformed() {
        assert_eq!(extract_api_key("<configuration></configuration>"), None);
        assert_eq!(extract_api_key("<apikey>no closing tag"), None);
        assert_eq!(extract_api_key(""), None);
    }

    #[test]
    fn test_extract_api_key_empty_value() {
        assert_eq!(extract_api_key("<apikey></apikey>").as_deref(), Some(""));
    }

    // -- API key → header (ROADMAP 18.2) --------------------------------
    //
    // The key is user-editable file content and it is interpolated into an
    // outbound header. `HeaderValue::from_str(..).unwrap()` used to panic the
    // request task on a malformed value, so every case below is a panic that
    // used to reach the handler.

    #[test]
    fn test_api_key_header_accepts_a_realistic_key() {
        let hv = api_key_header("abcDEF123456ghiJKL7890mnoPQR")
            .expect("a well-formed key must be accepted")
            .expect("a non-blank key yields a header");
        assert_eq!(hv, "abcDEF123456ghiJKL7890mnoPQR");
    }

    #[test]
    fn test_api_key_header_trims_the_xml_indentation() {
        // Pretty-printed config.xml: the extracted value starts with a newline,
        // which is not a valid header value on its own. Without the trim this
        // is exactly the "panic on a perfectly valid config" case.
        let xml = "<configuration>\n  <gui>\n    <apikey>\n      abcDEF\n    </apikey>\n  </gui>\n</configuration>";
        let raw = extract_api_key(xml).expect("key present");
        assert_eq!(raw, "\n      abcDEF\n    ");
        assert_eq!(
            api_key_header(&raw).expect("must be accepted").unwrap(),
            "abcDEF"
        );
    }

    #[test]
    fn test_api_key_header_rejects_control_characters_without_panicking() {
        for bad in ["abc\ndef", "abc\rdef", "abc\u{0}def", "abc\u{7f}def"] {
            let err = api_key_header(bad).expect_err("must be an error, not a panic");
            assert!(err.starts_with(KEY_ERROR), "{err}");
            assert!(err.contains("not a valid HTTP header value"), "{err}");
            assert!(
                !err.contains(bad.trim()),
                "the key itself must never be echoed back: {err}"
            );
        }
    }

    #[test]
    fn test_api_key_header_rejects_non_ascii() {
        let err = api_key_header("clé-secrète").expect_err("non-ASCII must be refused");
        assert!(err.starts_with(KEY_ERROR), "{err}");
        assert!(!err.contains("clé-secrète"), "{err}");
    }

    #[test]
    fn test_api_key_header_blank_means_no_header() {
        // A blank `<apikey></apikey>` is "no key configured", not an error: the
        // daemon with the GUI API key disabled expects no header at all.
        assert_eq!(api_key_header("").unwrap(), None);
        assert_eq!(api_key_header("   \n\t ").unwrap(), None);
    }

    #[test]
    fn test_build_headers_sets_or_omits_the_header() {
        let with = build_headers(Some("k3y")).expect("valid key");
        assert_eq!(with.get("X-API-Key").unwrap(), "k3y");

        assert!(build_headers(None).expect("no key").is_empty());
        assert!(build_headers(Some("   ")).expect("blank key").is_empty());

        // The former `.unwrap()` panic path, one level up.
        assert!(build_headers(Some("bad\nkey")).is_err());
    }

    /// TLS verification must stay on (ROADMAP 18.2).
    ///
    /// The client builder used to switch certificate verification off
    /// unconditionally. reqwest exposes no way to read that setting back from a
    /// built `Client`, so the guard is a source scan of this module — the same
    /// technique `dependencies.rs` uses for the installer. The forbidden
    /// identifier is assembled at run time so neither this doc comment nor the
    /// needle itself can satisfy the search.
    #[test]
    fn test_http_client_never_disables_certificate_verification() {
        let source = include_str!("syncthing.rs");
        let forbidden = concat!("danger_accept_invalid", "_certs");
        assert!(
            !source.contains(forbidden),
            "http_client() must not disable certificate verification: the flag was inert \
             only because SYNCTHING_ADDR is loopback, and it becomes a man-in-the-middle \
             the day the address is configurable"
        );
        // And the client still builds (with its timeout) without that flag.
        let _ = http_client();
    }

    // -- system/config parsing -----------------------------------------

    const CONFIG_JSON: &str = r#"{
      "folders": [
        {"id":"default","label":"Default Folder","path":"/srv/sync/default",
         "type":"sendreceive","paused":false,
         "devices":[{"deviceID":"AAAA-BBBB"},{"deviceID":"CCCC-DDDD"}]},
        {"id":"ro","label":"Read Only","path":"/srv/sync/ro",
         "type":"receiveonly","devices":[{"deviceID":"AAAA-BBBB"}]}
      ],
      "devices": [
        {"deviceID":"AAAA-BBBB","name":"nas","addresses":["dynamic"],
         "compression":"metadata","certName":"","introducer":false,"paused":false},
        {"deviceID":"CCCC-DDDD","name":"laptop","addresses":["tcp://10.0.0.5:22000"],
         "compression":"metadata","certName":"syncthing","introducer":true,"paused":true}
      ]
    }"#;

    #[test]
    fn test_config_parses_folders_and_devices() {
        let cfg: SyncthingConfig = serde_json::from_str(CONFIG_JSON).expect("must parse");
        assert_eq!(cfg.folders.len(), 2);
        assert_eq!(cfg.folders[0].id, "default");
        assert_eq!(cfg.folders[0].folder_type, "sendreceive"); // aliased from "type"
        assert_eq!(cfg.folders[0].devices.len(), 2);
        assert_eq!(cfg.folders[1].folder_type, "receiveonly");
        // second folder has no `paused` key → #[serde(default)] → false
        assert!(!cfg.folders[1].paused);

        assert_eq!(cfg.devices.len(), 2);
        assert_eq!(cfg.devices[0].device_id, "AAAA-BBBB"); // aliased from "deviceID"
        assert_eq!(cfg.devices[1].cert_name, "syncthing"); // aliased from "certName"
        assert!(cfg.devices[1].paused);
        // `introduced_by` absent → #[serde(default)] → ""
        assert_eq!(cfg.devices[0].introduced_by, "");
    }

    #[test]
    fn test_device_missing_certname_alias_would_break_without_it() {
        // Regression: `certName` was once un-aliased and this failed.
        let one = r#"{"deviceID":"X","name":"n","addresses":[],"compression":"metadata",
                      "certName":"cn","introducer":false,"paused":false}"#;
        let d: SyncthingDevice = serde_json::from_str(one).expect("must parse with certName");
        assert_eq!(d.cert_name, "cn");
    }

    // -- system/connections parsing ----------------------------------

    #[test]
    fn test_connections_response_parses_with_aliases() {
        let raw = r#"{
          "total": {"at":"2026-09-06T12:00:00Z","inBytesTotal":10,"outBytesTotal":20},
          "connections": {
            "AAAA-BBBB": {
              "address":"10.0.0.5:22000",
              "at":"2026-09-06T11:59:00Z",
              "clientVersion":"v1.27.2",
              "connected":true,
              "paused":false,
              "inBytesTotal":123456,
              "outBytesTotal":7890,
              "completion":100
            }
          }
        }"#;
        let conns: ConnectionsResponse = serde_json::from_str(raw).expect("must parse");
        let c = conns.connections.get("AAAA-BBBB").expect("device present");
        assert_eq!(c.connected_at, "2026-09-06T11:59:00Z"); // aliased from "at"
        assert_eq!(c.client_version, "v1.27.2"); // aliased from "clientVersion"
        assert_eq!(c.in_bytes_total, 123456);
        assert_eq!(c.out_bytes_total, 7890);
        assert_eq!(c.completion, Some(100));
    }

    // -- db/status parsing (one folder) ----------------------------

    #[test]
    fn test_db_folder_status_models_only_the_stat_fields() {
        // The real db/status response has many more keys (state, version,
        // sequence, ...) — parsing must ignore them and never require the
        // id/label/path/receiveOnly fields that were wrongly declared once.
        let raw = r#"{
          "globalBytes": 1000, "globalFiles": 10,
          "localBytes": 900, "localFiles": 9,
          "needBytes": 100, "needFiles": 1,
          "state": "idle", "version": 42, "sequence": 99,
          "inSyncBytes": 900, "ignorePatterns": false
        }"#;
        let s: DbFolderStatus = serde_json::from_str(raw).expect("must parse");
        assert_eq!(s.global_bytes, 1000);
        assert_eq!(s.local_files, 9);
        assert_eq!(s.need_files, 1);
    }

    // -- system/status parsing -----------------------------------

    #[test]
    fn test_system_status_parses_myid_alias() {
        let raw = r#"{"myID":"AAAA-BBBB-CCCC","uptimeSeconds":3600,"cpuPercent":0.5}"#;
        let s: SyncthingSystemStatus = serde_json::from_str(raw).expect("must parse");
        assert_eq!(s.my_id, "AAAA-BBBB-CCCC");
        assert_eq!(s.uptime_seconds, Some(3600));
    }

    // -- request bodies -----------------------------------------

    #[test]
    fn test_pause_and_scan_request_bodies() {
        let p: PauseDeviceRequest = serde_json::from_str(r#"{"device":"AAAA-BBBB"}"#).unwrap();
        assert_eq!(p.device, "AAAA-BBBB");
        assert!(serde_json::from_str::<PauseDeviceRequest>("{}").is_err());

        let s: ScanFolderRequest = serde_json::from_str(r#"{"folder":"default"}"#).unwrap();
        assert_eq!(s.folder, "default");
        assert!(serde_json::from_str::<ScanFolderRequest>("{}").is_err());
    }

    // -- ApiResponse envelope ----------------------------------

    #[test]
    fn test_api_response_shapes() {
        let ok = serde_json::to_value(ApiResponse::success("x".to_string())).unwrap();
        assert_eq!(ok["success"], true);
        let err = serde_json::to_value(ApiResponse::<String>::error("no")).unwrap();
        assert_eq!(err["success"], false);
        assert_eq!(err["error"], "no");
    }

    // -- receive_only derivation (handler logic mirrored) ---------

    #[test]
    fn test_receive_only_is_derived_from_folder_type() {
        let cfg: SyncthingConfig = serde_json::from_str(CONFIG_JSON).unwrap();
        assert!(cfg.folders[0].folder_type != "receiveonly");
        assert!(cfg.folders[1].folder_type == "receiveonly");
    }

    // -- config edits: folder and device management (ROADMAP 18.1) ----
    //
    // Everything below is interpolated into the daemon's persisted
    // configuration, so each rule is pinned on its own: an accepted value is
    // the *normalised* one that gets sent, a refused value names the rule and
    // never reaches the daemon.

    /// A structurally valid device ID: 56 base-32 characters in eight groups.
    const DEVICE_ID: &str =
        "AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD-EEEEEEE-FFFFFFF-GGGGGGG-HHHHHHH";

    fn allowed_roots() -> Vec<std::path::PathBuf> {
        vec![
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/mnt/pools/data"),
        ]
    }

    #[test]
    fn test_validate_folder_id_accepts_normal_ids() {
        assert_eq!(validate_folder_id("photos").unwrap(), "photos");
        assert_eq!(validate_folder_id("  my-folder_1.2  ").unwrap(), "my-folder_1.2");
        assert_eq!(validate_folder_id(&"a".repeat(MAX_FOLDER_ID_LEN)).unwrap().len(), MAX_FOLDER_ID_LEN);
    }

    #[test]
    fn test_validate_folder_id_rejects_the_ways_an_id_can_be_unsafe() {
        for (input, needle) in [
            ("", "must not be empty"),
            ("   ", "must not be empty"),
            (".", "not a usable identifier"),
            ("..", "not a usable identifier"),
            ("a/b", "may only contain"),
            ("a b", "may only contain"),
            ("a\\b", "may only contain"),
            ("photos\n", "may only contain"),
            ("é", "may only contain"),
        ] {
            let err = validate_folder_id(input).expect_err(input);
            assert!(err.contains(needle), "{input:?} → {err}");
        }
        let too_long = "a".repeat(MAX_FOLDER_ID_LEN + 1);
        assert!(validate_folder_id(&too_long).unwrap_err().contains("too long"));
    }

    #[test]
    fn test_validate_folder_label_trims_and_bounds() {
        assert_eq!(validate_folder_label("  Photos  ").unwrap(), "Photos");
        assert_eq!(validate_folder_label("").unwrap(), "");
        assert!(validate_folder_label("a\u{0}b").unwrap_err().contains("control characters"));
        let long = "a".repeat(MAX_FOLDER_LABEL_LEN + 1);
        assert!(validate_folder_label(&long).unwrap_err().contains("too long"));
    }

    #[test]
    fn test_validate_folder_path_accepts_a_folder_inside_a_managed_root() {
        // The standard case: an admin creates a folder that does not exist yet
        // — Syncthing creates it, so existence is deliberately not required.
        assert_eq!(
            validate_folder_path("/tmp/syncthing/photos", &allowed_roots()).unwrap(),
            "/tmp/syncthing/photos"
        );
        // Trailing slashes are trimmed so the value round-trips.
        assert_eq!(
            validate_folder_path("/tmp/photos/", &allowed_roots()).unwrap(),
            "/tmp/photos"
        );
        // A whole share root is legitimate.
        assert_eq!(
            validate_folder_path("/mnt/pools/data", &allowed_roots()).unwrap(),
            "/mnt/pools/data"
        );
    }

    #[test]
    fn test_validate_folder_path_rejects_relative_traversal_root_and_outside_roots() {
        for (input, needle) in [
            ("", "must not be empty"),
            ("relative/photos", "must be an absolute path"),
            ("/tmp/../etc", "must not contain '..'"),
            ("/tmp/./photos", ""), // '.' is filtered, this one is accepted below
            ("/", "must not be the filesystem root"),
            ("/etc", "must be inside a storage location"),
            ("/tmpfoo/photos", "must be inside a storage location"),
            ("/tmp/a\u{0}b", "must not contain control characters"),
        ] {
            match validate_folder_path(input, &allowed_roots()) {
                Ok(v) => assert!(needle.is_empty(), "{input:?} should be refused, got {v:?}"),
                Err(e) => assert!(e.contains(needle), "{input:?} → {e}"),
            }
        }
        let long = format!("/tmp/{}", "a".repeat(MAX_FOLDER_PATH_LEN));
        assert!(validate_folder_path(&long, &allowed_roots())
            .unwrap_err()
            .contains("too long"));
    }

    #[test]
    fn test_validate_folder_path_without_roots_refuses_everything() {
        // A guard built with no configured root must not become a blanket
        // allow — the same fail-closed rule the transfer-quota guard follows.
        assert!(validate_folder_path("/tmp/photos", &[])
            .unwrap_err()
            .contains("must be inside a storage location"));
    }

    #[test]
    fn test_validate_folder_type_allowlist() {
        assert_eq!(validate_folder_type("").unwrap(), "sendreceive");
        assert_eq!(validate_folder_type(" sendreceive ").unwrap(), "sendreceive");
        assert_eq!(validate_folder_type("sendonly").unwrap(), "sendonly");
        assert_eq!(validate_folder_type("receiveonly").unwrap(), "receiveonly");
        // Offered nowhere in the UI and only meaningful for an untrusted
        // device; refused so a folder cannot be created in a mode that does
        // not sync.
        assert!(validate_folder_type("receiveencrypted")
            .unwrap_err()
            .contains("not supported"));
        assert!(validate_folder_type("whatever").unwrap_err().contains("not supported"));
    }

    #[test]
    fn test_normalize_device_id_accepts_what_users_paste() {
        // Canonical form round-trips.
        assert_eq!(normalize_device_id(DEVICE_ID).unwrap(), DEVICE_ID);
        // Dashes optional, lower case accepted, surrounding spaces ignored.
        let compact: String = DEVICE_ID.replace('-', "");
        assert_eq!(normalize_device_id(&compact).unwrap(), DEVICE_ID);
        assert_eq!(normalize_device_id(&DEVICE_ID.to_lowercase()).unwrap(), DEVICE_ID);
        assert_eq!(normalize_device_id(&format!("  {compact}\n")).unwrap(), DEVICE_ID);
        // Dashes in unexpected places are re-grouped, not trusted.
        assert_eq!(normalize_device_id(&format!("{}--{}", &compact[..7], &compact[7..])).unwrap(), DEVICE_ID);
    }

    #[test]
    fn test_normalize_device_id_rejects_bad_shape_and_alphabet() {
        assert!(normalize_device_id("").unwrap_err().contains("must not be empty"));
        assert!(normalize_device_id("AAAAAAA-BBBBBBB").unwrap_err().contains("56 characters"));
        // 0, 1, 8 and 9 are not in Syncthing's base-32 alphabet.
        let bad = "0".repeat(DEVICE_ID_LEN);
        assert!(normalize_device_id(&bad).unwrap_err().contains("letters A-Z and digits 2-7"));
        // A UTF-8 device ID would break the daemon's config, never mind ours.
        let dashed_unicode = format!("{}-AAAAAAA", "é".repeat(DEVICE_ID_LEN - 8));
        assert!(normalize_device_id(&dashed_unicode).is_err());
    }

    #[test]
    fn test_validate_device_name_defaults_to_the_short_id() {
        assert_eq!(validate_device_name("", DEVICE_ID).unwrap(), "AAAAAAA");
        assert_eq!(validate_device_name("  Laptop  ", DEVICE_ID).unwrap(), "Laptop");
        assert!(validate_device_name("a\nb", DEVICE_ID).unwrap_err().contains("control characters"));
        let long = "n".repeat(MAX_DEVICE_NAME_LEN + 1);
        assert!(validate_device_name(&long, DEVICE_ID).unwrap_err().contains("too long"));
    }

    #[test]
    fn test_validate_folder_devices_normalises_dedupes_and_caps() {
        let other = "IIIIIII-JJJJJJJ-KKKKKKK-LLLLLLL-MMMMMMM-NNNNNNN-OOOOOOO-PPPPPPP";
        let out = validate_folder_devices(&[
            DEVICE_ID.to_lowercase(),
            DEVICE_ID.to_string(),
            other.to_string(),
        ])
        .unwrap();
        assert_eq!(out, vec![DEVICE_ID.to_string(), other.to_string()]);

        let too_many: Vec<String> = (0..=MAX_FOLDER_DEVICES).map(|_| DEVICE_ID.to_string()).collect();
        // 65 *identical* IDs collapse to one — the cap is on distinct devices.
        assert_eq!(validate_folder_devices(&too_many).unwrap().len(), 1);
        assert!(validate_folder_devices(&["nope".to_string()])
            .unwrap_err()
            .contains("56 characters"));
    }

    #[test]
    fn test_config_has_folder_and_device() {
        let cfg: SyncthingConfig = serde_json::from_str(CONFIG_JSON).unwrap();
        assert!(config_has_folder(&cfg, "default"));
        assert!(!config_has_folder(&cfg, "default2"));
        assert!(config_has_device(&cfg, "AAAA-BBBB"));
        // A pasted ID is normalised before the comparison, but the comparison
        // itself is case-insensitive so a legacy lower-case config still
        // answers truthfully.
        assert!(config_has_device(&cfg, "aaaa-bbbb"));
        assert!(!config_has_device(&cfg, "ZZZZ-ZZZZ"));
    }

    #[test]
    fn test_folder_devices_with_self_owns_the_folder() {
        let self_id = "SELF-SELF";
        assert_eq!(
            folder_devices_with_self(&[], Some(self_id)),
            vec![self_id.to_string()]
        );
        // Already listed (in any case) → not duplicated.
        let listed = vec![self_id.to_lowercase(), DEVICE_ID.to_string()];
        assert_eq!(folder_devices_with_self(&listed, Some(self_id)).len(), 2);
        // Unknown self (daemon did not answer) → the picked devices are sent
        // as-is and Syncthing normalises the list itself.
        assert_eq!(folder_devices_with_self(&[DEVICE_ID.to_string()], None), vec![DEVICE_ID.to_string()]);
        assert_eq!(folder_devices_with_self(&[], Some("   ")), Vec::<String>::new());
    }

    #[test]
    fn test_folder_config_body_matches_the_daemon_schema() {
        let body = folder_config_body(
            "photos",
            "Photos",
            "/tmp/photos",
            "receiveonly",
            &[DEVICE_ID.to_string()],
        );
        assert_eq!(body["id"], "photos");
        assert_eq!(body["label"], "Photos");
        assert_eq!(body["path"], "/tmp/photos");
        assert_eq!(body["type"], "receiveonly");
        assert_eq!(body["paused"], false);
        // The devices array is `[{ "deviceID": … }]`, never bare strings.
        assert_eq!(body["devices"][0]["deviceID"], DEVICE_ID);
        assert_eq!(body["devices"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_device_config_body_uses_the_gui_defaults() {
        let body = device_config_body(DEVICE_ID, "Laptop");
        assert_eq!(body["deviceID"], DEVICE_ID);
        assert_eq!(body["name"], "Laptop");
        assert_eq!(body["addresses"][0], "dynamic");
        assert_eq!(body["compression"], "metadata");
        assert_eq!(body["introducer"], false);
        assert_eq!(body["paused"], false);
    }

    #[test]
    fn test_create_request_bodies_accept_the_documented_shapes() {
        let folder: CreateFolderRequest =
            serde_json::from_str(r#"{"id":"photos","path":"/tmp/photos"}"#).unwrap();
        assert_eq!(folder.id, "photos");
        assert_eq!(folder.folder_type, "");
        assert!(folder.devices.is_empty());
        assert_eq!(folder.label, "");

        // The screen sends `type` (Syncthing's own field name).
        let folder: CreateFolderRequest = serde_json::from_str(
            r#"{"id":"p","label":"P","path":"/tmp/p","type":"sendonly","devices":["X"]}"#,
        )
        .unwrap();
        assert_eq!(folder.folder_type, "sendonly");
        assert_eq!(folder.devices, vec!["X".to_string()]);
        // A missing required field is what the extractor 422s on.
        assert!(serde_json::from_str::<CreateFolderRequest>(r#"{"id":"p"}"#).is_err());

        let device: CreateDeviceRequest =
            serde_json::from_str(r#"{"device_id":"AAAA","name":"n"}"#).unwrap();
        assert_eq!(device.device_id, "AAAA");
        // `deviceID` is the daemon's spelling and is accepted too.
        let device: CreateDeviceRequest = serde_json::from_str(r#"{"deviceID":"AAAA"}"#).unwrap();
        assert_eq!(device.device_id, "AAAA");
        assert_eq!(device.name, "");
        assert!(serde_json::from_str::<CreateDeviceRequest>("{}").is_err());
    }
}
