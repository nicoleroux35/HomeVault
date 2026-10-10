use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
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
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Service not found: {0}")]
    ServiceNotFound(String),
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
    pub fn run_all_checks() -> Result<SystemHealth, HealthError> {
        let checks = vec![
            Self::check_authentication()?,
            Self::check_storage()?,
            Self::check_memory()?,
            Self::check_disk_space()?,
            Self::check_disk_smart()?,
            Self::check_network_interfaces()?,
            Self::check_systemd_services()?,
            Self::check_temperatures()?,
        ];

        let overall_status = Self::determine_overall_status(&checks);

        Ok(SystemHealth {
            overall_status,
            checks,
            alerts: Vec::new(),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Run a single health check by name.
    pub fn run_check(name: &str) -> Result<HealthCheckResult, HealthError> {
        match name {
            "authentication" => Self::check_authentication(),
            "storage" => Self::check_storage(),
            "memory" => Self::check_memory(),
            "disk_space" => Self::check_disk_space(),
            "disk_smart" => Self::check_disk_smart(),
            "network" => Self::check_network_interfaces(),
            "services" => Self::check_systemd_services(),
            "temperatures" => Self::check_temperatures(),
            _ => Err(HealthError::Parse(format!("Unknown check: {}", name))),
        }
    }

    pub fn check_authentication() -> Result<HealthCheckResult, HealthError> {
        Ok(HealthCheckResult {
            name: "authentication".to_string(),
            status: HealthStatus::Healthy,
            message: "Authentication system operational".to_string(),
            details: None,
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
        let output = Command::new("df")
            .args(["-B1", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs"])
            .output()?;

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

        let lsblk_output = Command::new("lsblk")
            .args(["-d", "-n", "-o", "NAME,TYPE"])
            .output()?;

        for line in String::from_utf8_lossy(&lsblk_output.stdout).lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            if parts[1] != "disk" {
                continue;
            }

            let device = format!("/dev/{}", parts[0]);

            let output = Command::new("smartctl").args(["-H", &device]).output();

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

    pub fn check_network_interfaces() -> Result<HealthCheckResult, HealthError> {
        let output = std::fs::read_to_string("/proc/net/dev")?;

        let mut interfaces = Vec::new();
        let mut up_count = 0;
        let mut down_count = 0;

        for line in output.lines().skip(2) {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 2 {
                continue;
            }

            let name = parts[0].trim().to_string();
            let stats: Vec<&str> = parts[1].split_whitespace().collect();

            let rx_bytes: u64 = stats.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            let tx_bytes: u64 = stats.get(8).and_then(|s| s.parse().ok()).unwrap_or(0);

            let is_up = rx_bytes > 0 || tx_bytes > 0;

            if is_up {
                up_count += 1;
            } else {
                down_count += 1;
            }

            interfaces.push(serde_json::json!({
                "name": name,
                "up": is_up,
                "rx_bytes": rx_bytes,
                "tx_bytes": tx_bytes
            }));
        }

        let (status, message) = if up_count > 0 {
            (
                HealthStatus::Healthy,
                format!("{} interfaces up, {} down", up_count, down_count),
            )
        } else {
            (
                HealthStatus::Warning,
                "No active network interfaces".to_string(),
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

    pub fn check_systemd_services() -> Result<HealthCheckResult, HealthError> {
        let services = vec![
            "cron.service",
            "systemd-logind.service",
            "network-manager.service",
        ];

        let mut running = Vec::new();
        let mut failed = Vec::new();

        for service in services {
            let output = Command::new("systemctl")
                .args(["is-active", service])
                .output();

            if let Ok(out) = output {
                let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if status == "active" {
                    running.push(service.replace(".service", ""));
                } else if status == "failed" {
                    failed.push(service.replace(".service", ""));
                }
            }
        }

        let (status, message) = if !failed.is_empty() {
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
                "failed": failed
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

    fn determine_overall_status(checks: &[HealthCheckResult]) -> HealthStatus {
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

    // -- run_all_checks / run_check ------------------------------------------

    #[test]
    fn test_run_all_checks_returns_the_eight_checks_in_order() {
        let _g = env_guard();
        let health = HealthService::run_all_checks().expect("run_all_checks");
        let names: Vec<&str> = health.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "authentication",
                "storage",
                "memory",
                "disk_space",
                "disk_smart",
                "network",
                "services",
                "temperatures",
            ]
        );
        // Every check carries a non-empty human message.
        for c in &health.checks {
            assert!(!c.message.trim().is_empty(), "empty message for {}", c.name);
        }
        // The aggregate matches the per-check precedence.
        assert_eq!(
            health.overall_status,
            HealthService::determine_overall_status(&health.checks)
        );
        assert!(health.alerts.is_empty());
    }

    #[test]
    fn test_run_check_authentication_is_always_healthy() {
        let c = HealthService::run_check("authentication").unwrap();
        assert_eq!(c.name, "authentication");
        assert_eq!(c.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_run_check_rejects_an_unknown_name() {
        let err = HealthService::run_check("does-not-exist").unwrap_err();
        assert!(matches!(err, HealthError::Parse(_)));
    }

    #[test]
    fn test_run_check_dispatches_every_known_name() {
        let _g = env_guard();
        // `run_check`'s match arm name ("network"/"services") and the
        // resulting `HealthCheckResult.name` deliberately differ for two
        // checks — assert each arm resolves without error.
        for (arm, result_name) in [
            ("authentication", "authentication"),
            ("storage", "storage"),
            ("memory", "memory"),
            ("disk_space", "disk_space"),
            ("disk_smart", "disk_smart"),
            ("network", "network"),
            ("services", "services"),
            ("temperatures", "temperatures"),
        ] {
            let c = HealthService::run_check(arm).unwrap_or_else(|e| panic!("{arm}: {e}"));
            assert_eq!(c.name, result_name);
        }
    }

    // -- individual checks -------------------------------------------------

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

    #[test]
    fn test_check_network_interfaces_always_lists_loopback() {
        let c = HealthService::check_network_interfaces().unwrap();
        assert_eq!(c.name, "network");
        let ifaces = c.details.unwrap();
        let names: Vec<String> = ifaces["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "lo"), "no loopback in {names:?}");
    }
}
