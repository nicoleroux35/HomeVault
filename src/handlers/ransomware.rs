//! HTTP handlers for anti-ransomware protection: settings, protected paths
//! (with honeypot placement), and detected events.

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::ransomware;
use crate::ransomware_watcher::WatchCommand;
use crate::storage::RansomwareRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

/// `GET /api/ransomware/status`
pub async fn handle_ransomware_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = RansomwareRepository::new(state.db.pool());
    let settings = repo.get_settings().await.unwrap_or_default();
    let paths = repo.list_protected_paths().await.unwrap_or_default();
    ResponseHelper::json_success(serde_json::json!({
        "settings": settings,
        "protected_path_count": paths.len(),
    }))
}

/// `GET /api/ransomware/settings`
pub async fn handle_ransomware_settings_get(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = RansomwareRepository::new(state.db.pool());
    match repo.get_settings().await {
        Ok(settings) => ResponseHelper::json_success(settings),
        Err(e) => ResponseHelper::json_error(format!("Failed to load ransomware settings: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct RansomwareSettingsUpdate {
    pub enabled: bool,
    pub honeypots_enabled: bool,
    pub rate_threshold_events: u32,
    pub rate_threshold_seconds: u32,
    pub auto_response: String,
}

const VALID_AUTO_RESPONSES: [&str; 3] = ["alert_only", "lock_account", "disable_share"];

/// `PUT /api/ransomware/settings`
pub async fn handle_ransomware_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RansomwareSettingsUpdate>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    if !VALID_AUTO_RESPONSES.contains(&body.auto_response.as_str()) {
        return ResponseHelper::json_error("Invalid auto_response value".to_string());
    }

    let repo = RansomwareRepository::new(state.db.pool());
    match repo
        .save_settings(
            body.enabled,
            body.honeypots_enabled,
            body.rate_threshold_events,
            body.rate_threshold_seconds,
            &body.auto_response,
        )
        .await
    {
        Ok(()) => {
            let _ = state.ransomware_watch_tx.send(WatchCommand::ReloadSettings);
            ResponseHelper::json_success(serde_json::json!({ "ok": true }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save ransomware settings: {}", e)),
    }
}

/// `GET /api/ransomware/protected-paths`
pub async fn handle_ransomware_protected_paths_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = RansomwareRepository::new(state.db.pool());
    match repo.list_protected_paths().await {
        Ok(paths) => ResponseHelper::json_success(paths),
        Err(e) => ResponseHelper::json_error(format!("Failed to list protected paths: {}", e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ProtectedPathRequest {
    pub path: String,
    pub share_name: Option<String>,
    pub share_type: Option<String>,
    #[serde(default = "default_true")]
    pub place_honeypot: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /api/ransomware/protected-paths` — registers a path for watching
/// (optionally placing a honeypot decoy file in it) and live-adds it to the
/// running watcher via `ransomware_watch_tx`, no restart required.
pub async fn handle_ransomware_protected_path_add(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ProtectedPathRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let Some(resolved) = state.path_guard.validate(&body.path) else {
        return ResponseHelper::json_error("Path is not within an allowed root".to_string());
    };
    if !resolved.is_dir() {
        return ResponseHelper::json_error("Path must be an existing directory".to_string());
    }

    let honeypot_filename = if body.place_honeypot {
        match ransomware::place_honeypot(&resolved).await {
            Ok(name) => Some(name),
            Err(e) => {
                tracing::warn!("Failed to place honeypot in '{}': {}", resolved.display(), e);
                None
            }
        }
    } else {
        None
    };

    let repo = RansomwareRepository::new(state.db.pool());
    let path_str = resolved.to_string_lossy().to_string();
    match repo
        .add_protected_path(&path_str, body.share_name.as_deref(), body.share_type.as_deref(), honeypot_filename.as_deref())
        .await
    {
        Ok(protected) => {
            let _ = state.ransomware_watch_tx.send(WatchCommand::AddPath {
                id: protected.id.clone(),
                path: PathBuf::from(&protected.path),
                honeypot_filename: protected.honeypot_filename.clone(),
            });
            crate::events::log_event(format!(
                "Admin '{}' added ransomware-protected path '{}'",
                admin_username, path_str
            ));
            ResponseHelper::json_success(protected)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to add protected path: {}", e)),
    }
}

/// `DELETE /api/ransomware/protected-paths/{id}`
pub async fn handle_ransomware_protected_path_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = RansomwareRepository::new(state.db.pool());
    let existing = match repo.get_protected_path(&id).await {
        Ok(Some(p)) => p,
        Ok(None) => return ResponseHelper::json_error("Protected path not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to load protected path: {}", e)),
    };

    match repo.remove_protected_path(&id).await {
        Ok(true) => {
            let _ = state.ransomware_watch_tx.send(WatchCommand::RemovePath { id: id.clone() });
            if let Some(filename) = &existing.honeypot_filename {
                ransomware::remove_honeypot(&PathBuf::from(&existing.path), filename).await;
            }
            crate::events::log_event(format!(
                "Admin '{}' removed ransomware-protected path '{}'",
                admin_username, existing.path
            ));
            ResponseHelper::json_success(serde_json::json!({ "ok": true }))
        }
        Ok(false) => ResponseHelper::json_error("Protected path not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to remove protected path: {}", e)),
    }
}

/// `GET /api/ransomware/events`
pub async fn handle_ransomware_events_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = RansomwareRepository::new(state.db.pool());
    match repo.list_events(100).await {
        Ok(events) => ResponseHelper::json_success(events),
        Err(e) => ResponseHelper::json_error(format!("Failed to list events: {}", e)),
    }
}
