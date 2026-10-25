/* =============================================================================
 * File          : cache.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Disk cache-tier service: a designated fast pool absorbs writes for
 *   named cache shares, each mirrored on a slower target pool; the mover
 *   drains aged data from cache to target on schedule (single-flight
 *   guard, live progress, per-run history).
 *
 * Dependencies:
 *   - internal storage layer (cache repository/settings)
 *
 * Usage:
 *   handlers/cache.rs; the mover is triggered by cache_scheduler.rs or the
 *   manual endpoint.
 * =============================================================================
 */

//! Disk cache tier service (HomeVault write caching).
//!
//! A designated fast pool ("cache") absorbs writes for named datasets
//! ("cache shares"). Each cache share also lives on a slower "target" pool
//! (the array). The `mover` transfers data between the two according to the
//! share's cache mode:
//!
//! * `yes`    — new data lands on cache; mover copies it to the array and
//!   frees the cache.
//! * `no`     — data lives on the array only; anything left on cache is moved
//!   back to the array.
//! * `prefer` — data prefers the cache, but is moved to the array when the
//!   cache fills past `min_free_percent`.
//! * `only`   — data stays on the cache permanently; anything on the array is
//!   moved back to the cache.

use crate::pools::PoolService;
use crate::shares::{Share, ShareType, ShareUpdate, SharesService};
use crate::sse::ProgressEmitter;
use crate::storage::cache_repo::{CacheRepository, CacheSettings, CacheShare, ShareOutcomeRecord};
use crate::storage::NotificationRepository;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing;
use uuid::Uuid;

use crate::utils::process::run_timed;

// ---------------------------------------------------------------------------
// Deadlines (ROADMAP X.2)
//
// The mover shells out to `rsync`, `find` and `df` from inside
// `spawn_blocking`, and `run_mover` holds the process-wide single-flight slot
// for the whole run. A wedged `rsync` therefore did not just stall one job: it
// pinned that blocking thread **and** kept `MOVER_RUNNING` set, so no mover ran
// again — manually or on schedule — until the service was restarted. Each value
// below is a ceiling for the slowest honest case, not an estimate.
// ---------------------------------------------------------------------------

/// `df -B1 <path>` — a mount-table read. Also bounds the free-space gate, which
/// must answer *before* `rsync --remove-source-files` deletes anything.
pub const CACHE_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// `find` — sizing a share and building the age list. Proportional to the
/// number of files on the cache tier, so a large library is minutes.
pub const CACHE_FIND_TIMEOUT: Duration = Duration::from_secs(600);
/// `rsync` — one share's transfer. A first drain of a full cache tier
/// legitimately takes hours, so this is a ceiling like rclone's transfer
/// budget; the child is killed at the deadline instead of being abandoned,
/// which is what frees the single-flight slot.
pub const CACHE_RSYNC_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

/// Run one host command under a deadline, keeping the reason it failed.
///
/// The mover is synchronous by design (it runs on the blocking pool), so this
/// is the blocking helper. The timeout is a parameter rather than a constant
/// baked into each call site so the kill path is unit-testable, exactly like
/// `disk::bounded`.
fn run_command(cmd: &mut Command, timeout: Duration) -> Result<std::process::Output, String> {
    run_timed(cmd, timeout)
}

/// Guards against the mover running twice at once (manual trigger racing the
/// scheduler, or a double click). `run_mover` is the single entry point used
/// by both, so claiming/releasing it there covers every trigger path.
static MOVER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Non-authoritative peek used for an early, friendly rejection in the HTTP
/// handler before a job/progress entry is even created. The actual claim
/// (and the race-free guarantee) happens inside `run_mover`.
pub fn mover_is_running() -> bool {
    MOVER_RUNNING.load(Ordering::SeqCst)
}

/// RAII guard that releases the mover slot on drop, including on panic
/// unwind, so a claim can never be leaked.
struct MoverRunGuard;

impl Drop for MoverRunGuard {
    fn drop(&mut self) {
        MOVER_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// The four cache modes for per-share cache placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheMode {
    Yes,
    No,
    Prefer,
    Only,
}

impl CacheMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheMode::Yes => "yes",
            CacheMode::No => "no",
            CacheMode::Prefer => "prefer",
            CacheMode::Only => "only",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "yes" => Some(CacheMode::Yes),
            "no" => Some(CacheMode::No),
            "prefer" => Some(CacheMode::Prefer),
            "only" => Some(CacheMode::Only),
            _ => None,
        }
    }
}

/// Result of a single mover execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoverResult {
    pub files_moved: u64,
    pub bytes_moved: u64,
    /// Shares that actually transferred something (a `Moved` outcome).
    pub shares_processed: u32,
    /// One entry per enabled share, in processing order — including the ones
    /// that were skipped or failed, so the UI can show which shares are now
    /// split across the two tiers.
    pub shares: Vec<ShareOutcome>,
}

/// How one share fared during a mover run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareOutcomeStatus {
    /// Data was transferred (and the source copy removed).
    Moved,
    /// Nothing to do — the source side of the move was empty.
    Skipped,
    /// This share could not be moved. The run continues with the next one and
    /// **nothing** was deleted for this share.
    Failed,
}

impl ShareOutcomeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShareOutcomeStatus::Moved => "moved",
            ShareOutcomeStatus::Skipped => "skipped",
            ShareOutcomeStatus::Failed => "failed",
        }
    }
}

/// Per-share result of a mover run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareOutcome {
    pub name: String,
    pub mode: String,
    pub status: ShareOutcomeStatus,
    pub files_moved: u64,
    pub bytes_moved: u64,
    /// Why the share failed (or why it was skipped) — `None` on success.
    pub message: Option<String>,
}

impl ShareOutcome {
    fn new(share: &CacheShare) -> Self {
        Self {
            name: share.name.clone(),
            mode: share.cache_mode.clone(),
            status: ShareOutcomeStatus::Skipped,
            files_moved: 0,
            bytes_moved: 0,
            message: None,
        }
    }

    fn failed(mut self, message: impl Into<String>) -> Self {
        self.status = ShareOutcomeStatus::Failed;
        self.message = Some(message.into());
        self
    }

    fn moved(mut self, files: u64, bytes: u64) -> Self {
        self.status = ShareOutcomeStatus::Moved;
        self.files_moved = files;
        self.bytes_moved = bytes;
        self
    }
}

impl MoverResult {
    /// `completed` when every attempted share succeeded, `partial` when some
    /// did and some did not, `failed` when every attempted share failed.
    ///
    /// A run with nothing to do (no enabled share, or every share skipped) is
    /// `completed`: nothing was left in a half-moved state.
    pub fn status(&self) -> &'static str {
        let failed = self
            .shares
            .iter()
            .filter(|s| s.status == ShareOutcomeStatus::Failed)
            .count();
        let moved = self
            .shares
            .iter()
            .filter(|s| s.status == ShareOutcomeStatus::Moved)
            .count();

        if failed == 0 {
            "completed"
        } else if moved == 0 {
            "failed"
        } else {
            "partial"
        }
    }

    /// Names of the shares that could not be moved, for the run summary.
    pub fn failed_shares(&self) -> Vec<&str> {
        self.shares
            .iter()
            .filter(|s| s.status == ShareOutcomeStatus::Failed)
            .map(|s| s.name.as_str())
            .collect()
    }

    /// The rows to persist for the per-share history.
    pub fn outcome_records(&self) -> Vec<ShareOutcomeRecord> {
        self.shares
            .iter()
            .map(|s| ShareOutcomeRecord {
                share_name: s.name.clone(),
                mode: s.mode.clone(),
                status: s.status.as_str().to_string(),
                files_moved: s.files_moved,
                bytes_moved: s.bytes_moved,
                message: s.message.clone(),
            })
            .collect()
    }
}

/// Pool name → mount point, resolved by **one** `PoolService::scan_pools()`
/// per mover run or per request (ROADMAP 2.3).
///
/// The scan shells out to `zpool`, `mdadm`, `btrfs` and `df`; doing it once per
/// share (as the old `pool_mount_point` helper did) meant three subprocesses
/// per share on every run, and inline in the async handlers it parked a
/// runtime worker.
#[derive(Debug, Clone, Default)]
pub struct PoolMounts(HashMap<String, String>);

impl PoolMounts {
    /// One scan for the whole run. The error is the *scan* failure, which is a
    /// different condition from "this pool does not exist" — callers must not
    /// collapse the two.
    pub fn scan() -> Result<Self, String> {
        let pools =
            PoolService::scan_pools().map_err(|e| format!("Failed to scan pools: {}", e))?;
        Ok(Self::from_pools(pools))
    }

    pub fn from_pools(pools: Vec<crate::pools::PoolInfo>) -> Self {
        Self(
            pools
                .into_iter()
                .filter_map(|p| p.mount_point.map(|m| (p.name, m)))
                .collect(),
        )
    }

    /// Build from explicit pairs (tests, and any caller that already knows the
    /// mounts).
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }

    pub fn mount_of(&self, pool: &str) -> Option<&str> {
        self.0.get(pool).map(String::as_str)
    }
}

/// Service for managing the cache tier and running the mover.
pub struct CacheService;

impl CacheService {
    /// Active on-disk path for a cache share given its mode.
    ///
    /// * `no`      → array (target pool)
    /// * `only`    → cache
    /// * `yes`/`prefer` → cache (writes land on cache, mover drains it)
    pub fn active_path(share: &CacheShare, cache_mount: &str, target_mount: &str) -> String {
        match CacheMode::parse(&share.cache_mode) {
            Some(CacheMode::No) => format!("{}/{}", target_mount, share.name),
            _ => format!("{}/{}", cache_mount, share.name),
        }
    }

    /// Run the mover. Walks every enabled share and applies its mode's
    /// transfer rule. This is CPU/IO-bound (it shells out to `rsync`, `find`
    /// and `df`), so callers must invoke it inside
    /// `tokio::task::spawn_blocking`.
    ///
    /// Only the run-level conditions (no cache pool configured, cache pool not
    /// mounted) are `Err`. A share that cannot be moved is recorded as a
    /// `Failed` outcome and the run continues with the next share, so one
    /// broken share no longer strands the rest of the queue (ROADMAP 2.2).
    ///
    /// `job_id`, when set, receives live SSE progress updates.
    pub fn perform_moves(
        settings: &CacheSettings,
        shares: &[CacheShare],
        mounts: &PoolMounts,
        job_id: Option<&str>,
    ) -> Result<MoverResult, String> {
        let cache_pool = settings
            .cache_pool
            .as_ref()
            .ok_or_else(|| "No cache pool configured".to_string())?;

        let cache_mount = mounts
            .mount_of(cache_pool)
            .ok_or_else(|| format!("Cache pool '{}' is not mounted", cache_pool))?;

        let excludes: Vec<String> = settings
            .mover_excludes
            .as_deref()
            .map(|s| {
                s.split([',', ' ', '\n', '\t'])
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let targets: Vec<&CacheShare> = shares.iter().filter(|s| s.enabled).collect();
        let total = targets.len().max(1) as f64;

        let mut outcomes = Vec::with_capacity(targets.len());
        let mut total_files = 0u64;
        let mut total_bytes = 0u64;
        let mut processed = 0u32;

        for (i, share) in targets.iter().enumerate() {
            if let Some(jid) = job_id {
                ProgressEmitter::progress(
                    jid,
                    (i as f64 / total) * 100.0,
                    &format!(
                        "Moving cache share '{}' (mode: {})",
                        share.name, share.cache_mode
                    ),
                );
            }

            let outcome = Self::move_share(share, settings, cache_mount, mounts, &excludes);

            match outcome.status {
                ShareOutcomeStatus::Moved => {
                    processed += 1;
                    total_files += outcome.files_moved;
                    total_bytes += outcome.bytes_moved;
                }
                ShareOutcomeStatus::Failed => {
                    tracing::warn!(
                        "Cache share '{}' could not be moved: {}",
                        outcome.name,
                        outcome.message.as_deref().unwrap_or("unknown error")
                    );
                }
                ShareOutcomeStatus::Skipped => {}
            }

            outcomes.push(outcome);
        }

        Ok(MoverResult {
            files_moved: total_files,
            bytes_moved: total_bytes,
            shares_processed: processed,
            shares: outcomes,
        })
    }

    /// Move one share according to its mode. Never returns `Err`: a share-level
    /// problem becomes a `Failed` outcome so the run can carry on.
    fn move_share(
        share: &CacheShare,
        settings: &CacheSettings,
        cache_mount: &str,
        mounts: &PoolMounts,
        excludes: &[String],
    ) -> ShareOutcome {
        let outcome = ShareOutcome::new(share);

        let Some(target_mount) = mounts.mount_of(&share.target_pool) else {
            return outcome.failed(format!(
                "Target pool '{}' is not mounted",
                share.target_pool
            ));
        };

        let Some(mode) = CacheMode::parse(&share.cache_mode) else {
            return outcome.failed(format!("Unknown cache mode '{}'", share.cache_mode));
        };

        let cache_dir = format!("{}/{}", cache_mount, share.name);
        let array_dir = format!("{}/{}", target_mount, share.name);

        // `(source, destination)` for this mode.
        let (src, dst) = match mode {
            // Data belongs on the array; move anything still on the cache.
            CacheMode::No | CacheMode::Yes => (&cache_dir, &array_dir),
            // Data belongs on the cache; move anything on the array back.
            CacheMode::Only => (&array_dir, &cache_dir),
            CacheMode::Prefer => {
                let cache_full =
                    is_cache_full(Self::pool_usage_pct(cache_mount), settings.min_free_percent);
                if cache_full {
                    // Cache is (nearly) full: drain to the array.
                    (&cache_dir, &array_dir)
                } else {
                    // Cache has room: keep/promote data onto cache.
                    (&array_dir, &cache_dir)
                }
            }
        };

        if !Self::dir_has_content(src) {
            return outcome;
        }

        match Self::rsync_move(
            src,
            dst,
            true,
            excludes,
            settings.mover_bwlimit,
            settings.mover_age_days,
        ) {
            Ok((files, bytes)) => outcome.moved(files, bytes),
            Err(e) => outcome.failed(e),
        }
    }

    /// Returns true if `dir` exists and contains at least one entry.
    fn dir_has_content(dir: &str) -> bool {
        let p = Path::new(dir);
        if !p.exists() {
            return false;
        }
        match std::fs::read_dir(p) {
            Ok(mut entries) => entries.next().is_some(),
            Err(_) => false,
        }
    }

    /// `(used, available)` bytes of the filesystem holding `path`, via
    /// `df -B1`. `None` when `df` is unavailable or its output is unparseable.
    fn df_space(path: &str) -> Option<(u64, u64)> {
        let output =
            run_command(Command::new("df").args(["-B1", path]), CACHE_QUERY_TIMEOUT).ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_df_space(&stdout)
    }

    /// Returns the used-percentage (0-100) of the filesystem holding `path`,
    /// via `df`. Used to decide whether `prefer` shares should be drained.
    fn pool_usage_pct(path: &str) -> u64 {
        match Self::df_space(path) {
            Some((used, avail)) => {
                let total = used + avail;
                (used * 100).checked_div(total).unwrap_or(0)
            }
            None => 0,
        }
    }

    /// Total apparent size of the regular files a move would transfer.
    ///
    /// The age filter is applied exactly as `rsync_move` applies it, so the
    /// figure is the payload that is about to land on the destination. The
    /// `mover_excludes` patterns are deliberately **not** subtracted: an
    /// over-estimate makes the free-space gate conservative, which is the safe
    /// direction.
    fn source_payload_bytes(src: &str, min_age_days: u32) -> Result<u64, String> {
        if !Path::new(src).exists() {
            return Ok(0);
        }
        let src_slash = format!("{}/", src.trim_end_matches('/'));
        let mut args: Vec<String> = vec![src_slash, "-type".into(), "f".into()];
        if min_age_days > 0 {
            args.push("-mtime".into());
            args.push(format!("+{}", min_age_days.saturating_sub(1)));
        }
        args.push("-printf".into());
        args.push("%s\\n".into());

        let out = run_command(Command::new("find").args(&args), CACHE_FIND_TIMEOUT)
            .map_err(|e| format!("failed to run find: {}", e))?;
        if !out.status.success() {
            return Err(format!(
                "find failed while sizing {}: {}",
                src,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(sum_sizes(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Refuse a destructive move that would not fit on the destination.
    ///
    /// `rsync --remove-source-files` deletes each file after copying it, so
    /// draining a cache into a nearly-full array used to fill the array and
    /// leave the un-copied files on a cache tier that is now the only copy and
    /// out of space (ROADMAP 2.1). Checking the destination *before* rsync runs
    /// means a refused share deletes nothing at all.
    fn ensure_destination_has_room(src: &str, dst: &str, min_age_days: u32) -> Result<u64, String> {
        let payload = Self::source_payload_bytes(src, min_age_days)?;
        if payload == 0 {
            return Ok(0);
        }
        let Some((_, avail)) = Self::df_space(dst) else {
            return Err(format!(
                "cannot read free space on {} — refusing to move {} without it",
                dst,
                Self::human_bytes(payload)
            ));
        };
        if avail < payload {
            return Err(format!(
                "not enough space on {}: {} to move, {} free",
                dst,
                Self::human_bytes(payload),
                Self::human_bytes(avail)
            ));
        }
        Ok(payload)
    }

    /// Move data from `src` into `dst` with `rsync`, optionally deleting the
    /// source files afterwards. Returns `(files_moved, bytes_moved)`.
    ///
    /// * `excludes`     — glob patterns passed as `--exclude` (e.g. `*.partial`).
    /// * `bwlimit_kbps` — rsync `--bwlimit` in KB/s (0 = unlimited).
    /// * `min_age_days` — when > 0, only move files modified more than N days
    ///   ago ("move old files first"), via `find` + `--files-from`.
    ///
    /// A destructive move (`remove_source`) first proves the destination can
    /// hold the payload; if it cannot, this returns `Err` **before** rsync runs,
    /// so nothing is deleted (ROADMAP 2.1).
    fn rsync_move(
        src: &str,
        dst: &str,
        remove_source: bool,
        excludes: &[String],
        bwlimit_kbps: u32,
        min_age_days: u32,
    ) -> Result<(u64, u64), String> {
        std::fs::create_dir_all(dst).map_err(|e| format!("create_dir {}: {}", dst, e))?;
        std::fs::create_dir_all(src).map_err(|e| format!("create_dir {}: {}", src, e))?;

        if remove_source {
            let payload = Self::ensure_destination_has_room(src, dst, min_age_days)?;
            if payload == 0 && min_age_days > 0 {
                // Age filter selected nothing: skip the rsync round-trip.
                return Ok((0, 0));
            }
        }

        let src_slash = if src.ends_with('/') {
            src.to_string()
        } else {
            format!("{}/", src)
        };
        let dst_slash = if dst.ends_with('/') {
            dst.to_string()
        } else {
            format!("{}/", dst)
        };

        // When an age filter is set, build the list of files older than N days
        // (relative paths) and let rsync transfer only those via --files-from.
        let age_list = if min_age_days > 0 {
            let list_path =
                std::env::temp_dir().join(format!("homevault-cache-age-{}.txt", Uuid::new_v4()));
            let out = run_command(
                Command::new("find").args([
                    &src_slash,
                    "-type",
                    "f",
                    "-mtime",
                    &format!("+{}", min_age_days.saturating_sub(1)),
                    "-printf",
                    "%P\\0",
                ]),
                CACHE_FIND_TIMEOUT,
            );
            match out {
                Ok(o) if o.status.success() => {
                    std::fs::write(&list_path, &o.stdout)
                        .map_err(|e| format!("write age list: {}", e))?;
                    Some(list_path)
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    return Err(format!("find failed: {}", stderr));
                }
                Err(e) => return Err(format!("failed to run find: {}", e)),
            }
        } else {
            None
        };

        let mut args: Vec<String> = vec!["-a".to_string(), "--stats".to_string()];
        if remove_source {
            args.push("--remove-source-files".to_string());
        }
        for ex in excludes {
            args.push(format!("--exclude={}", ex));
        }
        if bwlimit_kbps > 0 {
            args.push(format!("--bwlimit={}", bwlimit_kbps));
        }
        if let Some(list) = &age_list {
            args.push(format!("--files-from={}", list.display()));
            args.push("--from0".to_string());
            args.push(src_slash.clone());
            args.push(dst_slash.clone());
        } else {
            args.push(src_slash.clone());
            args.push(dst_slash.clone());
        }

        let output = run_command(Command::new("rsync").args(&args), CACHE_RSYNC_TIMEOUT);
        if let Some(list) = &age_list {
            let _ = std::fs::remove_file(list);
        }
        match output {
            // Exit 0 = full success. Exit 24 = "partial transfer due to
            // vanished source files": a benign race where a file was removed
            // by another process after rsync built its file list but before it
            // was transferred. The files rsync *did* transfer are still
            // verified and complete, so treat 24 as success rather than
            // failing the whole mover run.
            Ok(out) if out.status.success() || out.status.code() == Some(24) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if out.status.code() == Some(24) {
                    tracing::warn!(
                        "rsync reported vanished source files (exit 24) moving {} -> {}; \
                         transferred files are complete",
                        src,
                        dst
                    );
                }
                Ok(parse_rsync_stats(&stdout))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let code = out.status.code().unwrap_or(-1);
                Err(format!("rsync failed (exit {}): {}", code, stderr))
            }
            Err(e) => Err(format!("failed to run rsync: {}", e)),
        }
    }

    /// Re-point the SMB export for a cache share to its *current* active path,
    /// so network clients keep seeing the data after the mover moves it between
    /// the cache and array mounts.
    fn refresh_smb_export(share: &CacheShare, cache_mount: &str, target_mount: &str) {
        let active = Self::active_path(share, cache_mount, target_mount);
        let smb = Share {
            name: share.name.clone(),
            share_type: ShareType::Smb,
            path: active,
            enabled: share.enabled,
            comment: format!("HomeVault cache share (mode: {})", share.cache_mode),
            options: vec![],
        };
        if SharesService::update_share(
            &share.name,
            ShareType::Smb,
            ShareUpdate {
                path: Some(smb.path.clone()),
                enabled: Some(share.enabled),
                comment: Some(smb.comment.clone()),
                options: Some(vec![]),
            },
        )
        .is_err()
        {
            let _ = SharesService::create_share(&smb);
        }
    }

    /// Re-point all enabled cache shares' SMB exports after a mover run.
    fn refresh_smb_exports(settings: &CacheSettings, shares: &[CacheShare], mounts: &PoolMounts) {
        let Some(cache_pool) = &settings.cache_pool else {
            return;
        };
        let Some(cache_mount) = mounts.mount_of(cache_pool) else {
            return;
        };
        for share in shares.iter().filter(|s| s.enabled) {
            let Some(target_mount) = mounts.mount_of(&share.target_pool) else {
                continue;
            };
            Self::refresh_smb_export(share, cache_mount, target_mount);
        }
    }

    /// Record a system notification about a mover run.
    async fn notify_mover(pool: Arc<sqlx::SqlitePool>, success: bool, detail: &str) {
        let repo = NotificationRepository::new(pool);
        let (title, severity, message) = if success {
            (
                "Cache mover completed",
                "info",
                format!("Mover finished. {}", detail),
            )
        } else {
            (
                "Cache mover failed",
                "error",
                format!("Mover failed: {}", detail),
            )
        };
        let _ = repo
            .create(title, &message, severity, "cache", "cache-mover")
            .await;
    }

    /// Human-readable byte count for notifications.
    fn human_bytes(bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
        let mut size = bytes as f64;
        let mut unit = 0;
        while size >= 1024.0 && unit < UNITS.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }
        format!("{:.1} {}", size, UNITS[unit])
    }
}

/// Whether a `Prefer`-mode cache share should be drained to the array.
///
/// `min_free_percent` is the minimum free space to preserve on the cache
/// (e.g. 20 = keep at least 20% free), so the cache is "full" once free space
/// drops to/below that threshold — i.e. usage climbs to/above
/// `100 - min_free_percent`.
fn is_cache_full(usage_pct: u64, min_free_percent: u32) -> bool {
    let free_pct = 100u64.saturating_sub(usage_pct);
    free_pct <= min_free_percent as u64
}

/// Parse the `used` and `available` byte counts out of `df -B1` output.
///
/// The second line is `Filesystem 1B-blocks Used Available Use% Mounted-on`;
/// fields 2 and 3 are what the mover needs. `None` when the output is too short
/// or the numbers do not parse — the caller then refuses a destructive move
/// rather than guessing.
fn parse_df_space(stdout: &str) -> Option<(u64, u64)> {
    let line = stdout.lines().nth(1)?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let used: u64 = parts[2].parse().ok()?;
    let avail: u64 = parts[3].parse().ok()?;
    Some((used, avail))
}

/// Sum the sizes printed by `find … -printf '%s\n'`.
///
/// Every line is one file's apparent size in bytes; anything that is not a
/// number (a filename that slipped into the stream, an empty line) is ignored
/// rather than aborting the estimate.
fn sum_sizes(stdout: &str) -> u64 {
    stdout
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .fold(0u64, |acc, n| acc.saturating_add(n))
}

/// Parse `rsync --stats` output for files transferred and total bytes.
fn parse_rsync_stats(stdout: &str) -> (u64, u64) {
    // `rsync --stats` (no `-h`) formats the numbers following the process
    // locale: "1,234,567 bytes" in C, but "1 234 567" (space-grouped) or
    // "1.234.567" under some locales. Splitting on whitespace or stripping
    // only ',' then broke on those — take every ASCII digit up to the
    // trailing unit word instead. The mover never passes `-h`, so there is
    // no "1.20K" form to preserve a fraction from.
    fn first_number(rest: &str) -> u64 {
        rest.split(|c: char| c.is_ascii_alphabetic())
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    }

    let mut files = 0u64;
    let mut bytes = 0u64;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Number of regular files transferred:") {
            files = first_number(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Total transferred file size:") {
            bytes = first_number(rest);
        }
    }
    (files, bytes)
}

/// Convenience for handlers/scheduler: run the mover end-to-end against the DB.
///
/// Creates a `running` run row, performs the moves (in a blocking task), then
/// records the final status. When `job_id` is `Some`, live SSE progress is
/// emitted and a system notification is recorded on completion/failure.
///
/// Only one mover run can be in flight at a time (manual trigger and the
/// scheduler both funnel through here); a concurrent call is rejected
/// immediately rather than racing two `rsync` processes over the same
/// directories.
pub async fn run_mover(
    pool: Arc<sqlx::SqlitePool>,
    job_id: Option<String>,
) -> Result<(String, MoverResult), String> {
    if MOVER_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let msg = "Cache mover is already running".to_string();
        if let Some(jid) = &job_id {
            ProgressEmitter::fail(jid, &msg);
            ProgressEmitter::remove(jid);
        }
        return Err(msg);
    }
    let _guard = MoverRunGuard;

    let repo = CacheRepository::new(pool.clone());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;
    let shares = repo.list_shares().await.map_err(|e| e.to_string())?;

    // The single-flight guard is process-local, so a restart in the middle of a
    // move leaves its row on `running` forever. Holding the slot proves no run
    // is in flight: every other `running` row is stale.
    repo.fail_stale_runs("Interrupted by a server restart")
        .await
        .map_err(|e| e.to_string())?;

    let run_id = repo
        .create_run("running")
        .await
        .map_err(|e| e.to_string())?;

    let move_settings = settings.clone();
    let move_shares = shares.clone();
    let blocking_job = job_id.clone();
    // One pool scan for the whole run (the scan itself shells out), then the
    // moves — both off the async runtime.
    let result = tokio::task::spawn_blocking(move || {
        let mounts = PoolMounts::scan()?;
        let outcome = CacheService::perform_moves(
            &move_settings,
            &move_shares,
            &mounts,
            blocking_job.as_deref(),
        );
        outcome.map(|summary| (summary, mounts))
    })
    .await
    .map_err(|e| format!("mover task panicked: {}", e))?;

    match result {
        Ok((summary, mounts)) => {
            let status = summary.status();
            repo.record_run_shares(&run_id, &summary.outcome_records())
                .await
                .map_err(|e| e.to_string())?;
            let failed = summary.failed_shares();
            let error_message = if failed.is_empty() {
                None
            } else {
                Some(format!("Shares not moved: {}", failed.join(", ")))
            };
            repo.update_run(
                &run_id,
                status,
                summary.files_moved,
                summary.bytes_moved,
                error_message.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;

            // The data may have changed location: re-point SMB exports.
            CacheService::refresh_smb_exports(&settings, &shares, &mounts);

            let mut detail = format!(
                "{} files moved, {} transferred across {} share(s)",
                summary.files_moved,
                CacheService::human_bytes(summary.bytes_moved),
                summary.shares_processed
            );
            if !failed.is_empty() {
                detail.push_str(&format!(". Not moved: {}", failed.join(", ")));
            }

            if let Some(jid) = &job_id {
                ProgressEmitter::progress(jid, 100.0, &detail);
                if failed.is_empty() {
                    ProgressEmitter::complete(jid, "Mover completed");
                } else {
                    ProgressEmitter::complete(jid, &format!("Mover finished — {}", detail));
                }
                ProgressEmitter::remove(jid);
            }
            CacheService::notify_mover(pool.clone(), failed.is_empty(), &detail).await;

            if failed.is_empty() {
                Ok((run_id, summary))
            } else {
                // The run happened and its per-share results are recorded; the
                // scheduler must not treat it as a clean success.
                Err(format!(
                    "Cache mover finished with {} share(s) not moved: {}",
                    failed.len(),
                    failed.join(", ")
                ))
            }
        }
        Err(e) => {
            repo.update_run(&run_id, "failed", 0, 0, Some(&e))
                .await
                .map_err(|e2| e2.to_string())?;
            if let Some(jid) = &job_id {
                ProgressEmitter::fail(jid, &e);
                ProgressEmitter::remove(jid);
            }
            CacheService::notify_mover(pool.clone(), false, &e).await;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefer_mode_keeps_promoting_below_threshold() {
        // Default min_free_percent = 20: cache should keep absorbing writes
        // all the way up to 80% used, not drain as soon as it's 20% used.
        assert!(!is_cache_full(0, 20));
        assert!(!is_cache_full(19, 20));
        assert!(!is_cache_full(79, 20));
    }

    #[test]
    fn prefer_mode_drains_once_free_space_hits_threshold() {
        assert!(is_cache_full(80, 20));
        assert!(is_cache_full(95, 20));
        assert!(is_cache_full(100, 20));
    }

    // -- CacheMode ------------------------------------------------------

    #[test]
    fn test_cache_mode_parse_all_valid() {
        assert_eq!(CacheMode::parse("yes"), Some(CacheMode::Yes));
        assert_eq!(CacheMode::parse("no"), Some(CacheMode::No));
        assert_eq!(CacheMode::parse("prefer"), Some(CacheMode::Prefer));
        assert_eq!(CacheMode::parse("only"), Some(CacheMode::Only));
    }

    #[test]
    fn test_cache_mode_parse_is_case_insensitive() {
        assert_eq!(CacheMode::parse("YES"), Some(CacheMode::Yes));
        assert_eq!(CacheMode::parse("Prefer"), Some(CacheMode::Prefer));
    }

    #[test]
    fn test_cache_mode_parse_rejects_junk() {
        for bad in ["", "maybe", "y", "1", "always", " yes"] {
            assert_eq!(CacheMode::parse(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn test_cache_mode_as_str_roundtrips() {
        for m in [
            CacheMode::Yes,
            CacheMode::No,
            CacheMode::Prefer,
            CacheMode::Only,
        ] {
            assert_eq!(CacheMode::parse(m.as_str()), Some(m));
        }
    }

    #[test]
    fn test_cache_mode_serde_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&CacheMode::Prefer).unwrap(),
            "\"prefer\""
        );
        assert_eq!(
            serde_json::from_str::<CacheMode>("\"only\"").unwrap(),
            CacheMode::Only
        );
    }

    // -- CacheService::active_path -----------------------------------

    fn share(mode: &str) -> CacheShare {
        CacheShare {
            id: "id1".into(),
            name: "media".into(),
            cache_mode: mode.into(),
            target_pool: "array".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn test_active_path_mode_no_points_at_the_array() {
        assert_eq!(
            CacheService::active_path(&share("no"), "/mnt/cache", "/mnt/array"),
            "/mnt/array/media"
        );
    }

    #[test]
    fn test_active_path_caching_modes_point_at_the_cache() {
        for mode in ["yes", "prefer", "only"] {
            assert_eq!(
                CacheService::active_path(&share(mode), "/mnt/cache", "/mnt/array"),
                "/mnt/cache/media",
                "mode {mode}"
            );
        }
    }

    #[test]
    fn test_active_path_unparseable_mode_defaults_to_cache() {
        // The `_ =>` arm: an invalid stored mode must not panic — it falls
        // back to the cache path.
        assert_eq!(
            CacheService::active_path(&share("bogus"), "/mnt/cache", "/mnt/array"),
            "/mnt/cache/media"
        );
    }

    // -- parse_rsync_stats -----------------------------------------

    #[test]
    fn test_parse_rsync_stats_real_output_block() {
        let out = "\
Number of files: 12 (reg: 10, dir: 2)
Number of created files: 0
Number of regular files transferred: 7
Total file size: 1,234,567 bytes
Total transferred file size: 987,654 bytes
Literal data: 987,654 bytes
";
        assert_eq!(parse_rsync_stats(out), (7, 987_654));
    }

    #[test]
    fn test_parse_rsync_stats_missing_lines_are_zero() {
        assert_eq!(parse_rsync_stats("nothing useful here\n"), (0, 0));
        assert_eq!(parse_rsync_stats(""), (0, 0));
    }

    #[test]
    fn test_parse_rsync_stats_survives_a_locale_thousands_separator() {
        // Under a locale that groups with a space (fr_FR), `rsync --stats`
        // prints "1 234 567 bytes" — the old split-on-whitespace parse took
        // just "1". Every ASCII digit before the unit word must be kept.
        let space = "\
Number of regular files transferred: 1 234
Total transferred file size: 12 345 678 bytes
";
        assert_eq!(parse_rsync_stats(space), (1234, 12_345_678));

        let dot = "Total transferred file size: 9.876.543 bytes\n";
        assert_eq!(parse_rsync_stats(dot), (0, 9_876_543));
    }

    // -- parse_df_space --------------------------------------------

    #[test]
    fn test_parse_df_space_reads_used_and_available() {
        let out = "\
Filesystem      1B-blocks         Used    Available Use% Mounted on
/dev/sda1    500107862016 123456789012 376651073004   25% /mnt/array
";
        assert_eq!(
            parse_df_space(out),
            Some((123_456_789_012, 376_651_073_004))
        );
    }

    #[test]
    fn test_parse_df_space_rejects_unusable_output() {
        assert_eq!(parse_df_space(""), None);
        assert_eq!(
            parse_df_space("Filesystem 1B-blocks Used Available\n"),
            None
        );
        assert_eq!(
            parse_df_space("fs 100 10 nope 90% /mnt\n"),
            None,
            "a non-numeric avail must not be read as 0 free space"
        );
    }

    // -- sum_sizes --------------------------------------------------

    #[test]
    fn test_sum_sizes_adds_every_line() {
        assert_eq!(sum_sizes("0\n1\n2048\n"), 2049);
    }

    #[test]
    fn test_sum_sizes_ignores_noise_and_empty_input() {
        assert_eq!(sum_sizes(""), 0);
        assert_eq!(sum_sizes("\n\n"), 0);
        assert_eq!(sum_sizes("12\nweird\n\n7\n"), 19);
    }

    #[test]
    fn test_sum_sizes_saturates_instead_of_overflowing() {
        assert_eq!(sum_sizes(&format!("{}\n{}", u64::MAX, 5)), u64::MAX);
    }

    // -- PoolMounts -------------------------------------------------

    #[test]
    fn test_pool_mounts_resolve_known_pools_only() {
        let mounts = PoolMounts::from_pairs([("cache", "/mnt/cache"), ("array", "/mnt/array")]);
        assert_eq!(mounts.mount_of("cache"), Some("/mnt/cache"));
        assert_eq!(mounts.mount_of("array"), Some("/mnt/array"));
        assert_eq!(
            mounts.mount_of("absent"),
            None,
            "a pool that is not mounted is None, never a silent default"
        );
    }

    #[test]
    fn test_pool_mounts_skip_pools_without_a_mount_point() {
        let mounts = PoolMounts::from_pools(vec![
            crate::pools::PoolInfo {
                name: "cache".into(),
                pool_type: crate::pools::PoolType::Zfs,
                size_bytes: 1,
                used_bytes: 0,
                available_bytes: 1,
                health: "ONLINE".into(),
                device_count: 1,
                mount_point: Some("/mnt/cache".into()),
                raid_profile: None,
            },
            crate::pools::PoolInfo {
                name: "offline".into(),
                pool_type: crate::pools::PoolType::Zfs,
                size_bytes: 1,
                used_bytes: 0,
                available_bytes: 1,
                health: "OFFLINE".into(),
                device_count: 1,
                mount_point: None,
                raid_profile: None,
            },
        ]);
        assert_eq!(mounts.mount_of("cache"), Some("/mnt/cache"));
        assert_eq!(mounts.mount_of("offline"), None);
    }

    // -- MoverResult / ShareOutcome ---------------------------------

    fn outcome(name: &str, status: ShareOutcomeStatus) -> ShareOutcome {
        ShareOutcome {
            name: name.into(),
            mode: "yes".into(),
            status,
            files_moved: if status == ShareOutcomeStatus::Moved {
                3
            } else {
                0
            },
            bytes_moved: 0,
            message: (status == ShareOutcomeStatus::Failed).then(|| "boom".to_string()),
        }
    }

    fn result(statuses: &[(&str, ShareOutcomeStatus)]) -> MoverResult {
        MoverResult {
            files_moved: 0,
            bytes_moved: 0,
            shares_processed: 0,
            shares: statuses.iter().map(|(n, s)| outcome(n, *s)).collect(),
        }
    }

    #[test]
    fn test_mover_status_is_completed_when_nothing_failed() {
        use ShareOutcomeStatus::*;
        assert_eq!(result(&[]).status(), "completed");
        assert_eq!(
            result(&[("a", Moved), ("b", Skipped)]).status(),
            "completed"
        );
    }

    #[test]
    fn test_mover_status_is_partial_when_some_shares_failed() {
        use ShareOutcomeStatus::*;
        let r = result(&[("a", Moved), ("b", Failed)]);
        assert_eq!(r.status(), "partial");
        assert_eq!(r.failed_shares(), vec!["b"]);
    }

    #[test]
    fn test_mover_status_is_failed_when_every_share_failed() {
        use ShareOutcomeStatus::*;
        // A skipped share is not a success: if nothing moved and something
        // failed, the run failed.
        let r = result(&[("a", Failed), ("b", Failed)]);
        assert_eq!(r.status(), "failed");
        assert_eq!(r.failed_shares(), vec!["a", "b"]);
    }

    #[test]
    fn test_outcome_records_carry_the_failure_message() {
        use ShareOutcomeStatus::*;
        let records = result(&[("a", Moved), ("b", Failed)]).outcome_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "moved");
        assert_eq!(records[0].files_moved, 3);
        assert_eq!(records[0].message, None);
        assert_eq!(records[1].status, "failed");
        assert_eq!(records[1].message.as_deref(), Some("boom"));
    }

    #[test]
    fn test_share_outcome_status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ShareOutcomeStatus::Skipped).unwrap(),
            "\"skipped\""
        );
        assert_eq!(
            serde_json::from_str::<ShareOutcomeStatus>("\"failed\"").unwrap(),
            ShareOutcomeStatus::Failed
        );
    }

    // -- ensure_destination_has_room (ROADMAP 2.1) ------------------

    /// A destination that cannot hold the payload must be refused **before**
    /// anything is deleted. A sparse file gives a payload far larger than any
    /// real filesystem while costing no disk, so this exercises the real
    /// `find`/`df` path.
    #[test]
    fn test_ensure_destination_has_room_refuses_an_oversized_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        let big = src.join("huge.img");
        // Twice the destination's free space, as a sparse file: it costs no
        // real disk but `find -printf '%s'` reports the apparent size, so the
        // payload cannot fit even if `df` wobbles by a few blocks between the
        // two measurements.
        let avail = CacheService::df_space(dst.to_str().unwrap())
            .expect("df must work on a temp dir")
            .1;
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(avail.saturating_mul(2)).unwrap();
        drop(f);

        let err = CacheService::ensure_destination_has_room(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            0,
        )
        .expect_err("a payload larger than the free space must be refused");
        assert!(
            err.contains("not enough space"),
            "unexpected message: {err}"
        );
        assert!(big.exists(), "the source must still be there");
        assert_eq!(
            std::fs::read_dir(&dst).unwrap().count(),
            0,
            "nothing may be written to a destination we refused"
        );
    }

    #[test]
    fn test_ensure_destination_has_room_accepts_a_small_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a.txt"), vec![b'x'; 1024]).unwrap();

        let payload = CacheService::ensure_destination_has_room(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            0,
        )
        .expect("1 KiB fits anywhere");
        assert_eq!(payload, 1024);
    }

    #[test]
    fn test_ensure_destination_has_room_treats_a_missing_source_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("missing");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        assert_eq!(
            CacheService::ensure_destination_has_room(
                src.to_str().unwrap(),
                dst.to_str().unwrap(),
                0
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn test_ensure_destination_has_room_honours_the_age_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("fresh.txt"), vec![b'x'; 4096]).unwrap();

        // A file written just now is not "older than 30 days", so the payload
        // that would actually move is empty — no refusal, and no rsync.
        assert_eq!(
            CacheService::ensure_destination_has_room(
                src.to_str().unwrap(),
                dst.to_str().unwrap(),
                30
            )
            .unwrap(),
            0
        );
    }

    // -- perform_moves: one failure must not abort the run (ROADMAP 2.2) --

    fn cache_share(name: &str, target: &str) -> CacheShare {
        CacheShare {
            id: format!("id-{name}"),
            name: name.into(),
            cache_mode: "yes".into(),
            target_pool: target.into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn settings_for(cache_pool: &str) -> CacheSettings {
        CacheSettings {
            cache_pool: Some(cache_pool.into()),
            mover_cron: "0 3 * * *".into(),
            mover_enabled: true,
            min_free_percent: 20,
            mover_last_run_at: None,
            mover_age_days: 0,
            mover_excludes: None,
            mover_bwlimit: 0,
        }
    }

    #[test]
    fn test_perform_moves_records_a_failed_share_and_still_moves_the_next_one() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let array = tmp.path().join("array");
        std::fs::create_dir_all(cache.join("broken")).unwrap();
        std::fs::create_dir_all(cache.join("good")).unwrap();
        std::fs::create_dir_all(&array).unwrap();
        std::fs::write(cache.join("broken/a.txt"), b"x").unwrap();
        std::fs::write(cache.join("good/b.txt"), b"y").unwrap();

        let mounts = PoolMounts::from_pairs([
            ("cache", cache.to_str().unwrap()),
            ("array", array.to_str().unwrap()),
        ]);
        // First share points at a pool that is not mounted; the second is fine.
        let shares = vec![cache_share("broken", "ghost"), cache_share("good", "array")];
        let summary =
            CacheService::perform_moves(&settings_for("cache"), &shares, &mounts, None).unwrap();

        assert_eq!(summary.shares.len(), 2, "every share is reported");
        assert_eq!(summary.shares[0].status, ShareOutcomeStatus::Failed);
        assert!(
            summary.shares[0]
                .message
                .as_deref()
                .unwrap()
                .contains("ghost"),
            "the failure names the unmounted pool"
        );
        assert_eq!(
            summary.shares[1].status,
            ShareOutcomeStatus::Moved,
            "a failed share must not strand the rest of the queue"
        );
        assert_eq!(summary.status(), "partial");
        assert_eq!(summary.failed_shares(), vec!["broken"]);
        assert_eq!(summary.shares_processed, 1);
        assert!(
            cache.join("broken/a.txt").exists(),
            "nothing is deleted for the share that failed"
        );
    }

    #[test]
    fn test_perform_moves_without_a_cache_pool_is_a_run_level_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mounts = PoolMounts::from_pairs([("cache", tmp.path().to_str().unwrap())]);
        let mut settings = settings_for("cache");
        settings.cache_pool = None;
        let err = CacheService::perform_moves(&settings, &[], &mounts, None).unwrap_err();
        assert!(err.contains("No cache pool"), "{err}");
    }

    #[test]
    fn test_perform_moves_refuses_an_unmounted_cache_pool() {
        let mounts = PoolMounts::from_pairs([("array", "/mnt/array")]);
        let err =
            CacheService::perform_moves(&settings_for("cache"), &[], &mounts, None).unwrap_err();
        assert!(err.contains("not mounted"), "{err}");
    }

    #[test]
    fn test_perform_moves_skips_an_empty_share_without_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let array = tmp.path().join("array");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&array).unwrap();

        let mounts = PoolMounts::from_pairs([
            ("cache", cache.to_str().unwrap()),
            ("array", array.to_str().unwrap()),
        ]);
        let summary = CacheService::perform_moves(
            &settings_for("cache"),
            &[cache_share("empty", "array")],
            &mounts,
            None,
        )
        .unwrap();

        assert_eq!(summary.shares[0].status, ShareOutcomeStatus::Skipped);
        assert_eq!(summary.status(), "completed");
    }

    #[test]
    fn test_perform_moves_actually_moves_a_file() {
        if run_timed(
            Command::new("rsync").arg("--version"),
            Duration::from_secs(10),
        )
        .is_err()
        {
            return; // rsync not installed: the move path cannot be exercised.
        }
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let array = tmp.path().join("array");
        std::fs::create_dir_all(cache.join("media")).unwrap();
        std::fs::create_dir_all(&array).unwrap();
        std::fs::write(cache.join("media/movie.mkv"), vec![b'z'; 8192]).unwrap();

        let mounts = PoolMounts::from_pairs([
            ("cache", cache.to_str().unwrap()),
            ("array", array.to_str().unwrap()),
        ]);
        let summary = CacheService::perform_moves(
            &settings_for("cache"),
            &[cache_share("media", "array")],
            &mounts,
            None,
        )
        .unwrap();

        assert_eq!(summary.shares[0].status, ShareOutcomeStatus::Moved);
        assert_eq!(summary.files_moved, 1);
        assert_eq!(summary.bytes_moved, 8192);
        assert_eq!(summary.status(), "completed");
        assert!(array.join("media/movie.mkv").exists());
        assert!(
            !cache.join("media/movie.mkv").exists(),
            "the source copy must be removed once it is safely on the array"
        );
    }

    // -- ROADMAP X.2: the mover's host tools are bounded ---------------------

    /// The defect class this guards against is invisible to a behavioural
    /// test: a bare `.output()` still answers correctly on an idle box and only
    /// pins the blocking thread — **and** the process-wide single-flight slot,
    /// so no mover ever runs again — when `rsync`/`find`/`df` stops answering.
    /// So the invariant is asserted at the source.
    ///
    /// `concat!` keeps each pattern out of this file's own source, so the
    /// assertion cannot count itself.
    #[test]
    fn test_no_mover_subprocess_runs_without_a_deadline() {
        let src = include_str!("cache.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        for unbounded in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!("wait_with_", "output()"),
        ] {
            assert!(
                !production.contains(unbounded),
                "`{unbounded}` waits without a deadline — use `run_command` (ROADMAP X.2)"
            );
        }
        // Every host command is wrapped: one `Command::new` per tool, one
        // `run_command` per `Command::new` (the helper's own definition is the
        // extra match, and is not a `Command::new`).
        let commands = production.matches("Command::new(").count();
        let bounded = production.matches("run_command(").count();
        assert!(commands >= 4, "the mover lost a host command: {commands}");
        assert_eq!(
            bounded,
            commands + 1,
            "a host command of the mover is not wrapped in `run_command`: \
             {commands} Command::new vs {bounded} run_command"
        );
    }

    #[test]
    fn test_run_command_kills_a_hung_tool_and_names_the_deadline() {
        let started = std::time::Instant::now();
        let error = run_command(Command::new("sleep").arg("30"), Duration::from_millis(120))
            .expect_err("a hung tool must not succeed");
        assert!(error.contains("did not finish within"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must be enforced, not waited out"
        );
    }

    #[test]
    fn test_run_command_keeps_a_non_zero_exit_as_data() {
        // `rsync_move` branches on the exit *status* (exit 24 means "vanished
        // source files", which is success for the files that did transfer), so
        // a non-zero exit must arrive as output, never as an error.
        let out = run_command(&mut Command::new("false"), Duration::from_secs(5))
            .expect("`false` still runs to completion");
        assert_eq!(out.status.code(), Some(1));
    }

    #[test]
    fn test_run_command_reports_a_missing_tool() {
        let error = run_command(
            &mut Command::new("homevault-no-such-binary"),
            Duration::from_secs(1),
        )
        .expect_err("a missing tool is an error");
        assert!(error.contains("Failed to run"), "{error}");
    }

    #[test]
    fn test_mover_deadlines_are_ceilings_and_ordered() {
        // `df` is a mount-table read; `find` walks the tier; `rsync` moves it.
        assert!(CACHE_QUERY_TIMEOUT >= Duration::from_secs(5));
        assert!(CACHE_FIND_TIMEOUT >= CACHE_QUERY_TIMEOUT);
        assert!(CACHE_RSYNC_TIMEOUT >= CACHE_FIND_TIMEOUT);
        // Still finite: a wedged transfer must free the single-flight slot.
        assert!(CACHE_RSYNC_TIMEOUT <= Duration::from_secs(12 * 60 * 60));
    }

    #[test]
    fn test_df_space_answers_none_when_the_tool_fails() {
        // The free-space gate reads `df` *before* `rsync
        // --remove-source-files` deletes anything, so "no answer" must be a
        // clean `None` (the caller then refuses the move) rather than a panic
        // or a fabricated 0.
        assert_eq!(CacheService::df_space("/definitely/not/a/mount"), None);
    }
}
