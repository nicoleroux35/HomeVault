use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use reqwest::Client;

use crate::handlers::AppStateArc;

const SYNCTHING_ADDR: &str = "http://127.0.0.1:8384";

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
        Self { success: true, data: Some(data), error: None }
    }
    fn error(msg: &str) -> Self {
        Self { success: false, data: None, error: Some(msg.to_string()) }
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

fn get_api_key() -> Option<String> {
    let xml = find_syncthing_config()?;
    extract_api_key(&xml)
}

fn http_client() -> reqwest::Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

fn build_headers() -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(key) = get_api_key() {
        headers.insert(
            "X-API-Key",
            reqwest::header::HeaderValue::from_str(&key).unwrap(),
        );
    }
    headers
}

async fn syncthing_get(endpoint: &str) -> Result<String, String> {
    let url = format!("{}/rest/{}", SYNCTHING_ADDR, endpoint);
    let client = http_client();
    let response = client
        .get(&url)
        .headers(build_headers())
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Syncthing: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Syncthing API error ({}): {}", status, body));
    }

    response.text().await.map_err(|e| format!("Failed to read response: {}", e))
}

async fn syncthing_post(endpoint: &str) -> Result<String, String> {
    let url = format!("{}/rest/{}", SYNCTHING_ADDR, endpoint);
    let client = http_client();
    let response = client
        .post(&url)
        .headers(build_headers())
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Syncthing: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Syncthing API error ({}): {}", status, body));
    }

    response.text().await.map_err(|e| format!("Failed to read response: {}", e))
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

    // Try to ping the API
    match syncthing_get("system/ping").await {
        Ok(_) => {
            // Get version and status
            let version_str = syncthing_get("system/version").await.ok();
            let version: Option<SyncthingVersion> = version_str
                .and_then(|v| serde_json::from_str(&v).ok());

            let status_str = syncthing_get("system/status").await.ok();
            let status: Option<SyncthingSystemStatus> = status_str
                .and_then(|s| serde_json::from_str(&s).ok());

            Json(ApiResponse::success(SyncthingStatus {
                available: true,
                version: version.map(|v| v.version),
                my_id: status.as_ref().map(|s| s.my_id.clone()),
                uptime: status.as_ref().and_then(|s| s.uptime_seconds),
                last_seen: status.as_ref().and_then(|s| s.last_seen.clone()),
            }))
        }
        Err(_) => {
            Json(ApiResponse::success(SyncthingStatus {
                available: false,
                version: Some("Installed but not running".to_string()),
                my_id: None,
                uptime: None,
                last_seen: None,
            }))
        }
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
        Err(e) => Json(ApiResponse::error(&format!("Failed to parse config: {}", e))),
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
        Err(e) => return Json(ApiResponse::error(&format!("Failed to parse config: {}", e))),
    };

    let connections_raw = syncthing_get("system/connections").await.ok();
    let connections: Option<ConnectionsResponse> = connections_raw
        .and_then(|r| serde_json::from_str(&r).ok());

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
        Err(e) => return Json(ApiResponse::error(&format!("Failed to parse config: {}", e))),
    };

    let mut folder_statuses = Vec::new();

    // `db/status` must be called once per folder (see `DbFolderStatus`'s
    // doc comment) — there's no bulk endpoint, so this is one request per
    // configured folder. `receive_only` comes from the folder's own `type`
    // in system/config (`db/status` doesn't report it at all), matching how
    // `id`/`label`/`path` already have to come from `config`, not `db_info`.
    for folder in config.folders {
        let device_ids: Vec<String> = folder.devices.iter().map(|d| d.device_id.clone()).collect();
        let encoded_id: String = url::form_urlencoded::byte_serialize(folder.id.as_bytes()).collect();
        let db_info: Option<DbFolderStatus> = syncthing_get(&format!("db/status?folder={}", encoded_id))
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
            in_sync_files: db_info.as_ref().map(|f| f.local_files.saturating_sub(f.need_files)).unwrap_or(0),
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
        Err(e) => Json(ApiResponse::error(&format!("Failed to parse connections: {}", e))),
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
        let p: PauseDeviceRequest =
            serde_json::from_str(r#"{"device":"AAAA-BBBB"}"#).unwrap();
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
        assert_eq!(cfg.folders[0].folder_type == "receiveonly", false);
        assert_eq!(cfg.folders[1].folder_type == "receiveonly", true);
    }
}
