/* =============================================================================
 * File          : restic.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Backup engine service wrapping the restic CLI: repository
 *   initialisation and credential handling (password supplied per run,
 *   never stored in plaintext), snapshot listing, and the backup/restore/
 *   check/forget/prune operations with their typed results. Every argument
 *   that reaches `restic`'s argv is validated first, long runs stream their
 *   `--json` status to a progress callback and can be cancelled, and the
 *   blocking calls are dispatched off the async runtime by
 *   [`ResticService::run_restic_op`].
 *
 * Dependencies:
 *   - chrono (>= 0.4), serde (>= 1.0), libc (>= 0.2)
 *
 * Usage:
 *   handlers/restic.rs (manual operations) and the backup scheduler
 *   (scheduled runs); progress flows through the SSE progress manager.
 * =============================================================================
 */

//! Backup engine service wrapping the restic CLI (repositories, snapshots,
//! backup/restore/check/forget/prune).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use thiserror::Error;
use tracing::info;

/// Ceiling for a metadata call (`snapshots`, `stats`, `diff`, `find`,
/// `unlock`, a snapshot forget). These read the repository index, never the
/// data, so a minute is already generous — and a hung `restic` (an
/// unreachable SFTP/S3 backend) must not pin a blocking thread forever.
pub const RESTIC_QUERY_TIMEOUT: Duration = Duration::from_secs(120);

/// Ceiling for `restic init`, which on a remote backend creates a config
/// object and a set of pack files over the network.
pub const RESTIC_INIT_TIMEOUT: Duration = Duration::from_secs(300);

/// A path, repository or pattern is never longer than this. Guards the DB
/// columns and keeps a pathological request out of argv.
pub const MAX_PATH_LEN: usize = 4096;

/// Longest snapshot id we accept. A full restic id is 64 hex chars; short
/// ids are 8+.
pub const MAX_SNAPSHOT_ID_LEN: usize = 128;

#[derive(Error, Debug)]
pub enum ResticError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Restic CLI not found")]
    NotFound,
    #[error("Repository not initialized: {0}")]
    NotInitialized(String),
    #[error("Restic command failed: {0}")]
    CommandFailed(String),
    #[error("JSON parse error: {0}")]
    ParseError(String),
    #[error("Snapshot error: {0}")]
    SnapError(#[from] crate::snapshots::SnapError),
    #[error("Backup not found: {0}")]
    BackupNotFound(String),
    /// The run was stopped on request (`DELETE /api/progress/{job_id}`).
    #[error("Cancelled")]
    Cancelled,
    /// An argument was refused before `restic` was spawned.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticRepo {
    pub id: String,
    pub name: String,
    pub repository: String,
    pub password: String,
    pub retention_policy_id: Option<String>,
    pub source_paths: Vec<String>,
    pub exclude_patterns: Option<String>,
    pub tags: Option<String>,
    pub snapshot_source: String,
    pub snapshot_pool: Option<String>,
    pub snapshot_dataset: Option<String>,
    pub snapshot_create_before_backup: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticSnapshot {
    pub id: String,
    pub short_id: String,
    pub time: String,
    pub tree: String,
    pub paths: Vec<String>,
    pub hostname: String,
    pub program_version: String,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResticBackupResult {
    pub snapshot_id: String,
    pub files_new: u64,
    pub files_changed: u64,
    pub files_unmodified: u64,
    pub data_added_bytes: u64,
    pub total_files_processed: u64,
    pub total_bytes: u64,
    pub duration_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticStats {
    pub total_size: u64,
    pub total_file_count: u64,
    pub snapshots_count: u64,
    pub total_blob_count: u64,
    pub compressed_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResticCheckResult {
    pub healthy: bool,
    pub message: String,
    pub errors: Vec<String>,
}

/// One `{"message_type":"status"}` line from `restic backup --json`.
///
/// This struct existed but was never produced before 2026-10-21: the backup
/// was buffered with `Command::output()` and the UI jumped from 5 % to 90 %,
/// so a two-hour run looked frozen. [`ResticService::create_backup_streaming`]
/// now emits one of these per status line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResticProgress {
    pub percent: f64,
    pub message: String,
    pub current_files: u64,
    pub total_files: u64,
    pub bytes_done: u64,
    pub total_bytes: u64,
}

/// One decoded line of `restic backup --json` output.
#[derive(Debug, Clone, PartialEq)]
pub enum BackupEvent {
    /// A progress tick (`message_type: "status"`).
    Status(ResticProgress),
    /// The terminal `message_type: "summary"` object.
    Summary(ResticBackupResult),
    /// `message_type: "error"` — restic reported a per-file error but kept
    /// going. Collected and surfaced; a summary still follows.
    Error(String),
    /// A line that carries nothing we act on (`verbose`, blank, non-JSON).
    Ignored,
}

// ---------------------------------------------------------------------------
// Cancellable child registry
// ---------------------------------------------------------------------------

/// A restic child process that a progress job can kill.
struct RunningRun {
    child: Mutex<Option<Child>>,
    cancelled: AtomicBool,
}

/// Live restic runs keyed by progress job id, so
/// `DELETE /api/progress/{job_id}` reaches the process. Without this the
/// progress manager dropped the job from its map and answered "cancelled"
/// while restic kept writing to the repository.
static RUNNING_RUNS: LazyLock<Mutex<HashMap<String, Arc<RunningRun>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn running_runs() -> std::sync::MutexGuard<'static, HashMap<String, Arc<RunningRun>>> {
    RUNNING_RUNS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Repositories with a backup/restore in flight.
///
/// restic takes an exclusive lock inside the repository, so a second run
/// against the same repo fails with "repository is already locked" — an
/// obscure message, and only *after* the second run has already done its
/// scan. Refusing up front is both clearer and cheaper. Two runs against
/// *different* repositories stay allowed.
static ACTIVE_REPOS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Held for the duration of a backup/restore on one repository; releases the
/// slot when dropped (including on a panic or an early return).
pub struct RepoRunLock {
    repo_id: String,
}

impl RepoRunLock {
    /// `None` when another run already owns this repository.
    pub fn acquire(repo_id: &str) -> Option<Self> {
        let mut set = ACTIVE_REPOS.lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(repo_id.to_string()) {
            return None;
        }
        Some(Self {
            repo_id: repo_id.to_string(),
        })
    }

    pub fn is_busy(repo_id: &str) -> bool {
        ACTIVE_REPOS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(repo_id)
    }
}

impl Drop for RepoRunLock {
    fn drop(&mut self) {
        ACTIVE_REPOS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.repo_id);
    }
}

/// A spawned restic process plus its captured stderr, registered so it can be
/// cancelled. Dropping it without [`Tracked::wait`] kills and reaps the child,
/// so an error path can never leak an orphan restic.
struct Tracked {
    key: String,
    run: Arc<RunningRun>,
    stdout: Option<ChildStdout>,
    stderr: Option<std::thread::JoinHandle<String>>,
}

impl Tracked {
    fn spawn(mut cmd: Command, job_id: Option<&str>) -> Result<Self, ResticError> {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take();
        // restic writes its error report to stderr; drain it on its own thread
        // or a full pipe buffer would deadlock both sides.
        let stderr = child.stderr.take().map(|err| {
            std::thread::spawn(move || {
                let mut buf = String::new();
                let _ = BufReader::new(err).read_to_string(&mut buf);
                buf
            })
        });
        let run = Arc::new(RunningRun {
            child: Mutex::new(Some(child)),
            cancelled: AtomicBool::new(false),
        });
        let key = match job_id {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("restic-anon-{}", uuid::Uuid::new_v4()),
        };
        running_runs().insert(key.clone(), run.clone());
        Ok(Self {
            key,
            run,
            stdout,
            stderr,
        })
    }

    /// True once a cancellation reached this run (checked after the exit).
    fn cancelled(&self) -> bool {
        self.run.cancelled.load(Ordering::SeqCst)
    }

    /// Wait for the process and return `(exit_ok, stderr, cancelled)`.
    fn wait(mut self) -> Result<(bool, String, bool), ResticError> {
        let status = {
            let mut slot = self.run.child.lock().unwrap_or_else(|e| e.into_inner());
            match slot.take() {
                Some(mut child) => child.wait()?,
                None => return Err(ResticError::Cancelled),
            }
        };
        let cancelled = self.cancelled();
        running_runs().remove(&self.key);
        let stderr = self
            .stderr
            .take()
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        Ok((status.success(), stderr, cancelled))
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let mut slot = self.run.child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(slot);
        running_runs().remove(&self.key);
    }
}

pub struct ResticService;

impl ResticService {
    fn restic_cmd() -> Result<Command, ResticError> {
        let which = Command::new("which").arg("restic").output();
        match which {
            Ok(out) if out.status.success() => Ok(Command::new("restic")),
            _ => Err(ResticError::NotFound),
        }
    }

    fn set_env(cmd: &mut Command, repo: &ResticRepo) {
        cmd.env("RESTIC_REPOSITORY", &repo.repository);
        cmd.env("RESTIC_PASSWORD", &repo.password);
        // restic honours RESTIC_CACHE_DIR; pinning it keeps a backup from
        // filling a read-only or tiny $HOME on an appliance.
        if std::env::var_os("RESTIC_CACHE_DIR").is_none() {
            cmd.env("RESTIC_CACHE_DIR", crate::config::paths::restic_cache_dir());
        }
    }

    // -----------------------------------------------------------------------
    // Argument validation
    //
    // Every one of these guards a value that would otherwise reach restic's
    // argv. `restic forget <id>` with `id = "--keep-last=0"` is a full
    // repository wipe, and `backup --exclude` style arguments are positional
    // in the `backup` command — so an unvalidated value is an argument
    // injection, not a cosmetic bug.
    // -----------------------------------------------------------------------

    /// A snapshot id (`latest`, a short id, or a full 64-char id).
    ///
    /// Refuses anything that could be read as an option or a path: a leading
    /// `-`, a `/`, a control character, `.`/`..`, or an over-long value.
    pub fn validate_snapshot_id(id: &str) -> Result<(), ResticError> {
        let v = id.trim();
        if v.is_empty() {
            return Err(ResticError::InvalidInput(
                "A snapshot id is required".to_string(),
            ));
        }
        if v.len() > MAX_SNAPSHOT_ID_LEN {
            return Err(ResticError::InvalidInput(format!(
                "Snapshot id is longer than {MAX_SNAPSHOT_ID_LEN} characters"
            )));
        }
        if v.starts_with('-') {
            return Err(ResticError::InvalidInput(
                "Snapshot id cannot start with '-'".to_string(),
            ));
        }
        if v == "." || v == ".." {
            return Err(ResticError::InvalidInput("Invalid snapshot id".to_string()));
        }
        if !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(ResticError::InvalidInput(format!(
                "Snapshot id '{v}' contains characters that are not allowed"
            )));
        }
        Ok(())
    }

    /// A filesystem path handed to restic as a source or a restore target.
    ///
    /// Must be absolute (which also makes a leading `-` impossible), free of
    /// control characters and of `..` components, and within
    /// [`MAX_PATH_LEN`].
    fn validate_absolute_path(what: &str, path: &str) -> Result<(), ResticError> {
        // Control characters are checked on the *raw* value: trimming first
        // would silently accept `"/srv/data\n"` and then hand the untrimmed
        // string to restic.
        if path.chars().any(|c| c.is_control()) {
            return Err(ResticError::InvalidInput(format!(
                "{what} contains a control character"
            )));
        }
        let p = path.trim();
        if p.is_empty() {
            return Err(ResticError::InvalidInput(format!("{what} cannot be empty")));
        }
        if p.len() > MAX_PATH_LEN {
            return Err(ResticError::InvalidInput(format!(
                "{what} is longer than {MAX_PATH_LEN} characters"
            )));
        }
        if !p.starts_with('/') {
            return Err(ResticError::InvalidInput(format!(
                "{what} '{p}' must be an absolute path"
            )));
        }
        if std::path::Path::new(p)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ResticError::InvalidInput(format!(
                "{what} '{p}' cannot contain '..'"
            )));
        }
        Ok(())
    }

    /// A `restic backup` source directory.
    pub fn validate_backup_path(path: &str) -> Result<(), ResticError> {
        Self::validate_absolute_path("Backup source path", path)
    }

    /// A `restic restore --target` directory.
    pub fn validate_restore_target(target: &str) -> Result<(), ResticError> {
        Self::validate_absolute_path("Restore target", target)
    }

    /// A filesystem snapshot name, used to build the ZFS/Btrfs snapshot and
    /// the temporary mount point `/tmp/restic-snap-<name>`.
    ///
    /// The mount point used to be built from the raw request value, so a name
    /// of `../../etc` mounted a snapshot over an arbitrary directory and then
    /// `remove_dir`'d it.
    pub fn validate_snapshot_name(name: &str) -> Result<(), ResticError> {
        let n = name.trim();
        if n.is_empty() {
            return Err(ResticError::InvalidInput(
                "A snapshot name is required".to_string(),
            ));
        }
        if n.len() > 64 {
            return Err(ResticError::InvalidInput(
                "Snapshot name is longer than 64 characters".to_string(),
            ));
        }
        if n.starts_with('-') || n.starts_with('.') {
            return Err(ResticError::InvalidInput(
                "Snapshot name cannot start with '-' or '.'".to_string(),
            ));
        }
        if !n
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
        {
            return Err(ResticError::InvalidInput(format!(
                "Snapshot name '{n}' contains characters that are not allowed"
            )));
        }
        Ok(())
    }

    /// A `RESTIC_REPOSITORY` value (a local path or a remote spec such as
    /// `sftp:user@host:/srv/backup`). It is passed through the environment,
    /// never argv, but it is still validated so a newline or a NUL cannot
    /// reach the environment block.
    pub fn validate_repository(repository: &str) -> Result<(), ResticError> {
        if repository.chars().any(|c| c.is_control()) {
            return Err(ResticError::InvalidInput(
                "Repository location contains a control character".to_string(),
            ));
        }
        let r = repository.trim();
        if r.is_empty() {
            return Err(ResticError::InvalidInput(
                "A repository location is required".to_string(),
            ));
        }
        if r.len() > 1024 {
            return Err(ResticError::InvalidInput(
                "Repository location is longer than 1024 characters".to_string(),
            ));
        }
        if r.starts_with('-') {
            return Err(ResticError::InvalidInput(
                "Repository location cannot start with '-'".to_string(),
            ));
        }
        Ok(())
    }

    /// A repository password. Non-empty, no control characters, bounded.
    pub fn validate_password(password: &str) -> Result<(), ResticError> {
        if password.is_empty() {
            return Err(ResticError::InvalidInput(
                "A repository password is required".to_string(),
            ));
        }
        if password.len() > 1024 {
            return Err(ResticError::InvalidInput(
                "Repository password is longer than 1024 characters".to_string(),
            ));
        }
        if password.chars().any(|c| c.is_control()) {
            return Err(ResticError::InvalidInput(
                "Repository password contains a control character".to_string(),
            ));
        }
        Ok(())
    }

    /// A human-facing name (repository, schedule, retention policy).
    pub fn validate_display_name(what: &str, name: &str) -> Result<(), ResticError> {
        let n = name.trim();
        if n.is_empty() {
            return Err(ResticError::InvalidInput(format!("{what} is required")));
        }
        if n.len() > 100 {
            return Err(ResticError::InvalidInput(format!(
                "{what} is longer than 100 characters"
            )));
        }
        if n.chars().any(|c| c.is_control()) {
            return Err(ResticError::InvalidInput(format!(
                "{what} contains a control character"
            )));
        }
        Ok(())
    }

    /// `--keep-within` duration: a sequence of `<number><unit>` with units
    /// `y`, `m`, `d`, `h` (restic's own grammar, e.g. `30d`, `1y2m`).
    ///
    /// An unvalidated value used to reach restic, which answered with a
    /// generic "Fatal: invalid duration" long after the caller had committed.
    pub fn validate_keep_within(value: &str) -> Result<(), ResticError> {
        let v = value.trim();
        if v.is_empty() {
            return Err(ResticError::InvalidInput(
                "keep-within cannot be empty".to_string(),
            ));
        }
        let mut digits = String::new();
        let mut units = 0usize;
        for c in v.chars() {
            if c.is_ascii_digit() {
                digits.push(c);
            } else if matches!(c, 'y' | 'm' | 'd' | 'h') {
                if digits.is_empty() {
                    return Err(ResticError::InvalidInput(format!(
                        "keep-within '{v}' has a unit without a number"
                    )));
                }
                digits.clear();
                units += 1;
            } else {
                return Err(ResticError::InvalidInput(format!(
                    "keep-within '{v}' must be a sequence like 30d or 1y2m"
                )));
            }
        }
        if !digits.is_empty() {
            return Err(ResticError::InvalidInput(format!(
                "keep-within '{v}' ends with a number and no unit"
            )));
        }
        if units == 0 {
            return Err(ResticError::InvalidInput(format!(
                "keep-within '{v}' must be a sequence like 30d or 1y2m"
            )));
        }
        Ok(())
    }

    /// Comma-separated tags: each one a short, shell-safe token. A newline in
    /// a tag would otherwise become a second `--tag` argument.
    pub fn validate_tags(tags: Option<&str>) -> Result<(), ResticError> {
        let Some(t) = tags else { return Ok(()) };
        for tag in t.split(',') {
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            if tag.len() > 64 {
                return Err(ResticError::InvalidInput(
                    "A backup tag is longer than 64 characters".to_string(),
                ));
            }
            if tag.chars().any(|c| c.is_control() || c == ' ') {
                return Err(ResticError::InvalidInput(format!(
                    "Tag '{tag}' contains a control character or a space"
                )));
            }
        }
        Ok(())
    }

    /// Newline-separated exclude patterns: no control characters, bounded
    /// count and length.
    pub fn validate_exclude_patterns(patterns: Option<&str>) -> Result<(), ResticError> {
        let Some(p) = patterns else { return Ok(()) };
        let mut count = 0usize;
        for line in p.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            count += 1;
            if count > 256 {
                return Err(ResticError::InvalidInput(
                    "More than 256 exclude patterns".to_string(),
                ));
            }
            if line.len() > MAX_PATH_LEN {
                return Err(ResticError::InvalidInput(
                    "An exclude pattern is longer than 4096 characters".to_string(),
                ));
            }
            if line.chars().any(|c| c.is_control()) {
                return Err(ResticError::InvalidInput(
                    "An exclude pattern contains a control character".to_string(),
                ));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Availability
    // -----------------------------------------------------------------------

    pub fn is_available() -> bool {
        Command::new("restic")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn get_version() -> Result<String, ResticError> {
        let output = Command::new("restic").arg("version").output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(ResticError::NotFound)
        }
    }

    // -----------------------------------------------------------------------
    // Off-runtime dispatch
    // -----------------------------------------------------------------------

    /// Run a blocking [`ResticService`] call on the blocking pool instead of a
    /// Tokio worker.
    ///
    /// Every method below shells out with `std::process::Command`. Called
    /// directly from a handler, a backup of a multi-TB dataset parked a Tokio
    /// worker for hours — and with a handful of concurrent requests the whole
    /// runtime. `timeout` bounds the metadata calls; the data-moving ones
    /// (backup, restore, check, prune) pass `None` and rely on cancellation.
    ///
    /// A timed-out call **keeps running** in the background (a blocking thread
    /// cannot be interrupted); only use a timeout on operations that are safe
    /// to abandon.
    pub async fn run_restic_op<T, F>(
        op: &str,
        timeout: Option<Duration>,
        f: F,
    ) -> Result<T, ResticError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, ResticError> + Send + 'static,
    {
        let handle = tokio::task::spawn_blocking(f);
        match timeout {
            Some(d) => match tokio::time::timeout(d, handle).await {
                Ok(joined) => joined
                    .map_err(|e| ResticError::CommandFailed(format!("{op} task panicked: {e}")))?,
                Err(_) => Err(ResticError::CommandFailed(format!(
                    "{op} did not answer within {}s",
                    d.as_secs()
                ))),
            },
            None => handle
                .await
                .map_err(|e| ResticError::CommandFailed(format!("{op} task panicked: {e}")))?,
        }
    }

    // -----------------------------------------------------------------------
    // Cancellation
    // -----------------------------------------------------------------------

    /// Stop the restic process backing progress job `job_id`.
    ///
    /// Returns `false` when no run is registered for that id (the job already
    /// finished, or it is not a restic job). SIGINT first, so restic removes
    /// its repository lock and exits cleanly; a watchdog sends SIGKILL after
    /// ten seconds if it is stuck in an uninterruptible read.
    pub fn cancel_run(job_id: &str) -> bool {
        let Some(run) = running_runs().get(job_id).cloned() else {
            return false;
        };
        run.cancelled.store(true, Ordering::SeqCst);
        let pid = {
            let mut slot = run.child.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_mut() {
                Some(child) => child.id() as i32,
                None => return true,
            }
        };
        #[cfg(unix)]
        // SAFETY: `kill` is async-signal-safe and we only pass a pid we own.
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
        #[cfg(not(unix))]
        {
            let mut slot = run.child.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(child) = slot.as_mut() {
                let _ = child.kill();
            }
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            #[cfg(unix)]
            // SAFETY: same as above; signal 0 only probes for the pid.
            unsafe {
                if libc::kill(pid, 0) == 0 {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        });
        true
    }

    /// Whether a restic run is still registered for `job_id`.
    pub fn is_run_active(job_id: &str) -> bool {
        running_runs().contains_key(job_id)
    }

    // -----------------------------------------------------------------------
    // Repositories
    // -----------------------------------------------------------------------

    pub fn init_repo(repo: &ResticRepo) -> Result<(), ResticError> {
        Self::validate_repository(&repo.repository)?;
        Self::validate_password(&repo.password)?;
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("init");

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic repository initialized: {}", repo.repository);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("already initialized") {
                info!("Restic repository already initialized: {}", repo.repository);
                return Ok(());
            }
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    // -----------------------------------------------------------------------
    // Backup
    // -----------------------------------------------------------------------

    /// `restic backup`, buffered. Kept for callers with no progress job.
    pub fn create_backup(
        repo: &ResticRepo,
        paths: &[String],
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
    ) -> Result<ResticBackupResult, ResticError> {
        Self::create_backup_streaming(repo, paths, exclude_patterns, tags, None, &mut |_| {})
    }

    /// `restic backup --json`, streamed.
    ///
    /// restic emits newline-delimited JSON: a stream of
    /// `{"message_type":"status",...}` objects, then one
    /// `{"message_type":"summary",...}`. Reading it line by line (instead of
    /// buffering with `Command::output()`) gives the UI a real percentage and
    /// byte count, and lets [`ResticService::cancel_run`] stop the run through
    /// the registered child.
    pub fn create_backup_streaming(
        repo: &ResticRepo,
        paths: &[String],
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
        job_id: Option<&str>,
        on_status: &mut dyn FnMut(ResticProgress),
    ) -> Result<ResticBackupResult, ResticError> {
        if paths.is_empty() {
            return Err(ResticError::InvalidInput(
                "No source paths configured for this repository".to_string(),
            ));
        }
        for path in paths {
            Self::validate_backup_path(path)?;
        }
        Self::validate_exclude_patterns(exclude_patterns)?;
        Self::validate_tags(tags)?;

        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("backup");
        cmd.arg("--json");

        if let Some(exclude) = exclude_patterns {
            for pattern in exclude.split('\n') {
                let p = pattern.trim();
                if !p.is_empty() {
                    cmd.arg("--exclude");
                    cmd.arg(p);
                }
            }
        }

        if let Some(t) = tags {
            for tag in t.split(',') {
                let tg = tag.trim();
                if !tg.is_empty() {
                    cmd.arg("--tag");
                    cmd.arg(tg);
                }
            }
        }

        for path in paths {
            cmd.arg(path);
        }

        let mut tracked = Tracked::spawn(cmd, job_id)?;

        let mut summary: Option<ResticBackupResult> = None;
        let mut reported: Vec<String> = Vec::new();

        if let Some(stdout) = tracked.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match Self::parse_backup_line(&line) {
                    BackupEvent::Status(p) => on_status(p),
                    BackupEvent::Summary(result) => summary = Some(result),
                    BackupEvent::Error(msg) => {
                        if reported.len() < 20 && !reported.contains(&msg) {
                            reported.push(msg);
                        }
                    }
                    BackupEvent::Ignored => {}
                }
            }
        }

        let (exit_ok, stderr, cancelled) = tracked.wait()?;

        if cancelled {
            return Err(ResticError::Cancelled);
        }
        if !exit_ok {
            let detail = if !stderr.trim().is_empty() {
                stderr
            } else if !reported.is_empty() {
                reported.join("; ")
            } else {
                "restic backup exited with a non-zero status".to_string()
            };
            return Err(ResticError::CommandFailed(detail));
        }
        summary.ok_or_else(|| {
            ResticError::ParseError("restic backup produced no summary object".to_string())
        })
    }

    /// Parse the summary object out of a **buffered** `restic backup --json`
    /// blob.
    ///
    /// The streaming path ([`Self::create_backup_streaming`]) does not need
    /// this — it decodes each line as it arrives — but the helper stays public
    /// for callers that captured the whole output, and its tests pin the real
    /// 0.16.x NDJSON shape.
    ///
    /// `restic backup --json` (0.16.x) emits **newline-delimited JSON**: a
    /// stream of `{"message_type":"status",...}` progress objects followed by
    /// one final `{"message_type":"summary",...}`. Parsing the whole blob as
    /// a single object fails ("trailing characters at line 2") — and because
    /// that parse happened *after* restic had already run, every manual and
    /// scheduled backup reported "Backup failed" even though the snapshot was
    /// created. Take the last non-`status` JSON object (the summary).
    pub fn parse_backup_summary(
        stdout: &str,
    ) -> Result<HashMap<String, serde_json::Value>, ResticError> {
        stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<HashMap<String, serde_json::Value>>(l).ok())
            .rfind(|v| v.get("message_type").and_then(|m| m.as_str()) != Some("status"))
            .ok_or_else(|| {
                ResticError::ParseError(format!(
                    "no summary object in restic backup output: {stdout}"
                ))
            })
    }

    /// Decode one line of `restic backup --json` output.
    ///
    /// Pure: the whole streaming path is exercised by this function in tests,
    /// with real 0.16.x fixtures.
    pub fn parse_backup_line(line: &str) -> BackupEvent {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return BackupEvent::Ignored;
        }
        let Ok(value) = serde_json::from_str::<HashMap<String, serde_json::Value>>(trimmed) else {
            return BackupEvent::Ignored;
        };
        match value.get("message_type").and_then(|m| m.as_str()) {
            Some("status") => {
                let num = |k: &str| value.get(k).and_then(|v| v.as_u64());
                let float = |k: &str| value.get(k).and_then(|v| v.as_f64());
                let percent = float("percent_done").unwrap_or(0.0).clamp(0.0, 1.0) * 100.0;
                let files_done = num("files_done").unwrap_or(0);
                let total_files = num("total_files").unwrap_or(0);
                let message = if total_files > 0 {
                    format!("Backing up {files_done}/{total_files} files")
                } else {
                    "Backing up…".to_string()
                };
                BackupEvent::Status(ResticProgress {
                    percent,
                    message,
                    current_files: files_done,
                    total_files,
                    bytes_done: num("bytes_done").unwrap_or(0),
                    total_bytes: num("total_bytes").unwrap_or(0),
                })
            }
            Some("summary") => BackupEvent::Summary(Self::backup_result_from_summary(&value)),
            Some("error") => {
                let msg = value
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .or_else(|| value.get("error").and_then(|e| e.as_str()))
                    .unwrap_or("restic reported an error")
                    .to_string();
                BackupEvent::Error(msg)
            }
            _ => BackupEvent::Ignored,
        }
    }

    fn backup_result_from_summary(
        summary: &HashMap<String, serde_json::Value>,
    ) -> ResticBackupResult {
        let num = |k: &str| summary.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        ResticBackupResult {
            snapshot_id: summary
                .get("snapshot_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            files_new: num("files_new"),
            files_changed: num("files_changed"),
            files_unmodified: num("files_unmodified"),
            data_added_bytes: num("data_added"),
            total_files_processed: num("total_files_processed"),
            total_bytes: summary
                .get("total_bytes_processed")
                .or_else(|| summary.get("total_bytes"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            duration_secs: summary
                .get("total_duration")
                .and_then(|v| v.as_f64())
                .map(|d| d as u64)
                .unwrap_or(0),
        }
    }

    /// Back up a filesystem snapshot instead of the live tree.
    ///
    /// A failed mount used to be ignored: the backup then ran against the
    /// **empty** mount point, reported success and deleted the snapshot — the
    /// user believed their data was safe. The mount is now part of the
    /// contract: if it does not succeed, nothing is backed up and the run
    /// fails with the mount error.
    #[allow(clippy::too_many_arguments)]
    pub fn create_backup_with_snapshot(
        repo: &ResticRepo,
        pool: &str,
        dataset: &str,
        snapshot_name: &str,
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
        job_id: Option<&str>,
        on_status: &mut dyn FnMut(ResticProgress),
    ) -> Result<ResticBackupResult, ResticError> {
        Self::validate_snapshot_name(snapshot_name)?;
        if dataset.trim().is_empty() {
            return Err(ResticError::InvalidInput(
                "A dataset is required to snapshot before a backup".to_string(),
            ));
        }
        Self::validate_backup_path(dataset)?;

        let snapshot_path = format!("{}@{}", dataset, snapshot_name);

        crate::snapshots::SnapshotService::create_snapshot(
            pool,
            snapshot_name,
            Some(dataset),
            false,
        )
        .map_err(|e| {
            ResticError::CommandFailed(format!("Failed to create snapshot {snapshot_path}: {e}"))
        })?;
        info!("Created ZFS/Btrfs snapshot: {}", snapshot_path);

        let mount_point = format!("/tmp/restic-snap-{}", snapshot_name);
        let guard = SnapshotMount::mount(&snapshot_path, &mount_point, dataset)?;

        let result = Self::create_backup_streaming(
            repo,
            std::slice::from_ref(&mount_point),
            exclude_patterns,
            tags,
            job_id,
            on_status,
        );

        // The mount guard unmounts and removes the directory on drop; the
        // snapshot itself is deleted here so a failed backup still cleans up.
        drop(guard);
        let _ = crate::snapshots::SnapshotService::delete_snapshot(pool, &snapshot_path);

        result
    }

    // -----------------------------------------------------------------------
    // Snapshots
    // -----------------------------------------------------------------------

    pub fn list_snapshots(repo: &ResticRepo) -> Result<Vec<ResticSnapshot>, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["snapshots", "--json"]);

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResticError::CommandFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let snapshots: Vec<ResticSnapshot> =
            serde_json::from_str(&stdout).map_err(|e| ResticError::ParseError(e.to_string()))?;

        Ok(snapshots)
    }

    /// `restic restore <id> --target <dir>`, cancellable through `job_id`.
    pub fn restore_snapshot(
        repo: &ResticRepo,
        snapshot_id: &str,
        target: &str,
        exclude_patterns: Option<&str>,
        include_paths: Option<&[String]>,
        job_id: Option<&str>,
    ) -> Result<(), ResticError> {
        Self::validate_snapshot_id(snapshot_id)?;
        Self::validate_restore_target(target)?;
        Self::validate_exclude_patterns(exclude_patterns)?;
        if let Some(paths) = include_paths {
            for p in paths {
                let p = p.trim();
                if p.is_empty() {
                    continue;
                }
                if p.len() > MAX_PATH_LEN || p.chars().any(|c| c.is_control()) {
                    return Err(ResticError::InvalidInput(
                        "An include path is too long or contains a control character".to_string(),
                    ));
                }
            }
        }

        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["restore", snapshot_id, "--target", target]);

        if let Some(exclude) = exclude_patterns {
            for pattern in exclude.split('\n') {
                let p = pattern.trim();
                if !p.is_empty() {
                    cmd.arg("--exclude");
                    cmd.arg(p);
                }
            }
        }

        if let Some(include) = include_paths {
            for path in include {
                let p = path.trim();
                if !p.is_empty() {
                    cmd.arg("--include");
                    cmd.arg(p);
                }
            }
        }

        let tracked = Tracked::spawn(cmd, job_id)?;
        let (exit_ok, stderr, cancelled) = tracked.wait()?;
        if cancelled {
            return Err(ResticError::Cancelled);
        }
        if !exit_ok {
            return Err(ResticError::CommandFailed(stderr));
        }
        info!("Restic restore completed: {} -> {}", snapshot_id, target);
        Ok(())
    }

    /// `restic check`, cancellable through `job_id`.
    pub fn check_repo(
        repo: &ResticRepo,
        read_data: bool,
        job_id: Option<&str>,
    ) -> Result<ResticCheckResult, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("check");

        if read_data {
            cmd.arg("--read-data");
        }

        let tracked = Tracked::spawn(cmd, job_id)?;
        let (exit_ok, stderr, cancelled) = tracked.wait()?;
        if cancelled {
            return Err(ResticError::Cancelled);
        }
        // `restic check` writes its report to stderr (and the pack progress to
        // stdout, which we do not need here).
        Ok(Self::parse_check_output(exit_ok, "", &stderr))
    }

    /// Turn a `restic check` exit status + output into a verdict.
    ///
    /// The old implementation collected every line containing the substring
    /// `error` — and restic's **success** line is `no errors were found`. So
    /// `healthy` was `false` on every healthy repository, and the UI reported
    /// a broken backup to the user after each check. The success sentinel is
    /// now excluded and only real error markers count.
    pub fn parse_check_output(exit_ok: bool, stdout: &str, stderr: &str) -> ResticCheckResult {
        let mut errors: Vec<String> = Vec::new();
        for line in stdout.lines().chain(stderr.lines()) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            // The success sentinel contains "error" — never a failure marker.
            if lower.contains("no errors were found") {
                continue;
            }
            let is_error = lower.starts_with("error:")
                || lower.starts_with("fatal:")
                || lower.starts_with("unable to")
                || lower.starts_with("failed to")
                || lower.contains("is damaged")
                || lower.contains("damaged:");
            if is_error && errors.len() < 50 {
                errors.push(trimmed.to_string());
            }
        }
        if !exit_ok && errors.is_empty() {
            errors.push("restic check exited with a non-zero status".to_string());
        }
        let healthy = exit_ok && errors.is_empty();
        ResticCheckResult {
            healthy,
            message: if healthy {
                "Repository is healthy".to_string()
            } else {
                "Repository check found issues".to_string()
            },
            errors,
        }
    }

    // -----------------------------------------------------------------------
    // Retention
    // -----------------------------------------------------------------------

    /// `restic forget`. Refuses a call with **no** retention rule: restic
    /// answers "Fatal: no policy was specified" and the caller reported it as
    /// a generic failure.
    #[allow(clippy::too_many_arguments)]
    pub fn forget_snapshots(
        repo: &ResticRepo,
        keep_last: Option<u64>,
        keep_hourly: Option<u64>,
        keep_daily: Option<u64>,
        keep_weekly: Option<u64>,
        keep_monthly: Option<u64>,
        keep_yearly: Option<u64>,
        keep_within: Option<String>,
        prune: bool,
    ) -> Result<(), ResticError> {
        let has_rule = keep_last.is_some()
            || keep_hourly.is_some()
            || keep_daily.is_some()
            || keep_weekly.is_some()
            || keep_monthly.is_some()
            || keep_yearly.is_some()
            || keep_within.is_some();
        if !has_rule {
            return Err(ResticError::InvalidInput(
                "At least one retention rule (keep-last, keep-daily, keep-within, …) is required"
                    .to_string(),
            ));
        }
        if let Some(ref w) = keep_within {
            Self::validate_keep_within(w)?;
        }

        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("forget");

        if let Some(n) = keep_last {
            cmd.args(["--keep-last", &n.to_string()]);
        }
        if let Some(n) = keep_hourly {
            cmd.args(["--keep-hourly", &n.to_string()]);
        }
        if let Some(n) = keep_daily {
            cmd.args(["--keep-daily", &n.to_string()]);
        }
        if let Some(n) = keep_weekly {
            cmd.args(["--keep-weekly", &n.to_string()]);
        }
        if let Some(n) = keep_monthly {
            cmd.args(["--keep-monthly", &n.to_string()]);
        }
        if let Some(n) = keep_yearly {
            cmd.args(["--keep-yearly", &n.to_string()]);
        }
        if let Some(ref w) = keep_within {
            cmd.args(["--keep-within", w]);
        }

        if prune {
            cmd.arg("--prune");
        }

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic forget completed for repo: {}", repo.name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn prune_repo(repo: &ResticRepo) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("prune");

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic prune completed for repo: {}", repo.name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn get_stats(repo: &ResticRepo) -> Result<ResticStats, ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["stats", "--json"]);

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResticError::CommandFailed(stderr.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let raw: HashMap<String, serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| ResticError::ParseError(e.to_string()))?;

        Ok(ResticStats {
            total_size: raw.get("total_size").and_then(|v| v.as_u64()).unwrap_or(0),
            total_file_count: raw
                .get("total_file_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            snapshots_count: raw
                .get("snapshots_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            total_blob_count: raw
                .get("total_blob_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            compressed_size: raw
                .get("compressed_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        })
    }

    pub fn unlock_repo(repo: &ResticRepo) -> Result<(), ResticError> {
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.arg("unlock");

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic repo unlocked: {}", repo.name);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    /// Restored byte size of one snapshot (`restic stats --mode restore-size`).
    /// Used by the restore wizard's pre-flight space check so a restore never
    /// starts when the target cannot hold it.
    pub fn restore_size(repo: &ResticRepo, snapshot_id: &str) -> Result<u64, ResticError> {
        Self::validate_snapshot_id(snapshot_id)?;
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["stats", "--json", "--mode", "restore-size", snapshot_id]);

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ResticError::CommandFailed(stderr.to_string()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let raw: HashMap<String, serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| ResticError::ParseError(e.to_string()))?;
        Ok(raw.get("total_size").and_then(|v| v.as_u64()).unwrap_or(0))
    }

    pub fn diff_snapshots(
        repo: &ResticRepo,
        snapshot_a: &str,
        snapshot_b: &str,
    ) -> Result<String, ResticError> {
        Self::validate_snapshot_id(snapshot_a)?;
        Self::validate_snapshot_id(snapshot_b)?;
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["diff", snapshot_a, snapshot_b]);

        let output = cmd.output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn find_in_snapshots(
        repo: &ResticRepo,
        pattern: &str,
        snapshot_id: Option<&str>,
    ) -> Result<String, ResticError> {
        if pattern.trim().is_empty() {
            return Err(ResticError::InvalidInput(
                "A search pattern is required".to_string(),
            ));
        }
        if let Some(snap) = snapshot_id {
            Self::validate_snapshot_id(snap)?;
        }
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["find", pattern]);

        if let Some(snap) = snapshot_id {
            cmd.args(["--snapshot", snap]);
        }

        let output = cmd.output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }

    pub fn delete_snapshot(repo: &ResticRepo, snapshot_id: &str) -> Result<(), ResticError> {
        Self::validate_snapshot_id(snapshot_id)?;
        let mut cmd = Self::restic_cmd()?;
        Self::set_env(&mut cmd, repo);
        cmd.args(["forget", snapshot_id]);

        let output = cmd.output()?;
        if output.status.success() {
            info!("Restic snapshot deleted: {}", snapshot_id);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(ResticError::CommandFailed(stderr.to_string()))
        }
    }
}

/// Mounts a filesystem snapshot at a temporary path and guarantees the
/// unmount + directory removal on **every** exit path.
struct SnapshotMount {
    mount_point: String,
}

impl SnapshotMount {
    /// Mount `snapshot_path` at `mount_point`, or fail.
    ///
    /// Returns an error (after cleaning up) when the mount command does not
    /// succeed. The caller must never back up an unmounted mount point: that
    /// is an empty directory, and a backup of an empty directory that reports
    /// success is worse than a failed backup.
    fn mount(snapshot_path: &str, mount_point: &str, dataset: &str) -> Result<Self, ResticError> {
        let guard = Self {
            mount_point: mount_point.to_string(),
        };
        std::fs::create_dir_all(mount_point)
            .map_err(|e| ResticError::CommandFailed(format!("Cannot create {mount_point}: {e}")))?;

        let is_zfs = Command::new("zfs")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let output = if is_zfs {
            Command::new("mount")
                .args(["-t", "zfs", snapshot_path, mount_point])
                .output()?
        } else {
            Command::new("mount")
                .args(["--bind", dataset, mount_point])
                .output()?
        };

        if output.status.success() {
            Ok(guard)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(ResticError::CommandFailed(format!(
                "Cannot mount {snapshot_path} at {mount_point}: {}",
                if stderr.is_empty() {
                    "mount failed".to_string()
                } else {
                    stderr
                }
            )))
        }
    }
}

impl Drop for SnapshotMount {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.mount_point).output();
        let _ = std::fs::remove_dir(&self.mount_point);
    }
}

#[cfg(test)]
mod tests {
    //! Pure parsing and validation tests for `ResticService`.
    //!
    //! Fixtures are real output shapes from `restic` 0.16.x. The important
    //! ones are `parse_backup_summary` (newline-delimited JSON) and
    //! `parse_check_output` (restic's success line *contains* the substring
    //! "error", which made every healthy repository look broken).
    //!
    //! Run: `cargo test restic::tests`

    use super::*;

    const BACKUP_JSON: &str = r#"{"message_type":"status","percent_done":0,"total_files":1,"total_bytes":6}
{"message_type":"status","percent_done":0.5,"files_done":0,"bytes_done":0,"total_files":1,"total_bytes":6}
{"message_type":"status","percent_done":1,"files_done":1,"bytes_done":6,"total_files":1,"total_bytes":6}
{"message_type":"summary","files_new":1,"files_changed":0,"files_unmodified":0,"dirs_new":1,"dirs_changed":0,"dirs_unmodified":0,"data_blobs":1,"tree_blobs":1,"data_added":419,"total_files_processed":1,"total_bytes_processed":6,"total_duration":0.012345,"snapshot_id":"da0f81d6ff3be9013fb2646c8f779504c12e2fb2769b2d5923fd3c45f9db1a6e"}"#;

    #[test]
    fn test_parse_backup_summary_picks_the_summary_line() {
        let s = ResticService::parse_backup_summary(BACKUP_JSON).expect("must parse");
        assert_eq!(
            s.get("message_type").and_then(|v| v.as_str()),
            Some("summary")
        );
        assert_eq!(s.get("files_new").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(s.get("data_added").and_then(|v| v.as_u64()), Some(419));
        assert_eq!(
            s.get("total_bytes_processed").and_then(|v| v.as_u64()),
            Some(6)
        );
        assert!(s
            .get("snapshot_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("da0f81d6"));
    }

    #[test]
    fn test_parse_backup_summary_rejects_empty_output() {
        assert!(ResticService::parse_backup_summary("").is_err());
        assert!(ResticService::parse_backup_summary("   \n\n").is_err());
    }

    #[test]
    fn test_parse_backup_summary_tolerates_only_status_lines() {
        // A backup with no summary object (interrupted) — must not panic,
        // must not silently pick a status line.
        let only_status = "{\"message_type\":\"status\",\"percent_done\":0.3}\n";
        assert!(ResticService::parse_backup_summary(only_status).is_err());
    }

    #[test]
    fn test_parse_backup_summary_single_line_still_works() {
        // Older/quieter restic could emit just the summary.
        let one = r#"{"message_type":"summary","files_new":2,"data_added":10,"total_bytes_processed":20,"total_duration":1.5,"snapshot_id":"abc123"}"#;
        let s = ResticService::parse_backup_summary(one).unwrap();
        assert_eq!(s.get("files_new").and_then(|v| v.as_u64()), Some(2));
    }

    #[test]
    fn test_backup_result_mapping_from_summary() {
        // End-to-end of the mapping in create_backup (minus the Command).
        let s = ResticService::parse_backup_summary(BACKUP_JSON).unwrap();
        let result = ResticService::backup_result_from_summary(&s);
        assert_eq!(result.files_new, 1);
        assert_eq!(result.data_added_bytes, 419);
        assert_eq!(result.total_files_processed, 1);
        assert_eq!(result.total_bytes, 6);
        assert_eq!(result.duration_secs, 0); // 0.0123s truncates to 0
        assert!(!result.snapshot_id.is_empty());
    }

    // -- streaming line decoder ------------------------------------------

    #[test]
    fn test_parse_backup_line_status_carries_real_progress() {
        let ev = ResticService::parse_backup_line(
            r#"{"message_type":"status","percent_done":0.4242,"files_done":42,"total_files":100,"bytes_done":1024,"total_bytes":4096}"#,
        );
        match ev {
            BackupEvent::Status(p) => {
                assert!(
                    (p.percent - 42.42).abs() < 0.01,
                    "percent was {}",
                    p.percent
                );
                assert_eq!(p.current_files, 42);
                assert_eq!(p.total_files, 100);
                assert_eq!(p.bytes_done, 1024);
                assert_eq!(p.total_bytes, 4096);
                assert!(p.message.contains("42/100"));
            }
            other => panic!("expected a status event, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_backup_line_status_is_clamped_and_tolerates_missing_fields() {
        // restic emits `percent_done: null` on the very first tick, and the
        // final tick can overshoot 1.0 by a rounding error.
        let ev = ResticService::parse_backup_line(
            r#"{"message_type":"status","percent_done":null,"total_files":3}"#,
        );
        match ev {
            BackupEvent::Status(p) => {
                assert_eq!(p.percent, 0.0);
                assert_eq!(p.total_files, 3);
                assert_eq!(p.bytes_done, 0);
            }
            other => panic!("expected a status event, got {other:?}"),
        }
        let ev = ResticService::parse_backup_line(
            r#"{"message_type":"status","percent_done":1.0000001}"#,
        );
        match ev {
            BackupEvent::Status(p) => assert_eq!(p.percent, 100.0),
            other => panic!("expected a status event, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_backup_line_summary_is_the_typed_result() {
        let line = BACKUP_JSON.lines().last().unwrap();
        match ResticService::parse_backup_line(line) {
            BackupEvent::Summary(r) => {
                assert_eq!(r.files_new, 1);
                assert_eq!(r.data_added_bytes, 419);
                assert!(r.snapshot_id.starts_with("da0f81d6"));
            }
            other => panic!("expected a summary, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_backup_line_error_and_noise() {
        match ResticService::parse_backup_line(
            r#"{"message_type":"error","error":{"message":"open /x: permission denied"}}"#,
        ) {
            BackupEvent::Error(m) => assert!(m.contains("permission denied")),
            other => panic!("expected an error event, got {other:?}"),
        }
        assert_eq!(
            ResticService::parse_backup_line("not json at all"),
            BackupEvent::Ignored
        );
        assert_eq!(
            ResticService::parse_backup_line("   "),
            BackupEvent::Ignored
        );
        assert_eq!(
            ResticService::parse_backup_line(r#"{"message_type":"verbose","message":"x"}"#),
            BackupEvent::Ignored
        );
    }

    // -- restic check verdict --------------------------------------------

    #[test]
    fn test_check_output_healthy_repo_is_not_flagged() {
        // The regression: "no errors were found" contains "error", so the old
        // collector reported every healthy repository as broken.
        let stdout = "using temporary cache in /tmp/restic-check-cache-1\nload indexes\ncheck all packs\ncheck snapshots, trees and blobs\n[0:01] 100.00%  3 / 3 snapshots\nno errors were found\n";
        let r = ResticService::parse_check_output(true, stdout, "");
        assert!(r.healthy, "a healthy repo must not be flagged: {r:?}");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.message, "Repository is healthy");
    }

    #[test]
    fn test_check_output_detects_real_errors() {
        let stdout = "load indexes\nFatal: repository contains errors\n";
        let r = ResticService::parse_check_output(false, stdout, "");
        assert!(!r.healthy);
        assert!(r.errors.iter().any(|e| e.contains("contains errors")));

        let r = ResticService::parse_check_output(
            false,
            "",
            "error: pack 0123abcd is damaged: ciphertext verification failed\n",
        );
        assert!(!r.healthy);
        assert_eq!(r.errors.len(), 1);

        // A non-zero exit with no recognizable marker still reports something,
        // instead of a silent "unhealthy" with an empty reason list.
        let r = ResticService::parse_check_output(false, "something odd", "");
        assert!(!r.healthy);
        assert_eq!(r.errors.len(), 1);
        assert!(r.errors[0].contains("non-zero status"));
    }

    #[test]
    fn test_check_output_ignores_unrelated_lines() {
        let stdout = "using temporary cache\nload indexes\ncheck all packs\ncheck snapshots, trees and blobs\nno errors were found\n";
        let r = ResticService::parse_check_output(true, stdout, "");
        assert!(r.healthy);
        assert!(r.errors.is_empty());
    }

    // -- argument validation ---------------------------------------------

    #[test]
    fn test_validate_snapshot_id_accepts_real_ids() {
        for id in [
            "latest",
            "da0f81d6",
            "da0f81d6ff3be9013fb2646c8f779504c12e2fb2769b2d5923fd3c45f9db1a6e",
        ] {
            assert!(
                ResticService::validate_snapshot_id(id).is_ok(),
                "{id} should be accepted"
            );
        }
    }

    #[test]
    fn test_validate_snapshot_id_rejects_option_injection() {
        // `restic forget --keep-last=0` would wipe the repository.
        for bad in [
            "--keep-last=0",
            "-x",
            "../etc",
            "a/b",
            "",
            "   ",
            ".",
            "..",
            "id with spaces",
            "id\nnewline",
        ] {
            assert!(
                ResticService::validate_snapshot_id(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(ResticService::validate_snapshot_id(&"a".repeat(129)).is_err());
    }

    #[test]
    fn test_validate_backup_path_requires_absolute_clean_paths() {
        assert!(ResticService::validate_backup_path("/srv/data").is_ok());
        assert!(ResticService::validate_backup_path("/").is_ok());
        for bad in [
            "",
            "srv/data",
            "./data",
            "-rf",
            "/srv/../etc",
            "/srv/da\0ta",
            "/srv/data\n",
        ] {
            assert!(
                ResticService::validate_backup_path(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(ResticService::validate_backup_path(&format!("/{}", "a".repeat(4096))).is_err());
    }

    #[test]
    fn test_validate_restore_target_matches_the_backup_rules() {
        assert!(ResticService::validate_restore_target("/").is_ok());
        assert!(ResticService::validate_restore_target("/mnt/restore").is_ok());
        assert!(ResticService::validate_restore_target("mnt/restore").is_err());
        assert!(ResticService::validate_restore_target("/mnt/../../etc").is_err());
        assert!(ResticService::validate_restore_target("--target").is_err());
    }

    #[test]
    fn test_validate_snapshot_name_blocks_path_traversal() {
        // The name builds /tmp/restic-snap-<name>: `../../etc` must never
        // become a mount point.
        assert!(ResticService::validate_snapshot_name("restic-20261021_120000").is_ok());
        assert!(ResticService::validate_snapshot_name("restic:1").is_ok());
        for bad in ["", "../../etc", "a/b", "-rf", ".hidden", "with space", "é"] {
            assert!(
                ResticService::validate_snapshot_name(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(ResticService::validate_snapshot_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn test_validate_repository_accepts_local_and_remote_specs() {
        for good in [
            "/srv/backup/repo",
            "sftp:user@nas:/srv/backup",
            "s3:s3.amazonaws.com/bucket",
            "rest:https://backup.example.com/",
        ] {
            assert!(
                ResticService::validate_repository(good).is_ok(),
                "{good} should be accepted"
            );
        }
        for bad in ["", "  ", "-o", "/srv/a\nb", "/srv/a\0b"] {
            assert!(
                ResticService::validate_repository(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(ResticService::validate_repository(&"/".repeat(1025)).is_err());
    }

    #[test]
    fn test_validate_password_rejects_empty_and_control_characters() {
        assert!(ResticService::validate_password("s3cret").is_ok());
        assert!(ResticService::validate_password("").is_err());
        assert!(ResticService::validate_password("a\nb").is_err());
    }

    #[test]
    fn test_validate_display_name_bounds() {
        assert!(ResticService::validate_display_name("Repository name", "nas").is_ok());
        assert!(ResticService::validate_display_name("Name", "   ").is_err());
        assert!(ResticService::validate_display_name("Name", "a\nb").is_err());
        assert!(ResticService::validate_display_name("Name", &"a".repeat(101)).is_err());
    }

    #[test]
    fn test_validate_keep_within_grammar() {
        for good in ["30d", "1y", "1y2m3d4h", "12h", "7d"] {
            assert!(
                ResticService::validate_keep_within(good).is_ok(),
                "{good} should be accepted"
            );
        }
        for bad in ["", "30", "d", "30x", "1y2", "-1d", "1 d", "30days"] {
            assert!(
                ResticService::validate_keep_within(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn test_validate_tags_and_excludes() {
        assert!(ResticService::validate_tags(Some("daily,nightly")).is_ok());
        assert!(ResticService::validate_tags(None).is_ok());
        assert!(ResticService::validate_tags(Some("two words")).is_err());
        assert!(ResticService::validate_tags(Some("a\nb")).is_err());

        assert!(ResticService::validate_exclude_patterns(Some("*.tmp\n/cache")).is_ok());
        assert!(ResticService::validate_exclude_patterns(None).is_ok());
        assert!(ResticService::validate_exclude_patterns(Some("a\0b")).is_err());
        let too_many = (0..300)
            .map(|i| format!("p{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(ResticService::validate_exclude_patterns(Some(&too_many)).is_err());
    }

    #[test]
    fn test_forget_without_any_rule_is_refused_before_spawning_restic() {
        // restic answers "Fatal: no policy was specified"; we refuse earlier
        // with a message the UI can act on. No `restic` binary is needed for
        // this path.
        let repo = ResticRepo {
            id: "r".into(),
            name: "r".into(),
            repository: "/tmp/does-not-matter".into(),
            password: "p".into(),
            retention_policy_id: None,
            source_paths: vec![],
            exclude_patterns: None,
            tags: None,
            snapshot_source: "directory".into(),
            snapshot_pool: None,
            snapshot_dataset: None,
            snapshot_create_before_backup: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let err =
            ResticService::forget_snapshots(&repo, None, None, None, None, None, None, None, true)
                .unwrap_err();
        assert!(matches!(err, ResticError::InvalidInput(_)));
        assert!(err.to_string().contains("retention rule"));

        let err = ResticService::forget_snapshots(
            &repo,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("30 days".into()),
            true,
        )
        .unwrap_err();
        assert!(matches!(err, ResticError::InvalidInput(_)));
    }

    #[test]
    fn test_backup_with_no_paths_is_refused_before_spawning_restic() {
        let repo = ResticRepo {
            id: "r".into(),
            name: "r".into(),
            repository: "/tmp/does-not-matter".into(),
            password: "p".into(),
            retention_policy_id: None,
            source_paths: vec![],
            exclude_patterns: None,
            tags: None,
            snapshot_source: "directory".into(),
            snapshot_pool: None,
            snapshot_dataset: None,
            snapshot_create_before_backup: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let err = ResticService::create_backup_streaming(&repo, &[], None, None, None, &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, ResticError::InvalidInput(_)));
        // and a relative path is refused just as early
        let err = ResticService::create_backup_streaming(
            &repo,
            &["etc".to_string()],
            None,
            None,
            None,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, ResticError::InvalidInput(_)));
    }

    // -- repository run lock ---------------------------------------------

    #[test]
    fn test_repo_run_lock_is_exclusive_and_released_on_drop() {
        let id = "repo-lock-test-1";
        assert!(!RepoRunLock::is_busy(id));
        let guard = RepoRunLock::acquire(id).expect("first acquire");
        assert!(RepoRunLock::is_busy(id));
        assert!(
            RepoRunLock::acquire(id).is_none(),
            "a second run on the same repository must be refused"
        );
        // A different repository stays available.
        let other = RepoRunLock::acquire("repo-lock-test-2").expect("other repo");
        drop(other);
        drop(guard);
        assert!(!RepoRunLock::is_busy(id));
        assert!(RepoRunLock::acquire(id).is_some(), "released after drop");
    }

    #[test]
    fn test_cancel_run_unknown_job_is_false() {
        assert!(!ResticService::cancel_run("no-such-job"));
        assert!(!ResticService::is_run_active("no-such-job"));
    }

    // -- ResticSnapshot deserialization (restic snapshots --json) --------

    #[test]
    fn test_snapshot_list_deserializes_real_json() {
        let raw = r#"[
          {"time":"2026-09-06T14:53:02.752523986+02:00","tree":"c87bcc11","paths":["/srv/data"],
           "hostname":"nas","username":"root","program_version":"restic 0.16.4",
           "tags":["daily"],"id":"da0f81d6ff3be9013fb2646c8f779504","short_id":"da0f81d6"},
          {"time":"2026-09-05T14:53:02Z","tree":"aa11","paths":["/srv/data","/etc"],
           "hostname":"nas","program_version":"restic 0.16.4",
           "id":"bb22cc33dd44","short_id":"bb22cc33"}
        ]"#;
        let snaps: Vec<ResticSnapshot> = serde_json::from_str(raw).expect("must parse");
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].short_id, "da0f81d6");
        assert_eq!(snaps[0].tags.as_deref(), Some(&["daily".to_string()][..]));
        assert_eq!(snaps[1].paths, vec!["/srv/data", "/etc"]);
        // second snapshot has no `tags` key at all → None, not an error
        assert!(snaps[1].tags.is_none());
    }

    #[test]
    fn test_empty_snapshot_list_parses() {
        let snaps: Vec<ResticSnapshot> = serde_json::from_str("[]").unwrap();
        assert!(snaps.is_empty());
    }

    // -- serialization contracts (frontend expects these field names) ----

    #[test]
    fn test_result_structs_serialize_with_snake_case_keys() {
        let r = ResticBackupResult {
            snapshot_id: "x".into(),
            files_new: 1,
            files_changed: 2,
            files_unmodified: 3,
            data_added_bytes: 4,
            total_files_processed: 5,
            total_bytes: 6,
            duration_secs: 7,
        };
        let v = serde_json::to_value(&r).unwrap();
        for k in [
            "snapshot_id",
            "files_new",
            "data_added_bytes",
            "total_files_processed",
            "duration_secs",
        ] {
            assert!(v.get(k).is_some(), "missing {k}");
        }

        let stats = ResticStats {
            total_size: 1,
            total_file_count: 2,
            snapshots_count: 3,
            total_blob_count: 4,
            compressed_size: 5,
        };
        let sv = serde_json::to_value(&stats).unwrap();
        assert_eq!(sv["snapshots_count"], 3);
    }
}
