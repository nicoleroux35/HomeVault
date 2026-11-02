/* =============================================================================
 * File          : replication.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Snapshot-replication endpoints: list the jobs, create/update/delete one,
 *   preview its pipeline (dry run), run it now and read its last outcome.
 *   Reads need a session; every mutation is admin-only.
 *
 * Dependencies:
 *   - axum (>= 0.8), crate::replication, crate::replication_scheduler
 *
 * Usage:
 *   Routes under /api/replication in src/web/mod.rs; see
 *   docs/guides/replication.md.
 * =============================================================================
 */

//! Snapshot-replication endpoints (admin-only mutations), see
//! docs/guides/replication.md.
//!
//! * `GET    /api/replication`               — jobs + capability evidence
//! * `POST   /api/replication`               — create a job
//! * `PUT    /api/replication/{id}`          — update a job
//! * `DELETE /api/replication/{id}`          — delete a job
//! * `GET    /api/replication/{id}/preview`  — dry run (the exact pipeline)
//! * `POST   /api/replication/{id}/run`      — run now

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::replication::{self, DestinationKind, ReplicationBackend, ReplicationJob};
use crate::replication_scheduler;
use crate::storage::ReplicationRepository;
use crate::utils::ResponseHelper;

use super::AppState;

/// The body of a create/update request. Every field is supplied by the UI;
/// validation lives in [`crate::replication::validate_job`].
#[derive(Debug, Deserialize)]
pub struct ReplicationJobRequest {
    pub name: String,
    pub source_dataset: String,
    pub backend: String,
    pub destination_kind: String,
    pub destination_dataset: String,
    #[serde(default)]
    pub destination_host: Option<String>,
    #[serde(default)]
    pub destination_user: Option<String>,
    #[serde(default)]
    pub destination_port: Option<i64>,
    #[serde(default)]
    pub interval_minutes: Option<i64>,
    #[serde(default)]
    pub keep_last: Option<i64>,
    #[serde(default)]
    pub bandwidth_kbps: Option<i64>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn job_from_request(id: i64, body: &ReplicationJobRequest, created_by: &str) -> ReplicationJob {
    let now = Utc::now();
    let mut job = ReplicationJob {
        id,
        name: body.name.trim().to_string(),
        source_dataset: body.source_dataset.trim().to_string(),
        backend: body.backend.trim().to_lowercase(),
        destination_kind: body.destination_kind.trim().to_lowercase(),
        destination_dataset: body.destination_dataset.trim().to_string(),
        destination_host: body
            .destination_host
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        destination_user: body
            .destination_user
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        destination_port: body.destination_port.unwrap_or(22),
        interval_minutes: body.interval_minutes.unwrap_or(1440),
        keep_last: body.keep_last.unwrap_or(7),
        bandwidth_kbps: body.bandwidth_kbps.unwrap_or(0),
        enabled: i64::from(body.enabled.unwrap_or(true)),
        next_run_at: None,
        last_run_at: None,
        last_snapshot: None,
        last_status: None,
        last_error: None,
        created_by: created_by.to_string(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };
    job.next_run_at = replication_scheduler::next_run_at(&job, now).map(|at| at.to_rfc3339());
    job
}

fn job_json(job: &ReplicationJob) -> serde_json::Value {
    serde_json::json!({
        "id": job.id,
        "name": job.name,
        "source_dataset": job.source_dataset,
        "backend": job.backend,
        "destination_kind": job.destination_kind,
        "destination_dataset": job.destination_dataset,
        "destination_host": job.destination_host,
        "destination_user": job.destination_user,
        "destination_port": job.destination_port,
        "interval_minutes": job.interval_minutes,
        "keep_last": job.keep_last,
        "bandwidth_kbps": job.bandwidth_kbps,
        "enabled": job.enabled != 0,
        "next_run_at": job.next_run_at,
        "last_run_at": job.last_run_at,
        "last_snapshot": job.last_snapshot,
        "last_status": job.last_status,
        "last_error": job.last_error,
        "running": replication_scheduler::is_running(job.id),
        "bandwidth_note": replication::bandwidth_note(job),
    })
}

fn capability_json() -> serde_json::Value {
    serde_json::json!({
        "zfs": crate::dependencies::binary_available("zfs", "/usr/sbin/zfs"),
        "btrfs": crate::dependencies::binary_available("btrfs", "/usr/bin/btrfs"),
        "ssh": crate::dependencies::binary_available("ssh", "/usr/bin/ssh"),
        "pv": replication::pv_available(),
        "max_keep_last": replication::MAX_KEEP_LAST,
        "max_bandwidth_kbps": replication::MAX_BANDWIDTH_KBPS,
        "min_interval_minutes": replication::MIN_INTERVAL_MINUTES,
        "max_interval_minutes": replication::MAX_INTERVAL_MINUTES,
        "prefix": replication::REPLICATION_PREFIX,
    })
}

/// `GET /api/replication`
pub async fn handle_replication_jobs_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let repo = ReplicationRepository::new(state.db.pool());
    match repo.list().await {
        Ok(jobs) => ResponseHelper::json_success(serde_json::json!({
            "jobs": jobs.iter().map(job_json).collect::<Vec<_>>(),
            "capabilities": capability_json(),
        })),
        Err(e) => ResponseHelper::json_error(format!("Failed to list the jobs: {e}")),
    }
}

/// `POST /api/replication`
pub async fn handle_replication_job_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ReplicationJobRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let job = job_from_request(0, &body, &admin);
    if let Err(e) = replication::validate_job(&job) {
        return ResponseHelper::json_error(e);
    }
    let repo = ReplicationRepository::new(state.db.pool());
    match repo.name_taken(&job.name, None).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "A replication job named '{}' already exists",
                job.name
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }
    match repo.create(&job).await {
        Ok(saved) => {
            crate::events::log_event(format!(
                "Admin '{}' created replication job '{}' ({} → {})",
                admin, saved.name, saved.source_dataset, saved.destination_dataset
            ));
            ResponseHelper::json_success(serde_json::json!({ "job": job_json(&saved) }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the job: {e}")),
    }
}

/// `PUT /api/replication/{id}`
pub async fn handle_replication_job_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<ReplicationJobRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = ReplicationRepository::new(state.db.pool());
    let existing = match repo.find(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return ResponseHelper::json_error("That job no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the job: {e}")),
    };
    let mut job = job_from_request(id, &body, &existing.created_by);
    // Keep the run history the update does not own.
    job.last_run_at = existing.last_run_at.clone();
    job.last_snapshot = existing.last_snapshot.clone();
    job.last_status = existing.last_status.clone();
    job.last_error = existing.last_error.clone();
    job.created_at = existing.created_at.clone();
    if let Err(e) = replication::validate_job(&job) {
        return ResponseHelper::json_error(e);
    }
    match repo.name_taken(&job.name, Some(id)).await {
        Ok(true) => {
            return ResponseHelper::json_error(format!(
                "A replication job named '{}' already exists",
                job.name
            ))
        }
        Ok(false) => {}
        Err(e) => return ResponseHelper::json_error(format!("Failed to check the name: {e}")),
    }
    match repo.update(&job).await {
        Ok(saved) => {
            crate::events::log_event(format!(
                "Admin '{}' updated replication job '{}'",
                admin, saved.name
            ));
            ResponseHelper::json_success(serde_json::json!({ "job": job_json(&saved) }))
        }
        Err(crate::storage::DbError::NotFound) => {
            ResponseHelper::json_error("That job no longer exists".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save the job: {e}")),
    }
}

/// `DELETE /api/replication/{id}`
pub async fn handle_replication_job_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = ReplicationRepository::new(state.db.pool());
    let existing = repo.find(id).await.ok().flatten();
    if replication_scheduler::is_running(id) {
        return ResponseHelper::json_error(
            "This job is running — wait for the transfer to finish".to_string(),
        );
    }
    match repo.delete(id).await {
        Ok(()) => {
            if let Some(job) = existing {
                crate::events::log_event(format!(
                    "Admin '{}' deleted replication job '{}' (destination snapshots are kept)",
                    admin, job.name
                ));
            }
            ResponseHelper::json_success(serde_json::json!({ "deleted": true }))
        }
        Err(crate::storage::DbError::NotFound) => {
            ResponseHelper::json_error("That job no longer exists".to_string())
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to delete the job: {e}")),
    }
}

/// `GET /api/replication/{id}/preview`
///
/// A dry run: the exact pipeline that would run, with no snapshot created and
/// nothing sent. This is what makes "dry-run first" a real safety feature
/// rather than a promise in the documentation.
pub async fn handle_replication_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    let repo = ReplicationRepository::new(state.db.pool());
    let job = match repo.find(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return ResponseHelper::json_error("That job no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the job: {e}")),
    };
    match replication_scheduler::preview_job(&job).await {
        Ok(plan) => ResponseHelper::json_success(serde_json::json!({
            "snapshot": plan.snapshot,
            "base": plan.base,
            "command": plan.describe(),
            "bandwidth_note": replication::bandwidth_note(&job),
            "resumable": true,
        })),
        Err(e) => ResponseHelper::json_error(format!("The job cannot run: {e}")),
    }
}

/// `POST /api/replication/{id}/run`
pub async fn handle_replication_run_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(username) => username,
        Err(resp) => return resp,
    };
    let repo = ReplicationRepository::new(state.db.pool());
    let job = match repo.find(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return ResponseHelper::json_error("That job no longer exists".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to read the job: {e}")),
    };
    if matches!(job.backend_kind(), Some(ReplicationBackend::Zfs))
        && !crate::dependencies::binary_available("zfs", "/usr/sbin/zfs")
    {
        return ResponseHelper::json_error(
            "This host has no ZFS, so a ZFS job cannot run".to_string(),
        );
    }
    if matches!(job.destination(), Some(DestinationKind::Ssh))
        && !crate::dependencies::binary_available("ssh", "/usr/bin/ssh")
    {
        return ResponseHelper::json_error(
            "The ssh client is not installed, so an SSH destination cannot be reached".to_string(),
        );
    }
    crate::events::log_event(format!(
        "Admin '{}' started replication job '{}'",
        admin, job.name
    ));
    match replication_scheduler::run_job(&state, &job).await {
        Ok(outcome) => ResponseHelper::json_success(serde_json::json!({
            "snapshot": outcome.snapshot,
            "message": outcome.message,
        })),
        Err(e) => ResponseHelper::json_error(format!("The replication failed: {e}")),
    }
}
