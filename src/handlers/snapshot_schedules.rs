/* =============================================================================
 * File          : snapshot_schedules.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled-snapshot endpoints: CRUD for pool snapshot schedules (cron +
 *   GFS retention), run-now, and a pool snapshot listing for the UI. Every
 *   mutation is admin-only — snapshots are created and *destroyed* here; the
 *   listing is a read and only needs a session (`require_auth`).
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/snapshot-schedules in src/web/mod.rs; loop in
 *   snapshot_scheduler.rs; storage in snapshot_schedule_repo.rs.
 * =============================================================================
 */

//! Snapshot schedule CRUD + run-now (admin) + pool snapshot list (auth).

use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::pools::PoolService;
use crate::restic_scheduler::compute_next_run;
use crate::scheduled_tasks_scheduler::is_valid_pool_name;
use crate::snapshot_scheduler::{
    parse_auto_snapshot_time, run_schedule, GfsPolicy, RunScheduleError,
};
use crate::storage::snapshot_schedule_repo::SnapshotScheduleRepository;
use crate::utils::ResponseHelper;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
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
    if let Err(message) = validate_known_pool(req.pool.trim(), &known_pool_names().await) {
        return ResponseHelper::json_error(message);
    }
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
        Err(e) if is_prefix_conflict(&e) => ResponseHelper::json_error_with_status(
            StatusCode::CONFLICT,
            format!(
                "A schedule for pool '{}' with prefix '{}' already exists",
                req.pool.trim(),
                req.prefix.trim()
            ),
        ),
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
    if let Err(message) = validate_known_pool(req.pool.trim(), &known_pool_names().await) {
        return ResponseHelper::json_error(message);
    }
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
        Err(e) if is_prefix_conflict(&e) => ResponseHelper::json_error_with_status(
            StatusCode::CONFLICT,
            format!(
                "A schedule for pool '{}' with prefix '{}' already exists",
                req.pool.trim(),
                req.prefix.trim()
            ),
        ),
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
        // The scheduler tick (or another "Run now") already owns this
        // schedule or its pool: nothing was attempted, so say so instead of
        // pretending the run happened.
        Err(RunScheduleError::Busy(message)) => {
            ResponseHelper::json_error_with_status(StatusCode::CONFLICT, message)
        }
        Err(RunScheduleError::Failed(message)) => {
            ResponseHelper::json_error(format!("Schedule run failed: {message}"))
        }
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
    // A read, like every other Storage read (`require_auth`) — the schedule
    // mutations below stay admin-only.
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }
    if !is_valid_pool_name(&params.pool) {
        return ResponseHelper::json_error(format!("Invalid pool name '{}'", params.pool));
    }
    let pool = params.pool.clone();

    // The prefixes this pool's schedules own: only their snapshots are
    // auto-managed, so the `auto` badge needs the configured prefixes.
    let repo = SnapshotScheduleRepository::new(state.db.pool());
    let prefixes: Vec<String> = repo
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.pool == pool)
        .map(|s| s.prefix)
        .collect();

    match tokio::task::spawn_blocking(move || {
        crate::snapshots::SnapshotService::list_snapshots(&pool)
    })
    .await
    {
        Ok(Ok(snapshots)) => {
            // Tag auto snapshots (those a schedule's prefix owns) so the UI
            // can mark what the schedules manage.
            let enriched: Vec<serde_json::Value> = snapshots
                .iter()
                .map(|s| {
                    let bare = s.name.rsplit('@').next().unwrap_or(&s.name);
                    let auto = is_auto_snapshot(bare, &prefixes);
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

/// Tags a snapshot as auto-managed when its bare name parses as
/// `{prefix}-YYYYMMDD-HHMMSS` for **one of the schedules that target this
/// pool**. The prefix used to be ignored (the name was split on its first
/// `-`), so a manual snapshot called `mydata-20261021-030000` was badged
/// `auto` and looked managed by a schedule.
fn is_auto_snapshot(bare_name: &str, prefixes: &[String]) -> bool {
    prefixes
        .iter()
        .any(|prefix| parse_auto_snapshot_time(bare_name, prefix).is_some())
}

/// A schedule on a pool the host doesn't have can only fail on every tick,
/// so it is refused up front. Skipped when the scan reports no pool at all
/// (neither ZFS nor Btrfs installed, or a fresh install): the check would
/// reject everything while the run itself reports a clear error.
fn validate_known_pool(pool: &str, known: &[String]) -> Result<(), String> {
    if known.is_empty() || known.iter().any(|p| p == pool) {
        Ok(())
    } else {
        Err(format!(
            "Pool '{pool}' was not found on this host — known pools: {}",
            known.join(", ")
        ))
    }
}

/// The (pool, prefix) pair is unique (migration 053): two schedules sharing
/// it would prune each other's snapshots.
fn is_prefix_conflict(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if is_prefix_conflict_message(db.message()))
}

fn is_prefix_conflict_message(message: &str) -> bool {
    message.contains("UNIQUE constraint failed: snapshot_schedules.pool")
}

/// Pool names the host currently reports, for the existence check. A scan
/// failure is not fatal here — the run itself reports the real error.
async fn known_pool_names() -> Vec<String> {
    match tokio::task::spawn_blocking(PoolService::scan_pools).await {
        Ok(Ok(pools)) => pools.into_iter().map(|p| p.name).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_badge_requires_one_of_the_pools_own_prefixes() {
        let prefixes = vec!["hv-auto".to_string(), "weekly".to_string()];
        assert!(is_auto_snapshot("hv-auto-20261021-030000", &prefixes));
        assert!(is_auto_snapshot("weekly-20261021-030000", &prefixes));
        // A manual snapshot that merely looks like a timestamped name is not
        // schedule-managed (it used to be badged `auto`).
        assert!(!is_auto_snapshot("mydata-20261021-030000", &prefixes));
        // No schedule on this pool → nothing is auto-managed.
        assert!(!is_auto_snapshot("hv-auto-20261021-030000", &[]));
    }

    #[test]
    fn pool_existence_is_checked_only_when_pools_are_reported() {
        let known = vec!["tank".to_string(), "data".to_string()];
        assert!(validate_known_pool("tank", &known).is_ok());
        let err = validate_known_pool("tanl", &known).expect_err("a typo must be refused");
        assert!(err.contains("tanl") && err.contains("tank"), "got {err}");
        // Nothing to compare against: don't block on an unverifiable name.
        assert!(validate_known_pool("tanl", &[]).is_ok());
    }

    #[test]
    fn a_duplicate_pool_prefix_pair_is_recognised() {
        assert!(is_prefix_conflict_message(
            "UNIQUE constraint failed: snapshot_schedules.pool, snapshot_schedules.prefix"
        ));
        assert!(!is_prefix_conflict_message("database is locked"));
    }
}
