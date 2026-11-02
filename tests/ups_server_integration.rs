//! Integration tests for UPS server mode

use homevault::ups_server::{UpsServerConfig, UpsServerService, ShutdownPolicy};
use std::collections::HashMap;

#[tokio::test]
async fn test_ups_server_availability() {
    // Test if upsd is available on the system
    let available = UpsServerService::is_available().await;

    // This test documents availability, not requires it
    println!("UPS server (upsd) available: {}", available);
}

#[test]
fn test_validate_config_accepts_valid() {
    let config = UpsServerConfig {
        enabled: true,
        bind_interface: "eth0".to_string(),
        bind_address: Some("192.168.1.1".parse().unwrap()),
        port: 3493,
        allowed_clients: vec![
            homevault::ups_server::UpsClient {
                name: "client1".to_string(),
                address: "192.168.1.100".to_string(),
                username: "monitor".to_string(),
                password_hash: "hashed".to_string(),
                master: false,
            },
        ],
        shutdown_policy: ShutdownPolicy::default(),
        max_clients: 10,
    };

    let result = UpsServerService::validate_config(&config);
    assert!(result.is_ok());
}

/// Non-vacuity: enabling the server with no allowed client is refused, so the
/// config that passes validation in the test above is passing for a real reason
/// (the guard is "at least one client when enabled", not a `confirmed` flag the
/// service no longer carries).
#[test]
fn test_validate_config_requires_a_client_when_enabled() {
    let mut config = UpsServerConfig::default();
    config.enabled = true;
    config.allowed_clients.clear();

    let result = UpsServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("client"));
}

#[test]
fn test_validate_config_rejects_empty_interface() {
    let mut config = UpsServerConfig::default();
    config.bind_interface = "".to_string();

    let result = UpsServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("interface"));
}

#[test]
fn test_validate_config_rejects_reserved_ports() {
    let reserved_ports = [22, 80, 443, 8443];

    for port in &reserved_ports {
        let mut config = UpsServerConfig::default();
        config.port = *port;

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reserved"));
    }
}

#[test]
fn test_validate_config_requires_clients_when_enabled() {
    let mut config = UpsServerConfig::default();
    config.enabled = true;
    config.allowed_clients.clear();

    let result = UpsServerService::validate_config(&config);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("at least one client"));
}

#[test]
fn test_validate_client_name_format() {
    use homevault::ups_server::UpsClient;

    let valid_clients = vec![
        UpsClient {
            name: "client1".to_string(),
            address: "192.168.1.100".to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        },
        UpsClient {
            name: "test-client_2".to_string(),
            address: "192.168.1.101".to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        },
    ];

    for client in valid_clients {
        let mut config = UpsServerConfig::default();
        config.enabled = true;
        config.allowed_clients = vec![client];

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_ok());
    }
}

#[test]
fn test_validate_client_rejects_invalid_address() {
    use homevault::ups_server::UpsClient;

    let invalid_addresses = vec!["invalid", "999.999.999.999", "not-an-ip"];

    for addr in invalid_addresses {
        let mut config = UpsServerConfig::default();
        config.enabled = true;
        config.allowed_clients = vec![UpsClient {
            name: "client1".to_string(),
            address: addr.to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        }];

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid address"));
    }
}

#[test]
fn test_default_config_is_disabled() {
    let config = UpsServerConfig::default();

    assert!(!config.enabled);
    assert_eq!(config.port, 3493);
    assert_eq!(config.max_clients, 10);
    assert!(config.allowed_clients.is_empty());
}

#[test]
fn test_default_shutdown_policy() {
    let policy = ShutdownPolicy::default();

    assert!(policy.stop_containers);
    assert!(policy.shutdown_vms);
    assert!(policy.flush_restic);
    assert!(policy.stop_shares);
}

#[tokio::test]
async fn test_server_state_when_not_running() {
    let state = UpsServerService::get_state().await;

    // State should be obtainable even when not running
    assert!(!state.shutdown_in_progress);
    assert!(state.shutdown_stage.is_none());
}

#[test]
fn test_max_clients_validation() {
    let invalid_values = vec![0, 101, 200];

    for max_clients in invalid_values {
        let mut config = UpsServerConfig::default();
        config.max_clients = max_clients;

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("between 1 and 100"));
    }
}

#[test]
fn test_validate_client_username_special_chars() {
    use homevault::ups_server::UpsClient;

    let invalid_usernames = vec![
        "user name",  // space
        "user:name",  // colon
        "user=name",  // equals
        "user[name]", // brackets
    ];

    for username in invalid_usernames {
        let mut config = UpsServerConfig::default();
        config.enabled = true;
        config.allowed_clients = vec![UpsClient {
            name: "client1".to_string(),
            address: "192.168.1.100".to_string(),
            username: username.to_string(),
            password_hash: "".to_string(),
            master: false,
        }];

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid characters"));
    }
}
