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
//! networks, volumes, builds, registry credentials, daemon config).

use serde::{Deserialize, Serialize};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Ceiling for a Docker CLI call that should answer quickly (inspect, ps,
/// start, stop, rm, network ops). Without it a wedged daemon (a stuck
/// `dockerd`, an NFS-backed graph driver) would block a request forever.
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling for operations that legitimately take minutes: image pull, build,
/// `compose up`, `system prune`. Still bounded — an unbounded CLI call is a
/// hung request and a leaked process.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(900);

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
    /// The CLI did not answer within its budget and was killed. Reported
    /// separately from `CommandFailed` so the UI can say "timed out" instead
    /// of echoing an empty stderr.
    #[error("Docker command timed out after {0}s")]
    Timeout(u64),
}

/// Run a `docker` command with a hard timeout, capturing both streams.
///
/// `std::process::Command::output()` has no timeout at all: a daemon that
/// stops answering hangs the calling thread indefinitely. Every Docker call in
/// this module goes through here so the worst case is a bounded failure.
pub fn run_docker(args: &[&str], timeout: Duration) -> Result<Output, DockerError> {
    let mut cmd = Command::new("docker");
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_command(cmd, timeout)
}

/// Spawn `cmd`, drain stdout/stderr on two threads, and kill it when
/// `timeout` elapses. Returns the captured [`Output`].
pub fn run_command(mut cmd: Command, timeout: Duration) -> Result<Output, DockerError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn()?;
    wait_with_timeout(child, timeout)
}

/// Same as [`run_command`], but feeds `stdin_data` to the child. Used by
/// `docker login --password-stdin` and `sudo tee`, which deliberately keep the
/// secret off the command line (visible in `/proc/<pid>/cmdline` to every
/// local user).
pub fn run_command_with_stdin(
    mut cmd: Command,
    stdin_data: &str,
    timeout: Duration,
) -> Result<Output, DockerError> {
    use std::io::Write;

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        let data = stdin_data.as_bytes().to_vec();
        // Writing from a thread keeps a large payload from deadlocking
        // against a child that fills its own stdout before reading stdin.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        });
    }
    wait_with_timeout(child, timeout)
}

/// Poll `child` until it exits or `timeout` elapses (then SIGKILL it), while
/// two threads drain its piped streams.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<Output, DockerError> {
    use std::io::Read;

    let mut child_out = child.stdout.take();
    let mut child_err = child.stderr.take();

    let out_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_out.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });
    let err_h = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_err.as_mut() {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    // SIGKILL: `docker` only forwards SIGTERM to the engine,
                    // which may itself be the thing that is stuck.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DockerError::Timeout(timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    };

    Ok(Output {
        status,
        stdout: out_h.join().unwrap_or_default(),
        stderr: err_h.join().unwrap_or_default(),
    })
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
        run_docker(&["info"], SHORT_TIMEOUT)
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
        let output = run_docker(
            &[
                "ps",
                "-a",
                "--format",
                "{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}|{{.State}}|{{.CreatedAt}}|{{.Ports}}|{{.Label \"com.docker.compose.project\"}}",
            ],
            SHORT_TIMEOUT,
        )?;

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
        let output = run_docker(
            &[
                "stats",
                "--no-stream",
                "--format",
                "{{.ID}}|{{.MemUsage}}|{{.CPUPerc}}|{{.NetIO}}|{{.BlockIO}}",
            ],
            SHORT_TIMEOUT,
        )?;

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
        let output = run_docker(
            &["images", "--format", "{{.Repository}}:{{.Tag}}"],
            SHORT_TIMEOUT,
        )?;

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
        let output = run_docker(
            &[
                "ps",
                "--format",
                "{{.ID}}|{{.Names}}|{{.Image}}|{{.Status}}|{{.State}}|{{.CreatedAt}}|{{.Ports}}|{{.Label \"com.docker.compose.project\"}}",
            ],
            SHORT_TIMEOUT,
        )?;

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
        let output = run_docker(&["inspect", name], SHORT_TIMEOUT)?;

        if !output.status.success() {
            return Err(DockerError::ContainerNotFound(name.to_string()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Return the last `lines` of log output from a container.
    ///
    /// `docker logs` writes the container's stdout to *stdout* and its stderr
    /// to *stderr*; a container that logs on stderr (most daemons) used to
    /// come back with an empty log view. Both streams are returned, stdout
    /// first, so nothing is silently dropped.
    pub fn get_container_logs(name: &str, lines: u32) -> Result<String, DockerError> {
        let output = Self::get_container_logs_full(name, lines, false, false)?;
        Ok(output)
    }

    /// Full log read: bounded `tail`, optional timestamps, optional
    /// stderr-only view. Both streams are merged (stdout then stderr) and
    /// trailing newlines normalised.
    pub fn get_container_logs_full(
        name: &str,
        lines: u32,
        timestamps: bool,
        stderr_only: bool,
    ) -> Result<String, DockerError> {
        let tail = lines.to_string();
        let mut args = vec!["logs", "--tail", tail.as_str()];
        if timestamps {
            args.push("--timestamps");
        }
        if stderr_only {
            args.push("--stderr-only");
        } else {
            // Keep both streams: `docker logs` without a stream flag already
            // emits both, so nothing extra is needed here.
        }
        args.push(name);

        let output = run_docker(&args, SHORT_TIMEOUT)?;
        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        let mut merged = String::from_utf8_lossy(&output.stdout).to_string();
        let err = String::from_utf8_lossy(&output.stderr);
        if !err.trim().is_empty() {
            if !merged.is_empty() && !merged.ends_with('\n') {
                merged.push('\n');
            }
            merged.push_str(&err);
        }
        Ok(merged)
    }

    /// Start a stopped container by name.
    pub fn start_container(name: &str) -> Result<(), DockerError> {
        let output = run_docker(&["start", name], SHORT_TIMEOUT)?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Stop a running container by name.
    pub fn stop_container(name: &str) -> Result<(), DockerError> {
        let output = run_docker(&["stop", name], SHORT_TIMEOUT)?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Restart a container by name.
    pub fn restart_container(name: &str) -> Result<(), DockerError> {
        let output = run_docker(&["restart", name], SHORT_TIMEOUT)?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// List all locally cached Docker images.
    pub fn get_images() -> Result<Vec<Image>, DockerError> {
        let output = run_docker(
            &[
                "images",
                "--format",
                "{{.ID}}|{{.Repository}}|{{.Tag}}|{{.Size}}|{{.CreatedAt}}",
            ],
            SHORT_TIMEOUT,
        )?;

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
        let output = run_docker(
            &["info", "--format", "{{.ServerVersion}}|{{.Architecture}}|{{.OSType}}|{{.Containers}}|{{.ContainersRunning}}|{{.ContainersPaused}}|{{.ContainersStopped}}|{{.Images}}"],
            SHORT_TIMEOUT,
        )?;

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
        let output = run_docker(
            &["version", "--format", "{{.Server.Version}}"],
            SHORT_TIMEOUT,
        )?;

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

        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = run_docker(&refs, LONG_TIMEOUT)?;

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

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = run_docker(&arg_refs, SHORT_TIMEOUT)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Pull a Docker image from the configured registry.
    pub fn pull_image(image: &str, tag: &str) -> Result<(), DockerError> {
        let reference = format!("{}:{}", image, tag);
        let output = run_docker(&["pull", &reference], LONG_TIMEOUT)?;

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

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = run_docker(&arg_refs, SHORT_TIMEOUT)?;

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
        let output = run_docker(
            &[
                "inspect",
                "--format",
                "{{.NetworkSettings.Networks}}",
                container_id,
            ],
            SHORT_TIMEOUT,
        );

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
        let Ok(out) = run_docker(
            &["ps", "-a", "--format", "{{.Names}}|{{.Ports}}"],
            SHORT_TIMEOUT,
        ) else {
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
        // `enable` failing (e.g. already enabled) is not fatal — only `start`
        // determines success.
        let _ = run_command(
            {
                let mut c = Command::new("sudo");
                c.args(["systemctl", "enable", "docker"]);
                c
            },
            SHORT_TIMEOUT,
        );

        let output = run_command(
            {
                let mut c = Command::new("sudo");
                c.args(["systemctl", "start", "docker"]);
                c
            },
            LONG_TIMEOUT,
        )?;

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
        let n = lines.to_string();
        let output = run_command(
            {
                let mut c = Command::new("sudo");
                c.args(["journalctl", "-u", "docker", "--no-pager", "-n", n.as_str()]);
                c
            },
            SHORT_TIMEOUT,
        )?;

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
    let output = run_docker(
        &[
            "network",
            "ls",
            "--format",
            "{{.ID}}|{{.Name}}|{{.Driver}}|{{.Scope}}",
        ],
        SHORT_TIMEOUT,
    )?;

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
    let output = run_docker(
        &[
            "network",
            "inspect",
            name,
            "--format",
            "{{.ID}}|{{.Name}}|{{.Driver}}|{{.Scope}}|{{.Internal}}",
        ],
        SHORT_TIMEOUT,
    )?;

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
    let output = run_docker(
        &[
            "network",
            "inspect",
            network_id,
            "--format",
            "{{range $k, $v := .Containers}}{{$k}} {{end}}",
        ],
        SHORT_TIMEOUT,
    );

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if raw.is_empty() {
                return vec![];
            }
            // Get container names from the IDs
            raw.split_whitespace()
                .filter_map(|id| {
                    let out = run_docker(&["inspect", "--format", "{{.Name}}", id], SHORT_TIMEOUT)
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
    let output = run_docker(
        &[
            "network",
            "inspect",
            network_id,
            "--format",
            "{{(index .IPAM.Config 0).Subnet}}|{{(index .IPAM.Config 0).Gateway}}|{{.Internal}}",
        ],
        SHORT_TIMEOUT,
    );

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

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let output = run_docker(&arg_refs, SHORT_TIMEOUT)?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

/// Remove a Docker network by name or ID.
pub fn remove_network(name: &str) -> Result<(), DockerError> {
    let output = run_docker(&["network", "rm", name], SHORT_TIMEOUT)?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

/// Connect a container to a network.
pub fn connect_container_to_network(container: &str, network: &str) -> Result<(), DockerError> {
    let output = run_docker(&["network", "connect", network, container], SHORT_TIMEOUT)?;

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
    let output = run_docker(
        &["network", "disconnect", network, container],
        SHORT_TIMEOUT,
    )?;

    if !output.status.success() {
        return Err(DockerError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Volumes
// ---------------------------------------------------------------------------

/// A Docker named volume, with the containers currently mounting it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub mountpoint: String,
    /// `true` when at least one container (running or stopped) mounts it —
    /// Docker refuses to remove those without `--force`.
    pub in_use: bool,
    /// Names of the containers mounting it (empty when unused).
    pub used_by: Vec<String>,
}

// ---------------------------------------------------------------------------
// Disk usage / prune
// ---------------------------------------------------------------------------

/// One row of `docker system df`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskUsageRow {
    /// `Images`, `Containers`, `Local Volumes`, `Build Cache`.
    pub kind: String,
    pub total: u64,
    pub active: u64,
    pub size: String,
    pub reclaimable: String,
}

/// Parsed `docker system df` summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskUsage {
    pub rows: Vec<DiskUsageRow>,
    pub total_size: String,
    pub total_reclaimable: String,
}

/// What a prune operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PruneTarget {
    /// Dangling images (`-a` also removes unused tagged images).
    Images,
    Volumes,
    Containers,
    Networks,
    Builders,
    /// `docker system prune` — stopped containers, unused networks, dangling
    /// images, build cache. `-a` widens it to unused images and (with
    /// `--volumes`) anonymous volumes.
    System,
}

impl PruneTarget {
    /// Parse the wire value. Unknown values are rejected rather than silently
    /// mapping to a destructive default.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "images" => Some(Self::Images),
            "volumes" => Some(Self::Volumes),
            "containers" => Some(Self::Containers),
            "networks" => Some(Self::Networks),
            "builders" => Some(Self::Builders),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Images => "images",
            Self::Volumes => "volumes",
            Self::Containers => "containers",
            Self::Networks => "networks",
            Self::Builders => "builders",
            Self::System => "system",
        }
    }
}

/// Outcome of a prune run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneReport {
    pub target: String,
    /// Space freed, as Docker prints it (e.g. `"1.2GB"`), when available.
    pub reclaimed: String,
    /// Full CLI output, for the "details" disclosure in the UI.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Container detail (parsed `docker inspect`)
// ---------------------------------------------------------------------------

/// A mount attached to a container (bind, volume or tmpfs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerMount {
    /// `bind`, `volume` or `tmpfs`.
    pub kind: String,
    pub source: String,
    pub destination: String,
    pub read_only: bool,
    /// Volume name, for `kind == "volume"`.
    pub name: Option<String>,
}

/// Everything the container detail panel shows, parsed from `docker inspect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerDetail {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    /// Image digest (`sha256:…`) when the daemon reports one.
    pub image_digest: Option<String>,
    pub created: String,
    pub state: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub restart_count: i64,
    pub pid: Option<i64>,
    pub command: Vec<String>,
    pub entrypoint: Vec<String>,
    pub working_dir: String,
    pub hostname: String,
    pub user: String,
    pub restart_policy: String,
    pub privileged: bool,
    /// Memory limit in bytes (`0` when unlimited).
    pub memory_limit: Option<u64>,
    pub nano_cpus: Option<i64>,
    pub env: Vec<String>,
    pub labels: Vec<(String, String)>,
    pub mounts: Vec<ContainerMount>,
    pub ports: Vec<PortMapping>,
    pub networks: Vec<String>,
    pub ip_address: Option<String>,
    /// `healthy`, `unhealthy`, `starting`, or `None` when no healthcheck.
    pub health: Option<String>,
    pub log_driver: Option<String>,
    pub log_path: Option<String>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
}

/// Result of a non-interactive `docker exec`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Parameters for building an image from a Dockerfile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildImageRequest {
    /// Host directory used as the build context. Must exist.
    pub context: String,
    /// Dockerfile name or path, relative to the context (default
    /// `Dockerfile`).
    #[serde(default)]
    pub dockerfile: Option<String>,
    /// `name:tag` to tag the built image with.
    pub tag: String,
    #[serde(default)]
    pub no_cache: bool,
    /// Always attempt to pull a newer base image.
    #[serde(default)]
    pub pull: bool,
    #[serde(default)]
    pub build_args: Vec<EnvVar>,
}

// ---------------------------------------------------------------------------
// Log-stream tickets
// ---------------------------------------------------------------------------

/// A redeemed log-stream ticket: the exact parameters the admin asked for, so
/// the SSE endpoint never trusts a query string for them.
#[derive(Debug, Clone)]
pub struct LogTicket {
    pub container: String,
    pub tail: u32,
    pub follow: bool,
    pub timestamps: bool,
}

/// How long a log ticket stays valid. Short on purpose: it is redeemed by the
/// `EventSource` created immediately after the ticket response.
const LOG_TICKET_TTL: Duration = Duration::from_secs(30);

static LOG_TICKETS: once_cell::sync::Lazy<
    parking_lot::RwLock<std::collections::HashMap<String, (LogTicket, Instant)>>,
> = once_cell::sync::Lazy::new(|| parking_lot::RwLock::new(std::collections::HashMap::new()));

/// Mint a single-use ticket for the container log SSE stream.
///
/// `EventSource` cannot send the `X-Session-Token` header, so the frontend
/// exchanges its session for this ticket over a normal authenticated request —
/// the same pattern as the notifications and explorer watchers. The session
/// token never travels in a URL.
pub fn issue_log_ticket(container: &str, tail: u32, follow: bool, timestamps: bool) -> String {
    let ticket = uuid::Uuid::new_v4().to_string();
    let now = Instant::now();
    let mut tickets = LOG_TICKETS.write();
    tickets.retain(|_, (_, issued)| now.duration_since(*issued) < LOG_TICKET_TTL);
    tickets.insert(
        ticket.clone(),
        (
            LogTicket {
                container: container.to_string(),
                tail,
                follow,
                timestamps,
            },
            now,
        ),
    );
    ticket
}

/// Redeem a log ticket. Single use, expires after [`LOG_TICKET_TTL`].
pub fn consume_log_ticket(ticket: &str) -> Option<LogTicket> {
    let (entry, issued) = LOG_TICKETS.write().remove(ticket)?;
    (issued.elapsed() < LOG_TICKET_TTL).then_some(entry)
}

// ---------------------------------------------------------------------------
// Extended operations
// ---------------------------------------------------------------------------

impl DockerService {
    // ── Volumes ────────────────────────────────────────────────────────────

    /// List named volumes and the containers mounting them.
    pub fn list_volumes() -> Result<Vec<Volume>, DockerError> {
        let output = run_docker(
            &[
                "volume",
                "ls",
                "--format",
                "{{.Name}}|{{.Driver}}|{{.Scope}}|{{.Mountpoint}}",
            ],
            SHORT_TIMEOUT,
        )?;

        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        // One `docker ps` call gives every container's mounts; deriving the
        // volume → container map from it avoids an inspect per volume.
        let usage = Self::volume_usage();

        let mut volumes = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 4 {
                continue;
            }
            let name = parts[0].trim().to_string();
            if name.is_empty() {
                continue;
            }
            let used_by = usage.get(&name).cloned().unwrap_or_default();
            volumes.push(Volume {
                in_use: !used_by.is_empty(),
                used_by,
                name,
                driver: parts[1].trim().to_string(),
                scope: parts[2].trim().to_string(),
                mountpoint: parts[3].trim().to_string(),
            });
        }
        volumes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(volumes)
    }

    /// Map volume name → container names mounting it, from
    /// `docker ps -a --format '{{.Names}}|{{.Mounts}}'`.
    fn volume_usage() -> std::collections::HashMap<String, Vec<String>> {
        let mut map: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let Ok(out) = run_docker(
            &["ps", "-a", "--format", "{{.Names}}|{{.Mounts}}"],
            SHORT_TIMEOUT,
        ) else {
            return map;
        };
        if !out.status.success() {
            return map;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some((name, mounts)) = line.split_once('|') else {
                continue;
            };
            for entry in mounts.split(',') {
                // Each entry looks like `myvol` or `myvol:/data` or a bind
                // path. A named volume has no `/` in its first token.
                let first = entry.trim().split(':').next().unwrap_or("").trim();
                if first.is_empty() || first.contains('/') {
                    continue;
                }
                map.entry(first.to_string())
                    .or_default()
                    .push(name.trim().to_string());
            }
        }
        map
    }

    /// Create a named volume. `name` is validated by the handler; `driver`
    /// defaults to `local`.
    pub fn create_volume(
        name: &str,
        driver: Option<&str>,
        labels: &[(String, String)],
    ) -> Result<(), DockerError> {
        let mut args: Vec<String> = vec!["volume".into(), "create".into()];
        if let Some(d) = driver.filter(|d| !d.is_empty() && *d != "local") {
            args.push("--driver".into());
            args.push(d.to_string());
        }
        for (k, v) in labels {
            args.push("--label".into());
            args.push(format!("{}={}", k, v));
        }
        args.push(name.to_string());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let output = run_docker(&refs, SHORT_TIMEOUT)?;
        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    /// Remove a named volume. `force` allows removing one still in use.
    pub fn remove_volume(name: &str, force: bool) -> Result<(), DockerError> {
        let mut args = vec!["volume", "rm"];
        if force {
            args.push("-f");
        }
        args.push(name);
        let output = run_docker(&args, SHORT_TIMEOUT)?;
        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    // ── Prune / disk usage ─────────────────────────────────────────────────

    /// Parse `docker system df` into structured rows.
    ///
    /// The table is fixed-width and `Local Volumes` / `Build Cache` contain a
    /// space, so the type is matched by prefix and the remaining columns are
    /// read from the right (size and reclaimable may themselves be `1.2GB
    /// (66%)`). A row that doesn't parse is skipped rather than guessed at.
    pub fn parse_system_df(stdout: &str) -> DiskUsage {
        let mut rows = Vec::new();
        let mut total_size = String::new();
        let mut total_reclaimable = String::new();

        for line in stdout.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() {
                continue;
            }
            let kind = if line.starts_with("Images") {
                "Images"
            } else if line.starts_with("Containers") {
                "Containers"
            } else if line.starts_with("Local Volumes") {
                "Local Volumes"
            } else if line.starts_with("Build Cache") {
                "Build Cache"
            } else {
                continue;
            };

            let rest = line[kind.len()..].trim();
            // `<total> <active> <size> <reclaimable>` — reclaimable may carry
            // a trailing percentage in parentheses.
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            if tokens.len() < 4 {
                continue;
            }
            let total = tokens[0].parse::<u64>().unwrap_or(0);
            let active = tokens[1].parse::<u64>().unwrap_or(0);
            let size = tokens[2].to_string();
            let mut reclaimable = tokens[3].to_string();
            if tokens.len() > 4 {
                reclaimable.push(' ');
                reclaimable.push_str(&tokens[4..].join(" "));
            }

            if kind == "Images" {
                total_size = size.clone();
            }
            if kind == "Images" {
                total_reclaimable = reclaimable.clone();
            }
            rows.push(DiskUsageRow {
                kind: kind.to_string(),
                total,
                active,
                size,
                reclaimable,
            });
        }

        DiskUsage {
            rows,
            total_size,
            total_reclaimable,
        }
    }

    /// `docker system df`, parsed.
    pub fn disk_usage() -> Result<DiskUsage, DockerError> {
        let output = run_docker(&["system", "df"], SHORT_TIMEOUT)?;
        if !output.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(Self::parse_system_df(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    /// Reclaim space. `all` widens images (unused tagged ones) and system
    /// (unused images + anonymous volumes).
    pub fn prune(target: PruneTarget, all: bool) -> Result<PruneReport, DockerError> {
        let mut args: Vec<String> = match target {
            PruneTarget::Images => vec!["image".into(), "prune".into()],
            PruneTarget::Volumes => vec!["volume".into(), "prune".into()],
            PruneTarget::Containers => vec!["container".into(), "prune".into()],
            PruneTarget::Networks => vec!["network".into(), "prune".into()],
            PruneTarget::Builders => vec!["builder".into(), "prune".into()],
            PruneTarget::System => vec!["system".into(), "prune".into()],
        };
        args.push("-f".into());
        if all && matches!(target, PruneTarget::Images | PruneTarget::System) {
            args.push("-a".into());
        }
        if all && matches!(target, PruneTarget::System) {
            args.push("--volumes".into());
        }
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let output = run_docker(&refs, LONG_TIMEOUT)?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !output.status.success() {
            return Err(DockerError::CommandFailed(if stderr.is_empty() {
                stdout
            } else {
                stderr
            }));
        }

        Ok(PruneReport {
            target: target.as_str().to_string(),
            reclaimed: Self::parse_reclaimed(&stdout).unwrap_or_default(),
            message: stdout,
        })
    }

    /// Pull `Total reclaimed space: 1.2GB` out of a prune's output.
    ///
    /// `docker builder prune` is the odd one out: it prints a table whose last
    /// line is `Total:\t12.29kB` instead of the sentence every other prune
    /// command emits, so that shape is handled too.
    fn parse_reclaimed(stdout: &str) -> Option<String> {
        if let Some(size) = stdout
            .lines()
            .find_map(|l| l.trim().strip_prefix("Total reclaimed space:"))
        {
            return Some(size.trim().to_string());
        }
        stdout.lines().find_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("Total:")?;
            // Guard against a bare header line with no size.
            rest.split_whitespace().last().map(|s| s.to_string())
        })
    }

    // ── Container lifecycle extras ─────────────────────────────────────────

    /// Freeze a running container (`docker pause`).
    pub fn pause_container(name: &str) -> Result<(), DockerError> {
        Self::expect_ok(run_docker(&["pause", name], SHORT_TIMEOUT)?)
    }

    /// Resume a paused container (`docker unpause`).
    pub fn unpause_container(name: &str) -> Result<(), DockerError> {
        Self::expect_ok(run_docker(&["unpause", name], SHORT_TIMEOUT)?)
    }

    /// Send a signal (default SIGKILL) to a container.
    pub fn kill_container(name: &str, signal: Option<&str>) -> Result<(), DockerError> {
        let mut args = vec!["kill"];
        if let Some(sig) = signal.filter(|s| !s.is_empty()) {
            args.push("--signal");
            args.push(sig);
        }
        args.push(name);
        Self::expect_ok(run_docker(&args, SHORT_TIMEOUT)?)
    }

    /// Rename a container.
    pub fn rename_container(name: &str, new_name: &str) -> Result<(), DockerError> {
        Self::expect_ok(run_docker(&["rename", name, new_name], SHORT_TIMEOUT)?)
    }

    /// Change a running/stopped container's resource limits and restart
    /// policy in place (`docker update`) — no recreation, no data loss.
    pub fn update_container(
        name: &str,
        memory: Option<&str>,
        cpus: Option<&str>,
        restart_policy: Option<&str>,
    ) -> Result<(), DockerError> {
        let mut args = vec!["update"];
        let mut touched = false;
        if let Some(m) = memory.filter(|m| !m.trim().is_empty()) {
            args.push("--memory");
            args.push(m.trim());
            touched = true;
        }
        if let Some(c) = cpus.filter(|c| !c.trim().is_empty()) {
            args.push("--cpus");
            args.push(c.trim());
            touched = true;
        }
        if let Some(p) = restart_policy.filter(|p| !p.trim().is_empty()) {
            if !matches!(p, "no" | "on-failure" | "always" | "unless-stopped") {
                return Err(DockerError::CommandFailed(format!(
                    "Unsupported restart policy: {}",
                    p
                )));
            }
            args.push("--restart");
            args.push(p);
            touched = true;
        }
        if !touched {
            return Err(DockerError::CommandFailed(
                "Nothing to update: provide memory, cpus or a restart policy".to_string(),
            ));
        }
        args.push(name);
        Self::expect_ok(run_docker(&args, SHORT_TIMEOUT)?)
    }

    /// Map a successful `docker` [`Output`] to `Ok(())`, else the stderr text.
    fn expect_ok(output: Output) -> Result<(), DockerError> {
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(DockerError::CommandFailed(if stderr.is_empty() {
                "docker command failed".to_string()
            } else {
                stderr
            }))
        }
    }

    // ── Inspect ────────────────────────────────────────────────────────────

    /// Read and parse `docker inspect` into a [`ContainerDetail`].
    pub fn get_container_detail(name: &str) -> Result<ContainerDetail, DockerError> {
        let raw = Self::get_container_info(name)?;
        Self::parse_container_detail(&raw)
    }

    /// Parse the JSON array `docker inspect` emits (it always returns an
    /// array, even for a single container).
    pub fn parse_container_detail(raw: &str) -> Result<ContainerDetail, DockerError> {
        let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
            DockerError::CommandFailed(format!("Failed to parse container inspect: {}", e))
        })?;
        let obj = match &value {
            serde_json::Value::Array(arr) => arr.first().cloned().unwrap_or_default(),
            other => other.clone(),
        };
        if !obj.is_object() {
            return Err(DockerError::ContainerNotFound(
                "inspect returned no container".to_string(),
            ));
        }

        let s = |ptr: &str| -> Option<String> {
            obj.pointer(ptr)
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
        };
        let str_list = |ptr: &str| -> Vec<String> {
            obj.pointer(ptr)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|x| x.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let num = |ptr: &str| -> Option<i64> { obj.pointer(ptr).and_then(|v| v.as_i64()) };

        let env = str_list("/Config/Env");
        let labels: Vec<(String, String)> = obj
            .pointer("/Config/Labels")
            .and_then(|v| v.as_object())
            .map(|m| {
                let mut out: Vec<(String, String)> = m
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                    .collect();
                out.sort_by(|a, b| a.0.cmp(&b.0));
                out
            })
            .unwrap_or_default();

        let mounts = obj
            .pointer("/Mounts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|m| ContainerMount {
                        kind: m
                            .get("Type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        source: m
                            .get("Source")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        destination: m
                            .get("Destination")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        read_only: m
                            .get("RW")
                            .and_then(|v| v.as_bool())
                            .map(|rw| !rw)
                            .unwrap_or(false),
                        name: m
                            .get("Name")
                            .and_then(|v| v.as_str())
                            .filter(|n| !n.is_empty())
                            .map(|n| n.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let networks: Vec<String> = obj
            .pointer("/NetworkSettings/Networks")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let ip_address = obj
            .pointer("/NetworkSettings/Networks")
            .and_then(|v| v.as_object())
            .and_then(|m| {
                m.values().find_map(|n| {
                    n.get("IPAddress")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                })
            });

        let ports: Vec<PortMapping> = obj
            .pointer("/HostConfig/PortBindings")
            .and_then(|v| v.as_object())
            .map(|m| {
                let mut out = Vec::new();
                for (container_spec, binds) in m {
                    let (cport, proto) = container_spec
                        .split_once('/')
                        .unwrap_or((container_spec.as_str(), "tcp"));
                    let Ok(container_port) = cport.parse::<u16>() else {
                        continue;
                    };
                    let Some(binds) = binds.as_array() else {
                        continue;
                    };
                    for b in binds {
                        let host = b
                            .get("HostPort")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim();
                        if let Ok(host_port) = host.parse::<u16>() {
                            if !out.iter().any(|p: &PortMapping| {
                                p.host_port == host_port
                                    && p.container_port == container_port
                                    && p.protocol == proto
                            }) {
                                out.push(PortMapping {
                                    host_port,
                                    container_port,
                                    protocol: proto.to_string(),
                                });
                            }
                        }
                    }
                }
                out
            })
            .unwrap_or_default();

        let name = s("/Name")
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_default();

        Ok(ContainerDetail {
            id: s("/Id").unwrap_or_default(),
            name,
            image: s("/Config/Image").unwrap_or_default(),
            image_id: s("/Image").unwrap_or_default(),
            image_digest: s("/Config/Image")
                .filter(|i| i.contains('@'))
                .map(|i| i.split('@').nth(1).unwrap_or("").to_string()),
            created: s("/Created").unwrap_or_default(),
            state: s("/State/Status").unwrap_or_default(),
            status: s("/State/Status").unwrap_or_default(),
            exit_code: num("/State/ExitCode"),
            started_at: s("/State/StartedAt"),
            finished_at: s("/State/FinishedAt"),
            restart_count: num("/RestartCount").unwrap_or(0),
            pid: num("/State/Pid"),
            command: str_list("/Config/Cmd"),
            entrypoint: str_list("/Config/Entrypoint"),
            working_dir: s("/Config/WorkingDir").unwrap_or_default(),
            hostname: s("/Config/Hostname").unwrap_or_default(),
            user: s("/Config/User").unwrap_or_default(),
            restart_policy: s("/HostConfig/RestartPolicy/Name").unwrap_or_else(|| "no".to_string()),
            privileged: obj
                .pointer("/HostConfig/Privileged")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            memory_limit: num("/HostConfig/Memory").map(|m| m as u64),
            nano_cpus: num("/HostConfig/NanoCpus"),
            env,
            labels,
            mounts,
            ports,
            networks,
            ip_address,
            health: s("/State/Health/Status"),
            log_driver: s("/HostConfig/LogConfig/Type"),
            log_path: s("/LogPath"),
            compose_project: obj
                .pointer("/Config/Labels/com.docker.compose.project")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            compose_service: obj
                .pointer("/Config/Labels/com.docker.compose.service")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }

    // ── Image update / recreate ────────────────────────────────────────────

    /// Rebuild the `docker run` argument list for an existing container from
    /// its `docker inspect` JSON, so it can be recreated on a newer image
    /// without the user re-entering anything.
    ///
    /// Deliberately replayed: image, name, env, published ports, mounts,
    /// restart policy, network(s), command/entrypoint, working dir, hostname,
    /// user, privileged, memory/CPU limits, DNS. Deliberately *not* replayed:
    /// runtime state (`State.*`), and HomeVault-own labels are dropped while
    /// `com.docker.compose.*` labels are dropped too (the container is no
    /// longer the compose one; compose would otherwise adopt it).
    pub fn run_args_from_inspect(raw: &str) -> Result<Vec<String>, DockerError> {
        let detail = Self::parse_container_detail(raw)?;
        if detail.image.is_empty() {
            return Err(DockerError::CommandFailed(
                "Container has no image reference".to_string(),
            ));
        }

        let mut args: Vec<String> = vec!["run".into(), "-d".into()];
        args.push("--name".into());
        args.push(detail.name.clone());

        for port in &detail.ports {
            args.push("-p".into());
            args.push(format!(
                "{}:{}/{}",
                port.host_port, port.container_port, port.protocol
            ));
        }

        for mount in &detail.mounts {
            let spec = match mount.kind.as_str() {
                "volume" => {
                    let src = mount
                        .name
                        .clone()
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| mount.source.clone());
                    format!("{}:{}", src, mount.destination)
                }
                "tmpfs" => continue,
                _ => format!("{}:{}", mount.source, mount.destination),
            };
            args.push("-v".into());
            args.push(if mount.read_only {
                format!("{}:ro", spec)
            } else {
                spec
            });
        }

        for entry in &detail.env {
            args.push("-e".into());
            args.push(entry.clone());
        }

        if detail.restart_policy != "no" {
            args.push("--restart".into());
            args.push(detail.restart_policy.clone());
        }
        if !detail.working_dir.is_empty() {
            args.push("-w".into());
            args.push(detail.working_dir.clone());
        }
        if !detail.hostname.is_empty() {
            args.push("-h".into());
            args.push(detail.hostname.clone());
        }
        if !detail.user.is_empty() {
            args.push("-u".into());
            args.push(detail.user.clone());
        }
        if detail.privileged {
            args.push("--privileged".into());
        }
        if let Some(mem) = detail.memory_limit.filter(|m| *m > 0) {
            args.push("-m".into());
            args.push(mem.to_string());
        }
        if let Some(nano) = detail.nano_cpus.filter(|n| *n > 0) {
            args.push("--cpus".into());
            args.push(format!("{:.2}", nano as f64 / 1_000_000_000.0));
        }

        // `docker inspect` reports the *effective* network; `bridge` is the
        // default and passing it explicitly is harmless but noisy.
        for net in &detail.networks {
            if net != "bridge" {
                args.push("--network".into());
                args.push(net.clone());
            }
        }

        for label in &detail.labels {
            if label.0.starts_with("com.docker.compose.") {
                continue;
            }
            args.push("--label".into());
            args.push(format!("{}={}", label.0, label.1));
        }

        // `--entrypoint` must precede the image. It takes a single string, so
        // it is only replayed when the container's entrypoint is a single
        // argument — otherwise the image's own entrypoint is reused, which is
        // what a container created from the same image has anyway.
        if detail.entrypoint.len() == 1 {
            args.push("--entrypoint".into());
            args.push(detail.entrypoint[0].clone());
        }

        args.push(detail.image.clone());
        for cmd in &detail.command {
            args.push(cmd.clone());
        }

        Ok(args)
    }

    /// Pull the container's image and recreate the container from it,
    /// preserving every setting. Returns the new container id (stdout of
    /// `docker run`).
    ///
    /// The old container is renamed first and only removed once the new one
    /// started; on failure it is renamed back, so a failed update never
    /// leaves the user without a container.
    pub fn recreate_container(name: &str) -> Result<String, DockerError> {
        let raw = Self::get_container_info(name)?;
        let detail = Self::parse_container_detail(&raw)?;

        // 1. Pull first: a failed pull must not touch the running container.
        let image_ref = detail.image.clone();
        let pull = run_docker(&["pull", &image_ref], LONG_TIMEOUT)?;
        if !pull.status.success() {
            return Err(DockerError::CommandFailed(format!(
                "Pull failed: {}",
                String::from_utf8_lossy(&pull.stderr).trim()
            )));
        }

        // 2. Rebuild the run arguments from the pre-pull inspect (same spec).
        let args = Self::run_args_from_inspect(&raw)?;

        // 3. Park the old container under a temporary name.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let parked = format!("{}-hv-old-{}", name, stamp);
        Self::expect_ok(run_docker(&["rename", name, &parked], SHORT_TIMEOUT)?)?;

        // 4. Start the replacement.
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match run_docker(&refs, LONG_TIMEOUT) {
            Ok(out) if out.status.success() => {
                // 5. Success: drop the parked container (best effort — a
                // failure here leaves a stopped leftover, not a broken app).
                let _ = run_docker(&["rm", "-f", &parked], SHORT_TIMEOUT);
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                // Roll back: the user must keep a working container.
                let _ = run_docker(&["rename", &parked, name], SHORT_TIMEOUT);
                Err(DockerError::CommandFailed(format!(
                    "Recreate failed: {}",
                    if err.is_empty() {
                        "docker run failed"
                    } else {
                        &err
                    }
                )))
            }
            Err(e) => {
                let _ = run_docker(&["rename", &parked, name], SHORT_TIMEOUT);
                Err(e)
            }
        }
    }

    // ── Logs (streaming) ───────────────────────────────────────────────────

    /// Build the `docker logs` command used by the SSE log follower.
    ///
    /// Returned as a [`Command`] (not run here) so the caller can spawn it and
    /// read line by line; `run_docker` buffers, which is the opposite of what
    /// a live tail needs.
    pub fn logs_command(name: &str, lines: u32, follow: bool, timestamps: bool) -> Command {
        let mut cmd = Command::new("docker");
        cmd.arg("logs").arg("--tail").arg(lines.to_string());
        if follow {
            cmd.arg("--follow");
        }
        if timestamps {
            cmd.arg("--timestamps");
        }
        cmd.arg(name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    // ── Exec ───────────────────────────────────────────────────────────────

    /// Run a command inside a running container (`docker exec`, no TTY) and
    /// return its output. Admin-only at the handler level.
    pub fn exec_in_container(
        name: &str,
        command: &[String],
        timeout: Duration,
    ) -> Result<ExecResult, DockerError> {
        if command.is_empty() {
            return Err(DockerError::CommandFailed("No command given".to_string()));
        }
        let mut args: Vec<String> = vec!["exec".into(), name.to_string()];
        args.extend(command.iter().cloned());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        // A non-zero exit is a normal result (the command ran and failed), not
        // a transport error: report its code instead of raising.
        match run_docker(&refs, timeout) {
            Ok(out) => Ok(ExecResult {
                stdout: String::from_utf8_lossy(&out.stdout).to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                exit_code: out.status.code().unwrap_or(-1),
            }),
            Err(DockerError::Timeout(secs)) => Err(DockerError::Timeout(secs)),
            Err(e) => Err(e),
        }
    }

    // ── Image build ────────────────────────────────────────────────────────

    /// `docker build` a context directory. Returns the build log.
    pub fn build_image(req: &BuildImageRequest) -> Result<String, DockerError> {
        if req.tag.trim().is_empty() {
            return Err(DockerError::CommandFailed(
                "Image tag is required".to_string(),
            ));
        }
        let context = std::path::Path::new(&req.context);
        if !context.is_dir() {
            return Err(DockerError::CommandFailed(format!(
                "Build context is not a directory: {}",
                req.context
            )));
        }

        let mut args: Vec<String> = vec!["build".into(), "-t".into(), req.tag.trim().to_string()];
        if let Some(df) = req.dockerfile.as_deref().filter(|d| !d.trim().is_empty()) {
            args.push("-f".into());
            args.push(df.trim().to_string());
        }
        if req.no_cache {
            args.push("--no-cache".into());
        }
        if req.pull {
            args.push("--pull".into());
        }
        for a in &req.build_args {
            args.push("--build-arg".into());
            args.push(format!("{}={}", a.key, a.value));
        }
        args.push(req.context.clone());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let output = run_docker(&refs, LONG_TIMEOUT)?;
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            return Err(DockerError::CommandFailed(if log.trim().is_empty() {
                "docker build failed".to_string()
            } else {
                log
            }));
        }
        Ok(log)
    }

    // ── Registries ─────────────────────────────────────────────────────────

    /// `docker login` with the password on stdin (never on the command line).
    pub fn registry_login(
        registry: &str,
        username: &str,
        password: &str,
    ) -> Result<(), DockerError> {
        if username.trim().is_empty() {
            return Err(DockerError::CommandFailed(
                "Username is required".to_string(),
            ));
        }
        let registry = registry.trim();
        let mut cmd = Command::new("docker");
        cmd.arg("login");
        if !registry.is_empty() {
            cmd.arg(registry);
        }
        cmd.arg("--username").arg(username);
        cmd.arg("--password-stdin");

        let out = run_command_with_stdin(cmd, password, SHORT_TIMEOUT)?;
        if !out.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    /// `docker logout`.
    pub fn registry_logout(registry: &str) -> Result<(), DockerError> {
        let registry = registry.trim();
        let args: Vec<&str> = if registry.is_empty() {
            vec!["logout"]
        } else {
            vec!["logout", registry]
        };
        Self::expect_ok(run_docker(&args, SHORT_TIMEOUT)?)
    }

    /// Registries currently holding credentials, read from
    /// `~/.docker/config.json` (`auths` keys). Never returns the credentials
    /// themselves.
    pub fn registry_list() -> Vec<String> {
        let path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/root"))
            .join(".docker/config.json");
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        let mut out: Vec<String> = value
            .pointer("/auths")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    // ── Daemon configuration ───────────────────────────────────────────────

    /// Path of the daemon configuration file.
    pub fn daemon_config_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/etc/docker/daemon.json")
    }

    /// Current `daemon.json` (empty string when absent).
    pub fn read_daemon_config() -> Result<String, DockerError> {
        match std::fs::read_to_string(Self::daemon_config_path()) {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(DockerError::Io(e)),
        }
    }

    /// Validate `daemon.json` content: it must be a JSON object. This runs
    /// before writing, because a malformed file makes `dockerd` refuse to
    /// start — i.e. writing blindly can take Docker down.
    pub fn validate_daemon_config(raw: &str) -> Result<(), DockerError> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| DockerError::CommandFailed(format!("Invalid JSON: {}", e)))?;
        if !value.is_object() {
            return Err(DockerError::CommandFailed(
                "daemon.json must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }

    /// Write `daemon.json` (through `sudo tee`, the service runs as root) and
    /// restart the daemon so it takes effect. Returns the restart output.
    pub fn write_daemon_config(raw: &str) -> Result<String, DockerError> {
        Self::validate_daemon_config(raw)?;

        let mut cmd = Command::new("sudo");
        cmd.args(["tee", "/etc/docker/daemon.json"]);
        let out = run_command_with_stdin(cmd, raw, SHORT_TIMEOUT)?;
        if !out.status.success() {
            return Err(DockerError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }

        let restart = run_command(
            {
                let mut c = Command::new("sudo");
                c.args(["systemctl", "restart", "docker"]);
                c
            },
            LONG_TIMEOUT,
        )?;
        if !restart.status.success() {
            return Err(DockerError::CommandFailed(format!(
                "Configuration saved, but restarting Docker failed: {}",
                String::from_utf8_lossy(&restart.stderr).trim()
            )));
        }
        Ok("Configuration saved and Docker restarted".to_string())
    }
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

    // ── Prune targets ──────────────────────────────────────────────────────

    #[test]
    fn test_prune_target_roundtrip_and_rejects_unknown() {
        for t in [
            PruneTarget::Images,
            PruneTarget::Volumes,
            PruneTarget::Containers,
            PruneTarget::Networks,
            PruneTarget::Builders,
            PruneTarget::System,
        ] {
            assert_eq!(PruneTarget::parse(t.as_str()), Some(t));
        }
        // An unknown target must never fall back to a destructive default.
        assert_eq!(PruneTarget::parse("everything"), None);
        assert_eq!(PruneTarget::parse(""), None);
    }

    #[test]
    fn test_parse_reclaimed() {
        let out = "Deleted Volumes:\nmyvol\n\nTotal reclaimed space: 1.2GB\n";
        assert_eq!(
            DockerService::parse_reclaimed(out).as_deref(),
            Some("1.2GB")
        );
        assert_eq!(DockerService::parse_reclaimed("nothing deleted"), None);
        // `docker builder prune` reports a table ending in `Total:\t<size>`
        // instead of the "Total reclaimed space:" sentence.
        let builders = "ID\t\t\t\tRECLAIMABLE\tSIZE\t\tLAST ACCESSED\n\
                        h15j3fd8a6ewc92iv2m57exc7*\ttrue\t8.192kB\t8 days ago\n\
                        Total:\t12.29kB";
        assert_eq!(
            DockerService::parse_reclaimed(builders).as_deref(),
            Some("12.29kB")
        );
        // A header-only table must not yield a bogus size.
        assert_eq!(DockerService::parse_reclaimed("Total:"), None);
    }

    // ── `docker system df` ─────────────────────────────────────────────────

    #[test]
    fn test_parse_system_df_real_output() {
        // Verbatim shape of `docker system df` (Docker 24+).
        let out = "\
TYPE            TOTAL     ACTIVE    SIZE      RECLAIMABLE
Images          5         2         1.234GB   800.5MB (64%)
Containers      3         1         12.3MB    0B (0%)
Local Volumes   2         1         40MB      20MB (50%)
Build Cache     0         0         0B        0B
";
        let df = DockerService::parse_system_df(out);
        assert_eq!(df.rows.len(), 4);

        let images = &df.rows[0];
        assert_eq!(images.kind, "Images");
        assert_eq!(images.total, 5);
        assert_eq!(images.active, 2);
        assert_eq!(images.size, "1.234GB");
        assert_eq!(images.reclaimable, "800.5MB (64%)");

        // The two-word kinds must not be mistaken for a generic row.
        assert_eq!(df.rows[2].kind, "Local Volumes");
        assert_eq!(df.rows[3].kind, "Build Cache");
        assert_eq!(df.total_size, "1.234GB");
        assert_eq!(df.total_reclaimable, "800.5MB (64%)");
    }

    #[test]
    fn test_parse_system_df_ignores_header_and_junk() {
        let df = DockerService::parse_system_df("TYPE TOTAL ACTIVE SIZE RECLAIMABLE\nnonsense\n");
        assert!(df.rows.is_empty());
        assert!(df.total_size.is_empty());
    }

    // ── Container inspect ──────────────────────────────────────────────────

    /// Trimmed but structurally faithful `docker inspect` output.
    const INSPECT_FIXTURE: &str = r#"[{
      "Id": "abc123",
      "Created": "2026-10-01T10:00:00Z",
      "Path": "/init",
      "Name": "/jellyfin",
      "Image": "sha256:deadbeef",
      "LogPath": "/var/lib/docker/containers/abc/abc-json.log",
      "RestartCount": 2,
      "State": {"Status": "running", "ExitCode": 0, "Pid": 4242,
                "StartedAt": "2026-10-01T10:00:05Z", "FinishedAt": "0001-01-01T00:00:00Z",
                "Health": {"Status": "healthy"}},
      "Config": {
        "Hostname": "jellyfin",
        "User": "1000",
        "Image": "jellyfin/jellyfin:latest",
        "Env": ["PUID=1000", "TZ=Europe/Paris"],
        "Cmd": ["/jellyfin/jellyfin"],
        "Entrypoint": ["/init"],
        "WorkingDir": "/config",
        "Labels": {
          "com.docker.compose.project": "jellyfin",
          "com.docker.compose.service": "jellyfin",
          "org.example.keep": "yes"
        }
      },
      "HostConfig": {
        "Privileged": false,
        "Memory": 2147483648,
        "NanoCpus": 2000000000,
        "RestartPolicy": {"Name": "unless-stopped"},
        "LogConfig": {"Type": "json-file"},
        "PortBindings": {"8096/tcp": [{"HostIp": "0.0.0.0", "HostPort": "8096"}]}
      },
      "Mounts": [
        {"Type": "bind", "Source": "/srv/jf/config", "Destination": "/config", "RW": true},
        {"Type": "volume", "Name": "jf_cache", "Source": "/var/lib/docker/volumes/jf_cache/_data",
         "Destination": "/cache", "RW": false}
      ],
      "NetworkSettings": {"Networks": {"bridge": {"IPAddress": "172.17.0.2"}}}
    }]"#;

    #[test]
    fn test_parse_container_detail_fields() {
        let d = DockerService::parse_container_detail(INSPECT_FIXTURE).expect("parse");

        assert_eq!(d.id, "abc123");
        assert_eq!(d.name, "jellyfin", "leading slash must be stripped");
        assert_eq!(d.image, "jellyfin/jellyfin:latest");
        assert_eq!(d.state, "running");
        assert_eq!(d.health.as_deref(), Some("healthy"));
        assert_eq!(d.restart_count, 2);
        assert_eq!(d.pid, Some(4242));
        assert_eq!(d.user, "1000");
        assert_eq!(d.working_dir, "/config");
        assert_eq!(d.restart_policy, "unless-stopped");
        assert_eq!(d.memory_limit, Some(2_147_483_648));
        assert_eq!(d.nano_cpus, Some(2_000_000_000));
        assert!(!d.privileged);
        assert_eq!(d.env, vec!["PUID=1000", "TZ=Europe/Paris"]);
        assert_eq!(d.compose_project.as_deref(), Some("jellyfin"));
        assert_eq!(d.compose_service.as_deref(), Some("jellyfin"));
        assert_eq!(d.networks, vec!["bridge"]);
        assert_eq!(d.ip_address.as_deref(), Some("172.17.0.2"));
        assert_eq!(d.ports.len(), 1);
        assert_eq!(d.ports[0].host_port, 8096);
        assert_eq!(d.ports[0].container_port, 8096);

        assert_eq!(d.mounts.len(), 2);
        assert_eq!(d.mounts[0].kind, "bind");
        assert!(!d.mounts[0].read_only);
        assert_eq!(d.mounts[1].name.as_deref(), Some("jf_cache"));
        assert!(d.mounts[1].read_only, "RW:false must mean read-only");

        // Labels are sorted by key.
        assert_eq!(d.labels[0].0, "com.docker.compose.project");
        assert!(d.labels.iter().any(|(k, _)| k == "org.example.keep"));
    }

    #[test]
    fn test_parse_container_detail_rejects_garbage() {
        assert!(DockerService::parse_container_detail("not json").is_err());
        assert!(DockerService::parse_container_detail("[]").is_err());
        assert!(DockerService::parse_container_detail("null").is_err());
    }

    #[test]
    fn test_run_args_from_inspect_replays_settings() {
        let args = DockerService::run_args_from_inspect(INSPECT_FIXTURE).expect("args");

        assert_eq!(args[0], "run");
        assert!(args.contains(&"-d".to_string()));

        let joined = args.join(" ");
        assert!(joined.contains("--name jellyfin"));
        assert!(joined.contains("-p 8096:8096/tcp"));
        assert!(joined.contains("-v /srv/jf/config:/config"));
        // A named volume is replayed by name, not by its host path.
        assert!(joined.contains("-v jf_cache:/cache:ro"));
        assert!(joined.contains("-e PUID=1000"));
        assert!(joined.contains("--restart unless-stopped"));
        assert!(joined.contains("-w /config"));
        assert!(joined.contains("-h jellyfin"));
        assert!(joined.contains("-u 1000"));
        assert!(joined.contains("-m 2147483648"));
        assert!(joined.contains("--cpus 2.00"));
        assert!(joined.contains("--entrypoint /init"));

        // Compose labels are dropped (the container is no longer compose's),
        // other user labels survive.
        assert!(!joined.contains("com.docker.compose.project"));
        assert!(joined.contains("--label org.example.keep=yes"));

        // The image precedes the command.
        let img = args
            .iter()
            .position(|a| a == "jellyfin/jellyfin:latest")
            .unwrap();
        let cmd = args.iter().position(|a| a == "/jellyfin/jellyfin").unwrap();
        assert!(img < cmd, "IMAGE must come before COMMAND: {:?}", args);
        assert_eq!(cmd, args.len() - 1);
    }

    #[test]
    fn test_run_args_from_inspect_skips_default_bridge_network() {
        let args = DockerService::run_args_from_inspect(INSPECT_FIXTURE).unwrap();
        assert!(
            !args.iter().any(|a| a == "--network"),
            "the default bridge network must not be passed explicitly"
        );
    }
}
