/* =============================================================================
 * File          : ups.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   UPS monitoring through the NUT client tooling: enumerates configured
 *   devices, and reads live variables (battery charge, load, runtime,
 *   status) per device, parsing the key:value output into typed snapshots.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tracing (>= 0.1)
 *
 * Usage:
 *   handlers/ups.rs (device list, status, config management).
 * =============================================================================
 */

//! UPS monitoring via the NUT client (device enumeration + live variables).

use crate::utils::process::run_timed;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, warn};

/// Deadline for a `upsc` query. A healthy local `upsd` answers in
/// milliseconds; this only has to be shorter than a client poll interval so
/// a wedged NUT socket cannot pile requests up behind it.
pub const NUT_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for one `systemctl restart <nut unit>`. `nut-driver-enumerator`
/// re-probes every configured device, so it gets more room than an `upsc`
/// read — but still a bound, because a hung systemd job otherwise keeps the
/// HTTP request (and its blocking thread) open forever.
pub const NUT_RESTART_TIMEOUT: Duration = Duration::from_secs(60);

/// The units NUT needs restarted after `/etc/nut/ups.conf` changes:
/// `nut-driver-enumerator` regenerates one driver instance per `[section]`,
/// and `nut-server` (`upsd`) must re-read the list. Kept as a constant so the
/// argv scoped in `/etc/sudoers.d/homevault` and the argv actually run cannot
/// drift apart (ROADMAP 13.2).
pub const NUT_UNITS: [&str; 2] = ["nut-driver-enumerator.service", "nut-server"];

#[derive(Error, Debug)]
pub enum UpsError {
    #[error("UPS command failed: {0}")]
    CommandFailed(String),
    #[error("UPS not available")]
    NotAvailable,
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("UPS not found: {0}")]
    NotFound(String),
    /// The caller-supplied name/variable is not a NUT identifier. This is
    /// rejected *before* any command runs: the value becomes an `upsc`
    /// argument, so an unchecked name is an argv-injection vector.
    #[error("Invalid UPS name or variable: {0}")]
    InvalidName(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsStatus {
    pub name: String,
    pub driver: String,
    pub model: String,
    pub status: String,
    pub line_voltage: f64,
    pub load_percent: f64,
    pub battery_charge_percent: f64,
    pub battery_voltage: f64,
    pub runtime_remaining: u32,
    pub is_online: bool,
    pub is_charging: bool,
    pub is_low_battery: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsInfo {
    pub name: String,
    pub description: String,
    pub master: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsVariable {
    pub name: String,
    pub value: String,
    pub description: String,
}

pub struct UpsService;

impl UpsService {
    pub fn is_available() -> bool {
        Self::check_upsc().is_ok()
    }

    fn check_upsc() -> Result<(), UpsError> {
        let output = run_timed(&mut Command::new("upsc").arg("-L"), NUT_QUERY_TIMEOUT);

        match output {
            Ok(out) => {
                if out.status.success() {
                    Ok(())
                } else {
                    Err(UpsError::NotAvailable)
                }
            }
            Err(_) => {
                // UPS is an optional integration: a host without NUT is a
                // normal, supported state (the Settings card shows "not
                // available"). `is_available()` is also polled per request,
                // so this must never be louder than debug.
                debug!("UPS not available: upsc command not found");
                Err(UpsError::NotAvailable)
            }
        }
    }

    pub fn list_ups() -> Result<Vec<UpsInfo>, UpsError> {
        let output = run_timed(&mut Command::new("upsc").arg("-L"), NUT_QUERY_TIMEOUT)
            .map_err(UpsError::CommandFailed)?;

        if !output.status.success() {
            return Err(UpsError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_ups_list(&output)
    }

    pub fn get_ups_status(name: &str) -> Result<UpsStatus, UpsError> {
        validate_ups_name(name).map_err(UpsError::InvalidName)?;
        let output = run_timed(
            &mut Command::new("upsc").args(upsc_args(name, None)),
            NUT_QUERY_TIMEOUT,
        )
        .map_err(UpsError::CommandFailed)?;

        if !output.status.success() {
            if String::from_utf8_lossy(&output.stderr).contains("Error") {
                return Err(UpsError::NotFound(name.to_string()));
            }
            return Err(UpsError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_ups_status(name, &output)
    }

    pub fn get_ups_variables(name: &str) -> Result<Vec<UpsVariable>, UpsError> {
        validate_ups_name(name).map_err(UpsError::InvalidName)?;
        let output = run_timed(
            &mut Command::new("upsc").args(upsc_args(name, None)),
            NUT_QUERY_TIMEOUT,
        )
        .map_err(UpsError::CommandFailed)?;

        if !output.status.success() {
            return Err(UpsError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_ups_variables(&output)
    }

    /// Read one NUT variable (`upsc <name> <variable>`).
    ///
    /// ROADMAP 13.2: both the name and the variable are validated before the
    /// argv is built. The old code passed `"{var}@{name}"` as a single
    /// argument, which is not `upsc`'s syntax at all — every per-variable
    /// read was a malformed target rather than a scoped query.
    pub fn get_ups_variable(name: &str, var: &str) -> Result<String, UpsError> {
        validate_ups_name(name).map_err(UpsError::InvalidName)?;
        validate_ups_variable(var).map_err(UpsError::InvalidName)?;
        let output = run_timed(
            &mut Command::new("upsc").args(upsc_args(name, Some(var))),
            NUT_QUERY_TIMEOUT,
        )
        .map_err(UpsError::CommandFailed)?;

        if !output.status.success() {
            return Err(UpsError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let value = String::from_utf8_lossy(&output.stdout);
        Ok(value.trim().to_string())
    }

    pub fn list_all_status() -> Result<Vec<UpsStatus>, UpsError> {
        let ups_list = Self::list_ups()?;
        let mut statuses = Vec::new();

        for ups in ups_list {
            match Self::get_ups_status(&ups.name) {
                Ok(status) => statuses.push(status),
                Err(UpsError::NotFound(_)) => {
                    // A configured UPS whose driver is not connected yet (no
                    // hardware plugged in, or the usbhid-ups `port = auto`
                    // driver still retrying) is the normal state of the
                    // auto-detection config — not a warning.
                    debug!(
                        "UPS {} is configured but its driver is not connected",
                        ups.name
                    );
                }
                Err(e) => {
                    warn!("Failed to get status for UPS {}: {}", ups.name, e);
                }
            }
        }

        Ok(statuses)
    }
}

fn parse_ups_list(output: &str) -> Result<Vec<UpsInfo>, UpsError> {
    let mut ups_list = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 2 {
            let name = parts[0].trim();
            let description = parts.get(1).map(|s| s.trim()).unwrap_or("");

            ups_list.push(UpsInfo {
                name: name.to_string(),
                description: description.to_string(),
                master: String::new(),
            });
        }
    }

    Ok(ups_list)
}

fn parse_ups_status(name: &str, output: &str) -> Result<UpsStatus, UpsError> {
    let mut status = UpsStatus {
        name: name.to_string(),
        driver: String::new(),
        model: String::new(),
        status: String::new(),
        line_voltage: 0.0,
        load_percent: 0.0,
        battery_charge_percent: 0.0,
        battery_voltage: 0.0,
        runtime_remaining: 0,
        is_online: false,
        is_charging: false,
        is_low_battery: false,
    };

    for line in output.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 2 {
            continue;
        }

        let key = parts[0].trim();
        let value = parts[1].trim();

        match key {
            "driver.name" | "driver" => status.driver = value.to_string(),
            "device.model" | "model" => status.model = value.to_string(),
            "ups.status" => {
                status.status = value.to_string();
                status.is_online = value.contains("OL");
                status.is_charging = value.contains("CHRG");
                status.is_low_battery = value.contains("LB") || value.contains("LOW");
            }
            "input.voltage.nominal" | "input.voltage" => {
                status.line_voltage = value.parse().unwrap_or(0.0);
            }
            "ups.load" | "ups.load.output" => {
                status.load_percent = value.parse().unwrap_or(0.0);
            }
            "battery.charge" | "battery.chargepercent" => {
                status.battery_charge_percent = value.parse().unwrap_or(0.0);
            }
            "battery.voltage" | "battery.voltage.nominal" => {
                status.battery_voltage = value.parse().unwrap_or(0.0);
            }
            "battery.runtime" | "battery.runtime.low" => {
                status.runtime_remaining = value.parse().unwrap_or(0);
            }
            _ => {}
        }
    }

    Ok(status)
}

fn parse_ups_variables(output: &str) -> Result<Vec<UpsVariable>, UpsError> {
    let mut variables = Vec::new();

    for line in output.lines() {
        let parts: Vec<&str> = line.splitn(2, ':').collect();
        if parts.len() == 2 {
            variables.push(UpsVariable {
                name: parts[0].trim().to_string(),
                value: parts[1].trim().to_string(),
                description: String::new(),
            });
        }
    }

    Ok(variables)
}

impl Default for UpsService {
    fn default() -> Self {
        Self
    }
}

/// The NUT agent configuration file — one `[section]` per UPS.
pub const UPS_CONF_PATH: &str = "/etc/nut/ups.conf";

/// NUT drivers HomeVault is willing to write into `ups.conf`.
///
/// The driver name is not a free-form command: NUT execs
/// `/lib/nut/<driver>`, so an unchecked value is a path-traversal / arbitrary
/// execution vector. A strict allow-list is the security gate — extend it
/// deliberately, never by accepting arbitrary input.
pub const ALLOWED_UPS_DRIVERS: &[&str] = &[
    "usbhid-ups",   // USB HID (the auto-detect default)
    "blazer_ser",   // serial (Megatec/Q1 protocol)
    "blazer_usb",   // USB serial
    "nutdrv_qx",    // modern serial/USB (Megatec/Q* protocol)
    "apcsmart",     // APC serial
    "apcsmart-old", // APC serial (legacy)
    "snmp-ups",     // network SNMP UPS
    "bcmxcp",       // serial
    "bestups",      // serial
    "dummy-ups",    // simulated (testing)
];

/// One `[section]` of `ups.conf`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsConfigEntry {
    pub name: String,
    pub driver: String,
    pub port: String,
    #[serde(default)]
    pub desc: String,
}

/// A section name is a NUT identifier: letters, digits, `-` and `_`, and it
/// becomes `[name]` in the config file, so anything else (brackets, spaces,
/// newlines) is rejected outright.
pub fn validate_ups_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 32 {
        return Err("UPS name must be 1–32 characters".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("UPS name may only contain letters, digits, '-' and '_'".to_string());
    }
    Ok(())
}

/// A NUT variable name is a dotted identifier (`ups.status`, `battery.charge`,
/// `driver.parameter.port`). It is passed as its own `upsc` argument, so the
/// same allow-list reasoning as `validate_ups_name` applies: no spaces, no
/// leading `-` (which `upsc` would read as an option), no metacharacters.
pub fn validate_ups_variable(var: &str) -> Result<(), String> {
    let var = var.trim();
    if var.is_empty() || var.len() > 64 {
        return Err("UPS variable must be 1–64 characters".to_string());
    }
    let valid = var.split('.').all(|part| {
        !part.is_empty()
            && !part.starts_with('-')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    });
    if !valid {
        return Err(
            "UPS variable must be a dotted identifier such as 'battery.charge'".to_string(),
        );
    }
    Ok(())
}

/// The exact `upsc` argv for one query: `upsc <name>` (all variables) or
/// `upsc <name> <variable>`. Pure, so the security-relevant shape is
/// unit-testable without a NUT daemon.
pub fn upsc_args(name: &str, var: Option<&str>) -> Vec<String> {
    let mut args = vec![name.to_string()];
    if let Some(var) = var {
        args.push(var.to_string());
    }
    args
}

/// Validate a full UPS section before it is written to `ups.conf`.
pub fn validate_ups_entry(entry: &UpsConfigEntry) -> Result<(), String> {
    validate_ups_name(&entry.name)?;
    if !ALLOWED_UPS_DRIVERS.contains(&entry.driver.as_str()) {
        return Err(format!("Unsupported UPS driver '{}'", entry.driver));
    }
    let port = entry.port.trim();
    if port.is_empty() || port.len() > 256 {
        return Err("A port or address is required".to_string());
    }
    if port.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("The port/address must not contain spaces or control characters".to_string());
    }
    if port.starts_with('/') && port.contains("..") {
        return Err("The port path must not contain '..'".to_string());
    }
    if entry.desc.len() > 128 || entry.desc.chars().any(|c| c.is_control()) {
        return Err("The description is too long or contains control characters".to_string());
    }
    Ok(())
}

/// Parse `ups.conf` into its device sections. Global directives before the
/// first `[section]` (e.g. `maxretry = 3`) are ignored.
pub fn parse_ups_conf(text: &str) -> Vec<UpsConfigEntry> {
    let mut entries: Vec<UpsConfigEntry> = Vec::new();
    let mut current: Option<UpsConfigEntry> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(prev) = current.take() {
                entries.push(prev);
            }
            current = Some(UpsConfigEntry {
                name: name.trim().to_string(),
                driver: String::new(),
                port: String::new(),
                desc: String::new(),
            });
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if let Some(entry) = current.as_mut() {
                let value = value.trim().trim_matches('"').to_string();
                match key.trim() {
                    "driver" => entry.driver = value,
                    "port" => entry.port = value,
                    "desc" => entry.desc = value,
                    _ => {}
                }
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

/// Insert or replace one `[section]` in `ups.conf`, preserving every other
/// line (global directives and the other devices).
pub fn upsert_ups_section(text: &str, entry: &UpsConfigEntry) -> String {
    // `desc` is quoted in the file; strip quotes so a value can never break out
    // of the string. Newlines/control chars are already rejected by
    // `validate_ups_entry`.
    let desc = entry.desc.replace('"', "'");
    let block = format!(
        "[{}]\n    driver = {}\n    port = {}\n    desc = \"{}\"",
        entry.name, entry.driver, entry.port, desc
    );

    let lines: Vec<&str> = text.lines().collect();
    let header = format!("[{}]", entry.name);
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    let mut replaced = false;

    while i < lines.len() {
        if lines[i].trim() == header {
            i += 1;
            // Skip the section body up to the next `[section]`.
            while i < lines.len() && !lines[i].trim_start().starts_with('[') {
                i += 1;
            }
            out.push(block.clone());
            replaced = true;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }

    if !replaced {
        if out.iter().any(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(block);
    }

    let mut result = out.join("\n");
    while result.ends_with('\n') {
        result.pop();
    }
    result.push('\n');
    result
}

/// Remove one `[section]` from `ups.conf`. Returns the text unchanged when the
/// section is absent.
pub fn remove_ups_section(text: &str, name: &str) -> String {
    let header = format!("[{name}]");
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    let mut removed = false;

    while i < lines.len() {
        if lines[i].trim() == header {
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with('[') {
                i += 1;
            }
            removed = true;
        } else {
            out.push(lines[i]);
            i += 1;
        }
    }

    if !removed {
        return text.to_string();
    }

    let mut result = out.join("\n");
    while result.ends_with('\n') {
        result.pop();
    }
    result.push('\n');
    result
}

/// List the UPS devices currently configured in `ups.conf`.
pub fn list_config() -> Result<Vec<UpsConfigEntry>, UpsError> {
    let text = std::fs::read_to_string(UPS_CONF_PATH).unwrap_or_default();
    Ok(parse_ups_conf(&text))
}

/// The auto-detection section `installer/install.sh` ships: one generic
/// `usbhid-ups` entry with `port = auto`, which attaches to whichever
/// supported USB UPS is present.
pub fn default_ups_entry() -> UpsConfigEntry {
    UpsConfigEntry {
        name: "ups".to_string(),
        driver: "usbhid-ups".to_string(),
        port: "auto".to_string(),
        desc: "HomeVault auto-detected USB UPS".to_string(),
    }
}

/// The text of `ups.conf` after "Reset to Defaults": global directives are
/// preserved verbatim, every device section HomeVault may have written is
/// dropped, and the shipped auto-detect section is restored.
///
/// Pure, so "reset restores the shipped state" is unit-testable without
/// touching `/etc/nut`.
pub fn reset_ups_conf_text(text: &str) -> String {
    let globals: Vec<&str> = text
        .lines()
        .take_while(|line| !line.trim_start().starts_with('['))
        .collect();
    let globals = globals.join("\n");
    let globals = globals.trim_end();
    upsert_ups_section(globals, &default_ups_entry())
}

/// Restore the shipped `ups.conf` state and reload NUT (ROADMAP 13.5: the
/// UPS device list previously survived a "Reset to Defaults" like every
/// other un-wired setting).
pub fn reset_config() -> Result<Vec<String>, UpsError> {
    let text = std::fs::read_to_string(UPS_CONF_PATH).unwrap_or_default();
    let cleared = reset_ups_conf_text(&text);
    crate::shares::write_file_as_root(UPS_CONF_PATH, &cleared)
        .map_err(|e| UpsError::CommandFailed(e.to_string()))?;
    Ok(restart_nut())
}

/// Add or update one UPS section and reload NUT.
///
/// The `Ok` payload lists reload failures: the config write and the service
/// reload are two different outcomes, and ROADMAP 13.2 was precisely the bug
/// of reporting only the first. An empty vector means NUT really reloaded.
pub fn save_config(entry: UpsConfigEntry) -> Result<Vec<String>, UpsError> {
    validate_ups_entry(&entry).map_err(UpsError::ParseError)?;
    let text = std::fs::read_to_string(UPS_CONF_PATH).unwrap_or_default();
    let updated = upsert_ups_section(&text, &entry);
    crate::shares::write_file_as_root(UPS_CONF_PATH, &updated)
        .map_err(|e| UpsError::CommandFailed(e.to_string()))?;
    Ok(restart_nut())
}

/// Remove one UPS section and reload NUT (see `save_config` for the `Ok`
/// payload).
pub fn delete_config(name: &str) -> Result<Vec<String>, UpsError> {
    validate_ups_name(name).map_err(UpsError::InvalidName)?;
    let text = std::fs::read_to_string(UPS_CONF_PATH).unwrap_or_default();
    let updated = remove_ups_section(&text, name);
    crate::shares::write_file_as_root(UPS_CONF_PATH, &updated)
        .map_err(|e| UpsError::CommandFailed(e.to_string()))?;
    Ok(restart_nut())
}

/// The argv used to reload one NUT unit. Pure, so the sudoers grant and the
/// runtime command can be asserted to be the same shape.
pub fn nut_restart_args(unit: &str) -> Vec<String> {
    vec![
        "systemctl".to_string(),
        "restart".to_string(),
        unit.to_string(),
    ]
}

/// Regenerate the per-device driver instances from `ups.conf` and reload
/// `upsd`. Returns one message per unit that did not reload.
///
/// ROADMAP 13.2: the previous version ran `systemctl` with no `sudo` and no
/// grant existed for either unit, so on the shipped unprivileged service
/// account every call failed silently — and the caller reported success.
pub fn restart_nut() -> Vec<String> {
    let mut failures = Vec::new();
    for unit in NUT_UNITS {
        let mut cmd = Command::new("sudo");
        cmd.args(nut_restart_args(unit));
        match run_timed(&mut cmd, NUT_RESTART_TIMEOUT) {
            Ok(out) if out.status.success() => {
                debug!("NUT unit {} restarted", unit);
            }
            Ok(out) => {
                let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
                warn!("NUT unit {} did not restart: {}", unit, detail);
                failures.push(format!(
                    "{}: {}",
                    unit,
                    if detail.is_empty() {
                        "systemctl exited with a failure status".to_string()
                    } else {
                        detail
                    }
                ));
            }
            Err(e) => {
                warn!("NUT unit {} did not restart: {}", unit, e);
                failures.push(format!("{}: {}", unit, e));
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ups_list() {
        let output = "ups@localhost: NUT zoom 600 -巡视";
        let result = parse_ups_list(output);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn test_parse_ups_status() {
        let output = r#"
driver.name: usbhid-ups
driver.parameter.port: /dev/ugen0
ups.status: OL CHRG
ups.load: 25
battery.charge: 100
battery.voltage: 27.5
"#;
        let result = parse_ups_status("test-ups", output);
        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(status.is_online);
        assert!(status.is_charging);
        assert_eq!(status.load_percent, 25.0);
    }

    #[test]
    fn test_parse_ups_conf_reads_sections_and_ignores_globals() {
        let text = "\
# comment
maxretry = 3

[ups]
    driver = usbhid-ups
    port = auto
    desc = \"Auto USB\"

[serial-ups]
    driver = blazer_ser
    port = /dev/ttyS0
";
        let entries = parse_ups_conf(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "ups");
        assert_eq!(entries[0].driver, "usbhid-ups");
        assert_eq!(entries[0].port, "auto");
        assert_eq!(entries[0].desc, "Auto USB");
        assert_eq!(entries[1].name, "serial-ups");
        assert_eq!(entries[1].driver, "blazer_ser");
        assert_eq!(entries[1].port, "/dev/ttyS0");
    }

    #[test]
    fn test_upsert_appends_a_new_section() {
        let text = "maxretry = 3\n\n[ups]\n    driver = usbhid-ups\n    port = auto\n";
        let entry = UpsConfigEntry {
            name: "snmp-ups".into(),
            driver: "snmp-ups".into(),
            port: "192.168.1.10".into(),
            desc: "Rack UPS".into(),
        };
        let out = upsert_ups_section(text, &entry);
        let parsed = parse_ups_conf(&out);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "ups");
        assert_eq!(parsed[1].name, "snmp-ups");
        assert_eq!(parsed[1].driver, "snmp-ups");
        assert_eq!(parsed[1].port, "192.168.1.10");
        // Global directive preserved.
        assert!(out.contains("maxretry = 3"));
    }

    #[test]
    fn test_upsert_replaces_an_existing_section() {
        let text = "maxretry = 3\n\n[ups]\n    driver = usbhid-ups\n    port = auto\n\n[serial]\n    driver = blazer_ser\n    port = /dev/ttyS0\n";
        let entry = UpsConfigEntry {
            name: "serial".into(),
            driver: "blazer_ser".into(),
            port: "/dev/ttyS1".into(),
            desc: "".into(),
        };
        let out = upsert_ups_section(text, &entry);
        let parsed = parse_ups_conf(&out);
        assert_eq!(parsed.len(), 2, "no duplicate section");
        let serial = parsed.iter().find(|e| e.name == "serial").unwrap();
        assert_eq!(serial.port, "/dev/ttyS1");
        // The auto USB section is untouched.
        assert!(out.contains("[ups]"));
        assert!(out.contains("port = auto"));
    }

    #[test]
    fn test_remove_ups_section() {
        let text = "maxretry = 3\n\n[ups]\n    driver = usbhid-ups\n    port = auto\n\n[serial]\n    driver = blazer_ser\n    port = /dev/ttyS0\n";
        let out = remove_ups_section(text, "serial");
        let parsed = parse_ups_conf(&out);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "ups");
        assert!(!out.contains("blazer_ser"));
        // Removing an absent section is a no-op.
        assert_eq!(remove_ups_section(text, "nope"), text);
    }

    #[test]
    fn test_validate_ups_entry_rejects_bad_input() {
        let ok = UpsConfigEntry {
            name: "rack".into(),
            driver: "snmp-ups".into(),
            port: "10.0.0.5".into(),
            desc: "Rack".into(),
        };
        assert!(validate_ups_entry(&ok).is_ok());

        let bad_name = UpsConfigEntry {
            name: "a b".into(),
            ..ok.clone()
        };
        assert!(validate_ups_entry(&bad_name).is_err());

        let bad_driver = UpsConfigEntry {
            driver: "../../bin/sh".into(),
            ..ok.clone()
        };
        assert!(validate_ups_entry(&bad_driver).is_err());

        let bad_port = UpsConfigEntry {
            port: "a b".into(),
            ..ok.clone()
        };
        assert!(validate_ups_entry(&bad_port).is_err());

        let traversal = UpsConfigEntry {
            driver: "blazer_ser".into(),
            port: "/dev/../etc/shadow".into(),
            ..ok.clone()
        };
        assert!(validate_ups_entry(&traversal).is_err());

        let empty_port = UpsConfigEntry {
            port: String::new(),
            ..ok.clone()
        };
        assert!(validate_ups_entry(&empty_port).is_err());
    }

    // -- ROADMAP 13.2: the name reaches the `upsc` argv -------------------

    #[test]
    fn test_validate_ups_name_rejects_argv_injection() {
        for bad in [
            "",
            "   ",
            "ups; reboot",
            "ups&&reboot",
            "$(reboot)",
            "../etc",
            "-L",
            "ups name",
            "ups\n",
            "[ups]",
            &"a".repeat(33),
        ] {
            assert!(
                validate_ups_name(bad).is_err(),
                "'{bad}' must not be accepted as a UPS name"
            );
        }
        for ok in ["ups", "rack_1", "serial-2", "UPS03"] {
            assert!(validate_ups_name(ok).is_ok(), "'{ok}' is a valid name");
        }
    }

    #[test]
    fn test_validate_ups_variable_is_a_dotted_identifier() {
        for ok in [
            "ups.status",
            "battery.charge",
            "driver.parameter.port",
            "input_voltage",
        ] {
            assert!(validate_ups_variable(ok).is_ok(), "'{ok}' is valid");
        }
        for bad in [
            "",
            " ups.status",
            "--help",
            "battery..charge",
            "battery.charge; reboot",
            "battery.charge\n",
            "a/b",
            &"a".repeat(65),
        ] {
            assert!(
                validate_ups_variable(bad).is_err(),
                "'{bad}' must not be accepted as a UPS variable"
            );
        }
    }

    #[test]
    fn test_upsc_argv_is_one_argument_per_field() {
        // `upsc <name>` and `upsc <name> <variable>` — never the old
        // `"{var}@{name}"` shape, which was not `upsc` syntax at all.
        assert_eq!(upsc_args("ups", None), vec!["ups".to_string()]);
        assert_eq!(
            upsc_args("ups", Some("battery.charge")),
            vec!["ups".to_string(), "battery.charge".to_string()]
        );
    }

    #[test]
    fn test_a_bad_name_is_refused_before_any_command_runs() {
        // No `upsc` exists in a test environment; the point is that the
        // error is the validation one, not "command not found".
        let err = UpsService::get_ups_status("ups; reboot").unwrap_err();
        assert!(matches!(err, UpsError::InvalidName(_)), "{err:?}");
        let err = UpsService::get_ups_variables("../ups").unwrap_err();
        assert!(matches!(err, UpsError::InvalidName(_)), "{err:?}");
        let err = UpsService::get_ups_variable("ups", "--help").unwrap_err();
        assert!(matches!(err, UpsError::InvalidName(_)), "{err:?}");
    }

    #[test]
    fn test_nut_restart_argv_matches_the_granted_units() {
        assert_eq!(
            nut_restart_args(NUT_UNITS[0]),
            vec![
                "systemctl".to_string(),
                "restart".to_string(),
                "nut-driver-enumerator.service".to_string()
            ]
        );
        assert_eq!(
            nut_restart_args(NUT_UNITS[1]),
            vec![
                "systemctl".to_string(),
                "restart".to_string(),
                "nut-server".to_string()
            ]
        );
        // The enumerator runs before `upsd` reads the list it generates.
        assert_eq!(NUT_UNITS, ["nut-driver-enumerator.service", "nut-server"]);
    }

    // -- ROADMAP 13.5: "Reset to Defaults" restores the shipped UPS state --

    #[test]
    fn test_reset_ups_conf_text_keeps_globals_and_restores_the_default() {
        let text = "\
# comment
maxretry = 3

[ups]
    driver = usbhid-ups
    port = auto

[serial]
    driver = blazer_ser
    port = /dev/ttyS0
";
        let out = reset_ups_conf_text(text);
        let parsed = parse_ups_conf(&out);
        assert_eq!(parsed.len(), 1, "only the shipped section remains");
        assert_eq!(parsed[0], default_ups_entry());
        assert!(out.contains("maxretry = 3"), "global directives survive");
        assert!(!out.contains("blazer_ser"));
        assert!(!out.contains("ttyS0"));
    }

    #[test]
    fn test_reset_ups_conf_text_on_an_empty_file_writes_the_default() {
        let out = reset_ups_conf_text("");
        let parsed = parse_ups_conf(&out);
        assert_eq!(parsed, vec![default_ups_entry()]);
    }
}
