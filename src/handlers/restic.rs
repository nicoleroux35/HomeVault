use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::restic::{ResticRepo, ResticService};
use crate::restic_scheduler::compute_next_run;
use crate::sse::ProgressEmitter;
use crate::storage::restic_repo::ResticRepository;
use crate::storage::{ResticScheduleRepository, RetentionPolicyRepository};
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub repository: String,
    pub password: String,
    pub retention_policy_id: Option<String>,
    pub source_paths: Vec<String>,
    pub exclude_patterns: Option<String>,
    pub tags: Option<String>,
    pub snapshot_source: Option<String>,
    pub snapshot_pool: Option<String>,
    pub snapshot_dataset: Option<String>,
    pub snapshot_create_before_backup: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateRepoRequest {
    pub name: Option<String>,
    pub repository: Option<String>,
    pub password: Option<String>,
    pub retention_policy_id: Option<String>,
    pub source_paths: Option<Vec<String>>,
    pub exclude_patterns: Option<String>,
    pub tags: Option<String>,
    pub snapshot_source: Option<String>,
    pub snapshot_pool: Option<String>,
    pub snapshot_dataset: Option<String>,
    pub snapshot_create_before_backup: Option<bool>,
}

#[derive(Deserialize)]
pub struct CreateBackupRequest {
    pub repo_id: String,
    pub tags: Option<String>,
    pub exclude_patterns: Option<String>,
    pub snapshot_name: Option<String>,
}

#[derive(Deserialize)]
pub struct RestoreRequest {
    pub repo_id: String,
    pub snapshot_id: String,
    pub target: String,
    pub exclude_patterns: Option<String>,
    pub include_paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct ForgetRequest {
    pub repo_id: String,
    pub keep_last: Option<u64>,
    pub keep_hourly: Option<u64>,
    pub keep_daily: Option<u64>,
    pub keep_weekly: Option<u64>,
    pub keep_monthly: Option<u64>,
    pub keep_yearly: Option<u64>,
    pub keep_within: Option<String>,
    pub prune: Option<bool>,
}

#[derive(Deserialize)]
pub struct CheckRequest {
    pub repo_id: String,
    pub read_data: Option<bool>,
}

#[derive(Deserialize)]
pub struct ReposQuery {
    pub include_password: Option<bool>,
}

pub async fn handle_restic_available(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    ResponseHelper::json_success(serde_json::json!({
        "available": ResticService::is_available()
    }))
}

pub async fn handle_restic_version(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    match ResticService::get_version() {
        Ok(v) => ResponseHelper::json_success(serde_json::json!({ "version": v })),
        Err(e) => ResponseHelper::json_error(e.to_string()),
    }
}

pub async fn handle_restic_repos_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ReposQuery>,
) -> Response {
    // Returning stored repository passwords is admin-only; a plain
    // authenticated user only ever gets the masked list.
    if query.include_password.unwrap_or(false) {
        if let Err(response) = require_admin(&state, &headers).await {
            return response;
        }
    } else if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = ResticRepository::new(state.db.pool());
    match repo.list_all().await {
        Ok(mut repos) => {
            if !query.include_password.unwrap_or(false) {
                for r in &mut repos {
                    r.password = "********".to_string();
                }
            }
            ResponseHelper::json_success(repos)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list repos: {}", e)),
    }
}

pub async fn handle_restic_repo_get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = ResticRepository::new(state.db.pool());
    match repo.get_by_id(&id).await {
        Ok(Some(mut r)) => {
            r.password = "********".to_string();
            ResponseHelper::json_success(r)
        }
        Ok(None) => ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to get repo: {}", e)),
    }
}

pub async fn handle_restic_repo_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateRepoRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if !ResticService::is_available() {
        return ResponseHelper::json_error(
            "restic CLI is not installed or not found in PATH".to_string(),
        );
    }

    let repo = ResticRepository::new(state.db.pool());

    match repo
        .create(
            &req.name,
            &req.repository,
            &req.password,
            req.retention_policy_id.as_deref(),
            &req.source_paths,
            req.exclude_patterns.as_deref(),
            req.tags.as_deref(),
            req.snapshot_source.as_deref().unwrap_or("directory"),
            req.snapshot_pool.as_deref(),
            req.snapshot_dataset.as_deref(),
            req.snapshot_create_before_backup.unwrap_or(false),
        )
        .await
    {
        Ok(new_repo) => {
            let repo_config = ResticRepo {
                id: new_repo.id.clone(),
                name: new_repo.name.clone(),
                repository: new_repo.repository.clone(),
                password: req.password.clone(),
                retention_policy_id: new_repo.retention_policy_id.clone(),
                source_paths: new_repo.source_paths.clone(),
                exclude_patterns: new_repo.exclude_patterns.clone(),
                tags: new_repo.tags.clone(),
                snapshot_source: new_repo.snapshot_source.clone(),
                snapshot_pool: new_repo.snapshot_pool.clone(),
                snapshot_dataset: new_repo.snapshot_dataset.clone(),
                snapshot_create_before_backup: new_repo.snapshot_create_before_backup,
                created_at: new_repo.created_at,
                updated_at: new_repo.updated_at,
            };

            match ResticService::init_repo(&repo_config) {
                Ok(()) => {
                    crate::events::log_event(format!(
                        "Admin '{}' created and initialized backup repository '{}'",
                        admin_username, new_repo.name
                    ));
                    ResponseHelper::json_success(serde_json::json!({
                        "success": true,
                        "message": "Repository created and initialized",
                        "repo": new_repo
                    }))
                }
                Err(e) => {
                    let _ = repo.delete(&new_repo.id).await;
                    ResponseHelper::json_error(format!("Failed to initialize repository: {}", e))
                }
            }
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create repo: {}", e)),
    }
}

pub async fn handle_restic_repo_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<UpdateRepoRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo = ResticRepository::new(state.db.pool());
    match repo
        .update(
            &id,
            req.name.as_deref(),
            req.repository.as_deref(),
            req.password.as_deref(),
            req.retention_policy_id.as_deref(),
            req.source_paths.as_deref(),
            req.exclude_patterns.as_deref(),
            req.tags.as_deref(),
            req.snapshot_source.as_deref(),
            req.snapshot_pool.as_deref(),
            req.snapshot_dataset.as_deref(),
            req.snapshot_create_before_backup,
        )
        .await
    {
        Ok(Some(r)) => ResponseHelper::json_success(r),
        Ok(None) => ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to update repo: {}", e)),
    }
}

pub async fn handle_restic_repo_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = ResticRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => {
            crate::events::log_event_warn(format!(
                "Admin '{}' deleted backup repository (id: {})",
                admin_username, id
            ));
            ResponseHelper::json_success("Repository deleted".to_string())
        }
        Ok(false) => ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete repo: {}", e)),
    }
}

pub async fn handle_restic_snapshots_list(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    match ResticService::list_snapshots(&repo_config) {
        Ok(snapshots) => ResponseHelper::json_success(snapshots),
        Err(e) => ResponseHelper::json_error(format!("Failed to list snapshots: {}", e)),
    }
}

pub async fn handle_restic_backup_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateBackupRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&req.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    crate::events::log_event(format!(
        "Admin '{}' started a manual backup for repository '{}'",
        admin_username, repo_config.name
    ));

    let job_id =
        ProgressEmitter::backup(&repo_config.source_paths.join(","), &repo_config.repository);
    ProgressEmitter::progress(&job_id, 5.0, "Starting restic backup...");

    let result = if repo_config.snapshot_create_before_backup {
        let pool = repo_config.snapshot_pool.as_deref().unwrap_or("zpool");
        let dataset = repo_config
            .snapshot_dataset
            .as_deref()
            .unwrap_or("zpool/data");
        let snap_name = req
            .snapshot_name
            .unwrap_or_else(|| format!("restic-{}", chrono::Utc::now().format("%Y%m%d_%H%M%S")));

        ProgressEmitter::progress(&job_id, 15.0, "Creating filesystem snapshot...");
        ResticService::create_backup_with_snapshot(
            &repo_config,
            pool,
            dataset,
            &snap_name,
            req.exclude_patterns.as_deref(),
            req.tags.as_deref(),
        )
    } else {
        let paths = if repo_config.source_paths.is_empty() {
            return ResponseHelper::json_error(
                "No source paths configured for this repository".to_string(),
            );
        } else {
            repo_config.source_paths.clone()
        };

        ProgressEmitter::progress(&job_id, 30.0, "Running restic backup...");
        ResticService::create_backup(
            &repo_config,
            &paths,
            req.exclude_patterns.as_deref(),
            req.tags.as_deref(),
        )
    };

    match result {
        Ok(backup_result) => {
            ProgressEmitter::progress(&job_id, 90.0, "Recording backup job...");

            if let Ok(db_job_id) = repo_repo
                .create_backup_job(
                    &req.repo_id,
                    Some(&backup_result.snapshot_id),
                    &repo_config.source_paths.join(","),
                    "manual",
                )
                .await
            {
                let _ = repo_repo
                    .update_backup_job(
                        &db_job_id,
                        "completed",
                        backup_result.files_new,
                        backup_result.files_changed,
                        backup_result.files_unmodified,
                        backup_result.data_added_bytes,
                        backup_result.total_files_processed,
                        backup_result.total_bytes,
                        backup_result.duration_secs,
                        None,
                    )
                    .await;
            }

            ProgressEmitter::complete(&job_id, "Restic backup completed successfully");
            crate::events::log_event(format!(
                "Backup completed for repository '{}' ({} files, {} bytes added)",
                repo_config.name,
                backup_result.total_files_processed,
                backup_result.data_added_bytes
            ));
            ResponseHelper::json_success(serde_json::json!({
                "result": backup_result,
                "job_id": job_id
            }))
        }
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Backup failed for repository '{}': {}",
                repo_config.name, e
            ));
            if let Ok(db_job_id) = repo_repo
                .create_backup_job(
                    &req.repo_id,
                    None,
                    &repo_config.source_paths.join(","),
                    "manual",
                )
                .await
            {
                let _ = repo_repo
                    .update_backup_job(
                        &db_job_id,
                        "failed",
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        Some(&e.to_string()),
                    )
                    .await;
            }

            ProgressEmitter::fail(&job_id, &e.to_string());
            ResponseHelper::json_error(format!("Backup failed: {}", e))
        }
    }
}

pub async fn handle_restic_restore(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<RestoreRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&req.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    let job_id = ProgressEmitter::restore(&req.snapshot_id, &req.target);
    ProgressEmitter::progress(&job_id, 10.0, "Starting restore...");
    crate::events::log_event_warn(format!(
        "Admin '{}' started a restore from repository '{}' (snapshot: {}) to '{}'",
        admin_username, repo_config.name, req.snapshot_id, req.target
    ));

    match ResticService::restore_snapshot(
        &repo_config,
        &req.snapshot_id,
        &req.target,
        req.exclude_patterns.as_deref(),
        req.include_paths.as_deref(),
    ) {
        Ok(()) => {
            ProgressEmitter::complete(&job_id, "Restore completed successfully");
            crate::events::log_event(format!(
                "Restore completed for repository '{}' to '{}'",
                repo_config.name, req.target
            ));
            ResponseHelper::json_success(serde_json::json!({ "success": true, "job_id": job_id }))
        }
        Err(e) => {
            ProgressEmitter::fail(&job_id, &e.to_string());
            crate::events::log_event_warn(format!(
                "Restore failed for repository '{}': {}",
                repo_config.name, e
            ));
            ResponseHelper::json_error(format!("Restore failed: {}", e))
        }
    }
}

pub async fn handle_restic_check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CheckRequest>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&req.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    let job_id = ProgressEmitter::verify(&repo_config.name);
    ProgressEmitter::progress(&job_id, 10.0, "Checking repository integrity...");

    match ResticService::check_repo(&repo_config, req.read_data.unwrap_or(false)) {
        Ok(result) => {
            ProgressEmitter::complete(
                &job_id,
                if result.healthy {
                    "Repository is healthy"
                } else {
                    "Repository check found issues"
                },
            );
            ResponseHelper::json_success(serde_json::json!({
                "check": result,
                "job_id": job_id
            }))
        }
        Err(e) => {
            ProgressEmitter::fail(&job_id, &e.to_string());
            ResponseHelper::json_error(format!("Check failed: {}", e))
        }
    }
}

pub async fn handle_restic_stats(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    match ResticService::get_stats(&repo_config) {
        Ok(stats) => ResponseHelper::json_success(stats),
        Err(e) => ResponseHelper::json_error(format!("Failed to get stats: {}", e)),
    }
}

pub async fn handle_restic_forget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<ForgetRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&req.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    match ResticService::forget_snapshots(
        &repo_config,
        req.keep_last,
        req.keep_hourly,
        req.keep_daily,
        req.keep_weekly,
        req.keep_monthly,
        req.keep_yearly,
        req.keep_within,
        req.prune.unwrap_or(true),
    ) {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "success": true })),
        Err(e) => ResponseHelper::json_error(format!("Forget failed: {}", e)),
    }
}

pub async fn handle_restic_prune(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    match ResticService::prune_repo(&repo_config) {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "success": true })),
        Err(e) => ResponseHelper::json_error(format!("Prune failed: {}", e)),
    }
}

pub async fn handle_restic_unlock(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    match ResticService::unlock_repo(&repo_config) {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "success": true })),
        Err(e) => ResponseHelper::json_error(format!("Unlock failed: {}", e)),
    }
}

pub async fn handle_restic_backup_jobs(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = ResticRepository::new(state.db.pool());
    match repo.list_backup_jobs(&repo_id, 50).await {
        Ok(jobs) => ResponseHelper::json_success(jobs),
        Err(e) => ResponseHelper::json_error(format!("Failed to list jobs: {}", e)),
    }
}

pub async fn handle_restic_snapshot_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<serde_json::Value>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo_id = req.get("repo_id").and_then(|v| v.as_str()).unwrap_or("");
    let snapshot_id = req
        .get("snapshot_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if repo_id.is_empty() || snapshot_id.is_empty() {
        return ResponseHelper::json_error("repo_id and snapshot_id are required".to_string());
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    match ResticService::delete_snapshot(&repo_config, snapshot_id) {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "success": true })),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete snapshot: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Backup reminders + restore pre-flight
// ---------------------------------------------------------------------------

/// A repo whose newest successful backup is older than this is "stale" and
/// shows up in the reminder list (and the dashboard banner).
pub(crate) const REMINDER_STALE_DAYS: i64 = 7;

/// Days since `finished_at` (RFC 3339) relative to `now_secs`. Returns `None`
/// when the timestamp is missing or unparseable — the caller treats that as
/// "never backed up".
pub(crate) fn days_since(finished_at: Option<&str>, now_secs: i64) -> Option<i64> {
    let s = finished_at?;
    let dt = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some((now_secs - dt.timestamp()).div_euclid(86_400))
}

/// Parses `df -B1 --output=avail <target>` stdout (header line + bytes).
/// Returns `None` on any unexpected shape — the plan endpoint reports
/// `available_bytes: null` instead of failing.
pub(crate) fn parse_df_available(stdout: &str) -> Option<u64> {
    let mut lines = stdout.lines();
    lines.next()?; // header
    lines.next()?.trim().parse::<u64>().ok()
}

/// Free bytes at `target` via `df`. `None` when the path is unusable.
fn target_available_bytes(target: &str) -> Option<u64> {
    std::process::Command::new("df")
        .args(["-B1", "--output=avail", target])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| parse_df_available(&String::from_utf8_lossy(&o.stdout)))
}

#[derive(Deserialize)]
pub struct RestorePlanQuery {
    pub repo_id: String,
    pub snapshot_id: String,
    pub target: String,
}

/// GET /api/restic/reminders — repos with no recent successful backup.
///
/// Read-only (`require_auth`): each entry carries the repo id/name, days
/// since the last success (`null` = never), and whether an enabled schedule
/// exists, so the UI can suggest "run now" vs "enable the schedule".
pub async fn handle_restic_reminders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }
    let repo_repo = ResticRepository::new(state.db.pool());
    let sched_repo = ResticScheduleRepository::new(state.db.pool());
    let repos = match repo_repo.list_all().await {
        Ok(r) => r,
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };
    let now = chrono::Utc::now().timestamp();
    let mut reminders = Vec::new();
    for repo in repos {
        let last = repo_repo
            .last_successful_backup_at(&repo.id)
            .await
            .unwrap_or(None);
        let days = days_since(last.as_deref(), now);
        let stale = days.map(|d| d >= REMINDER_STALE_DAYS).unwrap_or(true);
        if !stale {
            continue;
        }
        let has_schedule = sched_repo
            .list_by_repo(&repo.id)
            .await
            .map(|s| s.iter().any(|e| e.enabled))
            .unwrap_or(false);
        reminders.push(serde_json::json!({
            "repo_id": repo.id,
            "repo_name": repo.name,
            "days_since_last_success": days,
            "has_enabled_schedule": has_schedule,
        }));
    }
    ResponseHelper::json_success(serde_json::json!({ "reminders": reminders }))
}

/// GET /api/restic/restore-plan — pre-flight space check before a restore.
///
/// Admin-only (the target path is a privileged write). Returns the
/// snapshot's restore size, the target's free bytes (`null` when the target
/// cannot be statted), and whether the restore fits.
pub async fn handle_restic_restore_plan(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RestorePlanQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    if params.repo_id.trim().is_empty()
        || params.snapshot_id.trim().is_empty()
        || params.target.trim().is_empty()
    {
        return ResponseHelper::json_error(
            "repo_id, snapshot_id and target are required".to_string(),
        );
    }
    if params.target.len() > 4096 || params.target.contains('\0') {
        return ResponseHelper::json_error("Invalid target path".to_string());
    }
    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&params.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };
    let required = match ResticService::restore_size(&repo_config, &params.snapshot_id) {
        Ok(n) => n,
        Err(e) => return ResponseHelper::json_error(format!("Failed to size snapshot: {}", e)),
    };
    let target = params.target.clone();
    let available =
        tokio::task::spawn_blocking(move || target_available_bytes(&target)).await.unwrap_or(None);
    let fits = available.map(|a| a >= required).unwrap_or(false);
    ResponseHelper::json_success(serde_json::json!({
        "repo_id": params.repo_id,
        "snapshot_id": params.snapshot_id,
        "target": params.target,
        "required_bytes": required,
        "available_bytes": available,
        "fits": fits,
    }))
}

// ---------------------------------------------------------------------------
// Retention policies
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateRetentionPolicyRequest {
    pub name: String,
    pub max_count: Option<i64>,
    pub max_age_days: Option<i64>,
    pub max_size_gb: Option<i64>,
    pub apply_to_all: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateRetentionPolicyRequest {
    pub name: Option<String>,
    pub max_count: Option<i64>,
    pub max_age_days: Option<i64>,
    pub max_size_gb: Option<i64>,
    pub apply_to_all: Option<bool>,
}

pub async fn handle_retention_policies_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = RetentionPolicyRepository::new(state.db.pool());
    match repo.list_all().await {
        Ok(policies) => ResponseHelper::json_success(policies),
        Err(e) => ResponseHelper::json_error(format!("Failed to list retention policies: {}", e)),
    }
}

pub async fn handle_retention_policy_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateRetentionPolicyRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if req.name.trim().is_empty() {
        return ResponseHelper::json_error("Policy name is required".to_string());
    }

    let repo = RetentionPolicyRepository::new(state.db.pool());
    match repo
        .create(
            req.name.trim(),
            req.max_count,
            req.max_age_days,
            req.max_size_gb,
            req.apply_to_all.unwrap_or(false),
        )
        .await
    {
        Ok(policy) => {
            crate::events::log_event(format!(
                "Admin '{}' created backup retention policy '{}'",
                admin_username,
                req.name.trim()
            ));
            ResponseHelper::json_success(policy)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create retention policy: {}", e)),
    }
}

pub async fn handle_retention_policy_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<UpdateRetentionPolicyRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let repo = RetentionPolicyRepository::new(state.db.pool());
    match repo
        .update(
            &id,
            req.name.as_deref(),
            req.max_count,
            req.max_age_days,
            req.max_size_gb,
            req.apply_to_all,
        )
        .await
    {
        Ok(Some(policy)) => ResponseHelper::json_success(policy),
        Ok(None) => ResponseHelper::json_error("Retention policy not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to update retention policy: {}", e)),
    }
}

pub async fn handle_retention_policy_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = RetentionPolicyRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{}' deleted backup retention policy (id: {})",
                admin_username, id
            ));
            ResponseHelper::json_success("Retention policy deleted".to_string())
        }
        Ok(false) => ResponseHelper::json_error("Retention policy not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete retention policy: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// Backup schedules
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateScheduleRequest {
    pub repo_id: String,
    pub name: String,
    pub cron_expression: String,
    pub enabled: Option<bool>,
}

#[derive(Deserialize)]
pub struct UpdateScheduleRequest {
    pub name: Option<String>,
    pub cron_expression: Option<String>,
}

#[derive(Deserialize)]
pub struct SetScheduleEnabledRequest {
    pub enabled: bool,
}

/// GET /api/restic/schedules?repo_id=... - list schedules, optionally filtered to one repo
pub async fn handle_restic_schedules_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SchedulesQuery>,
) -> Response {
    if let Err(response) = require_auth(&state, &headers).await {
        return response;
    }

    let repo = ResticScheduleRepository::new(state.db.pool());
    let result = match query.repo_id {
        Some(repo_id) => repo.list_by_repo(&repo_id).await,
        None => repo.list_all().await,
    };

    match result {
        Ok(schedules) => ResponseHelper::json_success(schedules),
        Err(e) => ResponseHelper::json_error(format!("Failed to list schedules: {}", e)),
    }
}

#[derive(Deserialize)]
pub struct SchedulesQuery {
    pub repo_id: Option<String>,
}

pub async fn handle_restic_schedule_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CreateScheduleRequest>,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    if req.name.trim().is_empty() {
        return ResponseHelper::json_error("Schedule name is required".to_string());
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    match repo_repo.get_by_id(&req.repo_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    }

    let next_run = match compute_next_run(&req.cron_expression, chrono::Utc::now()) {
        Ok(next) => next,
        Err(e) => return ResponseHelper::json_error(format!("Invalid cron expression: {}", e)),
    };

    let enabled = req.enabled.unwrap_or(true);
    let schedule_repo = ResticScheduleRepository::new(state.db.pool());
    match schedule_repo
        .create(
            &req.repo_id,
            req.name.trim(),
            req.cron_expression.trim(),
            enabled,
            next_run.map(|d| d.to_rfc3339()).as_deref(),
        )
        .await
    {
        Ok(schedule) => {
            crate::events::log_event(format!(
                "Admin '{}' created backup schedule '{}' ({})",
                admin_username,
                req.name.trim(),
                req.cron_expression.trim()
            ));
            ResponseHelper::json_success(schedule)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create schedule: {}", e)),
    }
}

pub async fn handle_restic_schedule_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<UpdateScheduleRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let schedule_repo = ResticScheduleRepository::new(state.db.pool());

    // Recompute next_run_at whenever the cron expression changes, so a saved
    // edit takes effect on its new schedule immediately rather than waiting
    // for the stale next_run_at to be reached first.
    let next_run_override = if let Some(ref expr) = req.cron_expression {
        match compute_next_run(expr, chrono::Utc::now()) {
            Ok(next) => Some(next.map(|d| d.to_rfc3339())),
            Err(e) => return ResponseHelper::json_error(format!("Invalid cron expression: {}", e)),
        }
    } else {
        None
    };

    match schedule_repo
        .update(
            &id,
            req.name.as_deref(),
            req.cron_expression.as_deref(),
            None,
            next_run_override.as_ref().map(|v| v.as_deref()),
        )
        .await
    {
        Ok(Some(schedule)) => ResponseHelper::json_success(schedule),
        Ok(None) => ResponseHelper::json_error("Schedule not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to update schedule: {}", e)),
    }
}

pub async fn handle_restic_schedule_set_enabled(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<SetScheduleEnabledRequest>,
) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }

    let schedule_repo = ResticScheduleRepository::new(state.db.pool());

    // Re-anchor next_run_at to "now" when re-enabling, so a schedule that was
    // disabled for a while doesn't immediately fire for every occurrence it
    // missed.
    let next_run_override = if req.enabled {
        let existing = match schedule_repo.get_by_id(&id).await {
            Ok(Some(s)) => s,
            Ok(None) => return ResponseHelper::json_error("Schedule not found".to_string()),
            Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
        };
        match compute_next_run(&existing.cron_expression, chrono::Utc::now()) {
            Ok(next) => Some(next.map(|d| d.to_rfc3339())),
            Err(e) => return ResponseHelper::json_error(format!("Invalid cron expression: {}", e)),
        }
    } else {
        None
    };

    match schedule_repo
        .update(
            &id,
            None,
            None,
            Some(req.enabled),
            next_run_override.as_ref().map(|v| v.as_deref()),
        )
        .await
    {
        Ok(Some(schedule)) => ResponseHelper::json_success(schedule),
        Ok(None) => ResponseHelper::json_error("Schedule not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to update schedule: {}", e)),
    }
}

pub async fn handle_restic_schedule_delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let admin_username = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };

    let repo = ResticScheduleRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{}' deleted backup schedule (id: {})",
                admin_username, id
            ));
            ResponseHelper::json_success("Schedule deleted".to_string())
        }
        Ok(false) => ResponseHelper::json_error("Schedule not found".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete schedule: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_days_since_counts_whole_days() {
        // 2026-01-01T00:00:00Z vs 2026-01-08T12:00:00Z -> 7 full days.
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-08T12:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(
            days_since(Some("2026-01-01T00:00:00Z"), now),
            Some(7)
        );
        assert_eq!(days_since(None, now), None);
        assert_eq!(days_since(Some("not-a-date"), now), None);
    }

    #[test]
    fn test_reminder_threshold_flags_stale_repos() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .timestamp();
        let stale = days_since(Some("2026-05-20T00:00:00Z"), now).unwrap();
        let fresh = days_since(Some("2026-05-30T00:00:00Z"), now).unwrap();
        assert!(stale >= REMINDER_STALE_DAYS);
        assert!(fresh < REMINDER_STALE_DAYS);
    }

    #[test]
    fn test_parse_df_available_reads_second_line() {
        assert_eq!(
            parse_df_available("Avail\n123456789\n"),
            Some(123456789)
        );
        assert_eq!(parse_df_available("Avail\n"), None);
        assert_eq!(parse_df_available(""), None);
        assert_eq!(parse_df_available("Avail\nnot-a-number\n"), None);
    }
}
