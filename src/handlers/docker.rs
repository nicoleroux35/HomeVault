/* =============================================================================
 * File          : docker.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Docker management endpoints: daemon status and control, container
 *   lifecycle (list, start/stop/restart, create, remove, logs), image
 *   pull/remove, network inspection, port checks and availability. Every
 *   endpoint is admin-only; the Docker work is delegated to
 *   crate::docker::DockerService.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/docker in src/web/mod.rs; the desktop Docker app and the
 *   Apps Center install flow both consume these endpoints.
 * =============================================================================
 */

//! Docker daemon and container management endpoints (admin-only).

use crate::docker::{
    self as docker_lib, CreateContainerRequest, DockerError, DockerService, PullImageRequest,
};
use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

/// Run a blocking Docker operation on the blocking pool.
///
/// Every `docker` CLI call blocks on a child process. Running one directly
/// inside an async handler parks a Tokio worker for the whole duration of a
/// `pull` / `build` / `compose up` — long enough to stall every other request
/// on that worker, the dashboard included. Reads already went through
/// `spawn_blocking`; mutations used not to, which is exactly where the slow
/// ones live.
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, DockerError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("Task join error: {}", e)),
    }
}

/// Returns Docker daemon information (version, containers, images counts).
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Vérification admin ajoutée
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match tokio::task::spawn_blocking(DockerService::get_docker_info).await {
        Ok(Ok(info)) => ResponseHelper::json_success(info),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to get Docker info: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Lists all Docker containers (running and stopped).
/// REQUIRES: Admin privileges
#[derive(Deserialize)]
pub struct ContainersQuery {
    /// When `false`, skip the slow `docker stats --no-stream` sampling so the
    /// list renders instantly; the frontend fills CPU/memory in afterwards.
    #[serde(default = "default_true")]
    pub stats: bool,
}

fn default_true() -> bool {
    true
}

#[axum_macros::debug_handler]
pub async fn handle_docker_containers_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ContainersQuery>,
) -> Response {
    // ✅ SECURITY: Vérification admin ajoutée
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let include_stats = q.stats;
    match tokio::task::spawn_blocking(move || DockerService::get_containers(include_stats)).await {
        Ok(Ok(containers)) => ResponseHelper::json_success(containers),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to list containers: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Lists only running Docker containers.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_containers_running(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Vérification admin ajoutée
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match blocking(DockerService::get_running_containers).await {
        Ok(containers) => ResponseHelper::json_success(containers),
        Err(e) => ResponseHelper::json_error(format!("Failed to list running containers: {}", e)),
    }
}

/// Lists all Docker images available locally.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_images_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Vérification admin ajoutée
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match tokio::task::spawn_blocking(DockerService::get_images).await {
        Ok(Ok(images)) => ResponseHelper::json_success(images),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to list images: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Starts a stopped Docker container by name.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_start(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Vérification admin ajoutée
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let target = name.clone();
    match blocking(move || DockerService::start_container(&target)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' started Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container started".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to start container: {}", e)),
    }
}

/// Stops a running Docker container by name.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_stop(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Vérification admin ajoutée
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let target = name.clone();
    match blocking(move || DockerService::stop_container(&target)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' stopped Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container stopped".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to stop container: {}", e)),
    }
}

/// Restarts a Docker container by name.
#[axum_macros::debug_handler]
pub async fn handle_docker_container_restart(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let target = name.clone();
    match blocking(move || DockerService::restart_container(&target)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' restarted Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container restarted".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to restart container: {}", e)),
    }
}

/// Query for the container log read. `tail` is bounded so a client can't ask
/// for the whole log file of a container that has been running for months.
#[derive(Deserialize)]
pub struct LogsQuery {
    #[serde(default = "default_log_tail")]
    pub tail: u32,
    #[serde(default)]
    pub timestamps: bool,
}

fn default_log_tail() -> u32 {
    200
}

/// Returns the last `tail` (default 200, max 5000) log lines from a container,
/// stdout and stderr merged.
#[axum_macros::debug_handler]
pub async fn handle_docker_container_logs(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    Query(q): Query<LogsQuery>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let tail = q.tail.clamp(1, 5000);
    let timestamps = q.timestamps;
    match blocking(move || DockerService::get_container_logs_full(&name, tail, timestamps, false))
        .await
    {
        Ok(logs) => ResponseHelper::json_success(serde_json::json!({ "logs": logs, "tail": tail })),
        Err(e) => ResponseHelper::json_error(format!("Failed to get container logs: {}", e)),
    }
}

/// Checks whether the Docker daemon is reachable.
/// REQUIRES: a valid session (matches `handle_catalog_list`, the other
/// low-sensitivity Docker read endpoint)
#[axum_macros::debug_handler]
pub async fn handle_docker_available(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return response;
    }

    let available = blocking(|| Ok::<_, DockerError>(DockerService::is_available()))
        .await
        .unwrap_or(false);
    ResponseHelper::json_success(serde_json::json!({ "available": available }))
}

/// Starts the Docker daemon via systemd (enables it for boot persistence too).
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_daemon_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match tokio::task::spawn_blocking(DockerService::start_daemon).await {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "Admin '{}' started the Docker daemon",
                admin_username
            ));
            ResponseHelper::json_success("Docker daemon started".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to start Docker daemon: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Returns the last 100 lines of the Docker daemon's journal, for diagnosing
/// why the daemon is down.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_daemon_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    match tokio::task::spawn_blocking(|| DockerService::get_daemon_logs(100)).await {
        Ok(Ok(logs)) => ResponseHelper::json_success(serde_json::json!({ "logs": logs })),
        Ok(Err(e)) => {
            ResponseHelper::json_error(format!("Failed to read Docker daemon logs: {}", e))
        }
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct PortCheckQuery {
    pub port: u16,
}

/// Reports whether a host port is free to publish a container on — checked
/// against the host's bound ports (`ss`), other Docker containers, and
/// HomeVault's own web port. Used by the create-container and catalog-install
/// forms to warn before submit; the same check is re-run server-side on
/// create/install so a direct API call can't bypass it.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_port_check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PortCheckQuery>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let port = q.port;
    match blocking(move || Ok::<_, DockerError>(DockerService::check_port(port))).await {
        Ok(status) => ResponseHelper::json_success(status),
        Err(e) => ResponseHelper::json_error(format!("Port check failed: {}", e)),
    }
}

/// Human-readable reason a port is taken, for a server-side rejection message.
fn port_conflict_message(port: u16, status: &crate::docker::PortStatus) -> String {
    match status.used_by.as_deref() {
        Some("container") => format!(
            "Port {} is already used by container '{}'",
            port,
            status.detail.as_deref().unwrap_or("?")
        ),
        Some("homevault") => format!("Port {} is reserved by the HomeVault server", port),
        _ => format!("Port {} is already in use on the host", port),
    }
}

#[derive(serde::Deserialize)]
pub struct CreateContainerRequestRaw {
    pub name: String,
    pub image: String,
    pub ports: Option<Vec<serde_json::Value>>,
    pub volumes: Option<Vec<serde_json::Value>>,
    pub env: Option<Vec<serde_json::Value>>,
    pub network: Option<String>,
    pub restart_policy: Option<String>,
    pub command: Option<String>,
    pub working_dir: Option<String>,
    pub memory_limit: Option<String>,
    pub cpu_limit: Option<String>,
    pub privileged: Option<bool>,
    pub auto_remove: Option<bool>,
    pub dns_servers: Option<Vec<String>>,
    pub hostname: Option<String>,
}

impl From<CreateContainerRequestRaw> for CreateContainerRequest {
    fn from(raw: CreateContainerRequestRaw) -> Self {
        let ports = raw
            .ports
            .map(|p| {
                p.into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect()
            })
            .unwrap_or_default();

        let volumes = raw
            .volumes
            .map(|v| {
                v.into_iter()
                    .filter_map(|vol| serde_json::from_value(vol).ok())
                    .collect()
            })
            .unwrap_or_default();

        let env = raw
            .env
            .map(|e| {
                e.into_iter()
                    .filter_map(|env| serde_json::from_value(env).ok())
                    .collect()
            })
            .unwrap_or_default();

        CreateContainerRequest {
            name: raw.name,
            image: raw.image,
            ports,
            volumes,
            env,
            network: raw.network,
            restart_policy: raw.restart_policy,
            command: raw.command,
            working_dir: raw.working_dir,
            memory_limit: raw.memory_limit,
            cpu_limit: raw.cpu_limit,
            privileged: raw.privileged.unwrap_or(false),
            auto_remove: raw.auto_remove.unwrap_or(false),
            dns_servers: raw.dns_servers.unwrap_or_default(),
            hostname: raw.hostname,
        }
    }
}

/// Creates a new Docker container from the specified image with ports, volumes, env, etc.
#[axum_macros::debug_handler]
pub async fn handle_docker_container_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateContainerRequestRaw>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    // Validate required fields
    if req.name.trim().is_empty() {
        return ResponseHelper::json_error("Container name is required".to_string());
    }

    if req.image.trim().is_empty() {
        return ResponseHelper::json_error("Image is required".to_string());
    }

    // Validate name: letters, numbers, hyphens, underscores
    let name_chars: Vec<char> = req.name.chars().collect();
    if name_chars.is_empty() || !name_chars[0].is_ascii_alphanumeric() {
        return ResponseHelper::json_error(
            "Invalid container name. Must start with a letter or number.".to_string(),
        );
    }
    if !req
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return ResponseHelper::json_error(
            "Invalid container name. Use letters, numbers, hyphens, and underscores only."
                .to_string(),
        );
    }

    let create_req: CreateContainerRequest = req.into();

    // Port-conflict guard + creation run on the blocking pool: the guard
    // shells out to `ss`/`docker ps` and the creation can pull nothing but
    // still waits on the daemon.
    let name_for_log = create_req.name.clone();
    let image_for_log = create_req.image.clone();
    let result = blocking(move || {
        let mut seen_host_ports = std::collections::HashSet::new();
        for p in &create_req.ports {
            if !seen_host_ports.insert(p.host_port) {
                return Err(DockerError::CommandFailed(format!(
                    "Host port {} is listed more than once",
                    p.host_port
                )));
            }
            let status = DockerService::check_port(p.host_port);
            if !status.available {
                return Err(DockerError::CommandFailed(port_conflict_message(
                    p.host_port,
                    &status,
                )));
            }
        }
        DockerService::create_container(&create_req)
    })
    .await;

    match result {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created Docker container '{}' (image: '{}')",
                admin_username, name_for_log, image_for_log
            ));
            ResponseHelper::json_success("Container created successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create container: {}", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct RemoveContainerRequest {
    pub force: Option<bool>,
}

/// Removes a Docker container (optionally force).
#[axum_macros::debug_handler]
pub async fn handle_docker_container_remove(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    query: axum::extract::Query<RemoveContainerRequest>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let force = query.force.unwrap_or(false);

    let target = name.clone();
    match blocking(move || DockerService::remove_container(&target, force)).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' removed Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container removed successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove container: {}", e)),
    }
}

/// Pulls a Docker image from a registry (defaults to Docker Hub).
#[axum_macros::debug_handler]
pub async fn handle_docker_image_pull(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<PullImageRequest>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let (image, tag) = (req.image.clone(), req.tag.clone());
    match blocking(move || DockerService::pull_image(&image, &tag)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' pulled Docker image '{}:{}'",
                admin_username, req.image, req.tag
            ));
            ResponseHelper::json_success("Image pulled successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to pull image: {}", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct RemoveImageRequest {
    pub force: Option<bool>,
}

/// Removes a Docker image by ID or tag (optionally force).
#[axum_macros::debug_handler]
pub async fn handle_docker_image_remove(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(image_id): axum::extract::Path<String>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<RemoveImageRequest>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let force = req.force.unwrap_or(false);

    let target = image_id.clone();
    match blocking(move || DockerService::remove_image(&target, force)).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' removed Docker image '{}'",
                admin_username, image_id
            ));
            ResponseHelper::json_success("Image removed successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove image: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Network handlers
// ---------------------------------------------------------------------------

/// Lists all Docker networks.
#[axum_macros::debug_handler]
pub async fn handle_docker_networks_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match tokio::task::spawn_blocking(docker_lib::get_networks).await {
        Ok(Ok(networks)) => ResponseHelper::json_success(networks),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to list networks: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Gets a single Docker network with details.
#[axum_macros::debug_handler]
pub async fn handle_docker_network_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match blocking(move || docker_lib::get_network(&name)).await {
        Ok(network) => ResponseHelper::json_success(network),
        Err(e) => ResponseHelper::json_error(format!("Failed to get network: {}", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct CreateNetworkRequest {
    pub name: String,
    pub driver: Option<String>,
    pub subnet: Option<String>,
    pub gateway: Option<String>,
    pub internal: Option<bool>,
}

/// Creates a new Docker network.
#[axum_macros::debug_handler]
pub async fn handle_docker_network_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateNetworkRequest>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    if req.name.trim().is_empty() {
        return ResponseHelper::json_error("Network name is required".to_string());
    }

    let driver = req.driver.as_deref().unwrap_or("bridge");
    let subnet = req.subnet.as_deref();
    let gateway = req.gateway.as_deref();
    let internal = req.internal.unwrap_or(false);

    let (net_name, net_driver) = (req.name.clone(), driver.to_string());
    let (net_subnet, net_gateway) = (
        subnet.map(|s| s.to_string()),
        gateway.map(|s| s.to_string()),
    );
    match blocking(move || {
        docker_lib::create_network(
            &net_name,
            &net_driver,
            net_subnet.as_deref(),
            net_gateway.as_deref(),
            internal,
        )
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created Docker network '{}' (driver: {})",
                admin_username, req.name, driver
            ));
            ResponseHelper::json_success("Network created successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create network: {}", e)),
    }
}

#[derive(serde::Deserialize)]
pub struct NetworkActionRequest {
    pub network: String,
    pub container: String,
}

/// Connects a container to a network.
#[axum_macros::debug_handler]
pub async fn handle_docker_network_connect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<NetworkActionRequest>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let (container, network) = (req.container.clone(), req.network.clone());
    match blocking(move || docker_lib::connect_container_to_network(&container, &network)).await {
        Ok(_) => ResponseHelper::json_success("Container connected to network".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to connect: {}", e)),
    }
}

/// Disconnects a container from a network.
#[axum_macros::debug_handler]
pub async fn handle_docker_network_disconnect(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<NetworkActionRequest>,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let (container, network) = (req.container.clone(), req.network.clone());
    match blocking(move || docker_lib::disconnect_container_from_network(&container, &network))
        .await
    {
        Ok(_) => ResponseHelper::json_success("Container disconnected from network".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to disconnect: {}", e)),
    }
}

/// Removes a Docker network.
#[axum_macros::debug_handler]
pub async fn handle_docker_network_remove(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let target = name.clone();
    match blocking(move || docker_lib::remove_network(&target)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' removed Docker network '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Network removed successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove network: {}", e)),
    }
}

/// Searches Docker Hub for images matching a query.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_hub_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let query = match params.get("q") {
        Some(q) if !q.trim().is_empty() => q.trim().to_string(),
        _ => return ResponseHelper::json_error("Query parameter 'q' is required".to_string()),
    };

    let client = reqwest::Client::new();

    let lib_url = match url::Url::parse_with_params(
        "https://hub.docker.com/v2/repositories/library",
        &[("page_size", "12"), ("name", &query)],
    ) {
        Ok(u) => u.to_string(),
        Err(_) => return ResponseHelper::json_error("Invalid search URL".to_string()),
    };

    let all_url = match url::Url::parse_with_params(
        "https://hub.docker.com/v2/repositories/v2",
        &[("page_size", "8"), ("name", &query)],
    ) {
        Ok(u) => u.to_string(),
        Err(_) => return ResponseHelper::json_error("Invalid search URL".to_string()),
    };

    let lib_resp = client
        .get(&lib_url)
        .header("User-Agent", "HomeVault/1.0")
        .send()
        .await;
    let all_resp = client
        .get(&all_url)
        .header("User-Agent", "HomeVault/1.0")
        .send()
        .await;

    let mut combined: Vec<serde_json::Value> = Vec::new();

    if let Ok(resp) = lib_resp {
        if let Ok(body) = resp.json::<serde_json::Value>().await {
            if let Some(results) = body["results"].as_array() {
                for r in results {
                    combined.push(serde_json::json!({
                        "name": r["name"].as_str().unwrap_or(""),
                        "description": r["description"].as_str().unwrap_or(""),
                        "star_count": r["star_count"].as_u64().unwrap_or(0),
                        "pull_count": r["pull_count"].as_u64().unwrap_or(0),
                        "is_official": true
                    }));
                }
            }
        }
    }

    if let Ok(resp) = all_resp {
        if let Ok(body) = resp.json::<serde_json::Value>().await {
            if let Some(results) = body["results"].as_array() {
                for r in results {
                    let repo_name = r["repo_name"]
                        .as_str()
                        .or_else(|| r["name"].as_str())
                        .unwrap_or("");
                    if !combined
                        .iter()
                        .any(|c| c["name"].as_str() == Some(repo_name))
                    {
                        combined.push(serde_json::json!({
                            "name": repo_name,
                            "description": r["description"].as_str().unwrap_or(""),
                            "star_count": r["star_count"].as_u64().unwrap_or(0),
                            "pull_count": r["pull_count"].as_u64().unwrap_or(0),
                            "is_official": false
                        }));
                    }
                }
            }
        }
    }

    ResponseHelper::json_success(combined)
}

/// Deletes an unused Docker network (alias for remove).
#[axum_macros::debug_handler]
pub async fn handle_docker_network_prune(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match blocking(|| DockerService::prune(docker_lib::PruneTarget::Networks, false)).await {
        Ok(report) => ResponseHelper::json_success(serde_json::json!({
            "message": report.message,
            "reclaimed": report.reclaimed,
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to prune networks: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Container detail / lifecycle extras
// ---------------------------------------------------------------------------

/// Returns a parsed `docker inspect` for one container: env, labels, mounts,
/// published ports, restart policy, health, limits. Backs the detail panel.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_inspect(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match blocking(move || DockerService::get_container_detail(&name)).await {
        Ok(detail) => ResponseHelper::json_success(detail),
        Err(e) => ResponseHelper::json_error(format!("Failed to inspect container: {}", e)),
    }
}

/// Freezes a running container.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_pause(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let target = name.clone();
    match blocking(move || DockerService::pause_container(&target)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' paused Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container paused".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to pause container: {}", e)),
    }
}

/// Resumes a paused container.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_unpause(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let target = name.clone();
    match blocking(move || DockerService::unpause_container(&target)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' resumed Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container resumed".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to resume container: {}", e)),
    }
}

/// Body for the kill action.
#[derive(Deserialize)]
pub struct KillContainerRequest {
    /// Signal name or number (default `SIGKILL`). Restricted to a known set:
    /// an arbitrary string would be passed straight to `docker kill --signal`.
    pub signal: Option<String>,
}

/// Known signals accepted by `docker kill --signal`. Anything else is
/// rejected instead of forwarded.
const ALLOWED_SIGNALS: [&str; 8] = [
    "SIGKILL", "SIGTERM", "SIGINT", "SIGHUP", "SIGQUIT", "SIGUSR1", "SIGUSR2", "SIGSTOP",
];

/// Sends a signal to a container (SIGKILL by default).
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_kill(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    body: Option<Json<KillContainerRequest>>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let signal = body.and_then(|Json(b)| b.signal).map(|s| {
        let up = s.trim().to_ascii_uppercase();
        if up.is_empty() {
            return "SIGKILL".to_string();
        }
        if ALLOWED_SIGNALS.contains(&up.as_str()) {
            up
        } else {
            // Fall back to the default rather than forwarding an unknown
            // token to the CLI.
            "SIGKILL".to_string()
        }
    });
    let signal_log = signal.clone().unwrap_or_else(|| "SIGKILL".to_string());

    let target = name.clone();
    match blocking(move || DockerService::kill_container(&target, signal.as_deref())).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' sent {} to Docker container '{}'",
                admin_username, signal_log, name
            ));
            ResponseHelper::json_success("Signal sent".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to kill container: {}", e)),
    }
}

/// Body for renaming a container.
#[derive(Deserialize)]
pub struct RenameContainerRequest {
    pub name: String,
}

/// Renames a container.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_rename(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(req): Json<RenameContainerRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // Same shape as container creation: alphanumeric first, then
    // letters/digits/underscore/hyphen.
    let new_name = req.name.trim().to_string();
    let valid = !new_name.is_empty()
        && new_name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && new_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return ResponseHelper::json_error(
            "Invalid container name. Use letters, numbers, hyphens and underscores.".to_string(),
        );
    }

    let old = name.clone();
    match blocking(move || DockerService::rename_container(&old, &new_name)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' renamed Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container renamed".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to rename container: {}", e)),
    }
}

/// Body for in-place resource-limit changes.
#[derive(Deserialize)]
pub struct UpdateContainerRequest {
    pub memory_limit: Option<String>,
    pub cpu_limit: Option<String>,
    pub restart_policy: Option<String>,
}

/// Changes a container's memory/CPU limits and restart policy without
/// recreating it (`docker update`).
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_update(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(req): Json<UpdateContainerRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let memory = req.memory_limit;
    let cpus = req.cpu_limit;
    let restart = req.restart_policy;
    let target = name.clone();
    match blocking(move || {
        DockerService::update_container(
            &target,
            memory.as_deref(),
            cpus.as_deref(),
            restart.as_deref(),
        )
    })
    .await
    {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' updated the limits of Docker container '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Container updated".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update container: {}", e)),
    }
}

/// Pulls the container's image and recreates it with the same settings —
/// the "update to the newest image" action for containers that are not part
/// of a Compose project (those go through the Compose update path instead).
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_recreate(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let target = name.clone();
    match blocking(move || DockerService::recreate_container(&target)).await {
        Ok(id) => {
            crate::events::log_event(format!(
                "Admin '{}' recreated Docker container '{}' on its newest image",
                admin_username, name
            ));
            ResponseHelper::json_success(serde_json::json!({ "id": id }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to update container: {}", e)),
    }
}

/// Body for a non-interactive exec.
#[derive(Deserialize)]
pub struct ExecContainerRequest {
    /// argv of the command to run inside the container.
    pub command: Vec<String>,
    /// Seconds before the command is killed (default 30, max 600).
    pub timeout_secs: Option<u64>,
}

/// Runs a command inside a running container and returns its output. This is
/// the "exec" a home admin needs to debug an app without SSH.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_exec(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(req): Json<ExecContainerRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let command: Vec<String> = req
        .command
        .into_iter()
        .filter(|c| !c.trim().is_empty())
        .collect();
    if command.is_empty() {
        return ResponseHelper::json_error("No command given".to_string());
    }
    let timeout = std::time::Duration::from_secs(req.timeout_secs.unwrap_or(30).clamp(1, 600));
    let log_cmd = command.join(" ");

    let target = name.clone();
    match blocking(move || DockerService::exec_in_container(&target, &command, timeout)).await {
        Ok(result) => {
            crate::events::log_event(format!(
                "Admin '{}' ran '{}' in Docker container '{}' (exit {})",
                admin_username, log_cmd, name, result.exit_code
            ));
            ResponseHelper::json_success(result)
        }
        Err(e) => ResponseHelper::json_error(format!("Exec failed: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Live logs (SSE)
// ---------------------------------------------------------------------------

/// Body for minting a log-stream ticket.
#[derive(Deserialize)]
pub struct LogTicketRequest {
    pub tail: Option<u32>,
    pub follow: Option<bool>,
    pub timestamps: Option<bool>,
}

/// Mints a single-use ticket for the live log stream. `EventSource` can't set
/// the `X-Session-Token` header, so the frontend exchanges its session here
/// first; the ticket (never the session) then appears in the stream URL.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_container_logs_ticket(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    Json(req): Json<LogTicketRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let tail = req.tail.unwrap_or(200).clamp(1, 5000);
    let ticket = crate::docker::issue_log_ticket(
        &name,
        tail,
        req.follow.unwrap_or(true),
        req.timestamps.unwrap_or(false),
    );
    ResponseHelper::json_success(serde_json::json!({
        "ticket": ticket,
        "url": format!("/api/docker/containers/{}/logs/stream?ticket={}", name, ticket),
    }))
}

/// Query for the log stream endpoint.
#[derive(Deserialize)]
pub struct LogStreamQuery {
    pub ticket: Option<String>,
}

/// Streams `docker logs` as Server-Sent Events.
///
/// Authenticated by a single-use ticket (see
/// [`handle_docker_container_logs_ticket`]). The child process is killed when
/// the client disconnects, so closing the log panel never leaves a `docker
/// logs -f` running.
pub async fn handle_docker_container_logs_stream(
    axum::extract::State(_state): axum::extract::State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Query(q): Query<LogStreamQuery>,
) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use std::convert::Infallible;

    let Some(ticket) = q.ticket.filter(|t| !t.is_empty()) else {
        return ResponseHelper::json_unauthorized("No log ticket provided".to_string());
    };
    let Some(params) = crate::docker::consume_log_ticket(&ticket) else {
        return ResponseHelper::json_unauthorized("Invalid or expired log ticket".to_string());
    };
    // The ticket carries the container: a ticket minted for container A must
    // not open the stream of container B.
    if params.container != name {
        return ResponseHelper::json_unauthorized("Ticket does not match container".to_string());
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(512);
    let mut cmd = DockerService::logs_command(&name, params.tail, params.follow, params.timestamps);

    let child_slot: Arc<std::sync::Mutex<Option<std::process::Child>>> =
        Arc::new(std::sync::Mutex::new(None));

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to start docker logs: {}", e));
        }
    };

    // Take the pipes before the child goes into the shared slot.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *child_slot.lock().unwrap() = Some(child);

    // One reader thread per stream: reading both from a single thread would
    // block on whichever the container writes to less often. `stdout` and
    // `stderr` have distinct types, hence the two spawns.
    fn pump<R: std::io::Read + Send + 'static>(pipe: R, tx: tokio::sync::mpsc::Sender<String>) {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(pipe).lines() {
                let Ok(line) = line else { break };
                if tx.blocking_send(line).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(out) = stdout {
        pump(out, tx.clone());
    }
    if let Some(err) = stderr {
        pump(err, tx.clone());
    }
    drop(tx);

    /// Kills the `docker logs` child when the stream is dropped (client
    /// disconnected, tab closed, or the handler finished).
    struct KillOnDrop(Arc<std::sync::Mutex<Option<std::process::Child>>>);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if let Ok(mut slot) = self.0.lock() {
                if let Some(mut child) = slot.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
    let guard = KillOnDrop(child_slot);

    let stream = async_stream::stream! {
        // Keep the guard alive for the whole stream lifetime.
        let _guard = guard;
        let mut rx = rx;
        while let Some(line) = rx.recv().await {
            yield Ok::<_, Infallible>(Event::default().event("log").data(line));
        }
        yield Ok(Event::default().event("end").data("stream closed"));
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ---------------------------------------------------------------------------
// Volumes
// ---------------------------------------------------------------------------

/// Lists Docker named volumes with the containers mounting them.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_volumes_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match blocking(DockerService::list_volumes).await {
        Ok(volumes) => ResponseHelper::json_success(volumes),
        Err(e) => ResponseHelper::json_error(format!("Failed to list volumes: {}", e)),
    }
}

/// Body for creating a volume.
#[derive(Deserialize)]
pub struct CreateVolumeRequest {
    pub name: String,
    pub driver: Option<String>,
    pub labels: Option<Vec<CreateVolumeLabel>>,
}

/// One `key=value` label on a volume.
#[derive(Deserialize)]
pub struct CreateVolumeLabel {
    pub key: String,
    pub value: String,
}

/// Creates a named volume.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_volume_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateVolumeRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let name = req.name.trim().to_string();
    let valid = !name.is_empty()
        && name.len() <= 255
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !valid {
        return ResponseHelper::json_error(
            "Invalid volume name. Use letters, numbers, hyphens, underscores and dots.".to_string(),
        );
    }

    let driver = req.driver;
    let labels: Vec<(String, String)> = req
        .labels
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.key, l.value))
        .collect();
    let name_for_log = name.clone();

    match blocking(move || DockerService::create_volume(&name, driver.as_deref(), &labels)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created Docker volume '{}'",
                admin_username, name_for_log
            ));
            ResponseHelper::json_success("Volume created".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create volume: {}", e)),
    }
}

/// Removes a named volume. `?force=true` allows removing one still in use —
/// destructive, and the UI confirms it explicitly.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_volume_remove(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    Query(q): Query<RemoveContainerRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let force = q.force.unwrap_or(false);
    let name_for_log = name.clone();
    match blocking(move || DockerService::remove_volume(&name, force)).await {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' removed Docker volume '{}'{}",
                admin_username,
                name_for_log,
                if force { " (forced)" } else { "" }
            ));
            ResponseHelper::json_success("Volume removed".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to remove volume: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Disk usage / prune
// ---------------------------------------------------------------------------

/// `docker system df`, parsed: per-kind totals and reclaimable space.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_system_df(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match blocking(DockerService::disk_usage).await {
        Ok(df) => ResponseHelper::json_success(df),
        Err(e) => ResponseHelper::json_error(format!("Failed to read disk usage: {}", e)),
    }
}

/// Body for a prune.
#[derive(Deserialize)]
pub struct PruneRequest {
    /// `images`, `volumes`, `containers`, `networks`, `builders` or `system`.
    pub target: String,
    /// Widen the prune: unused tagged images, and (for `system`) anonymous
    /// volumes.
    #[serde(default)]
    pub all: bool,
}

/// Reclaims disk space. Destructive: the UI confirms before calling it, and
/// the target is validated against a fixed list rather than forwarded.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_prune(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<PruneRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    // Validate the target first: it is cheap, and an unknown target must be
    // rejected even when the daemon is down (never default to a destructive
    // action).
    let Some(target) = crate::docker::PruneTarget::parse(req.target.trim()) else {
        return ResponseHelper::json_error(format!(
            "Unknown prune target '{}'. Use images, volumes, containers, networks, builders or system.",
            req.target
        ));
    };
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }
    let all = req.all;
    let target_log = target.as_str().to_string();

    match blocking(move || DockerService::prune(target, all)).await {
        Ok(report) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' pruned Docker {} ({})",
                admin_username, target_log, report.reclaimed
            ));
            ResponseHelper::json_success(report)
        }
        Err(e) => ResponseHelper::json_error(format!("Prune failed: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Image build
// ---------------------------------------------------------------------------

/// Body for building an image from a Dockerfile.
#[derive(Deserialize)]
pub struct BuildImageBody {
    pub context: String,
    pub dockerfile: Option<String>,
    pub tag: String,
    #[serde(default)]
    pub no_cache: bool,
    #[serde(default)]
    pub pull: bool,
    #[serde(default)]
    pub build_args: Vec<CreateVolumeLabel>,
}

/// Builds an image from a Dockerfile in a host directory.
///
/// The build context is validated against the configured path roots (the same
/// guard the file explorer uses), so an admin can't point the builder at
/// `/etc` or `/` through the API.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_image_build(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<BuildImageBody>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let Some(context) = state.path_guard.validate(req.context.trim()) else {
        return ResponseHelper::json_error(
            "Build context is outside the allowed directories".to_string(),
        );
    };
    if !context.is_dir() {
        return ResponseHelper::json_error("Build context is not a directory".to_string());
    }

    let build_req = crate::docker::BuildImageRequest {
        context: context.to_string_lossy().to_string(),
        dockerfile: req.dockerfile,
        tag: req.tag.clone(),
        no_cache: req.no_cache,
        pull: req.pull,
        build_args: req
            .build_args
            .into_iter()
            .map(|a| crate::docker::EnvVar {
                key: a.key,
                value: a.value,
            })
            .collect(),
    };
    let tag_log = build_req.tag.clone();

    match blocking(move || DockerService::build_image(&build_req)).await {
        Ok(log) => {
            crate::events::log_event(format!(
                "Admin '{}' built Docker image '{}'",
                admin_username, tag_log
            ));
            ResponseHelper::json_success(serde_json::json!({ "log": log }))
        }
        Err(e) => ResponseHelper::json_error(format!("Build failed: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Registries
// ---------------------------------------------------------------------------

/// Lists the registries HomeVault holds credentials for (names only — the
/// credentials never leave `~/.docker/config.json`).
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_registry_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let registries = blocking(|| Ok::<_, DockerError>(DockerService::registry_list()))
        .await
        .unwrap_or_default();
    ResponseHelper::json_success(registries)
}

/// Body for a registry login.
#[derive(Deserialize)]
pub struct RegistryLoginRequest {
    /// Registry host (`ghcr.io`, `registry.example.lan`). Empty means Docker Hub.
    #[serde(default)]
    pub registry: String,
    pub username: String,
    pub password: String,
}

/// Stores credentials for a private registry (`docker login`, password on
/// stdin). The password is never logged.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_registry_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RegistryLoginRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if req.password.is_empty() {
        return ResponseHelper::json_error("Password is required".to_string());
    }

    let registry = req.registry.trim().to_string();
    let username = req.username.trim().to_string();
    let password = req.password;
    let registry_log = if registry.is_empty() {
        "Docker Hub".to_string()
    } else {
        registry.clone()
    };

    match blocking(move || DockerService::registry_login(&registry, &username, &password)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' signed in to registry '{}'",
                admin_username, registry_log
            ));
            ResponseHelper::json_success("Signed in to the registry".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Registry login failed: {}", e)),
    }
}

/// Body for a registry logout.
#[derive(Deserialize)]
pub struct RegistryLogoutRequest {
    #[serde(default)]
    pub registry: String,
}

/// Forgets a registry's credentials.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_registry_logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RegistryLogoutRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let registry = req.registry.trim().to_string();
    let registry_log = if registry.is_empty() {
        "Docker Hub".to_string()
    } else {
        registry.clone()
    };

    match blocking(move || DockerService::registry_logout(&registry)).await {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' signed out of registry '{}'",
                admin_username, registry_log
            ));
            ResponseHelper::json_success("Signed out of the registry".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Registry logout failed: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Daemon configuration
// ---------------------------------------------------------------------------

/// Returns the current `/etc/docker/daemon.json`.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_daemon_config_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    match blocking(DockerService::read_daemon_config).await {
        Ok(config) => ResponseHelper::json_success(serde_json::json!({
            "config": config,
            "path": DockerService::daemon_config_path().to_string_lossy(),
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to read daemon.json: {}", e)),
    }
}

/// Body for writing the daemon configuration.
#[derive(Deserialize)]
pub struct DaemonConfigRequest {
    pub config: String,
}

/// Validates and writes `/etc/docker/daemon.json`, then restarts the daemon.
///
/// The JSON is validated *before* writing: a malformed file makes `dockerd`
/// refuse to start, so a blind write would take every container down.
/// REQUIRES: Admin privileges
#[axum_macros::debug_handler]
pub async fn handle_docker_daemon_config_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<DaemonConfigRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if let Err(e) = DockerService::validate_daemon_config(&req.config) {
        return ResponseHelper::json_error(format!("Invalid daemon.json: {}", e));
    }

    let config = req.config;
    match blocking(move || DockerService::write_daemon_config(&config)).await {
        Ok(message) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' updated /etc/docker/daemon.json and restarted Docker",
                admin_username
            ));
            ResponseHelper::json_success(message)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to write daemon.json: {}", e)),
    }
}
