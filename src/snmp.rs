/* =============================================================================
 * File          : snmp.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SNMP queries against monitored network devices (system info,
 *   interfaces, storage, availability) executed through the snmpget/
 *   snmpwalk CLI tools and parsed into typed results.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tracing (>= 0.1)
 *
 * Usage:
 *   handlers/snmp.rs and the UPS polling service.
 * =============================================================================
 */

//! SNMP queries against network devices via the net-snmp CLI tools.

use crate::utils::process::run_timed;
use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;
use tracing::debug;

/// Errors that can occur during SNMP operations.
#[derive(Error, Debug)]
pub enum SnmpError {
    #[error("SNMP command failed: {0}")]
    CommandFailed(String),
    #[error("SNMP not available")]
    NotAvailable,
    #[error("Parse error: {0}")]
    ParseError(String),
}

/// System information gathered via SNMP MIB-II (RFC 1213).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnmpSystemInfo {
    pub sys_name: String,
    pub sys_descr: String,
    pub sys_uptime: u64,
    pub sys_contact: String,
    pub sys_location: String,
}

/// Network interface statistics from SNMP IF-MIB.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterfaceStats {
    pub name: String,
    pub mtu: u32,
    pub ip_address: String,
    pub mac_address: String,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub packets_in: u64,
    pub packets_out: u64,
    pub errors_in: u64,
    pub errors_out: u64,
}

/// Storage filesystem statistics from SNMP HOST-RESOURCES-MIB.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageStats {
    pub fs_index: String,
    pub mount_point: String,
    pub device: String,
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub usage_percent: f64,
}

/// Service for querying SNMP agents on localhost.
pub struct SnmpService;

/// Deadline for one `snmpget`/`snmpwalk` invocation. An unreachable or wedged
/// agent retries for tens of seconds on its own; this keeps a Settings card
/// refresh bounded (ROADMAP X.2).
pub const SNMP_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// SNMP protocol versions HomeVault can speak.
///
/// ROADMAP 13.5: `run_walk` hardcoded `2c` and the saved `version` was never
/// read. v1 and v2c only need a community string, which is all the Settings
/// card collects; v3 needs a user, an auth protocol and a privacy protocol, so
/// it is refused with a message that says so rather than being silently
/// downgraded to v2c.
pub const SUPPORTED_SNMP_VERSIONS: [&str; 2] = ["1", "2c"];

/// Validate a configured SNMP version. Returns the value to pass to `-v`.
pub fn validate_snmp_version(version: &str) -> Result<String, String> {
    let version = version.trim();
    if version == "3" || version == "3u" {
        return Err(
            "SNMP v3 needs a user name and auth/privacy credentials, which HomeVault does not collect — use v1 or v2c"
                .to_string(),
        );
    }
    if SUPPORTED_SNMP_VERSIONS.contains(&version) {
        Ok(version.to_string())
    } else {
        Err(format!(
            "Unsupported SNMP version '{}'. Supported: {:?}",
            version, SUPPORTED_SNMP_VERSIONS
        ))
    }
}

/// The exact `snmpget` argv. Pure, so the version and community really reach
/// the command line and no field is ever split by a shell.
pub fn snmpget_args(version: &str, community: &str, host: &str, oid: &str) -> Vec<String> {
    vec![
        "-v".to_string(),
        version.to_string(),
        "-c".to_string(),
        community.to_string(),
        "-On".to_string(),
        host.to_string(),
        oid.to_string(),
    ]
}

/// The exact `snmpwalk` argv. Same shape as `snmpget` (`-On` numeric output
/// applies to both), named separately so a caller can see which tool it feeds.
pub fn snmpwalk_args(version: &str, community: &str, host: &str, oid: &str) -> Vec<String> {
    snmpget_args(version, community, host, oid)
}

/// Keep only the interface the caller asked for. An empty `iface` means "every
/// interface" (the Settings card renders the whole table).
///
/// ROADMAP 13.5: the parameter was accepted and ignored, so a caller could not
/// narrow the walk even though the API claimed to.
pub fn filter_interfaces(stats: Vec<InterfaceStats>, iface: &str) -> Vec<InterfaceStats> {
    let wanted = iface.trim();
    if wanted.is_empty() {
        return stats;
    }
    stats
        .into_iter()
        .filter(|s| s.name == wanted || s.name.eq_ignore_ascii_case(wanted))
        .collect()
}

impl SnmpService {
    /// Return `true` if the `snmpd` binary responds to `-v -V`.
    pub fn is_available() -> bool {
        Self::check_snmpd().is_ok()
    }

    /// Check whether the `snmpd` command is available on this system.
    fn check_snmpd() -> Result<(), SnmpError> {
        // `snmpd` (the daemon) lives under /usr/sbin, unlike the snmpget/
        // snmpwalk/snmpbulkwalk client tools below (/usr/bin, always on
        // PATH) - a bare `Command::new("snmpd")` fails to resolve it under
        // the unprivileged service account's minimal PATH even when it's
        // genuinely installed, the same /usr/sbin-not-on-PATH gap already
        // fixed for `firewall::ufw_available()` and documented in
        // `dependencies.rs::binary_present()`.
        let snmpd_bin = if std::path::Path::new("/usr/sbin/snmpd").exists() {
            "/usr/sbin/snmpd"
        } else {
            "snmpd"
        };
        let output = run_timed(
            Command::new(snmpd_bin).args(["-v", "-V"]),
            SNMP_QUERY_TIMEOUT,
        );

        match output {
            Ok(out) => {
                if out.status.success() {
                    Ok(())
                } else {
                    Err(SnmpError::NotAvailable)
                }
            }
            Err(_) => {
                // SNMP is an optional integration: a host without `snmpd`
                // is a normal, supported state (the Settings card shows
                // "not available"). `is_available()` is also polled per
                // request, so this must never be louder than debug.
                debug!("SNMP not available: snmpd command not found");
                Err(SnmpError::NotAvailable)
            }
        }
    }

    /// Query the local SNMP agent for system information (sysName, sysDescr, sysUptime, etc).
    pub fn get_system_info(community: &str, version: &str) -> Result<SnmpSystemInfo, SnmpError> {
        let version = validate_snmp_version(version).map_err(SnmpError::CommandFailed)?;
        let snmpget = |oid: &str| -> Result<String, SnmpError> {
            let output = run_timed(
                Command::new("snmpget").args(snmpget_args(&version, community, "localhost", oid)),
                SNMP_QUERY_TIMEOUT,
            )
            .map_err(SnmpError::CommandFailed)?;

            if !output.status.success() {
                return Err(SnmpError::CommandFailed(
                    String::from_utf8_lossy(&output.stderr).to_string(),
                ));
            }

            let output = String::from_utf8_lossy(&output.stdout);
            parse_snmp_string(&output)
        };

        Ok(SnmpSystemInfo {
            sys_name: snmpget("1.3.6.1.2.1.1.5.0").unwrap_or_else(|_| "Unknown".to_string()),
            sys_descr: snmpget("1.3.6.1.2.1.1.1.0").unwrap_or_else(|_| "Unknown".to_string()),
            sys_uptime: snmpget("1.3.6.1.2.1.1.3.0")
                .and_then(|s| s.parse().map_err(|_| SnmpError::ParseError(s)))
                .unwrap_or(0),
            sys_contact: snmpget("1.3.6.1.2.1.1.4.0").unwrap_or_else(|_| "Unknown".to_string()),
            sys_location: snmpget("1.3.6.1.2.1.1.6.0").unwrap_or_else(|_| "Unknown".to_string()),
        })
    }

    /// Walk IF-MIB (ifTable) + IF-MIB extensions (ifXTable) for interface
    /// statistics. The 64-bit `ifHCInOctets`/`ifHCOutOctets` counters from
    /// ifXTable are preferred over the 32-bit `ifInOctets`/`ifOutOctets`
    /// (Counter32 wraps at ~4 GiB); `ifName` is preferred over `ifDescr`.
    ///
    /// `iface` narrows the result to one interface (matched against `ifName`
    /// or `ifDescr`); an empty value keeps every interface. It is applied after
    /// the walk, since the row index is not the name.
    pub fn get_interface_stats(
        iface: &str,
        community: &str,
        version: &str,
    ) -> Result<Vec<InterfaceStats>, SnmpError> {
        let version = validate_snmp_version(version).map_err(SnmpError::CommandFailed)?;
        let mut output = run_walk("1.3.6.1.2.1.2.2.1", community, &version)?;
        // ifXTable may be absent from a locked-down `snmpd` view; that is not
        // fatal — ifTable alone still yields name + 32-bit counters.
        if let Ok(ifx) = run_walk("1.3.6.1.2.1.31.1.1.1", community, &version) {
            output.push_str(&ifx);
        }
        Ok(filter_interfaces(
            parse_interface_stats(&output),
            iface,
        ))
    }

    /// Walk HOST-RESOURCES-MIB for storage filesystem statistics.
    pub fn get_storage_stats(
        community: &str,
        version: &str,
    ) -> Result<Vec<StorageStats>, SnmpError> {
        let version = validate_snmp_version(version).map_err(SnmpError::CommandFailed)?;
        let output = run_walk("1.3.6.1.2.1.25.2.3.1", community, &version)?;
        Ok(parse_storage_stats(&output))
    }

    /// Return stats for all network interfaces via IF-MIB.
    pub fn get_network_interfaces(
        community: &str,
        version: &str,
    ) -> Result<Vec<InterfaceStats>, SnmpError> {
        Self::get_interface_stats("", community, version)
    }
}

/// Run `snmpwalk -On` (numeric OIDs, one `OID = value` per line) against the
/// local agent. `-On` is load-bearing: without the MIB files installed —
/// the default on Ubuntu/Debian — `snmpwalk` prints numeric OIDs like
/// `iso.3.6.1.2.1.2.2.1.2.1`, never the symbolic `IF-MIB::ifDescr.1` the old
/// parser matched, so interfaces/storage always came back empty. Forcing
/// numeric output makes the shape stable regardless of the host's MIB set.
fn run_walk(oid: &str, community: &str, version: &str) -> Result<String, SnmpError> {
    let output = run_timed(
        Command::new("snmpwalk").args(snmpwalk_args(version, community, "localhost", oid)),
        SNMP_QUERY_TIMEOUT,
    )
    .map_err(SnmpError::CommandFailed)?;

    if !output.status.success() {
        return Err(SnmpError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Split a `snmpwalk -On` line into `(numeric OID without leading dot, value)`.
/// Strips the leading type token (`STRING:`, `INTEGER:`, `Counter32:`,
/// `Counter64:`, `Hex-STRING:`, …) and any surrounding quotes.
fn split_oid_line(line: &str) -> Option<(&str, &str)> {
    let (oid, rest) = line.split_once(" = ")?;
    let oid = oid.trim().trim_start_matches('.');
    if oid.is_empty() {
        return None;
    }
    let value = rest.trim();
    let value = match value.find(':') {
        Some(pos) => value[pos + 1..].trim(),
        None => value,
    };
    Some((oid, value.trim_matches('"')))
}

/// For an OID under `base`, return `(column, index)` from the remaining
/// components — e.g. `1.3.6.1.2.1.2.2.1.2.3` under base
/// `1.3.6.1.2.1.2.2.1` yields `(2, 3)` (ifDescr of interface 3).
fn column_index(oid: &str, base: &str) -> Option<(u32, u64)> {
    let rest = oid.strip_prefix(base)?.strip_prefix('.')?;
    let mut parts = rest.split('.');
    let col: u32 = parts.next()?.parse().ok()?;
    let idx: u64 = parts.next()?.parse().ok()?;
    Some((col, idx))
}

/// Format an `ifPhysAddress` Hex-STRING (`64 00 6A 40 81 6C`) as
/// `64:00:6A:40:81:6C`; anything unexpected is returned trimmed/uppercased.
fn normalize_mac(raw: &str) -> String {
    let hex: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() == 12 {
        hex.as_bytes()
            .chunks(2)
            .map(|c| String::from_utf8_lossy(c).to_uppercase())
            .collect::<Vec<_>>()
            .join(":")
    } else {
        raw.trim().to_uppercase()
    }
}

fn parse_snmp_string(output: &str) -> Result<String, SnmpError> {
    let line = output
        .lines()
        .next()
        .ok_or_else(|| SnmpError::ParseError("Empty output".to_string()))?;

    if let Some(pos) = line.find("STRING:") {
        let value = line[pos + 8..].trim().trim_matches('"');
        Ok(value.to_string())
    } else if let Some(pos) = line.find(":") {
        Ok(line[pos + 1..].trim().to_string())
    } else {
        Ok(line.to_string())
    }
}

fn parse_interface_stats(output: &str) -> Vec<InterfaceStats> {
    const IF_BASE: &str = "1.3.6.1.2.1.2.2.1"; // ifTable
    const IFX_BASE: &str = "1.3.6.1.2.1.31.1.1.1"; // ifXTable

    let mut map: std::collections::BTreeMap<u64, InterfaceStats> =
        std::collections::BTreeMap::new();

    for line in output.lines() {
        let Some((oid, value)) = split_oid_line(line) else {
            continue;
        };

        if let Some((col, idx)) = column_index(oid, IF_BASE) {
            let e = map.entry(idx).or_default();
            match col {
                2 => {
                    if e.name.is_empty() {
                        e.name = value.to_string();
                    }
                }
                4 => e.mtu = value.parse().unwrap_or(0),
                6 => e.mac_address = normalize_mac(value),
                // 32-bit counters are the fallback; ifXTable below overrides.
                10 => {
                    if e.bytes_in == 0 {
                        e.bytes_in = value.parse().unwrap_or(0);
                    }
                }
                16 => {
                    if e.bytes_out == 0 {
                        e.bytes_out = value.parse().unwrap_or(0);
                    }
                }
                11 => e.packets_in = value.parse().unwrap_or(0),
                17 => e.packets_out = value.parse().unwrap_or(0),
                14 => e.errors_in = value.parse().unwrap_or(0),
                20 => e.errors_out = value.parse().unwrap_or(0),
                _ => {}
            }
        } else if let Some((col, idx)) = column_index(oid, IFX_BASE) {
            let e = map.entry(idx).or_default();
            match col {
                1 => e.name = value.to_string(), // ifName wins over ifDescr
                6 => e.bytes_in = value.parse().unwrap_or(0), // ifHCInOctets
                10 => e.bytes_out = value.parse().unwrap_or(0), // ifHCOutOctets
                _ => {}
            }
        }
    }

    map.into_values().collect()
}

fn parse_storage_stats(output: &str) -> Vec<StorageStats> {
    const BASE: &str = "1.3.6.1.2.1.25.2.3.1"; // hrStorageTable

    #[derive(Default)]
    struct Acc {
        descr: String,
        units: u64,
        size: u64,
        used: u64,
    }

    let mut map: std::collections::BTreeMap<u64, Acc> = std::collections::BTreeMap::new();

    for line in output.lines() {
        let Some((oid, value)) = split_oid_line(line) else {
            continue;
        };
        let Some((col, idx)) = column_index(oid, BASE) else {
            continue;
        };
        let e = map.entry(idx).or_default();
        match col {
            3 => e.descr = value.to_string(),          // hrStorageDescr
            4 => e.units = value.parse().unwrap_or(0), // hrStorageAllocationUnits
            5 => e.size = value.parse().unwrap_or(0),  // hrStorageSize (in units)
            6 => e.used = value.parse().unwrap_or(0),  // hrStorageUsed (in units)
            _ => {}
        }
    }

    map.into_iter()
        .map(|(idx, a)| {
            // hrStorageSize/Used are counts of hrStorageAllocationUnits, not
            // bytes — multiply to get real byte totals.
            let total = a.size.saturating_mul(a.units);
            let used = a.used.saturating_mul(a.units);
            let available = total.saturating_sub(used);
            let usage_percent = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            StorageStats {
                fs_index: idx.to_string(),
                mount_point: a.descr,
                device: String::new(),
                total,
                available,
                used,
                usage_percent,
            }
        })
        .collect()
}

impl Default for SnmpService {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interface_stats_parses_numeric_oid_walk() {
        // Real `snmpwalk -On` output from a default Ubuntu snmpd (numeric
        // OIDs — no MIB files installed). The old parser matched symbolic
        // names (`IF-MIB::ifDescr`) and returned empty on this.
        let output = "\
.1.3.6.1.2.1.2.2.1.2.1 = STRING: \"lo\"
.1.3.6.1.2.1.2.2.1.2.2 = STRING: \"enp2s0\"
.1.3.6.1.2.1.2.2.1.4.2 = INTEGER: 1500
.1.3.6.1.2.1.2.2.1.6.2 = Hex-STRING: 64 00 6A 40 81 6C
.1.3.6.1.2.1.2.2.1.10.1 = Counter32: 1868021845
.1.3.6.1.2.1.2.2.1.10.2 = Counter32: 2872183008
.1.3.6.1.2.1.2.2.1.16.2 = Counter32: 999000
.1.3.6.1.2.1.31.1.1.1.1.1 = STRING: \"lo\"
.1.3.6.1.2.1.31.1.1.1.1.2 = STRING: \"enp2s0\"
.1.3.6.1.2.1.31.1.1.1.6.2 = Counter64: 9000000000
.1.3.6.1.2.1.31.1.1.1.10.2 = Counter64: 7000000000
";
        let stats = parse_interface_stats(output);
        assert_eq!(stats.len(), 2, "lo + enp2s0");

        let lo = stats.iter().find(|i| i.name == "lo").unwrap();
        assert_eq!(lo.bytes_in, 1868021845);
        assert_eq!(lo.bytes_out, 0);

        let enp = stats.iter().find(|i| i.name == "enp2s0").unwrap();
        assert_eq!(enp.mtu, 1500);
        assert_eq!(enp.mac_address, "64:00:6A:40:81:6C");
        // ifHCInOctets (ifXTable) wins over the 32-bit ifInOctets.
        assert_eq!(enp.bytes_in, 9_000_000_000);
        assert_eq!(enp.bytes_out, 7_000_000_000);
    }

    #[test]
    fn test_storage_stats_parses_numeric_oid_walk() {
        // hrStorageSize/Used are counts of hrStorageAllocationUnits (1024).
        let output = "\
.1.3.6.1.2.1.25.2.3.1.3.1 = STRING: \"Physical memory\"
.1.3.6.1.2.1.25.2.3.1.4.1 = INTEGER: 1024
.1.3.6.1.2.1.25.2.3.1.5.1 = INTEGER: 12159548
.1.3.6.1.2.1.25.2.3.1.6.1 = INTEGER: 9632492
.1.3.6.1.2.1.25.2.3.1.3.10 = STRING: \"/\"
.1.3.6.1.2.1.25.2.3.1.4.10 = INTEGER: 4096
.1.3.6.1.2.1.25.2.3.1.5.10 = INTEGER: 1000000
.1.3.6.1.2.1.25.2.3.1.6.10 = INTEGER: 400000
";
        let stats = parse_storage_stats(output);
        assert_eq!(stats.len(), 2);

        let mem = stats
            .iter()
            .find(|s| s.mount_point == "Physical memory")
            .unwrap();
        assert_eq!(mem.total, 12159548 * 1024);
        assert_eq!(mem.used, 9632492 * 1024);
        assert_eq!(mem.available, (12159548 - 9632492) * 1024);
        assert!((mem.usage_percent - 79.2).abs() < 0.1);

        let root = stats.iter().find(|s| s.mount_point == "/").unwrap();
        assert_eq!(root.total, 1000000 * 4096);
        assert_eq!(root.available, 600000 * 4096);
        assert_eq!(root.used, 400000 * 4096);
    }

    #[test]
    fn test_interface_stats_empty_walk() {
        let stats = parse_interface_stats("");
        assert!(stats.is_empty());
    }

    #[test]
    fn test_normalize_mac_formats_hex_string() {
        assert_eq!(normalize_mac("64 00 6A 40 81 6C "), "64:00:6A:40:81:6C");
        assert_eq!(normalize_mac(""), "");
    }

    #[test]
    fn test_split_oid_line_strips_type_token_and_quotes() {
        assert_eq!(
            split_oid_line(".1.3.6.1.2.1.2.2.1.2.2 = STRING: \"enp2s0\""),
            Some(("1.3.6.1.2.1.2.2.1.2.2", "enp2s0"))
        );
        assert_eq!(
            split_oid_line(".1.3.6.1.2.1.2.2.1.6.2 = Hex-STRING: 64 00 6A"),
            Some(("1.3.6.1.2.1.2.2.1.6.2", "64 00 6A"))
        );
        assert_eq!(split_oid_line("not an snmp line"), None);
    }

    // -- ROADMAP 13.5: the saved version and the iface filter --------------

    #[test]
    fn test_supported_versions_are_the_ones_the_card_collects() {
        assert_eq!(SUPPORTED_SNMP_VERSIONS, ["1", "2c"]);
        assert_eq!(validate_snmp_version("1").unwrap(), "1");
        assert_eq!(validate_snmp_version("2c").unwrap(), "2c");
        assert_eq!(validate_snmp_version("  2c  ").unwrap(), "2c");
    }

    #[test]
    fn test_v3_is_refused_with_a_reason_instead_of_being_silently_downgraded() {
        // The card collects a community string only; quietly running v3 as v2c
        // would be worse than refusing it.
        for v3 in ["3", "3u", "v3"] {
            let err = validate_snmp_version(v3).unwrap_err();
            assert!(
                err.contains("v3") || err.contains("Unsupported"),
                "the refusal must say why: {err}"
            );
        }
        for bad in ["", "2", "2b", "4"] {
            assert!(validate_snmp_version(bad).is_err(), "'{bad}' is not a version");
        }
    }

    #[test]
    fn test_the_configured_version_reaches_the_argv() {
        // The old `run_walk` hardcoded `2c`; this pins that the saved value is
        // what both tools receive.
        let get = snmpget_args("1", "public", "localhost", "1.3.6.1.2.1.1.5.0");
        assert_eq!(
            get,
            vec![
                "-v",
                "1",
                "-c",
                "public",
                "-On",
                "localhost",
                "1.3.6.1.2.1.1.5.0"
            ]
        );
        let walk = snmpwalk_args("2c", "public", "localhost", "1.3.6.1.2.1.2.2.1");
        assert_eq!(walk[1], "2c");
        assert_eq!(walk[4], "-On", "numeric output is load-bearing");
    }

    #[test]
    fn test_the_iface_filter_narrows_the_interface_list() {
        let stats = vec![
            InterfaceStats {
                name: "lo".to_string(),
                ..Default::default()
            },
            InterfaceStats {
                name: "enp2s0".to_string(),
                ..Default::default()
            },
        ];
        // Empty = every interface (the card renders the whole table).
        assert_eq!(filter_interfaces(stats.clone(), "").len(), 2);
        assert_eq!(filter_interfaces(stats.clone(), "   ").len(), 2);

        let only = filter_interfaces(stats.clone(), "enp2s0");
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].name, "enp2s0");

        // A name that does not exist yields nothing rather than everything.
        assert!(filter_interfaces(stats, "wlan0").is_empty());
    }
}
