/* =============================================================================
 * File          : disk_health.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Disk-health history: sample the watched S.M.A.R.T. attributes, compare
 *   them with the previous sample, turn a worsening trend into a deduplicated
 *   finding, and also turn kernel I/O errors / btrfs / zpool / mdadm mismatch
 *   counters into findings. Findings are persisted and notified once when
 *   they first appear.
 *
 * Dependencies:
 *   - tokio, chrono, crate::disk, crate::smart, crate::storage
 *
 * Usage:
 *   `sample_once(&state)` from the daily scheduler and the admin endpoint;
 *   see docs/guides/health.md → "Disk health history".
 * =============================================================================
 */

//! S.M.A.R.T. history, trend thresholds and I/O-error findings.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

use crate::disk::DiskService;
use crate::handlers::AppState;
use crate::storage::disk_health_repo::{DiskHealthFinding, DiskHealthRepository};
use crate::utils::process::{run_timed_async_result, SMART_PROBE_TIMEOUT};

/// Attributes worth a history: reallocated/pending/uncorrectable sectors,
/// command timeouts, UDMA CRC errors, SSD wear, temperature and power-on
/// hours.
pub const WATCHED_ATTRS: &[(u8, &str)] = &[
    (5, "Reallocated_Sector_Ct"),
    (9, "Power_On_Hours"),
    (177, "Wear_Leveling_Count"),
    (187, "Reported_Uncorrect"),
    (188, "Command_Timeout"),
    (190, "Airflow_Temperature_Cel"),
    (194, "Temperature_Celsius"),
    (197, "Current_Pending_Sector"),
    (198, "Offline_Uncorrectable"),
    (199, "UDMA_CRC_Error_Count"),
    (231, "SSD_Life_Left"),
];

const IO_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Temperature is the one attribute where a single hot sample is noise: a
/// short burst of load can raise a drive above 55 °C and cool it again. The
/// alert requires three consecutive samples (this one plus the two before).
pub const TEMPERATURE_THRESHOLD_C: u64 = 55;
pub const CONSECUTIVE_TEMPERATURE_SAMPLES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub raw: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub fingerprint: String,
    pub kind: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SampleReport {
    pub sampled_disks: usize,
    pub sampled_attributes: usize,
    pub findings: usize,
    pub errors: Vec<String>,
}

/// Parse `smartctl -A` rows. The TYPE column (`Pre-fail`/`Old_age`) anchors
/// the columns, so a two-token attribute name or a multi-token RAW_VALUE
/// (`35 (Min/Max 20/45)`) does not shift the value.
pub fn parse_smart_attributes(output: &str) -> Vec<SmartAttribute> {
    let mut attrs = Vec::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 10 {
            continue;
        }
        let Ok(id) = parts[0].parse::<u8>() else {
            continue;
        };
        let Some(type_idx) = parts
            .iter()
            .position(|token| *token == "Pre-fail" || *token == "Old_age")
        else {
            continue;
        };
        if type_idx < 5 {
            continue;
        }
        let name = parts[1..type_idx - 4].join(" ");
        if name.is_empty() {
            continue;
        }
        let raw = parts
            .get(type_idx + 3..)
            .and_then(|rest| rest.iter().find_map(|token| token.parse::<u64>().ok()))
            .unwrap_or(0);
        attrs.push(SmartAttribute { id, name, raw });
    }
    attrs
}

/// `smartctl -H`: `Some(true)` on PASSED/OK, `Some(false)` on FAILED.
pub fn parse_health(output: &str) -> Option<bool> {
    if output.contains("FAILED") {
        Some(false)
    } else if output.contains("PASSED") || output.contains("OK") {
        Some(true)
    } else {
        None
    }
}

/// Turn one sample into zero or more findings, compared with the previous
/// value. A value that was already over the threshold does not re-alert.
pub fn analyze_attribute(
    serial: &str,
    attr_id: u8,
    raw: u64,
    previous: Option<u64>,
) -> Vec<Finding> {
    let make = |kind: &str, severity: &str, message: String| Finding {
        fingerprint: format!("{serial}:{attr_id}:{kind}"),
        kind: kind.to_string(),
        severity: severity.to_string(),
        message,
    };
    let label = WATCHED_ATTRS
        .iter()
        .find(|(id, _)| *id == attr_id)
        .map(|(_, name)| *name)
        .unwrap_or("SMART attribute");

    match attr_id {
        5 => match previous {
            Some(prev) if raw >= prev.saturating_add(5) => vec![make(
                "reallocated",
                "error",
                format!(
                    "Disk {serial}: {label} rose from {prev} to {raw} (possible failing surface)"
                ),
            )],
            None if raw >= 5 => vec![make(
                "reallocated",
                "error",
                format!("Disk {serial}: {label} is already at {raw}"),
            )],
            _ => Vec::new(),
        },
        187 | 197 | 198 => {
            let crossed = previous.map(|prev| raw > prev).unwrap_or(raw > 0);
            if crossed {
                vec![make(
                    "uncorrectable",
                    "error",
                    format!(
                        "Disk {serial}: {label} increased to {raw} (uncorrectable or pending sectors)"
                    ),
                )]
            } else {
                Vec::new()
            }
        }
        177 | 231 => {
            if raw <= 10 {
                vec![make(
                    "wear",
                    "warning",
                    format!("Disk {serial}: {label} is down to {raw}% (SSD endurance)"),
                )]
            } else {
                Vec::new()
            }
        }
        190 | 194 => Vec::new(),
        _ => Vec::new(),
    }
}

/// Temperature alert with a three-sample streak. `previous` is newest first,
/// before the current sample is inserted.
pub fn analyze_temperature(serial: &str, attr_id: u8, raw: u64, previous: &[u64]) -> Vec<Finding> {
    if raw < TEMPERATURE_THRESHOLD_C {
        return Vec::new();
    }
    let streak = std::iter::once(raw)
        .chain(previous.iter().copied())
        .take(CONSECUTIVE_TEMPERATURE_SAMPLES)
        .collect::<Vec<_>>();
    if streak.len() < CONSECUTIVE_TEMPERATURE_SAMPLES
        || streak.iter().any(|value| *value < TEMPERATURE_THRESHOLD_C)
    {
        return Vec::new();
    }
    vec![Finding {
        fingerprint: format!("{serial}:{attr_id}:temperature"),
        kind: "temperature".to_string(),
        severity: "warning".to_string(),
        message: format!(
            "Disk {serial}: temperature has been at or above {TEMPERATURE_THRESHOLD_C} °C for {CONSECUTIVE_TEMPERATURE_SAMPLES} consecutive samples (latest {raw} °C)"
        ),
    }]
}

/// One line of kernel log that describes a storage I/O failure.
pub fn parse_io_errors(output: &str) -> Vec<String> {
    const NEEDLES: &[&str] = &[
        "i/o error",
        "buffer i/o error",
        "medium error",
        "critical medium error",
        "blk_update_request: i/o error",
        "btrfs error",
        "md/raid",
        "read error",
    ];
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            NEEDLES.iter().any(|needle| lower.contains(needle))
        })
        .map(|line| line.trim().to_string())
        .collect()
}

/// Sum the error counters printed by `btrfs device stats`.
pub fn btrfs_error_total(output: &str) -> u64 {
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("_io_errs")
                || lower.contains("corruption_errs")
                || lower.contains("generation_errs")
        })
        .filter_map(|line| {
            line.split_whitespace()
                .last()
                .and_then(|value| value.parse::<u64>().ok())
        })
        .sum()
}

/// `zpool status -x` is healthy only when it says so.
pub fn zpool_has_errors(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    if lower.contains("no pools available") || lower.contains("all pools are healthy") {
        return false;
    }
    lower.contains("errors:") && !lower.contains("no known data errors")
}

/// `mdadm --detail` mismatch counter.
pub fn parse_md_mismatch(output: &str) -> Option<u64> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Mismatch Count") {
            return rest
                .split_once(':')
                .and_then(|(_, value)| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok());
        }
        None
    })
}

/// Sample every disk once, persist the watched attributes, and raise one
/// notification per newly appearing finding.
pub async fn sample_once(state: &Arc<AppState>) -> Result<SampleReport, String> {
    let disks = DiskService::list_disks()
        .await
        .map_err(|e| format!("Could not list disks: {e}"))?;
    let repo = DiskHealthRepository::new(state.db.pool());
    let now = Utc::now().to_rfc3339();
    let mut report = SampleReport::default();

    for disk in disks {
        if disk.serial.trim().is_empty() {
            continue;
        }
        let serial = disk.serial.trim().to_string();
        let model = disk.model.trim().to_string();
        let device = disk.device.clone();

        let attrs = match smart_attributes(&device).await {
            Ok(attrs) => attrs,
            Err(e) => {
                report.errors.push(format!("{device}: {e}"));
                continue;
            }
        };
        report.sampled_disks += 1;

        for attr in attrs
            .iter()
            .filter(|attr| WATCHED_ATTRS.iter().any(|(id, _)| *id == attr.id))
        {
            let previous = repo
                .latest_raw(&serial, attr.id as i64)
                .await
                .map_err(|e| e.to_string())?
                .map(|raw| raw.max(0) as u64);

            let findings = if attr.id == 190 || attr.id == 194 {
                let recent = repo
                    .recent_raws(
                        &serial,
                        attr.id as i64,
                        (CONSECUTIVE_TEMPERATURE_SAMPLES - 1) as i64,
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|raw| raw.max(0) as u64)
                    .collect::<Vec<_>>();
                analyze_temperature(&serial, attr.id, attr.raw, &recent)
            } else {
                analyze_attribute(&serial, attr.id, attr.raw, previous)
            };

            for finding in findings {
                record_finding(state, &repo, &serial, &model, &finding, &now).await;
                report.findings += 1;
            }

            if let Err(e) = repo
                .insert_sample(&serial, &model, attr.id as i64, attr.raw as i64, None, &now)
                .await
            {
                report
                    .errors
                    .push(format!("{device}: could not store sample: {e}"));
            } else {
                report.sampled_attributes += 1;
            }
        }

        if let Ok(Some(false)) = smart_health(&device).await {
            let finding = Finding {
                fingerprint: format!("{serial}:health:failed"),
                kind: "health".to_string(),
                severity: "critical".to_string(),
                message: format!("Disk {serial} reports S.M.A.R.T. health FAILED"),
            };
            record_finding(state, &repo, &serial, &model, &finding, &now).await;
            report.findings += 1;
        }
    }

    collect_io_findings(state, &repo, &now, &mut report).await;
    Ok(report)
}

async fn smart_attributes(device: &str) -> Result<Vec<SmartAttribute>, String> {
    let mut cmd = crate::smart::tokio_command(device, &["-A"]);
    let output = run_timed_async_result(&mut cmd, SMART_PROBE_TIMEOUT)
        .await
        .map_err(|e| e.to_string())?;
    Ok(parse_smart_attributes(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

async fn smart_health(device: &str) -> Result<Option<bool>, String> {
    let mut cmd = crate::smart::tokio_command(device, &["-H"]);
    let output = run_timed_async_result(&mut cmd, SMART_PROBE_TIMEOUT)
        .await
        .map_err(|e| e.to_string())?;
    Ok(parse_health(&String::from_utf8_lossy(&output.stdout)))
}

async fn record_finding(
    state: &Arc<AppState>,
    repo: &DiskHealthRepository,
    serial: &str,
    model: &str,
    finding: &Finding,
    now: &str,
) {
    let already_active = repo.is_active(&finding.fingerprint).await.unwrap_or(false);
    if let Err(e) = repo
        .upsert_finding(
            &finding.fingerprint,
            serial,
            model,
            &finding.kind,
            &finding.severity,
            &finding.message,
            now,
        )
        .await
    {
        tracing::warn!("Could not persist disk-health finding: {e}");
        return;
    }
    if !already_active {
        crate::notification_engine::notify(
            state,
            "health.disk_smart",
            "Disk health warning",
            &finding.message,
            &finding.severity,
            "health",
            "disk_health",
        )
        .await;
    }
}

async fn collect_io_findings(
    state: &Arc<AppState>,
    repo: &DiskHealthRepository,
    now: &str,
    report: &mut SampleReport,
) {
    if let Ok(output) = run_plain(
        Command::new("journalctl")
            .args(["--no-pager", "--output=short", "--since", "24 hours ago"])
            .kill_on_drop(true),
        IO_PROBE_TIMEOUT,
    )
    .await
    {
        let errors = parse_io_errors(&output);
        if !errors.is_empty() {
            let finding = Finding {
                fingerprint: "system:journal-io-errors".to_string(),
                kind: "io_errors".to_string(),
                severity: "warning".to_string(),
                message: format!(
                    "{} storage I/O error line(s) in the last 24h; first: {}",
                    errors.len(),
                    errors[0]
                ),
            };
            record_finding(state, repo, "-", "", &finding, now).await;
            report.findings += 1;
        }
    }

    // Btrfs device counters, one finding per mounted filesystem with errors.
    if let Ok(mounts) = std::fs::read_to_string("/proc/mounts") {
        for line in mounts.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 3 || fields[2] != "btrfs" {
                continue;
            }
            let mount = fields[1];
            let mut cmd = Command::new("sudo");
            cmd.args(["btrfs", "device", "stats", mount]);
            if let Ok(output) = run_plain(&mut cmd, IO_PROBE_TIMEOUT).await {
                let total = btrfs_error_total(&output);
                if total > 0 {
                    let finding = Finding {
                        fingerprint: format!("btrfs:{mount}:device-stats"),
                        kind: "btrfs_errors".to_string(),
                        severity: "warning".to_string(),
                        message: format!(
                            "Btrfs reports {total} device error counter(s) on {mount}"
                        ),
                    };
                    record_finding(state, repo, "-", "", &finding, now).await;
                    report.findings += 1;
                }
            }
        }
    }

    // ZFS pool health.
    let mut zpool = Command::new("sudo");
    zpool.args(["zpool", "status", "-x"]);
    if let Ok(output) = run_plain(&mut zpool, IO_PROBE_TIMEOUT).await {
        if zpool_has_errors(&output) {
            let finding = Finding {
                fingerprint: "zfs:pool-status".to_string(),
                kind: "zfs_errors".to_string(),
                severity: "warning".to_string(),
                message: "At least one ZFS pool reports errors (`zpool status -x`)".to_string(),
            };
            record_finding(state, repo, "-", "", &finding, now).await;
            report.findings += 1;
        }
    }

    // mdadm mismatch counters.
    if let Ok(entries) = std::fs::read_dir("/dev") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("md") || name == "md" {
                continue;
            }
            let device = format!("/dev/{name}");
            let mut cmd = Command::new("sudo");
            cmd.args(["mdadm", "--detail", &device]);
            if let Ok(output) = run_plain(&mut cmd, IO_PROBE_TIMEOUT).await {
                if let Some(mismatch) = parse_md_mismatch(&output) {
                    if mismatch > 0 {
                        let finding = Finding {
                            fingerprint: format!("mdadm:{device}:mismatch"),
                            kind: "raid_mismatch".to_string(),
                            severity: "warning".to_string(),
                            message: format!("mdadm {device} reports {mismatch} mismatch block(s)"),
                        };
                        record_finding(state, repo, "-", "", &finding, now).await;
                        report.findings += 1;
                    }
                }
            }
        }
    }
}

async fn run_plain(cmd: &mut Command, deadline: Duration) -> Result<String, String> {
    let output = run_timed_async_result(cmd, deadline)
        .await
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Active findings, newest first, for the Health screen.
pub async fn active_findings(state: &Arc<AppState>) -> Result<Vec<DiskHealthFinding>, String> {
    DiskHealthRepository::new(state.db.pool())
        .list_active_findings()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_rows_parse_two_token_names_and_multitoken_raw_values() {
        let output = "\
ID# ATTRIBUTE_NAME          FLAG     VALUE WORST THRESH TYPE      UPDATED  WHEN_FAILED RAW_VALUE
  5 Reallocated_Sector_Ct   0x0033   100   100   010    Pre-fail  Always       -       12
197 Current_Pending_Sector  0x0012   100   100   000    Old_age   Always       -       3
194 Temperature_Celsius     0x0022   045   055   000    Old_age   Always       -       35 (Min/Max 20/45)
";
        let attrs = parse_smart_attributes(output);
        assert_eq!(attrs.len(), 3);
        assert_eq!(attrs[0].id, 5);
        assert_eq!(attrs[0].raw, 12);
        assert_eq!(attrs[1].id, 197);
        assert_eq!(attrs[1].raw, 3);
        assert_eq!(attrs[2].id, 194);
        assert_eq!(attrs[2].raw, 35);
    }

    #[test]
    fn health_is_failed_only_on_the_failed_line() {
        assert_eq!(
            parse_health("SMART overall-health self-assessment test result: PASSED"),
            Some(true)
        );
        assert_eq!(
            parse_health("SMART overall-health self-assessment test result: FAILED!"),
            Some(false)
        );
        assert_eq!(parse_health(""), None);
    }

    #[test]
    fn reallocated_sectors_need_a_five_sector_jump() {
        assert!(analyze_attribute("S1", 5, 9, Some(10)).is_empty());
        let finding = analyze_attribute("S1", 5, 15, Some(10));
        assert_eq!(finding.len(), 1);
        assert_eq!(finding[0].severity, "error");
        assert_eq!(finding[0].fingerprint, "S1:5:reallocated");
        assert!(analyze_attribute("S1", 5, 5, None).len() == 1);
    }

    #[test]
    fn pending_sectors_alert_on_the_first_nonzero_sample_and_on_increase() {
        assert_eq!(analyze_attribute("S1", 197, 1, None).len(), 1);
        assert!(analyze_attribute("S1", 197, 1, Some(1)).is_empty());
        assert_eq!(analyze_attribute("S1", 197, 2, Some(1)).len(), 1);
        assert!(analyze_attribute("S1", 197, 0, Some(0)).is_empty());
    }

    #[test]
    fn wear_and_temperature_thresholds_are_explicit() {
        assert_eq!(analyze_attribute("S1", 231, 9, Some(50)).len(), 1);
        assert!(analyze_attribute("S1", 231, 50, Some(100)).is_empty());
        // Temperature is never a one-sample alert.
        assert!(analyze_attribute("S1", 194, 56, Some(40)).is_empty());
    }

    #[test]
    fn temperature_needs_three_consecutive_hot_samples() {
        assert!(analyze_temperature("S1", 194, 56, &[54, 53]).is_empty());
        assert!(analyze_temperature("S1", 194, 56, &[55, 54]).is_empty());
        let finding = analyze_temperature("S1", 194, 56, &[55, 57]);
        assert_eq!(finding.len(), 1);
        assert_eq!(finding[0].fingerprint, "S1:194:temperature");
        assert!(analyze_temperature("S1", 194, 54, &[56, 56]).is_empty());
        // Airflow and temperature sensors keep distinct fingerprints.
        assert_eq!(
            analyze_temperature("S1", 190, 60, &[56, 57])[0].fingerprint,
            "S1:190:temperature"
        );
    }

    #[test]
    fn io_errors_are_filtered_by_storage_keywords() {
        let text = "\
kernel: blk_update_request: I/O error, dev sda, sector 42
kernel: usb 1-1: new high-speed USB device
kernel: BTRFS error (device sdb1): bad tree block
kernel: EXT4-fs (sda1): mounted filesystem
";
        let errors = parse_io_errors(text);
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("I/O error"));
        assert!(errors[1].contains("BTRFS error"));
    }

    #[test]
    fn btrfs_counters_sum_only_error_lines() {
        let text = "\
[/dev/sda].write_io_errs    0
[/dev/sda].read_io_errs     2
[/dev/sda].flush_io_errs    0
[/dev/sda].corruption_errs  1
[/dev/sda].generation_errs  0
";
        assert_eq!(btrfs_error_total(text), 3);
    }

    #[test]
    fn zpool_health_and_md_mismatch_are_parsed() {
        assert!(!zpool_has_errors("all pools are healthy\n"));
        assert!(!zpool_has_errors("no pools available\n"));
        assert!(!zpool_has_errors("errors: No known data errors\n"));
        assert!(zpool_has_errors("errors: 2 data errors\n"));

        assert_eq!(parse_md_mismatch("Mismatch Count : 0\n"), Some(0));
        assert_eq!(parse_md_mismatch("  Mismatch Count : 17\n"), Some(17));
        assert_eq!(parse_md_mismatch("State : clean\n"), None);
    }
}
