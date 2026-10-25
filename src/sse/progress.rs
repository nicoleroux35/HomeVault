/* =============================================================================
 * File          : progress.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Real-time progress tracking for long-running operations (backups, file
 *   copies, docker installs, antivirus scans, ...). A global ProgressManager
 *   registry keeps one broadcast channel per job; handlers report updates
 *   through the ProgressEmitter helper and browsers follow along on the SSE
 *   endpoints. Jobs carry an optional owner so the unified activity feed
 *   can scope visibility (a user's jobs stay hidden from other users).
 *
 * Dependencies:
 *   - chrono (>= 0.4)
 *   - parking_lot (>= 0.12)
 *   - serde (>= 1.0)
 *   - tokio (>= 1.52)
 *   - uuid (>= 1.0)
 *
 * Usage:
 *   From a handler: `ProgressEmitter::backup(...)`-style helpers (or
 *   PROGRESS_MANAGER directly) to create/advance/close a job; clients read
 *   the /api/progress endpoints.
 * =============================================================================
 */

//! Server-Sent Events (SSE) progress system for long-running operations.
//!
//! Clients connect to an SSE endpoint and receive real-time [`ProgressEvent`]
//! messages via `broadcast` channels. Use the [`ProgressEmitter`] helper to
//! create jobs and report progress from handlers.
//!
//! # Example
//! ```rust,ignore
//! let job_id = ProgressEmitter::backup("source", "dest");
//! ProgressEmitter::progress(&job_id, 25.0, "Compressing files...");
//! ProgressEmitter::complete(&job_id, "Done");
//! ```

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    time::Duration,
};
use tokio::sync::broadcast;
use uuid::Uuid;

/// The type or category of a long-running job.
///
/// Wire shape: `{"type":"backup"}` for the built-in kinds and
/// `{"type":"custom","name":"explorer_copy"}` for a feature job.
///
/// **The `content = "name"` half is load-bearing.** With plain internal
/// tagging (`tag = "type"` only) a newtype variant wrapping a `String` cannot
/// be serialized at all — serde refuses with "cannot serialize tagged newtype
/// variant JobType::Custom containing a string", because the tagged map has
/// nowhere to put a bare string. `Custom` is what every feature job uses
/// (explorer copy/move, docker install/update, antivirus scan, rag index,
/// vm create, rescue, cloud sync, ollama pull), so the fallout was: the SSE
/// stream's `serde_json::to_string(&event).unwrap_or_default()` sent an
/// **empty** `data:` payload the client's `JSON.parse` rejects, and
/// `GET /api/progress/jobs` panicked inside the `json!` macro. Adjacent
/// tagging leaves the built-in kinds' shape byte-identical and gives `Custom`
/// a field of its own. Guarded by the serde round-trip tests at the bottom of
/// this file — do not "simplify" this back to `tag` alone.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "name", rename_all = "snake_case")]
pub enum JobType {
    Backup,
    Restore,
    Verify,
    Upload,
    Download,
    Sync,
    CacheMover,
    Custom(String),
}

impl Default for JobType {
    fn default() -> Self {
        JobType::Custom("unknown".to_string())
    }
}

/// Current state of a job
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProgressState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Progress update event sent to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEvent {
    /// Unique job identifier
    pub job_id: String,
    /// Type of job (backup, restore, etc.)
    pub job_type: JobType,
    /// Current state
    pub state: ProgressState,
    /// Progress percentage (0-100)
    pub progress: f64,
    /// Human-readable message
    pub message: String,
    /// Optional: bytes processed
    pub bytes_processed: Option<u64>,
    /// Optional: total bytes
    pub bytes_total: Option<u64>,
    /// Optional: speed in bytes/second
    pub speed_bps: Option<u64>,
    /// Timestamp of the event
    pub timestamp: DateTime<Utc>,
}

impl ProgressEvent {
    pub fn new(
        job_id: String,
        job_type: JobType,
        state: ProgressState,
        progress: f64,
        message: &str,
    ) -> Self {
        Self {
            job_id,
            job_type,
            state,
            progress: progress.clamp(0.0, 100.0),
            message: message.to_string(),
            bytes_processed: None,
            bytes_total: None,
            speed_bps: None,
            timestamp: Utc::now(),
        }
    }
}

/// Internal bookkeeping for an active progress-tracking job.
struct JobInfo {
    job_type: JobType,
    sender: broadcast::Sender<ProgressEvent>,
    created_at: DateTime<Utc>,
    /// Create-time seed only — the live state lives in `last_event`.
    state: ProgressState,
    /// The most recent event emitted for this job (interior-mutable so
    /// `emit_progress*` / `complete_job` / `fail_job` keep working under a
    /// `jobs.read()` lock). Seeded by `create_job*` with the initial event.
    last_event: Mutex<Option<ProgressEvent>>,
    /// `true` once `emit_progress*` has run at least once — lets the activity
    /// feed tell "0 %" apart from "no percentage reported yet" (indeterminate).
    has_progress: AtomicBool,
    /// Username that started the job. `None` = system- or admin-triggered
    /// (visible to every admin, hidden from non-admins).
    owner: Option<String>,
}

/// A point-in-time view of one active job, for the unified activity feed.
#[derive(Debug, Clone)]
pub struct ActivitySnapshot {
    pub id: String,
    pub job_type: JobType,
    pub owner: Option<String>,
    pub state: ProgressState,
    /// When the last event was emitted, or `None` for a job that has not
    /// reported anything yet.
    ///
    /// The activity feed needs it to answer "did this fail *just now*?" — a
    /// job that failed ten minutes ago is history, one that failed a second
    /// ago is what the user is waiting on (ROADMAP 28.2).
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `None` when no percentage has been reported yet (indeterminate).
    pub progress: Option<f64>,
    pub message: String,
    pub bytes_processed: Option<u64>,
    pub bytes_total: Option<u64>,
    pub speed_bps: Option<u64>,
}

/// One-shot cancellation callback registered by a job's producer.
///
/// A job that owns a real OS resource (a `sudo ddrescue` child process, a
/// running `photorec`) registers a hook that actually stops it. Cancelling the
/// job then runs the hook *and* flips the job's state to
/// [`ProgressState::Cancelled`], so the UI never reports "cancelled" for work
/// that is still running.
pub type CancelHook = Box<dyn FnOnce() + Send + 'static>;

/// Outcome of [`ProgressManager::cancel_job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelOutcome {
    /// Unknown job id — nothing to cancel.
    NotFound,
    /// The job was tracked but had no producer hook (nothing to stop beyond
    /// dropping it from the activity feed).
    NoHook,
    /// The producer's hook ran: its real work is being stopped.
    HookRan,
}

/// Central registry for all active progress-tracking jobs.
pub struct ProgressManager {
    jobs: RwLock<HashMap<String, JobInfo>>,
    /// Cancellation hooks, keyed like `jobs`. Kept in a separate map so the
    /// hook can be taken out (and run) without holding the `jobs` lock while
    /// the callback shells out to `kill`.
    cancel_hooks: Mutex<HashMap<String, CancelHook>>,
}

impl Default for ProgressManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressManager {
    pub fn new() -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
            cancel_hooks: Mutex::new(HashMap::new()),
        }
    }

    /// Register the producer's cancellation hook for `job_id`.
    ///
    /// Replaces any previously registered hook (the newest producer wins).
    /// Registering for an unknown job is allowed: the caller usually creates
    /// the job first, but the hook is keyed independently so ordering can't
    /// silently drop a cancellation.
    pub fn register_cancel_hook(&self, job_id: &str, hook: CancelHook) {
        self.cancel_hooks.lock().insert(job_id.to_string(), hook);
    }

    /// Drop the registered hook without running it (the job finished on its
    /// own). Prevents the map from growing with dead entries.
    pub fn clear_cancel_hook(&self, job_id: &str) {
        self.cancel_hooks.lock().remove(job_id);
    }

    /// Cancel a job: run its producer hook (if any), mark it `Cancelled` on the
    /// SSE stream, and drop it from the tracked set.
    ///
    /// Returns what actually happened, so the HTTP layer can answer honestly —
    /// the previous `remove_job`-only implementation replied "Job cancelled"
    /// while a `sudo ddrescue` clone kept writing to a disk.
    pub fn cancel_job(&self, job_id: &str) -> CancelOutcome {
        let known = self.jobs.read().contains_key(job_id);
        let hook = self.cancel_hooks.lock().remove(job_id);
        let had_hook = hook.is_some();

        if let Some(hook) = hook {
            hook();
        }

        if known {
            self.mark_cancelled(job_id, "Cancelled by admin");
        }

        match (known, had_hook) {
            (false, false) => CancelOutcome::NotFound,
            (_, false) => {
                self.jobs.write().remove(job_id);
                CancelOutcome::NoHook
            }
            (_, true) => {
                // The producer's task still owns the id until it observes the
                // cancellation and finishes; dropping it here would make its
                // final `fail_job` a no-op. `remove_job` is the producer's job.
                CancelOutcome::HookRan
            }
        }
    }

    /// Flip a tracked job to [`ProgressState::Cancelled`] and broadcast it.
    /// Returns `false` for an unknown id.
    pub fn mark_cancelled(&self, job_id: &str, message: &str) -> bool {
        if let Some(job) = self.jobs.read().get(job_id) {
            let event = ProgressEvent::new(
                job_id.to_string(),
                job.job_type.clone(),
                ProgressState::Cancelled,
                0.0,
                message,
            );
            *job.last_event.lock() = Some(event.clone());
            let _ = job.sender.send(event);
            true
        } else {
            false
        }
    }

    /// Create a new job and return its ID.
    pub fn create_job(&self, job_type: JobType, initial_message: &str) -> String {
        self.create_job_with_owner(job_type, initial_message, None)
    }

    /// Create a new job attributed to `owner` and return its ID.
    ///
    /// `owner = None` marks a system- or admin-triggered job (every admin sees
    /// it in the activity feed; non-admins never do). `owner = Some(username)`
    /// scopes the job to that user so their file paths don't leak to others.
    pub fn create_job_with_owner(
        &self,
        job_type: JobType,
        initial_message: &str,
        owner: Option<String>,
    ) -> String {
        let job_id = Uuid::new_v4().to_string();
        self.create_job_with_id_and_owner(job_id, job_type, initial_message, owner)
    }

    /// Create a job under an **id the caller chooses**.
    ///
    /// Every other constructor mints a `Uuid::new_v4()`, which is right for a
    /// handler-started job (each click is a new job) and wrong for a job that
    /// describes a *state of the world*. The activity poller (ROADMAP 28.3)
    /// needs the second kind: one mdadm resync must be **one** row that
    /// advances, so its id is derived from the source
    /// (`sys:raid:md0:resync`). Minting a uuid there meant every
    /// `emit_progress(&op.id, …)` addressed an id that was never inserted — the
    /// row silently never existed, which is exactly the class of bug a
    /// `debug_assert` in `apply_plan` was standing in for.
    ///
    /// Idempotent: an id already tracked is left as it is and returned, so a
    /// caller that races its own poll cannot reset a live row to 0 %.
    pub fn create_job_with_id(
        &self,
        job_id: String,
        job_type: JobType,
        initial_message: &str,
    ) -> String {
        self.create_job_with_id_and_owner(job_id, job_type, initial_message, None)
    }

    fn create_job_with_id_and_owner(
        &self,
        job_id: String,
        job_type: JobType,
        initial_message: &str,
        owner: Option<String>,
    ) -> String {
        // Idempotent insert: an id that is already tracked keeps its state and
        // its retained event rather than being reset to the seed.
        if self.jobs.read().contains_key(&job_id) {
            return job_id;
        }

        let (tx, _) = broadcast::channel(100);

        let initial_event = ProgressEvent::new(
            job_id.clone(),
            job_type.clone(),
            ProgressState::Running,
            0.0,
            initial_message,
        );

        let _ = tx.send(initial_event.clone());

        let job_info = JobInfo {
            job_type,
            sender: tx,
            created_at: Utc::now(),
            state: ProgressState::Pending,
            last_event: Mutex::new(Some(initial_event)),
            has_progress: AtomicBool::new(false),
            owner,
        };

        self.jobs.write().insert(job_id.clone(), job_info);
        job_id
    }

    /// Emit progress for a specific job
    pub fn emit_progress(&self, job_id: &str, progress: f64, message: &str) -> bool {
        if let Some(job) = self.jobs.read().get(job_id) {
            let event = ProgressEvent::new(
                job_id.to_string(),
                job.job_type.clone(),
                ProgressState::Running,
                progress,
                message,
            );
            *job.last_event.lock() = Some(event.clone());
            job.has_progress.store(true, Ordering::Relaxed);
            job.sender.send(event).is_ok()
        } else {
            false
        }
    }

    /// Emit progress for a job that **cannot report a percentage**, keeping it
    /// indeterminate.
    ///
    /// [`Self::emit_progress`] always marks the job as having reported a
    /// percentage, so re-emitting it for an operation whose source has no
    /// number (a `zpool scrub` that has not estimated yet, an mdadm resync the
    /// kernel reports as `DELAYED`) turned an honest "indeterminate" row into
    /// a fake `0 %` that then never moved. The activity poller (ROADMAP 28.3)
    /// is the caller: it advances a row's *message* every tick while its
    /// percentage is `None`, and the feed must keep answering `progress: null`
    /// (a sliding bar, no `%`) for as long as the source says nothing.
    pub fn emit_message_only(&self, job_id: &str, message: &str) -> bool {
        if let Some(job) = self.jobs.read().get(job_id) {
            // Keep the *last percentage this job reported*: writing a bare
            // `0.0` here made a row that had reached 29 % fall back to "0 %"
            // on the next tick, because `list_activity` answers with
            // `last_event.progress` whenever `has_progress` is set. A source
            // that stops reporting a number does not mean the work went back
            // to the start.
            let previous = job
                .last_event
                .lock()
                .as_ref()
                .map(|e| e.progress)
                .unwrap_or(0.0);
            let event = ProgressEvent::new(
                job_id.to_string(),
                job.job_type.clone(),
                ProgressState::Running,
                previous,
                message,
            );
            *job.last_event.lock() = Some(event.clone());
            // `has_progress` stays as it was: a job that *did* report a number
            // once keeps answering with it, and one that never did keeps
            // answering `null`.
            job.sender.send(event).is_ok()
        } else {
            false
        }
    }

    /// Emit progress with additional metadata
    pub fn emit_progress_extended(
        &self,
        job_id: &str,
        progress: f64,
        message: &str,
        bytes_processed: Option<u64>,
        bytes_total: Option<u64>,
        speed_bps: Option<u64>,
    ) -> bool {
        if let Some(job) = self.jobs.read().get(job_id) {
            let event = ProgressEvent {
                job_id: job_id.to_string(),
                job_type: job.job_type.clone(),
                state: ProgressState::Running,
                progress: progress.clamp(0.0, 100.0),
                message: message.to_string(),
                bytes_processed,
                bytes_total,
                speed_bps,
                timestamp: Utc::now(),
            };
            *job.last_event.lock() = Some(event.clone());
            job.has_progress.store(true, Ordering::Relaxed);
            job.sender.send(event).is_ok()
        } else {
            false
        }
    }

    /// Mark a job as completed
    pub fn complete_job(&self, job_id: &str, message: &str) -> bool {
        if let Some(job) = self.jobs.read().get(job_id) {
            let event = ProgressEvent::new(
                job_id.to_string(),
                job.job_type.clone(),
                ProgressState::Completed,
                100.0,
                message,
            );
            *job.last_event.lock() = Some(event.clone());
            let _ = job.sender.send(event);
            true
        } else {
            false
        }
    }

    /// Mark a job as failed
    pub fn fail_job(&self, job_id: &str, error: &str) -> bool {
        if let Some(job) = self.jobs.read().get(job_id) {
            let event = ProgressEvent::new(
                job_id.to_string(),
                job.job_type.clone(),
                ProgressState::Failed,
                0.0,
                &format!("Failed: {}", error),
            );
            *job.last_event.lock() = Some(event.clone());
            let _ = job.sender.send(event);
            true
        } else {
            false
        }
    }

    /// Get the current state of a job: its **last emitted event**.
    ///
    /// A polling client (`GET /api/progress/{job_id}/status`) gets the same
    /// snapshot the activity feed and the SSE stream carry — the latest
    /// percentage, message, byte counters and **state**, completed and failed
    /// included.
    ///
    /// This previously synthesised a brand-new `Running` / `0 %` /
    /// `"Waiting..."` event for whatever id happened to be in the map, so a
    /// job that had completed, failed, or reported 87 % still read back as
    /// "running, 0 %" for as long as it stayed tracked. Nothing consumed the
    /// placeholder, which is why it went unnoticed.
    ///
    /// `None` when the id is unknown — never created, or already dropped by
    /// [`Self::remove_job`] / [`Self::cleanup_old_jobs`].
    pub fn get_progress(&self, job_id: &str) -> Option<ProgressEvent> {
        self.jobs.read().get(job_id).map(|job| {
            // `last_event` is seeded by `create_job*` and only ever
            // overwritten, so the fallback below is unreachable in practice;
            // it keeps the create-time seed honest rather than inventing a
            // state, should the field ever become optional for real.
            let seed_state = job.state.clone();
            let seed_type = job.job_type.clone();
            job.last_event.lock().clone().unwrap_or_else(|| {
                ProgressEvent::new(job_id.to_string(), seed_type, seed_state, 0.0, "")
            })
        })
    }

    /// Remove a job (completed or abandoned) and drop any stale cancel hook.
    pub fn remove_job(&self, job_id: &str) {
        self.jobs.write().remove(job_id);
        self.cancel_hooks.lock().remove(job_id);
    }

    /// Get all tracked jobs (ID, type, state).
    pub fn list_active_jobs(&self) -> Vec<(String, JobType, ProgressState)> {
        self.jobs
            .read()
            .iter()
            .map(|(id, job)| (id.clone(), job.job_type.clone(), effective_state(job)))
            .collect()
    }

    /// Snapshot every tracked job for the unified activity feed — carries the
    /// live state, the last-reported percentage (or `None` when indeterminate),
    /// the latest message, byte counters, speed, and the owning username.
    pub fn list_activity(&self) -> Vec<ActivitySnapshot> {
        self.jobs
            .read()
            .iter()
            .map(|(id, job)| {
                let last = job.last_event.lock();
                let has_progress = job.has_progress.load(Ordering::Relaxed);
                ActivitySnapshot {
                    id: id.clone(),
                    job_type: job.job_type.clone(),
                    owner: job.owner.clone(),
                    state: last
                        .as_ref()
                        .map(|e| e.state.clone())
                        .unwrap_or_else(|| job.state.clone()),
                    updated_at: last.as_ref().map(|e| e.timestamp),
                    progress: if has_progress {
                        last.as_ref().map(|e| e.progress)
                    } else {
                        None
                    },
                    message: last.as_ref().map(|e| e.message.clone()).unwrap_or_default(),
                    bytes_processed: last.as_ref().and_then(|e| e.bytes_processed),
                    bytes_total: last.as_ref().and_then(|e| e.bytes_total),
                    speed_bps: last.as_ref().and_then(|e| e.speed_bps),
                }
            })
            .collect()
    }

    /// Subscribe to progress updates for a job.
    pub fn get_receiver(&self, job_id: &str) -> Option<broadcast::Receiver<ProgressEvent>> {
        self.jobs
            .read()
            .get(job_id)
            .map(|job| job.sender.subscribe())
    }

    /// Remove completed/failed jobs older than `max_age` (call periodically).
    pub fn cleanup_old_jobs(&self, max_age: Duration) {
        let now = Utc::now();
        let mut jobs = self.jobs.write();
        jobs.retain(|_, job| {
            let age = now.signed_duration_since(job.created_at);
            let age_duration = Duration::from_secs(age.num_seconds().max(0) as u64);
            // Never prune a job that is still going, however old it is
            // (`job.state` alone is only the create-time seed).
            effective_state(job) == ProgressState::Running || age_duration < max_age
        });
        // Drop hooks whose job is gone, so a long-lived process doesn't
        // accumulate closures holding child PIDs.
        let live: std::collections::HashSet<String> = jobs.keys().cloned().collect();
        drop(jobs);
        self.cancel_hooks.lock().retain(|id, _| live.contains(id));
    }
}

/// Live state of a job: the last emitted event's state, falling back to the
/// create-time seed when nothing has been emitted yet.
fn effective_state(job: &JobInfo) -> ProgressState {
    job.last_event
        .lock()
        .as_ref()
        .map(|e| e.state.clone())
        .unwrap_or_else(|| job.state.clone())
}

/// Create a new (empty) progress manager.
pub fn create_progress_manager() -> Arc<ProgressManager> {
    Arc::new(ProgressManager::new())
}

/// Global progress manager instance
pub static PROGRESS_MANAGER: std::sync::LazyLock<Arc<ProgressManager>> =
    std::sync::LazyLock::new(create_progress_manager);

/// Convenience wrapper around the global [`PROGRESS_MANAGER`] singleton.
pub struct ProgressEmitter;

impl ProgressEmitter {
    /// Create a new cache mover job
    pub fn cache_mover() -> String {
        PROGRESS_MANAGER.create_job(JobType::CacheMover, "Starting cache mover")
    }

    /// Create a new backup job
    pub fn backup(source: &str, destination: &str) -> String {
        let message = format!("Starting backup: {} -> {}", source, destination);
        PROGRESS_MANAGER.create_job(JobType::Backup, &message)
    }

    /// Create a new restore job
    pub fn restore(backup_name: &str, destination: &str) -> String {
        let message = format!("Starting restore: {} -> {}", backup_name, destination);
        PROGRESS_MANAGER.create_job(JobType::Restore, &message)
    }

    /// Create a new verify job
    pub fn verify(backup_name: &str) -> String {
        let message = format!("Verifying backup: {}", backup_name);
        PROGRESS_MANAGER.create_job(JobType::Verify, &message)
    }

    /// Emit progress update
    pub fn progress(job_id: &str, progress: f64, message: &str) -> bool {
        PROGRESS_MANAGER.emit_progress(job_id, progress, message)
    }

    /// Emit progress with metadata
    pub fn progress_extended(
        job_id: &str,
        progress: f64,
        message: &str,
        bytes_processed: Option<u64>,
        bytes_total: Option<u64>,
        speed_bps: Option<u64>,
    ) -> bool {
        PROGRESS_MANAGER.emit_progress_extended(
            job_id,
            progress,
            message,
            bytes_processed,
            bytes_total,
            speed_bps,
        )
    }

    /// Mark job as completed
    pub fn complete(job_id: &str, message: &str) -> bool {
        PROGRESS_MANAGER.complete_job(job_id, message)
    }

    /// Mark job as failed
    pub fn fail(job_id: &str, error: &str) -> bool {
        PROGRESS_MANAGER.fail_job(job_id, error)
    }

    /// Mark a job as cancelled (its producer observed the stop request and
    /// unwound). Distinct from `fail`: the UI shows "cancelled", not an error.
    pub fn cancelled(job_id: &str, message: &str) -> bool {
        PROGRESS_MANAGER.mark_cancelled(job_id, message)
    }

    /// Register the producer's cancellation hook — see
    /// [`ProgressManager::register_cancel_hook`].
    pub fn register_cancel_hook(job_id: &str, hook: crate::sse::CancelHook) {
        PROGRESS_MANAGER.register_cancel_hook(job_id, hook)
    }

    /// Drop the producer's cancellation hook once its work is over, so the
    /// registry does not keep a closure (and its captured job id) alive for
    /// every finished backup.
    pub fn clear_cancel_hook(job_id: &str) {
        PROGRESS_MANAGER.clear_cancel_hook(job_id)
    }

    /// Get current progress
    pub fn get(job_id: &str) -> Option<ProgressEvent> {
        PROGRESS_MANAGER.get_progress(job_id)
    }

    /// Remove a job
    pub fn remove(job_id: &str) {
        PROGRESS_MANAGER.remove_job(job_id)
    }
}

#[cfg(test)]
mod tests {
    //! Pure in-process tests — each builds its own `ProgressManager` (never the
    //! global singleton) so they stay isolated and deterministic.
    //! Run: `cargo test sse::progress::tests`
    use super::*;

    fn snap<'a>(list: &'a [ActivitySnapshot], id: &str) -> &'a ActivitySnapshot {
        list.iter()
            .find(|s| s.id == id)
            .expect("snapshot for job id")
    }

    #[test]
    fn test_create_job_seeds_last_event_pending() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "Starting backup");
        let list = m.list_activity();
        let s = snap(&list, &id);
        // Seeded, running, but no percentage reported yet → indeterminate.
        assert_eq!(s.state, ProgressState::Running);
        assert_eq!(s.progress, None);
        assert_eq!(s.message, "Starting backup");
    }

    #[test]
    fn test_emit_progress_updates_last_event() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        // `emit_progress`'s bool return means "delivered to a live SSE
        // subscriber", not "job exists" — irrelevant here; we only care that
        // the retained last-event advanced.
        m.emit_progress(&id, 42.0, "42/100 files");
        let list = m.list_activity();
        let s = snap(&list, &id);
        assert_eq!(s.progress, Some(42.0));
        assert_eq!(s.message, "42/100 files");
        assert_eq!(s.state, ProgressState::Running);
    }

    /// ROADMAP 28.3 — a job that cannot report a percentage stays
    /// indeterminate when its message advances.
    ///
    /// `emit_progress` flags the job as having reported a percentage (it is
    /// what tells `0 %` apart from "nothing reported yet"), so the activity
    /// poller re-emitting a no-number source through it turned an honest
    /// indeterminate row into a fake 0 % that then never moved.
    /// `emit_message_only` advances the message without claiming a number.
    #[test]
    fn emit_message_only_keeps_a_job_indeterminate() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Custom("raid_resync".into()), "resync");
        assert_eq!(snap(&m.list_activity(), &id).progress, None);

        // Three ticks of a source that has no number: still null, never 0.
        for i in 0..3 {
            m.emit_message_only(&id, &format!("resync {i}"));
            let list = m.list_activity();
            let s = snap(&list, &id);
            assert_eq!(
                s.progress, None,
                "tick {i}: a no-number source must stay indeterminate"
            );
        }
        assert_eq!(snap(&m.list_activity(), &id).message, "resync 2");
    }

    /// A job that already reported a number keeps it when a later tick has no
    /// number — the bar must not fall back to indeterminate mid-operation.
    #[test]
    fn emit_message_only_does_not_erase_a_known_percentage() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        m.emit_progress(&id, 29.4, "29 %");
        m.emit_message_only(&id, "still going");
        let list = m.list_activity();
        let s = snap(&list, &id);
        assert_eq!(s.progress, Some(29.4));
        assert_eq!(s.message, "still going");
    }

    #[test]
    fn test_complete_job_last_event_completed_100() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Verify, "start");
        m.emit_progress(&id, 10.0, "checking");
        assert!(m.complete_job(&id, "done"));
        let list = m.list_activity();
        let s = snap(&list, &id);
        assert_eq!(s.state, ProgressState::Completed);
        assert_eq!(s.progress, Some(100.0));
    }

    #[test]
    fn test_fail_job_last_event_failed() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Restore, "start");
        assert!(m.fail_job(&id, "disk full"));
        let list = m.list_activity();
        let s = snap(&list, &id);
        assert_eq!(s.state, ProgressState::Failed);
        assert!(s.message.contains("disk full"));
    }

    #[test]
    fn test_list_activity_none_progress_before_first_emit() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Custom("docker_install".into()), "Installing app");
        assert_eq!(snap(&m.list_activity(), &id).progress, None);
    }

    #[test]
    fn test_list_activity_some_progress_after_emit() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::CacheMover, "start");
        m.emit_progress(&id, 0.0, "0 %");
        // 0 % is a real reported value, not "indeterminate".
        assert_eq!(snap(&m.list_activity(), &id).progress, Some(0.0));
    }

    #[test]
    fn test_list_activity_pending_and_running_only() {
        // list_activity() returns every job; the /api/activity handler is what
        // filters to Pending|Running. Here we assert the state each job reports.
        let m = ProgressManager::new();
        let running = m.create_job(JobType::Backup, "r");
        let done = m.create_job(JobType::Backup, "d");
        m.complete_job(&done, "ok");
        let list = m.list_activity();
        assert_eq!(snap(&list, &running).state, ProgressState::Running);
        assert_eq!(snap(&list, &done).state, ProgressState::Completed);
    }

    #[test]
    fn test_create_job_with_owner_sets_owner() {
        let m = ProgressManager::new();
        let id = m.create_job_with_owner(
            JobType::Custom("explorer_copy".into()),
            "Starting copy",
            Some("alice".to_string()),
        );
        assert_eq!(
            snap(&m.list_activity(), &id).owner.as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn test_create_job_defaults_owner_none() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        assert_eq!(snap(&m.list_activity(), &id).owner, None);
    }

    #[test]
    fn test_job_type_round_trips_through_json_for_every_variant() {
        // `Custom` used to be unserialisable (see the note on `JobType`), which
        // silently emptied the SSE payload and panicked `GET /api/progress/jobs`.
        let variants = [
            JobType::Backup,
            JobType::Restore,
            JobType::Verify,
            JobType::Upload,
            JobType::Download,
            JobType::Sync,
            JobType::CacheMover,
            JobType::Custom("explorer_copy".to_string()),
            JobType::default(),
        ];
        for variant in variants {
            let json = serde_json::to_string(&variant)
                .unwrap_or_else(|e| panic!("{variant:?} must serialize: {e}"));
            let back: JobType = serde_json::from_str(&json).expect("and deserialize");
            assert_eq!(back, variant, "round-trip through {json}");
        }
    }

    #[test]
    fn test_job_type_wire_shape_of_builtin_kinds_is_unchanged() {
        assert_eq!(
            serde_json::to_string(&JobType::Backup).unwrap(),
            r#"{"type":"backup"}"#
        );
        assert_eq!(
            serde_json::to_string(&JobType::CacheMover).unwrap(),
            r#"{"type":"cache_mover"}"#
        );
    }

    #[test]
    fn test_job_type_custom_carries_its_slug() {
        assert_eq!(
            serde_json::to_string(&JobType::Custom("explorer_copy".to_string())).unwrap(),
            r#"{"type":"custom","name":"explorer_copy"}"#
        );
    }

    #[test]
    fn test_progress_event_of_a_custom_job_serializes() {
        // Exactly what `handle_progress_sse` does before handing the payload to
        // the browser, and what `GET /api/progress/{id}/status` returns.
        let event = ProgressEvent::new(
            "job-1".to_string(),
            JobType::Custom("explorer_copy".to_string()),
            ProgressState::Running,
            42.0,
            "42/100 files",
        );
        let json = serde_json::to_string(&event).expect("a Custom-job event must serialize");
        assert!(
            !json.is_empty(),
            "the SSE data payload must not come out empty"
        );
        let back: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.job_type, JobType::Custom("explorer_copy".to_string()));
        assert_eq!(back.progress, 42.0);
        assert_eq!(back.message, "42/100 files");
    }

    #[test]
    fn test_get_progress_returns_the_last_event_not_a_placeholder() {
        // The regression: this used to answer `Running` / 0 % / "Waiting..."
        // for *any* tracked id, discarding the real last event.
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Sync, "Starting sync");
        m.emit_progress_extended(
            &id,
            87.5,
            "87.5 % of 4.2 GiB",
            Some(3_950_000_000),
            Some(4_509_715_660),
            Some(12_000_000),
        );

        let event = m.get_progress(&id).expect("tracked job must be readable");
        assert_eq!(event.state, ProgressState::Running);
        assert_eq!(event.progress, 87.5);
        assert_eq!(event.message, "87.5 % of 4.2 GiB");
        assert_eq!(event.job_type, JobType::Sync);
        assert_eq!(event.bytes_processed, Some(3_950_000_000));
        assert_eq!(event.bytes_total, Some(4_509_715_660));
        assert_eq!(event.speed_bps, Some(12_000_000));
        assert_eq!(event.job_id, id);
    }

    #[test]
    fn test_get_progress_seeded_event_before_any_emit() {
        // Before the first `emit_progress*`, the answer is the create-time
        // seed — the caller's own initial message, not a generic placeholder.
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "Starting backup: /a -> /b");
        let event = m.get_progress(&id).unwrap();
        assert_eq!(event.state, ProgressState::Running);
        assert_eq!(event.progress, 0.0);
        assert_eq!(event.message, "Starting backup: /a -> /b");
        assert_eq!(event.job_type, JobType::Backup);
    }

    #[test]
    fn test_get_progress_reports_completion() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Restore, "start");
        m.emit_progress(&id, 60.0, "restoring");
        assert!(m.complete_job(&id, "Restore finished"));

        let event = m.get_progress(&id).unwrap();
        assert_eq!(event.state, ProgressState::Completed);
        assert_eq!(event.progress, 100.0);
        assert_eq!(event.message, "Restore finished");
    }

    #[test]
    fn test_get_progress_reports_failure() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Verify, "start");
        assert!(m.fail_job(&id, "checksum mismatch"));

        let event = m.get_progress(&id).unwrap();
        assert_eq!(event.state, ProgressState::Failed);
        assert!(event.message.contains("checksum mismatch"));
    }

    #[test]
    fn test_get_progress_unknown_job_is_none() {
        let m = ProgressManager::new();
        assert!(m.get_progress("never-created").is_none());

        let id = m.create_job(JobType::Backup, "start");
        assert!(m.get_progress(&id).is_some());
        m.remove_job(&id);
        assert!(
            m.get_progress(&id).is_none(),
            "a removed job must report None, not a stale event"
        );
    }

    #[test]
    fn test_cleanup_keeps_running_job_past_max_age() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        // max_age = 0 → every job counts as "old"; only still-running survives.
        m.cleanup_old_jobs(Duration::from_secs(0));
        assert!(m.list_activity().iter().any(|s| s.id == id));
    }

    #[test]
    fn test_cleanup_drops_completed_job_past_max_age() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        m.complete_job(&id, "ok");
        m.cleanup_old_jobs(Duration::from_secs(0));
        assert!(!m.list_activity().iter().any(|s| s.id == id));
    }

    // -----------------------------------------------------------------------
    // Cancellation — the producer hook is what actually stops the work
    // -----------------------------------------------------------------------

    #[test]
    fn cancel_job_runs_the_registered_hook() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");

        let fired = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&fired);
        m.register_cancel_hook(&id, Box::new(move || flag.store(true, Ordering::SeqCst)));

        assert_eq!(m.cancel_job(&id), CancelOutcome::HookRan);
        assert!(fired.load(Ordering::SeqCst), "the hook must run");
        // Marked cancelled on the stream, and still tracked so the producer
        // can report its own final state.
        let event = m.get_progress(&id).expect("job still tracked");
        assert_eq!(event.state, ProgressState::Cancelled);
    }

    #[test]
    fn cancel_job_of_an_unknown_id_is_not_found() {
        let m = ProgressManager::new();
        assert_eq!(m.cancel_job("never-created"), CancelOutcome::NotFound);
    }

    #[test]
    fn cancel_job_without_a_hook_still_drops_the_job() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        assert_eq!(m.cancel_job(&id), CancelOutcome::NoHook);
        assert!(
            m.get_progress(&id).is_none(),
            "a hook-less job has nothing to stop — it is simply dropped"
        );
    }

    #[test]
    fn cancel_job_runs_the_hook_only_once() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");

        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        m.register_cancel_hook(
            &id,
            Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        );

        assert_eq!(m.cancel_job(&id), CancelOutcome::HookRan);
        assert_eq!(m.cancel_job(&id), CancelOutcome::NoHook);
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "a taken hook must not run twice"
        );
    }

    #[test]
    fn remove_job_drops_a_registered_hook() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");

        let fired = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&fired);
        m.register_cancel_hook(&id, Box::new(move || flag.store(true, Ordering::SeqCst)));

        m.remove_job(&id);
        assert_eq!(m.cancel_job(&id), CancelOutcome::NotFound);
        assert!(!fired.load(Ordering::SeqCst));
    }

    #[test]
    fn mark_cancelled_reports_unknown_jobs() {
        let m = ProgressManager::new();
        assert!(!m.mark_cancelled("nope", "Cancelled"));
        let id = m.create_job(JobType::Backup, "start");
        assert!(m.mark_cancelled(&id, "Cancelled"));
        assert_eq!(m.get_progress(&id).unwrap().state, ProgressState::Cancelled);
    }
}
