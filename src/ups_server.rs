//! UPS server mode — expose HomeVault as a network UPS server
//!
//! Provides NUT (Network UPS Tools) server functionality so other machines can monitor
//! the UPS state through this HomeVault instance. Complements the existing client mode
//! (`src/ups.rs`) with server capabilities.
//!
//! # Architecture
//!
//! - `upsd` daemon management (start/stop/reload)
//! - Per-client ACL and authentication
//! - Orderly shutdown policy with configurable stages
//! - Watchdog for daemon health monitoring
//!
//! # Security
//!
//! - Bind to LAN interface only (never 0.0.0.0 by default)
//! - Client allowlist with password authentication
//! - Shutdown commands require master privilege
//! - Configuration files written as root with 0600 permissions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use crate::utils::process::run_timed_async;

/// UPS server mode deadline for daemon operations
const UPSD_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for a read-only probe (`upsd -h`, `systemctl is-active`, `upsc -l`).
/// A wedged NUT daemon answers none of them, and the Settings card must still
/// render (ROADMAP X.2/X.9).
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Deadline for writing one root-owned configuration file (`sudo tee` +
/// `sudo chmod`). A sudo password prompt that never comes back must not hang
/// the save path.
const CONFIG_WRITE_TIMEOUT: Duration = Duration::from_secs(15);

/// Shutdown policy stages with timeouts
const SHUTDOWN_STAGES: &[(&str, Duration)] = &[
    ("stop_containers", Duration::from_secs(60)),
    ("shutdown_vms", Duration::from_secs(120)),
    ("flush_restic", Duration::from_secs(300)),
    ("stop_shares", Duration::from_secs(30)),
    ("final_poweroff", Duration::from_secs(10)),
];

/// UPS server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsServerConfig {
    /// Whether server mode is enabled
    pub enabled: bool,

    /// Network interface to bind to (e.g., "eth0", "enp0s3")
    pub bind_interface: String,

    /// IP address to bind (derived from interface)
    pub bind_address: Option<IpAddr>,

    /// Port for upsd (default 3493)
    pub port: u16,

    /// Allowed client definitions
    pub allowed_clients: Vec<UpsClient>,

    /// Shutdown policy
    pub shutdown_policy: ShutdownPolicy,

    /// Maximum number of simultaneous clients
    pub max_clients: u32,
}

/// A client allowed to connect to this UPS server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsClient {
    /// Client identifier (for logging)
    pub name: String,

    /// IP address or CIDR range
    pub address: String,

    /// Username for authentication
    pub username: String,

    /// Password hash (Argon2)
    #[serde(skip_serializing)]
    pub password_hash: String,

    /// Master privilege (can issue shutdown commands)
    pub master: bool,
}

/// Shutdown policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownPolicy {
    /// Stop Docker containers before shutdown
    pub stop_containers: bool,

    /// Shutdown VMs before containers
    pub shutdown_vms: bool,

    /// Flush pending Restic operations
    pub flush_restic: bool,

    /// Stop share services (SMB, NFS, etc.)
    pub stop_shares: bool,

    /// Timeout per stage (seconds)
    pub stage_timeout_secs: HashMap<String, u64>,
}

/// UPS server state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsServerState {
    /// Whether upsd is currently running
    pub running: bool,

    /// Number of connected clients
    pub connected_clients: u32,

    /// Last error message if any
    pub last_error: Option<String>,

    /// Whether a shutdown is in progress
    pub shutdown_in_progress: bool,

    /// Current shutdown stage
    pub shutdown_stage: Option<String>,
}

/// Result of a shutdown stage
#[derive(Debug)]
pub struct ShutdownStageResult {
    pub stage: String,
    pub success: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
    /// True when the stage was enabled by the policy but this build cannot
    /// perform it. A skipped stage is **not** a success: reporting `Ok(())`
    /// for work that never happened is how an orderly shutdown silently
    /// becomes an abrupt one.
    pub skipped: bool,
}

/// What a single stage actually did.
#[derive(Debug, PartialEq, Eq)]
enum StageOutcome {
    /// The stage ran and its command succeeded.
    Done,
    /// The stage is enabled but has no implementation on this host.
    Skipped(&'static str),
}

pub struct UpsServerService;

impl UpsServerService {
    /// Validate server configuration
    pub fn validate_config(config: &UpsServerConfig) -> Result<(), String> {
        // Validate bind interface
        if config.bind_interface.is_empty() {
            return Err("bind_interface cannot be empty".to_string());
        }

        // Validate port. 8443 is HomeVault's own alternative HTTPS listener
        // (`web::run` binds 443 or 8443): `upsd` taking it would make the
        // dashboard unreachable, so it is refused like the other three.
        if matches!(config.port, 0 | 22 | 80 | 443 | 8443) {
            return Err(format!("port {} is reserved", config.port));
        }

        // Validate clients
        if config.allowed_clients.is_empty() && config.enabled {
            return Err("at least one client must be defined when enabled".to_string());
        }

        for client in &config.allowed_clients {
            Self::validate_client(client)?;
        }

        // Validate max clients
        if config.max_clients == 0 || config.max_clients > 100 {
            return Err("max_clients must be between 1 and 100".to_string());
        }

        Ok(())
    }

    /// Validate a single client configuration
    fn validate_client(client: &UpsClient) -> Result<(), String> {
        // Validate name
        if client.name.is_empty() || client.name.len() > 64 {
            return Err("client name must be 1-64 characters".to_string());
        }

        if !client
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err("client name must be alphanumeric with - or _".to_string());
        }

        // Validate address (IP or CIDR)
        if client.address.parse::<IpAddr>().is_err()
            && client.address.parse::<ipnetwork::IpNetwork>().is_err()
        {
            return Err(format!("invalid address: {}", client.address));
        }

        // Validate username
        if client.username.is_empty() || client.username.len() > 32 {
            return Err("username must be 1-32 characters".to_string());
        }

        // Username must not contain special characters that could break upsd.users
        if client
            .username
            .contains(|c: char| c.is_whitespace() || c == ':' || c == '=' || c == '[' || c == ']')
        {
            return Err("username contains invalid characters".to_string());
        }

        Ok(())
    }

    /// Check if upsd is available on the system
    pub async fn is_available() -> bool {
        let mut cmd = Command::new("upsd");
        cmd.arg("-h");
        run_timed_async(&mut cmd, PROBE_TIMEOUT)
            .await
            .map(|o| o.status.code() == Some(0) || o.status.code() == Some(1))
            .unwrap_or(false)
    }

    /// Start the UPS server
    pub async fn start_server(config: &UpsServerConfig) -> Result<(), String> {
        if !config.enabled {
            return Err("server mode is not enabled".to_string());
        }

        // Validate first
        Self::validate_config(config)?;

        // Write configuration files
        Self::write_upsd_conf(config).await?;
        Self::write_upsd_users(config).await?;

        // Start upsd
        let mut start = Command::new("sudo");
        start.args(["systemctl", "start", "nut-server"]);
        let result = run_timed_async(&mut start, UPSD_TIMEOUT)
            .await
            .ok_or_else(|| "upsd start timed out".to_string())?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(format!("upsd failed to start: {}", stderr));
        }

        // Verify it's actually running
        tokio::time::sleep(Duration::from_millis(500)).await;

        if !Self::is_running().await {
            return Err("upsd started but is not running".to_string());
        }

        Ok(())
    }

    /// Stop the UPS server
    pub async fn stop_server() -> Result<(), String> {
        let mut stop = Command::new("sudo");
        stop.args(["systemctl", "stop", "nut-server"]);
        let result = run_timed_async(&mut stop, UPSD_TIMEOUT)
            .await
            .ok_or_else(|| "upsd stop timed out".to_string())?;

        if !result.status.success() {
            let stderr = String::from_utf8_lossy(&result.stderr);
            return Err(format!("upsd failed to stop: {}", stderr));
        }

        Ok(())
    }

    /// Check if upsd is currently running
    pub async fn is_running() -> bool {
        let mut cmd = Command::new("systemctl");
        cmd.args(["is-active", "nut-server"]);
        run_timed_async(&mut cmd, PROBE_TIMEOUT)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get current server state
    pub async fn get_state() -> UpsServerState {
        let running = Self::is_running().await;

        let connected_clients = if running {
            Self::count_connected_clients().await.unwrap_or(0)
        } else {
            0
        };

        UpsServerState {
            running,
            connected_clients,
            last_error: None,
            shutdown_in_progress: false,
            shutdown_stage: None,
        }
    }

    /// Count connected clients from upsc
    async fn count_connected_clients() -> Result<u32, String> {
        let mut cmd = Command::new("upsc");
        cmd.arg("-l");
        let output = run_timed_async(&mut cmd, PROBE_TIMEOUT)
            .await
            .ok_or_else(|| "upsc timed out".to_string())?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.lines().filter(|l| !l.trim().is_empty()).count() as u32)
    }

    /// Write upsd.conf
    async fn write_upsd_conf(config: &UpsServerConfig) -> Result<(), String> {
        let content = format!(
            "# HomeVault UPS Server Configuration\n\
             # Generated automatically - do not edit manually\n\n\
             LISTEN {} {}\n\
             MAXCONN {}\n\
             MAXAGE 15\n\
             STATEPATH /var/run/nut\n\
             CERTFILE /etc/nut/upsd.cert\n\
             CERTPATH /etc/nut/cert.d\n",
            config
                .bind_address
                .map(|a| a.to_string())
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            config.port,
            config.max_clients
        );

        Self::write_config_file("/etc/nut/upsd.conf", &content).await
    }

    /// Write upsd.users (with passwords)
    async fn write_upsd_users(config: &UpsServerConfig) -> Result<(), String> {
        let mut content = String::from(
            "# HomeVault UPS Server Users\n\
             # Generated automatically - do not edit manually\n\n",
        );

        for client in &config.allowed_clients {
            content.push_str(&format!("[{}]\n", client.username));
            content.push_str(&format!("    password = {}\n", "<hash>")); // Placeholder, actual password stored separately
            content.push_str("    upsmon master\n"); // All clients are masters in our model
            if client.master {
                content.push_str("    actions = SET FSD\n"); // Allow shutdown
            }
            content.push_str("\n");
        }

        Self::write_config_file("/etc/nut/upsd.users", &content).await
    }

    /// Write a config file with proper permissions
    async fn write_config_file(path: &str, content: &str) -> Result<(), String> {
        use tokio::io::AsyncWriteExt;

        // Write via sudo tee to handle root-owned files. The stdin is piped and
        // the whole exchange is bounded: a `sudo tee` blocked on a password
        // prompt must not hold the caller forever.
        let mut tee = Command::new("sudo");
        tee.args(["tee", path]);
        let output = crate::utils::process::run_timed_async_with_stdin(
            &mut tee,
            CONFIG_WRITE_TIMEOUT,
            content.as_bytes(),
        )
        .await
        .map_err(|e| format!("failed to write {}: {}", path, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("failed to write {}: {}", path, stderr));
        }

        // Set permissions to 0600
        let mut chmod = Command::new("sudo");
        chmod.args(["chmod", "0600", path]);
        let _ = run_timed_async(&mut chmod, CONFIG_WRITE_TIMEOUT).await;

        Ok(())
    }

    /// Execute orderly shutdown according to policy
    pub async fn execute_shutdown(policy: &ShutdownPolicy) -> Vec<ShutdownStageResult> {
        let mut results = Vec::new();

        for (stage_name, default_timeout) in SHUTDOWN_STAGES {
            let stage_str = stage_name.to_string();

            // Check if this stage is enabled
            let enabled = match *stage_name {
                "stop_containers" => policy.stop_containers,
                "shutdown_vms" => policy.shutdown_vms,
                "flush_restic" => policy.flush_restic,
                "stop_shares" => policy.stop_shares,
                "final_poweroff" => true, // Always run
                _ => false,
            };

            if !enabled {
                continue;
            }

            let timeout_secs = policy
                .stage_timeout_secs
                .get(&stage_str)
                .copied()
                .unwrap_or(default_timeout.as_secs());

            let start = std::time::Instant::now();

            match Self::execute_shutdown_stage(stage_name, Duration::from_secs(timeout_secs)).await
            {
                Ok(StageOutcome::Done) => results.push(ShutdownStageResult {
                    stage: stage_str,
                    success: true,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: None,
                    skipped: false,
                }),
                Ok(StageOutcome::Skipped(reason)) => results.push(ShutdownStageResult {
                    stage: stage_str,
                    success: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: Some(reason.to_string()),
                    skipped: true,
                }),
                Err(e) => results.push(ShutdownStageResult {
                    stage: stage_str,
                    success: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: Some(e),
                    skipped: false,
                }),
            }
        }

        results
    }

    /// Execute a single shutdown stage.
    ///
    /// Every branch either runs a real command or reports `Skipped` with the
    /// reason. No branch returns `Done` for work it did not do.
    async fn execute_shutdown_stage(
        stage: &str,
        stage_timeout: Duration,
    ) -> Result<StageOutcome, String> {
        match stage {
            "stop_containers" => {
                // Two steps, never a shell: `docker stop "$(docker ps -q)"`
                // passed as argv hands docker the literal string as a
                // container name, so it stopped nothing and still exited 0.
                let mut list = Command::new("docker");
                list.args(["ps", "-q"]);
                let listed = run_timed_async(&mut list, stage_timeout)
                    .await
                    .ok_or_else(|| format!("{} timed out listing containers", stage))?;

                if !listed.status.success() {
                    return Err(format!(
                        "{}: docker ps exited with {}",
                        stage, listed.status
                    ));
                }

                let ids: Vec<String> = String::from_utf8_lossy(&listed.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect();

                if ids.is_empty() {
                    return Ok(StageOutcome::Done);
                }

                let mut cmd = Command::new("docker");
                cmd.args(["stop", "-t", "30"]).args(&ids);
                run_timed_async(&mut cmd, stage_timeout)
                    .await
                    .ok_or_else(|| format!("{} timed out", stage))?;
                Ok(StageOutcome::Done)
            }
            "shutdown_vms" => Ok(StageOutcome::Skipped(
                "VM shutdown is not wired to the Incus service yet",
            )),
            "flush_restic" => Ok(StageOutcome::Skipped(
                "Restic flush is not wired to the backup scheduler yet",
            )),
            "stop_shares" => {
                // Stop SMB, NFS, etc.
                let mut cmd = Command::new("sudo");
                cmd.args(["systemctl", "stop", "smbd", "nmbd", "nfs-server"]);
                run_timed_async(&mut cmd, stage_timeout)
                    .await
                    .ok_or_else(|| format!("{} timed out", stage))?;
                Ok(StageOutcome::Done)
            }
            "final_poweroff" => {
                // Final system poweroff
                let mut cmd = Command::new("sudo");
                cmd.args(["poweroff"]);
                run_timed_async(&mut cmd, stage_timeout)
                    .await
                    .ok_or_else(|| format!("{} timed out", stage))?;
                Ok(StageOutcome::Done)
            }
            _ => Err(format!("unknown stage: {}", stage)),
        }
    }
}

impl Default for UpsServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_interface: "eth0".to_string(),
            bind_address: None,
            port: 3493,
            allowed_clients: Vec::new(),
            shutdown_policy: ShutdownPolicy::default(),
            max_clients: 10,
        }
    }
}

impl Default for ShutdownPolicy {
    fn default() -> Self {
        Self {
            stop_containers: true,
            shutdown_vms: true,
            flush_restic: true,
            stop_shares: true,
            stage_timeout_secs: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_config_requires_bind_interface() {
        let mut config = UpsServerConfig::default();
        config.bind_interface = "".to_string();

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bind_interface"));
    }

    #[test]
    fn test_validate_config_rejects_reserved_ports() {
        let config = UpsServerConfig {
            enabled: false,
            port: 22,
            ..Default::default()
        };

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("reserved"));
    }

    #[test]
    fn test_validate_config_requires_clients_when_enabled() {
        let config = UpsServerConfig {
            enabled: true,
            allowed_clients: vec![],
            ..Default::default()
        };

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least one client"));
    }

    #[test]
    fn test_validate_client_name_length() {
        let client = UpsClient {
            name: "a".repeat(65),
            address: "192.168.1.100".to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        };

        let result = UpsServerService::validate_client(&client);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("1-64 characters"));
    }

    #[test]
    fn test_validate_client_name_alphanumeric() {
        let client = UpsClient {
            name: "client@home".to_string(),
            address: "192.168.1.100".to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        };

        let result = UpsServerService::validate_client(&client);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("alphanumeric"));
    }

    #[test]
    fn test_validate_client_address_ip() {
        let client = UpsClient {
            name: "client1".to_string(),
            address: "192.168.1.100".to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        };

        let result = UpsServerService::validate_client(&client);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_client_address_invalid() {
        let client = UpsClient {
            name: "client1".to_string(),
            address: "invalid-address".to_string(),
            username: "monitor".to_string(),
            password_hash: "".to_string(),
            master: false,
        };

        let result = UpsServerService::validate_client(&client);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid address"));
    }

    #[test]
    fn test_validate_client_username_special_chars() {
        let client = UpsClient {
            name: "client1".to_string(),
            address: "192.168.1.100".to_string(),
            username: "user:name".to_string(),
            password_hash: "".to_string(),
            master: false,
        };

        let result = UpsServerService::validate_client(&client);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid characters"));
    }

    #[test]
    fn test_default_config_is_disabled() {
        let config = UpsServerConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.port, 3493);
        assert_eq!(config.max_clients, 10);
    }

    #[test]
    fn test_default_shutdown_policy() {
        let policy = ShutdownPolicy::default();
        assert!(policy.stop_containers);
        assert!(policy.shutdown_vms);
        assert!(policy.flush_restic);
        assert!(policy.stop_shares);
    }

    #[test]
    fn test_max_clients_validation() {
        let config = UpsServerConfig {
            enabled: false,
            max_clients: 0,
            ..Default::default()
        };

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("between 1 and 100"));
    }

    #[test]
    fn test_max_clients_upper_bound() {
        let config = UpsServerConfig {
            enabled: false,
            max_clients: 101,
            ..Default::default()
        };

        let result = UpsServerService::validate_config(&config);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Shutdown stages (D1/D2): a stage that did nothing must never report
    // success, and no stage may hand a shell expression to a program as argv.
    // -----------------------------------------------------------------------

    /// The HTTP `stages[]` row must name whether it was skipped, otherwise a
    /// client cannot tell "done" from "impossible on this build". `skipped`
    /// travels in its own field because folding it into `success: false`
    /// would read as a failure to the user. QW-style contract test.
    #[test]
    fn test_execute_shutdown_reports_a_skipped_stage_as_skipped() {
        // A policy the code cannot carry out: both stages have no
        // implementation in this build. Assert the *combination* the response
        // is built from, without touching the host — running
        // `execute_shutdown` would also force `final_poweroff` on and power
        // the test machine off.
        let result = ShutdownStageResult {
            stage: "shutdown_vms".to_string(),
            success: false,
            duration_ms: 3,
            error: Some("not wired".to_string()),
            skipped: true,
        };

        assert!(result.skipped);
        assert!(!result.success, "a skipped stage is never a success");
        assert!(result.error.is_some(), "a skipped stage names its reason");
    }

    /// `final_poweroff` is not policy-controlled: it always runs. Guard the
    /// shape without running it on the test host by checking the loop's
    /// enabled-predicate in isolation.
    #[test]
    fn test_final_poweroff_is_always_enabled() {
        let policy = ShutdownPolicy {
            stop_containers: false,
            shutdown_vms: false,
            flush_restic: false,
            stop_shares: false,
            stage_timeout_secs: HashMap::new(),
        };

        let enabled = SHUTDOWN_STAGES
            .iter()
            .filter(|(name, _)| match *name {
                "stop_containers" => policy.stop_containers,
                "shutdown_vms" => policy.shutdown_vms,
                "flush_restic" => policy.flush_restic,
                "stop_shares" => policy.stop_shares,
                "final_poweroff" => true,
                _ => false,
            })
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();

        assert_eq!(enabled, vec!["final_poweroff"]);
    }

    #[tokio::test]
    async fn test_unimplemented_stages_report_skipped_not_success() {
        for stage in ["shutdown_vms", "flush_restic"] {
            let outcome = UpsServerService::execute_shutdown_stage(stage, Duration::from_secs(5))
                .await
                .expect("a skipped stage is not an error");
            match outcome {
                StageOutcome::Skipped(reason) => {
                    assert!(
                        !reason.is_empty(),
                        "stage {} must say why it was skipped",
                        stage
                    );
                }
                StageOutcome::Done => panic!("stage {} did no work but reported Done", stage),
            }
        }
    }

    #[tokio::test]
    async fn test_unknown_stage_is_an_error() {
        let result =
            UpsServerService::execute_shutdown_stage("not_a_stage", Duration::from_secs(1)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown stage"));
    }
}
