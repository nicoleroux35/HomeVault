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
use crate::storage::cache_repo::{CacheRepository, CacheSettings, CacheShare};
use crate::storage::NotificationRepository;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing;
use uuid::Uuid;

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
    pub shares_processed: u32,
}

/// Service for managing the cache tier and running the mover.
pub struct CacheService;

impl CacheService {
    /// Resolve a pool name to its mount point, if mounted.
    fn pool_mount_point(pool_name: &str) -> Option<String> {
        let pools = PoolService::scan_pools().ok()?;
        pools
            .into_iter()
            .find(|p| p.name == pool_name)
            .and_then(|p| p.mount_point)
    }

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

    /// Run the mover. Resolves pool mounts from `settings` and `shares`, then
    /// moves data between cache and array per each share's mode. This is
    /// CPU/IO-bound (it shells out to `rsync`), so callers should invoke it
    /// inside `tokio::task::spawn_blocking`.
    ///
    /// `job_id`, when set, receives live SSE progress updates.
    pub fn perform_moves(
        settings: &CacheSettings,
        shares: &[CacheShare],
        job_id: Option<&str>,
    ) -> Result<MoverResult, String> {
        let cache_pool = settings
            .cache_pool
            .as_ref()
            .ok_or_else(|| "No cache pool configured".to_string())?;

        let cache_mount = Self::pool_mount_point(cache_pool)
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

        let bwlimit = settings.mover_bwlimit;
        let age_days = settings.mover_age_days;

        let targets: Vec<&CacheShare> = shares.iter().filter(|s| s.enabled).collect();
        let total = targets.len().max(1) as f64;

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

            let target_mount = match Self::pool_mount_point(&share.target_pool) {
                Some(m) => m,
                None => {
                    tracing::warn!(
                        "Cache share '{}' target pool '{}' not mounted; skipping",
                        share.name,
                        share.target_pool
                    );
                    continue;
                }
            };

            let cache_dir = format!("{}/{}", cache_mount, share.name);
            let array_dir = format!("{}/{}", target_mount, share.name);

            let mode = match CacheMode::parse(&share.cache_mode) {
                Some(m) => m,
                None => {
                    tracing::warn!(
                        "Cache share '{}' has unknown mode '{}'",
                        share.name,
                        share.cache_mode
                    );
                    continue;
                }
            };

            let moved = match mode {
                CacheMode::No => {
                    // Data belongs on the array; move anything still on cache.
                    if Self::dir_has_content(&cache_dir) {
                        Self::rsync_move(
                            &cache_dir, &array_dir, true, &excludes, bwlimit, age_days,
                        )?
                    } else {
                        (0, 0)
                    }
                }
                CacheMode::Yes => {
                    // Drain cache to array.
                    if Self::dir_has_content(&cache_dir) {
                        Self::rsync_move(
                            &cache_dir, &array_dir, true, &excludes, bwlimit, age_days,
                        )?
                    } else {
                        (0, 0)
                    }
                }
                CacheMode::Only => {
                    // Data belongs on cache; move anything on the array back.
                    if Self::dir_has_content(&array_dir) {
                        Self::rsync_move(
                            &array_dir, &cache_dir, true, &excludes, bwlimit, age_days,
                        )?
                    } else {
                        (0, 0)
                    }
                }
                CacheMode::Prefer => {
                    let cache_full = is_cache_full(
                        Self::pool_usage_pct(&cache_mount),
                        settings.min_free_percent,
                    );
                    if cache_full {
                        // Cache is (nearly) full: drain to the array.
                        if Self::dir_has_content(&cache_dir) {
                            Self::rsync_move(
                                &cache_dir, &array_dir, true, &excludes, bwlimit, age_days,
                            )?
                        } else {
                            (0, 0)
                        }
                    } else {
                        // Cache has room: keep/promote data onto cache.
                        if Self::dir_has_content(&array_dir) {
                            Self::rsync_move(
                                &array_dir, &cache_dir, true, &excludes, bwlimit, age_days,
                            )?
                        } else {
                            (0, 0)
                        }
                    }
                }
            };

            total_files += moved.0;
            total_bytes += moved.1;
            processed += 1;
        }

        Ok(MoverResult {
            files_moved: total_files,
            bytes_moved: total_bytes,
            shares_processed: processed,
        })
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

    /// Returns the used-percentage (0-100) of the filesystem holding `path`,
    /// via `df`. Used to decide whether `prefer` shares should be drained.
    fn pool_usage_pct(path: &str) -> u64 {
        let output = Command::new("df").args(["-B1", path]).output();
        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();
            if lines.len() >= 2 {
                let parts: Vec<&str> = lines[1].split_whitespace().collect();
                if parts.len() >= 5 {
                    let used: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let avail: u64 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let total = used + avail;
                    if let Some(pct) = (used * 100).checked_div(total) {
                        return pct;
                    }
                }
            }
        }
        0
    }

    /// Move data from `src` into `dst` with `rsync`, optionally deleting the
    /// source files afterwards. Returns `(files_moved, bytes_moved)`.
    ///
    /// * `excludes`     — glob patterns passed as `--exclude` (e.g. `*.partial`).
    /// * `bwlimit_kbps` — rsync `--bwlimit` in KB/s (0 = unlimited).
    /// * `min_age_days` — when > 0, only move files modified more than N days
    ///   ago ("move old files first"), via `find` + `--files-from`.
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
            let out = Command::new("find")
                .args([
                    &src_slash,
                    "-type",
                    "f",
                    "-mtime",
                    &format!("+{}", min_age_days.saturating_sub(1)),
                    "-printf",
                    "%P\\0",
                ])
                .output();
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

        let output = Command::new("rsync").args(&args).output();
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
    fn refresh_smb_export(share: &CacheShare, cache_pool: &str, target_pool: &str) {
        let (Some(cache_mount), Some(target_mount)) = (
            Self::pool_mount_point(cache_pool),
            Self::pool_mount_point(target_pool),
        ) else {
            return;
        };
        let active = Self::active_path(share, &cache_mount, &target_mount);
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
    fn refresh_smb_exports(settings: &CacheSettings, shares: &[CacheShare]) {
        let Some(cache_pool) = &settings.cache_pool else {
            return;
        };
        for share in shares.iter().filter(|s| s.enabled) {
            Self::refresh_smb_export(share, cache_pool, &share.target_pool);
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

    let run_id = repo
        .create_run("running")
        .await
        .map_err(|e| e.to_string())?;

    let move_settings = settings.clone();
    let move_shares = shares.clone();
    let blocking_job = job_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        CacheService::perform_moves(&move_settings, &move_shares, blocking_job.as_deref())
    })
    .await
    .map_err(|e| format!("mover task panicked: {}", e))?;

    match result {
        Ok(summary) => {
            repo.update_run(
                &run_id,
                "completed",
                summary.files_moved,
                summary.bytes_moved,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
            // The data may have changed location: re-point SMB exports.
            CacheService::refresh_smb_exports(&settings, &shares);
            let detail = format!(
                "{} files moved, {} transferred across {} share(s)",
                summary.files_moved,
                CacheService::human_bytes(summary.bytes_moved),
                summary.shares_processed
            );
            if let Some(jid) = &job_id {
                ProgressEmitter::progress(jid, 100.0, &detail);
                ProgressEmitter::complete(jid, "Mover completed");
                ProgressEmitter::remove(jid);
            }
            CacheService::notify_mover(pool.clone(), true, &detail).await;
            Ok((run_id, summary))
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
}
