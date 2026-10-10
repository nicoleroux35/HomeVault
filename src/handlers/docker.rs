use crate::docker::{self as docker_lib, CreateContainerRequest, DockerService, PullImageRequest};
use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

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

    match DockerService::get_running_containers() {
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

    match DockerService::start_container(&name) {
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

    match DockerService::stop_container(&name) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match DockerService::restart_container(&name) {
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

/// Returns the last 100 log lines from a Docker container.
#[axum_macros::debug_handler]
pub async fn handle_docker_container_logs(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    // ✅ SECURITY: Admin privileges required
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let name = name.clone();
    match tokio::task::spawn_blocking(move || DockerService::get_container_logs(&name, 100)).await {
        Ok(Ok(logs)) => ResponseHelper::json_success(serde_json::json!({ "logs": logs })),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to get container logs: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    ResponseHelper::json_success(serde_json::json!({
        "available": DockerService::is_available()
    }))
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

    ResponseHelper::json_success(DockerService::check_port(q.port))
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

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

    // Port-conflict guard: re-checked here so a direct API call can't skip the
    // form's live check. Rejects a port already bound on the host, held by
    // another container, reserved by HomeVault, or repeated in this request.
    let mut seen_host_ports = std::collections::HashSet::new();
    for p in &create_req.ports {
        if !seen_host_ports.insert(p.host_port) {
            return ResponseHelper::json_error(format!(
                "Host port {} is listed more than once",
                p.host_port
            ));
        }
        let status = DockerService::check_port(p.host_port);
        if !status.available {
            return ResponseHelper::json_error(port_conflict_message(p.host_port, &status));
        }
    }

    match DockerService::create_container(&create_req) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created Docker container '{}' (image: '{}')",
                admin_username, create_req.name, create_req.image
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

    match DockerService::remove_container(&name, force) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match DockerService::pull_image(&req.image, &req.tag) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let force = req.force.unwrap_or(false);

    match DockerService::remove_image(&image_id, force) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match docker_lib::get_network(&name) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

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

    match docker_lib::create_network(&req.name, driver, subnet, gateway, internal) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match docker_lib::connect_container_to_network(&req.container, &req.network) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match docker_lib::disconnect_container_from_network(&req.container, &req.network) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    match docker_lib::remove_network(&name) {
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

    let session_token = headers
        .get("x-session-token")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    if session_token.is_empty() {
        return ResponseHelper::json_error("No session token provided".to_string());
    }

    if !DockerService::is_available() {
        return ResponseHelper::json_error("Docker is not available".to_string());
    }

    let output = std::process::Command::new("docker")
        .args(["network", "prune", "-f"])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let msg = String::from_utf8_lossy(&o.stdout).trim().to_string();
            ResponseHelper::json_success(serde_json::json!({ "message": msg }))
        }
        Ok(o) => ResponseHelper::json_error(String::from_utf8_lossy(&o.stderr).to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to prune networks: {}", e)),
    }
}
