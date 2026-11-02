/* =============================================================================
 * File          : replication_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Runs snapshot-replication jobs: create the source snapshot, push it with a
 *   resumable send/receive pipeline, prune the destination's own snapshots and
 *   record the outcome. One background loop for the schedule, plus a public
 *   entry point the API uses for "run now".
 *
 * Dependencies:
 *   - tokio (>= 1.52), crate::replication, crate::snapshots, crate::storage
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; handlers/replication.rs calls
 *   `run_job` for an immediate run; see docs/guides/replication.md.
 * =============================================================================
 */

//! Snapshot-replication runner and schedule loop (P2.4).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::handlers::AppState;
use crate::replication::{self, ReplicationBackend, ReplicationJob};
use crate::snapshots::SnapshotService;
use crate::storage::ReplicationRepository;

/// How often the schedule is polled. A job runs at most once per its own
/// interval, so a minute of slack only matters for a 5-minute job.
const POLL_INTERVAL_SECS: u64 = 60;

/// Jobs currently running in this process, so a "Run now" click cannot start a
/// second concurrent send of the same dataset (which ZFS/Btrfs would interleave
/// into a corrupt destination).
fn running() -> &'static Mutex<HashSet<i64>> {
    static RUNNING: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashSet::new()))
}

struct RunGuard(i64);

impl RunGuard {
    fn acquire(id: i64) -> Option<Self> {
        let mut set = running().lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(id) {
            return None;
        }
        Some(RunGuard(id))
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut set = running().lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&self.0);
    }
}

/// Whether a job is running right now (the UI disables "Run now").
pub fn is_running(id: i64) -> bool {
    running()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&id)
}

/// The pool a dataset belongs to (its first path component).
pub fn pool_of(dataset: &str) -> &str {
    dataset.split('/').next().unwrap_or(dataset)
}

/// The next run time of a job: `now + interval`, or `None` for a manual-only
/// job. Pure, so the arithmetic is testable.
pub fn next_run_at(job: &ReplicationJob, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if job.interval_minutes <= 0 {
        None
    } else {
        Some(now + chrono::Duration::minutes(job.interval_minutes))
    }
}

/// Outcome of one run, for the API's "run now" answer.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub snapshot: String,
    pub message: String,
}

/// Run one job to completion. A refused or failed run is recorded on the row
/// (and reported), never swallowed.
pub async fn run_job(state: &Arc<AppState>, job: &ReplicationJob) -> Result<RunOutcome, String> {
    let Some(_guard) = RunGuard::acquire(job.id) else {
        return Err("This job is already running".to_string());
    };
    let repo = ReplicationRepository::new(state.db.pool());
    match run_job_inner(job).await {
        Ok(outcome) => {
            let next = next_run_at(job, Utc::now()).map(|at| at.to_rfc3339());
            repo.record_run(
                job.id,
                true,
                &outcome.message,
                Some(&outcome.snapshot),
                next.as_deref(),
            )
            .await
            .ok();
            crate::events::log_event(format!(
                "Replication job '{}' completed: {}",
                job.name, outcome.message
            ));
            Ok(outcome)
        }
        Err(e) => {
            // A failed run keeps the old `next_run_at` — retried next tick.
            repo.record_run(job.id, false, &e, None, job.next_run_at.as_deref())
                .await
                .ok();
            crate::events::log_event(format!("Replication job '{}' failed: {}", job.name, e));
            Err(e)
        }
    }
}

/// The plan one run will execute, without running it (dry-run preview).
pub async fn preview_job(job: &ReplicationJob) -> Result<replication::SendPlan, String> {
    replication::validate_job(job)?;
    let backend = job
        .backend_kind()
        .ok_or_else(|| "unknown backend".to_string())?;
    let pool = pool_of(&job.source_dataset).to_string();
    let now = Utc::now();
    let name = replication::snapshot_name(job.id, now);
    let job_id = job.id;
    let recorded = job.last_snapshot.clone();
    let backend_for_list = backend;
    let pool_for_list = pool.clone();

    let sources = tokio::task::spawn_blocking(move || {
        let snapshot_kind = match backend_for_list {
            ReplicationBackend::Zfs => crate::snapshots::SnapshotKind::Zfs,
            ReplicationBackend::Btrfs => crate::snapshots::SnapshotKind::Btrfs,
        };
        let mut names: Vec<String> =
            SnapshotService::list_snapshots_of(snapshot_kind, &pool_for_list)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|snapshot| snapshot.name)
                .collect();
        names.sort();
        Ok::<_, String>(names)
    })
    .await
    .map_err(|e| format!("listing task panicked: {e}"))??;

    let base = replication::choose_base(job_id, recorded.as_deref(), &sources);
    let (source_path, destination_path) = paths_for(job, &pool, &name)?;
    replication::build_plan(
        job,
        &pool,
        base.as_deref(),
        &name,
        source_path.as_deref(),
        destination_path.as_deref(),
        // A preview describes the *intended* pipeline; it says separately when
        // the throttle cannot be applied.
        true,
    )
}

/// The filesystem paths a Btrfs plan needs (`None` for ZFS).
fn paths_for(
    job: &ReplicationJob,
    pool: &str,
    snapshot_name: &str,
) -> Result<(Option<String>, Option<String>), String> {
    match job.backend_kind() {
        Some(ReplicationBackend::Btrfs) => Ok((
            Some(replication::btrfs_source_snapshot_path(
                pool,
                snapshot_name,
            )?),
            replication::btrfs_destination_path(job),
        )),
        _ => Ok((None, None)),
    }
}

async fn run_job_inner(job: &ReplicationJob) -> Result<RunOutcome, String> {
    replication::validate_job(job)?;
    let backend = job
        .backend_kind()
        .ok_or_else(|| "unknown backend".to_string())?;
    let Some(kind) = job.destination() else {
        return Err("unknown destination kind".to_string());
    };
    let pool = pool_of(&job.source_dataset).to_string();

    // The job's declared backend must match what the pool actually is: a ZFS
    // job pointed at a Btrfs pool would otherwise run `zfs send` on a name
    // that is a mount path.
    let resolved = tokio::task::spawn_blocking({
        let pool = pool.clone();
        move || SnapshotService::kind_for(&pool).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("pool lookup panicked: {e}"))??;
    let matches = matches!(
        (resolved, backend),
        (crate::snapshots::SnapshotKind::Zfs, ReplicationBackend::Zfs)
            | (
                crate::snapshots::SnapshotKind::Btrfs,
                ReplicationBackend::Btrfs
            )
    );
    if !matches {
        return Err(format!(
            "job '{}' is declared {} but pool '{}' is {}",
            job.name,
            backend.as_str(),
            pool,
            match resolved {
                crate::snapshots::SnapshotKind::Zfs => "ZFS",
                crate::snapshots::SnapshotKind::Btrfs => "Btrfs",
            }
        ));
    }

    let now = Utc::now();
    let snapshot = replication::snapshot_name(job.id, now);

    // 1. List the source snapshots and pick the increment base.
    let recorded = job.last_snapshot.clone();
    let job_id = job.id;
    let source_list = tokio::task::spawn_blocking({
        let pool = pool.clone();
        move || {
            let snapshot_kind = match backend {
                ReplicationBackend::Zfs => crate::snapshots::SnapshotKind::Zfs,
                ReplicationBackend::Btrfs => crate::snapshots::SnapshotKind::Btrfs,
            };
            SnapshotService::list_snapshots_of(snapshot_kind, &pool)
                .map_err(|e| e.to_string())
                .map(|snapshots| {
                    snapshots
                        .into_iter()
                        .map(|snapshot| snapshot.name)
                        .collect::<Vec<String>>()
                })
        }
    })
    .await
    .map_err(|e| format!("source listing panicked: {e}"))??;
    let base = replication::choose_base(job_id, recorded.as_deref(), &source_list);

    // 2. Create this run's source snapshot.
    let create = tokio::task::spawn_blocking({
        let pool = pool.clone();
        let dataset = job.source_dataset.clone();
        let name = snapshot.clone();
        move || {
            SnapshotService::create_snapshot(&pool, &name, Some(&dataset), false)
                .map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| format!("snapshot creation panicked: {e}"))?;
    if let Err(e) = create {
        return Err(format!("the source snapshot could not be created: {e}"));
    }

    // 3. Build and run the pipeline.
    let (source_path, destination_path) = paths_for(job, &pool, &snapshot)?;
    let use_pv = replication::pv_available();
    let plan = replication::build_plan(
        job,
        &pool,
        base.as_deref(),
        &snapshot,
        source_path.as_deref(),
        destination_path.as_deref(),
        use_pv,
    )?;
    let stages = plan.stages.clone();
    let sent = tokio::task::spawn_blocking(move || {
        replication::run_pipeline(&stages, replication::REPLICATION_TIMEOUT)
    })
    .await
    .map_err(|e| format!("transfer task panicked: {e}"))?;
    if let Err(e) = sent {
        return Err(format!("the transfer failed: {e}"));
    }

    // 4. Destination-only retention.
    let pruned = prune_destination(job, backend, kind).await?;

    let mut message = format!("Sent {snapshot} to {}", job.destination_dataset);
    if let Some(base) = &base {
        message.push_str(&format!(" (incremental from {base})"));
    }
    if !pruned.is_empty() {
        message.push_str(&format!("; pruned {}", pruned.len()));
    }
    if let Some(note) = replication::bandwidth_note(job) {
        message.push_str(&format!("; {note}"));
    }
    Ok(RunOutcome {
        snapshot: snapshot.clone(),
        message,
    })
}

/// List the destination, select this job's expired snapshots and delete them.
///
/// Retention is best-effort in the sense that a failed *listing* is reported —
/// it never guesses what to delete. A `zfs destroy` that fails is returned too:
/// a destination filling up is exactly what an admin must hear about.
async fn prune_destination(
    job: &ReplicationJob,
    backend: ReplicationBackend,
    kind: replication::DestinationKind,
) -> Result<Vec<String>, String> {
    let Some(list_cmd) = replication::destination_list_command(job) else {
        return Ok(Vec::new());
    };
    let job = job.clone();
    let kind_for_task = kind;
    let output = tokio::task::spawn_blocking(move || {
        replication::run_stage(&list_cmd, replication::REPLICATION_QUERY_TIMEOUT)
    })
    .await
    .map_err(|e| format!("destination listing panicked: {e}"))??;
    if !output.status.success() {
        return Err(format!(
            "the destination could not be listed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let listing = String::from_utf8_lossy(&output.stdout).to_string();
    let names = replication::parse_destination_listing(backend, &listing);
    let to_prune = replication::select_prune(&names, job.id, job.keep_last as usize);
    let _ = kind_for_task;
    for name in &to_prune {
        let Some(delete_cmd) = replication::destination_delete_command(&job, name) else {
            continue;
        };
        let name_for_log = name.clone();
        let deleted = tokio::task::spawn_blocking(move || {
            replication::run_stage(&delete_cmd, replication::REPLICATION_QUERY_TIMEOUT)
        })
        .await
        .map_err(|e| format!("destination delete panicked: {e}"))??;
        if !deleted.status.success() {
            return Err(format!(
                "destination snapshot {name_for_log} could not be pruned: {}",
                String::from_utf8_lossy(&deleted.stderr).trim()
            ));
        }
    }
    Ok(to_prune)
}

pub async fn run_scheduler_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        let repo = ReplicationRepository::new(state.db.pool());
        let due = match repo.due(Utc::now()).await {
            Ok(due) => due,
            Err(e) => {
                tracing::warn!("Replication schedule could not be read: {e}");
                continue;
            }
        };
        for job in due {
            if is_running(job.id) {
                continue;
            }
            if let Err(e) = run_job(&state, &job).await {
                tracing::warn!("Replication job '{}' failed: {e}", job.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn job(interval_minutes: i64) -> ReplicationJob {
        ReplicationJob {
            id: 1,
            name: "nightly".to_string(),
            source_dataset: "tank/data".to_string(),
            backend: "zfs".to_string(),
            destination_kind: "local".to_string(),
            destination_dataset: "backup/data".to_string(),
            destination_host: None,
            destination_user: None,
            destination_port: 22,
            interval_minutes,
            keep_last: 7,
            bandwidth_kbps: 0,
            enabled: 1,
            next_run_at: None,
            last_run_at: None,
            last_snapshot: None,
            last_status: None,
            last_error: None,
            created_by: "admin".to_string(),
            created_at: "2026-10-31T00:00:00+00:00".to_string(),
            updated_at: "2026-10-31T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn the_next_run_is_the_interval_or_none() {
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap();
        assert_eq!(
            next_run_at(&job(90), now),
            Some(now + chrono::Duration::minutes(90))
        );
        // A manual-only job never becomes due, whatever the clock says.
        assert_eq!(next_run_at(&job(0), now), None);
        assert_eq!(next_run_at(&job(-5), now), None);
    }

    #[test]
    fn the_pool_of_a_dataset_is_its_first_component() {
        assert_eq!(pool_of("tank/data/sub"), "tank");
        assert_eq!(pool_of("tank"), "tank");
        assert_eq!(pool_of("/mnt/pools/tank"), "");
    }

    #[test]
    fn a_single_job_cannot_run_twice_at_once() {
        let first = RunGuard::acquire(42).expect("first run acquires");
        assert!(is_running(42));
        assert!(
            RunGuard::acquire(42).is_none(),
            "a second run must be refused"
        );
        drop(first);
        assert!(!is_running(42));
        assert!(RunGuard::acquire(42).is_some());
    }
}
