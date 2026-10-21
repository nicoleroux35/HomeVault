/* =============================================================================
 * File          : jobs.rs
 * Author        : Nicolas Le Roux
 * Created       : 21/10/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Registry of the rescue jobs that own a live OS process (ddrescue clones,
 *   PhotoRec carves), so an admin can actually stop them. Each job runs in its
 *   own process group; cancelling sends SIGTERM to the whole group (escalating
 *   to SIGKILL), which is the only way to stop `sudo ddrescue` — killing the
 *   `sudo` wrapper alone would leave the real tool writing to the disk.
 *
 * Dependencies:
 *   - std only (no tokio: the kill runs on its own thread)
 *
 * Usage:
 *   RescueService::ddrescue_clone / photorec_recover call `start()`, register
 *   the progress-manager cancel hook through `cancel(job_id)` and call
 *   `finish()` on the handle when the process exits.
 * =============================================================================
 */

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tracing::warn;

/// How long a cancelled job gets to exit after SIGTERM before SIGKILL.
const TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

struct JobState {
    /// Process-group id of the running tool, or 0 while the child is still
    /// being spawned. Shared so the service can attach it after `spawn()`.
    pgid: AtomicI32,
    cancelled: AtomicBool,
    /// What to show the user in the cancellation message.
    label: String,
}

struct RunningJob {
    state: Arc<JobState>,
}

static RUNNING: LazyLock<Mutex<HashMap<String, RunningJob>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Handle on one registered rescue job.
#[derive(Clone)]
pub struct RescueJobHandle {
    job_id: String,
    state: Arc<JobState>,
}

impl RescueJobHandle {
    /// `true` once an admin asked for this job to stop.
    pub fn cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    /// Attach the real process-group id, right after `spawn()`.
    ///
    /// If a cancellation arrived while the child was still being spawned (the
    /// pgid was still 0, so nothing could be signalled), the kill is issued
    /// here instead — the request is never silently dropped.
    pub fn attach(&self, pgid: u32) {
        self.state.pgid.store(pgid as i32, Ordering::SeqCst);
        if self.cancelled() {
            warn!(
                "Rescue: job '{}' was cancelled before its process started — stopping pgid {}",
                self.job_id, pgid
            );
            kill_group(pgid);
        }
    }

    /// Unregister on normal (or failed) completion. The hook is dropped by the
    /// progress manager on its own when the job is removed.
    pub fn finish(&self) {
        if let Ok(mut map) = RUNNING.lock() {
            map.remove(&self.job_id);
        }
    }
}

/// Register a job that is about to spawn a child process.
pub fn start(job_id: &str, label: &str) -> RescueJobHandle {
    let state = Arc::new(JobState {
        pgid: AtomicI32::new(0),
        cancelled: AtomicBool::new(false),
        label: label.to_string(),
    });
    if let Ok(mut map) = RUNNING.lock() {
        map.insert(
            job_id.to_string(),
            RunningJob {
                state: Arc::clone(&state),
            },
        );
    }
    RescueJobHandle {
        job_id: job_id.to_string(),
        state,
    }
}

/// Cancel a job by id. Returns `true` when a live job was found and the stop
/// request was issued (the process may take a few seconds to die).
///
/// Idempotent: cancelling twice is harmless, the second call still reports
/// `true` while the job is registered.
pub fn cancel(job_id: &str) -> bool {
    let (state, pgid, label) = match RUNNING.lock() {
        Ok(map) => match map.get(job_id) {
            Some(job) => {
                let state = Arc::clone(&job.state);
                let pgid = state.pgid.load(Ordering::SeqCst);
                let label = state.label.clone();
                (state, pgid, label)
            }
            None => return false,
        },
        Err(_) => return false,
    };

    state.cancelled.store(true, Ordering::SeqCst);
    if pgid > 0 {
        warn!(
            "Rescue: cancelling {} (job {}, pgid {})",
            label, job_id, pgid
        );
        kill_group(pgid as u32);
    } else {
        warn!(
            "Rescue: cancellation requested for job '{}' before its process group exists — it will be stopped as soon as it starts",
            job_id
        );
    }
    true
}

/// `true` while the job is registered (its process may still be exiting).
pub fn is_running(job_id: &str) -> bool {
    RUNNING
        .lock()
        .map(|map| map.contains_key(job_id))
        .unwrap_or(false)
}

/// Ids of every registered job, for the `GET /jobs` listing / diagnostics.
pub fn running_job_ids() -> Vec<String> {
    RUNNING
        .lock()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// Send SIGTERM to a process group, then SIGKILL after [`TERM_GRACE`] if the
/// group is still alive.
///
/// Runs on a detached thread: the caller is a progress-manager cancellation
/// hook invoked from a request handler, which must not block on `sudo kill`.
fn kill_group(pgid: u32) {
    std::thread::spawn(move || {
        if !run_kill("-s", "TERM", pgid) {
            warn!("Rescue: could not send SIGTERM to pgid {}", pgid);
            return;
        }

        std::thread::sleep(TERM_GRACE);
        // `kill -0` is the portable "is this group still there?" probe.
        if run_kill("-0", "", pgid) {
            warn!(
                "Rescue: pgid {} still alive {}s after SIGTERM — sending SIGKILL",
                pgid,
                TERM_GRACE.as_secs()
            );
            if !run_kill("-s", "KILL", pgid) {
                warn!("Rescue: could not send SIGKILL to pgid {}", pgid);
            }
        }
    });
}

/// `sudo kill <flag> <signal> -- -<pgid>`; the negative pid addresses the whole
/// process group. `sudo` is required because the group's leader (`ddrescue`,
/// `photorec`) runs as root — a plain `kill(2)` from the service uid is
/// refused with EPERM, which is exactly why a naive "kill the child" cancel
/// looked like it worked while the tool kept running.
fn run_kill(flag: &str, signal: &str, pgid: u32) -> bool {
    let target = format!("-{pgid}");
    let mut cmd = std::process::Command::new("sudo");
    cmd.arg("kill").arg(flag);
    if !signal.is_empty() {
        cmd.arg(signal);
    }
    cmd.arg("--").arg(&target);

    match cmd.output() {
        Ok(out) => out.status.success(),
        Err(e) => {
            warn!("Rescue: failed to run 'sudo kill': {}", e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_registers_and_finish_unregisters() {
        let id = format!("test-job-{}", std::process::id());
        let handle = start(&id, "test clone");
        assert!(is_running(&id));
        assert!(!handle.cancelled());

        handle.finish();
        assert!(!is_running(&id));
    }

    #[test]
    fn cancel_of_an_unknown_job_reports_false() {
        assert!(!cancel("definitely-not-a-registered-job"));
    }

    #[test]
    fn cancel_sets_the_flag_without_signalling_a_missing_process_group() {
        // pgid stays 0 here: no real child is ever spawned in a unit test, so
        // this exercises the flag path only (no `sudo kill` must run).
        let id = format!("test-job-cancel-{}", std::process::id());
        let handle = start(&id, "test clone");
        assert!(cancel(&id));
        assert!(handle.cancelled(), "the flag must be observable by the job");
        assert!(cancel(&id), "cancelling twice stays true while registered");
        handle.finish();
        assert!(!is_running(&id));
        assert!(!cancel(&id));
    }

    #[test]
    fn handles_are_independent() {
        let a = format!("test-job-a-{}", std::process::id());
        let b = format!("test-job-b-{}", std::process::id());
        let ha = start(&a, "a");
        let hb = start(&b, "b");
        cancel(&a);
        assert!(ha.cancelled());
        assert!(!hb.cancelled());
        ha.finish();
        hb.finish();
    }
}
