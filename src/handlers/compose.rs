/* =============================================================================
 * File          : compose.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Docker Compose project endpoints: create, read, update and delete
 *   projects, validate their configuration, start/stop/restart them, list
 *   their services, and check availability of the compose tooling. All
 *   endpoints are admin-only; execution is delegated to ComposeManager.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/compose in src/web/mod.rs; the Apps Center install flow
 *   also goes through ComposeManager (see handlers/docker_catalog.rs).
 * =============================================================================
 */

//! Docker Compose project endpoints (CRUD, validation, lifecycle) — admin-only.

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

    let (name, config) = (req.name.clone(), req.config.clone());
    match tokio::task::spawn_blocking(move || ComposeManager::create_project(&name, &config)).await
    {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "Admin '{}' created Compose project '{}'",
                admin_username, req.name
            ));
            ResponseHelper::json_success("Project created successfully".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to create project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    let config = req.config;
    match tokio::task::spawn_blocking(move || ComposeManager::update_project(&name, &config)).await
    {
        Ok(Ok(_)) => ResponseHelper::json_success("Project updated successfully".to_string()),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to update project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    let target = name.clone();
    match tokio::task::spawn_blocking(move || ComposeManager::delete_project(&target)).await {
        Ok(Ok(_)) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project deleted successfully".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to delete project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    let target = name.clone();
    match tokio::task::spawn_blocking(move || ComposeManager::start_project(&target)).await {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "Admin '{}' started Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project started successfully".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to start project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    let target = name.clone();
    match tokio::task::spawn_blocking(move || ComposeManager::stop_project(&target)).await {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "Admin '{}' stopped Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project stopped successfully".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to stop project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    let target = name.clone();
    match tokio::task::spawn_blocking(move || ComposeManager::restart_project(&target)).await {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "Admin '{}' restarted Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project restarted successfully".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to restart project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
    }
}

/// Pulls the newest images for a project and redeploys it. `up -d` alone
/// never re-pulls, so without this a stack running for months stays on its
/// original image.
#[axum_macros::debug_handler]
pub async fn handle_compose_pull(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let target = name.clone();
    match tokio::task::spawn_blocking(move || ComposeManager::pull_project(&target)).await {
        Ok(Ok(_)) => {
            crate::events::log_event(format!(
                "Admin '{}' pulled and redeployed Compose project '{}'",
                admin_username, name
            ));
            ResponseHelper::json_success("Project updated successfully".to_string())
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to update project: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    match tokio::task::spawn_blocking(move || ComposeManager::get_project_services(&name)).await {
        Ok(Ok(services)) => ResponseHelper::json_success(services),
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to get services: {}", e)),
        Err(e) => ResponseHelper::json_error(format!("Task join error: {}", e)),
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

    let available = tokio::task::spawn_blocking(ComposeManager::is_available)
        .await
        .unwrap_or(false);
    ResponseHelper::json_success(serde_json::json!({ "available": available }))
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
