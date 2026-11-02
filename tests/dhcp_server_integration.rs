//! Integration tests for DHCP/TFTP server

use homevault::dhcp_server::{DhcpServerConfig, DhcpServerService};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;

#[tokio::test]
async fn test_dhcp_server_availability() {
    let available = DhcpServerService::is_available().await;
    println!("dnsmasq available: {}", available);
}

#[test]
fn test_validate_config_valid() {
    let mut config = DhcpServerConfig::default();
    config.confirmed = true;

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_ok());
}

#[test]
fn test_validate_config_requires_interface() {
    let mut config = DhcpServerConfig::default();
    config.interface = "".to_string();

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("interface"));
}

#[test]
fn test_validate_config_range_order() {
    let mut config = DhcpServerConfig::default();
    config.dhcp_enabled = true;
    config.confirmed = true;
    config.dhcp_range_start = Ipv4Addr::new(192, 168, 1, 200);
    config.dhcp_range_end = Ipv4Addr::new(192, 168, 1, 100);

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("less than"));
}

#[test]
fn test_validate_config_same_subnet() {
    let mut config = DhcpServerConfig::default();
    config.dhcp_enabled = true;
    config.confirmed = true;
    config.dhcp_range_start = Ipv4Addr::new(192, 168, 1, 100);
    config.dhcp_range_end = Ipv4Addr::new(192, 168, 2, 200);
    config.netmask = Ipv4Addr::new(255, 255, 255, 0);

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("same subnet"));
}

#[test]
fn test_validate_lease_time_bounds() {
    let invalid_values = vec![30, 86400 * 31, 0];

    for lease_time in invalid_values {
        let mut config = DhcpServerConfig::default();
        config.lease_time_secs = lease_time;

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("between 60 and"));
    }
}

#[test]
fn test_validate_lease_time_accepts_valid() {
    let valid_values = vec![60, 3600, 86400, 86400 * 7];

    for lease_time in valid_values {
        let mut config = DhcpServerConfig::default();
        config.lease_time_secs = lease_time;
        config.confirmed = true;

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_ok());
    }
}

#[test]
fn test_validate_mac_format_valid() {
    let valid_macs = vec![
        "aa:bb:cc:dd:ee:ff",
        "AA:BB:CC:DD:EE:FF",
        "00:11:22:33:44:55",
        "01:23:45:67:89:ab",
    ];

    for mac in valid_macs {
        let mut config = DhcpServerConfig::default();
        config.dhcp_enabled = true;
        config.confirmed = true;
        config.reservations.insert(mac.to_string(), Ipv4Addr::new(192, 168, 1, 150));

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_ok(), "MAC {} should be valid", mac);
    }
}

#[test]
fn test_validate_mac_format_invalid() {
    let invalid_macs = vec![
        "invalid",
        "aa:bb:cc:dd:ee",       // too short
        "aa:bb:cc:dd:ee:ff:gg", // too long
        "zz:bb:cc:dd:ee:ff",    // invalid hex
        "aa-bb-cc-dd-ee-ff",    // wrong separator
    ];

    for mac in invalid_macs {
        let mut config = DhcpServerConfig::default();
        config.dhcp_enabled = true;
        config.confirmed = true;
        config.reservations.insert(mac.to_string(), Ipv4Addr::new(192, 168, 1, 150));

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err(), "MAC {} should be invalid", mac);
    }
}

#[test]
fn test_validate_reservation_in_range() {
    let mut config = DhcpServerConfig::default();
    config.dhcp_enabled = true;
    config.confirmed = true;
    config.dhcp_range_start = Ipv4Addr::new(192, 168, 1, 100);
    config.dhcp_range_end = Ipv4Addr::new(192, 168, 1, 200);

    // Reservation inside range - OK
    config.reservations.insert(
        "aa:bb:cc:dd:ee:ff".to_string(),
        Ipv4Addr::new(192, 168, 1, 150)
    );
    assert!(DhcpServerService::validate_config(&config).is_ok());

    // Reservation outside range - error
    config.reservations.clear();
    config.reservations.insert(
        "aa:bb:cc:dd:ee:ff".to_string(),
        Ipv4Addr::new(192, 168, 1, 50)
    );
    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("outside DHCP range"));
}

#[test]
fn test_validate_tftp_root_absolute() {
    let mut config = DhcpServerConfig::default();
    config.tftp_enabled = true;
    config.confirmed = true;
    config.tftp_root = PathBuf::from("relative/path");

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("absolute"));
}

#[test]
fn test_validate_tftp_root_no_parent_dir() {
    let mut config = DhcpServerConfig::default();
    config.tftp_enabled = true;
    config.confirmed = true;
    config.tftp_root = PathBuf::from("/srv/../etc");

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("'..'"));
}

#[test]
fn test_validate_tftp_root_forbidden_paths() {
    let forbidden = ["/", "/etc", "/bin", "/boot", "/sys"];

    for path in &forbidden {
        let mut config = DhcpServerConfig::default();
        config.tftp_enabled = true;
        config.confirmed = true;
        config.tftp_root = PathBuf::from(path);

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be in"));
    }
}

#[test]
fn test_validate_tftp_root_allows_safe_paths() {
    let safe_paths = ["/srv/tftp", "/var/tftp", "/mnt/pxe"];

    for path in &safe_paths {
        let mut config = DhcpServerConfig::default();
        config.tftp_enabled = true;
        config.confirmed = true;
        config.tftp_root = PathBuf::from(path);

        let result = DhcpServerService::validate_config(&config);
        assert!(result.is_ok(), "Path {} should be allowed", path);
    }
}

#[test]
fn test_validate_pxe_boot_file_relative() {
    let mut config = DhcpServerConfig::default();
    config.tftp_enabled = true;
    config.confirmed = true;
    config.tftp_root = PathBuf::from("/srv/tftp");
    config.pxe_boot_file = Some("/absolute/path".to_string());

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("relative"));
}

#[test]
fn test_validate_pxe_boot_file_no_parent_dir() {
    let mut config = DhcpServerConfig::default();
    config.tftp_enabled = true;
    config.confirmed = true;
    config.tftp_root = PathBuf::from("/srv/tftp");
    config.pxe_boot_file = Some("../etc/passwd".to_string());

    let result = DhcpServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("'..'"));
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
fn test_default_config() {
    let config = DhcpServerConfig::default();

    assert!(!config.dhcp_enabled);
    assert!(!config.tftp_enabled);
    assert!(!config.confirmed);
    assert_eq!(config.interface, "eth0");
    assert_eq!(config.lease_time_secs, 86400);
    assert_eq!(config.tftp_root, PathBuf::from("/srv/tftp"));
}

#[tokio::test]
async fn test_server_not_running_initially() {
    let running = DhcpServerService::is_running().await;

    // May or may not be running depending on system
    println!("dnsmasq running: {}", running);
}

#[test]
fn test_same_subnet_check() {
    // Same subnet
    assert!(DhcpServerService::same_subnet(
        Ipv4Addr::new(192, 168, 1, 10),
        Ipv4Addr::new(192, 168, 1, 20),
        Ipv4Addr::new(255, 255, 255, 0),
    ));

    // Different subnet
    assert!(!DhcpServerService::same_subnet(
        Ipv4Addr::new(192, 168, 1, 10),
        Ipv4Addr::new(192, 168, 2, 10),
        Ipv4Addr::new(255, 255, 255, 0),
    ));
}

#[test]
fn test_ip_in_range_check() {
    let start = Ipv4Addr::new(192, 168, 1, 100);
    let end = Ipv4Addr::new(192, 168, 1, 200);

    // In range
    assert!(DhcpServerService::ip_in_range(
        Ipv4Addr::new(192, 168, 1, 150),
        start,
        end
    ));

    // Boundaries
    assert!(DhcpServerService::ip_in_range(start, start, end));
    assert!(DhcpServerService::ip_in_range(end, start, end));

    // Out of range
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
