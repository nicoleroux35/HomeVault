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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    pub fn new(job_id: String, job_type: JobType, state: ProgressState, progress: f64, message: &str) -> Self {
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
    /// `None` when no percentage has been reported yet (indeterminate).
    pub progress: Option<f64>,
    pub message: String,
    pub bytes_processed: Option<u64>,
    pub bytes_total: Option<u64>,
    pub speed_bps: Option<u64>,
}

/// Central registry for all active progress-tracking jobs.
pub struct ProgressManager {
    jobs: RwLock<HashMap<String, JobInfo>>,
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

    /// Get current progress for a job
    pub fn get_progress(&self, job_id: &str) -> Option<ProgressEvent> {
        self.jobs.read().get(job_id).map(|_| {
            ProgressEvent::new(
                job_id.to_string(),
                JobType::default(),
                ProgressState::Running,
                0.0,
                "Waiting...",
            )
        })
    }

    /// Remove a job (completed or abandoned).
    pub fn remove_job(&self, job_id: &str) {
        self.jobs.write().remove(job_id);
    }

    /// Get all tracked jobs (ID, type, state).
    pub fn list_active_jobs(&self) -> Vec<(String, JobType, ProgressState)> {
        self.jobs.read()
            .iter()
            .map(|(id, job)| (id.clone(), job.job_type.clone(), effective_state(job)))
            .collect()
    }

    /// Snapshot every tracked job for the unified activity feed — carries the
    /// live state, the last-reported percentage (or `None` when indeterminate),
    /// the latest message, byte counters, speed, and the owning username.
    pub fn list_activity(&self) -> Vec<ActivitySnapshot> {
        self.jobs.read()
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
        self.jobs.read().get(job_id).map(|job| job.sender.subscribe())
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
        PROGRESS_MANAGER.emit_progress_extended(job_id, progress, message, bytes_processed, bytes_total, speed_bps)
    }

    /// Mark job as completed
    pub fn complete(job_id: &str, message: &str) -> bool {
        PROGRESS_MANAGER.complete_job(job_id, message)
    }

    /// Mark job as failed
    pub fn fail(job_id: &str, error: &str) -> bool {
        PROGRESS_MANAGER.fail_job(job_id, error)
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
        list.iter().find(|s| s.id == id).expect("snapshot for job id")
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
        assert_eq!(snap(&m.list_activity(), &id).owner.as_deref(), Some("alice"));
    }

    #[test]
    fn test_create_job_defaults_owner_none() {
        let m = ProgressManager::new();
        let id = m.create_job(JobType::Backup, "start");
        assert_eq!(snap(&m.list_activity(), &id).owner, None);
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
}
