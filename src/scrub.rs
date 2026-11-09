/* =============================================================================
 * File          : scrub.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One entry point for a data-consistency check (scrub) on a virtual disk.
 *   Used by the scheduled-task engine (`scrub` task type) and by the
 *   boot-recovery pass after an unclean shutdown, so both paths run the same
 *   argv, the same timeout and the same single-flight lock.
 *
 * Dependencies:
 *   - tokio, crate::pools
 *
 * Usage:
 *   `scrub::run(pool, kind).await`; see docs/guides/scheduled-tasks.md.
 * =============================================================================
 */

//! Data-consistency scrub for ZFS, Btrfs and mdadm.

use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

use crate::pools::{PoolService, PoolType};
use crate::utils::process::run_timed_async;

/// A scrub start is asynchronous in the kernel; this only bounds the command
/// that *starts* it. A wedged `zpool`/`btrfs` must not hold a scheduled task
/// or the boot-recovery pass for ever.
const SCRUB_START_TIMEOUT: Duration = Duration::from_secs(30);

/// One scrub at a time across the whole process. A scheduled task and the
/// post-unclean-shutdown pass must never start two scrubs on the same box.
pub static SCRUB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The durable marker naming the scrub that was started last. It survives a
/// crash or a reboot, which is what lets the boot pass resume *that* scrub
/// instead of starting an unrelated one.
fn marker_path() -> PathBuf {
    crate::config::paths::data_dir()
        .join("state")
        .join("scrub-in-progress")
}

/// One scrub recorded on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedScrub {
    pub pool: String,
    pub kind: ScrubKind,
}

/// What to do with a recorded scrub, given the kernel/library status string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDecision {
    /// The scrub is still running — leave it alone.
    LeaveRunning,
    /// The scrub finished (or the pool is gone) — forget the marker.
    Clear,
    /// The scrub was interrupted — start it again from the beginning.
    Restart,
}

/// ZFS prints `scan: scrub in progress since …`, Btrfs `Status: running`,
/// mdadm `check`/`resync` in `/sys/block/*/md/sync_action`. A finished scrub
/// prints `repaired`/`finished`, and an idle array prints `idle`.
pub fn resume_decision(status: &str) -> ResumeDecision {
    let s = status.trim().to_ascii_lowercase();
    if s.is_empty() {
        return ResumeDecision::Restart;
    }
    if s.contains("in progress")
        || s.contains("running")
        || s.contains("resync")
        || s.contains("reshape")
        || s.starts_with("check")
    {
        return ResumeDecision::LeaveRunning;
    }
    if s.contains("finished")
        || s.contains("completed")
        || s.contains("repaired")
        || s.contains("no errors")
        || s.contains("no scrub")
        || s.contains("not available")
        || s.contains("unsupported")
    {
        return ResumeDecision::Clear;
    }
    // `idle` on mdadm, or an output we do not recognise: no scrub is running
    // now, so the recorded one was interrupted.
    ResumeDecision::Restart
}

fn encode_marker(pool: &str, kind: ScrubKind) -> String {
    format!("pool={}\nkind={}\n", pool, kind.as_str())
}

fn decode_marker(raw: &str) -> Option<InterruptedScrub> {
    let mut pool = None;
    let mut kind = None;
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("pool=") {
            pool = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("kind=") {
            kind = ScrubKind::parse(value);
        }
    }
    let pool = pool.filter(|p| !p.is_empty())?;
    Some(InterruptedScrub { pool, kind: kind? })
}

/// Persist the scrub that just started. Atomic, so a crash mid-write leaves
/// either the previous marker or the new one, never a half file.
pub fn record_interrupted(pool: &str, kind: ScrubKind) {
    let path = marker_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = crate::storage::atomic::AtomicWriter::write(&path, &encode_marker(pool, kind)) {
        tracing::warn!("could not record the scrub marker: {e}");
    }
}

pub fn clear_interrupted() {
    let _ = std::fs::remove_file(marker_path());
}

pub fn load_interrupted() -> Option<InterruptedScrub> {
    let raw = std::fs::read_to_string(marker_path()).ok()?;
    decode_marker(&raw)
}

/// Finish or resume the scrub a previous process started. `Ok(None)` means
/// there was nothing to resume; `Ok(Some(message))` is the message to log.
pub async fn resume_interrupted() -> Result<Option<String>, String> {
    let Some(marker) = load_interrupted() else {
        return Ok(None);
    };
    let pool = marker.pool.clone();
    let pool_type = marker.kind.pool_type();
    let status = tokio::task::spawn_blocking(move || {
        PoolService::get_scrub_status(&pool, pool_type)
            .map(|s| s)
            .unwrap_or_else(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("scrub status check panicked: {e}"))?;

    match resume_decision(&status) {
        ResumeDecision::LeaveRunning => Ok(Some(format!(
            "an interrupted scrub on '{}' is still running; not starting another",
            marker.pool
        ))),
        ResumeDecision::Clear => {
            clear_interrupted();
            Ok(None)
        }
        ResumeDecision::Restart => {
            let message = run(&marker.pool, marker.kind).await?;
            Ok(Some(format!(
                "resumed the interrupted scrub on '{}': {message}",
                marker.pool
            )))
        }
    }
}

/// The storage technology a pool name belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubKind {
    Zfs,
    Btrfs,
    Mdadm,
}

impl ScrubKind {
    /// Parse the stable wire value stored in a scheduled task payload.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "zfs" => Some(Self::Zfs),
            "btrfs" => Some(Self::Btrfs),
            "mdadm" | "md" | "raid" => Some(Self::Mdadm),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zfs => "zfs",
            Self::Btrfs => "btrfs",
            Self::Mdadm => "mdadm",
        }
    }

    fn pool_type(self) -> PoolType {
        match self {
            Self::Zfs => PoolType::Zfs,
            Self::Btrfs => PoolType::Btrfs,
            Self::Mdadm => PoolType::Mdadm,
        }
    }
}

/// The device node `mdadm --action check` needs. Accepts either the
/// normalised pool name (`md0`) or an explicit `/dev/md/...` path.
pub fn mdadm_device(pool: &str) -> String {
    let pool = pool.trim();
    if pool.starts_with("/dev/") {
        pool.to_string()
    } else {
        format!("/dev/md/{pool}")
    }
}

/// Start one scrub and return a human-readable sentence for the task log.
pub async fn run(pool: &str, kind: ScrubKind) -> Result<String, String> {
    let _guard = SCRUB_LOCK.lock().await;

    if kind == ScrubKind::Mdadm {
        let device = mdadm_device(pool);
        let mut cmd = Command::new("sudo");
        cmd.args(["mdadm", "--action", "check", &device]);
        let output = run_timed_async(&mut cmd, SCRUB_START_TIMEOUT)
            .await
            .ok_or_else(|| format!("mdadm --action check {device} timed out"))?;
        if !output.status.success() {
            return Err(format!(
                "mdadm --action check {device} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        record_interrupted(pool, kind);
        return Ok(format!("mdadm consistency check started on {device}"));
    }

    let started_pool = pool.to_string();
    let worker_pool = started_pool.clone();
    let pool_type = kind.pool_type();
    let label = kind.as_str().to_string();
    let started = tokio::task::spawn_blocking(move || {
        PoolService::start_scrub(&worker_pool, pool_type)
            .map(|_| format!("{label} scrub started on {worker_pool}"))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("scrub task panicked: {e}"))??;
    // Recorded only after the start command succeeded: a refused start must
    // not leave a marker that makes every boot retry the same failure.
    record_interrupted(&started_pool, kind);
    Ok(started)
}

/// The pool types a scheduled scrub can target.
pub fn supports(pool_type: &str) -> bool {
    ScrubKind::parse(pool_type).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parses_the_three_storage_technologies() {
        assert_eq!(ScrubKind::parse("zfs"), Some(ScrubKind::Zfs));
        assert_eq!(ScrubKind::parse("BTRFS"), Some(ScrubKind::Btrfs));
        assert_eq!(ScrubKind::parse("mdadm"), Some(ScrubKind::Mdadm));
        assert_eq!(ScrubKind::parse("md"), Some(ScrubKind::Mdadm));
        assert_eq!(ScrubKind::parse("ext4"), None);
        assert_eq!(ScrubKind::parse(""), None);
        assert!(supports("btrfs"));
        assert!(!supports("ext4"));
    }

    #[test]
    fn mdadm_device_accepts_a_name_or_a_full_path() {
        assert_eq!(mdadm_device("md0"), "/dev/md/md0");
        assert_eq!(mdadm_device("/dev/md127"), "/dev/md127");
        assert_eq!(mdadm_device("  /dev/md/storage "), "/dev/md/storage");
    }

    #[test]
    fn wire_names_are_stable() {
        assert_eq!(ScrubKind::Zfs.as_str(), "zfs");
        assert_eq!(ScrubKind::Btrfs.as_str(), "btrfs");
        assert_eq!(ScrubKind::Mdadm.as_str(), "mdadm");
    }

    #[test]
    fn the_marker_round_trips_and_ignores_unknown_fields() {
        let raw = encode_marker("tank", ScrubKind::Zfs);
        assert_eq!(
            decode_marker(&raw),
            Some(InterruptedScrub {
                pool: "tank".to_string(),
                kind: ScrubKind::Zfs,
            })
        );
        assert_eq!(
            decode_marker("kind=btrfs\npool=photos\nstarted=whatever\n"),
            Some(InterruptedScrub {
                pool: "photos".to_string(),
                kind: ScrubKind::Btrfs,
            })
        );
        assert!(decode_marker("pool=tank\n").is_none());
        assert!(decode_marker("kind=ext4\npool=tank\n").is_none());
        assert!(decode_marker("pool=\nkind=zfs\n").is_none());
    }

    #[test]
    fn a_finished_or_running_scrub_is_not_restarted() {
        assert_eq!(
            resume_decision("scan: scrub in progress since Thu Nov 5 10:00:00 2026"),
            ResumeDecision::LeaveRunning
        );
        assert_eq!(
            resume_decision("Status: running"),
            ResumeDecision::LeaveRunning
        );
        assert_eq!(resume_decision("check"), ResumeDecision::LeaveRunning);
        assert_eq!(resume_decision("resync"), ResumeDecision::LeaveRunning);
        assert_eq!(
            resume_decision("scan: scrub repaired 0B in 02:03:04 with 0 errors"),
            ResumeDecision::Clear
        );
        assert_eq!(resume_decision("Status: finished"), ResumeDecision::Clear);
        assert_eq!(
            resume_decision("No scrub in progress"),
            ResumeDecision::Clear
        );
        assert_eq!(resume_decision("Not available"), ResumeDecision::Clear);
        assert_eq!(resume_decision("idle"), ResumeDecision::Restart);
        assert_eq!(resume_decision(""), ResumeDecision::Restart);
    }
}
