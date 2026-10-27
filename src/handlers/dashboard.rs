/* =============================================================================
 * File          : dashboard.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Dashboard endpoints: the aggregated dashboard payload, per-widget data,
 *   and the user's widget layout (add/remove/reorder — scoped to the
 *   caller's own dashboard). Live metrics (CPU cores, RAM, network
 *   traffic, disk I/O, storage, temperature) are computed from deltas
 *   between samples kept in memory; the health section digs into SMART
 *   status and self-test logs, retrying with an explicit SATA-over-USB
 *   device type when smartctl's autodetection cannot see through a USB
 *   bridge. All endpoints are session-auth (no admin gating — the
 *   dashboard is per-user).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - chrono (>= 0.4)
 *   - parking_lot (>= 0.12)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/dashboard in src/web/mod.rs; consumed by the Dashboard
 *   screen and its widgets.
 * =============================================================================
 */

//! Dashboard endpoints: aggregated data, widget layout, live metrics and
//! SMART-backed health — session-auth, per-user.

use axum::{
    Json as AxumJson,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use chrono::Utc;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;

use crate::handlers::AppState;
use crate::handlers::admin_guard::require_auth;
use crate::storage::models::Widget;
use crate::utils::ProcParser;
use crate::utils::ResponseHelper;

/// Run one async host probe under the shared deadline (ROADMAP 15.2).
///
/// Every command this module shells out to — `df`, `lsblk`, `mdadm`,
/// `zpool` — is a probe whose answer is optional: on a timeout the caller
/// reports the field as unknown instead of letting a wedged NFS mount or a
/// spinning disk pile requests up behind it. The child is killed on expiry.
async fn run_probe(cmd: &mut tokio::process::Command) -> std::io::Result<std::process::Output> {
    crate::utils::process::run_timed_async(cmd, crate::utils::process::HOST_PROBE_TIMEOUT)
        .await
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "probe did not finish within its deadline",
            )
        })
}

/// Blocking counterpart, for the `spawn_blocking` fan-out in
/// `build_server_details` (nvidia-smi, rocm-smi, ip, lsblk, sensors,
/// dpkg-query).
fn run_probe_blocking(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    crate::utils::process::run_timed(cmd, crate::utils::process::HOST_PROBE_TIMEOUT)
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::TimedOut, message))
}

/// Blocking `smartctl` probe with the wider SMART deadline.
fn run_smart_blocking(cmd: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    crate::utils::process::run_timed(cmd, crate::utils::process::SMART_PROBE_TIMEOUT)
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::TimedOut, message))
}

/// Stores previous per-core CPU stats for delta calculation.
static PREV_PER_CORE: Lazy<RwLock<HashMap<String, (u64, u64)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// How long a disk-health snapshot is reused before the probes run again.
///
/// The browser polls health every 5 s and each collection forks `lsblk`, `df`
/// and up to three commands per disk; without this the same 5 s tick re-runs
/// the whole SMART fan-out. A 30 s snapshot decouples the cheap JSON poll from
/// the expensive hardware probes and is what keeps requests from piling up
/// behind a slow drive (ROADMAP 15.2).
const DISK_HEALTH_TTL: Duration = Duration::from_secs(30);

/// Last disk-health collection, coalesced: concurrent callers wait on the
/// mutex and the second one reuses the first one's snapshot.
static DISK_HEALTH_SNAPSHOT: Lazy<tokio::sync::Mutex<Option<DiskHealthSnapshot>>> =
    Lazy::new(|| tokio::sync::Mutex::new(None));

struct DiskHealthSnapshot {
    fetched_at: Instant,
    disks: Vec<DiskHealth>,
    filesystems: Vec<FsHealth>,
}

/// Is a snapshot taken at `fetched_at` still within `ttl` at `now`?
fn snapshot_is_fresh(fetched_at: Instant, now: Instant, ttl: Duration) -> bool {
    now.saturating_duration_since(fetched_at) < ttl
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthData {
    pub score: u32,
    pub status: String,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub memory_total: u64,
    pub memory_used: u64,
    pub disk_summary: DiskSummary,
    pub disks: Vec<DiskHealth>,
    pub filesystems: Vec<FsHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSummary {
    pub total_count: usize,
    pub healthy_count: usize,
    pub warning_count: usize,
    pub critical_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskHealth {
    pub name: String,
    pub device: String,
    pub model: String,
    pub size_bytes: u64,
    pub size_human: String,
    pub smart_status: String,
    pub temperature: Option<f64>,
    pub role: String,
    pub usage_percent: f64,
    pub health_score: u32,
    pub connection_type: String,
    /// Result of the most recent scheduled/manual SMART self-test
    /// (`smartctl -l selftest`), distinct from `smart_status` (the live
    /// overall-health self-assessment recomputed on every poll). `None`
    /// when the drive has never had a self-test run, which is the common
    /// case on a NAS with no self-test schedule configured.
    pub last_self_test: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsHealth {
    pub mount: String,
    pub filesystem: String,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub size_human: String,
    pub used_human: String,
    pub available_human: String,
    pub usage_percent: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    pub status: String,
    pub title: String,
    pub app_name: String,
    pub widgets: Vec<Widget>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddWidgetRequest {
    pub id: String,
    pub wtype: String,
    pub title: String,
    pub size: String,
    #[serde(default = "default_row")]
    pub row: u32,
    #[serde(default = "default_col")]
    pub col: u32,
    #[serde(default = "default_span")]
    pub row_span: u32,
    #[serde(default = "default_span")]
    pub col_span: u32,
}

fn default_row() -> u32 {
    1
}
fn default_col() -> u32 {
    1
}
fn default_span() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReorderWidgetsRequest {
    pub widgets: Vec<String>,
}

// ---------------------------------------------------------------------------
// Widget tiles (ROADMAP 15.1)
//
// A tile belongs to exactly one account (`widgets` is keyed by
// `(username, id)`), every handler resolves the owner through `require_auth`
// and passes that username to the repository, and every client-supplied field
// is validated here before it reaches SQL. A tile the caller does not own is
// indistinguishable from a missing one (404), so the endpoint never confirms
// that another account has a tile with that id.
// ---------------------------------------------------------------------------

/// Longest widget id / type tag accepted from a client.
pub const MAX_WIDGET_ID_LEN: usize = 64;
/// Longest tile title accepted from a client.
pub const MAX_WIDGET_TITLE_LEN: usize = 120;
/// Highest row/column a client may place a tile on.
pub const MAX_WIDGET_POSITION: u32 = 64;
/// Largest row/column span a tile may declare (`size` is `NxN`).
pub const MAX_WIDGET_SPAN: u32 = 6;
/// Most tiles one account may keep, so a client cannot fill the table.
pub const MAX_WIDGETS_PER_USER: i64 = 64;

/// A widget id / type tag is a short, opaque slug: no whitespace, no path
/// separators, no quotes. Keeping the charset narrow is what makes the id
/// safe to echo in a message and to use as a map key.
fn is_widget_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_WIDGET_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// `size` is the grid footprint (`1x1`, `2x2`, …). Anything else — including a
/// valid CSS value — is refused: the grid renders spans, not free-form CSS.
fn validate_widget_size(size: &str) -> Result<(), String> {
    let (rows, cols) = size
        .split_once('x')
        .ok_or_else(|| format!("Invalid widget size '{size}': expected NxN"))?;
    let parse = |part: &str| -> Result<u32, String> {
        part.parse::<u32>()
            .map_err(|_| format!("Invalid widget size '{size}': expected NxN"))
    };
    let (rows, cols) = (parse(rows)?, parse(cols)?);
    if rows == 0 || cols == 0 || rows > MAX_WIDGET_SPAN || cols > MAX_WIDGET_SPAN {
        return Err(format!(
            "Invalid widget size '{size}': each side must be 1..={MAX_WIDGET_SPAN}"
        ));
    }
    Ok(())
}

/// Validate one new tile before it reaches the database.
///
/// Rejects an unsafe id/type tag, an empty or over-long title, a control
/// character in the title, an unparsable `size`, and a position/span outside
/// the grid. Every message names the offending field, which is what the UI
/// shows back to the user.
pub fn validate_add_widget(req: &AddWidgetRequest) -> Result<(), String> {
    if !is_widget_slug(&req.id) {
        return Err(format!(
            "Invalid widget id: 1-{MAX_WIDGET_ID_LEN} characters from A-Z, a-z, 0-9, '-', '_', '.' or ':'"
        ));
    }
    if !is_widget_slug(&req.wtype) {
        return Err(format!(
            "Invalid widget type: 1-{MAX_WIDGET_ID_LEN} characters from A-Z, a-z, 0-9, '-', '_', '.' or ':'"
        ));
    }
    let title = req.title.trim();
    if title.is_empty() {
        return Err("Widget title cannot be empty".to_string());
    }
    if title.chars().count() > MAX_WIDGET_TITLE_LEN {
        return Err(format!(
            "Widget title is too long (max {MAX_WIDGET_TITLE_LEN} characters)"
        ));
    }
    if title.chars().any(|c| c.is_control()) {
        return Err("Widget title cannot contain control characters".to_string());
    }
    validate_widget_size(&req.size)?;

    for (label, value) in [
        ("row", req.row),
        ("col", req.col),
        ("row_span", req.row_span),
        ("col_span", req.col_span),
    ] {
        if value == 0 || value > MAX_WIDGET_POSITION {
            return Err(format!(
                "Invalid widget {label}: must be 1..={MAX_WIDGET_POSITION}"
            ));
        }
    }
    if req.row_span > MAX_WIDGET_SPAN || req.col_span > MAX_WIDGET_SPAN {
        return Err(format!(
            "Invalid widget span: each side must be 1..={MAX_WIDGET_SPAN}"
        ));
    }
    Ok(())
}

/// Bounds for the floating-panel positions kept in `user_widget_preferences`:
/// the screen the panels live on is far smaller than this, but the value is
/// still made finite and bounded instead of trusted verbatim.
pub const MAX_WIDGET_PANEL_OFFSET: i32 = 10_000;

/// Validate an incoming widget preference (panel position + visibility).
pub fn validate_widget_preference(req: &UpdateWidgetPreferenceRequest) -> Result<(), String> {
    if !is_widget_slug(&req.widget_key) {
        return Err(format!(
            "Invalid widget key: 1-{MAX_WIDGET_ID_LEN} characters from A-Z, a-z, 0-9, '-', '_', '.' or ':'"
        ));
    }
    for (label, value) in [
        ("position_x", req.position_x),
        ("position_y", req.position_y),
    ] {
        if !(-MAX_WIDGET_PANEL_OFFSET..=MAX_WIDGET_PANEL_OFFSET).contains(&value) {
            return Err(format!(
                "Invalid {label}: must be between -{MAX_WIDGET_PANEL_OFFSET} and {MAX_WIDGET_PANEL_OFFSET}"
            ));
        }
    }
    Ok(())
}

/// The caller's widget grid (never another account's).
pub async fn handle_dashboard_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    let widgets: Vec<Widget> = state
        .widgets
        .find_by_username(&username)
        .await
        .unwrap_or_default();

    let data = DashboardData {
        status: "ok".to_string(),
        title: "Dashboard".to_string(),
        app_name: "HomeVault".to_string(),
        widgets,
        timestamp: Utc::now().to_rfc3339(),
    };

    ResponseHelper::json_success(data)
}

/// Live data for one tile type.
///
/// Replies 404 for an unknown id instead of a `type: "unknown"` shell: the
/// desktop asks for ids it knows about, so an unknown one is a client bug, and
/// answering 200 with an empty payload is how a placeholder card used to
/// appear. `memory` reports real `/proc/meminfo` figures (ROADMAP 15.3).
pub async fn handle_widget_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    let widget_data = match id.as_str() {
        "cpu" => {
            let loadavg = ProcParser::parse_loadavg();
            let usage = crate::utils::proc::cpu_usage_percent_since_last_sample(&username);
            serde_json::json!({
                "type": "cpu",
                "title": "CPU Usage",
                "data": {
                    "loadavg": loadavg,
                    "usage_percent": usage
                }
            })
        }
        "memory" => {
            let mem = ProcParser::parse_meminfo();
            let total = mem.total_kb * 1024;
            let available = mem.available_kb * 1024;
            let used = total.saturating_sub(available);
            let usage_percent = if mem.total_kb > 0 {
                (used as f64 / total as f64 * 100.0).round() as u32
            } else {
                0
            };
            serde_json::json!({
                "type": "memory",
                "title": "RAM Usage",
                "data": {
                    "total_bytes": total,
                    "used_bytes": used,
                    "available_bytes": available,
                    "usage_percent": usage_percent
                }
            })
        }
        "uptime" => {
            let uptime = ProcParser::parse_uptime();
            serde_json::json!({
                "type": "uptime",
                "title": "Server Uptime",
                "data": {
                    "uptime_raw": uptime.to_string(),
                    "uptime_seconds": uptime,
                    "uptime_human": format_uptime(uptime)
                }
            })
        }
        _ => {
            return ResponseHelper::json_error_with_status(
                axum::http::StatusCode::NOT_FOUND,
                format!("Unknown widget '{id}'"),
            );
        }
    };

    ResponseHelper::json_success(widget_data)
}

/// Add a tile to the caller's own grid.
pub async fn handle_add_widget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(req): AxumJson<AddWidgetRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    if let Err(message) = validate_add_widget(&req) {
        return ResponseHelper::json_error(message);
    }

    match state.widgets.count_for(&username).await {
        Ok(count) if count >= MAX_WIDGETS_PER_USER => {
            return ResponseHelper::json_error(format!(
                "You already have the maximum of {MAX_WIDGETS_PER_USER} widgets"
            ));
        }
        Ok(_) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to add widget: {e}")),
    }

    // Only the caller's own tiles are probed, so this cannot leak whether
    // another account already uses the id.
    match state.widgets.find_by_id(&username, &req.id).await {
        Ok(Some(_)) => {
            return ResponseHelper::json_error(format!(
                "Widget with ID '{}' already exists",
                req.id
            ));
        }
        Ok(None) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to add widget: {e}")),
    }

    let widget = Widget {
        id: req.id,
        wtype: req.wtype,
        title: req.title.trim().to_string(),
        size: req.size,
        row_pos: req.row,
        col_pos: req.col,
        row_span: req.row_span,
        col_span: req.col_span,
        username,
    };

    match state.widgets.create(&widget).await {
        Ok(_) => ResponseHelper::json_success("Widget added successfully".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to add widget: {}", e)),
    }
}

/// Remove one of the caller's tiles. 404 when the id is not theirs.
pub async fn handle_remove_widget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    match state.widgets.delete_for(&username, &id).await {
        Ok(true) => ResponseHelper::json_success(format!("Widget {id} removed")),
        Ok(false) => ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            format!("Widget '{id}' not found"),
        ),
        Err(e) => ResponseHelper::json_error(format!("Failed to remove widget: {}", e)),
    }
}

/// Reorder the caller's grid. Ids they do not own are ignored; a list that
/// matches none of their tiles is a 404 rather than a silent success.
pub async fn handle_reorder_widgets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(req): AxumJson<ReorderWidgetsRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    if req.widgets.len() as i64 > MAX_WIDGETS_PER_USER {
        return ResponseHelper::json_error(format!(
            "Too many widgets in one reorder (max {MAX_WIDGETS_PER_USER})"
        ));
    }
    if let Some(bad) = req.widgets.iter().find(|id| !is_widget_slug(id)) {
        return ResponseHelper::json_error(format!("Invalid widget id '{bad}'"));
    }

    match state.widgets.reorder_for(&username, &req.widgets).await {
        Ok(0) if !req.widgets.is_empty() => ResponseHelper::json_error_with_status(
            axum::http::StatusCode::NOT_FOUND,
            "None of those widgets belong to you".to_string(),
        ),
        Ok(_) => ResponseHelper::json_success("Widgets reordered successfully".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to reorder widgets: {}", e)),
    }
}

/// The caller's widget grid (alias of `/dashboard/data`'s `widgets` field).
pub async fn handle_get_all_widgets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    match state.widgets.find_by_username(&username).await {
        Ok(widgets) => ResponseHelper::json_success(widgets),
        Err(e) => ResponseHelper::json_error(format!("Failed to get widgets: {}", e)),
    }
}

pub async fn handle_network_traffic_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let content = ProcParser::read_proc_file("/proc/net/dev");
    let lines: Vec<&str> = content.lines().skip(2).collect();

    let mut rx_bytes: u64 = 0;
    let mut tx_bytes: u64 = 0;

    for line in lines {
        if let Some(colon_pos) = line.find(':') {
            let iface_name = line[..colon_pos].split_whitespace().next().unwrap_or("");
            if !iface_name.is_empty() && iface_name != "lo" {
                let after_colon = &line[colon_pos + 1..];
                let mut stats = after_colon.split_whitespace();
                if let (Some(rx), Some(tx)) = (stats.next(), stats.nth(7)) {
                    if let (Ok(rx_val), Ok(tx_val)) = (rx.parse::<u64>(), tx.parse::<u64>()) {
                        rx_bytes += rx_val;
                        tx_bytes += tx_val;
                    }
                }
            }
        }
    }

    let data = serde_json::json!({
        "id": "network",
        "type": "network_traffic",
        "title": "Network Traffic",
        "rx_bytes": rx_bytes,
        "tx_bytes": tx_bytes,
        "rx_mb": rx_bytes / (1024 * 1024),
        "tx_mb": tx_bytes / (1024 * 1024)
    });

    ResponseHelper::json_success(data)
}

pub async fn handle_disk_io_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let content = ProcParser::read_proc_file("/proc/diskstats");
    let mut total_read_sectors: u64 = 0;
    let mut total_write_sectors: u64 = 0;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 11 {
            if let (Ok(read), Ok(write)) = (parts[5].parse::<u64>(), parts[9].parse::<u64>()) {
                total_read_sectors += read;
                total_write_sectors += write;
            }
        }
    }

    let data = serde_json::json!({
        "id": "disk_io",
        "type": "disk_io",
        "title": "Disk I/O",
        "read_sectors": total_read_sectors,
        "write_sectors": total_write_sectors,
        "read_mb": (total_read_sectors * 512) / (1024 * 1024),
        "write_mb": (total_write_sectors * 512) / (1024 * 1024)
    });

    ResponseHelper::json_success(data)
}

pub async fn handle_storage_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let output = run_probe(Command::new("df").args([
        "-h",
        "--output=source,size,used,avail,pcent,target",
        "-x",
        "tmpfs",
        "-x",
        "devtmpfs",
        "-x",
        "squashfs",
    ]))
    .await;

    let filesystems = if let Ok(output) = output {
        let output_str = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = output_str.lines().skip(1).collect();

        lines
            .iter()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 6 {
                    Some(serde_json::json!({
                        "mount": parts[5],
                        "size": parts[1],
                        "used": parts[2],
                        "available": parts[3],
                        "percent": parts[4]
                    }))
                } else {
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let data = serde_json::json!({
        "id": "storage",
        "type": "storage_overview",
        "title": "Storage Overview",
        "filesystems": filesystems
    });

    ResponseHelper::json_success(data)
}

pub async fn handle_cpu_temperature(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let temp = get_cpu_temperature();

    let data = serde_json::json!({
        "id": "cpu_temp",
        "type": "cpu_temperature",
        "title": "CPU Temperature",
        "temperature": temp,
        "unit": "C"
    });

    ResponseHelper::json_success(data)
}

fn get_cpu_temperature() -> f64 {
    // Try to read from thermal zones
    let thermal_paths = [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/thermal/thermal_zone1/temp",
        "/sys/class/thermal/thermal_zone2/temp",
    ];

    let mut temps = Vec::new();

    for path in &thermal_paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(millidegrees) = content.trim().parse::<i32>() {
                // Convert millidegrees to degrees Celsius
                let celsius = millidegrees as f64 / 1000.0;
                temps.push(celsius);
            }
        }
    }

    // Return the maximum temperature (hottest zone)
    if !temps.is_empty() {
        temps.iter().fold(0.0f64, |a, &b| a.max(b))
    } else {
        0.0
    }
}

/// Returns comprehensive health data including disk status.
pub async fn handle_health_data(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    let (cpu_usage, memory_usage, memory_total, memory_used) = get_system_metrics(&username);
    let (disks, filesystems) = get_disk_health().await;

    // Calculate disk health score
    let disk_summary = calculate_disk_summary(&disks);
    let disk_score = calculate_disk_score(&disks, &filesystems);

    // Calculate overall health score
    let cpu_score = if cpu_usage < 70.0 {
        100.0 - cpu_usage
    } else {
        (100.0 - cpu_usage) * 0.5
    }
    .max(0.0);
    let mem_score = if memory_usage < 70.0 {
        100.0 - memory_usage
    } else {
        (100.0 - memory_usage) * 0.5
    }
    .max(0.0);
    let overall_score = ((cpu_score * 0.25) + (mem_score * 0.25) + (disk_score * 0.5)) as u32;

    // One vocabulary across the dashboard payload, matching what the UI
    // declares (`healthy | warning | unhealthy | unknown`); "critical" was a
    // fifth word nothing rendered (ROADMAP 15.3).
    let status = health_status_for_score(overall_score).to_string();

    let health_data = HealthData {
        score: overall_score,
        status,
        cpu_usage,
        memory_usage,
        memory_total,
        memory_used,
        disk_summary,
        disks,
        filesystems,
    };

    ResponseHelper::json_success(health_data)
}

/// Mean of the per-core readings, rounded; `0` when no core answered.
fn mean_core_usage(cores: &[PerCoreUsageItem]) -> u32 {
    if cores.is_empty() {
        return 0;
    }
    let total: u32 = cores.iter().map(|c| c.usage_percent).sum();
    ((total as f64) / (cores.len() as f64)).round() as u32
}

/// Map an overall health score to the dashboard's status vocabulary.
///
/// `healthy` at 70+, `warning` at 40-69, `unhealthy` below. Extracted so the
/// payload and its tests share one definition instead of two branches that
/// can drift apart.
fn health_status_for_score(score: u32) -> &'static str {
    if score >= 70 {
        "healthy"
    } else if score >= 40 {
        "warning"
    } else {
        "unhealthy"
    }
}

/// CPU and memory usage plus memory totals.
///
/// CPU is a real `/proc/stat` delta for this caller (ROADMAP 15.3), not
/// `load1 / cores` — a load figure can exceed 100 % and contradicts the
/// Metrics app, which already reports deltas.
fn get_system_metrics(username: &str) -> (f64, f64, u64, u64) {
    let cpu_usage = crate::utils::proc::cpu_usage_percent_since_last_sample(username)
        .map(|percent| percent.clamp(0.0, 100.0))
        .unwrap_or(0.0);

    let mem_info = ProcParser::parse_meminfo();
    let memory_total = mem_info.total_kb * 1024;
    let memory_used = mem_info.total_kb.saturating_sub(mem_info.available_kb) * 1024;
    let memory_usage = if mem_info.total_kb > 0 {
        memory_used as f64 / memory_total as f64 * 100.0
    } else {
        0.0
    };

    (cpu_usage, memory_usage, memory_total, memory_used)
}

/// Disk and filesystem health, from a snapshot at most [`DISK_HEALTH_TTL`] old.
async fn get_disk_health() -> (Vec<DiskHealth>, Vec<FsHealth>) {
    let mut snapshot = DISK_HEALTH_SNAPSHOT.lock().await;
    if let Some(cached) = snapshot.as_ref() {
        if snapshot_is_fresh(cached.fetched_at, Instant::now(), DISK_HEALTH_TTL) {
            return (cached.disks.clone(), cached.filesystems.clone());
        }
    }

    let (disks, filesystems) = collect_disk_health().await;
    *snapshot = Some(DiskHealthSnapshot {
        fetched_at: Instant::now(),
        disks: disks.clone(),
        filesystems: filesystems.clone(),
    });
    (disks, filesystems)
}

/// Run the actual probes (lsblk, df, one SMART query per disk, per-disk role
/// and usage lookups).
async fn collect_disk_health() -> (Vec<DiskHealth>, Vec<FsHealth>) {
    let mut disks = Vec::new();
    let mut filesystems = Vec::new();

    // Get disk list using lsblk
    if let Ok(lsblk_output) = run_probe(Command::new("lsblk").args([
        "-d",
        "-n",
        "-o",
        "NAME,SIZE,TYPE,VENDOR,MODEL,SERIAL,TRAN",
    ]))
    .await
    {
        // Parse every disk line first, then look up SMART/role/usage for all
        // of them concurrently (3 subprocess-backed async calls per disk,
        // previously all sequential — for N disks that's up to 3N round
        // trips run one after another). tokio::process::Command already
        // yields the runtime while a subprocess runs rather than blocking a
        // worker thread, so this join is purely a latency win (this endpoint
        // is polled every 5s while the Dashboard is open), not a
        // correctness or thread-starvation fix.
        let stdout = String::from_utf8_lossy(&lsblk_output.stdout).into_owned();
        let disk_lines: Vec<(String, String, u64, String, String, String)> = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3
                    && parts[2] == "disk"
                    && !crate::disk::DiskService::is_pseudo_disk_name(parts[0])
                {
                    let name = parts[0].to_string();
                    let device = format!("/dev/{}", name);
                    let size = parse_size(parts.get(1).unwrap_or(&"0"));
                    let vendor = parts.get(3).unwrap_or(&"Unknown").to_string();
                    let model = parts.get(4).unwrap_or(&"Unknown").to_string();
                    let connection_type = parts.get(6).unwrap_or(&"Unknown").to_string();
                    Some((name, device, size, vendor, model, connection_type))
                } else {
                    None
                }
            })
            .collect();

        let disk_futures = disk_lines.into_iter().map(
            |(name, device, size, vendor, model, connection_type)| async move {
                // These four are independent per-disk lookups — run them
                // concurrently instead of one after another.
                let (smart, role, usage_percent, last_self_test) = tokio::join!(
                    get_smart_status(&name, &connection_type),
                    detect_disk_role(&name),
                    get_disk_usage_percent(&name),
                    get_last_self_test(&name, &connection_type)
                );
                let (smart_status, temperature) = smart;
                let health_score =
                    calculate_individual_disk_score(&smart_status, temperature, usage_percent);

                DiskHealth {
                    name: name.clone(),
                    device,
                    model: format!("{} {}", vendor, model).trim().to_string(),
                    size_bytes: size,
                    size_human: format_bytes(size),
                    smart_status,
                    temperature,
                    role,
                    usage_percent,
                    health_score,
                    connection_type,
                    last_self_test,
                }
            },
        );

        disks = futures_util::future::join_all(disk_futures).await;
    }

    // Get filesystem data using df
    if let Ok(df_output) = run_probe(Command::new("df").args([
        "-B1",
        "--output=source,fstype,size,used,avail,target",
        "-x",
        "tmpfs",
        "-x",
        "devtmpfs",
        "-x",
        "squashfs",
    ]))
    .await
    {
        let stdout = String::from_utf8_lossy(&df_output.stdout);
        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let _source = parts[0].to_string();
                let fstype = parts[1].to_string();
                let size: u64 = parts[2].parse().unwrap_or(0);
                let used: u64 = parts[3].parse().unwrap_or(0);
                let available: u64 = parts[4].parse().unwrap_or(0);
                let mount = parts[5].to_string();

                let usage_percent = if size > 0 {
                    used as f64 / size as f64 * 100.0
                } else {
                    0.0
                };

                let status = if usage_percent > 95.0 {
                    "unhealthy".to_string()
                } else if usage_percent > 85.0 {
                    "warning".to_string()
                } else {
                    "healthy".to_string()
                };

                filesystems.push(FsHealth {
                    mount,
                    filesystem: fstype,
                    size_bytes: size,
                    used_bytes: used,
                    available_bytes: available,
                    size_human: format_bytes(size),
                    used_human: format_bytes(used),
                    available_human: format_bytes(available),
                    usage_percent,
                    status,
                });
            }
        }
    }

    (disks, filesystems)
}

async fn get_smart_status(device_name: &str, connection_type: &str) -> (String, Option<f64>) {
    let device = format!("/dev/{}", device_name);

    // Get SMART health status: the drive's own live overall-health
    // self-assessment, recomputed by its firmware on every query — not a
    // cached/stale value. `smartctl` needs root to open a real
    // `660 root:disk` block device (see setup.rs::configure_sudoers) — run
    // unprivileged, this always failed silently and fell through to the
    // hardcoded "Unknown" default below, regardless of the disk's real
    // health. `homevault` is delegated root for `smartctl` via sudoers the
    // same way it is for `mdadm`/`blkid`.
    let stdout = run_smartctl(&device, connection_type, &["-H"], |s| {
        s.contains("PASSED") || s.contains("OK") || s.contains("FAILED")
    })
    .await;

    let mut smart_status = "Unknown".to_string();
    if let Some(stdout) = &stdout {
        if stdout.contains("PASSED") || stdout.contains("OK") {
            smart_status = "Healthy".to_string();
        } else if stdout.contains("FAILED") {
            smart_status = "Failed".to_string();
        }
    }

    // Get temperature
    let temperature = get_smart_temperature(&device, connection_type).await;

    (smart_status, temperature)
}

async fn get_smart_temperature(device: &str, connection_type: &str) -> Option<f64> {
    let stdout = run_smartctl(device, connection_type, &["-A"], |s| {
        s.contains("Temperature")
    })
    .await?;

    for line in stdout.lines() {
        if line.contains("Temperature_Celsius") || line.contains("Airflow_Temperature") {
            let parts: Vec<&str> = line.split_whitespace().filter(|s| !s.is_empty()).collect();
            if let Some(temp_str) = parts.last() {
                if let Ok(temp) = temp_str.parse::<f64>() {
                    return Some(temp);
                }
            }
        }
    }
    None
}

/// Result of the most recent scheduled/manual SMART self-test
/// (`smartctl -l selftest`'s most recent log entry), as distinct from the
/// live overall-health self-assessment `get_smart_status` reports. `None`
/// when the drive has no self-test log at all — the common case on a NAS
/// with no self-test schedule configured, and the only possible outcome
/// for many NVMe SSDs (self-test logging is an optional NVMe feature) and
/// for USB flash drives (no SMART capability over USB at all).
async fn get_last_self_test(device_name: &str, connection_type: &str) -> Option<String> {
    let device = format!("/dev/{}", device_name);
    let stdout = run_smartctl(&device, connection_type, &["-l", "selftest"], |s| {
        s.lines().any(|l| l.trim_start().starts_with('#'))
    })
    .await?;

    // The log lists the most recent entry first.
    let line = stdout.lines().find(|l| l.trim_start().starts_with('#'))?;
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let percent_idx = tokens.iter().position(|t| t.ends_with('%'))?;
    let hours = tokens
        .get(percent_idx + 1)
        .and_then(|t| t.parse::<u64>().ok());

    let status = if line.contains("Completed without error") {
        "Completed without error"
    } else if line.contains("in progress") {
        "In progress"
    } else if line.contains("Interrupted") {
        "Interrupted"
    } else if line.contains("Aborted") {
        "Aborted"
    } else {
        "Completed with errors"
    };

    match hours {
        Some(h) => Some(format!("{status} ({h}h)")),
        None => Some(status.to_string()),
    }
}

/// Run `smartctl` as root against `device`, retrying with an explicit
/// `-d sat` device-type override for USB-attached disks (mSATA/SATA behind
/// a USB bridge, e.g. this host's ASMT mSATA and PNY USB devices) when the
/// plain auto-detected call reports nothing usable per `is_usable` —
/// `smartctl`'s device autodetection frequently can't see through a
/// USB-SATA bridge on its own, while native SATA/NVMe devices are
/// unaffected by this and never hit the retry. Returns `None` if the
/// device genuinely has no data for this query (e.g. a plain USB flash
/// drive, which has no SMART capability at all) or the command fails
/// outright.
async fn run_smartctl(
    device: &str,
    connection_type: &str,
    args: &[&str],
    is_usable: impl Fn(&str) -> bool,
) -> Option<String> {
    // A SMART query is the slowest probe on the box (a drive may have to spin
    // up), so it gets the wider deadline; on expiry the child is killed and
    // the tile reports "Unknown" instead of the request hanging (ROADMAP 15.2).
    let mut cmd = crate::smart::tokio_command(device, args);
    let output = crate::utils::process::run_timed_async(
        &mut cmd,
        crate::utils::process::SMART_PROBE_TIMEOUT,
    )
    .await?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    if is_usable(&stdout) {
        return Some(stdout);
    }

    if connection_type.eq_ignore_ascii_case("usb") {
        // A bare autodetect frequently can't see through a USB-SATA/mSATA
        // bridge; retry with the common SAT (SCSI-ATA Translation) override
        // before giving up. Native SATA/NVMe devices already returned above
        // and never reach this branch.
        //
        // `-d sat` goes *first* so the whole argv keeps the shape the sudoers
        // grant pins (`smartctl -d * /dev/*`): appended after `-A`, the retry
        // was `smartctl -A -d sat /dev/…`, which no grant matched and which
        // therefore failed with a sudo denial on every USB bridge
        // (ROADMAP 7.4).
        let mut sat_args: Vec<&str> = vec!["-d", "sat"];
        sat_args.extend(args);
        let mut sat_cmd = crate::smart::tokio_command(device, &sat_args);
        if let Some(sat_output) = crate::utils::process::run_timed_async(
            &mut sat_cmd,
            crate::utils::process::SMART_PROBE_TIMEOUT,
        )
        .await
        {
            let sat_stdout = String::from_utf8_lossy(&sat_output.stdout).into_owned();
            if is_usable(&sat_stdout) {
                return Some(sat_stdout);
            }
        }
    }

    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

async fn detect_disk_role(device_name: &str) -> String {
    let device = format!("/dev/{}", device_name);

    // Check if part of RAID. `mdadm --examine` needs root even for this
    // read-only probe ("must be super-user" otherwise), so a bare call
    // silently failed and every array member was reported as a plain disk
    // (ROADMAP 7.4).
    if let Ok(output) = run_probe(Command::new("sudo").args(["mdadm", "--examine", &device])).await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("raid") {
            return "RAID".to_string();
        }
    }

    // Check if part of ZFS — same `sudo` reason as `mdadm --examine` above.
    if let Ok(output) = run_probe(Command::new("sudo").args(["zpool", "status", &device])).await {
        if output.status.success() {
            return "ZFS Pool".to_string();
        }
    }

    // Check mount points
    if let Ok(output) =
        run_probe(Command::new("lsblk").args(["-n", "-o", "MOUNTPOINT", &device])).await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mount = stdout.trim();
        if mount == "/" {
            return "System".to_string();
        } else if !mount.is_empty() {
            return "Data".to_string();
        }
    }

    "Unassigned".to_string()
}

async fn get_disk_usage_percent(device_name: &str) -> f64 {
    // Get partitions for this disk and calculate average usage
    if let Ok(output) = run_probe(Command::new("lsblk").args(["-n", "-o", "NAME,MOUNTPOINT"])).await
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut total_usage = 0.0;
        let mut count = 0;

        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2
                && parts[0].starts_with(device_name)
                && !parts[1].is_empty()
                && parts[1] != "MOUNTPOINT"
            {
                let partition = format!("/dev/{}", parts[0]);
                if let Ok(df_output) = run_probe(Command::new("df").args(["-B1", &partition])).await
                {
                    let df_stdout = String::from_utf8_lossy(&df_output.stdout);
                    let lines: Vec<&str> = df_stdout.lines().collect();
                    if lines.len() >= 2 {
                        let df_parts: Vec<&str> = lines[1].split_whitespace().collect();
                        if df_parts.len() >= 4 {
                            let size: f64 = df_parts[1].parse().unwrap_or(1.0);
                            let used: f64 = df_parts[2].parse().unwrap_or(0.0);
                            if size > 0.0 {
                                total_usage += (used / size) * 100.0;
                                count += 1;
                            }
                        }
                    }
                }
            }
        }

        if count > 0 {
            return total_usage / count as f64;
        }
    }
    0.0
}

/// Per-disk health score from the three things actually measured.
///
/// SMART verdict (40), temperature (20) and space usage (40) sum to 100, so
/// there is no leftover to fill: the old score added a hardcoded
/// `error_score = 10` that no disk ever earned, which lifted a failing disk
/// off the floor and capped the grade below 100 for a perfect one (ROADMAP
/// 15.3). Unknown temperature keeps half credit — the same policy as an
/// unknown SMART verdict — because a probe that could not answer is not
/// evidence of a fault.
fn calculate_individual_disk_score(
    smart_status: &str,
    temperature: Option<f64>,
    usage_percent: f64,
) -> u32 {
    let smart_score = match smart_status {
        "Healthy" => 40,
        "Warning" => 20,
        "Failed" => 0,
        _ => 20,
    };

    let temp_score: u32 = match temperature {
        Some(temp) if temp < 50.0 => 20,
        Some(temp) if temp < 70.0 => 15,
        Some(temp) if temp < 85.0 => 10,
        Some(_) => 0,
        None => 10,
    };

    let usage_score: u32 = if usage_percent < 70.0 {
        40
    } else if usage_percent < 85.0 {
        26
    } else if usage_percent < 95.0 {
        13
    } else {
        0
    };

    (smart_score + temp_score + usage_score).min(100)
}

fn calculate_disk_summary(disks: &[DiskHealth]) -> DiskSummary {
    let mut healthy = 0;
    let mut warning = 0;
    let mut critical = 0;

    for disk in disks {
        match disk.health_score {
            70..=100 => healthy += 1,
            40..=69 => warning += 1,
            _ => critical += 1,
        }
    }

    DiskSummary {
        total_count: disks.len(),
        healthy_count: healthy,
        warning_count: warning,
        critical_count: critical,
    }
}

fn calculate_disk_score(disks: &[DiskHealth], filesystems: &[FsHealth]) -> f64 {
    if disks.is_empty() {
        return 100.0;
    }

    // Calculate average disk health
    let disk_avg: f64 =
        disks.iter().map(|d| d.health_score as f64).sum::<f64>() / disks.len() as f64;

    // Calculate filesystem health based on critical usage
    let fs_score = if filesystems.iter().any(|f| f.usage_percent > 95.0) {
        0.0
    } else if filesystems.iter().any(|f| f.usage_percent > 85.0) {
        50.0
    } else {
        100.0
    };

    // Combine scores (60% disk health, 40% filesystem health)
    (disk_avg * 0.6 + fs_score * 0.4).min(100.0)
}

fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    let multiplier: u64 = if s.ends_with('G') {
        1024 * 1024 * 1024
    } else if s.ends_with('M') {
        1024 * 1024
    } else if s.ends_with('K') || s.ends_with("KB") {
        1024
    } else if s.ends_with('T') {
        1024 * 1024 * 1024 * 1024
    } else if s.ends_with('P') {
        1024 * 1024 * 1024 * 1024 * 1024
    } else {
        1
    };

    // `df -h` prints fractional, locale-formatted sizes ("9.8G", and
    // "465,8G" under a French locale). Two bugs, both already fixed the
    // same way in `disk.rs::parse_size` / `pools.rs::parse_size`:
    //   1. `str::parse::<f64>` only accepts '.', so ',' silently gave 0 —
    //      normalize ',' → '.' first.
    //   2. casting the parsed value to u64 *before* multiplying dropped the
    //      fractional part up front ("9.8G" reported as "9G"), undercounting
    //      by up to ~1 TiB on a T-scale size — multiply in f64, cast last.
    let num_str = s
        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != ',')
        .replace(',', ".");
    (num_str.parse::<f64>().unwrap_or(0.0) * multiplier as f64) as u64
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1}T", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

pub async fn handle_storage_volumes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let volumes = get_raid_volumes().await;

    let data = serde_json::json!({
        "id": "storage_volumes",
        "type": "storage_volumes",
        "title": "RAID Arrays",
        "volumes": volumes
    });

    ResponseHelper::json_success(data)
}

#[derive(Debug, Clone, Serialize)]
struct StorageVolume {
    name: String,
    raid_type: String,
    mount_point: String,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
    total_human: String,
    used_human: String,
    available_human: String,
    usage_percent: f64,
    status: String,
}

async fn get_raid_volumes() -> Vec<StorageVolume> {
    let mut volumes = Vec::new();

    // Check for mdadm RAID arrays
    if let Ok(mdstat_content) = tokio::fs::read_to_string("/proc/mdstat").await {
        for line in mdstat_content.lines() {
            if line.starts_with("md") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let device_name = parts[0].to_string();
                    let raid_level = parts[3].to_string();

                    // Get mount point and usage for this RAID device
                    if let Some(volume) = get_raid_device_info(&device_name, &raid_level).await {
                        volumes.push(volume);
                    }
                }
            }
        }
    }

    // Check for ZFS pools. `sudo` for the same reason every Storage call
    // needs it: `zpool` opens `/dev` nodes that are `660 root:disk`, and the
    // service account is not in the `disk` group — without it this read
    // silently reported "no pool" on a host that has one (ROADMAP 7.4).
    if let Ok(output) = run_probe(Command::new("sudo").args(["zpool", "list", "-H", "-p"])).await {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 4 {
                    let name = parts[0].to_string();
                    let size: u64 = parts[1].parse().unwrap_or(0);
                    let used: u64 = parts[2].parse().unwrap_or(0);
                    let available = size.saturating_sub(used);

                    let usage_percent = if size > 0 {
                        used as f64 / size as f64 * 100.0
                    } else {
                        0.0
                    };

                    let status = if usage_percent > 90.0 {
                        "unhealthy".to_string()
                    } else if usage_percent > 80.0 {
                        "warning".to_string()
                    } else {
                        "healthy".to_string()
                    };

                    volumes.push(StorageVolume {
                        name: name.clone(),
                        raid_type: "ZFS Pool".to_string(),
                        mount_point: format!("/{}", name),
                        total_bytes: size,
                        used_bytes: used,
                        available_bytes: available,
                        total_human: format_bytes(size),
                        used_human: format_bytes(used),
                        available_human: format_bytes(available),
                        usage_percent,
                        status,
                    });
                }
            }
        }
    }

    // Btrfs volumes are reported by the pool listing (`btrfs filesystem
    // show` here returned nothing usable — the parsed stdout was discarded
    // unconditionally), so the dead read is gone rather than left running
    // `btrfs` as root for no output.

    volumes
}

async fn get_raid_device_info(device_name: &str, raid_level: &str) -> Option<StorageVolume> {
    let device_path = format!("/dev/{}", device_name);

    // Get filesystem info using df
    if let Ok(output) = run_probe(Command::new("df").args(["-B1", &device_path])).await {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();

            if lines.len() >= 2 {
                let parts: Vec<&str> = lines[1].split_whitespace().collect();
                if parts.len() >= 6 {
                    let size: u64 = parts[1].parse().unwrap_or(0);
                    let used: u64 = parts[2].parse().unwrap_or(0);
                    let available: u64 = parts[3].parse().unwrap_or(0);
                    let mount = parts[5].to_string();

                    let usage_percent = if size > 0 {
                        used as f64 / size as f64 * 100.0
                    } else {
                        0.0
                    };

                    let status = if usage_percent > 90.0 {
                        "unhealthy".to_string()
                    } else if usage_percent > 80.0 {
                        "warning".to_string()
                    } else {
                        "healthy".to_string()
                    };

                    return Some(StorageVolume {
                        name: device_name.to_string(),
                        raid_type: raid_level.to_string(),
                        mount_point: mount,
                        total_bytes: size,
                        used_bytes: used,
                        available_bytes: available,
                        total_human: format_bytes(size),
                        used_human: format_bytes(used),
                        available_human: format_bytes(available),
                        usage_percent,
                        status,
                    });
                }
            }
        }
    }

    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetPreference {
    pub widget_key: String,
    pub position_x: i32,
    pub position_y: i32,
    pub is_visible: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWidgetPreferenceRequest {
    pub widget_key: String,
    pub position_x: i32,
    pub position_y: i32,
    pub is_visible: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerDetailsResponse {
    pub cpu: CpuDetails,
    pub memory: MemoryDetails,
    pub gpu: Vec<GpuDetails>,
    pub motherboard: MotherboardInfoResponse,
    pub sensors: SensorsResponse,
    pub network: NetworkDetails,
    pub os: OsDetails,
}

/// Operating-system / host facts shown in the Dashboard "Operating System"
/// and "Health Score" cards. Every field is best-effort — an unknown value
/// is an empty string, which the frontend renders as a hidden row.
#[derive(Debug, Clone, Serialize)]
pub struct OsDetails {
    /// `PRETTY_NAME` from `/etc/os-release`, e.g. "Ubuntu 24.04.1 LTS".
    pub distro: String,
    /// Kernel release, e.g. "6.8.0-45-generic".
    pub kernel: String,
    /// CPU architecture, e.g. "x86_64".
    pub architecture: String,
    pub hostname: String,
    /// PID 1's command name, e.g. "systemd".
    pub init: String,
    /// Desktop environment, usually empty on a headless server.
    pub desktop: String,
    /// Installed-package count with the manager that reported it, e.g.
    /// "2143 (dpkg)". Empty when no known package manager is present.
    pub packages: String,
    /// IANA timezone name, e.g. "Europe/Paris".
    pub timezone: String,
    /// Human-readable uptime, e.g. "5d 3h 21m".
    pub uptime: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkDetails {
    pub interfaces: Vec<NetworkInterfaceDetails>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkInterfaceDetails {
    pub name: String,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub mac_address: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub link_speed_mbps: Option<u32>,
    pub duplex: String,
    pub operstate: String,
    pub carrier: bool,
    pub is_vlan: bool,
    pub vlan_id: Option<u32>,
    pub is_bond: bool,
    pub bond_mode: String,
    pub bond_slaves: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuDetails {
    pub model: String,
    pub gpu_type: String,
    pub temperature_celsius: Option<f64>,
    pub utilization_percent: Option<u32>,
    pub memory_total_bytes: Option<u64>,
    pub memory_used_bytes: Option<u64>,
    pub power_watts: Option<f64>,
    pub freq_current_mhz: Option<u32>,
    pub freq_max_mhz: Option<u32>,
    pub encoder_util_percent: Option<u32>,
    pub decoder_util_percent: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MotherboardInfoResponse {
    pub manufacturer: String,
    pub model: String,
    pub bios_version: String,
    pub bios_date: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensorsResponse {
    pub cpu_temps: Vec<TempItem>,
    pub motherboard_temps: Vec<TempItem>,
    pub drive_temps: Vec<TempItem>,
    pub fan_speeds: Vec<FanItem>,
    pub voltages: Vec<VoltageItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FanItem {
    pub label: String,
    pub rpm: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoltageItem {
    pub label: String,
    pub voltage: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuDetails {
    pub model_name: String,
    pub vendor: String,
    pub architecture: String,
    pub total_cores: u32,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub physical_sockets: u32,
    pub frequency_mhz: f64,
    pub frequency_max_mhz: f64,
    pub cache_size: String,
    pub governor: String,
    /// 1-, 5- and 15-minute load averages from `/proc/loadavg`.
    pub load_avg: Vec<f64>,
    /// Mean of `per_core_usage`, i.e. the aggregate CPU utilisation.
    ///
    /// The "Live usage by app" card needs one host number to draw its base
    /// band; it used to read a field that did not exist and fell back to a
    /// literal `0` (ROADMAP 15.3).
    pub usage_percent: u32,
    pub per_core_usage: Vec<PerCoreUsageItem>,
    pub temperatures: Vec<TempItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerCoreUsageItem {
    pub core: String,
    pub usage_percent: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TempItem {
    pub label: String,
    pub temp_celsius: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryDetails {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub available_bytes: u64,
    pub usage_percent: u32,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_free_bytes: u64,
    /// `Cached` + `Buffers` from `/proc/meminfo`, in bytes.
    pub cached_bytes: u64,
    pub ram_type: String,
    pub ram_speed_mhz: u32,
}

pub async fn handle_server_details(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    // Everything below is synchronous I/O — /proc/sysfs reads plus, for GPU
    // (nvidia-smi/rocm-smi), drive temperatures (lsblk + one smartctl -A per
    // disk), voltages (`sensors`), network IPs (`ip -json addr show`), and
    // the dmidecode fallback in get_motherboard_info(), several blocking
    // subprocess spawns. This endpoint is polled every 5s while the
    // Dashboard is open (see frontend/src/routes/Dashboard.svelte), so
    // running it inline would repeatedly stall a Tokio worker thread for the
    // combined duration of every subprocess below. Deported as one unit via
    // spawn_blocking to keep the exact existing behavior/ordering while
    // taking it off the async runtime.
    let details = match tokio::task::spawn_blocking(build_server_details).await {
        Ok(details) => details,
        Err(e) => {
            return ResponseHelper::json_error(format!("Internal error: {}", e));
        }
    };

    ResponseHelper::json_success(details)
}

/// Gathers every field of [`ServerDetailsResponse`] via blocking I/O
/// (proc/sysfs reads and the subprocess calls documented on its call site in
/// [`handle_server_details`]). Must only ever run inside `spawn_blocking`.
fn build_server_details() -> ServerDetailsResponse {
    // CPU details
    let cpu_info = ProcParser::parse_cpuinfo();
    let per_core = get_per_core_delta();
    let sensors = ProcParser::get_sensor_info();

    let governor = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let freq_max = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|khz| khz / 1000.0)
        .unwrap_or(0.0);

    let temperatures: Vec<TempItem> = sensors
        .cpu_temperatures
        .iter()
        .map(|(label, temp)| TempItem {
            label: label.clone(),
            temp_celsius: *temp,
        })
        .collect();

    let per_core_usage: Vec<PerCoreUsageItem> = per_core;

    // Memory details
    let mem_info = ProcParser::parse_meminfo();
    let ram_info = ProcParser::get_ram_info();

    let total_bytes = mem_info.total_kb * 1024;
    let free_bytes = mem_info.free_kb * 1024;
    let available_bytes = mem_info.available_kb * 1024;
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    let usage_percent = if total_bytes > 0 {
        ((total_bytes - available_bytes) as f64 / total_bytes as f64 * 100.0) as u32
    } else {
        0
    };

    // Swap + page-cache info from /proc/meminfo
    let meminfo_content = ProcParser::read_proc_file("/proc/meminfo");
    let mut swap_total: u64 = 0;
    let mut swap_free: u64 = 0;
    let mut cached_kb: u64 = 0;
    let mut buffers_kb: u64 = 0;
    for line in meminfo_content.lines() {
        if line.starts_with("SwapTotal:") {
            swap_total = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
                * 1024;
        } else if line.starts_with("SwapFree:") {
            swap_free = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
                * 1024;
        } else if line.starts_with("Cached:") {
            cached_kb = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if line.starts_with("Buffers:") {
            buffers_kb = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }
    }
    let swap_used = swap_total.saturating_sub(swap_free);
    let cached_bytes = (cached_kb + buffers_kb) * 1024;

    // 1/5/15-minute load averages
    let load_avg: Vec<f64> = ProcParser::parse_loadavg()
        .split_whitespace()
        .take(3)
        .filter_map(|s| s.parse::<f64>().ok())
        .collect();

    // Operating-system / host facts
    let os = get_os_details(cpu_info.architecture.clone());

    // GPU details
    let gpu_list = get_gpu_details();

    // Motherboard info
    let mb = ProcParser::get_motherboard_info();
    let motherboard = MotherboardInfoResponse {
        manufacturer: mb.manufacturer,
        model: mb.model,
        bios_version: mb.bios_version,
        bios_date: mb.bios_date,
    };

    // Network
    let network = get_network_details();

    // Sensors
    let drive_temps = get_drive_temperatures();
    let voltages = parse_voltages(&ProcParser::read_proc_file("/proc/meminfo"), &sensors);

    let fan_items: Vec<FanItem> = sensors
        .fan_speeds
        .iter()
        .map(|(label, rpm)| FanItem {
            label: label.clone(),
            rpm: *rpm,
        })
        .collect();

    let mb_temps: Vec<TempItem> = sensors
        .motherboard_temperatures
        .iter()
        .map(|(label, temp)| TempItem {
            label: label.clone(),
            temp_celsius: *temp,
        })
        .collect();

    ServerDetailsResponse {
        cpu: CpuDetails {
            model_name: cpu_info.model_name,
            vendor: cpu_info.vendor,
            architecture: cpu_info.architecture,
            total_cores: cpu_info.total_cores,
            physical_cores: cpu_info.physical_cores,
            logical_cores: cpu_info.logical_cores,
            physical_sockets: cpu_info.physical_sockets,
            frequency_mhz: cpu_info.frequency_mhz,
            frequency_max_mhz: freq_max,
            cache_size: cpu_info.cache_size,
            governor,
            load_avg,
            usage_percent: mean_core_usage(&per_core_usage),
            per_core_usage,
            temperatures: temperatures.clone(),
        },
        memory: MemoryDetails {
            total_bytes,
            used_bytes,
            free_bytes,
            available_bytes,
            usage_percent,
            swap_total_bytes: swap_total,
            swap_used_bytes: swap_used,
            swap_free_bytes: swap_free,
            cached_bytes,
            ram_type: ram_info.ram_type,
            ram_speed_mhz: ram_info.speed_mhz,
        },
        gpu: gpu_list,
        motherboard,
        network,
        sensors: SensorsResponse {
            cpu_temps: temperatures,
            motherboard_temps: mb_temps,
            drive_temps,
            fan_speeds: fan_items,
            voltages,
        },
        os,
    }
}

/// Gather best-effort operating-system / host facts for the Dashboard.
/// Runs only inside `spawn_blocking` (called from [`build_server_details`]):
/// reads a handful of `/proc` and `/etc` files and shells out to the local
/// package manager for the installed-package count.
fn get_os_details(architecture: String) -> OsDetails {
    // Distribution name — PRETTY_NAME from /etc/os-release.
    let distro = std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Linux".to_string());

    let read_trimmed = |path: &str| {
        std::fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let kernel = read_trimmed("/proc/sys/kernel/osrelease");
    let hostname = read_trimmed("/proc/sys/kernel/hostname");
    // PID 1's command name — "systemd", "init", "openrc-init", …
    let init = read_trimmed("/proc/1/comm");

    // Desktop environment: normally unset on a headless server.
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_default();

    let packages = count_installed_packages();
    let timezone = read_timezone();
    let uptime = format_uptime(ProcParser::parse_uptime());

    OsDetails {
        distro,
        kernel,
        architecture,
        hostname,
        init,
        desktop,
        packages,
        timezone,
        uptime,
    }
}

/// Count installed packages, trying the common Linux package managers in
/// turn. Returns e.g. "2143 (dpkg)", or an empty string when none match.
fn count_installed_packages() -> String {
    let count_lines = |bytes: &[u8]| {
        bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count()
    };

    let candidates: [(&str, &[&str]); 3] = [
        ("dpkg", &["dpkg-query", "-f", "${binary:Package}\n", "-W"]),
        ("rpm", &["rpm", "-qa"]),
        ("pacman", &["pacman", "-Qq"]),
    ];

    for (label, argv) in candidates {
        if let Ok(output) = run_probe_blocking(std::process::Command::new(argv[0]).args(&argv[1..]))
        {
            if output.status.success() {
                let n = count_lines(&output.stdout);
                if n > 0 {
                    return format!("{n} ({label})");
                }
            }
        }
    }
    String::new()
}

/// Resolve the system timezone: `/etc/timezone` (Debian family) first, then
/// the symlink target of `/etc/localtime`. Empty string when neither works.
///
/// Single implementation, shared with the settings/scheduling code (ROADMAP
/// 13.4) — it lives in `handlers::settings` because the persisted setting is
/// the value that decides what the rest of the app uses.
fn read_timezone() -> String {
    crate::handlers::settings::system_timezone()
}

/// Format an uptime in seconds as a compact "5d 3h 21m" string.
fn format_uptime(secs: u64) -> String {
    if secs == 0 {
        return String::new();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 || parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}

/// Parse per-core CPU usage with delta tracking for real-time values.
fn get_per_core_delta() -> Vec<PerCoreUsageItem> {
    let content = ProcParser::read_proc_file("/proc/stat");
    let mut current_stats: HashMap<String, (u64, u64)> = HashMap::new();
    let mut results = Vec::new();

    for line in content.lines() {
        if line.starts_with("cpu") && !line.starts_with("cpu ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 8 {
                let core_name = parts[0].to_string();
                let user: u64 = parts[1].parse().unwrap_or(0);
                let nice: u64 = parts[2].parse().unwrap_or(0);
                let system: u64 = parts[3].parse().unwrap_or(0);
                let idle: u64 = parts[4].parse().unwrap_or(0);
                let iowait: u64 = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
                let irq: u64 = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);
                let softirq: u64 = parts.get(7).and_then(|s| s.parse().ok()).unwrap_or(0);

                let total = user + nice + system + idle + iowait + irq + softirq;
                let idle_total = idle + iowait;
                current_stats.insert(core_name.clone(), (idle_total, total));
            }
        }
    }

    // Compute delta against previous values
    {
        let mut prev = PREV_PER_CORE.write();
        for (core, (cur_idle, cur_total)) in &current_stats {
            let (prev_idle, prev_total) =
                prev.get(core).copied().unwrap_or((*cur_idle, *cur_total));
            let delta_total = cur_total.saturating_sub(prev_total);
            let delta_idle = cur_idle.saturating_sub(prev_idle);
            let usage = if delta_total > 0 {
                ((delta_total - delta_idle) as f64 / delta_total as f64 * 100.0) as u32
            } else {
                0
            };
            results.push(PerCoreUsageItem {
                core: core.clone(),
                usage_percent: usage.min(100),
            });
        }
        // Store current values for next call
        for (core, vals) in &current_stats {
            prev.insert(core.clone(), *vals);
        }
    }

    results
}

/// Detect and query GPU details via nvidia-smi, rocm-smi, or lspci fallback.
fn get_gpu_details() -> Vec<GpuDetails> {
    let mut gpus = Vec::new();

    // Try nvidia-smi first (NVIDIA GPUs)
    if let Ok(output) = run_probe_blocking(std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=index,name,temperature.gpu,utilization.gpu,memory.total,memory.used,power.draw,clocks.current.graphics,clocks.max.graphics,encoder.utilization,decoder.utilization",
            "--format=csv,noheader,nounits",
        ])
        )
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 11 {
                    let model = parts.get(1).unwrap_or(&"").to_string();
                    let temp = parts.get(2).and_then(|s| s.parse::<f64>().ok());
                    let util = parts.get(3).and_then(|s| s.parse::<u32>().ok());
                    let mem_total = parts.get(4).and_then(|s| s.parse::<u64>().ok()).map(|m| m * 1024 * 1024);
                    let mem_used = parts.get(5).and_then(|s| s.parse::<u64>().ok()).map(|m| m * 1024 * 1024);
                    let power = parts.get(6).and_then(|s| s.parse::<f64>().ok());
                    let freq_cur = parts.get(7).and_then(|s| s.parse::<u32>().ok());
                    let freq_max = parts.get(8).and_then(|s| s.parse::<u32>().ok());
                    let enc = parts.get(9).and_then(|s| s.parse::<u32>().ok());
                    let dec = parts.get(10).and_then(|s| s.parse::<u32>().ok());

                    gpus.push(GpuDetails {
                        model: model.clone(),
                        gpu_type: "Dedicated (NVIDIA)".to_string(),
                        temperature_celsius: temp,
                        utilization_percent: util,
                        memory_total_bytes: mem_total,
                        memory_used_bytes: mem_used,
                        power_watts: power,
                        freq_current_mhz: freq_cur,
                        freq_max_mhz: freq_max,
                        encoder_util_percent: enc,
                        decoder_util_percent: dec,
                    });
                }
            }
            if !gpus.is_empty() {
                return gpus;
            }
        }
    }

    // Try rocm-smi (AMD GPUs)
    let mut rocm_found = false;
    if let Ok(output) = run_probe_blocking(std::process::Command::new("rocm-smi").args([
        "--showtemp",
        "--showuse",
        "--showmeminfo",
        "--showpower",
        "--showclocks",
    ])) {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.contains("No devices") && !stdout.is_empty() {
                rocm_found = true;
                let model = ProcParser::get_gpu_info()
                    .into_iter()
                    .find(|g| g.gpu_type.contains("AMD"))
                    .map(|g| g.model)
                    .unwrap_or_else(|| "AMD GPU".to_string());
                for line in stdout.lines() {
                    if line.contains("GPU[") && line.contains("Temperature") {
                        if let Some(val) = line
                            .split(':')
                            .next_back()
                            .and_then(|s| s.trim().parse::<f64>().ok())
                        {
                            gpus.push(GpuDetails {
                                model: model.clone(),
                                gpu_type: "Dedicated (AMD)".to_string(),
                                temperature_celsius: Some(val),
                                utilization_percent: None,
                                memory_total_bytes: None,
                                memory_used_bytes: None,
                                power_watts: None,
                                freq_current_mhz: None,
                                freq_max_mhz: None,
                                encoder_util_percent: None,
                                decoder_util_percent: None,
                            });
                        }
                    }
                }
                for line in stdout.lines() {
                    if line.contains("GPU[") && line.contains("Utilization") {
                        if let Some(val) = line
                            .split(':')
                            .next_back()
                            .and_then(|s| s.trim().trim_end_matches('%').parse::<f64>().ok())
                        {
                            if let Some(gpu) = gpus.last_mut() {
                                gpu.utilization_percent = Some(val as u32);
                            }
                        }
                    }
                    if line.contains("VRAM Total Memory") {
                        if let Some(val) = line
                            .split(':')
                            .next_back()
                            .and_then(|s| s.trim().parse::<u64>().ok())
                        {
                            if let Some(gpu) = gpus.last_mut() {
                                gpu.memory_total_bytes = Some(val);
                            }
                        }
                    }
                    if line.contains("VRAM Used Memory") {
                        if let Some(val) = line
                            .split(':')
                            .next_back()
                            .and_then(|s| s.trim().parse::<u64>().ok())
                        {
                            if let Some(gpu) = gpus.last_mut() {
                                gpu.memory_used_bytes = Some(val);
                            }
                        }
                    }
                    if line.contains("Power Draw") {
                        if let Some(val) = line
                            .split(':')
                            .next_back()
                            .and_then(|s| s.trim().parse::<f64>().ok())
                        {
                            if let Some(gpu) = gpus.last_mut() {
                                gpu.power_watts = Some(val);
                            }
                        }
                    }
                    if line.contains("GPU Clock") {
                        if let Some(val) = line
                            .split(':')
                            .next_back()
                            .and_then(|s| s.trim().parse::<f64>().ok())
                        {
                            if let Some(gpu) = gpus.last_mut() {
                                gpu.freq_current_mhz = Some(val as u32);
                            }
                        }
                    }
                }
            }
        }
    }
    if rocm_found && !gpus.is_empty() {
        return gpus;
    }

    // Fallback: basic info from lspci
    let basic_gpus = ProcParser::get_gpu_info();
    for gpu in basic_gpus {
        gpus.push(GpuDetails {
            model: gpu.model,
            gpu_type: gpu.gpu_type,
            temperature_celsius: None,
            utilization_percent: None,
            memory_total_bytes: None,
            memory_used_bytes: None,
            power_watts: None,
            freq_current_mhz: None,
            freq_max_mhz: None,
            encoder_util_percent: None,
            decoder_util_percent: None,
        });
    }

    gpus
}

/// Gather network interface details from sysfs, /proc/net/dev, and ip commands.
fn get_network_details() -> NetworkDetails {
    let mut interfaces = Vec::new();

    let net_dir = std::path::Path::new("/sys/class/net");
    if !net_dir.is_dir() {
        return NetworkDetails { interfaces };
    }

    // Read traffic from /proc/net/dev
    let proc_net = std::fs::read_to_string("/proc/net/dev").unwrap_or_default();
    let mut traffic: HashMap<String, (u64, u64)> = HashMap::new();
    for line in proc_net.lines().skip(2) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 10 {
            let iface = parts[0].trim_end_matches(':');
            let rx: u64 = parts[1].parse().unwrap_or(0);
            let tx: u64 = parts[9].parse().unwrap_or(0);
            traffic.insert(iface.to_string(), (rx, tx));
        }
    }

    // Parse IP addresses from `ip -json addr show`
    let ip_output =
        run_probe_blocking(std::process::Command::new("ip").args(["-json", "addr", "show"]));
    let mut ipv4_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut ipv6_map: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(ref output) = ip_output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Parse JSON output from ip -json addr show
            // Structure: [{"ifname":"eth0","addr_info":[{"local":"192.168.1.1","family":"inet"},...]},...]
            if let Ok(json) = serde_json::from_str::<Vec<serde_json::Value>>(&stdout) {
                for entry in &json {
                    let ifname = entry["ifname"].as_str().unwrap_or("");
                    if let Some(addr_info) = entry["addr_info"].as_array() {
                        for addr in addr_info {
                            let local = addr["local"].as_str().unwrap_or("");
                            let family = addr["family"].as_str().unwrap_or("");
                            if local.is_empty() {
                                continue;
                            }
                            if family == "inet" {
                                let prefix = addr["prefixlen"].as_u64().unwrap_or(0);
                                ipv4_map
                                    .entry(ifname.to_string())
                                    .or_default()
                                    .push(format!("{}/{}", local, prefix));
                            } else if family == "inet6" {
                                let prefix = addr["prefixlen"].as_u64().unwrap_or(0);
                                ipv6_map
                                    .entry(ifname.to_string())
                                    .or_default()
                                    .push(format!("{}/{}", local, prefix));
                            }
                        }
                    }
                }
            }
        }
    }

    // Parse bonding info
    let mut bond_modes: HashMap<String, String> = HashMap::new();
    let mut bond_slaves: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(bond_dir) = std::fs::read_dir("/proc/net/bonding") {
        for entry in bond_dir.flatten() {
            let bond_name = entry.file_name().to_string_lossy().to_string();
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                for line in content.lines() {
                    if line.contains("Bonding Mode:") {
                        let mode = line.split(':').nth(1).unwrap_or("").trim().to_string();
                        bond_modes.insert(bond_name.clone(), mode);
                    }
                    if line.contains("Slave Interface:") {
                        let slave = line.split(':').nth(1).unwrap_or("").trim().to_string();
                        bond_slaves
                            .entry(bond_name.clone())
                            .or_default()
                            .push(slave);
                    }
                }
            }
        }
    }

    // Iterate over interfaces in /sys/class/net
    if let Ok(dir) = std::fs::read_dir(net_dir) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" {
                continue;
            }
            let path = entry.path();

            // MAC address
            let mac_address = std::fs::read_to_string(path.join("address"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            // Link speed (may not be available for all interfaces, e.g. virtual)
            let link_speed = std::fs::read_to_string(path.join("speed"))
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|&s| s != u32::MAX && s > 0);

            // Duplex
            let duplex = std::fs::read_to_string(path.join("duplex"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Operstate (up/down/unknown)
            let operstate = std::fs::read_to_string(path.join("operstate"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown".to_string());

            // Carrier (0 = down, 1 = up)
            let carrier = std::fs::read_to_string(path.join("carrier"))
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
                .map(|c| c == 1)
                .unwrap_or(false);

            // Detect VLAN (iface name contains dot: eth0.100 or enp0s3.100)
            let is_vlan = name.contains('.');
            let vlan_id = if is_vlan {
                name.split('.')
                    .next_back()
                    .and_then(|s| s.parse::<u32>().ok())
            } else {
                None
            };

            // Bonding info
            let is_bond = bond_modes.contains_key(&name);
            let bond_mode = bond_modes.get(&name).cloned().unwrap_or_default();
            let bond_slaves_list = bond_slaves.remove(&name).unwrap_or_default();

            // IP addresses
            let ipv4 = ipv4_map.remove(&name).unwrap_or_default();
            let ipv6 = ipv6_map.remove(&name).unwrap_or_default();

            // Traffic
            let (rx_bytes, tx_bytes) = traffic.remove(&name).unwrap_or((0, 0));

            interfaces.push(NetworkInterfaceDetails {
                name,
                ipv4,
                ipv6,
                mac_address,
                rx_bytes,
                tx_bytes,
                link_speed_mbps: link_speed,
                duplex,
                operstate,
                carrier,
                is_vlan,
                vlan_id,
                is_bond,
                bond_mode,
                bond_slaves: bond_slaves_list,
            });
        }
    }

    // Sort: active interfaces first, then by name
    interfaces.sort_by(|a, b| b.carrier.cmp(&a.carrier).then(a.name.cmp(&b.name)));

    NetworkDetails { interfaces }
}

/// Get temperatures from storage drives using smartctl.
fn get_drive_temperatures() -> Vec<TempItem> {
    let mut temps = Vec::new();

    if let Ok(output) =
        run_probe_blocking(std::process::Command::new("lsblk").args(["-d", "-n", "-o", "NAME"]))
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let dev = line.trim();
            if dev.is_empty() {
                continue;
            }
            // Needs root the same way get_smart_status/get_smart_temperature
            // above do — the shared helper carries the sudoers-granted argv
            // shape; see src/smart.rs.
            if let Ok(smart) = run_smart_blocking(&mut crate::smart::std_command(
                &format!("/dev/{}", dev),
                &["-A"],
            )) {
                let smart_out = String::from_utf8_lossy(&smart.stdout);
                for smart_line in smart_out.lines() {
                    if smart_line.contains("Temperature_Celsius")
                        || smart_line.contains("Airflow_Temperature")
                    {
                        let parts: Vec<&str> = smart_line
                            .split_whitespace()
                            .filter(|s| !s.is_empty())
                            .collect();
                        if let Some(temp_str) = parts.last() {
                            if let Ok(temp) = temp_str.parse::<f64>() {
                                temps.push(TempItem {
                                    label: format!("/dev/{}", dev),
                                    temp_celsius: temp,
                                });
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    temps
}

/// Parse voltage readings from `sensors` output.
fn parse_voltages(_meminfo: &str, _sensors: &crate::utils::proc::SensorInfo) -> Vec<VoltageItem> {
    let mut voltages = Vec::new();

    // Try to get voltages from `sensors` command output directly
    if let Ok(output) = run_probe_blocking(&mut std::process::Command::new("sensors")) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut _current_chip = String::new();

        for line in stdout.lines() {
            // Detect chip name (line before adapter)
            if line.contains("Adapter:") || (!line.starts_with(' ') && line.contains(':')) {
                if !line.starts_with(' ')
                    && line.contains(':')
                    && !line.contains('°')
                    && !line.contains("RPM")
                {
                    let parts: Vec<&str> = line.split(':').collect();
                    let label = parts[0].trim().to_string();
                    let val_str = parts[1].trim();
                    // Voltage lines: contain "V" or "mV" or " V"
                    if val_str.contains(" V") || val_str.ends_with('V') || val_str.contains(" mV") {
                        let clean = val_str
                            .replace(" V", "")
                            .replace(" mV", "")
                            .replace('+', "")
                            .trim()
                            .to_string();
                        if let Ok(v) = clean.parse::<f64>() {
                            let voltage = if val_str.contains(" mV") {
                                v / 1000.0
                            } else {
                                v
                            };
                            voltages.push(VoltageItem { label, voltage });
                        }
                    }
                }
                continue;
            }

            if line.starts_with(' ') && line.contains('V') && !line.contains('°') {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 {
                    let label = parts[0].trim().to_string();
                    let val_str = parts[1].trim();
                    if val_str.contains(" V") || val_str.contains(" mV") {
                        let clean = val_str
                            .replace(" V", "")
                            .replace(" mV", "")
                            .replace('+', "")
                            .trim()
                            .to_string();
                        if let Ok(v) = clean.parse::<f64>() {
                            let voltage = if val_str.contains(" mV") {
                                v / 1000.0
                            } else {
                                v
                            };
                            voltages.push(VoltageItem { label, voltage });
                        }
                    }
                }
            }
        }
    }

    voltages
}

/// The caller's floating-panel preferences.
///
/// Goes through `require_auth` like every other protected handler (ROADMAP
/// 15.1): the previous hand-rolled `SELECT username FROM sessions WHERE token
/// = ?` had no `expires_at` predicate, so an **expired** session still read and
/// wrote preferences, and an invalid one answered 400 instead of 401. The
/// guard resolves `/api/internal`'s session once — expiry included — and the
/// `user_widget_preferences` row is keyed by that resolved username.
pub async fn handle_get_widget_preferences(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    match load_widget_preferences(&state, &username).await {
        Ok(prefs) => ResponseHelper::json_success(prefs),
        Err(message) => ResponseHelper::json_error(message),
    }
}

/// Read one account's panel preferences (shared by the GET handler and tests).
async fn load_widget_preferences(
    state: &Arc<AppState>,
    username: &str,
) -> Result<Vec<WidgetPreference>, String> {
    let preferences = sqlx::query_as::<_, (String, i32, i32, bool)>(
        "SELECT widget_key, position_x, position_y, is_visible
         FROM user_widget_preferences WHERE user_id = ? ORDER BY widget_key",
    )
    .bind(username)
    .fetch_all(state.db.pool().as_ref())
    .await
    .map_err(|e| format!("Failed to fetch preferences: {e}"))?;

    Ok(preferences
        .into_iter()
        .map(
            |(widget_key, position_x, position_y, is_visible)| WidgetPreference {
                widget_key,
                position_x,
                position_y,
                is_visible,
            },
        )
        .collect())
}

/// Create or update one of the caller's panel preferences.
pub async fn handle_update_widget_preference(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    AxumJson(payload): AxumJson<UpdateWidgetPreferenceRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(username) => username,
        Err(response) => return response,
    };

    if let Err(message) = validate_widget_preference(&payload) {
        return ResponseHelper::json_error(message);
    }

    let is_visible = payload.is_visible.unwrap_or(true);

    // Upsert widget preference
    let result = sqlx::query(
        "INSERT INTO user_widget_preferences (user_id, widget_key, position_x, position_y, is_visible, updated_at)
         VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(user_id, widget_key) DO UPDATE SET
         position_x = excluded.position_x,
         position_y = excluded.position_y,
         is_visible = excluded.is_visible,
         updated_at = CURRENT_TIMESTAMP",
    )
    .bind(&username)
    .bind(&payload.widget_key)
    .bind(payload.position_x)
    .bind(payload.position_y)
    .bind(is_visible)
    .execute(state.db.pool().as_ref())
    .await;

    match result {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({
            "message": "Widget preference updated successfully"
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to update preference: {}", e)),
    }
}

#[cfg(test)]
mod os_details_tests {
    use super::format_uptime;

    #[test]
    fn test_format_uptime_zero_is_empty() {
        assert_eq!(format_uptime(0), "");
    }

    #[test]
    fn test_format_uptime_sub_minute_shows_zero_minutes() {
        assert_eq!(format_uptime(42), "0m");
    }

    #[test]
    fn test_format_uptime_minutes_only() {
        assert_eq!(format_uptime(25 * 60), "25m");
    }

    #[test]
    fn test_format_uptime_hours_and_minutes() {
        assert_eq!(format_uptime(3 * 3600 + 7 * 60), "3h 7m");
    }

    #[test]
    fn test_format_uptime_days_hours_minutes() {
        assert_eq!(format_uptime(5 * 86_400 + 3 * 3600 + 21 * 60), "5d 3h 21m");
    }

    #[test]
    fn test_format_uptime_skips_zero_middle_units() {
        // 2 days exactly — no hours, no minutes.
        assert_eq!(format_uptime(2 * 86_400), "2d");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    const TIB: u64 = GIB * 1024;

    // --- format_bytes: byte count → short human string ---

    #[test]
    fn test_format_bytes_uses_the_right_unit_at_each_threshold() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(KIB), "1.0K");
        assert_eq!(format_bytes(MIB), "1.0M");
        assert_eq!(format_bytes(GIB), "1.0G");
        assert_eq!(format_bytes(TIB), "1.0T");
    }

    #[test]
    fn test_format_bytes_keeps_one_decimal() {
        assert_eq!(format_bytes(GIB + GIB / 2), "1.5G");
        assert_eq!(format_bytes(2 * MIB + MIB / 2), "2.5M");
    }

    // --- parse_size: `df -h` size string → bytes ---

    #[test]
    fn test_parse_size_integer_values() {
        assert_eq!(parse_size("10G"), 10 * GIB);
        assert_eq!(parse_size("512M"), 512 * MIB);
        assert_eq!(parse_size("1K"), KIB);
        assert_eq!(parse_size("2T"), 2 * TIB);
        assert_eq!(parse_size("1P"), 1024 * TIB);
        assert_eq!(parse_size("512"), 512); // bare number, no suffix
    }

    #[test]
    fn test_parse_size_trims_whitespace_and_handles_junk() {
        assert_eq!(parse_size("  4G  "), 4 * GIB);
        assert_eq!(parse_size(""), 0);
        assert_eq!(parse_size("-"), 0);
    }

    #[test]
    fn test_parse_size_keeps_the_fraction() {
        // Regression: casting the f64 to u64 before multiplying used to
        // report "9.8G" as if it were "9G".
        assert_eq!(parse_size("9.8G"), (9.8_f64 * GIB as f64) as u64);
        assert_eq!(parse_size("1.5T"), (1.5_f64 * TIB as f64) as u64);
    }

    #[test]
    fn test_parse_size_accepts_a_locale_decimal_comma() {
        // `df -h` follows the process locale: "465,8G" under fr_FR.
        assert_eq!(parse_size("465,8G"), (465.8_f64 * GIB as f64) as u64);
    }

    // --- calculate_individual_disk_score ---

    #[test]
    fn test_individual_disk_score_healthy_cool_and_empty_is_perfect() {
        assert_eq!(
            calculate_individual_disk_score("Healthy", Some(35.0), 10.0),
            100
        );
    }

    #[test]
    fn test_individual_disk_score_failed_smart_drags_it_down() {
        // 0 (SMART) + 20 (30 °C) + 40 (20 % used) = 60; the old hardcoded
        // `error_score = 10` added a tenth of the score nobody measured.
        assert_eq!(
            calculate_individual_disk_score("Failed", Some(30.0), 20.0),
            60
        );
    }

    #[test]
    fn test_individual_disk_score_unknown_smart_and_temp_give_partial() {
        // 20 (unknown SMART) + 10 (unknown temp) + 40 (5 % used) = 70.
        assert_eq!(calculate_individual_disk_score("???", None, 5.0), 70);
    }

    #[test]
    fn test_individual_disk_score_hot_and_full_disk_is_penalised() {
        // 40 (SMART) + 0 (>=85 °C) + 0 (>=95 % used) = 40.
        assert_eq!(
            calculate_individual_disk_score("Healthy", Some(90.0), 99.0),
            40
        );
    }

    #[test]
    fn test_individual_disk_score_never_exceeds_100() {
        assert_eq!(
            calculate_individual_disk_score("Healthy", Some(0.0), 0.0),
            100
        );
    }

    #[test]
    fn test_individual_disk_score_has_no_unmeasured_component() {
        // Every point must come from one of the three measured inputs: a disk
        // whose inputs sum to zero scores zero, not a constant 10.
        assert_eq!(
            calculate_individual_disk_score("Failed", Some(95.0), 99.0),
            0
        );
        // And the three maximums add up to exactly 100.
        assert_eq!(
            calculate_individual_disk_score("Healthy", Some(20.0), 0.0),
            100
        );
    }

    // --- health_status_for_score: the one status vocabulary ---

    #[test]
    fn test_health_status_uses_the_documented_vocabulary() {
        assert_eq!(health_status_for_score(100), "healthy");
        assert_eq!(health_status_for_score(70), "healthy");
        assert_eq!(health_status_for_score(69), "warning");
        assert_eq!(health_status_for_score(40), "warning");
        assert_eq!(health_status_for_score(39), "unhealthy");
        assert_eq!(health_status_for_score(0), "unhealthy");
    }

    #[test]
    fn test_health_status_never_answers_critical() {
        // The UI declares healthy|warning|unhealthy|unknown; the fifth word the
        // backend used to emit was rendered by nothing.
        for score in 0..=100 {
            assert_ne!(health_status_for_score(score), "critical");
        }
    }

    // --- mean_core_usage: aggregate for the app-usage card ---

    #[test]
    fn test_mean_core_usage_averages_and_rounds() {
        let cores = vec![
            PerCoreUsageItem {
                core: "C0".into(),
                usage_percent: 10,
            },
            PerCoreUsageItem {
                core: "C1".into(),
                usage_percent: 21,
            },
            PerCoreUsageItem {
                core: "C2".into(),
                usage_percent: 30,
            },
        ];
        assert_eq!(mean_core_usage(&cores), 20); // 61/3 = 20.33
        assert_eq!(mean_core_usage(&[]), 0, "no core answered");
    }

    // --- snapshot_is_fresh: the disk-health cache window ---

    #[test]
    fn test_snapshot_is_fresh_only_inside_the_ttl() {
        let now = Instant::now();
        assert!(snapshot_is_fresh(now, now, DISK_HEALTH_TTL));
        assert!(snapshot_is_fresh(
            now - Duration::from_secs(29),
            now,
            DISK_HEALTH_TTL
        ));
        assert!(!snapshot_is_fresh(
            now - Duration::from_secs(31),
            now,
            DISK_HEALTH_TTL
        ));
        // A clock that appears to go backwards must not panic.
        assert!(snapshot_is_fresh(
            now + Duration::from_secs(5),
            now,
            DISK_HEALTH_TTL
        ));
    }

    // --- calculate_disk_summary: bucket disks by health_score ---

    fn disk(score: u32) -> DiskHealth {
        DiskHealth {
            name: "d".into(),
            device: "/dev/d".into(),
            model: "m".into(),
            size_bytes: 0,
            size_human: String::new(),
            smart_status: "Healthy".into(),
            temperature: None,
            role: String::new(),
            usage_percent: 0.0,
            health_score: score,
            connection_type: String::new(),
            last_self_test: None,
        }
    }

    #[test]
    fn test_disk_summary_buckets_by_score_bands() {
        let disks = [disk(100), disk(70), disk(69), disk(40), disk(39), disk(0)];
        let s = calculate_disk_summary(&disks);
        assert_eq!(s.total_count, 6);
        assert_eq!(s.healthy_count, 2); // 70..=100
        assert_eq!(s.warning_count, 2); // 40..=69
        assert_eq!(s.critical_count, 2); // <40
    }

    #[test]
    fn test_disk_summary_of_no_disks_is_all_zero() {
        let s = calculate_disk_summary(&[]);
        assert_eq!(
            (
                s.total_count,
                s.healthy_count,
                s.warning_count,
                s.critical_count
            ),
            (0, 0, 0, 0)
        );
    }

    // --- calculate_disk_score: 60% avg disk health + 40% filesystem headroom ---

    fn fs(usage: f64) -> FsHealth {
        FsHealth {
            mount: "/".into(),
            filesystem: "ext4".into(),
            size_bytes: 0,
            used_bytes: 0,
            available_bytes: 0,
            size_human: String::new(),
            used_human: String::new(),
            available_human: String::new(),
            usage_percent: usage,
            status: String::new(),
        }
    }

    #[test]
    fn test_disk_score_with_no_disks_is_100() {
        assert_eq!(calculate_disk_score(&[], &[]), 100.0);
    }

    #[test]
    fn test_disk_score_all_healthy_with_roomy_filesystems() {
        // disk_avg 100 * 0.6 + fs 100 * 0.4 = 100
        assert_eq!(
            calculate_disk_score(&[disk(100), disk(100)], &[fs(20.0)]),
            100.0
        );
    }

    #[test]
    fn test_disk_score_a_nearly_full_filesystem_halves_the_fs_component() {
        // disk_avg 100 * 0.6 + fs 50 * 0.4 = 80
        assert_eq!(calculate_disk_score(&[disk(100)], &[fs(90.0)]), 80.0);
    }

    #[test]
    fn test_disk_score_a_critically_full_filesystem_zeroes_the_fs_component() {
        // disk_avg 100 * 0.6 + fs 0 * 0.4 = 60
        assert_eq!(calculate_disk_score(&[disk(100)], &[fs(97.0)]), 60.0);
    }

    // --- widget validation (ROADMAP 15.1) ---

    fn add_request() -> AddWidgetRequest {
        AddWidgetRequest {
            id: "cpu".to_string(),
            wtype: "cpu".to_string(),
            title: "CPU".to_string(),
            size: "1x1".to_string(),
            row: 1,
            col: 1,
            row_span: 1,
            col_span: 1,
        }
    }

    #[test]
    fn test_validate_add_widget_accepts_a_minimal_tile() {
        assert!(validate_add_widget(&add_request()).is_ok());
    }

    #[test]
    fn test_validate_add_widget_accepts_the_whole_id_charset() {
        let mut req = add_request();
        req.id = "disk.usage_2:raid-0".to_string();
        assert!(validate_add_widget(&req).is_ok());
    }

    #[test]
    fn test_validate_add_widget_rejects_an_unsafe_id() {
        for bad in ["", "with space", "sla/sh", "quote'", "<script>", "a\nb"] {
            let mut req = add_request();
            req.id = bad.to_string();
            assert!(validate_add_widget(&req).is_err(), "id {bad:?}");
        }
    }

    #[test]
    fn test_validate_add_widget_rejects_an_over_long_id() {
        let mut req = add_request();
        req.id = "a".repeat(MAX_WIDGET_ID_LEN + 1);
        assert!(validate_add_widget(&req).is_err());
        req.id = "a".repeat(MAX_WIDGET_ID_LEN);
        assert!(validate_add_widget(&req).is_ok());
    }

    #[test]
    fn test_validate_add_widget_rejects_a_bad_type() {
        let mut req = add_request();
        req.wtype = "cpu usage".to_string();
        assert!(validate_add_widget(&req).is_err());
    }

    #[test]
    fn test_validate_add_widget_rejects_an_empty_or_over_long_title() {
        let mut req = add_request();
        req.title = "   ".to_string();
        assert!(validate_add_widget(&req).is_err());
        req.title = "t".repeat(MAX_WIDGET_TITLE_LEN + 1);
        assert!(validate_add_widget(&req).is_err());
        req.title = "t".repeat(MAX_WIDGET_TITLE_LEN);
        assert!(validate_add_widget(&req).is_ok());
    }

    #[test]
    fn test_validate_add_widget_rejects_control_characters_in_the_title() {
        let mut req = add_request();
        req.title = "CPU\u{0000}".to_string();
        assert!(validate_add_widget(&req).is_err());
    }

    #[test]
    fn test_validate_add_widget_rejects_a_malformed_or_oversized_tile() {
        for bad in [
            "", "2", "2x", "x2", "2x2x2", "0x1", "7x1", "1x7", "axb", "-1x2",
        ] {
            let mut req = add_request();
            req.size = bad.to_string();
            assert!(validate_add_widget(&req).is_err(), "size {bad:?}");
        }
        let mut req = add_request();
        req.size = format!("{MAX_WIDGET_SPAN}x{MAX_WIDGET_SPAN}");
        assert!(validate_add_widget(&req).is_ok());
    }

    #[test]
    fn test_validate_add_widget_rejects_a_position_or_span_outside_the_grid() {
        let mut req = add_request();
        req.row = 0;
        assert!(validate_add_widget(&req).is_err(), "row 0");
        req = add_request();
        req.col = MAX_WIDGET_POSITION + 1;
        assert!(validate_add_widget(&req).is_err(), "col too far");
        req = add_request();
        req.row_span = MAX_WIDGET_SPAN + 1;
        assert!(validate_add_widget(&req).is_err(), "row span too large");
        req = add_request();
        req.col_span = 0;
        assert!(validate_add_widget(&req).is_err(), "col span zero");
    }

    // --- widget preference validation (ROADMAP 15.1) ---

    fn preference_request() -> UpdateWidgetPreferenceRequest {
        UpdateWidgetPreferenceRequest {
            widget_key: "resourceMonitor".to_string(),
            position_x: 10,
            position_y: 20,
            is_visible: Some(true),
        }
    }

    #[test]
    fn test_validate_widget_preference_accepts_a_normal_panel() {
        assert!(validate_widget_preference(&preference_request()).is_ok());
    }

    #[test]
    fn test_validate_widget_preference_rejects_an_unsafe_key() {
        for bad in ["", "a b", "x/y", "k'".to_string().as_str()] {
            let mut req = preference_request();
            req.widget_key = bad.to_string();
            assert!(validate_widget_preference(&req).is_err(), "key {bad:?}");
        }
    }

    #[test]
    fn test_validate_widget_preference_bounds_the_panel_position() {
        let mut req = preference_request();
        req.position_x = MAX_WIDGET_PANEL_OFFSET + 1;
        assert!(validate_widget_preference(&req).is_err());
        req.position_x = -MAX_WIDGET_PANEL_OFFSET - 1;
        assert!(validate_widget_preference(&req).is_err());
        req.position_x = -MAX_WIDGET_PANEL_OFFSET;
        assert!(validate_widget_preference(&req).is_ok());
        req.position_y = MAX_WIDGET_PANEL_OFFSET;
        assert!(validate_widget_preference(&req).is_ok());
    }
}
