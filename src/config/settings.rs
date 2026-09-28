use serde::{Deserialize, Serialize};
use std::env;

/// Global application settings, loaded from environment variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// HTTP listen port (default: 8080, env: `PORT`).
    pub port: u16,
    /// Log level (default: `"info"`, env: `LOG_LEVEL`).
    pub log_level: String,
    /// Base directory for DB and data files (env: `DATA_DIRECTORY`).
    pub data_directory: String,
    /// Max simultaneous connections (env: `MAX_CONNECTIONS`).
    pub max_connections: usize,
    /// Session TTL in seconds (env: `SESSION_LIFETIME`).
    pub session_lifetime: u64,
    /// Read timeout in seconds.
    pub read_timeout: u64,
    /// Write timeout in seconds.
    pub write_timeout: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: 8080,
            log_level: "info".to_string(),
            data_directory: "/var/lib/homevault".to_string(),
            max_connections: 100,
            session_lifetime: 86400,
            read_timeout: 30,
            write_timeout: 30,
        }
    }
}

impl Settings {
    /// Load settings from environment variables, falling back to defaults.
    pub fn load() -> Self {
        let mut settings = Self::default();

        if let Ok(port) = env::var("PORT") {
            if let Ok(p) = port.parse::<u16>() {
                settings.port = p;
            }
        }

        if let Ok(log_level) = env::var("LOG_LEVEL") {
            settings.log_level = log_level;
        }

        if let Ok(data_dir) = env::var("DATA_DIRECTORY") {
            settings.data_directory = data_dir;
        }

        if let Ok(max_conn) = env::var("MAX_CONNECTIONS") {
            if let Ok(mc) = max_conn.parse::<usize>() {
                settings.max_connections = mc;
            }
        }

        if let Ok(session_lifetime) = env::var("SESSION_LIFETIME") {
            if let Ok(sl) = session_lifetime.parse::<u64>() {
                settings.session_lifetime = sl;
            }
        }

        settings
    }

    /// Check whether the host OS is a supported Debian or Ubuntu variant.
    pub fn is_supported_os() -> bool {
        if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
            return os_release.contains("debian") || os_release.contains("ubuntu");
        }
        false
    }

    /// Check whether the CPU architecture is supported (x86_64, aarch64, arm).
    pub fn is_supported_architecture() -> bool {
        let arch = std::env::consts::ARCH;
        matches!(arch, "x86_64" | "aarch64" | "arm")
    }

    /// Return detailed information about the host OS and architecture.
    pub fn get_architecture_info() -> ArchitectureInfo {
        ArchitectureInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            is_64bit: std::env::consts::ARCH == "x86_64" || std::env::consts::ARCH == "aarch64",
            arm_version: if std::env::consts::ARCH == "arm" {
                Some("v7".to_string())
            } else if std::env::consts::ARCH == "aarch64" {
                Some("v8".to_string())
            } else {
                None
            },
            distro: Self::get_distro_name(),
        }
    }

    fn get_distro_name() -> String {
        if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
            for line in os_release.lines() {
                if line.starts_with("ID=") {
                    return line.trim_start_matches("ID=").trim_matches('"').to_string();
                }
            }
        }
        "unknown".to_string()
    }
}

/// Information about the host OS, architecture, and distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureInfo {
    pub os: String,
    pub arch: String,
    pub is_64bit: bool,
    pub arm_version: Option<String>,
    /// Lowercase distro ID (e.g. `"ubuntu"`, `"debian"`).
    pub distro: String,
}
