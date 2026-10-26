/* =============================================================================
 * File          : health.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   System health checks: a registry of named checks (DB-backed
 *   authentication, storage dir, memory via /proc, disk usage via df, disk
 *   SMART, block devices via lsblk, mdadm array states, link states from
 *   /sys, systemd service states, temperatures), executed on demand and
 *   aggregated for the dashboard and the notification engine.
 *
 *   Two contracts that callers rely on:
 *   - **Nothing shells out on the async runtime here** — `run_all_checks` /
 *     `run_check` await their system subprocess work on
 *     `spawn_blocking` themselves (only the authentication check touches the
 *     async database pool directly). A hung `smartctl` can slow the answer
 *     but can never park a Tokio worker (item ROADMAP 1.1).
 *   - **A failed check degrades to `Unknown`, never an overall error** — one
 *     unreadable file or missing binary yields a per-check `Unknown` result;
 *     the remaining checks still run and the rest of the payload is served
 *     (item ROADMAP 1.5, "partial results").
 *
 * Dependencies:
 *   - once_cell, parking_lot (>= 0.12), serde (>= 1.0), sqlx (async pool read)
 *
 * Usage:
 *   handlers/storage.rs (health endpoints), web/mod.rs (`GET /api/health`)
 *   and notification_engine.rs.
 * =============================================================================
 */

//! System health checks (disks, SMART, services) executed on demand.

use crate::utils::process::{HOST_PROBE_TIMEOUT, SMART_PROBE_TIMEOUT, run_timed};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::process::Command;
use std::time::{Duration, Instant};
use thiserror::Error;

/// SMART status changes on the order of hours, not seconds — caching it
/// avoids spawning `lsblk` + one `smartctl` per disk on every 60s health-check
/// tick (`notification_engine::run_notification_loop`) and on every on-demand
/// `/api/health` call.
const DISK_SMART_CACHE_TTL: Duration = Duration::from_secs(600);

static DISK_SMART_CACHE: Lazy<Mutex<Option<(Instant, HealthCheckResult)>>> =
    Lazy::new(|| Mutex::new(None));

/// Errors that can occur during health checks.
#[derive(Error, Debug)]
pub enum HealthError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
}

// ---------------------------------------------------------------------------
// Deadlines (ROADMAP X.2)
//
// Every host probe of the health checks used to be a bare `Command::output`.
// The checks run on the blocking pool, but a `df` parked on a hung NFS mount,
// an `mdadm --detail` blocked on a failing disk or a `smartctl` on a
// spun-down drive pinned that blocking thread with no ceiling — and the
// health loop runs on a 60 s tick, so the probes piled up behind the wedged
// one. `df`/`lsblk`/`mdadm`/`systemctl` get the shared host-probe budget;
// `smartctl` gets its longer one (a drive can spin up on the first query).
// ---------------------------------------------------------------------------

/// Run one host probe under a deadline, keeping the reason it failed.
fn run_command(cmd: &mut Command, timeout: Duration) -> Result<std::process::Output, String> {
    run_timed(cmd, timeout)
}

/// Map a [`run_command`] failure into the `HealthError` the checks return.
fn io_error(e: String) -> HealthError {
    HealthError::Io(std::io::Error::other(e))
}

/// The result of a single health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub name: String,
    pub status: HealthStatus,
    pub message: String,
    pub details: Option<serde_json::Value>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Possible health status values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Warning,
    Unhealthy,
    Unknown,
}

/// A persistent alert that can be displayed on the health dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRecord {
    pub id: String,
    pub severity: AlertSeverity,
    pub title: String,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub read: bool,
}

/// Alert severity levels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

/// Aggregated system health combining multiple checks and active alerts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub overall_status: HealthStatus,
    pub checks: Vec<HealthCheckResult>,
    pub alerts: Vec<AlertRecord>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Service that runs system health checks (memory, disk, network, services, etc.).
pub struct HealthService;

impl HealthService {
    /// Run all built-in health checks and return an aggregated result.
    ///
    /// Async by design: the eight system checks (df, lsblk, smartctl, mdadm,
    /// systemctl, /proc + /sys reads) are offloaded to `spawn_blocking` inside
    /// this function, and `check_authentication` awaits the database pool on
    /// the async runtime where it belongs. Callers simply `.await` it — do not
    /// re-wrap this call in blocking offloading, it would be redundant.
    ///
    /// Every check degrades independently: a check that errors (unreadable
    /// file, missing binary, unreachable DB) becomes an `Unknown` entry named
    /// after its slot, and the remaining checks still run. The aggregate is
    /// always complete (all nine slots present); a hard `Err` is reserved for
    /// the aggregate genuinely failing (e.g. the check task panicking).
    pub async fn run_all_checks(pool: &SqlitePool) -> Result<SystemHealth, HealthError> {
        let authentication = Self::check_authentication(pool).await;

        let system_checks = tokio::task::spawn_blocking(move || {
            vec![
                Self::degrade("storage", Self::check_storage()),
                Self::degrade("memory", Self::check_memory()),
                Self::degrade("disk_space", Self::check_disk_space()),
                Self::degrade("disk_smart", Self::check_disk_smart()),
                Self::degrade("raid_arrays", Self::check_raid_arrays()),
                Self::degrade("network", Self::check_network_interfaces()),
                Self::degrade("services", Self::check_systemd_services()),
                Self::degrade("temperatures", Self::check_temperatures()),
            ]
        })
        .await
        .map_err(|e| HealthError::Parse(format!("health check task failed: {e}")))?;

        let mut checks = Vec::with_capacity(system_checks.len() + 1);
        checks.push(Self::degrade("authentication", authentication));
        checks.extend(system_checks);

        let overall_status = Self::determine_overall_status(&checks);

        Ok(SystemHealth {
            overall_status,
            checks,
            alerts: Vec::new(),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Run a single health check by name.
    ///
    /// Same contracts as [`run_all_checks`]: off-runtime for the system
    /// checks, direct DB read for `authentication`. A failed check returns
    /// its raw error here (the single-check handler maps it to a failed
    /// JSON body); only [`run_all_checks`] owes the UI the degraded
    /// nine-slot contract.
    pub async fn run_check(
        name: &str,
        pool: &SqlitePool,
    ) -> Result<HealthCheckResult, HealthError> {
        if name == "authentication" {
            return Self::check_authentication(pool).await;
        }

        let name = name.to_string();
        tokio::task::spawn_blocking(move || Self::run_system_check(&name))
            .await
            .map_err(|e| HealthError::Parse(format!("health check task failed: {e}")))?
    }

    /// The synchronous dispatcher for the eight DB-free checks
    /// (everything except `authentication`). Runs on the blocking pool only.
    fn run_system_check(name: &str) -> Result<HealthCheckResult, HealthError> {
        match name {
            "storage" => Self::check_storage(),
            "memory" => Self::check_memory(),
            "disk_space" => Self::check_disk_space(),
            "disk_smart" => Self::check_disk_smart(),
            "raid_arrays" => Self::check_raid_arrays(),
            "network" => Self::check_network_interfaces(),
            "services" => Self::check_systemd_services(),
            "temperatures" => Self::check_temperatures(),
            _ => Err(HealthError::Parse(format!("Unknown check: {}", name))),
        }
    }

    /// Turn a failed check into a well-formed `Unknown` result, preserving
    /// the slot name and carrying the failure in the human message. This is
    /// what makes a single broken check unable to blank `/api/health`.
    fn degrade(name: &str, result: Result<HealthCheckResult, HealthError>) -> HealthCheckResult {
        match result {
            Ok(result) => result,
            Err(e) => HealthCheckResult {
                name: name.to_string(),
                status: HealthStatus::Unknown,
                message: format!("Check unavailable: {}", e),
                details: None,
                timestamp: chrono::Utc::now(),
            },
        }
    }

    /// Real database probe of the account / session store (item ROADMAP 1.3).
    ///
    /// Answers whether the database-backed authentication store can actually
    /// answer a query (the account table). A corrupt / unreachable database
    /// surfaces here as an error — degraded to `Unknown` by the caller —
    /// instead of a hardcoded "operational". Only a scalar count is read:
    /// this endpoint is household-readable, so no tokens, usernames or
    /// session material may appear here.
    pub async fn check_authentication(pool: &SqlitePool) -> Result<HealthCheckResult, HealthError> {
        let accounts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(pool)
            .await?;

        Ok(HealthCheckResult {
            name: "authentication".to_string(),
            status: HealthStatus::Healthy,
            message: "Authentication store reachable".to_string(),
            details: Some(serde_json::json!({
                "accounts": accounts,
            })),
            timestamp: chrono::Utc::now(),
        })
    }

    pub fn check_storage() -> Result<HealthCheckResult, HealthError> {
        let data_dir = crate::config::paths::base_dir()
            .to_string_lossy()
            .into_owned();

        let path = std::path::Path::new(&data_dir);
        if path.exists() && path.is_dir() {
            Ok(HealthCheckResult {
                name: "storage".to_string(),
                status: HealthStatus::Healthy,
                message: format!("Storage directory accessible: {}", data_dir),
                details: None,
                timestamp: chrono::Utc::now(),
            })
        } else {
            Ok(HealthCheckResult {
                name: "storage".to_string(),
                status: HealthStatus::Warning,
                message: format!("Storage directory not found: {}", data_dir),
                details: None,
                timestamp: chrono::Utc::now(),
            })
        }
    }

    pub fn check_memory() -> Result<HealthCheckResult, HealthError> {
        let meminfo = std::fs::read_to_string("/proc/meminfo")?;

        let mut total = 0u64;
        let mut available = 0u64;

        for line in meminfo.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }

            let value: u64 = parts[1].parse().unwrap_or(0) * 1024;

            match parts[0] {
                "MemTotal:" => total = value,
                "MemAvailable:" => available = value,
                _ => {}
            }
        }

        let used = total.saturating_sub(available);
        let usage_percent = if total > 0 {
            (used as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };

        let (status, message) = if usage_percent > 90 {
            (
                HealthStatus::Unhealthy,
                format!("Memory usage critical: {}%", usage_percent),
            )
        } else if usage_percent > 75 {
            (
                HealthStatus::Warning,
                format!("Memory usage high: {}%", usage_percent),
            )
        } else {
            (
                HealthStatus::Healthy,
                format!("Memory usage normal: {}%", usage_percent),
            )
        };

        Ok(HealthCheckResult {
            name: "memory".to_string(),
            status,
            message,
            details: Some(serde_json::json!({
                "total_bytes": total,
                "used_bytes": used,
                "available_bytes": available,
                "usage_percent": usage_percent
            })),
            timestamp: chrono::Utc::now(),
        })
    }

    pub fn check_disk_space() -> Result<HealthCheckResult, HealthError> {
        let mut cmd = Command::new("df");
        cmd.args(["-B1", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs"]);
        let output = run_command(&mut cmd, HOST_PROBE_TIMEOUT).map_err(io_error)?;

        let mut critical_mounts = Vec::new();
        let mut warnings = Vec::new();

        for line in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 6 {
                continue;
            }

            let mount = parts.last().unwrap();
            if mount.starts_with("/snap") || mount.starts_with("/run") {
                continue;
            }

            let size: u64 = parts[1].parse().unwrap_or(0);
            let used: u64 = parts[2].parse().unwrap_or(0);

            let usage_percent = if size > 0 {
                (used as f64 / size as f64 * 100.0) as u32
            } else {
                0
            };

            if usage_percent > 95 {
                critical_mounts.push(format!("{}: {}%", mount, usage_percent));
            } else if usage_percent > 85 {
                warnings.push(format!("{}: {}%", mount, usage_percent));
            }
        }

        let (status, message) = if !critical_mounts.is_empty() {
            (
                HealthStatus::Unhealthy,
                format!("Critical: {}", critical_mounts.join(", ")),
            )
        } else if !warnings.is_empty() {
            (
                HealthStatus::Warning,
                format!("Warning: {}", warnings.join(", ")),
            )
        } else {
            (
                HealthStatus::Healthy,
                "All filesystems have sufficient space".to_string(),
            )
        };

        Ok(HealthCheckResult {
            name: "disk_space".to_string(),
            status,
            message,
            details: None,
            timestamp: chrono::Utc::now(),
        })
    }

    /// RAID array health: every mdadm array's reported state, so a degraded
    /// array (reduced redundancy, waiting for a replacement disk) surfaces
    /// as a warning, a failed/invalid array as an error, and a clean one as
    /// healthy. mdadm itself isn't installed on all builds — that yields
    /// `Unknown`, never a hard failure.
    pub fn check_raid_arrays() -> Result<HealthCheckResult, HealthError> {
        let mut cmd = Command::new("sudo");
        cmd.arg("mdadm");
        cmd.args(["--detail", "--scan"]);
        let scan = run_command(&mut cmd, HOST_PROBE_TIMEOUT);

        let scan = match scan {
            Ok(s) if s.status.success() => s,
            // mdadm missing (or the platform has no sudoers grant yet):
            // report an unknown, non-failing state.
            Ok(s) => {
                let stderr = String::from_utf8_lossy(&s.stderr).to_ascii_lowercase();
                // Only "mdadm missing/killed by permission" cases matter
                // here; mdadm installed but refusing is a real state too.
                let _ = stderr;
                return Ok(HealthCheckResult {
                    name: "raid_arrays".to_string(),
                    status: HealthStatus::Unknown,
                    message: "mdadm is not reporting array state".to_string(),
                    details: None,
                    timestamp: chrono::Utc::now(),
                });
            }
            Err(e) => {
                return Ok(HealthCheckResult {
                    name: "raid_arrays".to_string(),
                    status: HealthStatus::Unknown,
                    message: format!("Could not query mdadm: {}", e),
                    details: None,
                    timestamp: chrono::Utc::now(),
                });
            }
        };

        // "ARRAY /dev/md/<name> metadata=… name=… UUID=…"
        let devices: Vec<String> = String::from_utf8_lossy(&scan.stdout)
            .lines()
            .filter(|l| l.starts_with("ARRAY"))
            .filter_map(|l| l.split_whitespace().nth(1).map(String::from))
            .collect();

        if devices.is_empty() {
            return Ok(HealthCheckResult {
                name: "raid_arrays".to_string(),
                status: HealthStatus::Healthy,
                message: "No RAID arrays defined".to_string(),
                details: None,
                timestamp: chrono::Utc::now(),
            });
        }

        let mut degraded = Vec::new();
        let mut failed = Vec::new();
        let mut healthy = 0usize;
        for device in &devices {
            let mut cmd = Command::new("sudo");
            cmd.arg("mdadm");
            cmd.args(["--detail", device]);
            let detail = run_command(&mut cmd, HOST_PROBE_TIMEOUT);
            let state = detail
                .ok()
                .map(|d| {
                    String::from_utf8_lossy(&d.stdout)
                        .lines()
                        .find_map(|l| l.strip_prefix("State :").map(|s| s.trim().to_string()))
                        .unwrap_or_default()
                })
                .unwrap_or_default()
                .to_ascii_lowercase();
            if state.contains("failed") || state.contains("inactive") || state.contains("bad") {
                failed.push(format!("{} ({})", device, state));
            } else if state.contains("degraded") || state.contains("rebuil") {
                degraded.push(format!("{} ({})", device, state));
            } else {
                healthy += 1;
            }
        }

        let (status, message) = if !failed.is_empty() {
            (
                HealthStatus::Unhealthy,
                format!("Failed arrays: {}", failed.join(", ")),
            )
        } else if !degraded.is_empty() {
            (
                HealthStatus::Warning,
                format!("Degraded arrays: {}", degraded.join(", ")),
            )
        } else {
            (
                HealthStatus::Healthy,
                format!("All {} RAID array(s) healthy", healthy),
            )
        };

        Ok(HealthCheckResult {
            name: "raid_arrays".to_string(),
            status,
            message,
            details: None,
            timestamp: chrono::Utc::now(),
        })
    }

    pub fn check_disk_smart() -> Result<HealthCheckResult, HealthError> {
        if let Some((checked_at, cached)) = DISK_SMART_CACHE.lock().clone() {
            if checked_at.elapsed() < DISK_SMART_CACHE_TTL {
                return Ok(cached);
            }
        }

        let result = Self::check_disk_smart_uncached()?;
        *DISK_SMART_CACHE.lock() = Some((Instant::now(), result.clone()));
        Ok(result)
    }

    fn check_disk_smart_uncached() -> Result<HealthCheckResult, HealthError> {
        let mut unhealthy_disks = Vec::new();
        let mut healthy_disks = Vec::new();

        let mut cmd = Command::new("lsblk");
        cmd.args(["-d", "-n", "-o", "NAME,TYPE"]);
        let lsblk_output = run_command(&mut cmd, HOST_PROBE_TIMEOUT).map_err(io_error)?;

        for line in String::from_utf8_lossy(&lsblk_output.stdout).lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            if parts[1] != "disk" || crate::disk::DiskService::is_pseudo_disk_name(parts[0]) {
                continue;
            }

            let device = format!("/dev/{}", parts[0]);

            // `smartctl` needs root on `660 root:disk` devices; the shared
            // helper (src/smart.rs) carries the sudoers-granted argv shape
            // (item ROADMAP 1.2). Running this whole function is the job of
            // `spawn_blocking` in run_all_checks/run_check.
            let mut cmd = crate::smart::std_command(&device, &["-H"]);
            let output = run_command(&mut cmd, SMART_PROBE_TIMEOUT);

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.contains("PASSED") || stdout.contains("OK") {
                    healthy_disks.push(parts[0].to_string());
                } else if stdout.contains("FAILED") {
                    unhealthy_disks.push(parts[0].to_string());
                }
            }
        }

        let (status, message) = if !unhealthy_disks.is_empty() {
            (
                HealthStatus::Unhealthy,
                format!("Unhealthy disks: {}", unhealthy_disks.join(", ")),
            )
        } else if healthy_disks.is_empty() {
            (HealthStatus::Unknown, "No SMART data available".to_string())
        } else {
            (
                HealthStatus::Healthy,
                format!("All {} disks healthy", healthy_disks.len()),
            )
        };

        Ok(HealthCheckResult {
            name: "disk_smart".to_string(),
            status,
            message,
            details: Some(serde_json::json!({
                "healthy_disks": healthy_disks,
                "unhealthy_disks": unhealthy_disks
            })),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Network health: the **link state**, not the counters (item ROADMAP
    /// 1.4). The previous implementation inferred "up" from rx/tx activity in
    /// `/proc/net/dev`, which flagged a correctly configured but idle
    /// interface as down and kept calling a pulled cable "up" while stale
    /// counters survived. The link state now comes from
    /// `/sys/class/net/<iface>/operstate` (`up`/`down`/`unknown`/…, the same
    /// source `ip link` reports); `lo` is special-cased as up (its operstate
    /// is always `unknown`). The traffic counters are still reported in the
    /// details, they no longer decide the verdict.
    pub fn check_network_interfaces() -> Result<HealthCheckResult, HealthError> {
        // Traffic counters per interface, read for detail display only.
        let counters = std::fs::read_to_string("/proc/net/dev")
            .map(|content| {
                let mut map = std::collections::HashMap::new();
                for line in content.lines().skip(2) {
                    let Some((name, stats)) = line.split_once(':') else {
                        continue;
                    };
                    let stats: Vec<&str> = stats.split_whitespace().collect();
                    let rx_bytes: u64 = stats.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let tx_bytes: u64 = stats.get(8).and_then(|s| s.parse().ok()).unwrap_or(0);
                    map.insert(name.trim().to_string(), (rx_bytes, tx_bytes));
                }
                map
            })
            .unwrap_or_default();

        let mut interfaces = Vec::new();
        let mut up_count = 0;
        let mut down_count = 0;

        let entries = std::fs::read_dir("/sys/class/net")?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let operstate = std::fs::read_to_string(entry.path().join("operstate"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let is_loopback = name == "lo";
            let is_up = link_is_up(&operstate, is_loopback);

            if is_loopback {
                // Loopback is always operationally there; it must never
                // satisfy the "some interface is active" verdict alone.
                interfaces.push(serde_json::json!({
                    "name": name,
                    "up": is_up,
                    "operstate": operstate,
                    "loopback": true,
                }));
                continue;
            }

            if is_up {
                up_count += 1;
            } else {
                down_count += 1;
            }

            let (rx_bytes, tx_bytes) = counters.get(&name).copied().unwrap_or((0, 0));
            interfaces.push(serde_json::json!({
                "name": name,
                "up": is_up,
                "operstate": operstate,
                "rx_bytes": rx_bytes,
                "tx_bytes": tx_bytes
            }));
        }

        let (status, message) = if up_count > 0 {
            (
                HealthStatus::Healthy,
                format!("{} interfaces up, {} down", up_count, down_count),
            )
        } else if down_count > 0 {
            (
                HealthStatus::Warning,
                "No active network interfaces".to_string(),
            )
        } else {
            (
                HealthStatus::Unknown,
                "No non-loopback network interfaces".to_string(),
            )
        };

        Ok(HealthCheckResult {
            name: "network".to_string(),
            status,
            message,
            details: Some(serde_json::json!({ "interfaces": interfaces })),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Services health. The appliance's own unit is **derived**, not
    /// hardcoded (item ROADMAP 1.3): the cgroup path of the current process
    /// under systemd is `…/system.slice/<unit>.service`, so the check covers
    /// whatever name the service was installed under. If the process is not
    /// running inside a systemd unit (a dev checkout, a container) the own
    /// unit is simply not checked instead of misreporting `inactive`.
    pub fn check_systemd_services() -> Result<HealthCheckResult, HealthError> {
        let own_unit = parse_cgroup_unit(&std::fs::read_to_string("/proc/self/cgroup")?);
        let services = service_names(own_unit.as_deref());

        let mut running = Vec::new();
        let mut failed = Vec::new();
        let mut own_service_failed = None;

        for service in &services {
            let mut cmd = Command::new("systemctl");
            cmd.args(["is-active", service]);
            let output = run_command(&mut cmd, HOST_PROBE_TIMEOUT);

            if let Ok(out) = output {
                let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if status == "active" {
                    running.push(service.replace(".service", ""));
                } else if status == "failed" {
                    if Some(service.as_str()) == own_unit.as_deref() {
                        own_service_failed = Some(service.clone());
                    }
                    failed.push(service.replace(".service", ""));
                }
            }
        }

        // The appliance's own unit failing while we are still running is the
        // unlock-restart-loop shape: that is not a warning, it is broken.
        let (status, message) = if own_service_failed.is_some() {
            (
                HealthStatus::Unhealthy,
                format!("Failed: {}", failed.join(", ")),
            )
        } else if !failed.is_empty() {
            (
                HealthStatus::Warning,
                format!("Failed: {}", failed.join(", ")),
            )
        } else {
            (
                HealthStatus::Healthy,
                format!("{} critical services running", running.len()),
            )
        };

        Ok(HealthCheckResult {
            name: "services".to_string(),
            status,
            message,
            details: Some(serde_json::json!({
                "running": running,
                "failed": failed,
                "own_unit": own_unit,
            })),
            timestamp: chrono::Utc::now(),
        })
    }

    pub fn check_temperatures() -> Result<HealthCheckResult, HealthError> {
        let mut temperatures = Vec::new();
        let mut critical = Vec::new();
        let mut warnings = Vec::new();

        let temp_paths = vec![
            "/sys/class/thermal/thermal_zone0/temp",
            "/sys/class/hwmon/hwmon0/temp1_input",
        ];

        for path in temp_paths {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(temp_milli) = content.trim().parse::<i64>() {
                    let temp_celsius = temp_milli as f64 / 1000.0;
                    temperatures.push(temp_celsius);

                    if temp_celsius > 85.0 {
                        critical.push(temp_celsius);
                    } else if temp_celsius > 70.0 {
                        warnings.push(temp_celsius);
                    }
                }
            }
        }

        let (status, message) = if !critical.is_empty() {
            (
                HealthStatus::Unhealthy,
                format!("Critical temperatures: {:?}", critical),
            )
        } else if !warnings.is_empty() {
            (
                HealthStatus::Warning,
                format!("High temperatures: {:?}", warnings),
            )
        } else if temperatures.is_empty() {
            (
                HealthStatus::Unknown,
                "No temperature data available".to_string(),
            )
        } else {
            let avg = temperatures.iter().sum::<f64>() / temperatures.len() as f64;
            (HealthStatus::Healthy, format!("Average: {:.1}C", avg))
        };

        Ok(HealthCheckResult {
            name: "temperatures".to_string(),
            status,
            message,
            details: Some(serde_json::json!({ "temperatures": temperatures })),
            timestamp: chrono::Utc::now(),
        })
    }

    pub fn determine_overall_status(checks: &[HealthCheckResult]) -> HealthStatus {
        if checks.iter().any(|c| c.status == HealthStatus::Unhealthy) {
            HealthStatus::Unhealthy
        } else if checks.iter().any(|c| c.status == HealthStatus::Warning) {
            HealthStatus::Warning
        } else if checks.iter().all(|c| c.status == HealthStatus::Healthy) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unknown
        }
    }
}

/// The monitored service list: the infrastructure services HomeVault runs
/// on, plus the appliance's own (derived) unit.
fn service_names(own_unit: Option<&str>) -> Vec<String> {
    let mut services: Vec<String> = vec![
        "cron.service".into(),
        "systemd-logind.service".into(),
        "network-manager.service".into(),
    ];
    if let Some(own) = own_unit {
        if !services.iter().any(|s| s == own) {
            services.push(own.to_string());
        }
    }
    services
}

/// Decide a check's link state from its `operstate`, pure and unit-tested.
///
/// `lo` and `tun*`-style devices report `unknown` because they have no
/// carrier to report — loopback is always treated as up. Every other state
/// (`down`, `lowerlayerdown`, `unknown`, empty/unreadable) is **not** up;
/// none of them may ever be inferred from idle traffic counters again.
fn link_is_up(operstate: &str, is_loopback: bool) -> bool {
    if is_loopback {
        return true;
    }
    operstate == "up"
}

/// Extract the systemd unit the current process runs under from
/// `/proc/self/cgroup` (v2: `0::/system.slice/homevault.service`). Returns
/// `None` when the process is not running inside a `.service` unit.
fn parse_cgroup_unit(cgroup: &str) -> Option<String> {
    cgroup.lines().find_map(|line| {
        let last = line.rsplit('/').next()?;
        last.ends_with(".service").then(|| last.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    // `check_storage` / `run_all_checks` read the process-global
    // `DATA_DIRECTORY` env var — serialise the tests that mutate it.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn check(status: HealthStatus) -> HealthCheckResult {
        HealthCheckResult {
            name: "x".into(),
            status,
            message: String::new(),
            details: None,
            timestamp: chrono::Utc::now(),
        }
    }

    // -- determine_overall_status: documented precedence -------------------

    #[test]
    fn test_determine_overall_status_all_healthy() {
        let checks = [check(HealthStatus::Healthy), check(HealthStatus::Healthy)];
        assert_eq!(
            HealthService::determine_overall_status(&checks),
            HealthStatus::Healthy
        );
    }

    #[test]
    fn test_determine_overall_status_warning_outranks_healthy() {
        let checks = [check(HealthStatus::Healthy), check(HealthStatus::Warning)];
        assert_eq!(
            HealthService::determine_overall_status(&checks),
            HealthStatus::Warning
        );
    }

    #[test]
    fn test_determine_overall_status_unhealthy_outranks_warning() {
        let checks = [
            check(HealthStatus::Healthy),
            check(HealthStatus::Warning),
            check(HealthStatus::Unhealthy),
        ];
        assert_eq!(
            HealthService::determine_overall_status(&checks),
            HealthStatus::Unhealthy
        );
    }

    #[test]
    fn test_determine_overall_status_healthy_plus_unknown_is_unknown() {
        // No Unhealthy, no Warning, but not *all* Healthy either.
        let checks = [check(HealthStatus::Healthy), check(HealthStatus::Unknown)];
        assert_eq!(
            HealthService::determine_overall_status(&checks),
            HealthStatus::Unknown
        );
    }

    #[test]
    fn test_determine_overall_status_empty_is_healthy() {
        // `Iterator::all` is vacuously true on an empty slice — pin the
        // current behaviour so a refactor can't change it silently.
        assert_eq!(
            HealthService::determine_overall_status(&[]),
            HealthStatus::Healthy
        );
    }

    // -- HealthStatus serde ----------------------------------------------

    #[test]
    fn test_health_status_serializes_pascal_case() {
        // The `/api/health` handler maps these to lowercase by hand; if the
        // wire form here ever changes, that mapping (and the frontend
        // `CheckStatus` union) must change with it.
        for (variant, json) in [
            (HealthStatus::Healthy, "\"Healthy\""),
            (HealthStatus::Warning, "\"Warning\""),
            (HealthStatus::Unhealthy, "\"Unhealthy\""),
            (HealthStatus::Unknown, "\"Unknown\""),
        ] {
            assert_eq!(serde_json::to_string(&variant).unwrap(), json);
            let back: HealthStatus = serde_json::from_str(json).unwrap();
            assert_eq!(back, variant);
        }
    }

    // -- partial-results contract (item ROADMAP 1.5) ---------------------------

    #[test]
    fn test_degrade_passes_a_successful_check_through_unchanged() {
        let ok = check(HealthStatus::Healthy);
        // A passing check keeps its own name and message; the slot name is
        // only applied to the degraded fallback.
        let passed = HealthService::degrade("memory", Ok(ok.clone()));
        assert_eq!(passed.name, ok.name);
        assert_eq!(passed.status, ok.status);
        assert_eq!(passed.message, ok.message);
        assert_eq!(passed.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_degrade_turns_a_failure_into_an_unknown_slot_with_its_name() {
        let failed = HealthService::degrade(
            "disk_space",
            Err(HealthError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such file or directory",
            ))),
        );
        assert_eq!(failed.name, "disk_space");
        assert_eq!(failed.status, HealthStatus::Unknown);
        assert!(!failed.message.trim().is_empty());
        // The failure reason must be visible: "Check unavailable: <error>".
        assert!(
            failed.message.contains("Check unavailable"),
            "message: {}",
            failed.message
        );
    }

    // -- run_system_check: the known names still dispatch ------------------

    #[test]
    fn test_run_system_check_rejects_an_unknown_name() {
        let err = HealthService::run_system_check("does-not-exist").unwrap_err();
        assert!(matches!(err, HealthError::Parse(_)));
    }

    // -- run_all_checks / run_check DB paths → tests/health_integration.rs --

    // -- link state (item ROADMAP 1.4) --------------------------------------

    #[test]
    fn test_link_is_up_follows_operstate_not_counters() {
        // A real link states report.
        assert!(link_is_up("up", false));
        assert!(!link_is_up("down", false));
        // A dormant / lower-state link is not up either.
        assert!(!link_is_up("lowerlayerdown", false));
        // An unreadable operstate must never count as up.
        assert!(!link_is_up("", false));
        // unknown-without-carrier is not up for a physical interface…
        assert!(!link_is_up("unknown", false));
        // …but loopback is always operationally there.
        assert!(link_is_up("unknown", true));
        assert!(link_is_up("up", true));
    }

    #[test]
    fn test_check_network_interfaces_lists_loopback_and_reports_operstate() {
        let c = HealthService::check_network_interfaces().unwrap();
        assert_eq!(c.name, "network");
        let ifaces = c.details.unwrap()["interfaces"].as_array().unwrap().clone();
        let names: Vec<String> = ifaces
            .iter()
            .filter_map(|i| i["name"].as_str().map(String::from))
            .collect();
        assert!(names.iter().any(|n| n == "lo"), "no loopback in {names:?}");
        // Every interface entry carries its operstate — the source of truth
        // this check now uses — and loopback is flagged as such.
        for i in &ifaces {
            assert!(i["operstate"].is_string(), "no operstate: {i}");
        }
        let lo = ifaces.iter().find(|i| i["name"] == "lo").unwrap();
        assert_eq!(lo["loopback"], serde_json::json!(true));
        let _ = names;
    }

    // -- derived own service unit (item ROADMAP 1.3) ------------------------

    #[test]
    fn test_parse_cgroup_unit_reads_the_slice_path() {
        let unit = parse_cgroup_unit("0::/system.slice/homevault.service\n");
        assert_eq!(unit.as_deref(), Some("homevault.service"));
    }

    #[test]
    fn test_parse_cgroup_unit_ignores_non_service_units() {
        // A dev checkout or a container is not a systemd service — the own
        // unit must be absent rather than guessed.
        assert_eq!(parse_cgroup_unit("0::/\n"), None);
        assert_eq!(parse_cgroup_unit("0::/docker/"), None);
        assert_eq!(parse_cgroup_unit("0::/docker/abc123\n"), None);
    }

    #[test]
    fn test_parse_cgroup_unit_accepts_any_slice_depth() {
        assert_eq!(
            parse_cgroup_unit(
                "2::/user.slice/user-1000.slice/user@1000.service/app.slice/xyz.service\n"
            )
            .as_deref(),
            Some("xyz.service")
        );
    }

    #[test]
    fn test_service_names_include_the_derived_own_unit_without_duplicates() {
        let with_own = service_names(Some("homevault.service"));
        assert_eq!(
            with_own,
            vec![
                "cron.service".to_string(),
                "systemd-logind.service".to_string(),
                "network-manager.service".to_string(),
                "homevault.service".to_string(),
            ]
        );
        // An own unit already covered by the base list is not added twice.
        let deduped = service_names(Some("cron.service"));
        assert_eq!(
            deduped,
            vec![
                "cron.service".to_string(),
                "systemd-logind.service".to_string(),
                "network-manager.service".to_string(),
            ]
        );
    }

    // -- individual checks (DB paths → tests/health_integration.rs) ---------

    #[test]
    fn test_check_storage_healthy_when_data_dir_exists_warning_otherwise() {
        let _g = env_guard();
        let prev = std::env::var("DATA_DIRECTORY").ok();

        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", tmp.path());
        let ok = HealthService::check_storage().unwrap();
        assert_eq!(ok.name, "storage");
        assert_eq!(ok.status, HealthStatus::Healthy);

        std::env::set_var("DATA_DIRECTORY", tmp.path().join("nope-does-not-exist"));
        let missing = HealthService::check_storage().unwrap();
        assert_eq!(missing.status, HealthStatus::Warning);

        match prev {
            Some(v) => std::env::set_var("DATA_DIRECTORY", v),
            None => std::env::remove_var("DATA_DIRECTORY"),
        }
    }

    #[test]
    fn test_check_memory_reports_a_bounded_percent_matching_its_status() {
        let c = HealthService::check_memory().unwrap();
        assert_eq!(c.name, "memory");
        let pct = c.details.as_ref().unwrap()["usage_percent"]
            .as_u64()
            .unwrap();
        assert!(pct <= 100, "usage_percent out of range: {pct}");
        let expected = if pct > 90 {
            HealthStatus::Unhealthy
        } else if pct > 75 {
            HealthStatus::Warning
        } else {
            HealthStatus::Healthy
        };
        assert_eq!(c.status, expected, "status must match {pct}%");
        // `used` is `total.saturating_sub(available)` — so used + available
        // is exactly total whenever available <= total, which it is here.
        let d = c.details.unwrap();
        let total = d["total_bytes"].as_u64().unwrap();
        let used = d["used_bytes"].as_u64().unwrap();
        let avail = d["available_bytes"].as_u64().unwrap();
        assert!(total > 0);
        assert!(used <= total && avail <= total);
        assert_eq!(used + avail, total);
    }

    // -- subprocess deadlines (ROADMAP X.2) -----------------------------

    /// The defect this guards against is invisible to a behavioural test: a
    /// bare probe still answers correctly on an idle box and only pins the
    /// health loop when `df`/`mdadm`/`smartctl`/`systemctl` stops answering.
    ///
    /// `concat!` keeps each pattern out of this file's own source, so the
    /// assertion cannot count itself.
    #[test]
    fn test_no_subprocess_of_this_module_runs_without_a_deadline() {
        let src = include_str!("health.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for unbounded in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!(".", "status()"),
            concat!("wait_with_", "output()"),
        ] {
            assert!(
                !production.contains(unbounded),
                "`{unbounded}` waits without a deadline — use `run_command` (ROADMAP X.2)"
            );
        }
    }

    #[test]
    fn test_run_command_kills_a_hung_probe_and_names_the_deadline() {
        let started = Instant::now();
        let error = run_command(Command::new("sleep").arg("30"), Duration::from_millis(120))
            .expect_err("a hung probe must not succeed");
        assert!(error.contains("did not finish within"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must be enforced, not waited out"
        );
    }

    #[test]
    fn test_run_command_keeps_a_non_zero_exit_as_data() {
        // `systemctl is-active` on a stopped unit exits non-zero and the check
        // reads the status; a non-zero exit must arrive as output.
        let out = run_command(&mut Command::new("false"), Duration::from_secs(5))
            .expect("`false` still runs to completion");
        assert_eq!(out.status.code(), Some(1));
    }

    #[test]
    fn test_run_command_reports_a_missing_probe() {
        let error = run_command(
            &mut Command::new("homevault-no-such-binary"),
            Duration::from_secs(1),
        )
        .expect_err("a missing tool is an error");
        assert!(error.contains("Failed to run"), "{error}");
    }

    #[test]
    fn test_health_probe_budgets_are_bounded_and_ordered() {
        // SMART gets the longer slot because a spinning disk may need to spin
        // up; both must stay well under the 60 s health tick so the probes
        // cannot pile up behind one another.
        assert!(HOST_PROBE_TIMEOUT >= Duration::from_secs(1));
        assert!(SMART_PROBE_TIMEOUT >= HOST_PROBE_TIMEOUT);
        assert!(SMART_PROBE_TIMEOUT <= Duration::from_secs(30));
    }
}
