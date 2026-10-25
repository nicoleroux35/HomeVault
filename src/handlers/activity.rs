/* =============================================================================
 * File          : activity.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Unified background-activity feed for the desktop taskbar: merges every
 *   active job of the progress manager (backups, restores, cache mover,
 *   antivirus scans, file copies/moves, ...) with in-app download tasks
 *   into one live list. Read-only and session-auth.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   GET /api/activity in src/web/mod.rs; consumed by the desktop shell's
 *   activity taskbar.
 * =============================================================================
 */

//! Unified background-activity feed for the desktop taskbar.
//!
//! `GET /api/activity` merges every **active** `PROGRESS_MANAGER` job (restic
//! backup/restore/verify, cache mover, antivirus scan, File-Explorer copy/move,
//! Docker install/update, ddrescue/photorec) with aria2's currently-active
//! downloads. It is a read-only endpoint (`require_auth`).
//!
//! Since ROADMAP 28.3 the list also carries the **synthetic** jobs written by
//! `crate::activity_poller` — one row per live host operation HomeVault did not
//! start itself: an mdadm array resyncing/recovering/reshaping/checking
//! (`/proc/mdstat`), a ZFS or btrfs scrub in progress, and a Syncthing folder
//! that is still catching up. They are ordinary `PROGRESS_MANAGER` jobs pushed
//! by a background poller, so this handler needs no branch for them: the poller
//! creates them with a `kind` of `raid_resync` / `pool_scrub` /
//! `syncthing_sync` and no owner, which makes them admin-only like every other
//! ownerless job.
//!
//! Scoping: a non-admin only sees jobs they own (`owner == their username`);
//! an admin sees every job. The aria2 branch additionally requires admin and
//! **never starts aria2** — it checks the live process handle first, so polling
//! this endpoint has no side effect.
//!
//! The handler returns a stable `kind` slug per row; the frontend localises it
//! via `desktop.activity_kind_<slug>` (client-side i18n, like the rest of the
//! desktop shell).

use crate::handlers::admin_guard::require_auth;
use crate::handlers::downloader::{aria2_active_tasks, aria2_is_running, Aria2Task};
use crate::handlers::AppState;
use crate::sse::{ActivitySnapshot, JobType, ProgressState, PROGRESS_MANAGER};
use crate::utils::ResponseHelper;
use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;

/// How long a finished-with-failure job stays in the feed.
///
/// A job that fails has to be *seen*: it used to leave the list on the very
/// next poll (the filter only kept `pending`/`running`), so the row the user
/// was watching simply vanished — while the bell got a notification they were
/// not looking at. One minute is long enough for the panel to be open, short
/// enough that the taskbar is not a graveyard (ROADMAP 28.2).
pub const FAILED_GRACE: chrono::Duration = chrono::Duration::seconds(60);

/// One row in the desktop taskbar.
#[derive(Serialize, Debug, PartialEq)]
pub struct ActivityTask {
    /// `PROGRESS_MANAGER` job uuid, or `"aria2:<gid>"` for a download.
    pub id: String,
    /// Stable slug — the frontend maps it to a localised label.
    pub kind: String,
    /// Lifecycle slug: `"pending"`, `"running"`, `"failed"` or `"cancelled"`.
    ///
    /// The row's *state*, not a label: the client renders a failed job as a
    /// failure (no progress bar, a status word) and keeps it out of the active
    /// count. Before this field existed the feed had no way to say "this is
    /// over and it did not work", which is why a failure had to be hidden.
    pub state: String,
    /// Dynamic name (torrent / file). Empty for a generic job (the frontend
    /// then falls back to the localised `kind` label).
    pub label: String,
    /// `0..=100`, or `None` for an indeterminate job (sliding bar, no `%`).
    pub progress: Option<f64>,
    /// Latest message / "12/40 files" / "8.4 MB/s". Never carries a secret.
    pub detail: Option<String>,
}

/// `GET /api/activity`
pub async fn handle_activity(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let is_admin = state.auth.is_admin(&username).await;

    let now = chrono::Utc::now();
    let mut tasks: Vec<ActivityTask> = PROGRESS_MANAGER
        .list_activity()
        .into_iter()
        .filter(|s| is_visible(s, now))
        .filter(|s| is_admin || s.owner.as_deref() == Some(username.as_str()))
        .map(activity_task_from_snapshot)
        .collect();

    // aria2 downloads: admin-only, and only when the daemon is already up.
    if is_admin && aria2_is_running(&state.downloader).await {
        if let Ok(active) = aria2_active_tasks(&state.downloader).await {
            tasks.extend(active.into_iter().filter_map(activity_task_from_aria2));
        }
    }

    ResponseHelper::json_success(tasks).into_response()
}

/// Whether a job belongs in the feed at `now`.
///
/// Live work always does. A job that **failed** or was **cancelled** stays for
/// [`FAILED_GRACE`] after its last event so the user sees the outcome of the
/// row they were watching, then drops out. A job that **completed** is not
/// shown: the screen that started it reports its own success, and the feed is
/// about work in progress (ROADMAP 28.2).
fn is_visible(s: &ActivitySnapshot, now: chrono::DateTime<chrono::Utc>) -> bool {
    match s.state {
        ProgressState::Pending | ProgressState::Running => true,
        ProgressState::Failed | ProgressState::Cancelled => s
            .updated_at
            .map(|at| now.signed_duration_since(at) <= FAILED_GRACE)
            .unwrap_or(false),
        ProgressState::Completed => false,
    }
}

/// Lifecycle slug for the row's `state` field — the client branches on it.
fn state_slug(s: &ProgressState) -> &'static str {
    match s {
        ProgressState::Pending => "pending",
        ProgressState::Running => "running",
        ProgressState::Completed => "completed",
        ProgressState::Failed => "failed",
        ProgressState::Cancelled => "cancelled",
    }
}

/// Map a `JobType` to the stable slug the frontend localises.
fn job_type_kind(t: &JobType) -> String {
    match t {
        JobType::Backup => "backup".to_string(),
        JobType::Restore => "restore".to_string(),
        JobType::Verify => "verify".to_string(),
        JobType::Upload => "upload".to_string(),
        JobType::Download => "download".to_string(),
        JobType::Sync => "sync".to_string(),
        JobType::CacheMover => "cache_mover".to_string(),
        // "antivirus_scan", "explorer_copy", "explorer_move", "docker_install",
        // "docker_update", "rescue_ddrescue", "rescue_photorec", ...
        JobType::Custom(slug) => slug.clone(),
    }
}

fn activity_task_from_snapshot(s: ActivitySnapshot) -> ActivityTask {
    let mut detail = (!s.message.is_empty()).then(|| s.message.clone());
    if let Some(bps) = s.speed_bps.filter(|b| *b > 0) {
        let human = format!("{}/s", format_bytes(bps));
        detail = Some(match detail {
            Some(d) => format!("{d} · {human}"),
            None => human,
        });
    }
    ActivityTask {
        id: s.id,
        kind: job_type_kind(&s.job_type),
        state: state_slug(&s.state).to_string(),
        label: String::new(),
        progress: s.progress.map(|p| p.clamp(0.0, 100.0)),
        detail,
    }
}

fn activity_task_from_aria2(t: Aria2Task) -> Option<ActivityTask> {
    if t.status != "active" {
        return None;
    }
    let completed: u64 = t.completed_length.parse().unwrap_or(0);
    let total: u64 = t.total_length.parse().unwrap_or(0);
    let progress =
        (total > 0).then(|| ((completed as f64 / total as f64) * 100.0).clamp(0.0, 100.0));
    let speed: u64 = t.download_speed.parse().unwrap_or(0);
    let detail = (speed > 0).then(|| format!("{}/s", format_bytes(speed)));
    let kind = if t.bittorrent.is_some() {
        "download_torrent"
    } else {
        "download_url"
    };
    Some(ActivityTask {
        id: format!("aria2:{}", t.gid),
        kind: kind.to_string(),
        // aria2 only ever reaches this branch while it reports `active`.
        state: "running".to_string(),
        label: aria2_name(&t),
        progress,
        detail,
    })
}

/// Same derivation as `Downloader.svelte`: torrent name → first file's basename
/// → first URI → gid.
fn aria2_name(t: &Aria2Task) -> String {
    if let Some(bt) = &t.bittorrent {
        if !bt.name.is_empty() {
            return bt.name.clone();
        }
    }
    if let Some(f) = t.files.first() {
        if !f.path.is_empty() {
            if let Some(base) = std::path::Path::new(&f.path).file_name() {
                return base.to_string_lossy().into_owned();
            }
        }
        if let Some(u) = f.uris.first() {
            if !u.uri.is_empty() {
                return u.uri.clone();
            }
        }
    }
    t.gid.clone()
}

/// Compact human byte size (`1.4 MB`, `912 KB`, `340 B`).
fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let f = n as f64;
    if f >= GB {
        format!("{:.1} GB", f / GB)
    } else if f >= MB {
        format!("{:.1} MB", f / MB)
    } else if f >= KB {
        format!("{:.0} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    //! Pure mapping tests — no DB, no HTTP.
    //! Run: `cargo test activity::tests`
    use super::*;
    use crate::handlers::downloader::{Aria2BtInfo, Aria2File, Aria2Task, Aria2Uri};

    fn task(status: &str) -> Aria2Task {
        Aria2Task {
            gid: "abc123".into(),
            status: status.into(),
            total_length: "0".into(),
            completed_length: "0".into(),
            download_speed: "0".into(),
            upload_speed: "0".into(),
            error_message: None,
            bittorrent: None,
            files: vec![],
            dir: "/tmp".into(),
        }
    }

    fn file(path: &str, uri: &str) -> Aria2File {
        Aria2File {
            index: "1".into(),
            path: path.into(),
            length: "0".into(),
            completed_length: "0".into(),
            selected: "true".into(),
            uris: if uri.is_empty() {
                vec![]
            } else {
                vec![Aria2Uri {
                    uri: uri.into(),
                    status: "used".into(),
                }]
            },
        }
    }

    fn snapshot(job_type: JobType) -> ActivitySnapshot {
        ActivitySnapshot {
            id: "job-1".into(),
            job_type,
            owner: None,
            state: ProgressState::Running,
            updated_at: None,
            progress: None,
            message: String::new(),
            bytes_processed: None,
            bytes_total: None,
            speed_bps: None,
        }
    }

    #[test]
    fn test_job_type_kind_covers_all_variants() {
        assert_eq!(job_type_kind(&JobType::Backup), "backup");
        assert_eq!(job_type_kind(&JobType::Restore), "restore");
        assert_eq!(job_type_kind(&JobType::Verify), "verify");
        assert_eq!(job_type_kind(&JobType::Upload), "upload");
        assert_eq!(job_type_kind(&JobType::Download), "download");
        assert_eq!(job_type_kind(&JobType::Sync), "sync");
        assert_eq!(job_type_kind(&JobType::CacheMover), "cache_mover");
    }

    #[test]
    fn test_kind_custom_slug_passthrough() {
        assert_eq!(
            job_type_kind(&JobType::Custom("docker_install".into())),
            "docker_install"
        );
    }

    #[test]
    fn test_from_aria2_uses_bittorrent_name() {
        let mut t = task("active");
        t.bittorrent = Some(Aria2BtInfo {
            name: "ubuntu.iso".into(),
        });
        t.files = vec![file("/tmp/xyz.part", "")];
        assert_eq!(activity_task_from_aria2(t).unwrap().label, "ubuntu.iso");
    }

    #[test]
    fn test_from_aria2_falls_back_to_file_basename() {
        let mut t = task("active");
        t.files = vec![file("/downloads/movie.mkv", "http://example.com/movie.mkv")];
        assert_eq!(activity_task_from_aria2(t).unwrap().label, "movie.mkv");
    }

    #[test]
    fn test_from_aria2_falls_back_to_uri() {
        let mut t = task("active");
        t.files = vec![file("", "http://example.com/thing.bin")];
        assert_eq!(
            activity_task_from_aria2(t).unwrap().label,
            "http://example.com/thing.bin"
        );
    }

    #[test]
    fn test_from_aria2_progress_none_when_total_zero() {
        let t = task("active"); // total_length "0"
        assert_eq!(activity_task_from_aria2(t).unwrap().progress, None);
    }

    #[test]
    fn test_from_aria2_progress_ratio() {
        let mut t = task("active");
        t.total_length = "1000".into();
        t.completed_length = "250".into();
        assert_eq!(activity_task_from_aria2(t).unwrap().progress, Some(25.0));
    }

    #[test]
    fn test_from_aria2_skips_non_active_status() {
        assert!(activity_task_from_aria2(task("waiting")).is_none());
        assert!(activity_task_from_aria2(task("complete")).is_none());
        assert!(activity_task_from_aria2(task("paused")).is_none());
    }

    #[test]
    fn test_from_aria2_torrent_vs_url_kind() {
        let mut bt = task("active");
        bt.bittorrent = Some(Aria2BtInfo { name: "x".into() });
        assert_eq!(
            activity_task_from_aria2(bt).unwrap().kind,
            "download_torrent"
        );
        let mut url = task("active");
        url.files = vec![file("/tmp/a.bin", "http://h/a.bin")];
        assert_eq!(activity_task_from_aria2(url).unwrap().kind, "download_url");
    }

    #[test]
    fn test_progress_clamped_0_100() {
        let mut s = snapshot(JobType::Backup);
        s.progress = Some(150.0);
        assert_eq!(activity_task_from_snapshot(s).progress, Some(100.0));
        let mut s2 = snapshot(JobType::Backup);
        s2.progress = Some(-5.0);
        assert_eq!(activity_task_from_snapshot(s2).progress, Some(0.0));
    }

    #[test]
    fn test_from_snapshot_detail_from_message() {
        let mut s = snapshot(JobType::Custom("explorer_copy".into()));
        s.message = "12/40 files".into();
        let out = activity_task_from_snapshot(s);
        assert_eq!(out.kind, "explorer_copy");
        assert_eq!(out.label, "");
        assert_eq!(out.detail.as_deref(), Some("12/40 files"));
    }

    #[test]
    fn test_from_snapshot_detail_appends_speed() {
        let mut s = snapshot(JobType::Backup);
        s.message = "Running".into();
        s.speed_bps = Some(2 * 1024 * 1024);
        assert_eq!(
            activity_task_from_snapshot(s).detail.as_deref(),
            Some("Running · 2.0 MB/s")
        );
    }

    #[test]
    fn test_activity_task_serializes_expected_keys() {
        let json = serde_json::to_value(ActivityTask {
            id: "aria2:g1".into(),
            kind: "download_url".into(),
            state: "running".into(),
            label: "f.bin".into(),
            progress: Some(40.0),
            detail: None,
        })
        .unwrap();
        let obj = json.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["detail", "id", "kind", "label", "progress", "state"]);
    }

    // -----------------------------------------------------------------
    // ROADMAP 28.2 — a job that just failed must stay visible
    // -----------------------------------------------------------------

    /// The assertion 28.2 needed. The feed used to keep only
    /// `Pending | Running`, so a job that failed one second ago disappeared on
    /// the very next poll and the only trace left was a notification the user
    /// was not looking at.
    #[test]
    fn a_fresh_failure_is_visible_within_the_grace_window() {
        let now = chrono::Utc::now();
        let mut s = snapshot(JobType::Restore);
        s.state = ProgressState::Failed;
        s.updated_at = Some(now - chrono::Duration::seconds(1));
        assert!(is_visible(&s, now), "a job that just failed must be shown");
    }

    #[test]
    fn an_old_failure_drops_out_of_the_feed() {
        let now = chrono::Utc::now();
        let mut s = snapshot(JobType::Restore);
        s.state = ProgressState::Failed;
        s.updated_at = Some(now - FAILED_GRACE - chrono::Duration::seconds(1));
        assert!(
            !is_visible(&s, now),
            "the taskbar must not become a graveyard"
        );
    }

    #[test]
    fn a_cancelled_job_follows_the_same_grace_window() {
        let now = chrono::Utc::now();
        let mut s = snapshot(JobType::Backup);
        s.state = ProgressState::Cancelled;
        s.updated_at = Some(now);
        assert!(is_visible(&s, now));
        s.updated_at = Some(now - FAILED_GRACE - chrono::Duration::seconds(1));
        assert!(!is_visible(&s, now));
    }

    #[test]
    fn live_work_is_always_visible_and_completed_work_never_is() {
        let now = chrono::Utc::now();
        let mut s = snapshot(JobType::Backup);
        // Even an ancient, never-updated pending job stays: it has not failed.
        s.state = ProgressState::Pending;
        s.updated_at = None;
        assert!(is_visible(&s, now));
        s.state = ProgressState::Running;
        assert!(is_visible(&s, now));
        s.state = ProgressState::Completed;
        s.updated_at = Some(now);
        assert!(
            !is_visible(&s, now),
            "success is reported by the screen that started the job"
        );
    }

    /// A terminal state with no timestamp is not shown: there is no evidence it
    /// happened *recently*, and inventing one would keep it forever.
    #[test]
    fn a_terminal_state_without_a_timestamp_is_not_shown() {
        let now = chrono::Utc::now();
        let mut s = snapshot(JobType::Backup);
        s.state = ProgressState::Failed;
        s.updated_at = None;
        assert!(!is_visible(&s, now));
    }

    #[test]
    fn state_slug_covers_every_variant() {
        assert_eq!(state_slug(&ProgressState::Pending), "pending");
        assert_eq!(state_slug(&ProgressState::Running), "running");
        assert_eq!(state_slug(&ProgressState::Completed), "completed");
        assert_eq!(state_slug(&ProgressState::Failed), "failed");
        assert_eq!(state_slug(&ProgressState::Cancelled), "cancelled");
    }

    /// The row carries the state, so the client can render a failure instead of
    /// a stalled progress bar.
    #[test]
    fn a_failed_snapshot_maps_to_a_failed_row() {
        let mut s = snapshot(JobType::Backup);
        s.state = ProgressState::Failed;
        s.message = "Failed: disk full".into();
        let task = activity_task_from_snapshot(s);
        assert_eq!(task.state, "failed");
        assert_eq!(task.detail.as_deref(), Some("Failed: disk full"));
    }

    #[test]
    fn an_aria2_row_is_always_running() {
        let t = task("active");
        let row = activity_task_from_aria2(t).expect("an active download is a row");
        assert_eq!(row.state, "running");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
    }
}
