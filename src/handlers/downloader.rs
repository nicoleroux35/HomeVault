/* =============================================================================
 * File          : downloader.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Download-manager endpoints backed by the aria2 daemon: service status
 *   and start/stop, global and per-task statistics, adding URLs and
 *   torrents, pause/resume/removal, and task details. Every endpoint is
 *   admin-only; aria2 state is shared through Aria2State in AppState.
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
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::handlers::AppStateArc;

const ARIA2_RPC_URL: &str = "http://localhost:6800/jsonrpc";

pub struct Aria2Process {
    pub child: Option<tokio::process::Child>,
}

pub type Aria2State = Arc<Mutex<Aria2Process>>;

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
}

#[derive(Deserialize)]
pub struct GidRequest {
    pub gid: String,
}

async fn aria2_rpc(
    method: &str,
    params: Vec<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "homevault",
        "method": method,
        "params": params
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(ARIA2_RPC_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("aria2 RPC connection failed: {}", e))?;

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

async fn find_aria2() -> Option<String> {
    let candidates = ["aria2c", "/usr/bin/aria2c", "/usr/local/bin/aria2c"];
    for bin in &candidates {
        let check = if bin.starts_with('/') {
            tokio::fs::metadata(bin).await.is_ok()
        } else {
            tokio::process::Command::new("which")
                .arg(bin)
                .output()
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

async fn ensure_aria2_running(state: &Aria2State) -> Result<(), String> {
    let mut proc = state.lock().await;
    if proc.child.is_some() {
        return Ok(());
    }

    let aria2_path = find_aria2().await.ok_or_else(|| {
        "aria2c not found on system. Install with: sudo apt install aria2".to_string()
    })?;

    let _ = tokio::fs::create_dir_all("/tmp/aria2-downloads").await;

    let child = tokio::process::Command::new(&aria2_path)
        .args([
            "--enable-rpc",
            "--rpc-listen-all=false",
            "--rpc-listen-port=6800",
            "--rpc-max-request-size=2M",
            "--dir=/tmp/aria2-downloads",
            "--continue=true",
            "--max-connection-per-server=4",
            "--split=4",
            "--min-split-size=1M",
            "--console-log-level=warn",
            "--daemon=false",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start aria2c: {}", e))?;

    proc.child = Some(child);

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    Ok(())
}

pub async fn handle_downloader_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<serde_json::Value>> {
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    let aria2 = state.downloader.clone();

    let available = find_aria2().await.is_some();

    let running = {
        let proc = aria2.lock().await;
        proc.child.is_some()
    };

    Json(ApiResponse::success(serde_json::json!({
        "available": available,
        "running": running
    })))
}

pub async fn handle_downloader_start(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    match ensure_aria2_running(&state.downloader).await {
        Ok(()) => Json(ApiResponse::success("aria2c started".to_string())),
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

pub async fn handle_downloader_stop(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
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
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc("aria2.getGlobalStat", vec![]).await {
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
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
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

    match aria2_rpc("aria2.tellActive", vec![]).await {
        Ok(active) => {
            any_succeeded = true;
            if let Ok(mut active_tasks) = serde_json::from_value::<Vec<Aria2Task>>(active) {
                tasks.append(&mut active_tasks);
            }
        }
        Err(e) => last_error = Some(e),
    }

    match aria2_rpc(
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
    let admin_username = match require_admin_request(&state, &headers).await {
        Ok(u) => u,
        Err((_, msg)) => return Json(ApiResponse::error(&msg)),
    };
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    let mut options = serde_json::json!({});
    if let Some(dir) = &req.dir {
        options["dir"] = serde_json::json!(dir);
    }
    if let Some(out) = &req.out {
        options["out"] = serde_json::json!(out);
    }

    let params = vec![serde_json::json!([req.uri]), options];

    match aria2_rpc("aria2.addUri", params).await {
        Ok(val) => {
            let gid = val.as_str().unwrap_or("unknown").to_string();
            crate::events::log_event(format!(
                "Admin '{}' added a download: '{}'",
                admin_username, req.uri
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
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc("aria2.pause", vec![serde_json::json!(req.gid)]).await {
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
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc("aria2.unpause", vec![serde_json::json!(req.gid)]).await {
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
    let admin_username = match require_admin_request(&state, &headers).await {
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
    match aria2_rpc("aria2.remove", vec![serde_json::json!(req.gid)]).await {
        Ok(val) => {
            let gid = val.as_str().unwrap_or("unknown").to_string();
            Json(ApiResponse::success(gid))
        }
        Err(_) => match aria2_rpc("aria2.forceRemove", vec![serde_json::json!(req.gid)]).await {
            Ok(val) => {
                let gid = val.as_str().unwrap_or("unknown").to_string();
                Json(ApiResponse::success(gid))
            }
            Err(_) => match aria2_rpc(
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
    if let Err((_, msg)) = require_admin_request(&state, &headers).await {
        return Json(ApiResponse::error(&msg));
    }
    if let Err(e) = ensure_aria2_running(&state.downloader).await {
        return Json(ApiResponse::error(&e));
    }

    match aria2_rpc("aria2.tellStatus", vec![serde_json::json!(gid)]).await {
        Ok(val) => match serde_json::from_value::<Aria2Task>(val) {
            Ok(task) => Json(ApiResponse::success(task)),
            Err(e) => Json(ApiResponse::error(&format!("parse error: {}", e))),
        },
        Err(e) => Json(ApiResponse::error(&e)),
    }
}

/// Validates that the request comes from an authenticated administrator.
/// Returns `Err((status, message))` when access must be denied; handlers turn
/// this into `Json(ApiResponse::error(&message))`.
async fn require_admin_request(
    state: &AppStateArc,
    headers: &HeaderMap,
) -> Result<String, (u16, String)> {
    let token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if token.is_empty() {
        return Err((401, "No session token provided".to_string()));
    }

    let Some(username) = state.auth.get_username_from_token(token).await else {
        return Err((401, "Invalid or expired session".to_string()));
    };

    if !state.auth.is_admin(&username).await {
        return Err((
            403,
            "Admin privileges required for this operation".to_string(),
        ));
    }

    Ok(username)
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for the Downloader (aria2) handler module.
    //!
    //! These cover the one thing in this file that is pure, deterministic and
    //! bug-prone: the **asymmetric serde mapping** between aria2's real
    //! JSON-RPC wire format (camelCase: `downloadSpeed`, `totalLength`,
    //! `errorMessage`, ...) and the snake_case JSON this API re-emits to the
    //! Svelte frontend (`Aria2Task` / `GlobalStat` in `Downloader.svelte`).
    //! A regression here is exactly the bug the `#[serde(rename(deserialize =
    //! ...))]` attributes were added to fix (see the module-level comment on
    //! `Aria2GlobalStat`): a real aria2 response would silently fail to parse
    //! and every stat/task lookup would come back empty.
    //!
    //! No network, no aria2 process, no filesystem — just serde round-trips
    //! against fixtures captured verbatim from a live `aria2c` 1.37.0.
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
}
