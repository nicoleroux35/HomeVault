use crate::compose::ComposeManager;
use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

/// Body for creating a new Compose project.
#[derive(serde::Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub config: String,
}

/// Body for updating an existing Compose project's config.
#[derive(serde::Deserialize)]
pub struct UpdateProjectRequest {
    pub config: String,
}

/// Body for validating a Compose config string without deploying it.
#[derive(serde::Deserialize)]
pub struct ValidateConfigRequest {
    pub config: String,
}

/// Lists all Docker Compose projects managed by the system.
#[axum_macros::debug_handler]
pub async fn handle_compose_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    match tokio::task::spawn_blocking(ComposeManager::list_projects).await {
        Ok(Ok(projects)) => ResponseHelper::json_success(projects),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to list projects: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Gets details for a specific Compose project by name.
#[axum_macros::debug_handler]
pub async fn handle_compose_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let name = name.clone();
    match tokio::task::spawn_blocking(move || ComposeManager::get_project(&name)).await {
        Ok(Ok(project)) => ResponseHelper::json_success(project),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to get project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Returns the raw `docker-compose.yml` text for a project as
/// `{ "config": "<yaml>" }`, for the Docker app's "Edit compose" modal.
/// Paired with `handle_compose_update` (PUT), which writes an edited version
/// back. Admin-only, like every other Compose handler.
#[axum_macros::debug_handler]
pub async fn handle_compose_get_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    match tokio::task::spawn_blocking(move || ComposeManager::get_project_config(&name)).await {
        Ok(Ok(config)) => ResponseHelper::json_success(serde_json::json!({ "config": config })),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to read project config: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Creates a new Docker Compose project from a config string.
#[axum_macros::debug_handler]
pub async fn handle_compose_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateProjectRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if req.name.trim().is_empty() {
        return ResponseHelper::json_error("Project name is required".to_string());
    }

    if req.config.trim().is_empty() {
        return ResponseHelper::json_error("Configuration is required".to_string());
    }

    let name_chars: Vec<char> = req.name.chars().collect();
    if name_chars.is_empty() || !name_chars[0].is_ascii_alphanumeric() {
        return ResponseHelper::json_error(
            "Invalid project name. Must start with a letter or number.".to_string(),
        );
    }

    if !req
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return ResponseHelper::json_error(
            "Invalid project name. Use letters, numbers, hyphens, underscores, and dots only."
                .to_string(),
        );
    }

    match ComposeManager::create_project(&req.name, &req.config) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' created Compose project '{}'",
                admin_username, req.name
            ));
            ResponseHelper::json_success("Project created successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create project: {}", e)),
    }
}

/// Updates an existing Compose project's configuration.
#[axum_macros::debug_handler]
pub async fn handle_compose_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Json(req): axum::extract::Json<UpdateProjectRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    match ComposeManager::update_project(&name, &req.config) {
        Ok(_) => ResponseHelper::json_success("Project updated successfully".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to update project: {}", e)),
    }
}

/// Deletes a Compose project.
#[axum_macros::debug_handler]
pub async fn handle_compose_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match ComposeManager::delete_project(&name) {
        Ok(_) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project deleted successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete project: {}", e)),
    }
}

/// Starts all services in a Compose project.
#[axum_macros::debug_handler]
pub async fn handle_compose_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match ComposeManager::start_project(&name) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' started Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project started successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to start project: {}", e)),
    }
}

/// Stops all services in a Compose project.
#[axum_macros::debug_handler]
pub async fn handle_compose_stop(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match ComposeManager::stop_project(&name) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' stopped Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project stopped successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to stop project: {}", e)),
    }
}

/// Restarts all services in a Compose project.
#[axum_macros::debug_handler]
pub async fn handle_compose_restart(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    match ComposeManager::restart_project(&name) {
        Ok(_) => {
            crate::events::log_event(format!(
                "Admin '{}' restarted Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project restarted successfully".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to restart project: {}", e)),
    }
}

/// Lists all services within a specific Compose project.
#[axum_macros::debug_handler]
pub async fn handle_compose_services(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    match ComposeManager::get_project_services(&name) {
        Ok(services) => ResponseHelper::json_success(services),
        Err(e) => ResponseHelper::json_error(format!("Failed to get services: {}", e)),
    }
}

/// Returns whether Docker Compose is available on the system.
#[axum_macros::debug_handler]
pub async fn handle_compose_available(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    ResponseHelper::json_success(serde_json::json!({
        "available": ComposeManager::is_available()
    }))
}

/// Validates a Compose config string without deploying it (the Compose tab's
/// "Validate syntax" button). Returns success when the config parses cleanly,
/// or the compose error message otherwise.
#[axum_macros::debug_handler]
pub async fn handle_compose_validate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<ValidateConfigRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if req.config.trim().is_empty() {
        return ResponseHelper::json_error("Configuration is required".to_string());
    }

    let config = req.config;
    match tokio::task::spawn_blocking(move || ComposeManager::validate_config(&config)).await {
        Ok(Ok(())) => ResponseHelper::json_success("Configuration is valid".to_string()),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Invalid configuration: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}
