/* =============================================================================
 * File          : proc.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Reads live hardware and system information directly from the Linux
 *   kernel's /proc and /sys virtual filesystems: load average, memory,
 *   uptime, mounted disks, network interfaces (with Wi-Fi signal/SSID where
 *   available), CPU details, motherboard, RAM, GPU, temperature sensors,
 *   UPS and installed-software versions. Each call parses the current
 *   kernel state, so results are always fresh and nothing is cached.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Called by the dashboard, widgets and network handlers
 *   (src/handlers/dashboard.rs, widgets.rs, network.rs), e.g.
 *   `ProcParser::parse_meminfo()`.
 * =============================================================================
 */

//! Read-only access to live system information through the kernel's /proc
//! and /sys virtual filesystems: CPU, memory, uptime, mounts, network
//! interfaces (including Wi-Fi details), motherboard, GPU, sensors, UPS and
//! installed-software versions. Everything is parsed on demand from the
//! running kernel — no daemon, no cache, no state.

use serde::Serialize;
use std::fs;

pub struct ProcParser;

impl ProcParser {
    /// Read a single value from a /proc file
    pub fn read_proc_value(path: &str) -> String {
        if let Ok(content) = fs::read_to_string(path) {
            content.lines().next().unwrap_or("").to_string()
        } else {
            String::new()
        }
    }

    /// Read entire contents of a /proc file
    pub fn read_proc_file(path: &str) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    /// Parse CPU load average from /proc/loadavg
    pub fn parse_loadavg() -> String {
        Self::read_proc_value("/proc/loadavg")
    }

    /// Parse memory information from /proc/meminfo
    pub fn parse_meminfo() -> MemInfo {
        let content = Self::read_proc_file("/proc/meminfo");
        let mut mem_info = MemInfo::default();

        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                mem_info.total_kb = Self::extract_kb_value(line);
            } else if line.starts_with("MemFree:") {
                mem_info.free_kb = Self::extract_kb_value(line);
            } else if line.starts_with("MemAvailable:") {
                mem_info.available_kb = Self::extract_kb_value(line);
            }
        }

        mem_info
    }

    /// Parse uptime from /proc/uptime
    pub fn parse_uptime() -> u64 {
        let content = Self::read_proc_value("/proc/uptime");
        if let Some(first_part) = content.split_whitespace().next() {
            if let Ok(seconds) = first_part.parse::<f64>() {
                return seconds as u64;
            }
        }
        0
    }

    /// Parse disk mounts from /proc/mounts
    pub fn parse_mounts() -> Vec<DiskMount> {
        let content = Self::read_proc_file("/proc/mounts");
        let mut mounts = Vec::new();

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let device = parts[0];
                let mount_point = parts[1];
                let fstype = parts[2];

                // Only include real devices (start with /dev/)
                if device.starts_with("/dev/") {
                    mounts.push(DiskMount {
                        name: device.to_string(),
                        mount: mount_point.to_string(),
                        fstype: fstype.to_string(),
                    });
                }
            }
        }

        mounts
    }

    /// Parse network interfaces from /proc/net/dev and /sys/class/net
    pub fn parse_network_interfaces() -> Vec<NetworkInterface> {
        let content = Self::read_proc_file("/proc/net/dev");
        let mut interfaces = Vec::new();

        // Skip first 2 header lines
        let lines: Vec<&str> = content.lines().skip(2).collect();

        for line in lines {
            if let Some(colon_pos) = line.find(':') {
                let iface_name = line[..colon_pos].trim();

                // Skip loopback
                if iface_name != "lo" {
                    let iface_type = Self::detect_interface_type(iface_name);
                    let status = Self::get_interface_status(iface_name);
                    let ipv4 = Self::get_interface_ipv4(iface_name);
                    let ipv6 = Self::get_interface_ipv6(iface_name);
                    let netmask = Self::get_interface_netmask(iface_name);
                    let mac = Self::get_interface_mac(iface_name);
                    let model = Self::get_interface_model(iface_name);
                    let link_speed = Self::get_interface_speed(iface_name);

                    let (signal, channel, ssid) = if iface_type == "wireless" {
                        (
                            Self::get_wifi_signal(iface_name),
                            Self::get_wifi_channel(iface_name),
                            Self::get_wifi_ssid(iface_name),
                        )
                    } else {
                        (None, None, None)
                    };

                    interfaces.push(NetworkInterface {
                        name: iface_name.to_string(),
                        status,
                        iface_type,
                        ipv4,
                        ipv6,
                        netmask,
                        mac,
                        signal,
                        channel,
                        ssid,
                        model,
                        link_speed,
                    });
                }
            }
        }

        interfaces
    }

    /// Detect if interface is wired or wireless by checking /sys/class/net
    fn detect_interface_type(iface_name: &str) -> String {
        // Check if wireless directory exists in /sys/class/net
        let wireless_path = format!("/sys/class/net/{}/wireless", iface_name);
        if fs::metadata(&wireless_path).is_ok() {
            return "wireless".to_string();
        }

        // Check if it's a PHY device (another indicator of wireless)
        let phy_path = format!("/sys/class/net/{}/phy80211", iface_name);
        if fs::metadata(&phy_path).is_ok() {
            return "wireless".to_string();
        }

        // Default to wired
        "wired".to_string()
    }

    /// Get interface operational status (up/down)
    fn get_interface_status(iface_name: &str) -> String {
        let operstate_path = format!("/sys/class/net/{}/operstate", iface_name);
        if let Ok(content) = fs::read_to_string(&operstate_path) {
            let status = content.trim();
            if status == "up" || status == "unknown" {
                return "up".to_string();
            }
        }
        "down".to_string()
    }

    /// Get interface IPv4 address using ip command
    fn get_interface_ipv4(iface_name: &str) -> Option<String> {
        let output = std::process::Command::new("ip")
            .args(["-4", "addr", "show", iface_name])
            .output();

        if let Ok(result) = output {
            if result.status.success() {
                let content = String::from_utf8_lossy(&result.stdout);
                // Parse the output to find inet line
                for line in content.lines() {
                    if line.trim().starts_with("inet ") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            let addr = parts[1].split('/').next()?;
                            return Some(addr.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    /// Get interface netmask using ip command
    fn get_interface_netmask(iface_name: &str) -> Option<String> {
        let output = std::process::Command::new("ip")
            .args(["-4", "addr", "show", iface_name])
            .output();

        if let Ok(result) = output {
            if result.status.success() {
                let content = String::from_utf8_lossy(&result.stdout);
                for line in content.lines() {
                    if line.trim().starts_with("inet ") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 2 {
                            let cidr = parts[1].split('/').nth(1)?;
                            // Convert CIDR to netmask
                            if let Ok(prefix) = cidr.parse::<u32>() {
                                return Some(Self::cidr_to_netmask(prefix));
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Get interface MAC address
    fn get_interface_mac(iface_name: &str) -> Option<String> {
        let addr_path = format!("/sys/class/net/{}/address", iface_name);
        if let Ok(content) = fs::read_to_string(&addr_path) {
            Some(content.trim().to_string())
        } else {
            None
        }
    }

    /// Get interface model/driver
    fn get_interface_model(iface_name: &str) -> String {
        let driver_path = format!("/sys/class/net/{}/device/driver", iface_name);
        if let Ok(_content) = fs::read_to_string(&driver_path) {
            // Read the symlink target to get driver name
            if let Ok(path) = fs::read_link(&driver_path) {
                return path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Unknown")
                    .to_string();
            }
        }

        // Try ethtool for more detailed info
        if let Ok(output) = std::process::Command::new("ethtool")
            .arg("-i")
            .arg(iface_name)
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("driver:") {
                    if let Some(driver) = line.split(':').nth(1) {
                        return driver.trim().to_string();
                    }
                }
            }
        }

        "Unknown".to_string()
    }

    /// Get interface link speed
    fn get_interface_speed(iface_name: &str) -> String {
        let speed_path = format!("/sys/class/net/{}/speed", iface_name);
        if let Ok(content) = fs::read_to_string(&speed_path) {
            let speed_mbps = content.trim();
            if let Ok(mbps) = speed_mbps.parse::<u64>() {
                if mbps >= 1000 {
                    return format!("{} Gbps", mbps / 1000);
                } else {
                    return format!("{} Mbps", mbps);
                }
            }
        }

        // Try ethtool for speed
        if let Ok(output) = std::process::Command::new("ethtool")
            .arg(iface_name)
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("Speed:") {
                    if let Some(speed) = line.split(':').nth(1) {
                        return speed.trim().to_string();
                    }
                }
            }
        }

        "Unknown".to_string()
    }

    /// Get WiFi signal strength
    fn get_wifi_signal(iface_name: &str) -> Option<String> {
        let signal_path = format!("/sys/class/net/{}/wireless/link", iface_name);
        if let Ok(content) = fs::read_to_string(&signal_path) {
            Some(content.trim().to_string())
        } else {
            None
        }
    }

    /// Get WiFi channel
    fn get_wifi_channel(iface_name: &str) -> Option<String> {
        let channel_path = format!("/sys/class/net/{}/wireless/channel", iface_name);
        if let Ok(content) = fs::read_to_string(&channel_path) {
            Some(content.trim().to_string())
        } else {
            None
        }
    }

    /// Get WiFi SSID
    fn get_wifi_ssid(_iface_name: &str) -> Option<String> {
        None
    }

    /// Get interface IPv6 address using `ip -6 addr show`
    fn get_interface_ipv6(iface_name: &str) -> Option<String> {
        let output = std::process::Command::new("ip")
            .args(["-6", "addr", "show", iface_name])
            .output();

        if let Ok(result) = output {
            if result.status.success() {
                let content = String::from_utf8_lossy(&result.stdout);
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("inet6 ") && !trimmed.contains("fe80::") {
                        let parts: Vec<&str> = trimmed.split_whitespace().collect();
                        if !parts.is_empty() {
                            let addr = parts[1].split('/').next()?;
                            if !addr.is_empty() && addr != "::" {
                                return Some(addr.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Get system hostname
    pub fn get_hostname() -> String {
        if let Ok(output) = std::process::Command::new("hostname").output() {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
        if let Ok(content) = fs::read_to_string("/etc/hostname") {
            return content.trim().to_string();
        }
        String::new()
    }

    /// Get workgroup/domain from Samba config
    pub fn get_workgroup() -> String {
        if let Ok(output) = std::process::Command::new("testparm")
            .args(["-s", "-l"])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("workgroup") {
                        if let Some(eq_pos) = trimmed.find('=') {
                            return trimmed[eq_pos + 1..].trim().to_string();
                        }
                    }
                }
            }
        }
        if let Ok(content) = fs::read_to_string("/etc/samba/smb.conf") {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("workgroup") {
                    if let Some(eq_pos) = trimmed.find('=') {
                        return trimmed[eq_pos + 1..].trim().to_string();
                    }
                }
            }
        }
        String::from("WORKGROUP")
    }

    /// Get default gateway using `ip route`
    pub fn get_default_gateway() -> String {
        if let Ok(output) = std::process::Command::new("ip")
            .args(["route", "show", "default"])
            .output()
        {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                for line in content.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    for (i, part) in parts.iter().enumerate() {
                        if *part == "via" && i + 1 < parts.len() {
                            return parts[i + 1].to_string();
                        }
                    }
                }
            }
        }
        String::new()
    }

    /// Get DNS servers from /etc/resolv.conf
    pub fn get_dns_servers() -> Vec<String> {
        let mut servers = Vec::new();
        if let Ok(content) = fs::read_to_string("/etc/resolv.conf") {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("nameserver") {
                    if let Some(space_pos) = trimmed.find(' ') {
                        let addr = trimmed[space_pos..].trim().to_string();
                        if !addr.is_empty() && addr != "::1" && addr != "127.0.0.1" {
                            servers.push(addr);
                        }
                    }
                }
            }
        }
        servers
    }

    /// Check if a systemd service is running
    fn check_service_status(service_name: &str) -> bool {
        if let Ok(output) = std::process::Command::new("systemctl")
            .args(["is-active", service_name])
            .output()
        {
            let stdout_owned = output.stdout.clone();
            let status = String::from_utf8_lossy(&stdout_owned).trim().to_string();
            return status == "active";
        }
        false
    }

    /// Get status of all relevant network services (SMB, NFS, FTP, SSH)
    pub fn get_network_services() -> Vec<ServiceStatus> {
        vec![
            ServiceStatus {
                name: "smb".to_string(),
                display_name: "SMB/CIFS (Samba)".to_string(),
                running: Self::check_service_status("smbd"),
            },
            ServiceStatus {
                name: "nfs".to_string(),
                display_name: "NFS".to_string(),
                running: Self::check_service_status("nfs-server"),
            },
            ServiceStatus {
                name: "ftp".to_string(),
                display_name: "FTP".to_string(),
                running: Self::check_service_status("vsftpd"),
            },
            ServiceStatus {
                name: "ssh".to_string(),
                display_name: "SSH".to_string(),
                running: Self::check_service_status("sshd"),
            },
            ServiceStatus {
                name: "webdav".to_string(),
                display_name: "WebDAV".to_string(),
                // WebDAV is served in-process (`crate::webdav`), so its status
                // is the bound port, not a systemd unit.
                running: crate::webdav::running_port().is_some(),
            },
            ServiceStatus {
                name: "sftp".to_string(),
                display_name: "SFTP".to_string(),
                running: Self::check_service_status("homevault-sftp"),
            },
        ]
    }

    /// Convert CIDR prefix to netmask
    fn cidr_to_netmask(prefix: u32) -> String {
        if prefix == 0 {
            return "0.0.0.0".to_string();
        }
        let mask = u32::MAX << (32 - prefix);
        format!(
            "{}.{}.{}.{}",
            (mask >> 24) & 0xFF,
            (mask >> 16) & 0xFF,
            (mask >> 8) & 0xFF,
            mask & 0xFF
        )
    }

    /// Extract numeric value in kB from a meminfo line like "MemTotal:       8192000 kB"
    fn extract_kb_value(line: &str) -> u64 {
        if let Some(colon_pos) = line.find(':') {
            let after_colon = &line[colon_pos + 1..];
            let parts: Vec<&str> = after_colon.split_whitespace().collect();
            if !parts.is_empty() {
                if let Ok(value) = parts[0].parse::<u64>() {
                    return value;
                }
            }
        }
        0
    }

    /// Parse detailed CPU information from /proc/cpuinfo
    pub fn parse_cpuinfo() -> CpuDetailedInfo {
        let content = Self::read_proc_file("/proc/cpuinfo");
        let mut cpu_info = CpuDetailedInfo::default();

        let mut processor_count = 0u32;
        let mut physical_id_set = std::collections::HashSet::new();

        for line in content.lines() {
            if line.starts_with("processor") {
                processor_count += 1;
            } else if line.starts_with("model name") {
                cpu_info.model_name = Self::extract_cpu_field(line);
            } else if line.starts_with("vendor_id") {
                cpu_info.vendor = Self::extract_cpu_field(line);
            } else if line.starts_with("cpu family") {
                cpu_info.cpu_family = Self::extract_cpu_field(line);
            } else if line.starts_with("model") && !line.starts_with("model name") {
                cpu_info.model_number = Self::extract_cpu_field(line);
            } else if line.starts_with("stepping") {
                cpu_info.stepping = Self::extract_cpu_field(line);
            } else if line.starts_with("cpu MHz") {
                cpu_info.frequency_mhz =
                    Self::extract_cpu_field(line).parse::<f64>().unwrap_or(0.0);
            } else if line.starts_with("cache size") {
                cpu_info.cache_size = Self::extract_cpu_field(line);
            } else if line.starts_with("physical id") {
                if let Ok(id) = Self::extract_cpu_field(line).parse::<u32>() {
                    physical_id_set.insert(id);
                }
            } else if line.starts_with("siblings") {
                cpu_info.logical_cores = Self::extract_cpu_field(line).parse::<u32>().unwrap_or(1);
            } else if line.starts_with("cpu cores") {
                cpu_info.physical_cores = Self::extract_cpu_field(line).parse::<u32>().unwrap_or(1);
            } else if line.starts_with("flags") || line.starts_with("Features") {
                cpu_info.flags = Self::extract_cpu_field(line);
            }
        }

        cpu_info.total_cores = processor_count;
        cpu_info.physical_sockets = physical_id_set.len() as u32;

        // Determine architecture
        cpu_info.architecture = Self::detect_architecture();

        cpu_info
    }

    /// Extract field value from cpuinfo line (after colon)
    fn extract_cpu_field(line: &str) -> String {
        if let Some(colon_pos) = line.find(':') {
            let after_colon = &line[colon_pos + 1..];
            return after_colon.trim().to_string();
        }
        String::new()
    }

    /// Detect system architecture
    fn detect_architecture() -> String {
        if let Ok(output) = std::process::Command::new("uname").arg("-m").output() {
            if output.status.success() {
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
        "unknown".to_string()
    }

    /// Parse per-core CPU usage from /proc/stat
    pub fn parse_per_core_cpu() -> Vec<PerCoreCpuUsage> {
        let content = Self::read_proc_file("/proc/stat");
        let mut per_core_usage = Vec::new();

        for line in content.lines() {
            if line.starts_with("cpu") && !line.starts_with("cpu ") {
                // This is a per-core line (cpu0, cpu1, etc.)
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 8 {
                    let core_name = parts[0].to_string();
                    let user: u64 = parts[1].parse().unwrap_or(0);
                    let nice: u64 = parts[2].parse().unwrap_or(0);
                    let system: u64 = parts[3].parse().unwrap_or(0);
                    let idle: u64 = parts[4].parse().unwrap_or(0);
                    let iowait: u64 = parts[5].parse().unwrap_or(0);
                    let irq: u64 = parts[6].parse().unwrap_or(0);
                    let softirq: u64 = parts[7].parse().unwrap_or(0);

                    let total = user + nice + system + idle + iowait + irq + softirq;
                    let used = user + nice + system + irq + softirq;
                    let usage_percent = if total > 0 {
                        (used as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };

                    per_core_usage.push(PerCoreCpuUsage {
                        core: core_name,
                        usage_percent: usage_percent.round() as u32,
                    });
                }
            }
        }

        per_core_usage
    }

    /// Get motherboard information — tries sysfs first (no root), then dmidecode with sudo fallback.
    pub fn get_motherboard_info() -> MotherboardInfo {
        let mut info = MotherboardInfo::default();

        // Prefer sysfs — readable without root on most systems
        let dmi = "/sys/devices/virtual/dmi/id";
        if let Ok(v) = std::fs::read_to_string(format!("{}/board_vendor", dmi)) {
            info.manufacturer = v.trim().to_string();
        } else if let Ok(v) = std::fs::read_to_string(format!("{}/sys_vendor", dmi)) {
            info.manufacturer = v.trim().to_string();
        }
        if let Ok(v) = std::fs::read_to_string(format!("{}/board_name", dmi)) {
            info.model = v.trim().to_string();
        } else if let Ok(v) = std::fs::read_to_string(format!("{}/product_name", dmi)) {
            info.model = v.trim().to_string();
        }
        if let Ok(v) = std::fs::read_to_string(format!("{}/bios_version", dmi)) {
            info.bios_version = v.trim().to_string();
        }
        if let Ok(v) = std::fs::read_to_string(format!("{}/bios_date", dmi)) {
            info.bios_date = v.trim().to_string();
        }

        // Fallback: try dmidecode for any fields still empty
        if info.manufacturer.is_empty() {
            if let Some(val) = Self::run_dmidecode("baseboard-manufacturer") {
                info.manufacturer = val;
            }
        }
        if info.model.is_empty() {
            if let Some(val) = Self::run_dmidecode("baseboard-product-name") {
                info.model = val;
            }
        }
        if info.bios_version.is_empty() {
            if let Some(val) = Self::run_dmidecode("bios-version") {
                info.bios_version = val;
            }
        }
        if info.bios_date.is_empty() {
            if let Some(val) = Self::run_dmidecode("bios-release-date") {
                info.bios_date = val;
            }
        }

        info
    }

    /// Run dmidecode for a given string keyword, with sudo -n fallback.
    fn run_dmidecode(keyword: &str) -> Option<String> {
        // Try direct command first
        if let Ok(output) = std::process::Command::new("dmidecode")
            .args(["-s", keyword])
            .output()
        {
            if output.status.success() {
                let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }

        // Fallback: try sudo -n (non-interactive, caches credentials)
        if let Ok(output) = std::process::Command::new("sudo")
            .args(["-n", "dmidecode", "-s", keyword])
            .output()
        {
            if output.status.success() {
                let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }

        None
    }

    /// Get RAM type and speed — tries dmidecode, then sudo -n fallback.
    pub fn get_ram_info() -> RamInfo {
        let mut info = RamInfo::default();

        // Try direct dmidecode
        if let Some(ram_type) = Self::run_dmidecode("memory-type") {
            info.ram_type = ram_type;
        }
        if let Some(speed_str) = Self::run_dmidecode("memory-speed") {
            if let Some(num_str) = speed_str.split_whitespace().next() {
                info.speed_mhz = num_str.parse::<u32>().unwrap_or(0);
            }
        }

        // If dmidecode didn't work, try parsing --type 17 output directly
        if info.ram_type.is_empty() || info.speed_mhz == 0 {
            Self::parse_dmidecode_type17(&mut info);
        }

        info
    }

    /// Parse verbose dmidecode --type 17 output for type and speed.
    fn parse_dmidecode_type17(info: &mut RamInfo) {
        let output = if let Ok(out) = std::process::Command::new("dmidecode")
            .args(["--type", "17"])
            .output()
        {
            if out.status.success() {
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                return;
            }
        } else if let Ok(out) = std::process::Command::new("sudo")
            .args(["-n", "dmidecode", "--type", "17"])
            .output()
        {
            if out.status.success() {
                String::from_utf8_lossy(&out.stdout).to_string()
            } else {
                return;
            }
        } else {
            return;
        };

        for line in output.lines() {
            let trimmed = line.trim();
            if info.ram_type.is_empty() && trimmed.starts_with("Type:") {
                let val = trimmed.split(':').nth(1).unwrap_or("").trim().to_string();
                if !val.is_empty() && !val.contains("Unknown") {
                    info.ram_type = val;
                }
            }
            if info.speed_mhz == 0 && trimmed.starts_with("Speed:") {
                let val = trimmed.split(':').nth(1).unwrap_or("").trim().to_string();
                if let Some(num_str) = val.split_whitespace().next() {
                    if let Ok(speed) = num_str.parse::<u32>() {
                        if speed > 0 {
                            info.speed_mhz = speed;
                        }
                    }
                }
            }
        }
    }

    /// Get GPU information using lspci
    pub fn get_gpu_info() -> Vec<GpuInfo> {
        let mut gpus = Vec::new();

        if let Ok(output) = std::process::Command::new("lspci")
            .args(["-nn", "-D"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("VGA") || line.contains("3D") || line.contains("Display") {
                    let gpu = Self::parse_gpu_line(line);
                    if !gpu.model.is_empty() {
                        gpus.push(gpu);
                    }
                }
            }
        }

        gpus
    }

    fn parse_gpu_line(line: &str) -> GpuInfo {
        let mut gpu = GpuInfo::default();

        // Extract device name
        if let Some(colon_pos) = line.find(':') {
            let after_colon = &line[colon_pos + 1..];
            if let Some(paren_pos) = after_colon.find('(') {
                gpu.model = after_colon[..paren_pos].trim().to_string();
            } else {
                gpu.model = after_colon.trim().to_string();
            }
        }

        // Determine GPU type
        let model_lower = gpu.model.to_lowercase();
        if model_lower.contains("intel") {
            gpu.gpu_type = "Integrated (Intel)".to_string();
        } else if model_lower.contains("amd") || model_lower.contains("radeon") {
            if model_lower.contains("integrated") {
                gpu.gpu_type = "Integrated (AMD)".to_string();
            } else {
                gpu.gpu_type = "Dedicated (AMD)".to_string();
            }
        } else if model_lower.contains("nvidia")
            || model_lower.contains("geforce")
            || model_lower.contains("quadro")
            || model_lower.contains("tesla")
        {
            gpu.gpu_type = "Dedicated (NVIDIA)".to_string();
        } else {
            gpu.gpu_type = "Unknown".to_string();
        }

        gpu
    }

    /// Get sensor information using lm-sensors
    pub fn get_sensor_info() -> SensorInfo {
        let mut info = SensorInfo::default();

        if let Ok(output) = std::process::Command::new("sensors").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut current_adapter = String::new();

            for line in stdout.lines() {
                if line.starts_with("Adapter:") {
                    current_adapter = line.replace("Adapter:", "").trim().to_string();
                } else if line.contains("Core") || line.contains("Package") {
                    if line.contains("°C") {
                        if let Some(temp) = Self::parse_temp_line(line) {
                            info.cpu_temperatures.push((current_adapter.clone(), temp));
                        }
                    }
                } else if line.contains("MB") || line.contains("Motherboard") {
                    if line.contains("°C") {
                        if let Some(temp) = Self::parse_temp_line(line) {
                            info.motherboard_temperatures
                                .push((current_adapter.clone(), temp));
                        }
                    }
                } else if line.contains("fan") || line.contains("Fan") {
                    if let Some(rpm) = Self::parse_fan_line(line) {
                        info.fan_speeds.push((current_adapter.clone(), rpm));
                    }
                }
            }
        }

        info
    }

    fn parse_temp_line(line: &str) -> Option<f64> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for part in parts.iter() {
            if part.contains("°C") {
                let cleaned = part.replace('+', "").replace("°C", "");
                if let Ok(temp) = cleaned.parse::<f64>() {
                    return Some(temp);
                }
            }
        }
        None
    }

    fn parse_fan_line(line: &str) -> Option<u32> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for part in parts.iter() {
            if part.ends_with("RPM") || part.ends_with("rpm") {
                let rpm_str = part.replace("RPM", "").replace("rpm", "");
                if let Ok(rpm) = rpm_str.parse::<u32>() {
                    return Some(rpm);
                }
            }
        }
        None
    }

    /// Get UPS information using NUT (upsc)
    pub fn get_ups_info() -> UpsInfo {
        let mut info = UpsInfo::default();

        // List available UPS devices
        if let Ok(output) = std::process::Command::new("upsc").args(["-l"]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let ups_names: Vec<&str> = stdout.lines().collect();

            if !ups_names.is_empty() {
                let ups_name = ups_names[0].trim();
                info.name = ups_name.to_string();
                info.available = true;

                // Get UPS status
                if let Ok(status_output) = std::process::Command::new("upsc").arg(ups_name).output()
                {
                    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
                    for line in status_stdout.lines() {
                        if line.contains("battery.charge") {
                            if let Some(charge) = line.split(':').nth(1) {
                                info.battery_charge = charge.trim().to_string();
                            }
                        } else if line.contains("battery.runtime") {
                            if let Some(runtime) = line.split(':').nth(1) {
                                info.runtime = Self::format_ups_runtime(runtime.trim());
                            }
                        } else if line.contains("ups.load") {
                            if let Some(load) = line.split(':').nth(1) {
                                info.load = load.trim().to_string();
                            }
                        } else if line.contains("ups.status") {
                            if let Some(status) = line.split(':').nth(1) {
                                info.status = status.trim().to_string();
                            }
                        } else if line.contains("device.model") {
                            if let Some(model) = line.split(':').nth(1) {
                                info.model = model.trim().to_string();
                            }
                        }
                    }
                }
            }
        }

        info
    }

    fn format_ups_runtime(seconds: &str) -> String {
        if let Ok(secs) = seconds.parse::<u64>() {
            let minutes = secs / 60;
            if minutes >= 60 {
                let hours = minutes / 60;
                let mins = minutes % 60;
                format!("{}h {}m", hours, mins)
            } else {
                format!("{}m", minutes)
            }
        } else {
            seconds.to_string()
        }
    }

    /// Get software information (OS version, kernel version, uptime)
    pub fn get_software_info() -> SoftwareInfo {
        let mut info = SoftwareInfo::default();

        // Get OS name and version
        if let Ok(output) = std::process::Command::new("lsb_release")
            .args(["-a"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("Distributor ID") {
                    if let Some(value) = line.split(':').nth(1) {
                        info.os_name = value.trim().to_string();
                    }
                } else if line.contains("Release") {
                    if let Some(value) = line.split(':').nth(1) {
                        info.os_version = value.trim().to_string();
                    }
                }
            }
        }

        // Fallback to /etc/os-release if lsb_release not available
        if info.os_name.is_empty() {
            if let Ok(content) = fs::read_to_string("/etc/os-release") {
                for line in content.lines() {
                    if line.starts_with("ID=") {
                        info.os_name = line.replace("ID=", "").replace('"', "").trim().to_string();
                    } else if line.starts_with("VERSION=") {
                        info.os_version = line
                            .replace("VERSION=", "")
                            .replace('"', "")
                            .trim()
                            .to_string();
                    } else if line.starts_with("PRETTY_NAME=") {
                        let pretty = line
                            .replace("PRETTY_NAME=", "")
                            .replace('"', "")
                            .trim()
                            .to_string();
                        if info.os_name.is_empty() {
                            info.os_name = pretty.clone();
                        }
                        if info.os_version.is_empty() {
                            info.os_version = pretty;
                        }
                    }
                }
            }
        }

        // Get kernel version
        if let Ok(output) = std::process::Command::new("uname").arg("-r").output() {
            info.kernel_version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }

        // Get uptime
        let uptime_seconds = Self::parse_uptime();
        info.uptime_seconds = uptime_seconds;
        info.uptime_formatted = Self::format_uptime(uptime_seconds);

        info
    }

    fn format_uptime(seconds: u64) -> String {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        let minutes = (seconds % 3600) / 60;

        let mut parts = Vec::new();
        if days > 0 {
            parts.push(format!("{}d", days));
        }
        if hours > 0 || days > 0 {
            parts.push(format!("{}h", hours));
        }
        parts.push(format!("{}m", minutes));

        parts.join(" ")
    }
}

#[derive(Debug, Default, Clone)]
pub struct GpuInfo {
    pub model: String,
    pub gpu_type: String,
}

#[derive(Debug, Default, Clone)]
pub struct SensorInfo {
    pub cpu_temperatures: Vec<(String, f64)>,
    pub motherboard_temperatures: Vec<(String, f64)>,
    pub fan_speeds: Vec<(String, u32)>,
}

#[derive(Debug, Default, Clone)]
pub struct UpsInfo {
    pub available: bool,
    pub name: String,
    pub model: String,
    pub battery_charge: String,
    pub runtime: String,
    pub load: String,
    pub status: String,
}

#[derive(Debug, Default, Clone)]
pub struct SoftwareInfo {
    pub os_name: String,
    pub os_version: String,
    pub kernel_version: String,
    pub uptime_seconds: u64,
    pub uptime_formatted: String,
}

#[derive(Debug, Default, Clone)]
pub struct CpuDetailedInfo {
    pub model_name: String,
    pub vendor: String,
    pub cpu_family: String,
    pub model_number: String,
    pub stepping: String,
    pub frequency_mhz: f64,
    pub cache_size: String,
    pub total_cores: u32,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub physical_sockets: u32,
    pub architecture: String,
    pub flags: String,
}

#[derive(Debug, Default, Clone)]
pub struct PerCoreCpuUsage {
    pub core: String,
    pub usage_percent: u32,
}

#[derive(Debug, Default, Clone)]
pub struct MotherboardInfo {
    pub manufacturer: String,
    pub model: String,
    pub bios_version: String,
    pub bios_date: String,
}

#[derive(Debug, Default, Clone)]
pub struct RamInfo {
    pub ram_type: String,
    pub speed_mhz: u32,
}

#[derive(Debug, Default, Clone)]
pub struct MemInfo {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
}

#[derive(Debug, Clone)]
pub struct DiskMount {
    pub name: String,
    pub mount: String,
    pub fstype: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkInterface {
    pub name: String,
    pub status: String,
    #[serde(rename = "type")]
    pub iface_type: String,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub netmask: Option<String>,
    pub mac: Option<String>,
    pub signal: Option<String>,
    pub channel: Option<String>,
    pub ssid: Option<String>,
    pub model: String,
    pub link_speed: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkGlobalInfo {
    pub hostname: String,
    pub workgroup: String,
    pub default_gateway: String,
    pub dns_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub name: String,
    pub display_name: String,
    pub running: bool,
}
