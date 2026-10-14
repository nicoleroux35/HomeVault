/* =============================================================================
 * File          : docker.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Docker engine service wrapping the docker CLI: container lifecycle
 *   (create from typed requests, start/stop/restart/remove, logs), image
 *   pull/remove, network inspection, port availability checks and daemon
 *   control.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2)
 *
 * Usage:
 *   handlers/docker.rs and the app catalog install flow.
 * =============================================================================
 */

//! Docker engine service wrapping the docker CLI (containers, images,
//! networks, daemon control).

use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

/// All parameters required to create and run a new Docker container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateContainerRequest {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub ports: Vec<PortMappingRequest>,
    #[serde(default)]
    pub volumes: Vec<VolumeMappingRequest>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub restart_policy: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub memory_limit: Option<String>,
    #[serde(default)]
    pub cpu_limit: Option<String>,
    #[serde(default)]
    pub privileged: bool,
    #[serde(default)]
    pub auto_remove: bool,
    #[serde(default)]
    pub dns_servers: Vec<String>,
    #[serde(default)]
    pub hostname: Option<String>,
}

/// A port mapping from host to container with protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMappingRequest {
    pub host_port: u16,
    pub container_port: u16,
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

fn default_protocol() -> String {
    "tcp".to_string()
}

/// A bind-mount volume between host and container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMappingRequest {
    pub host_path: String,
    pub container_path: String,
    #[serde(default)]
    pub read_only: bool,
}

/// An environment variable key-value pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Request to pull a Docker image from a registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullImageRequest {
    pub image: String,
    #[serde(default = "default_pull_tag")]
    pub tag: String,
}

fn default_pull_tag() -> String {
    "latest".to_string()
}

/// Errors that can occur during Docker operations.
#[derive(Error, Debug)]
pub enum DockerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Docker not available")]
    NotAvailable,
    #[error("Container not found: {0}")]
    ContainerNotFound(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// A Docker container with runtime stats (memory, CPU) and update availability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub state: ContainerState,
    pub created: String,
    pub ports: Vec<PortMapping>,
    pub memory_mb: Option<u64>,
    pub cpu_percent: Option<f64>,
    /// Cumulative network RX / TX for the container's lifetime, in bytes
    /// (from `docker stats` `NetIO`). `None` when the container isn't
    /// running or stats are unavailable.
    #[serde(default)]
    pub net_rx_bytes: Option<u64>,
    #[serde(default)]
    pub net_tx_bytes: Option<u64>,
    /// Cumulative block-device read / write, in bytes (`docker stats`
    /// `BlockIO`).
    #[serde(default)]
    pub blk_read_bytes: Option<u64>,
    #[serde(default)]
    pub blk_write_bytes: Option<u64>,
    /// Whether a newer version of the image is available locally.
    pub update_available: bool,
    /// Docker Compose project name (if the container is part of a compose project).
    #[serde(default)]
    pub compose_project: Option<String>,
    /// Networks the container is connected to.
    #[serde(default)]
    pub networks: Vec<String>,
}

/// Known container lifecycle states.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContainerState {
    Running,
    Exited,
    Paused,
    Restarting,
    Dead,
    Created,
    Unknown,
}

impl From<&str> for ContainerState {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "running" => ContainerState::Running,
            "exited" => ContainerState::Exited,
            "paused" => ContainerState::Paused,
            "restarting" => ContainerState::Restarting,
            "dead" => ContainerState::Dead,
            "created" => ContainerState::Created,
            _ => ContainerState::Unknown,
        }
    }
}

/// A resolved port mapping for a running container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

/// Result of checking whether a host port is free to publish a container on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortStatus {
    /// `true` when nothing on the host (or a Docker container, or HomeVault
    /// itself) is already holding this port.
    pub available: bool,
    /// What is holding it, when unavailable: `"container"`, `"homevault"` or
    /// `"host"`. `None` when `available`. The frontend maps this to a
    /// localized message.
    pub used_by: Option<String>,
    /// Extra context for `used_by` — e.g. the container name. May be empty.
    #[serde(default)]
    pub detail: Option<String>,
}

/// A locally available Docker image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub size: String,
    pub created: String,
}

/// Summary Docker daemon information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerInfo {
    pub version: String,
    pub api_version: String,
    pub os: String,
    pub arch: String,
    pub containers_running: u32,
    pub containers_paused: u32,
    pub containers_stopped: u32,
    pub images: u32,
}

/// Per-container live resource usage, parsed from one `docker stats
/// --no-stream` line. All fields are `None` when the container isn't
/// running or the value couldn't be parsed.
#[derive(Debug, Clone, Copy, Default)]
struct ContainerStats {
    memory_mb: Option<u64>,
    cpu_percent: Option<f64>,
    net_rx_bytes: Option<u64>,
    net_tx_bytes: Option<u64>,
    blk_read_bytes: Option<u64>,
    blk_write_bytes: Option<u64>,
}

/// Service for interacting with the local Docker daemon via CLI.
pub struct DockerService;

impl DockerService {
    /// Return `true` if the `docker info` command succeeds.
    pub fn is_available() -> bool {
        Command::new("docker")
            .args(["info"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// List all containers (running and stopped) with stats and update info.
    ///
    /// `include_stats` controls whether the live CPU/memory stats are fetched.
    /// `docker stats --no-stream` samples CPU over ~1s with running containers,
    /// so callers that want the list to appear instantly (the Docker page's
    /// first paint) pass `false` and fill the stats in afterwards.
    pub fn get_containers(include_stats: bool) -> Result<Vec<Container>, DockerError> {
        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--format",
                "{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}|{{.State}}|{{.CreatedAt}}|{{.Ports}}|{{.Label \"com.docker.compose.project\"}}",
            ])
            .output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let ps_stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let ids: Vec<String> = ps_stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                (parts.len() >= 6).then(|| parts[0].to_string())
            })
            .collect();

        // `docker stats --no-stream` (samples CPU over ~1s with running
        // containers), `docker images`, and the per-container `docker inspect`
        // calls are all independent of each other — run them concurrently so
        // the wall-clock time is ~max(...) instead of the sum. Stats are only
        // spawned when requested (see the `include_stats` doc above).
        let (stats, local_images, networks) = std::thread::scope(|s| {
            let stats_h = include_stats.then(|| s.spawn(Self::get_container_stats));
            let images_h = s.spawn(Self::get_local_image_tags);

            let net_h: Vec<_> = ids
                .iter()
                .map(|id| s.spawn(move || Self::get_container_networks(id)))
                .collect();
            let networks: Vec<Vec<String>> = net_h
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect();

            let stats = match stats_h {
                Some(h) => h
                    .join()
                    .unwrap_or_else(|_| Ok(std::collections::HashMap::new())),
                None => Ok(std::collections::HashMap::new()),
            };
            let local_images = images_h
                .join()
                .unwrap_or_else(|_| Ok(std::collections::HashSet::new()));

            (stats, local_images, networks)
        });

        let stats = stats?;
        let local_images = local_images?;

        let mut containers = Vec::new();
        let mut net_idx = 0;

        for line in ps_stdout.lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 6 {
                let container_id = parts[0];
                let ports = Self::parse_ports(parts.get(6).unwrap_or(&""));

                let st = stats.get(container_id).copied().unwrap_or_default();

                let image_full = parts[2].to_string();
                let update_available = Self::check_update_available(&image_full, &local_images);

                let compose_project = parts
                    .get(7)
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string());

                // Networks were fetched concurrently, aligned by container order.
                let networks = networks.get(net_idx).cloned().unwrap_or_default();
                net_idx += 1;

                containers.push(Container {
                    id: container_id.to_string(),
                    name: parts[1].to_string(),
                    image: image_full,
                    status: parts[3].to_string(),
                    state: ContainerState::from(*parts.get(4).unwrap_or(&"")),
                    created: parts[5].to_string(),
                    ports,
                    memory_mb: st.memory_mb,
                    cpu_percent: st.cpu_percent,
                    net_rx_bytes: st.net_rx_bytes,
                    net_tx_bytes: st.net_tx_bytes,
                    blk_read_bytes: st.blk_read_bytes,
                    blk_write_bytes: st.blk_write_bytes,
                    update_available,
                    compose_project,
                    networks,
                });
            }
        }

        Ok(containers)
    }

    /// Fetch live per-container resource stats via `docker stats --no-stream`
    /// (memory, CPU, cumulative network I/O, cumulative block I/O).
    fn get_container_stats(
    ) -> Result<std::collections::HashMap<String, ContainerStats>, DockerError> {
        let output = Command::new("docker")
            .args([
                "stats",
                "--no-stream",
                "--format",
                "{{.ID}}|{{.MemUsage}}|{{.CPUPerc}}|{{.NetIO}}|{{.BlockIO}}",
            ])
            .output()?;

        let mut stats = std::collections::HashMap::new();

        if !output.status.success() {
            return Ok(stats);
        }

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 3 {
                let id = parts[0].to_string();
                let (net_rx, net_tx) = parts
                    .get(3)
                    .map(|s| Self::parse_io_pair(s))
                    .unwrap_or((None, None));
                let (blk_read, blk_write) = parts
                    .get(4)
                    .map(|s| Self::parse_io_pair(s))
                    .unwrap_or((None, None));
                stats.insert(
                    id,
                    ContainerStats {
                        memory_mb: Self::parse_mem_usage(parts[1]),
                        cpu_percent: Self::parse_cpu_percent(parts[2]),
                        net_rx_bytes: net_rx,
                        net_tx_bytes: net_tx,
                        blk_read_bytes: blk_read,
                        blk_write_bytes: blk_write,
                    },
                );
            }
        }

        Ok(stats)
    }

    /// Parse a `docker stats` I/O pair like `"1.2kB / 3.4MB"` into
    /// `(first, second)` byte counts. Docker uses SI units here (kB/MB/GB),
    /// unlike the binary units in `MemUsage`.
    fn parse_io_pair(s: &str) -> (Option<u64>, Option<u64>) {
        let mut it = s.split('/');
        let a = it.next().map(Self::parse_si_bytes).unwrap_or(None);
        let b = it.next().map(Self::parse_si_bytes).unwrap_or(None);
        (a, b)
    }

    /// Parse an SI byte string (`"1.2kB"`, `"3.4MB"`, `"512B"`, `"0B"`).
    fn parse_si_bytes(v: &str) -> Option<u64> {
        let v = v.trim();
        let (num, mult): (&str, f64) = if let Some(n) = v.strip_suffix("GB") {
            (n, 1_000_000_000.0)
        } else if let Some(n) = v.strip_suffix("MB") {
            (n, 1_000_000.0)
        } else if let Some(n) = v.strip_suffix("kB").or_else(|| v.strip_suffix("KB")) {
            (n, 1_000.0)
        } else {
            (v.strip_suffix("B")?, 1.0)
        };
        num.trim().parse::<f64>().ok().map(|f| (f * mult) as u64)
    }

    /// Parse a Docker memory usage string (e.g. `"123MiB / 512MiB"`) into MB.
    fn parse_mem_usage(mem_str: &str) -> Option<u64> {
        let trimmed = mem_str.trim();
        let parts: Vec<&str> = trimmed.split('/').collect();
        let value_str = parts.first().unwrap_or(&"").trim();

        if value_str.ends_with("GiB") || value_str.ends_with("GB") {
            let num: f64 = value_str
                .replace("GiB", "")
                .replace("GB", "")
                .trim()
                .parse()
                .ok()?;
            Some((num * 1024.0) as u64)
        } else if value_str.ends_with("MiB") || value_str.ends_with("MB") {
            let num: f64 = value_str
                .replace("MiB", "")
                .replace("MB", "")
                .trim()
                .parse()
                .ok()?;
            Some(num as u64)
        } else if value_str.ends_with("KiB")
            || value_str.ends_with("KB")
            || value_str.ends_with("kB")
        {
            let num: f64 = value_str
                .replace("KiB", "")
                .replace("KB", "")
                .replace("kB", "")
                .trim()
                .parse()
                .ok()?;
            Some((num / 1024.0) as u64)
        } else if value_str.ends_with("B") {
            let num: f64 = value_str.replace("B", "").trim().parse().ok()?;
            Some((num / 1024.0 / 1024.0) as u64)
        } else {
            None
        }
    }

    /// Parse a Docker CPU percentage string (e.g. `"2.50%"`) into a float.
    fn parse_cpu_percent(cpu_str: &str) -> Option<f64> {
        let trimmed = cpu_str.trim().replace('%', "");
        trimmed.parse::<f64>().ok()
    }

    /// Return the set of all locally cached image `repository:tag` strings.
    fn get_local_image_tags() -> Result<std::collections::HashSet<String>, DockerError> {
        let output = Command::new("docker")
            .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
            .output()?;

        let mut tags = std::collections::HashSet::new();
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let line = line.trim();
                if !line.is_empty() && !line.contains("<none>") {
                    tags.insert(line.to_string());
                    if let Some(repo) = line.split(':').next() {
                        tags.insert(repo.to_string());
                    }
                }
            }
        }
        Ok(tags)
    }

    /// Return `true` if a different tag (or `:latest`) of the image exists locally but the current one doesn't.
    fn check_update_available(
        image: &str,
        local_images: &std::collections::HashSet<String>,
    ) -> bool {
        let image_clean = image.split('@').next().unwrap_or(image).trim().to_string();

        if local_images.contains(&image_clean) {
            return false;
        }

        if let Some(colon_pos) = image_clean.rfind(':') {
            let (repo, _tag) = image_clean.split_at(colon_pos);
            let latest = format!("{}:latest", repo);
            if local_images.contains(&latest) {
                return true;
            }
        }

        false
    }

    /// List only running containers with stats.
    pub fn get_running_containers() -> Result<Vec<Container>, DockerError> {
        let output = Command::new("docker")
            .args([
                "ps",
                "--format",
                "{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}|{{.State}}|{{.CreatedAt}}|{{.Ports}}|{{.Label \"com.docker.compose.project\"}}",
            ])
            .output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let stats = Self::get_container_stats()?;
        let local_images = Self::get_local_image_tags()?;

        let mut containers = Vec::new();

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 6 {
                let container_id = parts[0];
                let ports = Self::parse_ports(parts.get(6).unwrap_or(&""));

                let st = stats.get(container_id).copied().unwrap_or_default();

                let image_full = parts[2].to_string();
                let update_available = Self::check_update_available(&image_full, &local_images);

                let compose_project = parts
                    .get(7)
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| s.trim().to_string());

                let networks = Self::get_container_networks(container_id);

                containers.push(Container {
                    id: container_id.to_string(),
                    name: parts[1].to_string(),
                    image: image_full,
                    status: parts[3].to_string(),
                    state: ContainerState::from(*parts.get(4).unwrap_or(&"")),
                    created: parts[5].to_string(),
                    ports,
                    memory_mb: st.memory_mb,
                    cpu_percent: st.cpu_percent,
                    net_rx_bytes: st.net_rx_bytes,
                    net_tx_bytes: st.net_tx_bytes,
                    blk_read_bytes: st.blk_read_bytes,
                    blk_write_bytes: st.blk_write_bytes,
                    update_available,
                    compose_project,
                    networks,
                });
            }
        }

        Ok(containers)
    }

    /// Return the full `docker inspect` JSON for a container.
    pub fn get_container_info(name: &str) -> Result<String, DockerError> {
        let output = Command::new("docker").args(["inspect", name]).output()?;

        if !output.status.success() {
            return Err(DockerError::ContainerNotFound(name.to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Return the last `lines` of log output from a container.
    pub fn get_container_logs(name: &str, lines: u32) -> Result<String, DockerError> {
        let output = Command::new("docker")
            .args(["logs", "--tail", &lines.to_string(), name])
            .output()?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Start a stopped container by name.
    pub fn start_container(name: &str) -> Result<(), DockerError> {
        let output = Command::new("docker").args(["start", name]).output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Stop a running container by name.
    pub fn stop_container(name: &str) -> Result<(), DockerError> {
        let output = Command::new("docker").args(["stop", name]).output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Restart a container by name.
    pub fn restart_container(name: &str) -> Result<(), DockerError> {
        let output = Command::new("docker").args(["restart", name]).output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// List all locally cached Docker images.
    pub fn get_images() -> Result<Vec<Image>, DockerError> {
        let output = Command::new("docker")
            .args([
                "images",
                "--format",
                "{{.ID}}|{{.Repository}}|{{.Tag}}|{{.Size}}|{{.CreatedAt}}",
            ])
            .output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let mut images = Vec::new();

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 5 {
                images.push(Image {
                    id: parts[0].to_string(),
                    repository: parts[1].to_string(),
                    tag: parts[2].to_string(),
                    size: parts[3].to_string(),
                    created: parts[4].to_string(),
                });
            }
        }

        Ok(images)
    }

    /// Return daemon information (version, OS, container/image counts).
    pub fn get_docker_info() -> Result<DockerInfo, DockerError> {
        let output = Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}|{{.Architecture}}|{{.OSType}}|{{.Containers}}|{{.ContainersRunning}}|{{.ContainersPaused}}|{{.ContainersStopped}}|{{.Images}}"])
            .output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.trim().split('|').collect();

        Ok(DockerInfo {
            version: parts.first().unwrap_or(&"").to_string(),
            api_version: parts.first().unwrap_or(&"").to_string(),
            os: parts.get(2).unwrap_or(&"").to_string(),
            arch: parts.get(1).unwrap_or(&"").to_string(),
            containers_running: parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0),
            containers_paused: parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(0),
            containers_stopped: parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(0),
            images: parts.get(7).and_then(|s| s.parse().ok()).unwrap_or(0),
        })
    }

    /// Return the Docker server version string.
    pub fn get_docker_version() -> Result<String, DockerError> {
        let output = Command::new("docker")
            .args(["version", "--format", "{{.Server.Version}}"])
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(DockerError::CommandFailed(
                "Failed to get Docker version".to_string(),
            ))
        }
    }

    /// Create and start a new container from the given request.
    pub fn create_container(req: &CreateContainerRequest) -> Result<(), DockerError> {
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            req.name.clone(),
        ];

        // Add port mappings
        for port in &req.ports {
            args.push("-p".to_string());
            let proto = if port.protocol == "udp" { "udp" } else { "tcp" };
            args.push(format!(
                "{}:{}/{}",
                port.host_port, port.container_port, proto
            ));
        }

        // Add volume mappings
        for vol in &req.volumes {
            if vol.read_only {
                args.push("-v".to_string());
                args.push(format!("{}:{}:ro", vol.host_path, vol.container_path));
            } else {
                args.push("-v".to_string());
                args.push(format!("{}:{}", vol.host_path, vol.container_path));
            }
        }

        // Add environment variables
        for env in &req.env {
            args.push("-e".to_string());
            args.push(format!("{}={}", env.key, env.value));
        }

        // Add network
        if let Some(ref network) = req.network {
            if !network.is_empty() && network != "bridge" {
                args.push("--network".to_string());
                args.push(network.clone());
            }
        }

        // Add restart policy
        if let Some(ref policy) = req.restart_policy {
            match policy.as_str() {
                "no" | "none" => {
                    args.push("--restart".to_string());
                    args.push("no".to_string());
                }
                "on-failure" => {
                    args.push("--restart".to_string());
                    args.push("on-failure".to_string());
                }
                "always" => {
                    args.push("--restart".to_string());
                    args.push("always".to_string());
                }
                "unless-stopped" => {
                    args.push("--restart".to_string());
                    args.push("unless-stopped".to_string());
                }
                _ => {}
            }
        }

        // Add working directory
        if let Some(ref wd) = req.working_dir {
            if !wd.is_empty() {
                args.push("-w".to_string());
                args.push(wd.clone());
            }
        }

        // Add memory limit
        if let Some(ref mem) = req.memory_limit {
            if !mem.is_empty() {
                args.push("-m".to_string());
                args.push(mem.clone());
            }
        }

        // Add CPU limit
        if let Some(ref cpu) = req.cpu_limit {
            if !cpu.is_empty() {
                args.push("--cpus".to_string());
                args.push(cpu.clone());
            }
        }

        // Add privileged mode
        if req.privileged {
            args.push("--privileged".to_string());
        }

        // Add auto-remove
        if req.auto_remove {
            args.push("--rm".to_string());
        }

        // Add DNS servers
        for dns in &req.dns_servers {
            args.push("--dns".to_string());
            args.push(dns.clone());
        }

        // Add hostname
        if let Some(ref hostname) = req.hostname {
            if !hostname.is_empty() {
                args.push("-h".to_string());
                args.push(hostname.clone());
            }
        }

        // Add image (must precede COMMAND in `docker run [OPTIONS] IMAGE [COMMAND] [ARG...]`)
        args.push(req.image.clone());

        // Add command override, split into words so it becomes COMMAND [ARG...]
        if let Some(ref cmd) = req.command {
            let words: Vec<String> = cmd.split_whitespace().map(String::from).collect();
            args.extend(words);
        }

        let output = Command::new("docker").args(&args).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Remove a container. When `force` is true, uses `-f` to kill it first.
    pub fn remove_container(name: &str, force: bool) -> Result<(), DockerError> {
        let mut args = vec!["rm".to_string()];
        if force {
            args.push("-f".to_string());
        }
        args.push(name.to_string());

        let output = Command::new("docker").args(&args).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Pull a Docker image from the configured registry.
    pub fn pull_image(image: &str, tag: &str) -> Result<(), DockerError> {
        let output = Command::new("docker")
            .args(["pull", &format!("{}:{}", image, tag)])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Remove a locally cached image. When `force` is true, uses `-f`.
    pub fn remove_image(image_id: &str, force: bool) -> Result<(), DockerError> {
        let mut args = vec!["rmi".to_string()];
        if force {
            args.push("-f".to_string());
        }
        args.push(image_id.to_string());

        let output = Command::new("docker").args(&args).output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Parse Docker's `{{.Ports}}` column into structured published mappings.
    ///
    /// Each comma-separated entry for a *published* port looks like
    /// `IP:HOSTPORT->CONTAINERPORT/proto` — e.g. `0.0.0.0:8096->8096/tcp` or
    /// `[::]:8096->8096/tcp`. Note the colon is on the **left** of `->` (the
    /// host `IP:port`); the right side is just `CONTAINERPORT/proto`. Entries
    /// with no `->` are merely *exposed* (not reachable from the host) and are
    /// skipped. IPv4 + IPv6 lines for the same mapping are de-duplicated so a
    /// container that binds both doesn't list every port twice.
    fn parse_ports(ports_str: &str) -> Vec<PortMapping> {
        let mut ports: Vec<PortMapping> = Vec::new();

        for entry in ports_str.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }

            // Only published ports have `->` (and therefore a host port).
            let Some((left, right)) = entry.split_once("->") else {
                continue;
            };

            // Host port = the token after the last ':' in the left side
            // (handles `0.0.0.0:8096`, `[::]:8096`, and a bare `8096`).
            let host_token = left.rsplit(':').next().unwrap_or(left).trim();
            let host_token = host_token.split('/').next().unwrap_or(host_token);

            // Right side: `CONTAINERPORT/proto`.
            let mut right_parts = right.trim().split('/');
            let container_token = right_parts.next().unwrap_or("").trim();
            let protocol = right_parts.next().unwrap_or("tcp").to_string();

            if let (Ok(host_port), Ok(container_port)) =
                (host_token.parse::<u16>(), container_token.parse::<u16>())
            {
                let mapping = PortMapping {
                    host_port,
                    container_port,
                    protocol,
                };
                if !ports.iter().any(|p| {
                    p.host_port == mapping.host_port
                        && p.container_port == mapping.container_port
                        && p.protocol == mapping.protocol
                }) {
                    ports.push(mapping);
                }
            }
        }

        ports
    }

    /// Get networks a container is connected to via `docker inspect`.
    fn get_container_networks(container_id: &str) -> Vec<String> {
        let output = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.NetworkSettings.Networks}}",
                container_id,
            ])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let raw = String::from_utf8_lossy(&o.stdout).to_string();
                // Parse format like: map[bridge:0x12345 mynet:0x67890]
                let raw = raw.trim().trim_start_matches("map[").trim_end_matches(']');
                if raw.is_empty() {
                    return vec![];
                }
                raw.split(' ')
                    .filter_map(|part| {
                        let parts: Vec<&str> = part.split(':').collect();
                        if parts.len() >= 2 {
                            let name = parts[0].trim().to_string();
                            if !name.is_empty() {
                                return Some(name);
                            }
                        }
                        None
                    })
                    .collect()
            }
            _ => vec![],
        }
    }

    /// Every TCP/UDP port currently in a LISTEN/UNCONN state on the host, from
    /// `ss`. This already includes Docker-published ports (the daemon binds
    /// them on the host) and every HomeVault-managed daemon (web, SFTP, SMB,
    /// NFS, FTP), so it's the single source of truth for "is this port free".
    fn host_bound_ports() -> std::collections::HashSet<u16> {
        let mut ports = std::collections::HashSet::new();
        // -H no header, -l listening, -n numeric, -t tcp, -u udp, -a incl. UNCONN
        for args in [["-Hltn"].as_slice(), ["-Hluna"].as_slice()] {
            if let Ok(out) = Command::new("ss").args(args).output() {
                if !out.status.success() {
                    continue;
                }
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    // Local address:port is the 4th whitespace field, e.g.
                    // `0.0.0.0:8096`, `[::]:8096`, `127.0.0.1:53`, `*:22`.
                    if let Some(local) = line.split_whitespace().nth(3) {
                        if let Some(p) = Self::port_from_socket_addr(local) {
                            ports.insert(p);
                        }
                    }
                }
            }
        }
        ports
    }

    /// Extract the port from an `ss` local-address token (`host:port`, where
    /// host may be an IPv4, a `[..]`-wrapped IPv6, or `*`).
    fn port_from_socket_addr(addr: &str) -> Option<u16> {
        addr.rsplit(':').next()?.parse::<u16>().ok()
    }

    /// Map of published host port -> container name, for attributing a
    /// conflict to a specific container in the UI. Lightweight (`docker ps`
    /// only, no stats).
    fn published_host_ports() -> std::collections::HashMap<u16, String> {
        let mut map = std::collections::HashMap::new();
        let Ok(out) = Command::new("docker")
            .args(["ps", "-a", "--format", "{{.Names}}|{{.Ports}}"])
            .output()
        else {
            return map;
        };
        if !out.status.success() {
            return map;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some((name, ports_str)) = line.split_once('|') else {
                continue;
            };
            for m in Self::parse_ports(ports_str) {
                map.entry(m.host_port)
                    .or_insert_with(|| name.trim().to_string());
            }
        }
        map
    }

    /// The TCP port HomeVault's own web server listens on (env `PORT`, else 8080).
    fn homevault_port() -> u16 {
        std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080)
    }

    /// Check whether `port` is free to publish a container on — against the
    /// host's bound ports, other Docker containers, and HomeVault itself.
    pub fn check_port(port: u16) -> PortStatus {
        if port == 0 {
            return PortStatus {
                available: false,
                used_by: Some("host".into()),
                detail: None,
            };
        }

        // Attribute to a container first (most useful message), then to
        // HomeVault, then to a generic host process.
        if let Some(name) = Self::published_host_ports().get(&port) {
            return PortStatus {
                available: false,
                used_by: Some("container".into()),
                detail: Some(name.clone()),
            };
        }
        if port == Self::homevault_port() {
            return PortStatus {
                available: false,
                used_by: Some("homevault".into()),
                detail: None,
            };
        }
        if Self::host_bound_ports().contains(&port) {
            return PortStatus {
                available: false,
                used_by: Some("host".into()),
                detail: None,
            };
        }
        PortStatus {
            available: true,
            used_by: None,
            detail: None,
        }
    }

    /// Start the Docker daemon via systemd. `homevault` is only in the `docker`
    /// group (see install.sh), so starting the daemon needs root — delegated via
    /// the scoped `systemctl start docker` / `systemctl enable docker` sudoers
    /// grants (see `configure_sudoers`). Enabling first makes the daemon survive
    /// a reboot; starting is what actually brings it up now.
    pub fn start_daemon() -> Result<(), DockerError> {
        let enable = Command::new("sudo")
            .args(["systemctl", "enable", "docker"])
            .output();
        // `enable` failing (e.g. already enabled) is not fatal — only `start`
        // determines success.
        let _ = enable;

        let output = Command::new("sudo")
            .args(["systemctl", "start", "docker"])
            .output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Return the last `lines` of the Docker daemon's journal, for diagnosing why
    /// the daemon is down. `journalctl` is already granted to `homevault` via the
    /// System Journal sudoers rule, so this needs no new grant.
    pub fn get_daemon_logs(lines: u32) -> Result<String, DockerError> {
        let output = Command::new("sudo")
            .args([
                "journalctl",
                "-u",
                "docker",
                "--no-pager",
                "-n",
                &lines.to_string(),
            ])
            .output()?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

// ---------------------------------------------------------------------------
// Network types
// ---------------------------------------------------------------------------

/// A Docker network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerNetwork {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub internal: bool,
    pub containers: Vec<String>,
}

/// List all Docker networks.
pub fn get_networks() -> Result<Vec<DockerNetwork>, DockerError> {
    let output = Command::new("docker")
        .args([
            "network",
            "ls",
            "--format",
            "{{.ID}}|{{.Name}}|{{.Driver}}|{{.Scope}}",
        ])
        .output()?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let mut networks = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 4 {
            let net_id = parts[0].trim();
            let net_name = parts[1].trim();

            // Get detailed info for this network
            let containers = get_network_containers(net_id);

            let (subnet, gateway, internal) = get_network_ipam(net_id);

            networks.push(DockerNetwork {
                id: net_id.to_string(),
                name: net_name.to_string(),
                driver: parts[2].to_string(),
                scope: parts[3].to_string(),
                subnet,
                gateway,
                internal,
                containers,
            });
        }
    }

    Ok(networks)
}

/// Get a single Docker network by name or ID with full details.
pub fn get_network(name: &str) -> Result<DockerNetwork, DockerError> {
    let output = Command::new("docker")
        .args([
            "network",
            "inspect",
            name,
            "--format",
            "{{.ID}}|{{.Name}}|{{.Driver}}|{{.Scope}}|{{.Internal}}",
        ])
        .output()?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let parts: Vec<&str> = line.split('|').collect();

    if parts.len() < 5 {
        return Err(DockerError::CommandFailed(
            "Failed to parse network info".to_string(),
        ));
    }

    let net_id = parts[0].trim();
    let net_name = parts[1].trim();
    let internal = parts[4].trim() == "true";

    let containers = get_network_containers(net_id);
    let (subnet, gateway, _) = get_network_ipam(net_id);

    Ok(DockerNetwork {
        id: net_id.to_string(),
        name: net_name.to_string(),
        driver: parts[2].to_string(),
        scope: parts[3].to_string(),
        subnet,
        gateway,
        internal,
        containers,
    })
}

/// Get container names connected to a network via `docker network inspect`.
fn get_network_containers(network_id: &str) -> Vec<String> {
    let output = Command::new("docker")
        .args([
            "network",
            "inspect",
            network_id,
            "--format",
            "{{range $k, $v := .Containers}}{{$k}} {{end}}",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if raw.is_empty() {
                return vec![];
            }
            // Get container names from the IDs
            raw.split_whitespace()
                .filter_map(|id| {
                    let out = Command::new("docker")
                        .args(["inspect", "--format", "{{.Name}}", id])
                        .output()
                        .ok()?;
                    if out.status.success() {
                        let name = String::from_utf8_lossy(&out.stdout)
                            .trim()
                            .trim_start_matches('/')
                            .to_string();
                        Some(name)
                    } else {
                        Some(id.to_string())
                    }
                })
                .collect()
        }
        _ => vec![],
    }
}

/// Get IPAM info (subnet, gateway, internal) for a network.
fn get_network_ipam(network_id: &str) -> (Option<String>, Option<String>, bool) {
    let output = Command::new("docker")
        .args([
            "network",
            "inspect",
            network_id,
            "--format",
            "{{(index .IPAM.Config 0).Subnet}}|{{(index .IPAM.Config 0).Gateway}}|{{.Internal}}",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let parts: Vec<&str> = raw.split('|').collect();
            let subnet = parts
                .first()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let gateway = parts
                .get(1)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let internal = parts.get(2).map(|s| s.trim() == "true").unwrap_or(false);
            (subnet, gateway, internal)
        }
        _ => (None, None, false),
    }
}

/// Create a Docker network.
pub fn create_network(
    name: &str,
    driver: &str,
    subnet: Option<&str>,
    gateway: Option<&str>,
    internal: bool,
) -> Result<(), DockerError> {
    let mut args = vec!["network".to_string(), "create".to_string()];

    if let Some(sub) = subnet {
        if !sub.is_empty() {
            args.push("--subnet".to_string());
            args.push(sub.to_string());
        }
    }

    if let Some(gw) = gateway {
        if !gw.is_empty() {
            args.push("--gateway".to_string());
            args.push(gw.to_string());
        }
    }

    if internal {
        args.push("--internal".to_string());
    }

    if !driver.is_empty() && driver != "bridge" {
        args.push("--driver".to_string());
        args.push(driver.to_string());
    }

    args.push(name.to_string());

    let output = Command::new("docker").args(&args).output()?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

/// Remove a Docker network by name or ID.
pub fn remove_network(name: &str) -> Result<(), DockerError> {
    let output = Command::new("docker")
        .args(["network", "rm", name])
        .output()?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

/// Connect a container to a network.
pub fn connect_container_to_network(container: &str, network: &str) -> Result<(), DockerError> {
    let output = Command::new("docker")
        .args(["network", "connect", network, container])
        .output()?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

/// Disconnect a container from a network.
pub fn disconnect_container_from_network(
    container: &str,
    network: &str,
) -> Result<(), DockerError> {
    let output = Command::new("docker")
        .args(["network", "disconnect", network, container])
        .output()?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ports_published_ipv4_and_ipv6_deduped() {
        // The real `docker ps` output for a `-p 8096:8096` container.
        let got = DockerService::parse_ports("0.0.0.0:8096->8096/tcp, [::]:8096->8096/tcp");
        assert_eq!(
            got.len(),
            1,
            "IPv4 + IPv6 lines must collapse to one mapping"
        );
        assert_eq!(got[0].host_port, 8096);
        assert_eq!(got[0].container_port, 8096);
        assert_eq!(got[0].protocol, "tcp");
    }

    #[test]
    fn test_parse_ports_distinct_mappings_kept() {
        let got = DockerService::parse_ports("0.0.0.0:8080->80/tcp, 0.0.0.0:8443->443/tcp");
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].host_port, got[0].container_port), (8080, 80));
        assert_eq!((got[1].host_port, got[1].container_port), (8443, 443));
    }

    #[test]
    fn test_parse_ports_random_host_port_and_udp() {
        let got = DockerService::parse_ports("0.0.0.0:32768->53/udp");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].host_port, 32768);
        assert_eq!(got[0].container_port, 53);
        assert_eq!(got[0].protocol, "udp");
    }

    #[test]
    fn test_parse_ports_exposed_only_is_skipped() {
        // No `->` => exposed but not published => not reachable, no web UI.
        assert!(DockerService::parse_ports("8096/tcp").is_empty());
        assert!(DockerService::parse_ports("").is_empty());
    }

    #[test]
    fn test_parse_ports_port_range_is_skipped_gracefully() {
        // Ranges don't parse as a single u16; must not panic, just yield nothing.
        assert!(DockerService::parse_ports("0.0.0.0:5000-5010->5000-5010/tcp").is_empty());
    }

    #[test]
    fn test_port_from_socket_addr() {
        assert_eq!(
            DockerService::port_from_socket_addr("0.0.0.0:8096"),
            Some(8096)
        );
        assert_eq!(
            DockerService::port_from_socket_addr("[::]:8096"),
            Some(8096)
        );
        assert_eq!(
            DockerService::port_from_socket_addr("[::1]:6010"),
            Some(6010)
        );
        assert_eq!(
            DockerService::port_from_socket_addr("127.0.0.1:53"),
            Some(53)
        );
        assert_eq!(DockerService::port_from_socket_addr("*:22"), Some(22));
        assert_eq!(DockerService::port_from_socket_addr("0.0.0.0:*"), None);
        assert_eq!(DockerService::port_from_socket_addr("garbage"), None);
    }

    #[test]
    fn test_check_port_zero_is_never_available() {
        let s = DockerService::check_port(0);
        assert!(!s.available);
        assert_eq!(s.used_by.as_deref(), Some("host"));
    }

    #[test]
    fn test_parse_si_bytes() {
        assert_eq!(DockerService::parse_si_bytes("0B"), Some(0));
        assert_eq!(DockerService::parse_si_bytes("512B"), Some(512));
        assert_eq!(DockerService::parse_si_bytes("1.2kB"), Some(1_200));
        assert_eq!(DockerService::parse_si_bytes(" 3.4MB "), Some(3_400_000));
        assert_eq!(DockerService::parse_si_bytes("2GB"), Some(2_000_000_000));
        assert_eq!(DockerService::parse_si_bytes("garbage"), None);
    }

    #[test]
    fn test_parse_io_pair() {
        assert_eq!(
            DockerService::parse_io_pair("1.2kB / 3.4MB"),
            (Some(1_200), Some(3_400_000))
        );
        // BlockIO is frequently "0B / 0B".
        assert_eq!(DockerService::parse_io_pair("0B / 0B"), (Some(0), Some(0)));
        assert_eq!(DockerService::parse_io_pair(""), (None, None));
    }
}
