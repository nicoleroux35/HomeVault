/* =============================================================================
 * File          : scheduled_tasks.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled-task endpoints: list/create/update/delete tasks, run a task
 *   immediately, and list a task's run history. Every endpoint is
 *   admin-only: tasks execute host actions (power, SMART, commands) —
 *   least privilege puts the whole surface behind require_admin.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/scheduled-tasks in src/web/mod.rs; persistence via
 *   ScheduledTaskRepository; execution via scheduled_tasks_scheduler.
 * =============================================================================
 */

//! Scheduled-task CRUD + run-now + run history (all admin-only).

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::scheduled_tasks_scheduler::{RunTaskError, TaskAction};
use crate::storage::scheduled_task_repo::ScheduledTaskRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

/// Body of POST /scheduled-tasks and PUT /scheduled-tasks/{id}.
#[derive(Debug, Deserialize)]
pub struct TaskRequest {
    pub name: String,
    pub task_type: String,
    pub cron: String,
    /// JSON object — shape depends on `task_type`; validated via
    /// `TaskAction::from_parts` before anything is persisted.
    pub payload: serde_json::Value,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Lists every scheduled task with its last-run summary and next run.
pub async fn handle_tasks_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ScheduledTaskRepository::new(state.db.pool());
    match repo.list().await {
        Ok(tasks) => ResponseHelper::json_success(serde_json::json!({ "tasks": tasks })),
        Err(e) => ResponseHelper::json_error(format!("Failed to list tasks: {e}")),
    }
}

/// Creates a task. The payload is validated here — not just at run time —
/// so an admin gets feedback on a bad cron/payload immediately.
pub async fn handle_task_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TaskRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    match validate_request(&req) {
        Err(resp) => resp,
        Ok((payload_json, next_run)) => {
            let repo = ScheduledTaskRepository::new(state.db.pool());
            match repo
                .create(
                    &req.name,
                    &req.task_type,
                    &req.cron,
                    &payload_json,
                    req.enabled,
                    next_run.as_deref(),
                )
                .await
            {
                Ok(task) => ResponseHelper::json_success(task),
                Err(e) => ResponseHelper::json_error(format!("Failed to create task: {e}")),
            }
        }
    }
}

/// Updates a task (also usable to enable/disable without touching the rest).
pub async fn handle_task_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<TaskRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ScheduledTaskRepository::new(state.db.pool());
    if repo
        .get(&id)
        .await
        .map_err(|e| e.to_string())
        .err()
        .is_some()
    {
        return ResponseHelper::json_error("Task not found".to_string());
    }
    match validate_request(&req) {
        Err(resp) => resp,
        Ok((payload_json, next_run)) => {
            match repo
                .update(
                    &id,
                    &req.name,
                    &req.task_type,
                    &req.cron,
                    &payload_json,
                    req.enabled,
                    next_run.as_deref(),
                )
                .await
            {
                Ok(()) => match repo.get(&id).await {
                    Ok(Some(task)) => ResponseHelper::json_success(task),
                    Ok(None) => ResponseHelper::json_error("Task not found".to_string()),
                    Err(e) => ResponseHelper::json_error(format!("Failed to load task: {e}")),
                },
                Err(e) => ResponseHelper::json_error(format!("Failed to update task: {e}")),
            }
        }
    }
}

/// Deletes a task (run history goes with it, via ON DELETE CASCADE).
pub async fn handle_task_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ScheduledTaskRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "deleted": true })),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete task: {e}")),
    }
}

/// Runs a task immediately (independent of its cron). Executes through the
/// same validated path as the scheduler.
pub async fn handle_task_run_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ScheduledTaskRepository::new(state.db.pool());
    let task = match repo.get(&id).await {
        Ok(Some(t)) => t,
        Ok(None) => return ResponseHelper::json_error("Task not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to load task: {e}")),
    };
    match crate::scheduled_tasks_scheduler::run_task_now(&state, &task).await {
        Ok(()) => match repo.get(&id).await {
            Ok(Some(t)) => ResponseHelper::json_success(t),
            _ => ResponseHelper::json_success(serde_json::json!({ "started": true })),
        },
        // A run of this task is still in progress (scheduled tick or another
        // "Run now"): nothing was executed, so the row is untouched.
        Err(RunTaskError::Busy(msg)) => {
            ResponseHelper::json_error_with_status(StatusCode::CONFLICT, msg)
        }
        Err(RunTaskError::Failed(e)) => ResponseHelper::json_error(format!("Task failed: {e}")),
    }
}

/// Run history for one task (newest first, bounded).
pub async fn handle_task_runs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = ScheduledTaskRepository::new(state.db.pool());
    match repo.list_runs(&id, 50).await {
        Ok(runs) => ResponseHelper::json_success(serde_json::json!({ "runs": runs })),
        Err(e) => ResponseHelper::json_error(format!("Failed to list runs: {e}")),
    }
}

/// Validates the request body end to end: name, cron parse, payload shape.
/// On success returns the canonical payload JSON plus the precomputed next
/// run (so a freshly created task displays a sane `next_run_at`).
// `Response` as Err mirrors the guard helpers' shape (see the `#[allow]`
// note on `require_admin` for why it isn't boxed).
#[allow(clippy::result_large_err)]
fn validate_request(req: &TaskRequest) -> Result<(String, Option<String>), Response> {
    let name = req.name.trim();
    if name.is_empty() || name.len() > 120 {
        return Err(ResponseHelper::json_error(
            "Task name must be 1-120 characters".to_string(),
        ));
    }
    // Both gates: cron must parse (compute_next_run) and the payload must
    // validate against the action schema.
    let after = chrono::Utc::now();
    let next_run = crate::restic_scheduler::compute_next_run(req.cron.trim(), after)
        .map_err(|e| ResponseHelper::json_error(format!("Invalid cron expression: {e}")))?
        .map(|d| d.to_rfc3339());
    let payload_json = serde_json::to_string(&req.payload)
        .map_err(|e| ResponseHelper::json_error(format!("Invalid payload: {e}")))?;
    TaskAction::from_parts(&req.task_type, &payload_json).map_err(ResponseHelper::json_error)?;
    Ok((payload_json, next_run))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(name: &str, task_type: &str, cron: &str, payload: serde_json::Value) -> TaskRequest {
        TaskRequest {
            name: name.into(),
            task_type: task_type.into(),
            cron: cron.into(),
            payload,
            enabled: true,
        }
    }

    #[test]
    fn validate_accepts_a_well_formed_task() {
        let r = req(
            "Nightly snapshot",
            "snapshot",
            "0 2 * * *",
            serde_json::json!({"pool": "tank", "recursive": false}),
        );
        let (payload, next) = validate_request(&r).expect("valid task");
        assert!(payload.contains("tank"));
        assert!(next.is_some(), "next run precomputed from the cron");
    }

    #[test]
    fn validate_rejects_bad_names_crons_and_payloads() {
        assert!(validate_request(&req(
            "",
            "command",
            "* * * * *",
            serde_json::json!({"command": "/bin/true", "args": []})
        ))
        .is_err());
        assert!(validate_request(&req(
            "x",
            "command",
            "not a cron",
            serde_json::json!({"command": "/bin/true", "args": []})
        ))
        .is_err());
        // Cron fine, payload invalid for the declared type:
        assert!(validate_request(&req(
            "x",
            "snapshot",
            "* * * * *",
            serde_json::json!({"pool": "a b"})
        ))
        .is_err());
        // Payload valid, type unknown:
        assert!(validate_request(&req("x", "format", "* * * * *", serde_json::json!({}))).is_err());
    }
}
