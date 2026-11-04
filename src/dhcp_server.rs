//! DHCP/TFTP/PXE server — network boot and address management
//!
//! Provides DHCP server for automatic IP address assignment and TFTP server for
//! network boot (PXE) scenarios. Designed for isolated networks or lab environments.
//!
//! # Architecture
//!
//! - dnsmasq for integrated DHCP + TFTP
//! - Per-interface binding with conflict detection
//! - Explicit LAN confirmation required
//! - TFTP root path confinement
//!
//! # One daemon, one drop-in
//!
//! The local DNS server (`crate::local_dns`) is the same `dnsmasq` process, so
//! the two features share `/etc/dnsmasq.d/homevault.conf`. [`render_conf`] is
//! the single composer — including the listening policy (`interface=`,
//! `bind-interfaces`), which is why neither half writes its own file: two
//! drop-ins would let them disagree about which interfaces the one daemon
//! answers on, and whichever was written last would win silently.
//! [`reconcile_daemon`] is the single gate that starts, restarts or stops the
//! daemon from *both* persisted configurations.
//!
//! # Security
//!
//! - Default disabled, opt-in only
//! - Never binds all interfaces without confirmation
//! - Conflict detection prevents dual DHCP servers
//! - TFTP paths are validated and confined
//! - All operations are journaled

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tokio::time::Duration;

use crate::config::paths;
use crate::local_dns::LocalDnsConfig;
use crate::utils::process::{run_timed_async, run_timed_async_with_stdin};

/// The one drop-in the appliance owns. dnsmasq reads every file in
/// `/etc/dnsmasq.d/`, so writing a second one would make the two halves race for
/// the same directives.
pub const DROPIN_PATH: &str = "/etc/dnsmasq.d/homevault.conf";

const DNSMASQ_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_DISCOVER_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for a read-only probe (`dnsmasq --version`, `systemctl is-active`).
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Deadline for writing the root-owned dnsmasq drop-in (`sudo tee`).
const CONFIG_WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// DHCP/TFTP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpServerConfig {
    /// Whether DHCP server is enabled
    pub dhcp_enabled: bool,

    /// Whether TFTP server is enabled
    pub tftp_enabled: bool,

    /// Network interface to bind
    pub interface: String,

    /// DHCP range start
    pub dhcp_range_start: Ipv4Addr,

    /// DHCP range end
    pub dhcp_range_end: Ipv4Addr,

    /// Subnet mask
    pub netmask: Ipv4Addr,

    /// Gateway (router) address
    pub gateway: Option<Ipv4Addr>,

    /// DNS servers to advertise
    pub dns_servers: Vec<Ipv4Addr>,

    /// Lease time in seconds
    pub lease_time_secs: u32,

    /// DHCP reservations (MAC -> IP)
    pub reservations: HashMap<String, Ipv4Addr>,

    /// TFTP root directory
    pub tftp_root: PathBuf,

    /// PXE boot file
    pub pxe_boot_file: Option<String>,

    /// Explicit confirmation flag
    pub confirmed: bool,
}

/// DHCP server state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpServerState {
    /// Whether dnsmasq is running
    pub running: bool,

    /// Active leases
    pub leases: Vec<DhcpLease>,

    /// Conflict detected on interface
    pub conflict_detected: bool,

    /// Last error message
    pub last_error: Option<String>,
}

/// DHCP lease information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpLease {
    /// MAC address
    pub mac: String,

    /// Assigned IP address
    pub ip: Ipv4Addr,

    /// Hostname if known
    pub hostname: Option<String>,

    /// Lease expiry timestamp
    pub expires_at: u64,
}

pub struct DhcpServerService;

impl DhcpServerService {
    /// Validate DHCP configuration
    pub fn validate_config(config: &DhcpServerConfig) -> Result<(), String> {
        // Validate interface
        if config.interface.is_empty() {
            return Err("interface cannot be empty".to_string());
        }

        // Validate DHCP range
        if config.dhcp_enabled {
            if config.dhcp_range_start >= config.dhcp_range_end {
                return Err("dhcp_range_start must be less than dhcp_range_end".to_string());
            }

            // Check range is in same subnet
            if !Self::same_subnet(
                config.dhcp_range_start,
                config.dhcp_range_end,
                config.netmask,
            ) {
                return Err("DHCP range must be in same subnet".to_string());
            }
        }

        // Validate lease time
        if config.lease_time_secs < 60 || config.lease_time_secs > 86400 * 30 {
            return Err("lease_time_secs must be between 60 and 2592000 (30 days)".to_string());
        }

        // Validate reservations
        for (mac, ip) in &config.reservations {
            Self::validate_mac(mac)?;

            if config.dhcp_enabled
                && !Self::ip_in_range(*ip, config.dhcp_range_start, config.dhcp_range_end)
            {
                return Err(format!("reservation {} is outside DHCP range", ip));
            }
        }

        // Validate TFTP root
        if config.tftp_enabled {
            Self::validate_tftp_root(&config.tftp_root)?;
        }

        // Validate PXE boot file if specified
        if let Some(ref boot_file) = config.pxe_boot_file {
            if boot_file.contains("..") || boot_file.starts_with('/') {
                return Err("pxe_boot_file must be relative and cannot contain '..'".to_string());
            }
        }

        // Require confirmation
        if !config.confirmed && (config.dhcp_enabled || config.tftp_enabled) {
            return Err("explicit confirmation required to enable DHCP/TFTP server".to_string());
        }

        Ok(())
    }

    /// Check if two IPs are in the same subnet.
    ///
    /// `pub` for `tests/dhcp_server_integration.rs`, which exercises the range
    /// validation against real addresses (an integration test is a separate
    /// crate and cannot see a private helper).
    pub fn same_subnet(ip1: Ipv4Addr, ip2: Ipv4Addr, netmask: Ipv4Addr) -> bool {
        let ip1_octets = ip1.octets();
        let ip2_octets = ip2.octets();
        let mask_octets = netmask.octets();

        for i in 0..4 {
            if (ip1_octets[i] & mask_octets[i]) != (ip2_octets[i] & mask_octets[i]) {
                return false;
            }
        }

        true
    }

    /// Check if IP is within range (see `same_subnet` for why this is `pub`).
    pub fn ip_in_range(ip: Ipv4Addr, start: Ipv4Addr, end: Ipv4Addr) -> bool {
        let ip_u32 = u32::from_be_bytes(ip.octets());
        let start_u32 = u32::from_be_bytes(start.octets());
        let end_u32 = u32::from_be_bytes(end.octets());

        ip_u32 >= start_u32 && ip_u32 <= end_u32
    }

    /// Validate MAC address format
    fn validate_mac(mac: &str) -> Result<(), String> {
        let parts: Vec<&str> = mac.split(':').collect();

        if parts.len() != 6 {
            return Err(format!("invalid MAC address format: {}", mac));
        }

        for part in parts {
            if part.len() != 2 {
                return Err(format!("invalid MAC address format: {}", mac));
            }

            if !part.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("invalid MAC address format: {}", mac));
            }
        }

        Ok(())
    }

    /// Validate TFTP root path
    fn validate_tftp_root(path: &Path) -> Result<(), String> {
        // Must be absolute
        if !path.is_absolute() {
            return Err("tftp_root must be absolute".to_string());
        }

        // Cannot contain ..
        if path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err("tftp_root cannot contain '..'".to_string());
        }

        // Must not be system paths
        let forbidden = [
            "/", "/bin", "/boot", "/dev", "/etc", "/lib", "/proc", "/root", "/run", "/sbin",
            "/sys", "/usr",
        ];

        let path_str = path.to_string_lossy();
        for forbidden_path in &forbidden {
            if path_str.starts_with(forbidden_path)
                && (path_str.len() == forbidden_path.len()
                    || path_str.as_bytes().get(forbidden_path.len()) == Some(&b'/'))
            {
                return Err(format!("tftp_root cannot be in {}", forbidden_path));
            }
        }

        Ok(())
    }

    /// Check if dnsmasq is available
    pub async fn is_available() -> bool {
        let mut cmd = Command::new("dnsmasq");
        cmd.arg("--version");
        run_timed_async(&mut cmd, PROBE_TIMEOUT)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Detect DHCP conflict on interface
    pub async fn detect_conflict(interface: &str) -> Result<bool, String> {
        // Send DHCP discover and listen for responses
        let mut cmd = Command::new("sudo");
        cmd.args([
            "nmap",
            "--script",
            "broadcast-dhcp-discover",
            "-e",
            interface,
        ]);
        let output = run_timed_async(&mut cmd, DHCP_DISCOVER_TIMEOUT)
            .await
            .ok_or_else(|| "conflict detection timed out".to_string())?;

        if !output.status.success() {
            return Ok(false); // Cannot detect, assume safe
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Check if any DHCP server responded
        Ok(stdout.contains("DHCP server") || stdout.contains("Server Identifier"))
    }

    /// Start DHCP/TFTP server
    ///
    /// The drop-in is composed with the *persisted* local DNS settings, so
    /// starting the address server never silently turns the LAN's resolver off.
    pub async fn start_server(config: &DhcpServerConfig) -> Result<(), String> {
        // Validate first
        Self::validate_config(config)?;

        // Check for conflicts
        if config.dhcp_enabled {
            let conflict = Self::detect_conflict(&config.interface).await?;
            if conflict {
                return Err(format!(
                    "DHCP server already active on {}",
                    config.interface
                ));
            }
        }

        // Write dnsmasq configuration
        Self::write_dnsmasq_conf(config).await?;

        // Start dnsmasq
        let mut start = Command::new("sudo");
        start.args(["systemctl", "start", "dnsmasq"]);
        let result = run_timed_async(&mut start, DNSMASQ_TIMEOUT)
            .await
            .ok_or_else(|| "dnsmasq start timed out".to_string())?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(format!("dnsmasq failed to start: {}", stderr));
        }

        // Verify running
        tokio::time::sleep(Duration::from_millis(500)).await;

        if !Self::is_running().await {
            return Err("dnsmasq started but is not running".to_string());
        }

        Ok(())
    }

    /// Stop DHCP/TFTP server
    pub async fn stop_server() -> Result<(), String> {
        let mut stop = Command::new("sudo");
        stop.args(["systemctl", "stop", "dnsmasq"]);
        let result = run_timed_async(&mut stop, DNSMASQ_TIMEOUT)
            .await
            .ok_or_else(|| "dnsmasq stop timed out".to_string())?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(format!("dnsmasq failed to stop: {}", stderr));
        }

        Ok(())
    }

    /// Check if dnsmasq is running
    pub async fn is_running() -> bool {
        let mut cmd = Command::new("systemctl");
        cmd.args(["is-active", "dnsmasq"]);
        run_timed_async(&mut cmd, PROBE_TIMEOUT)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get current leases
    pub async fn get_leases() -> Result<Vec<DhcpLease>, String> {
        let leases_file = "/var/lib/misc/dnsmasq.leases";

        let content = tokio::fs::read_to_string(leases_file)
            .await
            .map_err(|e| format!("failed to read leases: {}", e))?;

        let mut leases = Vec::new();

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let expires_at = parts[0].parse::<u64>().unwrap_or(0);
                let mac = parts[1].to_string();
                let ip = parts[2].parse::<Ipv4Addr>().ok();
                let hostname = parts.get(3).map(|s| s.to_string());

                if let Some(ip) = ip {
                    leases.push(DhcpLease {
                        mac,
                        ip,
                        hostname,
                        expires_at,
                    });
                }
            }
        }

        Ok(leases)
    }

    /// Write dnsmasq configuration
    async fn write_dnsmasq_conf(config: &DhcpServerConfig) -> Result<(), String> {
        let dns = load_persisted_dns();
        Self::write_config_file(DROPIN_PATH, &Self::render_conf(config, &dns)).await
    }

    /// Render the whole drop-in from both halves of the appliance.
    ///
    /// Pure on purpose: every rule below is unit-tested without touching a host
    /// (`mod tests` at the bottom of this file).
    pub fn render_conf(config: &DhcpServerConfig, dns: &LocalDnsConfig) -> String {
        let mut conf = String::from(
            "# HomeVault DNS / DHCP / TFTP server\n\
             # Generated automatically - do not edit manually\n\n",
        );

        // --- Listening policy (shared by both halves) --------------------
        // `bind-interfaces` plus one `interface=` per served interface: the
        // daemon answers on those and nothing else, which is what keeps a
        // resolver or an address server off the WAN.
        let mut interfaces: Vec<&str> = Vec::new();
        if config.dhcp_enabled || config.tftp_enabled {
            interfaces.push(config.interface.as_str());
        }
        if dns.enabled && !dns.interface.is_empty() && !interfaces.contains(&dns.interface.as_str())
        {
            interfaces.push(dns.interface.as_str());
        }
        for interface in &interfaces {
            conf.push_str(&format!("interface={}\n", interface));
        }
        if !interfaces.is_empty() {
            conf.push_str("bind-interfaces\n");
        }
        conf.push('\n');

        // --- DHCP --------------------------------------------------------
        if config.dhcp_enabled {
            conf.push_str(&format!(
                "dhcp-range={},{},{},{}s\n",
                config.dhcp_range_start,
                config.dhcp_range_end,
                config.netmask,
                config.lease_time_secs
            ));

            // Gateway
            if let Some(gateway) = config.gateway {
                conf.push_str(&format!("dhcp-option=3,{}\n", gateway));
            }

            // DNS servers
            if dns.enabled && dns.serve_dhcp_clients {
                // The local DNS server is on and asked to answer for the LAN's
                // DHCP clients: emitting nothing is how dnsmasq is told to send
                // its own address for option 6. Emitting both this and the list
                // below would leave two answers for the same option.
                conf.push_str(
                    "# DHCP clients are told to resolve through this appliance (dnsmasq\n\
                     # sends its own address for option 6 when no list is given).\n",
                );
            } else if !config.dns_servers.is_empty() {
                let dns_list: Vec<String> =
                    config.dns_servers.iter().map(|ip| ip.to_string()).collect();
                conf.push_str(&format!("dhcp-option=6,{}\n", dns_list.join(",")));
            }

            // Reservations
            for (mac, ip) in &config.reservations {
                conf.push_str(&format!("dhcp-host={},{}\n", mac, ip));
            }

            conf.push('\n');
        }

        // --- TFTP --------------------------------------------------------
        if config.tftp_enabled {
            conf.push_str("enable-tftp\n");
            conf.push_str(&format!("tftp-root={}\n", config.tftp_root.display()));

            if let Some(ref boot_file) = config.pxe_boot_file {
                conf.push_str(&format!("dhcp-boot={}\n", boot_file));
            }
            conf.push('\n');
        }

        // --- DNS ---------------------------------------------------------
        conf.push_str(&crate::local_dns::render_section(dns));

        conf
    }

    /// Reconcile the single daemon with **both** persisted configurations.
    ///
    /// The two screens each save their own document; neither of them may decide
    /// alone whether `dnsmasq` runs, because turning DHCP off used to stop a
    /// daemon that was also answering DNS. This is the one place that decides:
    ///
    /// - any switch on → compose the drop-in, `systemctl restart` (which starts
    ///   a stopped unit and reloads a running one), then verify it stayed up;
    /// - every switch off → remove the drop-in and stop the daemon, so a later
    ///   `systemctl start dnsmasq` cannot resurrect a configuration the admin
    ///   turned off.
    ///
    /// Returns whether the daemon is running afterwards.
    pub async fn reconcile_daemon() -> Result<bool, String> {
        let dhcp = match Self::load_persisted().await {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!("Reading the DHCP settings failed, assuming off: {}", e);
                DhcpServerConfig::default()
            }
        };
        let dns = load_persisted_dns();
        let wanted = Self::daemon_wanted(&dhcp, &dns);

        if !wanted {
            // Nothing to undo on a machine that never had a drop-in and whose
            // daemon is already stopped: skipping the two root commands keeps a
            // "save while everything is off" from asking for `sudo` at all.
            let present = std::path::Path::new(DROPIN_PATH).exists();
            if present {
                Self::remove_config_file(DROPIN_PATH).await?;
            }
            if Self::is_running().await {
                Self::stop_server().await?;
            }
            return Ok(false);
        }

        Self::write_config_file(DROPIN_PATH, &Self::render_conf(&dhcp, &dns)).await?;

        let mut restart = Command::new("sudo");
        restart.args(["systemctl", "restart", "dnsmasq"]);
        let result = run_timed_async(&mut restart, DNSMASQ_TIMEOUT)
            .await
            .ok_or_else(|| "dnsmasq restart timed out".to_string())?;
        if !result.status.success() {
            return Err(format!(
                "dnsmasq failed to start: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
        if !Self::is_running().await {
            return Err(
                "dnsmasq accepted the configuration but is not running — check that the interface exists and that port 53 is free"
                    .to_string(),
            );
        }
        Ok(true)
    }

    /// Does any switch need the daemon? The three of them, in one place.
    pub fn daemon_wanted(dhcp: &DhcpServerConfig, dns: &LocalDnsConfig) -> bool {
        dhcp.dhcp_enabled || dhcp.tftp_enabled || dns.enabled
    }

    /// Where the DHCP settings are stored.
    pub fn config_path() -> PathBuf {
        paths::base_dir().join("data/dhcp_server.json")
    }

    /// The saved DHCP settings, or the safe default (off) when there are none.
    pub async fn load_persisted() -> Result<DhcpServerConfig, String> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(DhcpServerConfig::default());
        }
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read config: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse config: {}", e))
    }

    /// Persist the DHCP settings (the caller has already validated them).
    pub async fn save_persisted(config: &DhcpServerConfig) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        let content = serde_json::to_string_pretty(config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| format!("Failed to write config: {}", e))
    }

    /// Write configuration file with sudo.
    ///
    /// `sudo tee` reads the body from stdin and the whole exchange is bounded:
    /// a password prompt that never returns must not hang the save path.
    async fn write_config_file(path: &str, content: &str) -> Result<(), String> {
        let mut tee = Command::new("sudo");
        tee.args(["tee", path]);
        let output = run_timed_async_with_stdin(&mut tee, CONFIG_WRITE_TIMEOUT, content.as_bytes())
            .await
            .map_err(|e| format!("failed to write {}: {}", path, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("failed to write {}: {}", path, stderr));
        }

        Ok(())
    }

    /// Remove the drop-in (the "everything off" state), under a deadline.
    async fn remove_config_file(path: &str) -> Result<(), String> {
        let mut rm = Command::new("sudo");
        rm.args(["rm", "-f", path]);
        let output = run_timed_async(&mut rm, CONFIG_WRITE_TIMEOUT)
            .await
            .ok_or_else(|| format!("removing {} timed out", path))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("failed to remove {}: {}", path, stderr));
        }

        Ok(())
    }
}

/// The persisted DNS half, for the composer.
///
/// A corrupt or unreadable document is reported and treated as "off": a broken
/// DNS file must never stop the address server from starting.
fn load_persisted_dns() -> LocalDnsConfig {
    // The module is async elsewhere, but the composer is called from an async
    // context that must not block on a nested runtime; the file is tiny and read
    // through the same synchronous helper the SNMP agent uses.
    match std::fs::read_to_string(crate::local_dns::config_path()) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!("The local DNS settings could not be parsed: {}", e);
            LocalDnsConfig::default()
        }),
        Err(_) => LocalDnsConfig::default(),
    }
}

impl Default for DhcpServerConfig {
    fn default() -> Self {
        Self {
            dhcp_enabled: false,
            tftp_enabled: false,
            interface: "eth0".to_string(),
            dhcp_range_start: Ipv4Addr::new(192, 168, 1, 100),
            dhcp_range_end: Ipv4Addr::new(192, 168, 1, 200),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            gateway: None,
            dns_servers: vec![],
            lease_time_secs: 86400, // 24 hours
            reservations: HashMap::new(),
            tftp_root: PathBuf::from("/srv/tftp"),
            pxe_boot_file: None,
            confirmed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_config_requires_interface() {
        let mut config = DhcpServerConfig::default();
        config.interface = "".to_string();

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_config_range_order() {
        let mut config = DhcpServerConfig::default();
        config.dhcp_enabled = true;
        config.dhcp_range_start = Ipv4Addr::new(192, 168, 1, 200);
        config.dhcp_range_end = Ipv4Addr::new(192, 168, 1, 100);

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("less than"));
    }

    #[test]
    fn test_validate_lease_time_bounds() {
        let mut config = DhcpServerConfig::default();
        config.lease_time_secs = 30;

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());

        config.lease_time_secs = 86400 * 31;
        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_mac_format() {
        assert!(DhcpServerService::validate_mac("aa:bb:cc:dd:ee:ff").is_ok());
        assert!(DhcpServerService::validate_mac("AA:BB:CC:DD:EE:FF").is_ok());
        assert!(DhcpServerService::validate_mac("00:11:22:33:44:55").is_ok());

        assert!(DhcpServerService::validate_mac("invalid").is_err());
        assert!(DhcpServerService::validate_mac("aa:bb:cc:dd:ee").is_err());
        assert!(DhcpServerService::validate_mac("aa:bb:cc:dd:ee:ff:gg").is_err());
        assert!(DhcpServerService::validate_mac("zz:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn test_validate_tftp_root_must_be_absolute() {
        let result = DhcpServerService::validate_tftp_root(Path::new("relative/path"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[test]
    fn test_validate_tftp_root_no_parent_dir() {
        let result = DhcpServerService::validate_tftp_root(Path::new("/srv/../etc"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_tftp_root_forbidden_paths() {
        let forbidden = ["/", "/etc", "/bin"];

        for path in &forbidden {
            let result = DhcpServerService::validate_tftp_root(Path::new(path));
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_validate_requires_confirmation() {
        let mut config = DhcpServerConfig::default();
        config.dhcp_enabled = true;
        config.confirmed = false;

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("confirmation"));
    }

    #[test]
    fn test_same_subnet() {
        let ip1 = Ipv4Addr::new(192, 168, 1, 100);
        let ip2 = Ipv4Addr::new(192, 168, 1, 200);
        let mask = Ipv4Addr::new(255, 255, 255, 0);

        assert!(DhcpServerService::same_subnet(ip1, ip2, mask));

        let ip3 = Ipv4Addr::new(192, 168, 2, 100);
        assert!(!DhcpServerService::same_subnet(ip1, ip3, mask));
    }

    #[test]
    fn test_ip_in_range() {
        let start = Ipv4Addr::new(192, 168, 1, 100);
        let end = Ipv4Addr::new(192, 168, 1, 200);

        assert!(DhcpServerService::ip_in_range(
            Ipv4Addr::new(192, 168, 1, 150),
            start,
            end
        ));
        assert!(DhcpServerService::ip_in_range(start, start, end));
        assert!(DhcpServerService::ip_in_range(end, start, end));
        assert!(!DhcpServerService::ip_in_range(
            Ipv4Addr::new(192, 168, 1, 50),
            start,
            end
        ));
        assert!(!DhcpServerService::ip_in_range(
            Ipv4Addr::new(192, 168, 1, 250),
            start,
            end
        ));
    }

    // -----------------------------------------------------------------------
    // The composed drop-in (phase C1): one daemon, two configuration halves.
    // -----------------------------------------------------------------------

    fn dns_on(interface: &str) -> LocalDnsConfig {
        LocalDnsConfig {
            enabled: true,
            interface: interface.to_string(),
            serve_dhcp_clients: false,
            forwarders: vec!["192.168.1.1".to_string()],
            records: vec![],
            overrides: vec![],
            confirmed: true,
        }
    }

    #[test]
    fn test_render_conf_only_binds_the_interfaces_that_serve_something() {
        let mut dhcp = DhcpServerConfig::default();
        dhcp.dhcp_enabled = true;
        dhcp.interface = "eth0".to_string();

        let conf = DhcpServerService::render_conf(&dhcp, &LocalDnsConfig::default());
        assert!(conf.contains("interface=eth0\n"), "{conf}");
        assert!(conf.contains("bind-interfaces"), "{conf}");
        // The DNS interface is not bound when the DNS server is off.
        assert_eq!(conf.matches("interface=").count(), 1, "{conf}");
    }

    #[test]
    fn test_render_conf_binds_both_halves_and_lists_one_interface_once() {
        let mut dhcp = DhcpServerConfig::default();
        dhcp.dhcp_enabled = true;
        dhcp.interface = "eth0".to_string();

        // Same interface on both halves: one `interface=` line, not two.
        let same = DhcpServerService::render_conf(&dhcp, &dns_on("eth0"));
        assert_eq!(same.matches("interface=").count(), 1, "{same}");
        assert_eq!(same.matches("bind-interfaces").count(), 1, "{same}");

        // Different interfaces: both are bound (dnsmasq serves each of them
        // for the feature that asked for it).
        let split = DhcpServerService::render_conf(&dhcp, &dns_on("eth1"));
        assert!(split.contains("interface=eth0"), "{split}");
        assert!(split.contains("interface=eth1"), "{split}");
        assert_eq!(split.matches("bind-interfaces").count(), 1, "{split}");
    }

    #[test]
    fn test_render_conf_carries_the_dns_section() {
        let mut dns = dns_on("eth0");
        dns.records = vec![crate::local_dns::DnsRecord {
            name: "nas.lan".to_string(),
            kind: crate::local_dns::RecordKind::A,
            value: "192.168.1.5".to_string(),
        }];
        let conf = DhcpServerService::render_conf(&DhcpServerConfig::default(), &dns);
        assert!(conf.contains("server=192.168.1.1"), "{conf}");
        assert!(conf.contains("host-record=nas.lan,192.168.1.5"), "{conf}");
        // Off DHCP: no address range, no reservations.
        assert!(!conf.contains("dhcp-range="), "{conf}");
    }

    #[test]
    fn test_advertising_the_appliance_replaces_the_dns_server_list() {
        let mut dhcp = DhcpServerConfig::default();
        dhcp.dhcp_enabled = true;
        dhcp.dns_servers = vec![Ipv4Addr::new(9, 9, 9, 9)];

        // Server off: the DHCP card's list is what clients get.
        let off = DhcpServerService::render_conf(&dhcp, &LocalDnsConfig::default());
        assert!(off.contains("dhcp-option=6,9.9.9.9"), "{off}");

        // Server on and asked to answer for DHCP clients: the list is dropped so
        // dnsmasq sends its own address, and there is still exactly one answer
        // for option 6.
        let mut dns = dns_on("eth0");
        dns.serve_dhcp_clients = true;
        let on = DhcpServerService::render_conf(&dhcp, &dns);
        assert!(!on.contains("dhcp-option=6,"), "{on}");
        assert_eq!(on.matches("dhcp-option=6").count(), 0, "{on}");
        assert!(on.contains("sends its own address for option 6"), "{on}");
    }

    #[test]
    fn test_daemon_wanted_needs_one_of_the_three_switches() {
        let off = DhcpServerConfig::default();
        assert!(!DhcpServerService::daemon_wanted(
            &off,
            &LocalDnsConfig::default()
        ));

        let mut dhcp = off.clone();
        dhcp.tftp_enabled = true;
        assert!(DhcpServerService::daemon_wanted(
            &dhcp,
            &LocalDnsConfig::default()
        ));

        assert!(DhcpServerService::daemon_wanted(&off, &dns_on("eth0")));
    }
}
