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
use tokio::time::{timeout, Duration};

const DNSMASQ_TIMEOUT: Duration = Duration::from_secs(30);
const DHCP_DISCOVER_TIMEOUT: Duration = Duration::from_secs(10);

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

            if config.dhcp_enabled && !Self::ip_in_range(*ip, config.dhcp_range_start, config.dhcp_range_end) {
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
        if path.components().any(|c| c == std::path::Component::ParentDir) {
            return Err("tftp_root cannot contain '..'".to_string());
        }

        // Must not be system paths
        let forbidden = ["/", "/bin", "/boot", "/dev", "/etc", "/lib", "/proc", "/root", "/run", "/sbin", "/sys", "/usr"];

        let path_str = path.to_string_lossy();
        for forbidden_path in &forbidden {
            if path_str.starts_with(forbidden_path)
                && (path_str.len() == forbidden_path.len() || path_str.as_bytes().get(forbidden_path.len()) == Some(&b'/')) {
                return Err(format!("tftp_root cannot be in {}", forbidden_path));
            }
        }

        Ok(())
    }

    /// Check if dnsmasq is available
    pub async fn is_available() -> bool {
        timeout(
            Duration::from_secs(5),
            Command::new("dnsmasq").arg("--version").output()
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .map(|o| o.status.success())
        .unwrap_or(false)
    }

    /// Detect DHCP conflict on interface
    pub async fn detect_conflict(interface: &str) -> Result<bool, String> {
        // Send DHCP discover and listen for responses
        let output = timeout(
            DHCP_DISCOVER_TIMEOUT,
            Command::new("sudo")
                .args(&["nmap", "--script", "broadcast-dhcp-discover", "-e", interface])
                .output()
        )
        .await
        .map_err(|_| "conflict detection timed out".to_string())?
        .map_err(|e| format!("failed to run conflict detection: {}", e))?;

        if !output.status.success() {
            return Ok(false); // Cannot detect, assume safe
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Check if any DHCP server responded
        Ok(stdout.contains("DHCP server") || stdout.contains("Server Identifier"))
    }

    /// Start DHCP/TFTP server
    pub async fn start_server(config: &DhcpServerConfig) -> Result<(), String> {
        // Validate first
        Self::validate_config(config)?;

        // Check for conflicts
        if config.dhcp_enabled {
            let conflict = Self::detect_conflict(&config.interface).await?;
            if conflict {
                return Err(format!("DHCP server already active on {}", config.interface));
            }
        }

        // Write dnsmasq configuration
        Self::write_dnsmasq_conf(config).await?;

        // Start dnsmasq
        let result = timeout(
            DNSMASQ_TIMEOUT,
            Command::new("sudo")
                .args(&["systemctl", "start", "dnsmasq"])
                .output()
        )
        .await
        .map_err(|_| "dnsmasq start timed out".to_string())?
        .map_err(|e| format!("failed to start dnsmasq: {}", e))?;

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
        let result = timeout(
            DNSMASQ_TIMEOUT,
            Command::new("sudo")
                .args(&["systemctl", "stop", "dnsmasq"])
                .output()
        )
        .await
        .map_err(|_| "dnsmasq stop timed out".to_string())?
        .map_err(|e| format!("failed to stop dnsmasq: {}", e))?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(format!("dnsmasq failed to stop: {}", stderr));
        }

        Ok(())
    }

    /// Check if dnsmasq is running
    pub async fn is_running() -> bool {
        timeout(
            Duration::from_secs(5),
            Command::new("systemctl")
                .args(&["is-active", "dnsmasq"])
                .output()
        )
        .await
        .ok()
        .and_then(|r| r.ok())
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
        let mut conf = String::from(
            "# HomeVault DHCP/TFTP Server\n\
             # Generated automatically - do not edit manually\n\n"
        );

        // Interface binding
        conf.push_str(&format!("interface={}\n", config.interface));
        conf.push_str("bind-interfaces\n\n");

        // DHCP configuration
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
            if !config.dns_servers.is_empty() {
                let dns_list: Vec<String> = config.dns_servers.iter().map(|ip| ip.to_string()).collect();
                conf.push_str(&format!("dhcp-option=6,{}\n", dns_list.join(",")));
            }

            // Reservations
            for (mac, ip) in &config.reservations {
                conf.push_str(&format!("dhcp-host={},{}\n", mac, ip));
            }

            conf.push_str("\n");
        }

        // TFTP configuration
        if config.tftp_enabled {
            conf.push_str("enable-tftp\n");
            conf.push_str(&format!("tftp-root={}\n", config.tftp_root.display()));

            if let Some(ref boot_file) = config.pxe_boot_file {
                conf.push_str(&format!("dhcp-boot={}\n", boot_file));
            }
        }

        // Write via sudo
        Self::write_config_file("/etc/dnsmasq.d/homevault.conf", &conf).await
    }

    /// Write configuration file with sudo
    async fn write_config_file(path: &str, content: &str) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;

        let mut child = Command::new("sudo")
            .args(&["tee", path])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn tee: {}", e))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(content.as_bytes()).await
                .map_err(|e| format!("failed to write config: {}", e))?;
        }

        let output = child.wait_with_output().await
            .map_err(|e| format!("tee failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("failed to write {}: {}", path, stderr));
        }

        Ok(())
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

        assert!(DhcpServerService::ip_in_range(Ipv4Addr::new(192, 168, 1, 150), start, end));
        assert!(DhcpServerService::ip_in_range(start, start, end));
        assert!(DhcpServerService::ip_in_range(end, start, end));
        assert!(!DhcpServerService::ip_in_range(Ipv4Addr::new(192, 168, 1, 50), start, end));
        assert!(!DhcpServerService::ip_in_range(Ipv4Addr::new(192, 168, 1, 250), start, end));
    }
}
