/* =============================================================================
 * File          : restic.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup endpoints backed by restic: repository CRUD (with stored
 *   passwords and retention policies), snapshot listing and deletion,
 *   backup/restore/check operations (long-running, tracked as background
 *   jobs with live SSE progress), forget/prune retention runs, stats, and
 *   the stale-repo reminder list used by the dashboard banner. Availability
 *   and listing are session-auth; repository changes, operations and
 *   revealing a stored password are admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Routes /api/restic in src/web/mod.rs; schedules handled by the restic
 *   scheduler, execution by crate::restic::ResticService.
 * =============================================================================
 */

//! Backup endpoints backed by restic (repositories, snapshots, backup /
//! restore / check / forget / prune as tracked background jobs).

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::restic::{
    RepoRunLock, ResticError, ResticRepo, ResticService, RESTIC_INIT_TIMEOUT, RESTIC_QUERY_TIMEOUT,
};
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

    // A repository with an empty name, an empty location or no source path
    // used to be accepted, then failed on the first backup with restic's own
    // wording. Refuse it here, where the form can show it. Validation comes
    // before the availability probe so the caller learns what is actually
    // wrong with the request, not just that restic is missing.
    if let Err(e) = ResticService::validate_display_name("Repository name", &req.name) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_repository(&req.repository) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_password(&req.password) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_exclude_patterns(req.exclude_patterns.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_tags(req.tags.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }
    for path in &req.source_paths {
        if let Err(e) = ResticService::validate_backup_path(path) {
            return ResponseHelper::json_error(e.to_string());
        }
    }
    if let Some(pool_id) = req.retention_policy_id.as_deref() {
        if !pool_id.trim().is_empty() {
            let policies = RetentionPolicyRepository::new(state.db.pool());
            match policies.get_by_id(pool_id).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return ResponseHelper::json_error("Retention policy not found".to_string())
                }
                Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
            }
        }
    }

    // A repo configured to snapshot before backing up is useless without a
    // pool and a dataset — and it used to be saved that way, failing only at
    // the first run.
    if req.snapshot_create_before_backup.unwrap_or(false) {
        let pool = req
            .snapshot_pool
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let dataset = req
            .snapshot_dataset
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        if pool.is_empty() {
            return ResponseHelper::json_error(
                "A snapshot pool is required when snapshotting before a backup".to_string(),
            );
        }
        if let Err(e) = ResticService::validate_backup_path(&dataset) {
            return ResponseHelper::json_error(format!(
                "Snapshot dataset: {}",
                e.to_string().trim_start_matches("Invalid input: ")
            ));
        }
    }

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

            match ResticService::run_restic_op(
                "restic init",
                Some(RESTIC_INIT_TIMEOUT),
                move || ResticService::init_repo(&repo_config),
            )
            .await
            {
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

    // Only the fields that are present are validated: `update` keeps the
    // stored value for the others.
    if let Some(name) = req.name.as_deref() {
        if let Err(e) = ResticService::validate_display_name("Repository name", name) {
            return ResponseHelper::json_error(e.to_string());
        }
    }
    if let Some(repository) = req.repository.as_deref() {
        if let Err(e) = ResticService::validate_repository(repository) {
            return ResponseHelper::json_error(e.to_string());
        }
    }
    if let Some(password) = req.password.as_deref() {
        if let Err(e) = ResticService::validate_password(password) {
            return ResponseHelper::json_error(e.to_string());
        }
    }
    if let Err(e) = ResticService::validate_exclude_patterns(req.exclude_patterns.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_tags(req.tags.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Some(paths) = req.source_paths.as_deref() {
        for path in paths {
            if let Err(e) = ResticService::validate_backup_path(path) {
                return ResponseHelper::json_error(e.to_string());
            }
        }
    }
    if let Some(policy_id) = req.retention_policy_id.as_deref() {
        if !policy_id.trim().is_empty() {
            let policies = RetentionPolicyRepository::new(state.db.pool());
            match policies.get_by_id(policy_id).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return ResponseHelper::json_error("Retention policy not found".to_string())
                }
                Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
            }
        }
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

    let cfg = repo_config.clone();
    match ResticService::run_restic_op("restic snapshots", Some(RESTIC_QUERY_TIMEOUT), move || {
        ResticService::list_snapshots(&cfg)
    })
    .await
    {
        Ok(snapshots) => ResponseHelper::json_success(snapshots),
        Err(e) => ResponseHelper::json_error(format!("Failed to list snapshots: {}", e)),
    }
}

/// `POST /api/restic/backup` — **starts** a backup and answers immediately.
///
/// The handler used to run restic inline, so the HTTP request stayed open for
/// the whole run (hours for a multi-TB dataset) and the frontend only called
/// `trackJob` once the job was already over: the floating progress island and
/// the activity taskbar both showed a finished job, and a browser timeout
/// looked like a failed backup. The run now happens in a spawned task, the
/// response carries the progress `job_id`, and the SSE stream reports real
/// percentages read from `restic backup --json`.
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

    // Everything is validated before the run slot is taken, so a rejected
    // request never blocks the repository.
    if let Err(e) = ResticService::validate_exclude_patterns(req.exclude_patterns.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_tags(req.tags.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }

    let snapshot_name = req
        .snapshot_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("restic-{}", chrono::Utc::now().format("%Y%m%d_%H%M%S")));

    let (snapshot_pool, snapshot_dataset) = if repo_config.snapshot_create_before_backup {
        if let Err(e) = ResticService::validate_snapshot_name(&snapshot_name) {
            return ResponseHelper::json_error(e.to_string());
        }
        let pool = repo_config.snapshot_pool.clone().unwrap_or_default();
        let dataset = repo_config.snapshot_dataset.clone().unwrap_or_default();
        if pool.trim().is_empty() || dataset.trim().is_empty() {
            return ResponseHelper::json_error(
                "This repository is configured to snapshot before backing up, but no snapshot pool/dataset is set"
                    .to_string(),
            );
        }
        if let Err(e) = ResticService::validate_backup_path(&dataset) {
            return ResponseHelper::json_error(e.to_string());
        }
        (pool, dataset)
    } else {
        if repo_config.source_paths.is_empty() {
            return ResponseHelper::json_error(
                "No source paths configured for this repository".to_string(),
            );
        }
        for path in &repo_config.source_paths {
            if let Err(e) = ResticService::validate_backup_path(path) {
                return ResponseHelper::json_error(e.to_string());
            }
        }
        (String::new(), String::new())
    };

    // One run per repository: restic takes an exclusive lock anyway, and a
    // second run only fails after scanning the source tree.
    let Some(run_lock) = RepoRunLock::acquire(&repo_config.id) else {
        return ResponseHelper::json_error(
            "A backup or restore is already running on this repository".to_string(),
        );
    };

    if !ResticService::is_available() {
        return ResponseHelper::json_error(
            "restic CLI is not installed or not found in PATH".to_string(),
        );
    }

    let source = if repo_config.snapshot_create_before_backup {
        snapshot_dataset.clone()
    } else {
        repo_config.source_paths.join(",")
    };

    // The job row is created *before* the run (status `running`), so the
    // history records when the backup started, the UI can show it while it
    // runs, and a crash leaves a row the startup reaper can close instead of
    // no trace at all.
    let db_job_id = match repo_repo
        .create_backup_job(&req.repo_id, None, &source, "manual")
        .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("Could not record the backup job before starting it: {}", e);
            None
        }
    };

    let job_id = ProgressEmitter::backup(&source, &repo_config.repository);
    ProgressEmitter::progress(&job_id, 1.0, "Starting restic backup...");
    // Returned to the client so it can reload the history row it will become.
    let backup_job_id = db_job_id.clone();

    let cancel_id = job_id.clone();
    ProgressEmitter::register_cancel_hook(
        &job_id,
        Box::new(move || {
            if !ResticService::cancel_run(&cancel_id) {
                tracing::warn!(
                    "Backup cancellation requested for '{}', which had not started its restic process yet",
                    cancel_id
                );
            }
        }),
    );

    crate::events::log_event(format!(
        "Admin '{}' started a manual backup for repository '{}'",
        admin_username, repo_config.name
    ));

    let task_job_id = job_id.clone();
    let task_repo = repo_config.clone();
    let task_pool = state.db.pool();
    let exclude = req.exclude_patterns.clone();
    let tags = req.tags.clone();
    tokio::spawn(async move {
        // Released when this task ends, whatever the outcome.
        let _run_lock = run_lock;
        let job = task_job_id.clone();
        let repo_for_run = task_repo.clone();

        let joined = tokio::task::spawn_blocking(move || {
            let mut emit = |p: crate::restic::ResticProgress| {
                // 1..99 so the bar never sits at 100 while restic is still
                // writing the summary.
                let pct = 1.0 + (p.percent.clamp(0.0, 100.0) * 0.98);
                ProgressEmitter::progress_extended(
                    &job,
                    pct,
                    &p.message,
                    Some(p.bytes_done),
                    Some(p.total_bytes),
                    None,
                );
            };
            if repo_for_run.snapshot_create_before_backup {
                ResticService::create_backup_with_snapshot(
                    &repo_for_run,
                    &snapshot_pool,
                    &snapshot_dataset,
                    &snapshot_name,
                    exclude.as_deref(),
                    tags.as_deref(),
                    Some(&job),
                    &mut emit,
                )
            } else {
                ResticService::create_backup_streaming(
                    &repo_for_run,
                    &repo_for_run.source_paths,
                    exclude.as_deref(),
                    tags.as_deref(),
                    Some(&job),
                    &mut emit,
                )
            }
        })
        .await;

        let repo_repo = ResticRepository::new(task_pool);
        let outcome: Result<_, String> = match joined {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(ResticError::Cancelled)) => Err("__cancelled__".to_string()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(join_err) => Err(format!("backup task panicked: {join_err}")),
        };

        match outcome {
            Ok(result) => {
                if let Some(id) = db_job_id.as_deref() {
                    let _ = repo_repo
                        .update_backup_job(
                            id,
                            "completed",
                            Some(&result.snapshot_id),
                            result.files_new,
                            result.files_changed,
                            result.files_unmodified,
                            result.data_added_bytes,
                            result.total_files_processed,
                            result.total_bytes,
                            result.duration_secs,
                            None,
                        )
                        .await;
                }
                ProgressEmitter::complete(&task_job_id, "Restic backup completed successfully");
                crate::events::log_event(format!(
                    "Backup completed for repository '{}' ({} files, {} bytes added)",
                    task_repo.name, result.total_files_processed, result.data_added_bytes
                ));
            }
            Err(msg) if msg == "__cancelled__" => {
                if let Some(id) = db_job_id.as_deref() {
                    let _ = repo_repo
                        .update_backup_job(
                            id,
                            "cancelled",
                            None,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            Some("Cancelled by admin"),
                        )
                        .await;
                }
                ProgressEmitter::cancelled(&task_job_id, "Backup cancelled");
                crate::events::log_event_warn(format!(
                    "Backup cancelled for repository '{}'",
                    task_repo.name
                ));
            }
            Err(msg) => {
                if let Some(id) = db_job_id.as_deref() {
                    let _ = repo_repo
                        .update_backup_job(id, "failed", None, 0, 0, 0, 0, 0, 0, 0, Some(&msg))
                        .await;
                }
                ProgressEmitter::fail(&task_job_id, &msg);
                crate::events::log_event_warn(format!(
                    "Backup failed for repository '{}': {}",
                    task_repo.name, msg
                ));
            }
        }
        ProgressEmitter::clear_cancel_hook(&task_job_id);
    });

    ResponseHelper::json_success(serde_json::json!({
        "status": "running",
        "job_id": job_id,
        "backup_job_id": backup_job_id,
        "repository": repo_config.name,
    }))
}

/// `POST /api/restic/restore` — starts a restore and answers immediately.
///
/// Same reason as the backup handler: a restore writes for as long as the
/// snapshot is big, and the frontend already treats the response's `job_id`
/// as a tracked background job.
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

    // `snapshot_id` and `target` reach restic's argv: validate them before
    // anything is spawned (`--target` as a snapshot id would otherwise be
    // re-parsed as an option).
    if let Err(e) = ResticService::validate_snapshot_id(&req.snapshot_id) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_restore_target(&req.target) {
        return ResponseHelper::json_error(e.to_string());
    }
    if let Err(e) = ResticService::validate_exclude_patterns(req.exclude_patterns.as_deref()) {
        return ResponseHelper::json_error(e.to_string());
    }

    let Some(run_lock) = RepoRunLock::acquire(&repo_config.id) else {
        return ResponseHelper::json_error(
            "A backup or restore is already running on this repository".to_string(),
        );
    };

    if !ResticService::is_available() {
        return ResponseHelper::json_error(
            "restic CLI is not installed or not found in PATH".to_string(),
        );
    }

    let job_id = ProgressEmitter::restore(&req.snapshot_id, &req.target);
    ProgressEmitter::progress(&job_id, 5.0, "Starting restore...");

    let cancel_id = job_id.clone();
    ProgressEmitter::register_cancel_hook(
        &job_id,
        Box::new(move || {
            if !ResticService::cancel_run(&cancel_id) {
                tracing::warn!(
                    "Restore cancellation requested for '{}', which had not started its restic process yet",
                    cancel_id
                );
            }
        }),
    );

    crate::events::log_event_warn(format!(
        "Admin '{}' started a restore from repository '{}' (snapshot: {}) to '{}'",
        admin_username, repo_config.name, req.snapshot_id, req.target
    ));

    let task_job_id = job_id.clone();
    let task_repo = repo_config.clone();
    let snapshot_id = req.snapshot_id.clone();
    let target = req.target.clone();
    let exclude = req.exclude_patterns.clone();
    let include = req.include_paths.clone();
    tokio::spawn(async move {
        let _run_lock = run_lock;
        let job = task_job_id.clone();
        let repo_for_run = task_repo.clone();
        let snap = snapshot_id.clone();
        let dest = target.clone();

        let joined = tokio::task::spawn_blocking(move || {
            ResticService::restore_snapshot(
                &repo_for_run,
                &snap,
                &dest,
                exclude.as_deref(),
                include.as_deref(),
                Some(&job),
            )
        })
        .await;

        let outcome: Result<(), String> = match joined {
            Ok(Ok(())) => Ok(()),
            Ok(Err(ResticError::Cancelled)) => Err("__cancelled__".to_string()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(join_err) => Err(format!("restore task panicked: {join_err}")),
        };

        match outcome {
            Ok(()) => {
                ProgressEmitter::complete(&task_job_id, "Restore completed successfully");
                crate::events::log_event(format!(
                    "Restore completed for repository '{}' to '{}'",
                    task_repo.name, target
                ));
            }
            Err(msg) if msg == "__cancelled__" => {
                ProgressEmitter::cancelled(&task_job_id, "Restore cancelled");
                crate::events::log_event_warn(format!(
                    "Restore cancelled for repository '{}'",
                    task_repo.name
                ));
            }
            Err(msg) => {
                ProgressEmitter::fail(&task_job_id, &msg);
                crate::events::log_event_warn(format!(
                    "Restore failed for repository '{}': {}",
                    task_repo.name, msg
                ));
            }
        }
        ProgressEmitter::clear_cancel_hook(&task_job_id);
    });

    ResponseHelper::json_success(serde_json::json!({
        "status": "running",
        "job_id": job_id,
        "repository": repo_config.name,
        "snapshot_id": req.snapshot_id,
        "target": req.target,
    }))
}

/// `POST /api/restic/check` — integrity check, synchronous.
///
/// `--read-data` re-reads every pack file, which is hours of I/O on a real
/// repository, so it requires **admin**: an ordinary session could otherwise
/// saturate the backup disk on demand. The light check (metadata only) stays
/// available to any authenticated user. The call itself runs on the blocking
/// pool and is cancellable through the progress job.
pub async fn handle_restic_check(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(req): axum::extract::Json<CheckRequest>,
) -> Response {
    let read_data = req.read_data.unwrap_or(false);
    let auth = if read_data {
        require_admin(&state, &headers).await.map(|_| ())
    } else {
        require_auth(&state, &headers).await.map(|_| ())
    };
    if let Err(response) = auth {
        return response;
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&req.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    let job_id = ProgressEmitter::verify(&repo_config.name);
    ProgressEmitter::progress(
        &job_id,
        10.0,
        if read_data {
            "Checking repository integrity (reading every pack)..."
        } else {
            "Checking repository integrity..."
        },
    );

    let cancel_id = job_id.clone();
    ProgressEmitter::register_cancel_hook(
        &job_id,
        Box::new(move || {
            if !ResticService::cancel_run(&cancel_id) {
                tracing::warn!(
                    "Check cancellation requested for '{}', which had not started its restic process yet",
                    cancel_id
                );
            }
        }),
    );

    let cfg = repo_config.clone();
    let check_job = job_id.clone();
    let result = ResticService::run_restic_op("restic check", None, move || {
        ResticService::check_repo(&cfg, read_data, Some(&check_job))
    })
    .await;
    ProgressEmitter::clear_cancel_hook(&job_id);

    match result {
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
        Err(ResticError::Cancelled) => {
            ProgressEmitter::cancelled(&job_id, "Check cancelled");
            ResponseHelper::json_error("Check cancelled".to_string())
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

    let cfg = repo_config.clone();
    match ResticService::run_restic_op("restic stats", Some(RESTIC_QUERY_TIMEOUT), move || {
        ResticService::get_stats(&cfg)
    })
    .await
    {
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

    // Refused before restic is spawned: a call with no rule at all used to
    // reach restic and come back as "Fatal: no policy was specified".
    if let Some(ref w) = req.keep_within {
        if let Err(e) = ResticService::validate_keep_within(w) {
            return ResponseHelper::json_error(e.to_string());
        }
    }
    let has_rule = req.keep_last.is_some()
        || req.keep_hourly.is_some()
        || req.keep_daily.is_some()
        || req.keep_weekly.is_some()
        || req.keep_monthly.is_some()
        || req.keep_yearly.is_some()
        || req.keep_within.is_some();
    if !has_rule {
        return ResponseHelper::json_error(
            "At least one retention rule (keep-last, keep-daily, keep-within, …) is required"
                .to_string(),
        );
    }

    let cfg = repo_config.clone();
    let result = ResticService::run_restic_op("restic forget", None, move || {
        ResticService::forget_snapshots(
            &cfg,
            req.keep_last,
            req.keep_hourly,
            req.keep_daily,
            req.keep_weekly,
            req.keep_monthly,
            req.keep_yearly,
            req.keep_within,
            req.prune.unwrap_or(true),
        )
    })
    .await;

    match result {
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

    let cfg = repo_config.clone();
    match ResticService::run_restic_op("restic prune", None, move || {
        ResticService::prune_repo(&cfg)
    })
    .await
    {
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

    let cfg = repo_config.clone();
    match ResticService::run_restic_op("restic unlock", Some(RESTIC_QUERY_TIMEOUT), move || {
        ResticService::unlock_repo(&cfg)
    })
    .await
    {
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
    // `snapshot_id` becomes a positional argument of `restic forget`; without
    // this check a value like `--keep-last=0` was parsed as an option and
    // wiped the repository instead of deleting one snapshot.
    if let Err(e) = ResticService::validate_snapshot_id(snapshot_id) {
        return ResponseHelper::json_error(e.to_string());
    }

    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };

    let cfg = repo_config.clone();
    let snap = snapshot_id.to_string();
    match ResticService::run_restic_op("restic forget", Some(RESTIC_QUERY_TIMEOUT), move || {
        ResticService::delete_snapshot(&cfg, &snap)
    })
    .await
    {
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
    // Bounded: `df` on a target whose mount is gone (an unmounted array, a
    // dead NFS export) blocks — and this decides whether a restore has room
    // (ROADMAP X.9).
    let mut cmd = std::process::Command::new("df");
    cmd.args(["-B1", "--output=avail", target]);
    crate::utils::process::run_timed(&mut cmd, std::time::Duration::from_secs(10))
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
    if let Err(e) = ResticService::validate_restore_target(&params.target) {
        return ResponseHelper::json_error(e.to_string());
    }
    let repo_repo = ResticRepository::new(state.db.pool());
    let repo_config = match repo_repo.get_by_id(&params.repo_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return ResponseHelper::json_error("Repository not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Database error: {}", e)),
    };
    let cfg = repo_config.clone();
    let snap = params.snapshot_id.clone();
    let required =
        match ResticService::run_restic_op("restic stats", Some(RESTIC_QUERY_TIMEOUT), move || {
            ResticService::restore_size(&cfg, &snap)
        })
        .await
        {
            Ok(n) => n,
            Err(e) => return ResponseHelper::json_error(format!("Failed to size snapshot: {}", e)),
        };
    let target = params.target.clone();
    let available = tokio::task::spawn_blocking(move || target_available_bytes(&target))
        .await
        .unwrap_or(None);
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

/// A retention limit is a positive integer: `max_count = 0` would mean
/// "forget every snapshot" and a negative age is meaningless.
///
/// `require_a_limit` is true on create (a policy with no limit at all would
/// silently never prune) and false on update (a partial update that only
/// renames the policy must stay valid).
fn validate_retention_limits(
    max_count: Option<i64>,
    max_age_days: Option<i64>,
    max_size_gb: Option<i64>,
    require_a_limit: bool,
) -> Result<(), String> {
    for (label, value) in [
        ("max_count", max_count),
        ("max_age_days", max_age_days),
        ("max_size_gb", max_size_gb),
    ] {
        if let Some(v) = value {
            if v <= 0 {
                return Err(format!("{label} must be greater than 0"));
            }
        }
    }
    if require_a_limit && max_count.is_none() && max_age_days.is_none() && max_size_gb.is_none() {
        return Err(
            "At least one retention limit (snapshot count, age or size) is required".to_string(),
        );
    }
    Ok(())
}

/// Only one policy may be the `apply_to_all` default: the scheduler resolves
/// it with `get_default`, which returns the first match by name — with two
/// flagged policies which one wins was arbitrary.
async fn ensure_single_default_policy(
    repo: &RetentionPolicyRepository,
    exclude_id: Option<&str>,
) -> Result<(), String> {
    let policies = repo
        .list_all()
        .await
        .map_err(|e| format!("Database error: {}", e))?;
    let clash = policies
        .into_iter()
        .find(|p| p.apply_to_all && Some(p.id.as_str()) != exclude_id);
    match clash {
        Some(existing) => Err(format!(
            "Retention policy '{}' is already the default for every repository",
            existing.name
        )),
        None => Ok(()),
    }
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
    if let Err(e) =
        validate_retention_limits(req.max_count, req.max_age_days, req.max_size_gb, true)
    {
        return ResponseHelper::json_error(e);
    }
    if let Err(e) = ResticService::validate_display_name("Policy name", &req.name) {
        return ResponseHelper::json_error(e.to_string());
    }

    let repo = RetentionPolicyRepository::new(state.db.pool());
    if req.apply_to_all.unwrap_or(false) {
        if let Err(e) = ensure_single_default_policy(&repo, None).await {
            return ResponseHelper::json_error(e);
        }
    }
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

    if let Err(e) =
        validate_retention_limits(req.max_count, req.max_age_days, req.max_size_gb, false)
    {
        return ResponseHelper::json_error(e);
    }
    if let Some(name) = req.name.as_deref() {
        if let Err(e) = ResticService::validate_display_name("Policy name", name) {
            return ResponseHelper::json_error(e.to_string());
        }
    }

    let repo = RetentionPolicyRepository::new(state.db.pool());
    if req.apply_to_all == Some(true) {
        if let Err(e) = ensure_single_default_policy(&repo, Some(&id)).await {
            return ResponseHelper::json_error(e);
        }
    }
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

    // -- retention-policy limits -----------------------------------------

    #[test]
    fn test_retention_limits_accept_positive_values() {
        assert!(validate_retention_limits(Some(30), None, None, true).is_ok());
        assert!(validate_retention_limits(None, Some(90), None, true).is_ok());
        assert!(validate_retention_limits(None, None, Some(500), true).is_ok());
        assert!(validate_retention_limits(Some(7), Some(30), Some(100), true).is_ok());
    }

    #[test]
    fn test_retention_limits_refuse_zero_and_negative() {
        // `max_count = 0` would mean "forget every snapshot".
        assert!(validate_retention_limits(Some(0), None, None, true).is_err());
        assert!(validate_retention_limits(None, Some(0), None, true).is_err());
        assert!(validate_retention_limits(None, None, Some(-5), true).is_err());
        assert!(validate_retention_limits(Some(-1), None, None, true).is_err());
    }

    #[test]
    fn test_retention_limits_require_a_limit_on_create_only() {
        // Create with no limit at all: refused (it would never prune).
        let err = validate_retention_limits(None, None, None, true).unwrap_err();
        assert!(err.contains("At least one retention limit"));
        // A partial update that only renames the policy stays valid.
        assert!(validate_retention_limits(None, None, None, false).is_ok());
    }

    #[test]
    fn test_days_since_counts_whole_days() {
        // 2026-01-01T00:00:00Z vs 2026-01-08T12:00:00Z -> 7 full days.
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-08T12:00:00Z")
            .unwrap()
            .timestamp();
        assert_eq!(days_since(Some("2026-01-01T00:00:00Z"), now), Some(7));
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
        assert_eq!(parse_df_available("Avail\n123456789\n"), Some(123456789));
        assert_eq!(parse_df_available("Avail\n"), None);
        assert_eq!(parse_df_available(""), None);
        assert_eq!(parse_df_available("Avail\nnot-a-number\n"), None);
    }
}
