use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::{ProcParser, ResponseHelper};
use axum::{
    extract::State,
    http::HeaderMap,
    response::Response,
};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

static PREV_STATS: Lazy<RwLock<HashMap<String, (u64, u64)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// CPU usage percentage, load averages, and core count.
#[derive(Serialize, Clone)]
pub struct CpuUsageResponse {
    pub usage_percent: u32,
    pub loadavg_1m: f64,
    pub loadavg_5m: f64,
    pub loadavg_15m: f64,
    pub cores: u32,
}

/// RAM usage percentage and memory breakdown in bytes.
#[derive(Serialize, Clone)]
pub struct RamUsageResponse {
    pub usage_percent: u32,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub available_bytes: u64,
}

/// Network traffic rates (RX/TX) per second in bytes and Mbps.
#[derive(Serialize, Clone)]
pub struct NetworkTrafficResponse {
    pub rx_bytes_per_sec: u64,
    pub tx_bytes_per_sec: u64,
    pub rx_mbps: f64,
    pub tx_mbps: f64,
}

/// CPU temperature in Celsius with a sensor label.
#[derive(Serialize, Clone)]
pub struct CpuTempResponse {
    pub temp_celsius: f64,
    pub label: String,
}

/// Software update availability for the app and server OS.
#[derive(Serialize, Clone)]
pub struct UpdateInfoResponse {
    pub has_app_update: bool,
    pub app_current_version: String,
    pub app_latest_version: Option<String>,
    pub has_server_update: bool,
    pub server_current_version: String,
}

/// Returns CPU usage percentage, load averages, and core count for the dashboard widget.
pub async fn handle_widget_cpu_usage(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let content = ProcParser::read_proc_file("/proc/stat");

    let mut total_idle: u64 = 0;
    let mut total_total: u64 = 0;

    if let Some(cpu_line) = content.lines().find(|l| l.starts_with("cpu ")) {
        let parts: Vec<&str> = cpu_line.split_whitespace().skip(1).collect();
        if parts.len() >= 5 {
            let user: u64 = parts[0].parse().unwrap_or(0);
            let nice: u64 = parts[1].parse().unwrap_or(0);
            let system: u64 = parts[2].parse().unwrap_or(0);
            let idle: u64 = parts[3].parse().unwrap_or(0);
            let iowait: u64 = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
            let irq: u64 = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
            let softirq: u64 = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0);

            total_idle = idle + iowait;
            total_total = user + nice + system + idle + iowait + irq + softirq;
        }
    }

    let (prev_idle, prev_total) = {
        let mut prev = PREV_STATS.write();
        let entry = prev.entry("cpu".to_string()).or_insert((0, 0));
        let result = *entry;
        *entry = (total_idle, total_total);
        result
    };

    let usage_percent = if total_total > prev_total {
        let delta_total = total_total - prev_total;
        let delta_idle = total_idle - prev_idle;
        if delta_total > 0 {
            ((delta_total - delta_idle) as f64 / delta_total as f64 * 100.0) as u32
        } else {
            0
        }
    } else {
        0
    };

    let loadavg_str = ProcParser::parse_loadavg();
    let parts: Vec<&str> = loadavg_str.split_whitespace().collect();
    let loadavg_1m: f64 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let loadavg_5m: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let loadavg_15m: f64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);

    let cpu_info = ProcParser::read_proc_file("/proc/cpuinfo");
    let cores = cpu_info
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count() as u32;

    let data = CpuUsageResponse {
        usage_percent,
        loadavg_1m,
        loadavg_5m,
        loadavg_15m,
        cores,
    };

    ResponseHelper::json_success(data)
}

/// Returns RAM usage percentage and memory breakdown for the dashboard widget.
pub async fn handle_widget_ram_usage(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let mem_info = ProcParser::parse_meminfo();

    let total_bytes = mem_info.total_kb * 1024;
    let available_bytes = mem_info.available_kb * 1024;
    let used_bytes = total_bytes.saturating_sub(mem_info.available_kb * 1024);
    let free_bytes = mem_info.free_kb * 1024;

    let usage_percent = if total_bytes > 0 {
        ((total_bytes - available_bytes) as f64 / total_bytes as f64 * 100.0) as u32
    } else {
        0
    };

    let data = RamUsageResponse {
        usage_percent,
        total_bytes,
        used_bytes,
        free_bytes,
        available_bytes,
    };

    ResponseHelper::json_success(data)
}

#[allow(clippy::type_complexity)]
static PREV_NETWORK_STATS: Lazy<RwLock<HashMap<String, (u64, u64, u64)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Returns network traffic rates (RX/TX) per second for the dashboard widget.
pub async fn handle_widget_network_traffic(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let content = ProcParser::read_proc_file("/proc/net/dev");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let mut total_rx: u64 = 0;
    let mut total_tx: u64 = 0;
    let mut iface_stats: HashMap<String, (u64, u64)> = HashMap::new();

    for line in content.lines().skip(2) {
        if let Some(colon_pos) = line.find(':') {
            let iface_name = line[..colon_pos].trim().to_string();
            if iface_name == "lo" {
                continue;
            }
            let after_colon = &line[colon_pos + 1..];
            let mut stats = after_colon.split_whitespace();
            if let (Some(rx), Some(tx)) = (stats.next(), stats.nth(7)) {
                if let (Ok(rx_val), Ok(tx_val)) = (rx.parse::<u64>(), tx.parse::<u64>()) {
                    total_rx += rx_val;
                    total_tx += tx_val;
                    iface_stats.insert(iface_name, (rx_val, tx_val));
                }
            }
        }
    }

    let (prev_rx, prev_tx, prev_ts) = {
        let mut prev = PREV_NETWORK_STATS.write();
        let entry = prev.entry("_total".to_string()).or_insert((0, 0, 0));
        let result = *entry;
        *entry = (total_rx, total_tx, now);
        result
    };

    let elapsed = now.saturating_sub(prev_ts).max(1);
    let rx_bytes_per_sec = total_rx.saturating_sub(prev_rx) * 1000 / elapsed;
    let tx_bytes_per_sec = total_tx.saturating_sub(prev_tx) * 1000 / elapsed;

    let data = NetworkTrafficResponse {
        rx_bytes_per_sec,
        tx_bytes_per_sec,
        rx_mbps: rx_bytes_per_sec as f64 * 8.0 / 1_000_000.0,
        tx_mbps: tx_bytes_per_sec as f64 * 8.0 / 1_000_000.0,
    };

    ResponseHelper::json_success(data)
}

/// Returns CPU temperature in Celsius for the dashboard widget.
pub async fn handle_widget_cpu_temp(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let sensor = ProcParser::get_sensor_info();

    let cpu_temps = &sensor.cpu_temperatures;
    let (label, temp) = if !cpu_temps.is_empty() {
        (cpu_temps[0].0.clone(), cpu_temps[0].1)
    } else {
        let content = ProcParser::read_proc_file("/sys/class/thermal/thermal_zone0/temp");
        let fallback_temp: f64 = content
            .trim()
            .parse::<u64>()
            .map(|v| v as f64 / 1000.0)
            .unwrap_or(0.0);
        ("CPU".to_string(), fallback_temp)
    };

    let data = CpuTempResponse {
        temp_celsius: temp,
        label,
    };

    ResponseHelper::json_success(data)
}

/// Returns software version info and update availability for the dashboard widget.
///
/// When the optional `UPDATE_CHECK_URL` env var points at a JSON manifest
/// (`{"version": "<semver>"}`), the handler compares it against the built-in
/// `CARGO_PKG_VERSION` and reports `has_app_update` / `app_latest_version`.
/// Unset URL, an offline NAS, a timeout or a non-2xx response all degrade
/// silently to "no update known" — a self-hosted box must never fail its
/// dashboard because the update feed is unreachable. Results are cached for
/// [`UPDATE_CACHE_TTL`] to avoid re-fetching on every 30s client refresh.
pub async fn handle_widget_update_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let software = ProcParser::get_software_info();

    let app_version = env!("CARGO_PKG_VERSION").to_string();

    let server_version = format!("{} {}", software.os_name, software.os_version);

    // Fast path: serve the last check result while it is still fresh.
    {
        let cache = UPDATE_CACHE.read();
        if let Some(checked_at) = cache.checked_at {
            if checked_at.elapsed() < UPDATE_CACHE_TTL {
                return ResponseHelper::json_success(UpdateInfoResponse {
                    has_app_update: cache.has_app_update,
                    app_current_version: app_version,
                    app_latest_version: cache.app_latest_version.clone(),
                    has_server_update: false,
                    server_current_version: server_version,
                });
            }
        }
    }

    let (has_app_update, app_latest_version) =
        match std::env::var("UPDATE_CHECK_URL") {
            Ok(url) if !url.trim().is_empty() => {
                match fetch_update_manifest(&url).await {
                    Some(latest) if is_newer_version(&app_version, &latest) => (true, Some(latest)),
                    Some(_) => (false, None),
                    None => (false, None),
                }
            }
            _ => (false, None),
        };

    {
        let mut cache = UPDATE_CACHE.write();
        cache.checked_at = Some(Instant::now());
        cache.has_app_update = has_app_update;
        cache.app_latest_version = app_latest_version.clone();
    }

    ResponseHelper::json_success(UpdateInfoResponse {
        has_app_update,
        app_current_version: app_version,
        app_latest_version,
        has_server_update: false,
        server_current_version: server_version,
    })
}

/// Time a successful update-check result stays valid.
const UPDATE_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// Single-request timeout for the update-manifest fetch.
const UPDATE_FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// Minimal update manifest served at `UPDATE_CHECK_URL`.
#[derive(Debug, Deserialize)]
struct UpdateManifest {
    version: String,
}

/// Process-wide cache of the last update check (self-contained: no AppState
/// change, so no harness churn across the ~40 integration binaries).
static UPDATE_CACHE: Lazy<RwLock<UpdateCheckCache>> =
    Lazy::new(|| RwLock::new(UpdateCheckCache::default()));

#[derive(Default)]
struct UpdateCheckCache {
    checked_at: Option<Instant>,
    has_app_update: bool,
    app_latest_version: Option<String>,
}

/// Fetch the update manifest and return the remote `version` string.
/// Every failure mode (network down, timeout, non-2xx, malformed JSON) maps
/// to `None` — the widget then reports "no update known" instead of erroring.
async fn fetch_update_manifest(url: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(UPDATE_FETCH_TIMEOUT)
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let manifest: UpdateManifest = resp.json().await.ok()?;
    let version = manifest.version.trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

/// `true` when `latest` is newer than `current`, comparing the numeric
/// 3-part version (a leading `v` and any non-numeric suffix are ignored).
fn is_newer_version(current: &str, latest: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .trim_start_matches('v')
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|s| s.parse::<u64>().ok())
            .take(3)
            .collect()
    }

    let current = parts(current);
    let latest = parts(latest);
    let width = current.len().max(latest.len());
    for i in 0..width {
        let a = current.get(i).copied().unwrap_or(0);
        let b = latest.get(i).copied().unwrap_or(0);
        if a != b {
            return b > a;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_check_reports_newer_versions_only() {
        // Same version — no update.
        assert!(!is_newer_version("1.2.3", "1.2.3"));
        // Older / equal-major patch releases — no update.
        assert!(!is_newer_version("1.2.3", "1.2.2"));
        assert!(!is_newer_version("1.2.3", "1.1.9"));
        // Newer patch / minor / major — update.
        assert!(is_newer_version("1.2.3", "1.2.4"));
        assert!(is_newer_version("1.2.3", "1.3.0"));
        assert!(is_newer_version("1.2.3", "2.0.0"));
        // A leading `v` is tolerated.
        assert!(is_newer_version("1.2.3", "v1.2.4"));
        assert!(!is_newer_version("v1.2.4", "1.2.4"));
        // Non-numeric suffixes (rc/beta) are ignored, numeric prefix wins.
        assert!(is_newer_version("1.2.3", "1.2.3-rc.1") == false);
        assert!(is_newer_version("1.2.3-rc.1", "1.2.3") == false);
        assert!(is_newer_version("1.2.2-rc.1", "1.2.3") == true);
    }
}
