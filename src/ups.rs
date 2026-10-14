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

use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;
use tracing::{debug, warn};

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
        let output = Command::new("upsc").args(["-L"]).output();

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
        let output = Command::new("upsc")
            .args(["-L"])
            .output()
            .map_err(|e| UpsError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(UpsError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_ups_list(&output)
    }

    pub fn get_ups_status(name: &str) -> Result<UpsStatus, UpsError> {
        let output = Command::new("upsc")
            .args([name])
            .output()
            .map_err(|e| UpsError::CommandFailed(e.to_string()))?;

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
        let output = Command::new("upsc")
            .args([name])
            .output()
            .map_err(|e| UpsError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(UpsError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let output = String::from_utf8_lossy(&output.stdout);
        parse_ups_variables(&output)
    }

    pub fn get_ups_variable(name: &str, var: &str) -> Result<String, UpsError> {
        let output = Command::new("upsc")
            .args([format!("{}@{}", var, name)])
            .output()
            .map_err(|e| UpsError::CommandFailed(e.to_string()))?;

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

/// Add or update one UPS section and reload NUT.
pub fn save_config(entry: UpsConfigEntry) -> Result<(), UpsError> {
    validate_ups_entry(&entry).map_err(UpsError::ParseError)?;
    let text = std::fs::read_to_string(UPS_CONF_PATH).unwrap_or_default();
    let updated = upsert_ups_section(&text, &entry);
    crate::shares::write_file_as_root(UPS_CONF_PATH, &updated)
        .map_err(|e| UpsError::CommandFailed(e.to_string()))?;
    restart_nut();
    Ok(())
}

/// Remove one UPS section and reload NUT.
pub fn delete_config(name: &str) -> Result<(), UpsError> {
    validate_ups_name(name).map_err(UpsError::ParseError)?;
    let text = std::fs::read_to_string(UPS_CONF_PATH).unwrap_or_default();
    let updated = remove_ups_section(&text, name);
    crate::shares::write_file_as_root(UPS_CONF_PATH, &updated)
        .map_err(|e| UpsError::CommandFailed(e.to_string()))?;
    restart_nut();
    Ok(())
}

/// Regenerate the per-device driver instances from `ups.conf` and reload
/// `upsd`. Best-effort: a config write that succeeded must not be reported as
/// failed just because the service restart hiccuped.
fn restart_nut() {
    for unit in ["nut-driver-enumerator.service", "nut-server"] {
        let _ = Command::new("systemctl").args(["restart", unit]).output();
    }
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
}
