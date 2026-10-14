/* =============================================================================
 * File          : snapshot_schedules.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled-snapshot endpoints: CRUD for pool snapshot schedules (cron +
 *   GFS retention), run-now, and a pool snapshot listing for the UI. Every
 *   endpoint is admin-only — snapshots are created and *destroyed* here.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/snapshot-schedules in src/web/mod.rs; loop in
 *   snapshot_scheduler.rs; storage in snapshot_schedule_repo.rs.
 * =============================================================================
 */

//! Snapshot schedule CRUD + run-now + pool snapshot list (all admin-only).

use crate::handlers::admin_guard::require_admin;
use crate::handlers::AppState;
use crate::restic_scheduler::compute_next_run;
use crate::scheduled_tasks_scheduler::is_valid_pool_name;
use crate::snapshot_scheduler::{run_schedule, GfsPolicy, SNAPSHOT_TIME_FORMAT};
use crate::storage::snapshot_schedule_repo::SnapshotScheduleRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

/// Body of POST/PUT — mirrors the schedule row's editable fields.
#[derive(Debug, Deserialize)]
pub struct SnapshotScheduleRequest {
    pub pool: String,
    pub cron: String,
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_true")]
    pub recursive: bool,
    #[serde(default = "default_hourly")]
    pub gfs_hourly: u32,
    #[serde(default = "default_daily")]
    pub gfs_daily: u32,
    #[serde(default = "default_weekly")]
    pub gfs_weekly: u32,
    #[serde(default = "default_monthly")]
    pub gfs_monthly: u32,
    #[serde(default = "default_yearly")]
    pub gfs_yearly: u32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_prefix() -> String {
    "hv-auto".to_string()
}
fn default_true() -> bool {
    true
}
fn default_hourly() -> u32 {
    24
}
fn default_daily() -> u32 {
    7
}
fn default_weekly() -> u32 {
    4
}
fn default_monthly() -> u32 {
    12
}
fn default_yearly() -> u32 {
    1
}

impl SnapshotScheduleRequest {
    fn policy(&self) -> GfsPolicy {
        GfsPolicy {
            hourly: self.gfs_hourly,
            daily: self.gfs_daily,
            weekly: self.gfs_weekly,
            monthly: self.gfs_monthly,
            yearly: self.gfs_yearly,
        }
    }

    // `Response` as Err mirrors the guard helpers' shape (see the
    // `#[allow]` note on `require_admin` for why it isn't boxed).
    #[allow(clippy::result_large_err)]
    fn validate(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>, Response> {
        if !is_valid_pool_name(&self.pool) {
            return Err(ResponseHelper::json_error(format!(
                "Invalid pool name '{}'",
                self.pool
            )));
        }
        // The prefix becomes part of a snapshot name (and the prune filter)
        // — keep it conservative: alphanumerics, dashes, underscores.
        let prefix = self.prefix.trim();
        if prefix.is_empty() || prefix.len() > 32 {
            return Err(ResponseHelper::json_error(
                "Prefix must be 1-32 characters".to_string(),
            ));
        }
        if !prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ResponseHelper::json_error(
                "Prefix may only contain letters, digits, dashes and underscores".to_string(),
            ));
        }
        // A prefix that could swallow another schedule's snapshots (or be
        // swallowed by one) would make pruning delete across schedules.
        for conflict in ["-", "hv-auto"] {
            if prefix == conflict && prefix != "hv-auto" {
                return Err(ResponseHelper::json_error(format!(
                    "Prefix '{conflict}' is reserved"
                )));
            }
        }
        let next = compute_next_run(self.cron.trim(), chrono::Utc::now())
            .map_err(|e| ResponseHelper::json_error(format!("Invalid cron expression: {e}")))?;
        Ok(next)
    }
}

/// Lists every snapshot schedule.
pub async fn handle_snapshot_schedules_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    match repo.list().await {
        Ok(schedules) => {
            ResponseHelper::json_success(serde_json::json!({ "schedules": schedules }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list schedules: {e}")),
    }
}

/// Creates a schedule.
pub async fn handle_snapshot_schedule_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<SnapshotScheduleRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let next = match req.validate() {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    match repo
        .create(
            req.pool.trim(),
            req.cron.trim(),
            req.prefix.trim(),
            req.recursive,
            req.policy(),
            next.map(|d| d.to_rfc3339()).as_deref(),
        )
        .await
    {
        Ok(schedule) => ResponseHelper::json_success(schedule),
        Err(e) => ResponseHelper::json_error(format!("Failed to create schedule: {e}")),
    }
}

/// Updates a schedule (also the enable/disable toggle path).
pub async fn handle_snapshot_schedule_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<SnapshotScheduleRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    if repo.get(&id).await.ok().flatten().is_none() {
        return ResponseHelper::json_error("Schedule not found".to_string());
    }
    let next = match req.validate() {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    match repo
        .update(
            &id,
            req.pool.trim(),
            req.cron.trim(),
            req.prefix.trim(),
            req.recursive,
            req.enabled,
            req.policy(),
            next.map(|d| d.to_rfc3339()).as_deref(),
        )
        .await
    {
        Ok(()) => match repo.get(&id).await {
            Ok(Some(schedule)) => ResponseHelper::json_success(schedule),
            _ => ResponseHelper::json_error("Schedule not found".to_string()),
        },
        Err(e) => ResponseHelper::json_error(format!("Failed to update schedule: {e}")),
    }
}

/// Deletes a schedule (snapshots it created are kept — deletion of history
/// is always explicit through the prune policy, never as a side effect).
pub async fn handle_snapshot_schedule_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(()) => ResponseHelper::json_success(serde_json::json!({ "deleted": true })),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete schedule: {e}")),
    }
}

/// Runs a schedule immediately (create + prune), independent of the cron.
pub async fn handle_snapshot_schedule_run_now(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    let schedule = match repo.get(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return ResponseHelper::json_error("Schedule not found".to_string()),
        Err(e) => return ResponseHelper::json_error(format!("Failed to load schedule: {e}")),
    };
    match run_schedule(&state, &schedule).await {
        Ok(summary) => ResponseHelper::json_success(serde_json::json!({ "summary": summary })),
        Err(e) => ResponseHelper::json_error(format!("Schedule run failed: {e}")),
    }
}

/// Lists a pool's snapshots (any prefix) so the UI can show what exists.
#[derive(Debug, Deserialize)]
pub struct PoolQuery {
    pub pool: String,
}

pub async fn handle_snapshot_pool_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<PoolQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }
    if !is_valid_pool_name(&params.pool) {
        return ResponseHelper::json_error(format!("Invalid pool name '{}'", params.pool));
    }
    let pool = params.pool.clone();
    match tokio::task::spawn_blocking(move || {
        crate::snapshots::SnapshotService::list_snapshots(&pool)
    })
    .await
    {
        Ok(Ok(snapshots)) => {
            // Tag auto snapshots (those the GFS prefix pattern owns) so the
            // UI can mark what the schedules manage.
            let enriched: Vec<serde_json::Value> = snapshots
                .iter()
                .map(|s| {
                    let bare = s.name.rsplit('@').next().unwrap_or(&s.name);
                    let auto = parse_auto(bare);
                    serde_json::json!({
                        "name": s.name,
                        "created": s.created,
                        "used": s.used,
                        "referenced": s.referenced,
                        "auto": auto,
                    })
                })
                .collect();
            ResponseHelper::json_success(serde_json::json!({ "snapshots": enriched }))
        }
        Ok(Err(e)) => ResponseHelper::json_error(format!("Failed to list snapshots: {e}")),
        Err(e) => ResponseHelper::json_error(format!("Snapshot list task failed: {e}")),
    }
}

fn parse_auto(bare_name: &str) -> bool {
    match bare_name.split_once('-') {
        Some((prefix, rest)) => {
            rest.len() == 15
                && rest.as_bytes()[8] == b'-'
                && chrono::NaiveDateTime::parse_from_str(rest, SNAPSHOT_TIME_FORMAT).is_ok()
                && !prefix.is_empty()
        }
        None => false,
    }
}
