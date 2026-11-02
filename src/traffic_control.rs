/* =============================================================================
 * File          : traffic_control.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-service bandwidth control (phase P3.2): an opt-in HTB queue on one
 *   egress interface with one class per protocol server, an optional weekly
 *   schedule and a pause-all switch. Validation, command generation and the
 *   schedule arithmetic are pure; only `apply` touches the host, through
 *   `sudo tc` with a bounded deadline.
 *
 * Dependencies:
 *   - serde (>= 1.0), chrono (>= 0.4), crate::utils::process
 *
 * Usage:
 *   handlers::traffic_control exposes the endpoints; web::run spawns
 *   `run_scheduler`; see docs/guides/traffic-control.md.
 * =============================================================================
 */

//! Per-service bandwidth shaping (see docs/guides/traffic-control.md).
//!
//! HomeVault shapes **egress** on the interface the admin selects. That is the
//! direction the appliance controls: an `smbclient get` (the acceptance case)
//! leaves the NAS through this queue. Management traffic (SSH/SFTP, the web
//! UI, the per-app proxy) is never placed in a limited class — an admin must
//! not be able to throttle their own session out of the appliance.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::{DateTime, Datelike, Timelike, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::storage::json::JsonStorage;
use crate::utils::process::run_timed;

/// Storage key → `{DATA_DIRECTORY}/data/traffic_control.json`.
const STORE_KEY: &str = "traffic_control";

/// HTB root handle. One fixed handle keeps a re-apply idempotent.
pub const ROOT_HANDLE: &str = "1:";

/// The class unshaped traffic falls into (the HTB default class).
pub const DEFAULT_CLASS_ID: u32 = 30;

/// The first class id handed to a limited service.
const FIRST_SERVICE_CLASS_ID: u32 = 100;

/// Upper bound on a per-service rate, in kbit/s (10 Gbit/s).
pub const MAX_RATE_KBPS: i64 = 10_000_000;

/// Upper bound on configured schedule windows.
pub const MAX_WINDOWS: usize = 8;

/// Deadline for one `tc` invocation.
pub const TC_TIMEOUT: Duration = Duration::from_secs(10);

/// Ports that must never be shaped, whatever a service rule says. Shaping the
/// admin's own SSH/HTTPS session is a lock-out, not a policy.
pub const MANAGEMENT_PORTS: &[u16] = &[22, 80, 443, 8443];

/// One protocol server HomeVault can limit, with the TCP ports it listens on.
///
/// The ports are what the `u32` filter matches on; a server that also has a
/// UDP side (NFS) is matched on its TCP port, which is where the bulk of a
/// home transfer happens.
pub const SERVICES: &[(&str, &[u16])] = &[
    ("smb", &[445, 139]),
    ("nfs", &[2049]),
    ("ftp", &[21, 20]),
    ("webdav", &[8081]),
    ("sftp", &[2222]),
];

/// Whether `service` is one the appliance knows how to shape.
pub fn is_known_service(service: &str) -> bool {
    SERVICES.iter().any(|(name, _)| *name == service)
}

/// The ports a known service listens on.
pub fn service_ports(service: &str) -> &'static [u16] {
    SERVICES
        .iter()
        .find(|(name, _)| *name == service)
        .map(|(_, ports)| *ports)
        .unwrap_or(&[])
}

/// One service limit. A rate of `0` means "unlimited for this service".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceRate {
    pub service: String,
    pub rate_kbps: i64,
}

/// A weekly window. `days` holds 0 = Monday … 6 = Sunday; `start_minute` and
/// `end_minute` are minutes after midnight. A window whose end is less than or
/// equal to its start wraps over midnight (`22:00 → 06:00`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleWindow {
    pub days: Vec<u8>,
    pub start_minute: u16,
    pub end_minute: u16,
}

impl ScheduleWindow {
    /// Whether this window covers `now` (UTC, as the scheduler evaluates it).
    pub fn covers(&self, now: DateTime<Utc>) -> bool {
        let minute = now.hour() as u16 * 60 + now.minute() as u16;
        let weekday = now.weekday().num_days_from_monday() as u8;
        let wraps = self.end_minute <= self.start_minute;
        if !wraps {
            return self.days.contains(&weekday)
                && minute >= self.start_minute
                && minute < self.end_minute;
        }
        // A wrapping window owns the tail of its own day and the head of the
        // next one; `(weekday + 6) % 7` asks whether the previous day starts
        // the window, so Sunday 22:00 → Monday 06:00 works.
        let tail = self.days.contains(&weekday) && minute >= self.start_minute;
        let head = self.days.contains(&((weekday + 6) % 7)) && minute < self.end_minute;
        tail || head
    }
}

/// The whole traffic-control policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrafficControlSettings {
    pub enabled: bool,
    /// Interface the queue is attached to; empty when never configured.
    pub interface: String,
    /// Pause-all: the policy is kept but nothing is shaped right now.
    pub paused: bool,
    pub services: Vec<ServiceRate>,
    pub schedule: Vec<ScheduleWindow>,
    pub updated_at: String,
}

impl TrafficControlSettings {
    /// Effective limits, ignoring unlimited (`0`) entries.
    pub fn limited_services(&self) -> Vec<&ServiceRate> {
        self.services
            .iter()
            .filter(|s| s.rate_kbps > 0 && is_known_service(&s.service))
            .collect()
    }

    /// Whether the schedule permits shaping at `now`. No window means "always".
    pub fn schedule_active(&self, now: DateTime<Utc>) -> bool {
        self.schedule.is_empty() || self.schedule.iter().any(|w| w.covers(now))
    }

    /// Whether the appliance should currently shape.
    pub fn should_shape(&self, now: DateTime<Utc>) -> bool {
        self.enabled
            && !self.paused
            && !self.interface.trim().is_empty()
            && !self.limited_services().is_empty()
            && self.schedule_active(now)
    }

    /// A machine-readable reason code, so the UI can localise it instead of
    /// echoing an English sentence. `None` when shaping is active.
    pub fn inactive_reason_code(&self, now: DateTime<Utc>) -> Option<&'static str> {
        if !self.enabled {
            Some("off")
        } else if self.paused {
            Some("paused")
        } else if self.interface.trim().is_empty() {
            Some("no_interface")
        } else if self.limited_services().is_empty() {
            Some("all_unlimited")
        } else if !self.schedule_active(now) {
            Some("outside_schedule")
        } else {
            None
        }
    }

    /// Why shaping is off, in a sentence for the event log. `None` when on.
    pub fn inactive_reason(&self, now: DateTime<Utc>) -> Option<&'static str> {
        match self.inactive_reason_code(now) {
            Some("off") => Some("Traffic control is off"),
            Some("paused") => Some("All transfers are paused"),
            Some("no_interface") => Some("No interface is selected"),
            Some("all_unlimited") => Some("Every service is unlimited"),
            Some("outside_schedule") => Some("Outside the configured schedule"),
            _ => None,
        }
    }
}

/// Validation errors are returned as plain sentences (the API contract the
/// other cards follow).
pub fn validate(settings: &TrafficControlSettings) -> Result<(), String> {
    let iface = settings.interface.trim();
    if settings.enabled || !iface.is_empty() {
        if !valid_interface_name(iface) {
            return Err(format!("'{iface}' is not a valid interface name"));
        }
        if iface == "lo" {
            return Err("The loopback interface cannot be shaped".to_string());
        }
    }
    if settings.services.len() > SERVICES.len() {
        return Err("Too many service limits".to_string());
    }
    let mut seen = Vec::new();
    for service in &settings.services {
        if !is_known_service(&service.service) {
            return Err(format!("Unknown service '{}'", service.service));
        }
        if seen.contains(&service.service) {
            return Err(format!("Service '{}' is listed twice", service.service));
        }
        seen.push(service.service.clone());
        if !(0..=MAX_RATE_KBPS).contains(&service.rate_kbps) {
            return Err(format!(
                "The limit for '{}' must be between 0 and {} kbit/s",
                service.service, MAX_RATE_KBPS
            ));
        }
    }
    if settings.schedule.len() > MAX_WINDOWS {
        return Err(format!(
            "At most {MAX_WINDOWS} schedule windows are allowed"
        ));
    }
    for window in &settings.schedule {
        if window.days.is_empty() {
            return Err("A schedule window needs at least one day".to_string());
        }
        if window.days.iter().any(|d| *d > 6) {
            return Err("A schedule window day must be 0 (Monday) to 6".to_string());
        }
        if window.days.len() > 7 {
            return Err("A schedule window can name at most seven days".to_string());
        }
        if window.start_minute >= 1440 || window.end_minute >= 1440 {
            return Err("A schedule time must be between 00:00 and 23:59".to_string());
        }
        if window.start_minute == window.end_minute {
            return Err("A schedule window cannot start and end at the same time".to_string());
        }
    }
    Ok(())
}

/// Interface-name gate. Mirrors the kernel's own `IFNAMSIZ` limit and refuses
/// anything that could be read as a `tc` argument or a path.
pub fn valid_interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'))
}

/// One argv-vector command. Never a shell string: `tc` is invoked directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub argv: Vec<String>,
}

impl Command {
    fn new(parts: &[&str]) -> Self {
        Self {
            argv: parts.iter().map(|p| p.to_string()).collect(),
        }
    }

    /// Rendering for the UI / the event log.
    pub fn describe(&self) -> String {
        self.argv.join(" ")
    }

    fn with_sudo(self) -> Self {
        let mut argv = vec!["sudo".to_string()];
        argv.extend(self.argv);
        Self { argv }
    }
}

/// The commands that install the current policy. `None` means "remove any
/// queue from this interface".
pub fn build_commands(settings: &TrafficControlSettings) -> Vec<Command> {
    let iface = settings.interface.trim();
    if iface.is_empty() {
        return Vec::new();
    }
    let mut commands = Vec::new();
    // Replace, don't add: applying twice must not stack qdiscs.
    commands.push(
        Command::new(&[
            "tc",
            "qdisc",
            "replace",
            "dev",
            iface,
            "root",
            "handle",
            ROOT_HANDLE,
            "htb",
            "default",
            "30",
        ])
        .with_sudo(),
    );
    let limited = settings.limited_services();
    let total: i64 = limited.iter().map(|s| s.rate_kbps).sum();
    // The root class must be able to lend its full rate to the children.
    commands.push(
        Command::new(&[
            "tc",
            "class",
            "replace",
            "dev",
            iface,
            "parent",
            "1:",
            "classid",
            "1:1",
            "htb",
            "rate",
            &format!("{total}kbit"),
            "ceil",
            &format!("{total}kbit"),
        ])
        .with_sudo(),
    );
    // The default class is deliberately generous: unlisted traffic, and every
    // management port, must not be shaped by a service rule it does not belong
    // to. It is still bounded by the parent's rate.
    commands.push(
        Command::new(&[
            "tc",
            "class",
            "replace",
            "dev",
            iface,
            "parent",
            "1:",
            "classid",
            &format!("1:{DEFAULT_CLASS_ID}"),
            "htb",
            "rate",
            &format!("{total}kbit"),
            "ceil",
            &format!("{total}kbit"),
        ])
        .with_sudo(),
    );
    for (index, service) in limited.iter().enumerate() {
        let class_id = FIRST_SERVICE_CLASS_ID + index as u32;
        commands.push(
            Command::new(&[
                "tc",
                "class",
                "replace",
                "dev",
                iface,
                "parent",
                "1:",
                "classid",
                &format!("1:{class_id}"),
                "htb",
                "rate",
                &format!("{}kbit", service.rate_kbps),
                "ceil",
                &format!("{}kbit", service.rate_kbps),
            ])
            .with_sudo(),
        );
        for port in service_ports(&service.service) {
            if MANAGEMENT_PORTS.contains(port) {
                continue;
            }
            commands.push(
                Command::new(&[
                    "tc",
                    "filter",
                    "replace",
                    "dev",
                    iface,
                    "protocol",
                    "ip",
                    "parent",
                    "1:",
                    "prio",
                    "1",
                    "u32",
                    "match",
                    "ip",
                    "sport",
                    &port.to_string(),
                    "0xffff",
                    "flowid",
                    &format!("1:{class_id}"),
                ])
                .with_sudo(),
            );
        }
    }
    commands
}

/// The commands that tear the queue down. `tc qdisc del` exits non-zero when
/// there is no queue; that is not an error for us.
pub fn teardown_command(interface: &str) -> Option<Command> {
    if !valid_interface_name(interface) {
        return None;
    }
    Some(Command::new(&["tc", "qdisc", "del", "dev", interface, "root"]).with_sudo())
}

/// Capability evidence for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct TrafficControlCapabilities {
    pub tc_available: bool,
    pub ifb_available: bool,
    pub services: Vec<ServiceCapability>,
    pub management_ports: Vec<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceCapability {
    pub id: &'static str,
    pub ports: Vec<u16>,
    pub supports_upload_limit: bool,
    pub supports_download_limit: bool,
    pub note: &'static str,
}

/// What this host can actually do. `tc` is the only hard requirement.
pub fn capabilities() -> TrafficControlCapabilities {
    let tc_available = crate::dependencies::binary_available("tc", "/usr/sbin/tc");
    TrafficControlCapabilities {
        tc_available,
        ifb_available: std::path::Path::new("/sys/module/ifb").exists(),
        services: SERVICES
            .iter()
            .map(|(id, ports)| ServiceCapability {
                id,
                ports: ports.to_vec(),
                supports_upload_limit: true,
                supports_download_limit: false,
                note: "Limits traffic leaving HomeVault (uploads and downloads to clients). \
                       Ingress shaping needs an IFB device and is not applied.",
            })
            .collect(),
        management_ports: MANAGEMENT_PORTS.to_vec(),
    }
}

/// The outcome of one apply pass.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyReport {
    pub applied: bool,
    pub reason: Option<String>,
    pub commands: Vec<String>,
    pub failed: Option<String>,
}

fn run_command(command: &Command) -> Result<(), String> {
    let mut cmd = std::process::Command::new(&command.argv[0]);
    cmd.args(&command.argv[1..]);
    match run_timed(&mut cmd, TC_TIMEOUT) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "`{}` failed: {}",
            command.describe(),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(e) => Err(format!("`{}` did not finish: {e}", command.describe())),
    }
}

/// Apply the policy to the host. Blocking; call it on the blocking pool.
///
/// On a host without `tc`, or with the feature off, the teardown still runs so
/// disabling the feature actually removes a queue a previous run installed.
pub fn apply_sync(settings: &TrafficControlSettings, now: DateTime<Utc>) -> ApplyReport {
    let iface = settings.interface.trim();
    if iface.is_empty() {
        return ApplyReport {
            applied: false,
            reason: Some("No interface is selected".to_string()),
            commands: Vec::new(),
            failed: None,
        };
    }
    if !crate::dependencies::binary_available("tc", "/usr/sbin/tc") {
        return ApplyReport {
            applied: false,
            reason: Some("The `tc` tool is not installed".to_string()),
            commands: Vec::new(),
            failed: None,
        };
    }
    if !settings.should_shape(now) {
        let commands = teardown_command(iface).into_iter().collect::<Vec<_>>();
        // A teardown that fails because there is no queue is the normal case.
        for command in &commands {
            let _ = run_command(command);
        }
        return ApplyReport {
            applied: false,
            reason: settings
                .inactive_reason(now)
                .map(str::to_string)
                .or_else(|| Some("Nothing to shape".to_string())),
            commands: commands.iter().map(Command::describe).collect(),
            failed: None,
        };
    }
    let mut executed = Vec::new();
    for command in build_commands(settings) {
        executed.push(command.describe());
        if let Err(e) = run_command(&command) {
            return ApplyReport {
                applied: false,
                reason: Some("The policy could not be applied".to_string()),
                commands: executed,
                failed: Some(e),
            };
        }
    }
    ApplyReport {
        applied: true,
        reason: None,
        commands: executed,
        failed: None,
    }
}

/// Load the saved policy, or the default.
pub fn load() -> TrafficControlSettings {
    match JsonStorage::new(data_directory()).load::<TrafficControlSettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => TrafficControlSettings::default(),
        Err(e) => {
            tracing::warn!("could not read the traffic-control settings: {e}");
            TrafficControlSettings::default()
        }
    }
}

/// Persist the policy (atomic write).
pub fn save(settings: &TrafficControlSettings) -> Result<(), String> {
    JsonStorage::new(data_directory())
        .save(STORE_KEY, settings)
        .map_err(|e| format!("could not save the traffic-control settings: {e}"))
}

fn data_directory() -> String {
    std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string())
}

/// Signature of the last applied state, so the scheduler only re-applies when
/// the *effective* policy changes (schedule boundaries included).
static LAST_APPLIED: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

fn signature(settings: &TrafficControlSettings, shaping: bool) -> String {
    format!(
        "{}|{}|{}|{}",
        settings.interface.trim(),
        shaping,
        settings
            .limited_services()
            .iter()
            .map(|s| format!("{}={}", s.service, s.rate_kbps))
            .collect::<Vec<_>>()
            .join(","),
        settings.schedule.len()
    )
}

/// Apply the policy if the effective state differs from the last pass — the
/// entry point the background scheduler and the API share, so the schedule is
/// honoured without a request.
pub async fn apply_if_changed(settings: TrafficControlSettings) -> Option<ApplyReport> {
    let now = Utc::now();
    let shaping = settings.should_shape(now);
    let current = signature(&settings, shaping);
    {
        let last = LAST_APPLIED.lock();
        if *last == current {
            return None;
        }
    }
    let report = tokio::task::spawn_blocking(move || apply_sync(&settings, now))
        .await
        .unwrap_or_else(|e| ApplyReport {
            applied: false,
            reason: Some("The traffic-control worker failed".to_string()),
            commands: Vec::new(),
            failed: Some(e.to_string()),
        });
    if report.failed.is_none() {
        *LAST_APPLIED.lock() = current;
    }
    Some(report)
}

/// Force the next pass to re-apply (used after a config save).
pub fn invalidate() {
    *LAST_APPLIED.lock() = String::new();
}

/// Poll interval: a schedule is minute-granular, so 60 seconds is the natural
/// ceiling and cheap (one file read, no command when nothing changed).
pub const SCHEDULER_TICK: Duration = Duration::from_secs(60);

/// The schedule loop spawned by `web::run`.
pub async fn run_scheduler() {
    let tick = Arc::new(AtomicU64::new(0));
    loop {
        let settings = load();
        if settings.enabled {
            let _ = apply_if_changed(settings).await;
        }
        tick.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(SCHEDULER_TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        // 2026-11-02 is a Monday.
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 11, 2 + (day - 1), hour, minute, 0).unwrap()
    }

    fn settings() -> TrafficControlSettings {
        TrafficControlSettings {
            enabled: true,
            interface: "eth0".to_string(),
            paused: false,
            services: vec![ServiceRate {
                service: "smb".to_string(),
                rate_kbps: 10_000,
            }],
            schedule: Vec::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn test_validate_accepts_a_normal_policy() {
        assert!(validate(&settings()).is_ok());
    }

    #[test]
    fn test_validate_refuses_bad_interface_names() {
        for name in [
            "",
            "lo",
            "eth0; reboot",
            "eth 0",
            "this-name-is-far-too-long",
        ] {
            let mut s = settings();
            s.interface = name.to_string();
            assert!(validate(&s).is_err(), "'{name}' should be refused");
        }
    }

    #[test]
    fn test_validate_refuses_unknown_and_duplicate_services() {
        let mut s = settings();
        s.services = vec![ServiceRate {
            service: "telnet".to_string(),
            rate_kbps: 100,
        }];
        assert!(validate(&s).is_err());

        let mut s = settings();
        s.services = vec![
            ServiceRate {
                service: "smb".to_string(),
                rate_kbps: 100,
            },
            ServiceRate {
                service: "smb".to_string(),
                rate_kbps: 200,
            },
        ];
        assert!(validate(&s).is_err());
    }

    #[test]
    fn test_validate_refuses_out_of_range_rates() {
        for rate in [-1, MAX_RATE_KBPS + 1] {
            let mut s = settings();
            s.services[0].rate_kbps = rate;
            assert!(validate(&s).is_err(), "{rate} should be refused");
        }
        let mut s = settings();
        s.services[0].rate_kbps = 0;
        assert!(validate(&s).is_ok(), "0 means unlimited and is valid");
    }

    #[test]
    fn test_validate_refuses_bad_windows() {
        let mut s = settings();
        s.schedule = vec![ScheduleWindow {
            days: vec![],
            start_minute: 60,
            end_minute: 120,
        }];
        assert!(validate(&s).is_err());

        let mut s = settings();
        s.schedule = vec![ScheduleWindow {
            days: vec![7],
            start_minute: 60,
            end_minute: 120,
        }];
        assert!(validate(&s).is_err());

        let mut s = settings();
        s.schedule = vec![ScheduleWindow {
            days: vec![1],
            start_minute: 120,
            end_minute: 120,
        }];
        assert!(validate(&s).is_err());

        let mut s = settings();
        s.schedule = (0..=MAX_WINDOWS)
            .map(|_| ScheduleWindow {
                days: vec![1],
                start_minute: 0,
                end_minute: 60,
            })
            .collect();
        assert!(validate(&s).is_err());
    }

    #[test]
    fn test_window_covers_plain_daytime() {
        let w = ScheduleWindow {
            days: vec![0, 1, 2, 3, 4],
            start_minute: 9 * 60,
            end_minute: 17 * 60,
        };
        assert!(w.covers(at(1, 9, 0)));
        assert!(w.covers(at(1, 16, 59)));
        assert!(!w.covers(at(1, 17, 0)));
        assert!(!w.covers(at(6, 10, 0)), "Saturday is not in the list");
    }

    #[test]
    fn test_window_covers_across_midnight_and_week_boundary() {
        // Sunday (6) 22:00 → 06:00.
        let w = ScheduleWindow {
            days: vec![6],
            start_minute: 22 * 60,
            end_minute: 6 * 60,
        };
        assert!(w.covers(at(7, 23, 0)), "Sunday 23:00");
        assert!(
            w.covers(at(1, 5, 59)),
            "Monday 05:59 is the head of Sunday's window"
        );
        assert!(!w.covers(at(1, 6, 0)));
        assert!(!w.covers(at(7, 21, 59)));
    }

    #[test]
    fn test_should_shape_honours_pause_and_schedule() {
        let mut s = settings();
        assert!(s.should_shape(at(1, 12, 0)));
        s.paused = true;
        assert!(!s.should_shape(at(1, 12, 0)));
        assert_eq!(
            s.inactive_reason(at(1, 12, 0)),
            Some("All transfers are paused")
        );

        let mut s = settings();
        s.schedule = vec![ScheduleWindow {
            days: vec![0],
            start_minute: 0,
            end_minute: 60,
        }];
        assert!(s.should_shape(at(1, 0, 30)));
        assert!(!s.should_shape(at(1, 2, 0)));
        assert_eq!(
            s.inactive_reason(at(1, 2, 0)),
            Some("Outside the configured schedule")
        );
    }

    #[test]
    fn test_inactive_reason_code_tracks_each_gate() {
        let mut s = settings();
        assert_eq!(s.inactive_reason_code(at(1, 12, 0)), None);

        s.enabled = false;
        assert_eq!(s.inactive_reason_code(at(1, 12, 0)), Some("off"));

        s.enabled = true;
        s.paused = true;
        assert_eq!(s.inactive_reason_code(at(1, 12, 0)), Some("paused"));

        s.paused = false;
        s.interface = "  ".to_string();
        assert_eq!(s.inactive_reason_code(at(1, 12, 0)), Some("no_interface"));

        s.interface = "eth0".to_string();
        s.services[0].rate_kbps = 0;
        assert_eq!(s.inactive_reason_code(at(1, 12, 0)), Some("all_unlimited"));

        s.services[0].rate_kbps = 500;
        s.schedule = vec![ScheduleWindow {
            days: vec![0],
            start_minute: 0,
            end_minute: 60,
        }];
        assert_eq!(
            s.inactive_reason_code(at(1, 2, 0)),
            Some("outside_schedule")
        );
        assert_eq!(s.inactive_reason_code(at(1, 0, 30)), None);
    }

    #[test]
    fn test_build_commands_matches_the_expected_pipeline() {
        let settings = settings();
        let commands = build_commands(&settings);
        let rendered: Vec<String> = commands.iter().map(Command::describe).collect();
        assert_eq!(
            rendered[0],
            "sudo tc qdisc replace dev eth0 root handle 1: htb default 30"
        );
        assert!(rendered.contains(
            &"sudo tc class replace dev eth0 parent 1: classid 1:100 htb rate 10000kbit ceil 10000kbit".to_string()
        ));
        assert!(rendered.contains(
            &"sudo tc filter replace dev eth0 protocol ip parent 1: prio 1 u32 match ip sport 445 0xffff flowid 1:100".to_string()
        ));
        assert!(rendered.contains(
            &"sudo tc filter replace dev eth0 protocol ip parent 1: prio 1 u32 match ip sport 139 0xffff flowid 1:100".to_string()
        ));
    }

    #[test]
    fn test_build_commands_never_shapes_a_management_port() {
        // Every known service's ports pass the management filter, and none of
        // the emitted filters uses a management port.
        for (service, _) in SERVICES {
            let mut s = settings();
            s.services = vec![ServiceRate {
                service: (*service).to_string(),
                rate_kbps: 500,
            }];
            for command in build_commands(&s) {
                let rendered = command.describe();
                if rendered.contains(" match ip sport ") {
                    for port in MANAGEMENT_PORTS {
                        assert!(
                            !rendered.contains(&format!("sport {port} ")),
                            "{rendered} shapes management port {port}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_unlimited_service_emits_no_class() {
        let mut s = settings();
        s.services.push(ServiceRate {
            service: "nfs".to_string(),
            rate_kbps: 0,
        });
        let rendered: Vec<String> = build_commands(&s).iter().map(Command::describe).collect();
        assert!(!rendered.iter().any(|c| c.contains("classid 1:101")));
        assert!(!rendered.iter().any(|c| c.contains("sport 2049")));
    }

    #[test]
    fn test_teardown_refuses_an_invalid_interface() {
        assert!(teardown_command("lo; rm -rf /").is_none());
        assert!(teardown_command("lo").is_some());
    }

    #[test]
    fn test_capabilities_lists_every_service_once() {
        let caps = capabilities();
        assert_eq!(caps.services.len(), SERVICES.len());
        for service in &caps.services {
            assert!(is_known_service(service.id));
            assert!(service.supports_upload_limit);
            // The honest claim: ingress is not shaped.
            assert!(!service.supports_download_limit);
        }
    }

    #[test]
    fn test_default_policy_shapes_nothing() {
        let s = TrafficControlSettings::default();
        assert!(!s.should_shape(at(1, 12, 0)));
        assert_eq!(
            s.inactive_reason(at(1, 12, 0)),
            Some("Traffic control is off")
        );
        assert!(build_commands(&s).is_empty());
    }
}
