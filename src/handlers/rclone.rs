use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::process::Command;

use crate::handlers::AppStateArc;
use crate::storage::sync_job_repo::{NewSyncJob, SyncJob, SyncJobRepository};

/// Response wrapper for API responses
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: &str) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }
    }
}

/// Returns the authenticated username, or a human-readable error if the
/// session is missing/invalid. Every handler in this module returns
/// `Json<ApiResponse<T>>` directly (not `axum::response::Response`, which
/// is what `handlers::admin_guard::require_auth`/`require_admin` return) —
/// duplicated here as a plain `Result<String, String>` so each handler can
/// fold the error straight into its own `ApiResponse<T>` without changing
/// every return type. Same session-token-header convention used throughout
/// the rest of the codebase (e.g. `handlers/explorer.rs`).
async fn check_session(state: &AppStateArc, headers: &HeaderMap) -> Result<String, String> {
    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return Err("No session token provided".to_string());
    }

    match state.auth.get_username_from_token(session_token).await {
        Some(username) => Ok(username),
        None => Err("Invalid or expired session".to_string()),
    }
}

/// Same as `check_session`, but also requires the session to be an admin —
/// used for every mutating rclone endpoint and for reading the raw config
/// (which contains every remote's credentials).
async fn check_admin(state: &AppStateArc, headers: &HeaderMap) -> Result<String, String> {
    let username = check_session(state, headers).await?;
    if state.auth.is_admin(&username).await {
        Ok(username)
    } else {
        Err("Admin privileges required for this operation".to_string())
    }
}

/// Remote information from configured remotes
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Remote {
    pub name: String,
    #[serde(rename = "type")]
    pub remote_type: String,
}

/// Provider tier information
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Provider {
    pub name: String,
    pub description: String,
    pub tier: String,
}

/// Mount information
#[derive(Serialize, Debug, Clone)]
pub struct Mount {
    pub remote: String,
    pub mount_point: String,
    pub path: Option<String>,
    pub is_active: bool,
    pub pid: Option<u32>,
}

/// Request for sync operation
#[derive(Deserialize)]
pub struct SyncRequest {
    pub source_type: Option<String>, // "cloud" or "local"
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: Option<String>, // "cloud" or "local"
    pub destination: String,
    pub dest_path: Option<String>,
    pub delete: bool,
    pub dry_run: bool,
    pub update: bool,
    pub exclude: Option<String>,
}

/// Request for copy operation
#[derive(Deserialize)]
pub struct CopyRequest {
    pub source_type: Option<String>, // "cloud" or "local"
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: Option<String>, // "cloud" or "local"
    pub destination: String,
    pub dest_path: Option<String>,
    pub delete: bool,
    pub dry_run: bool,
    pub interactive: bool,
}

/// Request for mount operation. `read_only`/`allow_other`/`daemon` default
/// to `false` when omitted — a real bug found during release verification
/// had these as required fields with no default, but the frontend
/// (`api.startMount()`) only ever sends `{ remote, mount_point, path }`, so
/// every real mount request failed JSON deserialization before the handler
/// even ran. `daemon` is kept for API compatibility but no longer changes
/// behavior — see `handle_mount_start`.
#[derive(Deserialize)]
pub struct MountRequest {
    pub remote: String,
    pub mount_point: String,
    pub path: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub allow_other: bool,
    #[serde(default)]
    pub daemon: bool,
}

/// Build a source or destination path for rclone
/// For cloud: "remote:path" or "remote:"
/// For local: "/path" or "/path/subpath"
fn build_rclone_path(path_type: &str, remote: &str, sub_path: &Option<String>) -> String {
    let clean_subpath = sub_path
        .as_ref()
        .map(|p| p.trim_start_matches('/'))
        .unwrap_or("");

    if path_type == "local" {
        // Local path: /path or /path/subpath
        if clean_subpath.is_empty() {
            remote.to_string()
        } else {
            format!("{}/{}", remote.trim_end_matches('/'), clean_subpath)
        }
    } else {
        // Cloud path: remote:path or remote:
        if clean_subpath.is_empty() {
            format!("{}:", remote)
        } else {
            format!("{}:{}", remote, clean_subpath)
        }
    }
}

/// Parse rclone config file to get remotes with their types
fn parse_rclone_config() -> Option<Vec<Remote>> {
    // Standard rclone config locations
    let config_paths = [
        std::env::var("RCLONE_CONFIG").ok(),
        Some("/etc/rclone.conf".to_string()),
        Some("/root/.config/rclone/rclone.conf".to_string()),
        Some(format!(
            "{}/.config/rclone/rclone.conf",
            std::env::var("HOME").unwrap_or_default()
        )),
    ];

    for config_path in config_paths.iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(config_path) {
            let mut remotes = Vec::new();
            let mut current_section = String::new();
            let mut current_type = String::new();

            for line in content.lines() {
                let line = line.trim();

                // Skip comments and empty lines
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }

                // Parse section headers like [section_name]
                if line.starts_with('[') && line.ends_with(']') {
                    // Save previous remote if it has a type
                    if !current_section.is_empty() && !current_type.is_empty() {
                        let description = get_provider_description(&current_type);
                        remotes.push(Remote {
                            name: current_section.clone(),
                            remote_type: description,
                        });
                    }
                    current_section = line[1..line.len() - 1].to_string();
                    current_type.clear();
                    continue;
                }

                // Parse key=value pairs
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim().to_lowercase();
                    let value = value.trim();

                    if key == "type" {
                        current_type = value.to_string();
                    }
                }
            }

            // Don't forget the last remote
            if !current_section.is_empty() && !current_type.is_empty() {
                let description = get_provider_description(&current_type);
                remotes.push(Remote {
                    name: current_section,
                    remote_type: description,
                });
            }

            if !remotes.is_empty() {
                remotes.sort_by(|a, b| a.name.cmp(&b.name));
                return Some(remotes);
            }
        }
    }

    None
}

/// Get human-readable description for provider type
fn get_provider_description(provider_type: &str) -> String {
    match provider_type.to_lowercase().as_str() {
        // Tier 1 - Major Cloud Providers
        "drive" => "Google Drive".to_string(),
        "dropbox" => "Dropbox".to_string(),
        "onedrive" => "Microsoft OneDrive".to_string(),
        "box" => "Box".to_string(),
        "pcloud" => "pCloud".to_string(),
        "yandex" => "Yandex Disk".to_string(),
        "filen" => "Filen".to_string(),
        "hidrive" => "HiDrive".to_string(),
        "mailru" => "Mail.ru Cloud".to_string(),
        "opendrive" => "Opendrive".to_string(),
        "pixeldrain" => "Pixeldrain".to_string(),
        "pikpak" => "PikPak".to_string(),
        "gofile" => "Gofile".to_string(),
        "netstorage" => "Akamai NetStorage".to_string(),
        "memory" => "Memory".to_string(),
        "drime" => "Drime".to_string(),
        "filescom" => "Files.com".to_string(),
        "imagekit" => "ImageKit".to_string(),
        "shade" => "Shade".to_string(),
        // Tier 1 - Object Storage
        "s3" => "Amazon S3".to_string(),
        "b2" | "backblaze" => "Backblaze B2".to_string(),
        "azureblob" => "Azure Blob Storage".to_string(),
        "googlecloudstorage" => "Google Cloud Storage".to_string(),
        "oracleobjectstorage" => "Oracle Object Storage".to_string(),
        "storj" => "Storj".to_string(),
        "swift" => "OpenStack Swift".to_string(),
        // Tier 1 - Protocols
        "ftp" => "FTP Server".to_string(),
        "sftp" => "SFTP / SSH".to_string(),
        "webdav" => "WebDAV".to_string(),
        "alias" => "Alias".to_string(),
        "combine" => "Combine".to_string(),
        "union" => "Union".to_string(),
        "file" => "Local".to_string(),
        // Tier 1 - Crypt
        "crypt" => "Encrypted (Crypt)".to_string(),
        // Tier 2 - Stable
        "mega" => "MEGA".to_string(),
        "jottacloud" => "Jottacloud".to_string(),
        "koofr" => "Koofr".to_string(),
        "putio" => "Put.io".to_string(),
        "internxt" => "Internxt".to_string(),
        "azurefiles" => "Azure Files".to_string(),
        "wasabi" => "Wasabi".to_string(),
        "digitalocean" => "DigitalOcean Spaces".to_string(),
        "smb" | "cifs" => "SMB/CIFS".to_string(),
        "hdfs" => "HDFS".to_string(),
        "doi" => "DOI".to_string(),
        // Tier 3 - Supported
        "iclouddrive" => "iCloud Drive".to_string(),
        "premiumizeme" => "Premiumize.me".to_string(),
        "huaweidrive" => "Huawei Cloud".to_string(),
        "quatrix" => "Quatrix".to_string(),
        "ulozto" => "Uloz.to".to_string(),
        "fichier" => "Fichier".to_string(),
        "qingstor" => "QingStor".to_string(),
        "http" => "HTTP Server".to_string(),
        "seafile" => "Seafile".to_string(),
        "internetarchive" => "Internet Archive".to_string(),
        "sugarsync" => "SugarSync".to_string(),
        "zoho" => "Zoho".to_string(),
        "cloudinary" => "Cloudinary".to_string(),
        // Tier 4 - Experimental
        "nfs" => "NFS".to_string(),
        "chunker" => "Chunker".to_string(),
        "hasher" => "Hasher".to_string(),
        "compress" => "Compress".to_string(),
        "filefabric" => "FileFabric".to_string(),
        "sia" => "Sia".to_string(),
        "protondrive" => "Proton Drive".to_string(),
        "telegram" => "Telegram".to_string(),
        "hubic" => "HubiC".to_string(),
        // Tier 5 - Deprecated
        "gphotos" => "Google Photos (deprecated)".to_string(),
        "cache" => "Cache (deprecated)".to_string(),
        "sharefile" => "Sharefile (deprecated)".to_string(),
        "linkbox" => "Linkbox (deprecated)".to_string(),
        _ => format!("{} (custom)", provider_type),
    }
}

/// List all configured remotes
pub async fn handle_list_remotes(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Remote>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Check if rclone is available
    let check = Command::new("rclone").arg("version").output().await;

    if check.is_err() {
        return Json(ApiResponse::error("rclone is not installed on this system"));
    }

    // Try to parse config file directly for better type detection
    if let Some(remotes) = parse_rclone_config() {
        return Json(ApiResponse::success(remotes));
    }

    // Fallback: use rclone listremotes command
    let output = Command::new("rclone").args(["listremotes"]).output().await;

    match output {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Json(ApiResponse::error(&format!(
                    "Failed to list remotes: {}",
                    stderr
                )));
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut remotes = Vec::new();

            for line in stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                // Parse output like: gdrive:
                if let Some(name) = line.strip_suffix(':') {
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        remotes.push(Remote {
                            name,
                            remote_type: "Remote".to_string(),
                        });
                    }
                }
            }

            Json(ApiResponse::success(remotes))
        }
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to execute rclone: {}",
            e
        ))),
    }
}

/// List available provider types for adding new remotes
/// Based on rclone tiers: https://rclone.org/tiers/
pub async fn handle_list_providers(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Provider>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Tier 1: Core (Production-grade, first-class)
    // Tier 2: Stable (Well-supported, minor gaps)
    // Tier 3: Supported (Works for many uses; known caveats)
    // Tier 4: Experimental (Use with care; expect gaps/changes)
    let providers = vec![
        // Tier 1 - Cloud Storage
        Provider {
            name: "drive".to_string(),
            description: "Google Drive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "onedrive".to_string(),
            description: "Microsoft OneDrive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "dropbox".to_string(),
            description: "Dropbox".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "box".to_string(),
            description: "Box".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "pcloud".to_string(),
            description: "pCloud".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "mega".to_string(),
            description: "MEGA".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "yandex".to_string(),
            description: "Yandex Disk".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "iclouddrive".to_string(),
            description: "iCloud Drive".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "gphotos".to_string(),
            description: "Google Photos".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "filen".to_string(),
            description: "Filen".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "hidrive".to_string(),
            description: "HiDrive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "mailru".to_string(),
            description: "Mail.ru Cloud".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "opendrive".to_string(),
            description: "Opendrive".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "pixeldrain".to_string(),
            description: "Pixeldrain".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "pikpak".to_string(),
            description: "PikPak".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "gofile".to_string(),
            description: "Gofile".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "jottacloud".to_string(),
            description: "Jottacloud".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "premiumizeme".to_string(),
            description: "Premiumize.me".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "koofr".to_string(),
            description: "Koofr".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "putio".to_string(),
            description: "Put.io".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "internxt".to_string(),
            description: "Internxt".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "huaweidrive".to_string(),
            description: "Huawei Cloud".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "quatrix".to_string(),
            description: "Quatrix".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "ulozto".to_string(),
            description: "Uloz.to".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "fichier".to_string(),
            description: "Fichier".to_string(),
            tier: "Tier 3".to_string(),
        },
        // Tier 1 - Object Storage / S3-compatible
        Provider {
            name: "s3".to_string(),
            description: "Amazon S3".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "b2".to_string(),
            description: "Backblaze B2".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "azureblob".to_string(),
            description: "Azure Blob Storage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "azurefiles".to_string(),
            description: "Azure Files".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "googlecloudstorage".to_string(),
            description: "Google Cloud Storage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "oracleobjectstorage".to_string(),
            description: "Oracle Object Storage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "wasabi".to_string(),
            description: "Wasabi".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "digitalocean".to_string(),
            description: "DigitalOcean Spaces".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "storj".to_string(),
            description: "Storj".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "qingstor".to_string(),
            description: "QingStor".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "swift".to_string(),
            description: "OpenStack Swift".to_string(),
            tier: "Tier 1".to_string(),
        },
        // Tier 1 - Protocol-based
        Provider {
            name: "ftp".to_string(),
            description: "FTP Server".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "sftp".to_string(),
            description: "SFTP / SSH".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "webdav".to_string(),
            description: "WebDAV".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "http".to_string(),
            description: "HTTP Server".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "smb".to_string(),
            description: "SMB/CIFS".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "nfs".to_string(),
            description: "NFS".to_string(),
            tier: "Tier 4".to_string(),
        },
        // Tier 1 - Local & Virtual
        Provider {
            name: "file".to_string(),
            description: "Local".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "alias".to_string(),
            description: "Alias".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "crypt".to_string(),
            description: "Encrypted (Crypt)".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "cache".to_string(),
            description: "Cache".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "combine".to_string(),
            description: "Combine".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "union".to_string(),
            description: "Union".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "chunker".to_string(),
            description: "Chunker".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "hasher".to_string(),
            description: "Hasher".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "compress".to_string(),
            description: "Compress".to_string(),
            tier: "Tier 4".to_string(),
        },
        // Tier 3 - Other services
        Provider {
            name: "seafile".to_string(),
            description: "Seafile".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "internetarchive".to_string(),
            description: "Internet Archive".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "sugarsync".to_string(),
            description: "SugarSync".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "hubic".to_string(),
            description: "HubiC".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "zoho".to_string(),
            description: "Zoho".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "cloudinary".to_string(),
            description: "Cloudinary".to_string(),
            tier: "Tier 3".to_string(),
        },
        Provider {
            name: "sharefile".to_string(),
            description: "Sharefile".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "linkbox".to_string(),
            description: "Linkbox".to_string(),
            tier: "Tier 5".to_string(),
        },
        Provider {
            name: "netstorage".to_string(),
            description: "Akamai NetStorage".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "memory".to_string(),
            description: "Memory".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "hdfs".to_string(),
            description: "HDFS".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "drime".to_string(),
            description: "Drime".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "filescom".to_string(),
            description: "Files.com".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "doi".to_string(),
            description: "DOI".to_string(),
            tier: "Tier 2".to_string(),
        },
        Provider {
            name: "imagekit".to_string(),
            description: "ImageKit".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "shade".to_string(),
            description: "Shade".to_string(),
            tier: "Tier 1".to_string(),
        },
        Provider {
            name: "sia".to_string(),
            description: "Sia".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "filefabric".to_string(),
            description: "FileFabric".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "protondrive".to_string(),
            description: "Proton Drive".to_string(),
            tier: "Tier 4".to_string(),
        },
        Provider {
            name: "telegram".to_string(),
            description: "Telegram".to_string(),
            tier: "Tier 4".to_string(),
        },
    ];

    Json(ApiResponse::success(providers))
}

/// List active mounts
pub async fn handle_list_mounts(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<Mount>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    // Read /proc/mounts to find rclone mounts
    let mounts_content = tokio::fs::read_to_string("/proc/mounts").await;

    match mounts_content {
        Ok(content) => {
            let mut mounts = Vec::new();

            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 3 {
                    continue;
                }

                let mount_point = parts[1].to_string();
                let fs_type = parts[2].to_string();

                // Check if this is a rclone mount
                if fs_type == "rclone"
                    || mount_point.starts_with(crate::config::paths::RCLONE_MOUNT_BASE)
                    || mount_point.starts_with("/run/rclone")
                {
                    // Try to parse the remote name from the mount point
                    let remote = mount_point
                        .split('/')
                        .next_back()
                        .unwrap_or("unknown")
                        .to_string();

                    mounts.push(Mount {
                        remote,
                        mount_point,
                        path: None,
                        is_active: true,
                        pid: None,
                    });
                }
            }

            // Sort by mount point name
            mounts.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
            Json(ApiResponse::success(mounts))
        }
        Err(_) => {
            // If we can't read /proc/mounts, return empty list
            Json(ApiResponse::success(Vec::new()))
        }
    }
}

/// Start a mount operation. Always spawns rclone detached (`--daemon`-style,
/// via `Command::spawn` without waiting) regardless of the request's
/// `daemon` field — a real bug found during release verification had a
/// "foreground" branch that `.output().await`'d the process, but
/// `rclone mount` without `--daemon` never exits on its own, so that branch
/// would hang the HTTP request forever.
pub async fn handle_mount_start(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<MountRequest>,
) -> Json<ApiResponse<Mount>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // Validate mount point
    if req.mount_point.is_empty() {
        return Json(ApiResponse::error("Mount point is required"));
    }

    // Create the mount point directory before spawning rclone.
    // Unlike other mount types, rclone's FUSE mount can create the dir itself,
    // but we create it explicitly for consistency with RAID/pools/encrypted
    // and to ensure proper permissions.
    if let Err(e) = std::fs::create_dir_all(&req.mount_point) {
        return Json(ApiResponse::error(&format!(
            "Failed to create mount point: {}",
            e
        )));
    }

    if req.remote.is_empty() {
        return Json(ApiResponse::error("Remote is required"));
    }

    // Build rclone mount command
    let source = if let Some(path) = &req.path {
        format!("{}:{}", req.remote, path)
    } else {
        format!("{}:", req.remote)
    };

    let mut args = vec!["mount".to_string(), source, req.mount_point.clone()];

    if req.read_only {
        args.push("--read-only".to_string());
    }

    if req.allow_other {
        args.push("--allow-other".to_string());
    }

    args.push("--daemon".to_string());

    // Add some sensible defaults
    args.push("--vfs-cache-mode".to_string());
    args.push("writes".to_string());

    let mut cmd = Command::new("rclone");
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match cmd.spawn() {
        Ok(_child) => {
            let mount = Mount {
                remote: req.remote,
                mount_point: req.mount_point,
                path: req.path,
                is_active: true,
                pid: None,
            };
            crate::events::log_event(format!(
                "Admin '{}' mounted cloud remote '{}' at '{}'",
                admin_username, mount.remote, mount.mount_point
            ));
            Json(ApiResponse::success(mount))
        }
        Err(e) => Json(ApiResponse::error(&format!("Failed to start mount: {}", e))),
    }
}

/// Stop a mount operation
pub async fn handle_mount_stop(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<MountRequest>,
) -> Json<ApiResponse<bool>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    // Use fusermount to unmount
    let output = Command::new("fusermount")
        .args(["-u", &req.mount_point])
        .output()
        .await;

    match output {
        Ok(output) => {
            if output.status.success() {
                crate::events::log_event(format!(
                    "Admin '{}' unmounted cloud mount at '{}'",
                    admin_username, req.mount_point
                ));
                Json(ApiResponse::success(true))
            } else {
                // Try with umount as fallback
                let output = Command::new("umount")
                    .args(["-f", &req.mount_point])
                    .output()
                    .await;

                match output {
                    Ok(output) => {
                        if output.status.success() {
                            crate::events::log_event(format!(
                                "Admin '{}' unmounted cloud mount at '{}'",
                                admin_username, req.mount_point
                            ));
                            Json(ApiResponse::success(true))
                        } else {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            Json(ApiResponse::error(&format!(
                                "Failed to unmount: {}",
                                stderr
                            )))
                        }
                    }
                    Err(e) => Json(ApiResponse::error(&format!(
                        "Failed to execute umount: {}",
                        e
                    ))),
                }
            }
        }
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to execute fusermount: {}",
            e
        ))),
    }
}

/// Run sync operation
pub async fn handle_sync(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<SyncRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    execute_sync(&admin_username, &req).await
}

/// Shared core of [`handle_sync`] and [`handle_run_sync_job`]: validates the
/// pair of paths and runs the `rclone sync` subprocess.
async fn execute_sync(admin_username: &str, req: &SyncRequest) -> Json<ApiResponse<String>> {
    let mut args = vec!["sync".to_string()];

    // Determine path types
    let source_type = req.source_type.as_deref().unwrap_or("cloud");
    let dest_type = req.dest_type.as_deref().unwrap_or("cloud");

    // Build source and destination paths
    let source = build_rclone_path(source_type, &req.source, &req.source_path);
    let dest = build_rclone_path(dest_type, &req.destination, &req.dest_path);

    // Validate
    if source.is_empty() || source == ":" {
        return Json(ApiResponse::error("Source is required"));
    }
    if dest.is_empty() || dest == ":" {
        return Json(ApiResponse::error("Destination is required"));
    }

    args.push(source);
    args.push(dest);

    if req.delete {
        args.push("--delete".to_string());
    }

    if req.dry_run {
        args.push("--dry-run".to_string());
    }

    if req.update {
        args.push("--update".to_string());
    }

    if let Some(exclude) = &req.exclude {
        if !exclude.is_empty() {
            args.push("--exclude".to_string());
            args.push(exclude.clone());
        }
    }

    // Add progress output
    args.push("--progress".to_string());

    // Show the transfer in the desktop activity taskbar (indeterminate — the
    // blocking `.output()` gives no incremental stats).
    let job = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("cloud_sync".to_string()),
        &format!("Sync {} -> {}", req.source, req.destination),
    );

    let output = Command::new("rclone")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                crate::sse::PROGRESS_MANAGER.complete_job(&job, "Sync complete");
                crate::events::log_event(format!(
                    "Admin '{}' ran a sync from '{}' to '{}'",
                    admin_username, req.source, req.destination
                ));
                Json(ApiResponse::success(stdout.to_string()))
            } else {
                crate::sse::PROGRESS_MANAGER.fail_job(&job, &stderr);
                Json(ApiResponse::error(&format!("Sync failed: {}", stderr)))
            }
        }
        Err(e) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job, &e.to_string());
            Json(ApiResponse::error(&format!(
                "Failed to execute rclone sync: {}",
                e
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Sync jobs (persisted server-side; the frontend stopped using localStorage)
// ---------------------------------------------------------------------------

/// Body for creating a sync job.
#[derive(Deserialize)]
pub struct CreateSyncJobRequest {
    pub name: String,
    #[serde(default = "default_cloud")]
    pub source_type: String,
    pub source: String,
    pub source_path: Option<String>,
    #[serde(default = "default_cloud")]
    pub dest_type: String,
    pub destination: String,
    pub dest_path: Option<String>,
}

fn default_cloud() -> String {
    "cloud".to_string()
}

/// Response body for listing jobs. Mirrors the frontend's `SyncJob` shape so
/// the UI can render `id`, `source`, `destination` and a derived `direction`
/// without a second mapping layer.
#[derive(Serialize)]
pub struct SyncJobView {
    pub id: i64,
    pub name: String,
    pub source: String,
    pub destination: String,
    pub direction: String,
    pub created_at: String,
}

impl From<SyncJob> for SyncJobView {
    fn from(job: SyncJob) -> Self {
        let direction = if job.source_type == "local" {
            "local-to-cloud"
        } else {
            "cloud-to-local"
        };
        Self {
            id: job.id,
            name: job.name,
            source: job.source,
            destination: job.destination,
            direction: direction.to_string(),
            created_at: job.created_at,
        }
    }
}

/// List saved sync jobs (any authenticated session).
pub async fn handle_list_sync_jobs(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<Vec<SyncJobView>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    let jobs = match repo.list_all().await {
        Ok(jobs) => jobs,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to list sync jobs: {}",
                e
            )))
        }
    };
    let views: Vec<SyncJobView> = jobs.into_iter().map(SyncJobView::from).collect();
    Json(ApiResponse::success(views))
}

/// Save a new sync job (admin).
pub async fn handle_create_sync_job(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<CreateSyncJobRequest>,
) -> Json<ApiResponse<SyncJobView>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let job = NewSyncJob {
        name: req.name,
        source_type: req.source_type,
        source: req.source,
        source_path: req.source_path,
        dest_type: req.dest_type,
        destination: req.destination,
        dest_path: req.dest_path,
    };
    if let Err(msg) = job.validate() {
        return Json(ApiResponse::error(&msg));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    let id = match repo.insert(&job).await {
        Ok(id) => id,
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to save sync job: {}",
                e
            )))
        }
    };
    let created = match repo.get_by_id(id).await {
        Ok(Some(job)) => job,
        Ok(None) => {
            return Json(ApiResponse::error("Sync job was not persisted"));
        }
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to load sync job: {}",
                e
            )))
        }
    };
    Json(ApiResponse::success(SyncJobView::from(created)))
}

/// Delete a saved sync job (admin).
pub async fn handle_delete_sync_job(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Json<ApiResponse<bool>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let repo = SyncJobRepository::new(state.db.pool());
    match repo.delete(id).await {
        Ok(true) => Json(ApiResponse::success(true)),
        Ok(false) => Json(ApiResponse::error("Sync job not found")),
        Err(e) => Json(ApiResponse::error(&format!(
            "Failed to delete sync job: {}",
            e
        ))),
    }
}

/// Run a saved sync job now (admin). Rethrows the saved configuration through
/// the same `execute_sync` core as the ad-hoc sync endpoint.
pub async fn handle_run_sync_job(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let repo = SyncJobRepository::new(state.db.pool());
    let job = match repo.get_by_id(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return Json(ApiResponse::error("Sync job not found")),
        Err(e) => {
            return Json(ApiResponse::error(&format!(
                "Failed to load sync job: {}",
                e
            )))
        }
    };

    let req = SyncRequest {
        source_type: Some(job.source_type),
        source: job.source,
        source_path: job.source_path,
        dest_type: Some(job.dest_type),
        destination: job.destination,
        dest_path: job.dest_path,
        delete: false,
        dry_run: false,
        update: true,
        exclude: None,
    };
    execute_sync(&admin_username, &req).await
}

/// Run copy operation
pub async fn handle_copy(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<CopyRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let mut args = vec!["copy".to_string()];

    // Determine path types
    let source_type = req.source_type.as_deref().unwrap_or("cloud");
    let dest_type = req.dest_type.as_deref().unwrap_or("cloud");

    // Build source and destination paths
    let source = build_rclone_path(source_type, &req.source, &req.source_path);
    let dest = build_rclone_path(dest_type, &req.destination, &req.dest_path);

    // Validate
    if source.is_empty() || source == ":" {
        return Json(ApiResponse::error("Source is required"));
    }
    if dest.is_empty() || dest == ":" {
        return Json(ApiResponse::error("Destination is required"));
    }

    args.push(source);
    args.push(dest);

    if req.delete {
        args.push("--delete".to_string());
    }

    if req.dry_run {
        args.push("--dry-run".to_string());
    }

    if req.interactive {
        args.push("--interactive".to_string());
    }

    // Add progress output
    args.push("--progress".to_string());

    let job = crate::sse::PROGRESS_MANAGER.create_job(
        crate::sse::JobType::Custom("cloud_sync".to_string()),
        &format!("Copy {} -> {}", req.source, req.destination),
    );

    let output = Command::new("rclone")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                crate::sse::PROGRESS_MANAGER.complete_job(&job, "Copy complete");
                crate::events::log_event(format!(
                    "Admin '{}' ran a copy from '{}' to '{}'",
                    admin_username, req.source, req.destination
                ));
                Json(ApiResponse::success(stdout.to_string()))
            } else {
                crate::sse::PROGRESS_MANAGER.fail_job(&job, &stderr);
                Json(ApiResponse::error(&format!("Copy failed: {}", stderr)))
            }
        }
        Err(e) => {
            crate::sse::PROGRESS_MANAGER.fail_job(&job, &e.to_string());
            Json(ApiResponse::error(&format!(
                "Failed to execute rclone copy: {}",
                e
            )))
        }
    }
}

/// Check if rclone is available
pub async fn handle_rclone_status(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<HashMap<String, bool>>> {
    if let Err(e) = check_session(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let mut status = HashMap::new();

    let check = Command::new("rclone").arg("version").output().await;

    let available = check.is_ok() && check.as_ref().map(|o| o.status.success()).unwrap_or(false);
    status.insert("available".to_string(), available);

    // Check for fusermount (needed for unmounting)
    let fusermount = Command::new("fusermount").arg("-V").output().await;
    let has_fusermount = fusermount.is_ok()
        && fusermount
            .as_ref()
            .map(|o| o.status.success())
            .unwrap_or(false);
    status.insert("fusermount".to_string(), has_fusermount);

    Json(ApiResponse::success(status))
}

/// Get the current rclone.conf content. Admin-only: this file contains
/// every configured remote's credentials — rclone "obscures" secrets with
/// a trivially-reversible encoding, not real encryption, so exposing this
/// to a non-admin session would be a plaintext credential leak.
pub async fn handle_get_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Json<ApiResponse<String>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let config_path = "/etc/rclone.conf";

    match tokio::fs::read_to_string(config_path).await {
        Ok(content) => Json(ApiResponse::success(content)),
        Err(e) => Json(ApiResponse::error(&format!("Failed to read config: {}", e))),
    }
}

/// Request for saving config
#[derive(Deserialize)]
pub struct SaveConfigRequest {
    pub config: String,
}

/// Save the rclone.conf content
pub async fn handle_save_config(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<SaveConfigRequest>,
) -> Json<ApiResponse<String>> {
    if let Err(e) = check_admin(&state, &headers).await {
        return Json(ApiResponse::error(&e));
    }

    let config_path = "/etc/rclone.conf";
    let backup_path = format!(
        "{}.backup.{}",
        config_path,
        chrono::Utc::now().format("%Y%m%d_%H%M%S")
    );

    // Create backup of existing config
    if tokio::fs::metadata(config_path).await.is_ok() {
        if let Err(e) = tokio::fs::copy(config_path, &backup_path).await {
            return Json(ApiResponse::error(&format!(
                "Failed to create backup: {}",
                e
            )));
        }
    }

    // Write new config
    match tokio::fs::write(config_path, &req.config).await {
        Ok(_) => Json(ApiResponse::success(format!(
            "Config saved successfully. Backup: {}",
            backup_path
        ))),
        Err(e) => Json(ApiResponse::error(&format!("Failed to save config: {}", e))),
    }
}

/// Request for adding a remote
#[derive(Deserialize)]
pub struct AddRemoteRequest {
    pub name: String,
    pub provider: String,
    pub config: Option<String>,
}

/// Add a new remote
pub async fn handle_add_remote(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(req): Json<AddRemoteRequest>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let config_path = "/etc/rclone.conf";

    // Validate name
    if req.name.is_empty() {
        return Json(ApiResponse::error("Remote name is required"));
    }

    if req.name.contains(':') || req.name.contains('[') || req.name.contains(']') {
        return Json(ApiResponse::error(
            "Remote name contains invalid characters",
        ));
    }

    // Reject a `config` value containing an embedded `[section]` line —
    // otherwise this free-text field, appended verbatim into the INI-style
    // config file below, could silently inject or override an unrelated,
    // already-configured remote's section.
    if let Some(config) = &req.config {
        if config.lines().any(|l| l.trim_start().starts_with('[')) {
            return Json(ApiResponse::error(
                "Config value cannot contain a line starting with '[' (would inject another config section)",
            ));
        }
    }

    // Read existing config
    let existing_config: String = tokio::fs::read_to_string(config_path)
        .await
        .unwrap_or_default();

    // Check if remote already exists
    let remote_section = format!("[{}]", req.name);
    if existing_config.contains(&remote_section) {
        return Json(ApiResponse::error(&format!(
            "Remote '{}' already exists",
            req.name
        )));
    }

    // Build new remote section
    let new_section = if let Some(config) = &req.config {
        format!(
            "\n{}\ntype = {}\n{}\n",
            remote_section, req.provider, config
        )
    } else {
        format!("\n{}\ntype = {}\n", remote_section, req.provider)
    };

    // Append to config
    let new_config = format!("{}{}", existing_config.trim_end(), new_section);

    match tokio::fs::write(config_path, new_config).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' added cloud remote '{}' ({})",
                admin_username, req.name, req.provider
            ));
            Json(ApiResponse::success(format!(
                "Remote '{}' added successfully",
                req.name
            )))
        }
        Err(e) => Json(ApiResponse::error(&format!("Failed to save config: {}", e))),
    }
}

/// Delete a remote
pub async fn handle_delete_remote(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Json<ApiResponse<String>> {
    let admin_username = match check_admin(&state, &headers).await {
        Ok(u) => u,
        Err(e) => return Json(ApiResponse::error(&e)),
    };

    let config_path = "/etc/rclone.conf";

    // Read existing config
    let existing_config = match tokio::fs::read_to_string(config_path).await {
        Ok(content) => content,
        Err(e) => return Json(ApiResponse::error(&format!("Failed to read config: {}", e))),
    };

    // Find and remove the remote section
    let section_start = format!("[{}]", name);
    let section_start_idx = existing_config.find(&section_start);

    if section_start_idx.is_none() {
        return Json(ApiResponse::error(&format!("Remote '{}' not found", name)));
    }

    // Find the end of this section (next [section] or end of file)
    let start_pos = section_start_idx.unwrap();
    let after_start = &existing_config[start_pos..];

    let end_pos = after_start[1..]
        .find('[')
        .map(|p| p + 1)
        .unwrap_or(after_start.len());

    let before_section = &existing_config[..start_pos].trim_end();
    let after_section = &existing_config[start_pos + end_pos..].trim_start();

    let new_config = if before_section.is_empty() {
        after_section.to_string()
    } else {
        format!("{}\n{}", before_section, after_section)
    };

    match tokio::fs::write(config_path, new_config).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted cloud remote '{}'",
                admin_username, name
            ));
            Json(ApiResponse::success(format!(
                "Remote '{}' deleted successfully",
                name
            )))
        }
        Err(e) => Json(ApiResponse::error(&format!("Failed to save config: {}", e))),
    }
}

#[cfg(test)]
mod tests {
    //! Pure logic/unit tests for the Cloud Sync (rclone) handler module.
    //!
    //! Covers the deterministic helpers: `build_rclone_path` (how a
    //! source/dest becomes an `rclone` argument — `remote:` vs `remote:sub`
    //! vs a local path), `get_provider_description` (the provider-type →
    //! label table, incl. the `<type> (custom)` fallback), the `rclone.conf`
    //! INI parser, and the `ApiResponse` envelope shape the frontend checks.
    //!
    //! No network, no `rclone` process. The config-parser tests point
    //! `RCLONE_CONFIG` at a tempfile (serialised behind a mutex).
    //!
    //! Run: `cargo test handlers::rclone::tests`

    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -- build_rclone_path -------------------------------------------------

    #[test]
    fn test_build_rclone_path_cloud_root() {
        assert_eq!(build_rclone_path("cloud", "gdrive", &None), "gdrive:");
        assert_eq!(
            build_rclone_path("cloud", "gdrive", &Some(String::new())),
            "gdrive:"
        );
    }

    #[test]
    fn test_build_rclone_path_cloud_with_subpath() {
        assert_eq!(
            build_rclone_path("cloud", "gdrive", &Some("backups/2026".to_string())),
            "gdrive:backups/2026"
        );
        // a leading slash on the subpath is stripped (rclone remote paths
        // are not absolute)
        assert_eq!(
            build_rclone_path("cloud", "gdrive", &Some("/backups".to_string())),
            "gdrive:backups"
        );
    }

    #[test]
    fn test_build_rclone_path_local() {
        assert_eq!(build_rclone_path("local", "/srv/data", &None), "/srv/data");
        assert_eq!(
            build_rclone_path("local", "/srv/data", &Some("photos".to_string())),
            "/srv/data/photos"
        );
        // trailing slash on the base + leading slash on the subpath collapse
        assert_eq!(
            build_rclone_path("local", "/srv/data/", &Some("/photos".to_string())),
            "/srv/data/photos"
        );
    }

    #[test]
    fn test_build_rclone_path_anything_not_local_is_treated_as_cloud() {
        assert_eq!(build_rclone_path("", "r", &None), "r:");
        assert_eq!(build_rclone_path("cloud", "r", &None), "r:");
    }

    // -- get_provider_description ---------------------------------------

    #[test]
    fn test_provider_description_known_types() {
        assert_eq!(get_provider_description("drive"), "Google Drive");
        assert_eq!(get_provider_description("s3"), "Amazon S3");
        assert_eq!(get_provider_description("webdav"), "WebDAV");
        assert_eq!(get_provider_description("crypt"), "Encrypted (Crypt)");
    }

    #[test]
    fn test_provider_description_is_case_insensitive_and_has_aliases() {
        assert_eq!(get_provider_description("DRIVE"), "Google Drive");
        assert_eq!(get_provider_description("b2"), "Backblaze B2");
        assert_eq!(get_provider_description("backblaze"), "Backblaze B2");
        assert_eq!(get_provider_description("smb"), "SMB/CIFS");
        assert_eq!(get_provider_description("cifs"), "SMB/CIFS");
    }

    #[test]
    fn test_provider_description_unknown_type_falls_back_to_custom() {
        assert_eq!(
            get_provider_description("weirdcloud"),
            "weirdcloud (custom)"
        );
    }

    // -- parse_rclone_config (INI) ------------------------------------

    fn with_config<T>(contents: &str, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rclone.conf");
        std::fs::write(&path, contents).unwrap();
        std::env::set_var("RCLONE_CONFIG", &path);
        let out = f();
        std::env::remove_var("RCLONE_CONFIG");
        out
    }

    #[test]
    fn test_parse_rclone_config_reads_sections_sorted() {
        let cfg = "\
# a comment
[zeta]
type = dropbox
token = xxx

[alpha]
type = drive
scope = drive
";
        let remotes = with_config(cfg, parse_rclone_config).expect("some remotes");
        let names: Vec<_> = remotes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]); // sorted by name
        assert_eq!(remotes[0].remote_type, "Google Drive");
        assert_eq!(remotes[1].remote_type, "Dropbox");
    }

    #[test]
    fn test_parse_rclone_config_skips_sections_without_a_type() {
        let cfg = "\
[good]
type = s3

[no_type]
provider = whatever
region = us-east-1
";
        let remotes = with_config(cfg, parse_rclone_config).expect("some remotes");
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "good");
    }

    // -- ApiResponse envelope ---------------------------------------

    #[test]
    fn test_api_response_shapes() {
        let ok = serde_json::to_value(ApiResponse::success("hi".to_string())).unwrap();
        assert_eq!(ok["success"], true);
        assert_eq!(ok["data"], "hi");
        assert_eq!(ok["error"], serde_json::Value::Null);

        let err = serde_json::to_value(ApiResponse::<String>::error("boom")).unwrap();
        assert_eq!(err["success"], false);
        assert_eq!(err["data"], serde_json::Value::Null);
        assert_eq!(err["error"], "boom");
    }

    // -- MountRequest defaults (the release-verification bug) -------

    #[test]
    fn test_mount_request_bool_fields_default_to_false() {
        // The frontend only sends { remote, mount_point, path } — the 3 bool
        // fields must be `#[serde(default)]` or every real mount request
        // fails deserialization before the handler runs.
        let req: MountRequest =
            serde_json::from_str(r#"{"remote":"gd","mount_point":"/mnt/gd","path":"backups"}"#)
                .expect("must deserialize without the bool fields");
        assert!(!req.read_only && !req.allow_other && !req.daemon);
        assert_eq!(req.path.as_deref(), Some("backups"));
    }
}
