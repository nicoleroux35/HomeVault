/* =============================================================================
 * File          : ws_discovery.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Windows network discovery (WS-Discovery) through `wsdd2`. The daemon only
 *   advertises the appliance's NetBIOS name and workgroup, so Windows Explorer
 *   lists it without the user typing a path; it exposes no share contents. The
 *   module owns the unit, its lifecycle and the status surfaced to Shares.
 *
 * Dependencies:
 *   - std::process, crate::utils::process (bounded commands)
 *
 * Usage:
 *   `ws_discovery::apply(enabled, workgroup)` from the SMB settings save and
 *   at startup; `status()` from GET /api/shares/ws-discovery.
 * =============================================================================
 */

//! Windows network discovery (WS-Discovery).

use crate::utils::process::run_timed;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// The generated unit's name and path.
pub const UNIT_NAME: &str = "homevault-wsdd";
pub const UNIT_PATH: &str = "/etc/systemd/system/homevault-wsdd.service";

/// A `systemctl` mutation can wait on the sudo/PAM stack or on the network
/// target; keep it finite.
const WS_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Read-only probes (`is-active`, `is-enabled`) are fast.
const WS_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Status shown on the Shares → SMB card.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WsDiscoveryStatus {
    pub available: bool,
    pub enabled: bool,
    pub running: bool,
    pub detail: String,
}

/// `wsdd2` binary, if this host has it. Only `wsdd2` is supported: its CLI is
/// stable and it is the Samba companion; the Python `wsdd` has a different
/// command line, and guessing it would produce a daemon that does not start.
pub fn binary_path() -> Option<&'static str> {
    ["/usr/sbin/wsdd2", "/usr/bin/wsdd2", "/sbin/wsdd2", "/bin/wsdd2"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
}

pub fn available() -> bool {
    binary_path().is_some()
}

fn unit_active(unit: &str) -> bool {
    let mut cmd = Command::new("systemctl");
    cmd.args(["is-active", "--quiet", unit]);
    run_timed(&mut cmd, WS_PROBE_TIMEOUT)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn unit_enabled(unit: &str) -> bool {
    let mut cmd = Command::new("systemctl");
    cmd.args(["is-enabled", "--quiet", unit]);
    run_timed(&mut cmd, WS_PROBE_TIMEOUT)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Current state, without changing anything.
pub fn status() -> WsDiscoveryStatus {
    let available = available();
    let enabled = unit_enabled(UNIT_NAME);
    let running = unit_active(UNIT_NAME);
    let detail = if !available {
        "wsdd2 is not installed (package: wsdd2)".to_string()
    } else if enabled && running {
        "advertising this appliance to Windows Explorer".to_string()
    } else if enabled {
        "enabled but not running; check journalctl -u homevault-wsdd".to_string()
    } else if running {
        "running but not enabled at boot".to_string()
    } else {
        "disabled".to_string()
    };
    WsDiscoveryStatus {
        available,
        enabled,
        running,
        detail,
    }
}

/// The host name Windows will see. `/proc/sys/kernel/hostname` is the source
/// of truth (Settings → General renames it); the mDNS label is only a fallback.
fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(crate::mdns::mdns_hostname)
}

/// NetBIOS names are at most 15 characters and may not contain dots or spaces.
/// Pure, so the matrix is unit-tested without touching the host.
pub fn netbios_name(raw: &str) -> String {
    let label = raw.split('.').next().unwrap_or(raw).trim();
    let cleaned: String = label
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .take(15)
        .collect();
    if cleaned.is_empty() {
        "HOMEVAULT".to_string()
    } else {
        cleaned
    }
}

/// Workgroup names share the NetBIOS limits. The SMB config validates this at
/// save time; this is the defensive copy used when the unit is rendered.
pub fn workgroup_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '-' || *character == '_'
        })
        .take(15)
        .collect();
    if cleaned.is_empty() {
        "WORKGROUP".to_string()
    } else {
        cleaned
    }
}

/// The unit body. `wsdd2` binds only 3702 (UDP multicast) and 5357 (HTTP), so
/// it never needs root; the sandbox is the same conservative set used for the
/// main unit, minus what a network daemon needs.
pub fn render_unit(binary: &str, host: &str, workgroup: &str) -> String {
    // Sanitize here too: the unit is a root-owned file, and a caller that
    // forgot the helpers must not be able to add a second directive.
    let host = netbios_name(host);
    let workgroup = workgroup_name(workgroup);
    format!(
        r#"[Unit]
Description=HomeVault Windows network discovery (WS-Discovery)
Documentation=man:wsdd2(8)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={binary} -n {host} -w {workgroup}
Restart=on-failure
RestartSec=5
DynamicUser=yes
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
RestrictNamespaces=true
LockPersonality=true

[Install]
WantedBy=multi-user.target
"#
    )
}

/// Applies the setting: installs/enables/starts the unit, or disables it and
/// removes the unit. Best-effort at call sites; returns the reason on failure
/// so the UI can say "not installed" instead of pretending it works.
pub fn apply(enabled: bool, workgroup: &str) -> Result<(), String> {
    if !enabled {
        return disable();
    }
    let Some(binary) = binary_path() else {
        return Err("wsdd2 is not installed (package: wsdd2)".to_string());
    };
    let host = netbios_name(&host_name());
    let group = workgroup_name(workgroup);
    let unit = render_unit(binary, &host, &group);
    crate::shares::write_file_as_root(UNIT_PATH, &unit)
        .map_err(|e| format!("cannot write {UNIT_PATH}: {e}"))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", UNIT_NAME])?;
    systemctl(&["restart", UNIT_NAME])?;
    Ok(())
}

fn disable() -> Result<(), String> {
    // Ignore a missing unit: disabling an already-off feature is a success.
    let _ = systemctl(&["disable", "--now", UNIT_NAME]);
    if Path::new(UNIT_PATH).exists() {
        let mut cmd = Command::new("sudo");
        cmd.args(["rm", "-f", UNIT_PATH]);
        let _ = run_timed(&mut cmd, WS_WRITE_TIMEOUT);
    }
    let _ = systemctl(&["daemon-reload"]);
    Ok(())
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("sudo");
    cmd.arg("systemctl");
    cmd.args(args);
    let output = run_timed(&mut cmd, WS_WRITE_TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_netbios_name_strips_dots_and_spaces() {
        assert_eq!(netbios_name("My NAS.local"), "MyNAS");
        assert_eq!(netbios_name("homevault"), "homevault");
    }

    #[test]
    fn test_netbios_name_truncates_to_fifteen_characters() {
        let name = netbios_name("a-very-long-host-name.example.lan");
        assert_eq!(name, "a-very-long-hos");
        assert_eq!(name.chars().count(), 15);
    }

    #[test]
    fn test_netbios_name_falls_back_when_empty() {
        assert_eq!(netbios_name(""), "HOMEVAULT");
        assert_eq!(netbios_name("..."), "HOMEVAULT");
    }

    #[test]
    fn test_workgroup_name_sanitizes_and_falls_back() {
        assert_eq!(workgroup_name("WORK GROUP"), "WORKGROUP");
        assert_eq!(workgroup_name(""), "WORKGROUP");
        assert_eq!(workgroup_name("HV"), "HV");
    }

    #[test]
    fn test_render_unit_carries_name_workgroup_and_sandbox() {
        let unit = render_unit("/usr/sbin/wsdd2", "homevault", "WORKGROUP");
        assert!(unit.contains("ExecStart=/usr/sbin/wsdd2 -n homevault -w WORKGROUP"));
        assert!(unit.contains("DynamicUser=yes"));
        assert!(unit.contains("NoNewPrivileges=true"));
        assert!(unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn test_render_unit_cannot_be_broken_by_a_newline() {
        // `render_unit` sanitizes its inputs, so a caller that forgot the
        // helpers still cannot add a second directive.
        let unit = render_unit(
            "/usr/sbin/wsdd2",
            "host\nExecStart=/bin/evil",
            "WORKGROUP\nExecStart=/bin/evil",
        );
        assert!(!unit.contains("ExecStart=/bin/evil"));
        assert!(unit.contains("ExecStart=/usr/sbin/wsdd2 -n hostExecStartbi -w WORKGROUPEExecS"));
    }
}
