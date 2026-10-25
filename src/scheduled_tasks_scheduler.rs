/* =============================================================================
 * File          : scheduled_tasks_scheduler.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Generic scheduled-task engine: one background loop picks up due tasks
 *   from the scheduled_tasks table and executes them by type (safe command,
 *   pool snapshot, SMART self-test, host power action). Replaces per-feature
 *   schedulers for things that are "just a cron job on the host".
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; CRUD via
 *   handlers/scheduled_tasks.rs.
 * =============================================================================
 */

//! Generic scheduled-task engine (the loop + the executor).
//!
//! Structural sibling of `restic_scheduler`/`antivirus_scheduler`: polls
//! every [`POLL_INTERVAL_SECS`], computes the next run from each task's cron
//! **in that task's timezone** ([`next_run_at`], over the shared
//! `restic_scheduler::parse_schedule`), and spawns executions so one long
//! command can never delay the other tasks' ticks.
//!
//! Security posture: every payload is validated **at creation time** by
//! [`TaskAction::from_parts`] (the handler refuses anything that doesn't
//! validate), and again at run time — a task row written directly into the
//! DB by some other path still can't make the executor do something the
//! validators reject (absolute, existing executable; bare device names;
//! whitelisted actions).

use crate::handlers::AppState;
use crate::power::{host_power_action, PowerAction};
use crate::snapshots::SnapshotService;
use crate::storage::scheduled_task_repo::{ScheduledTask, ScheduledTaskRepository};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 30;
/// A scheduled command gets at most 30 minutes; after that it is killed and
/// the run is recorded as failed (an orphaned child would otherwise hold a
/// `running` status forever).
const COMMAND_TIMEOUT_SECS: u64 = 30 * 60;

/// The validated form of a task's payload. `from_parts` is the single gate
/// every payload passes through — see the module docs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskAction {
    /// Run one program (no shell). `args` are passed verbatim as argv.
    Command { command: String, args: Vec<String> },
    /// Create a pool snapshot (ZFS-first, Btrfs fallback).
    Snapshot { pool: String, recursive: bool },
    /// Start a S.M.A.R.T. self-test on a disk (`short`|`long`|`conveyance`|`offline`).
    SmartTest { device: String, test_type: String },
    /// Reboot / shutdown / suspend the host.
    Power { action: PowerAction },
}

impl TaskAction {
    pub const fn type_name(&self) -> &'static str {
        match self {
            TaskAction::Command { .. } => "command",
            TaskAction::Snapshot { .. } => "snapshot",
            TaskAction::SmartTest { .. } => "smart_test",
            TaskAction::Power { .. } => "power",
        }
    }

    /// Validates `(task_type, payload JSON)` and builds the action. This is
    /// the single validation gate — used by the create/update handler and
    /// again by the executor before running.
    pub fn from_parts(task_type: &str, payload_json: &str) -> Result<TaskAction, String> {
        let action: TaskAction = match task_type {
            "command" => {
                let raw: RawCommandPayload = serde_json::from_str(payload_json)
                    .map_err(|e| format!("Invalid command payload: {e}"))?;
                let command = raw.command.trim().to_string();
                if !command.starts_with('/') {
                    return Err("Command must be an absolute path (no shell, no lookup)".into());
                }
                if command.contains("..")
                    || command.contains(';')
                    || command.contains('|')
                    || command.contains('&')
                    || command.contains('`')
                    || command.chars().any(|c| c.is_whitespace())
                {
                    return Err("Command must be a single executable path".into());
                }
                let path = std::path::Path::new(&command);
                if !path.is_file() {
                    return Err(format!("'{command}' does not exist on this host"));
                }
                // `mode & 0o111` — the executable bits for someone.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(path)
                        .map_err(|e| format!("'{command}' is not accessible: {e}"))?
                        .permissions()
                        .mode();
                    if mode & 0o111 == 0 {
                        return Err(format!("'{command}' is not executable"));
                    }
                }
                if raw.args.len() > 64 {
                    return Err("Too many arguments (max 64)".into());
                }
                TaskAction::Command {
                    command,
                    args: raw.args,
                }
            }
            "snapshot" => {
                let raw: RawSnapshotPayload = serde_json::from_str(payload_json)
                    .map_err(|e| format!("Invalid snapshot payload: {e}"))?;
                if !is_valid_pool_name(&raw.pool) {
                    return Err(format!("Invalid pool name '{}'", raw.pool));
                }
                TaskAction::Snapshot {
                    pool: raw.pool,
                    recursive: raw.recursive,
                }
            }
            "smart_test" => {
                let raw: RawSmartTestPayload = serde_json::from_str(payload_json)
                    .map_err(|e| format!("Invalid smart_test payload: {e}"))?;
                if !crate::power::is_valid_block_device_name(&raw.device) {
                    return Err(format!("Invalid device name '{}'", raw.device));
                }
                if !matches!(
                    raw.test_type.as_str(),
                    "short" | "long" | "conveyance" | "offline"
                ) {
                    return Err(format!("Unknown SMART test type '{}'", raw.test_type));
                }
                TaskAction::SmartTest {
                    device: raw.device,
                    test_type: raw.test_type,
                }
            }
            "power" => {
                let raw: RawPowerPayload = serde_json::from_str(payload_json)
                    .map_err(|e| format!("Invalid power payload: {e}"))?;
                let action = PowerAction::parse(&raw.action)
                    .ok_or_else(|| format!("Unknown power action '{}'", raw.action))?;
                TaskAction::Power { action }
            }
            other => return Err(format!("Unknown task type '{other}'")),
        };
        debug_assert_eq!(action.type_name(), task_type);
        Ok(action)
    }
}

#[derive(Deserialize)]
struct RawCommandPayload {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize)]
struct RawSnapshotPayload {
    pool: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct RawSmartTestPayload {
    device: String,
    test_type: String,
}

#[derive(Deserialize)]
struct RawPowerPayload {
    action: String,
}

/// Pool/dataset names end up spliced into `zfs`/`btrfs` argv — restrict to
/// the characters both filesystems actually accept.
pub fn is_valid_pool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':'))
}

/// Parses an IANA timezone name (`Europe/Paris`) into the zone a cron is
/// evaluated in.
///
/// This is the single gate every timezone value passes through: the HTTP
/// handler rejects an unknown name before it is stored, and the scheduler
/// re-validates at run time so a hand-edited row can never make the engine
/// silently fall back to UTC. The name is canonicalised (`europe/paris` →
/// `Europe/Paris`), which is what gets persisted.
pub fn parse_timezone(name: &str) -> Result<Tz, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Timezone must not be empty".to_string());
    }
    Tz::from_str(trimmed).map_err(|_| {
        format!("Unknown timezone '{trimmed}' — use an IANA name such as 'Europe/Paris'")
    })
}

/// The next occurrence of `cron`, evaluated **in `timezone`** and returned as
/// an absolute UTC instant.
///
/// A cron expression is only meaningful with the zone it is evaluated in
/// (ROADMAP 12.2): `0 2 * * *` in `Europe/Paris` is 00:00 or 01:00 UTC
/// depending on the season, and that shift is exactly what a fixed offset gets
/// wrong twice a year. `timezone` is the task's own zone when it has one;
/// `None` follows the appliance zone (`server_timezone`), which is what
/// `handlers::settings::effective_timezone()` returns.
///
/// An unknown task zone is an **error**, not a silent UTC fallback: the task
/// is then paused with a readable reason (see [`pause_unrunnable`]) instead of
/// firing at the wrong hour for months.
pub fn next_run_at(
    cron: &str,
    timezone: Option<&str>,
    server_timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    let schedule = crate::restic_scheduler::parse_schedule(cron)?;
    let zone = match timezone.map(str::trim).filter(|z| !z.is_empty()) {
        Some(name) => parse_timezone(name)?,
        None => parse_timezone(server_timezone).unwrap_or(chrono_tz::UTC),
    };
    Ok(schedule
        .after(&after.with_timezone(&zone))
        .next()
        .map(|d| d.with_timezone(&Utc)))
}

/// Tasks with a run in progress, process-wide.
///
/// One process owns both the scheduler loop and the HTTP handlers, so an
/// in-memory set is enough — same reasoning as `snapshot_scheduler::RUNNING`
/// and the cache mover's `MOVER_RUNNING`. It is a `static` (not a value
/// passed around) precisely because both entry points must share it: a
/// "Run now" and a scheduler tick that each held their own set could start
/// the same task twice.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Why a run was not started, or started and failed.
#[derive(Debug, PartialEq)]
pub enum RunTaskError {
    /// Another run of the same task is still in progress; nothing was
    /// executed and the task row was left untouched.
    Busy(String),
    /// The run was attempted and failed (already recorded on the task).
    Failed(String),
}

impl std::fmt::Display for RunTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunTaskError::Busy(m) | RunTaskError::Failed(m) => f.write_str(m),
        }
    }
}

/// Claims one task for the duration of a run. Dropping it releases the
/// claim, so an early return or a panic can never wedge a task forever.
/// Only the insert/remove touches the mutex — never across an await.
#[derive(Debug)]
struct TaskRunGuard {
    task_id: String,
}

impl TaskRunGuard {
    fn acquire(task_id: &str, task_name: &str) -> Result<Self, RunTaskError> {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if !running.insert(task_id.to_string()) {
            return Err(RunTaskError::Busy(format!(
                "Task '{task_name}' is already running"
            )));
        }
        Ok(Self {
            task_id: task_id.to_string(),
        })
    }
}

impl Drop for TaskRunGuard {
    fn drop(&mut self) {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        running.remove(&self.task_id);
    }
}

/// Runs forever. Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<AppState>) {
    // A run row is written before its execution starts, so anything still
    // `running` here belongs to a process that is gone (crash, restart,
    // `systemctl stop` during a run). Close those first — otherwise the screen
    // shows "Running" for ever and the history keeps a phantom run.
    let repo = ScheduledTaskRepository::new(state.db.pool());
    match repo.close_interrupted_runs().await {
        Ok(0) => {}
        Ok(n) => tracing::warn!("Closed {n} scheduled-task run(s) interrupted by a restart"),
        Err(e) => tracing::warn!("Could not close interrupted scheduled-task runs: {e}"),
    }

    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = tick_due(&state).await {
            tracing::warn!("Scheduled-task scheduler tick failed: {}", e);
        }
    }
}

async fn tick_due(state: &Arc<AppState>) -> Result<(), String> {
    let repo = ScheduledTaskRepository::new(state.db.pool());
    let tasks = repo.list_enabled().await.map_err(|e| e.to_string())?;
    let now = Utc::now();
    // Read once per tick, not once per task: it is the zone every task
    // without its own `timezone` is evaluated in (ROADMAP 12.2).
    let server_timezone = crate::handlers::settings::effective_timezone();

    for task in tasks {
        // A task that has never run is due when its cron first fires after
        // creation; a task that has run is due when the next occurrence
        // computed from its last run has passed. A cron (or timezone) that
        // cannot be parsed pauses the task instead of retrying every tick
        // (ROADMAP 12.3).
        let next = match next_run_at(
            &task.cron,
            task.timezone.as_deref(),
            &server_timezone,
            reference_time(&task),
        ) {
            Err(reason) => {
                pause_unrunnable(&repo, &task, &reason).await;
                continue;
            }
            Ok(next) => next,
        };
        let Some(next) = next else { continue };
        let _ = repo
            .set_next_run(&task.id, Some(next.to_rfc3339().as_str()))
            .await;

        if next <= now {
            // Skip (don't block) when the previous execution is still going —
            // whichever path started it.
            match TaskRunGuard::acquire(&task.id, &task.name) {
                Ok(guard) => spawn_execution(state.clone(), task, guard),
                Err(_) => tracing::info!(
                    "Scheduled task '{}' still running; skipping tick",
                    task.name
                ),
            }
        }
    }
    Ok(())
}

/// A task whose stored cron (or timezone) cannot be parsed is **paused**, not
/// retried: the tick used to record a failed run every 30 s for ever, so the
/// history table grew without bound and the screen filled with noise
/// (ROADMAP 12.3).
///
/// One failed run is recorded first, so the run-history drawer explains what
/// happened and when, then the task is disabled with the same sentence on its
/// row. The admin fixes the expression and re-enables it — no data is lost.
async fn pause_unrunnable(repo: &ScheduledTaskRepository, task: &ScheduledTask, reason: &str) {
    // Covers both halves: a cron the parser refuses and a zone the database
    // does not know (`next_run_at` reports which).
    let message = format!(
        "Invalid schedule for cron '{}': {reason}. The task has been paused.",
        task.cron
    );
    if let Ok(run_id) = repo.mark_started(&task.id).await {
        let _ = repo
            .mark_finished(&task.id, &run_id, false, Some(&message), None)
            .await;
    }
    if let Err(e) = repo.pause_with_error(&task.id, &message).await {
        tracing::warn!(
            "Scheduled task '{}' could not be paused after an invalid schedule: {e}",
            task.name
        );
    }
    tracing::warn!("Scheduled task '{}' paused: {message}", task.name);
}

/// The moment the next-occurrence computation starts from: the last run for
/// an existing task, creation time otherwise — capped to "at most 8 days
/// ago" so a task created with a cron that already passed today doesn't fire
/// immediately (mirrors the antivirus scheduler's window).
fn reference_time(task: &ScheduledTask) -> DateTime<Utc> {
    let parsed = task
        .last_run_at
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            DateTime::parse_from_rfc3339(&task.created_at)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })
        .unwrap_or_else(Utc::now);
    let earliest = Utc::now() - chrono::Duration::days(8);
    parsed.max(earliest)
}

/// Spawns one execution holding the run guard, so overlapping ticks and a
/// concurrent "Run now" never double-start the same task.
fn spawn_execution(state: Arc<AppState>, task: ScheduledTask, guard: TaskRunGuard) {
    tokio::spawn(async move {
        let _guard = guard;
        let repo = ScheduledTaskRepository::new(state.db.pool());
        match repo.mark_started(&task.id).await {
            Ok(run_id) => finish_run(&state, &task, &run_id).await,
            Err(e) => tracing::warn!("Scheduled task '{}' could not open a run: {e}", task.name),
        }
    });
}

/// A run that has been claimed and opened: the HTTP handler answers with this
/// and the execution continues on its own.
#[derive(Debug, Clone, Serialize)]
pub struct StartedRun {
    pub run_id: String,
    pub task_id: String,
    pub status: &'static str,
}

/// Starts a task **outside the request** (ROADMAP 12.4).
///
/// The run used to be awaited inside `handle_task_run_now`, with the
/// `TaskRunGuard` living on the request future: if the client went away, hyper
/// dropped that future, the guard was released while the child kept running,
/// and the next start was accepted — the same task running twice. Here the
/// guard is moved into the spawned task that owns the execution, so the claim
/// lives exactly as long as the run does, and the request returns as soon as
/// the run row exists.
///
/// The run row is opened **before** returning, so the caller can poll it and
/// the screen shows `running` immediately.
pub async fn start_task_run(
    state: &Arc<AppState>,
    task: &ScheduledTask,
) -> Result<StartedRun, RunTaskError> {
    let guard = TaskRunGuard::acquire(&task.id, &task.name)?;
    let repo = ScheduledTaskRepository::new(state.db.pool());
    let run_id = repo
        .mark_started(&task.id)
        .await
        .map_err(|e| RunTaskError::Failed(format!("Could not open a run: {e}")))?;

    let task_id = task.id.clone();
    let state = state.clone();
    let task = task.clone();
    let spawned_run_id = run_id.clone();
    tokio::spawn(async move {
        // The claim is released when the execution ends, success or not.
        let _guard = guard;
        finish_run(&state, &task, &spawned_run_id).await;
    });

    Ok(StartedRun {
        run_id,
        task_id,
        status: "running",
    })
}

/// Executes one already-opened run and records the outcome (output tail or
/// error) on both the task and the run row.
async fn finish_run(state: &Arc<AppState>, task: &ScheduledTask, run_id: &str) {
    let repo = ScheduledTaskRepository::new(state.db.pool());
    let result = match TaskAction::from_parts(&task.task_type, &task.payload) {
        // Re-validate at run time: a row written directly into the DB by
        // some other path still goes through the same validators.
        Ok(action) => perform(&action).await,
        Err(e) => Err(e),
    };

    match result {
        Ok(output) => {
            let _ = repo
                .mark_finished(&task.id, run_id, true, None, Some(&output))
                .await;
        }
        Err(e) => {
            tracing::warn!("Scheduled task '{}' failed: {}", task.name, e);
            let _ = repo
                .mark_finished(&task.id, run_id, false, Some(&e), None)
                .await;
        }
    }
}

/// Dispatches one validated action. Pure I/O — no schedule logic here.
async fn perform(action: &TaskAction) -> Result<String, String> {
    match action {
        TaskAction::Command { command, args } => {
            let mut cmd = tokio::process::Command::new(command);
            cmd.args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                // If the runtime shuts down mid-run, the child dies with it
                // instead of outliving the server (the run row is closed as
                // interrupted on the next start).
                .kill_on_drop(true);
            let child = cmd
                .spawn()
                .map_err(|e| format!("Failed to start {command}: {e}"))?;
            let output = tokio::time::timeout(
                Duration::from_secs(COMMAND_TIMEOUT_SECS),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| format!("Command {command} timed out after {COMMAND_TIMEOUT_SECS}s"))?
            .map_err(|e| format!("Command {command} failed: {e}"))?;
            let mut out = String::from_utf8_lossy(&output.stdout).to_string();
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.is_empty() {
                out.push_str(&err);
            }
            if !output.status.success() {
                return Err(format!(
                    "Command {command} exited with {}: {}",
                    output
                        .status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".into()),
                    out.trim()
                ));
            }
            Ok(out)
        }
        TaskAction::Snapshot { pool, recursive } => {
            let name = format!("hv-auto-{}", Utc::now().format("%Y%m%d-%H%M%S"));
            let pool = pool.clone();
            let recursive = *recursive;
            tokio::task::spawn_blocking(move || {
                SnapshotService::create_snapshot(&pool, &name, None, recursive)
                    .map(|_| format!("Created snapshot {name}"))
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| format!("Snapshot task panicked: {e}"))?
        }
        TaskAction::SmartTest { device, test_type } => {
            let device = device.clone();
            let test_type = test_type.clone();
            tokio::task::spawn_blocking(move || {
                // Bounded: a self-test on a disk in a bad power state can
                // block the `sudo smartctl` for ever, and the scheduler task
                // holds this blocking thread meanwhile (ROADMAP X.9).
                let mut smart_cmd = crate::smart::std_command(
                    &format!("/dev/{device}"),
                    &["-t", test_type.as_str()],
                );
                let output = crate::utils::process::run_timed(
                    &mut smart_cmd,
                    crate::utils::process::SMART_PROBE_TIMEOUT,
                )
                .map_err(|e| format!("smartctl not available: {e}"))?;
                // smartctl exits with a bitmask; bits 0-2 cover CLI/device
                // errors. Health-report bits (4 = SMART failing, 8 = self-test
                // logged failures) are about the *disk*, not about whether the
                // test was started — starting a test on a failing disk is a
                // legitimate outcome, so only bits 0-2 count as failure here.
                let started_ok = output
                    .status
                    .code()
                    .map(|c| (0..4).contains(&c))
                    .unwrap_or(false);
                if started_ok {
                    Ok(format!("SMART {test_type} test started on /dev/{device}"))
                } else {
                    Err(format!(
                        "smartctl -t {test_type} failed on /dev/{device}: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ))
                }
            })
            .await
            .map_err(|e| format!("SMART task panicked: {e}"))?
        }
        TaskAction::Power { action } => {
            let action = *action;
            tokio::task::spawn_blocking(move || host_power_action(action))
                .await
                .map_err(|e| format!("Power task panicked: {e}"))?
                .map(|_| format!("Host {} issued", action.as_str()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_payload_requires_absolute_existing_executable() {
        // Non-absolute paths are the shell-lookup hole — closed.
        let err = TaskAction::from_parts("command", r#"{"command":"curl","args":[]}"#).unwrap_err();
        assert!(err.contains("absolute path"), "got: {err}");

        // Shell metacharacters in the "path" are rejected outright.
        let err = TaskAction::from_parts("command", r#"{"command":"/tmp/evil; reboot","args":[]}"#)
            .unwrap_err();
        assert!(err.contains("single executable path"), "got: {err}");

        // An absolute but missing file is rejected.
        let err = TaskAction::from_parts("command", r#"{"command":"/no/such/binary","args":[]}"#)
            .unwrap_err();
        assert!(err.contains("does not exist"), "got: {err}");

        // A real, executable absolute path passes.
        let ok_path = "/bin/true";
        if std::path::Path::new(ok_path).is_file() {
            let action =
                TaskAction::from_parts("command", r#"{"command":"/bin/true","args":["-q"]}"#)
                    .expect("valid command payload");
            match action {
                TaskAction::Command { command, args } => {
                    assert_eq!(command, "/bin/true");
                    assert_eq!(args, vec!["-q"]);
                }
                other => panic!("wrong variant: {other:?}"),
            }
        }
    }

    #[test]
    fn smart_test_payload_is_whitelisted() {
        let action =
            TaskAction::from_parts("smart_test", r#"{"device":"sda","test_type":"long"}"#).unwrap();
        match action {
            TaskAction::SmartTest { device, test_type } => {
                assert_eq!(device, "sda");
                assert_eq!(test_type, "long");
            }
            other => panic!("wrong variant: {other:?}"),
        }
        // Device traversal must not reach /dev/.
        assert!(TaskAction::from_parts(
            "smart_test",
            r#"{"device":"../../etc","test_type":"short"}"#
        )
        .is_err());
        // Test types are a closed enum.
        assert!(
            TaskAction::from_parts("smart_test", r#"{"device":"sda","test_type":"--force"}"#)
                .is_err()
        );
    }

    #[test]
    fn power_payload_accepts_only_known_actions() {
        let action = TaskAction::from_parts("power", r#"{"action":"reboot"}"#).unwrap();
        assert_eq!(action.type_name(), "power");
        assert!(TaskAction::from_parts("power", r#"{"action":"detonate"}"#).is_err());
    }

    #[test]
    fn snapshot_payload_validates_pool_names() {
        let action =
            TaskAction::from_parts("snapshot", r#"{"pool":"tank/data","recursive":true}"#).unwrap();
        assert_eq!(action.type_name(), "snapshot");
        assert!(
            TaskAction::from_parts("snapshot", r#"{"pool":"a b","recursive":false}"#).is_err(),
            "spaces would split argv"
        );
        assert!(
            TaskAction::from_parts("snapshot", r#"{"pool":"-o x","recursive":false}"#).is_err(),
            "leading dash would parse as a zfs option"
        );
    }

    #[test]
    fn unknown_task_type_is_rejected() {
        assert!(TaskAction::from_parts("rm_rf", "{}").is_err());
    }

    /// 12.1: one process-wide running set. A second start of the same task is
    /// refused while the first is in progress — whichever path started it —
    /// and the claim is released when the run ends (success, failure or
    /// panic, since the guard is released on drop).
    #[test]
    fn a_task_cannot_run_twice_at_once() {
        let guard = TaskRunGuard::acquire("id-12-1-single-flight", "nightly backup")
            .expect("the first run takes the claim");

        match TaskRunGuard::acquire("id-12-1-single-flight", "nightly backup") {
            Err(RunTaskError::Busy(msg)) => assert!(
                msg.contains("nightly backup") && msg.contains("already running"),
                "the refusal must name the task: {msg}"
            ),
            other => panic!("a concurrent run of the same task must be refused, got {other:?}"),
        }

        // Another task is unaffected.
        assert!(TaskRunGuard::acquire("id-12-1-other", "other task").is_ok());

        drop(guard);
        assert!(
            TaskRunGuard::acquire("id-12-1-single-flight", "nightly backup").is_ok(),
            "the claim must be released when the run ends"
        );
    }

    /// 12.1: the two entry points share the registry — a manual run must see
    /// the claim a scheduler tick took, and vice versa. They used to hold
    /// separate sets, so a "Run now" could overlap a tick (two restic runs
    /// on the same target).
    #[test]
    fn manual_and_scheduled_paths_share_one_registry() {
        // The tick path claims first; the manual path must see it.
        let tick_claim = TaskRunGuard::acquire("id-12-1-shared", "shared").expect("tick claim");
        assert!(matches!(
            TaskRunGuard::acquire("id-12-1-shared", "shared"),
            Err(RunTaskError::Busy(_))
        ));
        drop(tick_claim);

        // And the other way round.
        let manual_claim = TaskRunGuard::acquire("id-12-1-shared", "shared").expect("manual claim");
        assert!(matches!(
            TaskRunGuard::acquire("id-12-1-shared", "shared"),
            Err(RunTaskError::Busy(_))
        ));
        drop(manual_claim);
    }

    #[test]
    fn reference_time_is_capped_to_eight_days() {
        let old = (Utc::now() - chrono::Duration::days(400)).to_rfc3339();
        let task = ScheduledTask {
            id: "t".into(),
            name: "t".into(),
            task_type: "command".into(),
            cron: "* * * * *".into(),
            payload: "{}".into(),
            enabled: true,
            timezone: None,
            last_run_at: Some(old.clone()),
            last_status: None,
            last_error: None,
            next_run_at: None,
            created_at: old,
            updated_at: String::new(),
        };
        let reference = reference_time(&task);
        assert!(
            reference > Utc::now() - chrono::Duration::days(9),
            "a stale last_run_at must be capped so past crons don't fire immediately"
        );
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// 12.2: a timezone name is validated, trimmed and canonicalised — the
    /// value that gets persisted is the one the zone database knows.
    #[test]
    fn parse_timezone_accepts_iana_names_only() {
        assert_eq!(
            parse_timezone(" Europe/Paris ").unwrap().name(),
            "Europe/Paris",
            "surrounding whitespace must not make a valid zone invalid"
        );
        assert_eq!(parse_timezone("UTC").unwrap().name(), "UTC");
        assert!(parse_timezone("Europe/Nowhere").is_err());
        assert!(parse_timezone("+02:00").is_err(), "offsets are not zones");
        assert!(parse_timezone("").is_err());
        assert!(parse_timezone("   ").is_err());
    }

    /// 12.2: `0 2 * * *` means 02:00 **in the task's zone**. The two cases
    /// below are the same wall-clock hour in Paris, six months apart — a fixed
    /// offset would return the same UTC instant for both.
    #[test]
    fn a_cron_is_evaluated_in_the_tasks_timezone() {
        // Winter: Paris is UTC+1, so 02:00 local is 01:00 UTC.
        let winter = next_run_at(
            "0 2 * * *",
            Some("Europe/Paris"),
            "UTC",
            at("2026-01-10T12:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(winter, at("2026-01-11T01:00:00Z"));

        // Summer: Paris is UTC+2, so 02:00 local is 00:00 UTC.
        let summer = next_run_at(
            "0 2 * * *",
            Some("Europe/Paris"),
            "UTC",
            at("2026-07-10T12:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(summer, at("2026-07-11T00:00:00Z"));

        // Without its own zone the task follows the appliance timezone.
        let server_zone = next_run_at(
            "0 2 * * *",
            None,
            "Europe/Paris",
            at("2026-01-10T12:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            server_zone, winter,
            "no per-task zone must mean the appliance zone, not UTC"
        );

        // An explicit UTC task is unaffected by the appliance zone.
        let utc = next_run_at(
            "0 2 * * *",
            Some("UTC"),
            "Europe/Paris",
            at("2026-01-10T12:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(utc, at("2026-01-11T02:00:00Z"));
    }

    /// 12.2/12.3: an unusable schedule is an error the caller can pause the
    /// task with — never a silent UTC fallback that fires at the wrong hour.
    #[test]
    fn an_unknown_timezone_or_cron_is_an_error() {
        let err = next_run_at(
            "0 2 * * *",
            Some("Mars/Olympus"),
            "UTC",
            at("2026-01-10T12:00:00Z"),
        )
        .unwrap_err();
        assert!(
            err.contains("Mars/Olympus"),
            "the reason must name it: {err}"
        );

        assert!(next_run_at("not a cron", None, "UTC", at("2026-01-10T12:00:00Z")).is_err());

        // An appliance zone that cannot be parsed falls back to UTC rather
        // than failing every task (the setting is validated on write, so this
        // is only reachable through a hand-edited settings file).
        let fallback = next_run_at(
            "0 2 * * *",
            None,
            "Nowhere/Special",
            at("2026-01-10T12:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(fallback, at("2026-01-11T02:00:00Z"));
    }

    /// 12.2: the day-of-week normalisation the restic scheduler relies on also
    /// applies here — "every Monday 03:00" in Paris is Monday, not Sunday.
    #[test]
    fn weekday_normalisation_survives_the_timezone_hop() {
        // 2026-01-10 is a Saturday; the next Monday 03:00 Paris = 02:00 UTC.
        let next = next_run_at(
            "0 3 * * 1",
            Some("Europe/Paris"),
            "UTC",
            at("2026-01-10T12:00:00Z"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next, at("2026-01-12T02:00:00Z"));
    }
}
