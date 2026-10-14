/* =============================================================================
 * File          : compose.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Docker Compose project management: writes each app's compose file under
 *   the compose directory, renders the {{DATA_DIR}}/{{PORT}} template
 *   placeholders, and drives the docker compose CLI (up/down/restart/ps,
 *   config validation) for install, update and uninstall flows.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tokio (>= 1.52)
 *
 * Usage:
 *   handlers/compose.rs and the app catalog install/uninstall/update
 *   flows (handlers/docker_catalog.rs).
 * =============================================================================
 */

//! Docker Compose project management (files, templates, CLI lifecycle).

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use thiserror::Error;

/// Errors that can occur during Docker Compose operations.
#[derive(Error, Debug)]
pub enum ComposeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Docker Compose not available")]
    NotAvailable,
    #[error("Project not found: {0}")]
    ProjectNotFound(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// A Docker Compose project with its services and current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeProject {
    pub name: String,
    pub config_path: String,
    pub services: Vec<String>,
    /// One of `"running"`, `"stopped"`, `"partial"`, or `"unknown"`.
    pub status: String,
}

/// A single container service within a Compose project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerService {
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
}

/// Manager for Docker Compose projects stored in the compose directory.
pub struct ComposeManager;

impl ComposeManager {
    fn get_compose_dir() -> PathBuf {
        crate::config::paths::compose_dir()
    }

    /// Validates a project name before it's ever joined onto the compose
    /// directory. Every method below takes a project name that ultimately
    /// comes from an admin-supplied URL path segment; without this check,
    /// a name like `..` or `../../etc` would let `get_compose_dir().join(name)`
    /// escape the compose directory entirely — most dangerously in
    /// `delete_project`, which recursively removes whatever that path
    /// resolves to. `create_project`'s caller already validates its `name`
    /// this same way; this makes that check apply everywhere, not just there.
    fn validate_name(name: &str) -> Result<(), ComposeError> {
        let valid = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            && !name.contains("..");
        if valid {
            Ok(())
        } else {
            Err(ComposeError::CommandFailed(format!(
                "Invalid project name: {}",
                name
            )))
        }
    }

    fn ensure_compose_dir() -> Result<(), ComposeError> {
        let dir = Self::get_compose_dir();
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn is_available() -> bool {
        Command::new("docker")
            .args(["compose", "version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Validates a `docker-compose.yml` string without deploying it, by piping
    /// it to `docker compose -f - config --quiet`. Returns `Ok(())` when the
    /// config parses cleanly, or the compose error message otherwise. Used by
    /// the Compose tab's "Validate syntax" button so an admin catches YAML /
    /// schema mistakes before hitting Deploy.
    pub fn validate_config(config: &str) -> Result<(), ComposeError> {
        let mut child = Command::new("docker")
            .args(["compose", "-f", "-", "config", "--quiet"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(config.as_bytes())?;
        }

        let output = child.wait_with_output()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(ComposeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ))
        }
    }

    pub fn list_projects() -> Result<Vec<ComposeProject>, ComposeError> {
        Self::ensure_compose_dir()?;
        let dir = Self::get_compose_dir();

        let mut projects = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();

                    let config_path = path.join("docker-compose.yml");
                    let services = Self::get_services_from_config(&config_path).unwrap_or_default();
                    let status =
                        Self::get_project_status(&name).unwrap_or_else(|_| "unknown".to_string());

                    projects.push(ComposeProject {
                        name,
                        config_path: config_path.to_string_lossy().to_string(),
                        services,
                        status,
                    });
                }
            }
        }

        Ok(projects)
    }

    fn get_services_from_config(config_path: &PathBuf) -> Result<Vec<String>, ComposeError> {
        if !config_path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(config_path)?;
        let mut services = Vec::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("services:") {
                continue;
            }
            if trimmed.ends_with(':') && !trimmed.starts_with('#') {
                let service_name = trimmed.trim_end_matches(':').to_string();
                if !service_name.is_empty()
                    && service_name
                        .chars()
                        .next()
                        .map(|c| c.is_lowercase())
                        .unwrap_or(false)
                {
                    services.push(service_name);
                }
            }
        }

        Ok(services)
    }

    /// Parses `docker compose ps --format json` stdout into per-service
    /// objects. Compose emits either a JSON array or, on newer Compose (v2+),
    /// one JSON object per line (JSON Lines) — a single running service comes
    /// back as one bare object, not an array. Handles all three shapes.
    fn parse_ps_output(stdout: &str) -> Vec<serde_json::Value> {
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(serde_json::Value::Array(arr)) => arr,
            Ok(obj @ serde_json::Value::Object(_)) => vec![obj],
            _ => trimmed
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
                .collect(),
        }
    }

    /// `"running"` (every service up), `"stopped"` (none / no project),
    /// `"partial"` (some up), from parsed `docker compose ps` objects.
    fn status_from_ps(states: &[serde_json::Value]) -> String {
        if states.is_empty() {
            return "stopped".to_string();
        }
        let running_count = states
            .iter()
            .filter(|s| s.get("State").and_then(|v| v.as_str()) == Some("running"))
            .count();
        if running_count == 0 {
            "stopped".to_string()
        } else if running_count == states.len() {
            "running".to_string()
        } else {
            "partial".to_string()
        }
    }

    fn get_project_status(name: &str) -> Result<String, ComposeError> {
        let project_dir = Self::get_compose_dir().join(name);

        let output = Command::new("docker")
            .args(["compose", "ps", "--format", "json"])
            .current_dir(&project_dir)
            .output()?;

        if !output.status.success() {
            return Ok("stopped".to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(Self::status_from_ps(&Self::parse_ps_output(&stdout)))
    }

    pub fn get_project(name: &str) -> Result<ComposeProject, ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        let config_path = project_dir.join("docker-compose.yml");
        let services = Self::get_services_from_config(&config_path)?;
        let status = Self::get_project_status(name)?;

        Ok(ComposeProject {
            name: name.to_string(),
            config_path: config_path.to_string_lossy().to_string(),
            services,
            status,
        })
    }

    /// Returns the raw `docker-compose.yml` text for a project, so the Docker
    /// app's "Edit compose" modal can show the current file. Paired with
    /// `update_project`, which writes an edited version back. Rejects the same
    /// traversal-prone names as every other method here.
    pub fn get_project_config(name: &str) -> Result<String, ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        let config_path = project_dir.join("docker-compose.yml");
        Ok(std::fs::read_to_string(&config_path)?)
    }

    pub fn create_project(name: &str, config: &str) -> Result<(), ComposeError> {
        Self::validate_name(name)?;
        Self::ensure_compose_dir()?;

        let project_dir = Self::get_compose_dir().join(name);

        if project_dir.exists() {
            return Err(ComposeError::CommandFailed(format!(
                "Project '{}' already exists",
                name
            )));
        }

        std::fs::create_dir_all(&project_dir)?;

        let config_path = project_dir.join("docker-compose.yml");
        std::fs::write(&config_path, config)?;

        Ok(())
    }

    pub fn update_project(name: &str, config: &str) -> Result<(), ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        let config_path = project_dir.join("docker-compose.yml");
        std::fs::write(&config_path, config)?;

        Ok(())
    }

    pub fn delete_project(name: &str) -> Result<(), ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        std::fs::remove_dir_all(&project_dir)?;

        Ok(())
    }

    pub fn start_project(name: &str) -> Result<(), ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        let output = Command::new("docker")
            .args(["compose", "up", "-d"])
            .current_dir(&project_dir)
            .output()?;

        if !output.status.success() {
            return Err(ComposeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    pub fn stop_project(name: &str) -> Result<(), ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        let output = Command::new("docker")
            .args(["compose", "down"])
            .current_dir(&project_dir)
            .output()?;

        if !output.status.success() {
            return Err(ComposeError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    pub fn restart_project(name: &str) -> Result<(), ComposeError> {
        Self::stop_project(name)?;
        Self::start_project(name)
    }

    pub fn get_project_services(name: &str) -> Result<Vec<ContainerService>, ComposeError> {
        Self::validate_name(name)?;
        let project_dir = Self::get_compose_dir().join(name);

        if !project_dir.exists() {
            return Err(ComposeError::ProjectNotFound(name.to_string()));
        }

        let output = Command::new("docker")
            .args(["compose", "ps", "--format", "json"])
            .current_dir(&project_dir)
            .output()?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Reuse the tolerant parser (array / JSON-Lines / single object), then
        // pull the PascalCase fields `docker compose ps` actually emits —
        // deserializing straight into ContainerService's lowercase fields
        // silently produced an empty list on every modern Compose.
        let str_field = |v: &serde_json::Value, key: &str| {
            v.get(key)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string()
        };
        let services = Self::parse_ps_output(&stdout)
            .iter()
            .map(|v| ContainerService {
                name: str_field(v, "Name"),
                image: str_field(v, "Image"),
                state: str_field(v, "State"),
                status: str_field(v, "Status"),
            })
            .collect();

        Ok(services)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ps_output_single_object() {
        // Newer `docker compose ps --format json` with one service: a bare
        // JSON object, not an array.
        let out = r#"{"Name":"jellyfin","Service":"jellyfin","State":"running","Status":"Up 6 hours (healthy)"}"#;
        let states = ComposeManager::parse_ps_output(out);
        assert_eq!(states.len(), 1);
        assert_eq!(ComposeManager::status_from_ps(&states), "running");
    }

    #[test]
    fn test_parse_ps_output_json_lines() {
        // Multiple services: one JSON object per line (JSON Lines).
        let out = "{\"Service\":\"server\",\"State\":\"running\"}\n{\"Service\":\"db\",\"State\":\"running\"}\n{\"Service\":\"redis\",\"State\":\"exited\"}";
        let states = ComposeManager::parse_ps_output(out);
        assert_eq!(states.len(), 3);
        assert_eq!(ComposeManager::status_from_ps(&states), "partial");
    }

    #[test]
    fn test_parse_ps_output_array() {
        // Older Compose emitted a JSON array.
        let out = r#"[{"Service":"a","State":"running"},{"Service":"b","State":"running"}]"#;
        let states = ComposeManager::parse_ps_output(out);
        assert_eq!(states.len(), 2);
        assert_eq!(ComposeManager::status_from_ps(&states), "running");
    }

    #[test]
    fn test_status_from_ps_empty_is_stopped() {
        assert_eq!(ComposeManager::parse_ps_output("   ").len(), 0);
        assert_eq!(ComposeManager::status_from_ps(&[]), "stopped");
    }

    #[test]
    fn test_get_project_config_rejects_traversal() {
        // A traversal-prone name must be refused before any filesystem access,
        // the same way get_project / delete_project reject it.
        for bad in ["..", "../etc", ".", "/etc", "foo/../bar"] {
            assert!(
                matches!(
                    ComposeManager::get_project_config(bad),
                    Err(ComposeError::CommandFailed(_))
                ),
                "name {bad:?} must be rejected as invalid",
            );
        }
    }

    #[test]
    fn test_status_from_ps_all_stopped() {
        let out =
            "{\"Service\":\"a\",\"State\":\"exited\"}\n{\"Service\":\"b\",\"State\":\"created\"}";
        let states = ComposeManager::parse_ps_output(out);
        assert_eq!(ComposeManager::status_from_ps(&states), "stopped");
    }
}
