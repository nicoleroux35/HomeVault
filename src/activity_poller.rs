/* =============================================================================
 * File          : activity_poller.rs
 * Author        : Nicolas Le Roux
 * Created       : 24/10/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Phase 2 of the unified activity feed (ROADMAP 28.3): a background poller
 *   that turns *live system state* into synthetic activity jobs, so the
 *   desktop taskbar also reports work HomeVault did not start itself — a
 *   Syncthing folder still catching up, an mdadm array resyncing, a zpool or
 *   btrfs scrub running, a RAID rebuild in flight. Every source is read
 *   read-only, bounded by a deadline, and mapped onto a
 *   `PROGRESS_MANAGER` job whose id is *derived from the source* so that a
 *   given resync is one row that advances, not a new row per poll.
 *
 * Dependencies:
 *   - chrono (>= 0.4)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   `web::run()` spawns `run_poller_loop(state)` at startup; every
 *   `POLL_INTERVAL` it reads the enabled sources and syncs `PROGRESS_MANAGER`.
 *   Pure parsers (`parse_mdstat`, `parse_zpool_scan`, `parse_btrfs_scrub_status`,
 *   `folder_sync_progress`) are unit-tested without hardware.
 * =============================================================================
 */

//! Live-hardware poller behind the activity feed (ROADMAP 28.3, phase 2).
//!
//! The feed used to report only what HomeVault itself started: a job is a
//! `PROGRESS_MANAGER` entry some handler created. Everything the *host* does on
//! its own — a Syncthing folder finishing a large scan, an mdadm array in the
//! middle of a `resync`, a `zpool scrub`, a btrfs scrub — was invisible, even
//! though it is exactly the kind of long, slow, I-wonder-if-it-is-stuck work
//! the taskbar exists for.
//!
//! This module closes that gap with a poller: every [`POLL_INTERVAL`] it reads
//! the live sources and **synthesises** a progress job per active operation.
//!
//! # Why the id is derived from the source
//!
//! A job id that changed on every poll would fill the taskbar with a new row
//! per minute; the panel would never show a bar that *advances*. So the id is a
//! deterministic function of the source (`raid:md0:resync`,
//! `pool:tank:scrub`, `syncthing:<folder-id>`), and the poller re-emits progress
//! on the same job for as long as the operation lasts. When the source says it
//! is done, the job is completed and the feed drops it (a `completed` job is
//! never listed — see `handlers::activity::is_visible`).
//!
//! # Failure policy
//!
//! A source that cannot be read is a **gap, never a fabricated value**:
//! `/proc/mdstat` missing means "no md arrays on this host", not "healthy"; a
//! `zpool status` that times out contributes nothing at all. The poller never
//! fails the loop, and never turns a probe failure into a red row — it is not
//! new information about the array, it is a missing measurement.
//!
//! # Why a poller and not `/api/activity/stream`
//!
//! The poller is server-side on purpose: the SAMPLING is what costs (a `sudo
//! zpool status` per pool, a Syncthing HTTP call per folder), and doing it once
//! per appliance instead of once per open desktop is the whole point. The
//! desktop keeps polling `GET /api/activity`, which now simply also contains
//! these rows.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use crate::handlers::AppStateArc;
use crate::sse::{JobType, PROGRESS_MANAGER};

// ---------------------------------------------------------------------------
// Tuning
// ---------------------------------------------------------------------------

/// How often every source is read.
///
/// Five seconds is the cadence the client already uses for the panel, and it
/// keeps a long operation's bar moving without turning a scrub into a
/// shell-command storm: on an idle host this is one `read_to_string` of
/// `/proc/mdstat` every tick and nothing else (the shelling sources are only
/// probed when the cheap source says there is something to look at).
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Deadline for one `zpool status` read. A pool probe on a busy box can take a
/// second; a wedged `zpool` must not stall the whole poll.
pub const ZPOOL_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for one `btrfs scrub status` read.
pub const BTRFS_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Job-id namespaces
// ---------------------------------------------------------------------------

/// Prefix of every synthetic job id this module creates.
///
/// The poller must never *complete* a job it did not create (a real backup
/// named `md0` is not a resync), so the sweep only touches ids carrying this
/// prefix.
pub const SYNTHETIC_PREFIX: &str = "sys:";

/// The `kind` slug of an mdadm array operation.
pub const KIND_RAID: &str = "raid_resync";

/// The `kind` slug of a ZFS/btrfs scrub.
pub const KIND_SCRUB: &str = "pool_scrub";

/// The `kind` slug of a Syncthing folder that is still catching up.
pub const KIND_SYNCTHING: &str = "syncthing_sync";

/// The job id of one mdadm operation: `sys:raid:<array>:<op>`.
pub fn raid_job_id(array: &str, op: &str) -> String {
    format!("{SYNTHETIC_PREFIX}raid:{array}:{op}")
}

/// The job id of one pool scrub: `sys:pool:<pool>:scrub`.
pub fn scrub_job_id(pool: &str) -> String {
    format!("{SYNTHETIC_PREFIX}pool:{pool}:scrub")
}

/// The job id of one Syncthing folder: `sys:syncthing:<folder-id>`.
pub fn syncthing_job_id(folder_id: &str) -> String {
    format!("{SYNTHETIC_PREFIX}syncthing:{folder_id}")
}

// ---------------------------------------------------------------------------
// A source reading
// ---------------------------------------------------------------------------

/// One live operation, as the poller sees it, before it becomes a job.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveOperation {
    /// Deterministic id (see [`raid_job_id`] / [`scrub_job_id`] /
    /// [`syncthing_job_id`]).
    pub id: String,
    /// `kind` slug the client localises (`raid_resync`, `pool_scrub`,
    /// `syncthing_sync`).
    pub kind: &'static str,
    /// `0..=100`, or `None` while the source cannot say how far along it is.
    pub progress: Option<f64>,
    /// One human sentence, already free of anything secret.
    pub message: String,
}

impl LiveOperation {
    /// The `JobType` the progress manager stores — every synthetic row is a
    /// `Custom` slug so the feed's `kind` mapping passes it through unchanged.
    pub fn job_type(&self) -> JobType {
        JobType::Custom(self.kind.to_string())
    }
}

// ---------------------------------------------------------------------------
// mdstat — the mdadm resync / rebuild parser
// ---------------------------------------------------------------------------

/// One mdadm array as `/proc/mdstat` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct MdArray {
    /// `md0`, `md127`, …
    pub name: String,
    /// `raid1`, `raid5`, … (the field after `active`).
    pub level: String,
    /// Operations currently running on the array.
    pub operations: Vec<MdAction>,
}

/// One operation running on an mdadm array, with what its status line says
/// about how far along it is.
#[derive(Debug, Clone, PartialEq)]
pub struct MdAction {
    /// Which operation it is.
    pub op: MdOperation,
    /// The percentage the kernel reported, or `None` while it reports
    /// `DELAYED` / `PENDING` / nothing yet.
    pub percent: Option<f64>,
    /// The kernel has not started the work (`resync = DELAYED`). A row for it
    /// would sit at 0 % forever, so it is not reported.
    pub delayed: bool,
}

/// One operation running on an mdadm array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdOperation {
    /// `[==>.......]  resync = 25.4% (…) finish=12.3min speed=…`
    Resync,
    /// `[=>........]  recovery = 10.0%`
    Recovery,
    /// `[==>.......]  reshape = 42.0%`
    Reshape,
    /// `[=>........]  check = 60.0%`
    Check,
}

impl MdOperation {
    /// The slug used in the job id (`resync`, `recovery`, `reshape`, `check`).
    pub fn slug(self) -> &'static str {
        match self {
            MdOperation::Resync => "resync",
            MdOperation::Recovery => "recovery",
            MdOperation::Reshape => "reshape",
            MdOperation::Check => "check",
        }
    }

    /// The word shown in the row, plain language for a home user.
    pub fn label(self) -> &'static str {
        match self {
            MdOperation::Resync => "RAID resync",
            MdOperation::Recovery => "RAID recovery",
            MdOperation::Reshape => "RAID reshape",
            MdOperation::Check => "RAID consistency check",
        }
    }

    /// Recognise the operation keyword that starts the status line.
    fn from_keyword(word: &str) -> Option<Self> {
        match word {
            "resync" => Some(MdOperation::Resync),
            "recovery" => Some(MdOperation::Recovery),
            "reshape" => Some(MdOperation::Reshape),
            "check" => Some(MdOperation::Check),
            _ => None,
        }
    }
}

/// Parse `/proc/mdstat` into one entry per **active** array, with the
/// operations that are currently running on it.
///
/// `/proc/mdstat` looks like this while an array is resyncing:
///
/// ```text
/// Personalities : [raid1] [raid6] [raid5] [raid4]
/// md0 : active raid1 sdb1[1] sda1[0]
///       976630464 blocks super 1.2 [2/2] [UU]
///       [==>..................]  resync = 12.6% (12345678/976630464) finish=118.4min speed=135000K/sec
///
/// md1 : active raid5 sdd1[3] sdc1[2] sdb1[1] sda1[0]
///       2929893888 blocks super 1.2 level 5, 512k chunk, algorithm 2 [4/4] [UUUU]
///
/// unused devices: <none>
/// ```
///
/// **The status line belongs to the array above it**, not to the line that
/// holds the name: the header line (`md0 : active raid1 …`) never carries a
/// percentage, and a parser that only looked at header lines would report every
/// resync as idle — which is exactly the bug that makes a "live" taskbar show
/// nothing while the disks grind. The array's devices and blocks lines sit
/// between the two, so the operation is attached to the most recent header.
///
/// An array with no operation is returned with an empty `operations` list (it
/// is running and healthy, which is not an activity row). An array whose
/// operation has **no** percentage yet (`[>....................]  resync =
/// DELAYED` or `PENDING`) is reported with `progress = None` — indeterminate,
/// which is the truth.
pub fn parse_mdstat(raw: &str) -> Vec<MdArray> {
    let mut arrays: Vec<MdArray> = Vec::new();
    // A header is `mdN : <state> [level] dev…`. Only `active` arrays are
    // running; `inactive` ones have nothing in flight.
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if let Some((name, rest)) = trimmed.split_once(" : ") {
            let name = name.trim();
            if !name.starts_with("md") || name.len() < 3 {
                continue;
            }
            let mut fields = rest.split_whitespace();
            let state = fields.next().unwrap_or_default();
            // `active` is what a working array says; `inactive` has nothing to
            // report and `readonly` is a fault the Journal/Health screens own.
            if state != "active" {
                continue;
            }
            let level = fields.next().unwrap_or_default().to_string();
            arrays.push(MdArray {
                name: name.to_string(),
                level,
                operations: Vec::new(),
            });
            continue;
        }

        // A status line starts with the bracket progress bar — that leading
        // `[` is the marker that distinguishes it from the blocks line.
        let Some(array) = arrays.last_mut() else {
            continue;
        };
        if !trimmed.trim_start().starts_with('[') {
            continue;
        }
        if let Some(action) = parse_md_status_line_full(trimmed) {
            array.operations.push(action);
        }
    }
    arrays
}

/// Parse one mdstat status line into the operation **and** its percentage.
///
/// `[==>……]  resync = 12.6% (123/976) finish=118.4min speed=…` →
/// `{ op: Resync, percent: Some(12.6), delayed: false }`.
/// `[>……]  resync = DELAYED` → `{ op: Resync, percent: None, delayed: true }`.
pub fn parse_md_status_line_full(line: &str) -> Option<MdAction> {
    let op = parse_md_status_line(line)?;
    let upper = line.to_uppercase();
    Some(MdAction {
        op,
        percent: parse_md_percent(line),
        delayed: upper.contains("DELAYED") || upper.contains("PENDING"),
    })
}

/// The percentage carried by one mdstat status line, if it carries one.
///
/// Returns the operation keyword and its percentage:
/// `[==>……]  resync = 12.6% (123/976) finish=118.4min speed=…` → `12.6`.
/// `[>……]  resync = DELAYED` → `None` (it has not started counting).
pub fn parse_md_status_line(line: &str) -> Option<MdOperation> {
    // The keyword is the first bare word after the bracket group.
    let after_bracket = line.rsplit_once(']')?.1.trim_start();
    let keyword = after_bracket.split_whitespace().next()?;
    // A line can carry two operations separated by ` ; ` (`resync = 1.0% ;
    // recovery = 2.0%`) — the first one is the one to report.
    MdOperation::from_keyword(keyword)
}

/// The percentage of one mdstat status line, or `None` when it is
/// `DELAYED` / `PENDING` / absent.
///
/// Kept separate from [`parse_md_status_line`] because the two answers are used
/// differently: the operation slug names the row, the percentage drives its
/// bar, and a row with no bar (indeterminate) is a legitimate state
/// (`ProgressState` semantics in `sse::progress`).
pub fn parse_md_percent(line: &str) -> Option<f64> {
    // The percentage follows the first `=` and ends at `%`.
    let after_eq = line.split_once('=')?.1.trim_start();
    let head = after_eq.split('%').next()?;
    // Anything non-numeric (`DELAYED`, `PENDING`, `pending`) is indeterminate.
    head.trim().parse::<f64>().ok().map(|p| p.clamp(0.0, 100.0))
}

/// Map every active operation of every array onto one [`LiveOperation`].
///
/// An operation the kernel says is `DELAYED` / `PENDING` is skipped
/// ([`is_reportable`]): a row stuck at 0 % for as long as the array waits is
/// worse than no row.
pub fn operations_from_mdstat(raw: &str) -> Vec<LiveOperation> {
    let mut ops = Vec::new();
    for array in parse_mdstat(raw) {
        for action in array.operations {
            if !is_reportable(&action) {
                continue;
            }
            ops.push(LiveOperation {
                id: raid_job_id(&array.name, action.op.slug()),
                kind: KIND_RAID,
                progress: action.percent,
                message: format!("{} on {}", action.op.label(), array.name),
            });
        }
    }
    ops
}

// ---------------------------------------------------------------------------
// zpool / btrfs scrub
// ---------------------------------------------------------------------------

/// One scrub as `zpool status` or `btrfs scrub status` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrubState {
    /// Pool name.
    pub pool: String,
    /// `0..=100` when the tool reports it, `None` when it does not
    /// (`zpool` prints a percentage only once the scan has finished, and
    /// btrfs prints the percentage of the work done, which both map here).
    pub progress: Option<f64>,
    /// The scan line, verbatim minus its prefix — free of secrets.
    pub message: String,
}

/// Parse one `zpool status <pool>` output.
///
/// The interesting line is the `scan:` one:
///
/// ```text
///   scan: scrub in progress since Sun Oct 24 09:12:01 2026
///         1.24T scanned at 823M/s, 412G issued at 273M/s, 1.40T total
///         0B repaired, 29.37% done, 01:04:22 to go
/// ```
///
/// A `scrub repaired 0B in 02:41:12 with 0 errors on …` line means the scrub is
/// **finished** and must not produce a row: an activity row that never ends is
/// worse than no row. `resilver` is mapped here too — a resilvering pool is
/// exactly the "array is catching up" case the taskbar is for, and the row says
/// which one it is.
pub fn parse_zpool_scan(pool: &str, raw: &str) -> Option<ScrubState> {
    // Join the continuation lines of the `scan:` block into one sentence: the
    // percentage and the "to go" countdown live on the *third* line, so a
    // parser that only read the first one would report an indeterminate bar
    // forever.
    let mut scan = String::new();
    let mut in_scan = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("scan:") {
            in_scan = true;
            scan.push_str(rest.trim());
            scan.push(' ');
            continue;
        }
        if in_scan {
            // A line that starts a new top-level field ends the block. The
            // fields `zpool status` prints are `scan:`, `config:`, `errors:`,
            // `status:`, `action:`; a blank line ends it too.
            if trimmed.is_empty()
                || trimmed.starts_with("config:")
                || trimmed.starts_with("errors:")
                || trimmed.starts_with("status:")
                || trimmed.starts_with("action:")
            {
                break;
            }
            scan.push_str(trimmed);
            scan.push(' ');
        }
    }
    let scan = scan.trim();
    if scan.is_empty() {
        return None;
    }

    // `scrub in progress` / `resilver in progress` are the two live cases.
    // Everything else (`scrub repaired …`, `resilvered …`, `none requested`) is
    // a finished or never-run scan.
    let live = scan.contains("scrub in progress") || scan.contains("resilver in progress");
    if !live {
        return None;
    }

    // `<pct>% done` is on the last line of the block. `zpool` omits it while
    // the scan is too young to estimate — then the bar is indeterminate.
    let progress = scan
        .split_whitespace()
        .zip(scan.split_whitespace().skip(1))
        .find_map(|(value, unit)| {
            (unit == "done").then(|| value.trim_end_matches('%').parse::<f64>().ok())?
        })
        .map(|p| p.clamp(0.0, 100.0));

    Some(ScrubState {
        pool: pool.to_string(),
        progress,
        message: collapse_whitespace(scan),
    })
}

/// Parse one `btrfs scrub status <mount>` output.
///
/// Two shapes are in the wild:
///
/// ```text
/// UUID:             …
/// Scrub started:    Sun Oct 24 09:12:01 2026
/// Status:           running
/// Duration:         0:12:34
/// Total to scrub:   412.00GiB
/// Rate:             250.00MiB/s
/// ...
/// ```
///
/// Modern `btrfs-progs` also prints a `Bytes scrubbed: 12.00GiB (10.00%)` line
/// while the same scrub runs. Both are read: `Status: running` is what makes it
/// live, and the percentage comes from the `Bytes scrubbed` line when it is
/// there (`None` otherwise — indeterminate).
pub fn parse_btrfs_scrub_status(pool: &str, raw: &str) -> Option<ScrubState> {
    let status = raw
        .lines()
        .find_map(|l| l.trim().strip_prefix("Status:").map(str::trim))?;
    if status != "running" {
        // `finished`, `aborted`, `interrupted` — none of them is work in
        // progress any more.
        return None;
    }

    let progress = raw.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("Bytes scrubbed:")?;
        // `12.00GiB (10.00%)` — the percentage is the parenthesised part.
        let start = rest.find('(')?;
        let end = rest.rfind('%')?;
        rest[start + 1..end].trim().parse::<f64>().ok()
    });

    let detail = raw
        .lines()
        .find_map(|l| l.trim().strip_prefix("Total to scrub:").map(str::trim))
        .unwrap_or("");

    Some(ScrubState {
        pool: pool.to_string(),
        progress: progress.map(|p| p.clamp(0.0, 100.0)),
        message: if detail.is_empty() {
            "btrfs scrub running".to_string()
        } else {
            format!("btrfs scrub running — {} to scrub", detail)
        },
    })
}

/// Map one live scrub onto a [`LiveOperation`].
pub fn operation_from_scrub(state: &ScrubState, kind: &'static str) -> LiveOperation {
    LiveOperation {
        id: scrub_job_id(&state.pool),
        kind,
        progress: state.progress,
        message: state.message.clone(),
    }
}

// ---------------------------------------------------------------------------
// Syncthing
// ---------------------------------------------------------------------------

/// Percentage of a Syncthing folder that is already local.
///
/// Syncthing reports the *need* (`needBytes` / `needFiles`) and the *total*
/// (`globalBytes` / `globalFiles`). A folder is "done" when the need is zero —
/// and that is the only case the feed must hide, because the work is over.
///
/// The percentage is computed on **bytes**, not on files: a folder catching up
/// on one 40 GB file is one file of the need, and a file-count ratio would read
/// 99 % done while 40 GB are still to transfer. Bytes are the honest measure of
/// what is left to do.
///
/// Returns `None` when the folder has nothing left (equality or an empty
/// folder), `Some(pct)` in `0..100` otherwise. An empty-but-not-yet-scanned
/// folder (`globalBytes == 0` with `needBytes == 0`) is `None` — there is no
/// work to show.
pub fn folder_sync_progress(global_bytes: u64, need_bytes: u64) -> Option<f64> {
    if need_bytes == 0 || global_bytes == 0 {
        return None;
    }
    let done = global_bytes.saturating_sub(need_bytes) as f64 / global_bytes as f64 * 100.0;
    Some(done.clamp(0.0, 100.0))
}

/// One Syncthing folder as the poller needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncthingFolderNeed {
    pub id: String,
    pub label: String,
    pub global_bytes: u64,
    pub need_bytes: u64,
    pub paused: bool,
}

/// Map every folder still catching up onto a [`LiveOperation`].
///
/// A **paused** folder is not work in progress — Syncthing is deliberately not
/// transferring it, and showing it as an activity row would be a lie that never
/// ends. It is skipped, exactly like a finished one.
pub fn operations_from_syncthing(folders: &[SyncthingFolderNeed]) -> Vec<LiveOperation> {
    folders
        .iter()
        .filter(|f| !f.paused)
        .filter_map(|f| {
            let progress = folder_sync_progress(f.global_bytes, f.need_bytes)?;
            let left = format_bytes(f.need_bytes);
            let name = if f.label.is_empty() {
                f.id.clone()
            } else {
                f.label.clone()
            };
            Some(LiveOperation {
                id: syncthing_job_id(&f.id),
                kind: KIND_SYNCTHING,
                progress: Some(progress),
                message: format!("{name} — {left} to transfer"),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Registry reconciliation
// ---------------------------------------------------------------------------

/// The operations seen at this tick, and how the manager must react.
#[derive(Debug, Default, PartialEq)]
pub struct ReconcilePlan {
    /// Ids that must be created or advanced (the live ones).
    pub live: Vec<LiveOperation>,
    /// Synthetic ids the previous tick knew about and that are gone now.
    pub finished: Vec<String>,
}

/// Compute what to do with `known` (the synthetic ids of the previous tick)
/// given the operations observed now.
///
/// Pure, so the "a scrub that ended must leave the feed" rule is testable
/// without a scrub. `finished` keeps only ids that are **synthetic** and were
/// `known`: the poller must never complete a job another handler created.
pub fn plan_reconcile(known: &HashSet<String>, live: Vec<LiveOperation>) -> ReconcilePlan {
    let live_ids: HashSet<String> = live.iter().map(|o| o.id.clone()).collect();
    let finished: Vec<String> = known
        .iter()
        .filter(|id| id.starts_with(SYNTHETIC_PREFIX))
        .filter(|id| !live_ids.contains(*id))
        .cloned()
        .collect();
    ReconcilePlan { live, finished }
}

/// The ids of every synthetic job currently tracked by the manager.
///
/// Used at startup so a restart does not orphan the rows the previous process
/// left behind: the first tick sees them as `known` and completes whichever
/// operation is no longer running.
pub fn synthetic_ids_in_manager() -> HashSet<String> {
    PROGRESS_MANAGER
        .list_active_jobs()
        .into_iter()
        .map(|(id, _, _)| id)
        .filter(|id| id.starts_with(SYNTHETIC_PREFIX))
        .collect()
}

/// Apply one plan to the global progress manager.
///
/// This is the only function in the module with a side effect, and it keeps it
/// to three primitives: create-or-advance (`emit_progress`, which is a no-op
/// returning `false` for an unknown id), complete, remove. An operation that is
/// live but no longer tracked (the manager's cleanup ran) is re-created, so a
/// long scrub cannot vanish from the feed halfway through.
pub fn apply_plan(plan: &ReconcilePlan) {
    for op in &plan.live {
        // Advance on the same id, keeping the row honest about what the source
        // actually reported: a percentage when there is one, and an
        // indeterminate bar (`progress: null`) when there is not — never a
        // fabricated 0 % that then never moves.
        let advanced = match op.progress {
            Some(p) => PROGRESS_MANAGER.emit_progress(&op.id, p, &op.message),
            None => PROGRESS_MANAGER.emit_message_only(&op.id, &op.message),
        };
        if !advanced {
            // Not tracked yet (first tick, or pruned): create it, then set the
            // real percentage. `create_job` seeds at 0 %/indeterminate, and a
            // scrub discovered at 60 % must show 60 %.
            let created = PROGRESS_MANAGER.create_job(op.job_type(), &op.message);
            debug_assert_eq!(created, op.id);
            if let Some(p) = op.progress {
                PROGRESS_MANAGER.emit_progress(&op.id, p, &op.message);
            }
        }
    }
    for id in &plan.finished {
        PROGRESS_MANAGER.remove_job(id);
    }
}

/// Whether the array's operation should be reported at all.
///
/// A `check` (consistency check) on a healthy array runs for hours at a crawl
/// and is exactly the long-running work the taskbar is for, so it is reported.
/// A `resync` the kernel says is `DELAYED` / `PENDING` has not started, and a
/// row for it would sit at 0 % forever — that is the case filtered here.
pub fn is_reportable(action: &MdAction) -> bool {
    !action.delayed
}

// ---------------------------------------------------------------------------
// I/O adapters
// ---------------------------------------------------------------------------

/// Read `/proc/mdstat` (read-only, no privilege) and map it onto operations.
///
/// A missing file means "this host runs no md array" — an empty list, not an
/// error. The read goes through `tokio::fs`, so it does not block a worker.
pub async fn probe_mdadm(path: &Path) -> Vec<LiveOperation> {
    let Ok(raw) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    operations_from_mdstat(&raw)
}

/// Read every pool scrub through `zpool status` / `btrfs scrub status`.
///
/// The pool list comes from the caller (the Storage screen already knows it),
/// so this function stays a pure "given these pools, which are scrubbing".
/// Every command is bounded ([`ZPOOL_PROBE_TIMEOUT`] / [`BTRFS_PROBE_TIMEOUT`])
/// and a failing probe contributes nothing.
pub async fn probe_pool_scrubs(pools: &[(String, ScrubKind)]) -> Vec<LiveOperation> {
    let mut ops = Vec::new();
    for (name, kind) in pools {
        let state = match kind {
            ScrubKind::Zfs => {
                let mut cmd = tokio::process::Command::new("sudo");
                cmd.args(["zpool", "status", name]);
                crate::utils::process::run_timed_async(&mut cmd, ZPOOL_PROBE_TIMEOUT)
                    .await
                    .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                    .and_then(|raw| parse_zpool_scan(name, &raw))
            }
            ScrubKind::Btrfs(mount) => {
                let mut cmd = tokio::process::Command::new("sudo");
                cmd.args(["btrfs", "scrub", "status", mount]);
                crate::utils::process::run_timed_async(&mut cmd, BTRFS_PROBE_TIMEOUT)
                    .await
                    .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                    .and_then(|raw| parse_btrfs_scrub_status(name, &raw))
            }
        };
        if let Some(state) = state {
            ops.push(operation_from_scrub(&state, KIND_SCRUB));
        }
    }
    ops
}

/// Which tool reports a given pool's scrub.
#[derive(Debug, Clone, PartialEq)]
pub enum ScrubKind {
    /// `zpool status <name>`.
    Zfs,
    /// `btrfs scrub status <mount-point>`.
    Btrfs(String),
}

/// Read every Syncthing folder's backlog through the daemon's own API.
///
/// Delegates to `handlers::syncthing` so the poller does not build a second
/// HTTP client, a second API-key reader, or a second notion of the daemon's
/// address. A daemon that is not running answers nothing and the folder list
/// comes back empty — the poller must not log a warning every five seconds on
/// a box that simply does not use Syncthing.
pub async fn probe_syncthing() -> Vec<LiveOperation> {
    let folders = crate::handlers::syncthing::folder_backlogs().await;
    operations_from_syncthing(&folders)
}

/// Whether this host has a Syncthing to ask about at all.
///
/// The cheap gate before an HTTP probe every five seconds: Syncthing is only
/// reachable when its `config.xml` exists, which is also where its API key
/// lives. A box that never installed it does no work here — and, unlike a
/// settings switch, this cannot go stale (the file appears when the package is
/// installed, not when a screen is saved).
///
/// The path probe is blocking file I/O, so it runs on the blocking pool
/// (ROADMAP X.1's rule: no `std::fs` on an async body).
pub async fn syncthing_available() -> bool {
    tokio::task::spawn_blocking(crate::handlers::syncthing::syncthing_config_present)
        .await
        .unwrap_or(false)
}

/// One full pass: read every source, then reconcile the manager.
///
/// Returns the plan that was applied, so a caller (or a test) can inspect it.
/// Every source is probed independently, so one wedged tool cannot hide the
/// other rows.
pub async fn tick(
    known: &HashSet<String>,
    mdstat_path: &Path,
    pools: &[(String, ScrubKind)],
    syncthing_enabled: bool,
) -> ReconcilePlan {
    let mut live = probe_mdadm(mdstat_path).await;
    if !pools.is_empty() {
        live.extend(probe_pool_scrubs(pools).await);
    }
    if syncthing_enabled {
        live.extend(probe_syncthing().await);
    }
    let plan = plan_reconcile(known, live);
    apply_plan(&plan);
    plan
}

/// The background loop, spawned once at startup.
///
/// `known` starts as whatever synthetic job the previous process left in the
/// manager (a restart must not orphan a scrub row), then follows each tick.
/// The loop is deliberately unkillable: a tick that fails is logged and the
/// next one runs — an activity feed is a convenience, and it must never be the
/// reason the server stops answering.
///
/// The `AppStateArc` is taken to match every other `run_*_loop` of the boot
/// path (one shape at the spawn site), and because the sources it reads are all
/// process-global today (`/proc`, `sudo zpool`, the daemon's own API). A source
/// that ever needs the database gets it here without changing the caller.
pub async fn run_poller_loop(_state: AppStateArc) {
    let mut known = synthetic_ids_in_manager();
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    // The first tick of a `tokio::interval` fires immediately; that is what we
    // want — the feed should be right as soon as the server is up, not five
    // seconds later.
    loop {
        interval.tick().await;
        // The pools and the Syncthing daemon presence are re-read each tick: an
        // admin who starts a scrub or installs Syncthing expects the row to
        // appear without a restart.
        let (pools, syncthing_enabled) = poller_inputs().await;
        let plan = tick(
            &known,
            Path::new("/proc/mdstat"),
            &pools,
            syncthing_enabled,
        )
        .await;
        known = plan.live.iter().map(|o| o.id.clone()).collect();
        if !plan.finished.is_empty() {
            tracing::debug!(
                "Activity poller: {} operation(s) ended",
                plan.finished.len()
            );
        }
    }
}

/// What the poller should look at, read once per tick.
///
/// Split out of the loop so the settings read is one place: `pools` comes from
/// the same `PoolService::scan_pools` the Storage screen uses, and
/// `syncthing_enabled` answers whether there is a Syncthing daemon to ask
/// (see [`syncthing_available`]), so a host that never installed one does not
/// answer an HTTP probe every five seconds.
async fn poller_inputs() -> (Vec<(String, ScrubKind)>, bool) {
    let pools = tokio::task::spawn_blocking(|| {
        crate::pools::PoolService::scan_pools()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| match p.pool_type {
                crate::pools::PoolType::Zfs => Some((p.name, ScrubKind::Zfs)),
                crate::pools::PoolType::Btrfs => p
                    .mount_point
                    .clone()
                    .map(|m| (p.name, ScrubKind::Btrfs(m))),
                // mdadm arrays are covered by `/proc/mdstat`; an unknown type
                // has no scrub to report.
                _ => None,
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    let syncthing_enabled = syncthing_available().await;
    (pools, syncthing_enabled)
}

/// Collapse runs of whitespace so a multi-line tool output reads as a sentence.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Compact human byte size (`1.4 GB`, `912 MB`, `340 B`).
///
/// Same ladder as `handlers::activity::format_bytes`, duplicated rather than
/// exported because that module's copy is private to the handler and the two
/// are deliberately allowed to diverge (the feed's is for speeds, this one for
/// backlogs, and pulling one helper across module boundaries for two call sites
/// would make the handler's signature `pub(crate)` for no benefit).
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
    //! Pure parsing and reconciliation tests — no hardware, no DB, no HTTP.
    //! Run: `cargo test activity_poller::tests`
    use super::*;

    const MDSTAT_IDLE: &str = "\
Personalities : [raid1] [raid6] [raid5] [raid4]
md0 : active raid1 sdb1[1] sda1[0]
      976630464 blocks super 1.2 [2/2] [UU]

md1 : active raid5 sdd1[3] sdc1[2] sdb1[1] sda1[0]
      2929893888 blocks super 1.2 level 5, 512k chunk, algorithm 2 [4/4] [UUUU]

unused devices: <none>
";

    const MDSTAT_RESYNC: &str = "\
Personalities : [raid1] [raid6] [raid5] [raid4]
md0 : active raid1 sdb1[1] sda1[0]
      976630464 blocks super 1.2 [2/2] [UU]
      [==>..................]  resync = 12.6% (123119104/976630464) finish=118.4min speed=135000K/sec

md1 : active raid5 sdd1[3] sdc1[2] sdb1[1] sda1[0]
      2929893888 blocks super 1.2 level 5, 512k chunk, algorithm 2 [4/4] [UUUU]
      [>....................]  recovery = 0.4% (12345678/2929893888) finish=340.1min speed=140000K/sec

unused devices: <none>
";

    // -----------------------------------------------------------------
    // mdstat
    // -----------------------------------------------------------------

    #[test]
    fn an_idle_mdstat_reports_arrays_but_no_operations() {
        let arrays = parse_mdstat(MDSTAT_IDLE);
        assert_eq!(arrays.len(), 2);
        assert_eq!(arrays[0].name, "md0");
        assert_eq!(arrays[0].level, "raid1");
        assert_eq!(arrays[1].name, "md1");
        assert!(arrays.iter().all(|a| a.operations.is_empty()));
        assert!(operations_from_mdstat(MDSTAT_IDLE).is_empty());
    }

    /// The assertion 28.3 needed. `/proc/mdstat` puts the percentage on a
    /// continuation line, not on the `mdN : active …` header — a parser that
    /// only read header lines reported every resync as idle, which is exactly
    /// a "live" taskbar showing nothing while the disks grind.
    #[test]
    fn a_resync_on_a_continuation_line_is_reported_with_its_percentage() {
        let arrays = parse_mdstat(MDSTAT_RESYNC);
        assert_eq!(arrays[0].operations.len(), 1);
        assert_eq!(arrays[0].operations[0].op, MdOperation::Resync);
        assert_eq!(arrays[0].operations[0].percent, Some(12.6));
        assert!(!arrays[0].operations[0].delayed);
        assert_eq!(arrays[1].operations[0].op, MdOperation::Recovery);
        assert_eq!(arrays[1].operations[0].percent, Some(0.4));

        // The same percentages reach the row the taskbar renders.
        let ops = operations_from_mdstat(MDSTAT_RESYNC);
        assert_eq!(ops[0].progress, Some(12.6));
        assert_eq!(ops[1].progress, Some(0.4));
    }

    /// The status line belongs to the array **above** it. `md1`'s recovery must
    /// not be attributed to `md0`.
    #[test]
    fn an_operation_is_attached_to_the_array_above_its_status_line() {
        let arrays = parse_mdstat(MDSTAT_RESYNC);
        assert_eq!(arrays[0].name, "md0");
        assert_eq!(arrays[0].operations[0].op, MdOperation::Resync);
        assert_eq!(arrays[1].name, "md1");
        assert_eq!(arrays[1].operations[0].op, MdOperation::Recovery);
    }

    #[test]
    fn every_mdadm_operation_keyword_is_recognised() {
        assert_eq!(
            parse_md_status_line("[=>..]  resync = 1.0%"),
            Some(MdOperation::Resync)
        );
        assert_eq!(
            parse_md_status_line("[=>..]  recovery = 1.0%"),
            Some(MdOperation::Recovery)
        );
        assert_eq!(
            parse_md_status_line("[=>..]  reshape = 42.0%"),
            Some(MdOperation::Reshape)
        );
        assert_eq!(
            parse_md_status_line("[=>..]  check = 60.0%"),
            Some(MdOperation::Check)
        );
        assert_eq!(parse_md_status_line("[..]  finish=12min"), None);
    }

    #[test]
    fn a_delayed_resync_is_indeterminate_and_not_reported() {
        let raw = "\
md0 : active raid1 sdb1[1] sda1[0]
      976630464 blocks super 1.2 [2/2] [UU]
      [>....................]  resync = DELAYED
";
        // The keyword is still recognised — the row would be honest about the
        // operation — but the percentage is unknown and the kernel has not
        // started the work.
        let actions = parse_mdstat(raw)[0].operations.clone();
        assert_eq!(actions[0].op, MdOperation::Resync);
        assert_eq!(actions[0].percent, None);
        assert!(actions[0].delayed);
        // …and `is_reportable` keeps a 0 %-forever row out of the feed.
        assert!(!is_reportable(&actions[0]));
        assert!(
            operations_from_mdstat(raw).is_empty(),
            "a delayed resync is not work in progress"
        );
    }

    #[test]
    fn a_freshly_started_check_is_reported_even_at_zero_percent() {
        let raw = "\
md0 : active raid5 sdd1[3] sdc1[2] sdb1[1] sda1[0]
      [>....................]  check = 0.0% (0/2929893888) finish=999.9min speed=0K/sec
";
        // 0.0 % is a real measurement, not a delay: the check just started and
        // must be shown (it will run for hours).
        let actions = parse_mdstat(raw)[0].operations.clone();
        assert_eq!(actions[0].percent, Some(0.0));
        assert!(!actions[0].delayed);
        assert!(is_reportable(&actions[0]));
        let ops = operations_from_mdstat(raw);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].progress, Some(0.0));
    }

    #[test]
    fn an_inactive_array_reports_nothing() {
        let raw = "\
md0 : inactive sdb1[1] sda1[0]
      976630464 blocks super 1.2
";
        assert!(parse_mdstat(raw).is_empty());
        assert!(operations_from_mdstat(raw).is_empty());
    }

    #[test]
    fn an_unreadable_mdstat_is_an_empty_list_not_a_panic() {
        assert!(parse_mdstat("").is_empty());
        assert!(parse_mdstat("garbage\nmore garbage").is_empty());
        assert!(operations_from_mdstat("no arrays here").is_empty());
    }

    #[test]
    fn the_raid_job_id_is_derived_from_the_array_and_the_operation() {
        let ops = operations_from_mdstat(MDSTAT_RESYNC);
        assert_eq!(ops[0].id, "sys:raid:md0:resync");
        assert_eq!(ops[0].kind, KIND_RAID);
        assert_eq!(ops[1].id, "sys:raid:md1:recovery");
        // Stable across polls — the row advances instead of being recreated.
        assert_eq!(
            operations_from_mdstat(MDSTAT_RESYNC)[0].id,
            operations_from_mdstat(MDSTAT_RESYNC)[0].id
        );
    }

    // -----------------------------------------------------------------
    // zpool scrub
    // -----------------------------------------------------------------

    const ZPOOL_SCRUBBING: &str = "\
  pool: tank
 state: ONLINE
  scan: scrub in progress since Sun Oct 24 09:12:01 2026
        1.24T scanned at 823M/s, 412G issued at 273M/s, 1.40T total
        0B repaired, 29.37% done, 01:04:22 to go
config:

        NAME        STATE     READ WRITE CKSUM
        tank        ONLINE       0     0     0
          mirror-0  ONLINE       0     0     0

errors: No known data errors
";

    const ZPOOL_FINISHED: &str = "\
  pool: tank
 state: ONLINE
  scan: scrub repaired 0B in 02:41:12 with 0 errors on Sun Oct 18 03:41:13 2026
config:

errors: No known data errors
";

    #[test]
    fn a_zpool_scrub_in_progress_is_reported_with_its_percentage() {
        let state = parse_zpool_scan("tank", ZPOOL_SCRUBBING).expect("a live scrub");
        assert_eq!(state.pool, "tank");
        assert_eq!(state.progress, Some(29.37));
        assert!(state.message.contains("scrub in progress"), "{state:?}");
        // A one-line message: the three `scan:` continuation lines are joined.
        assert!(!state.message.contains('\n'));
    }

    /// A finished scrub must not produce a row that never ends. This is the
    /// failure mode that makes an activity feed worse than nothing.
    #[test]
    fn a_finished_zpool_scrub_produces_no_row() {
        assert!(parse_zpool_scan("tank", ZPOOL_FINISHED).is_none());
        assert!(parse_zpool_scan("tank", "  pool: tank\n state: ONLINE\n").is_none());
        assert!(
            parse_zpool_scan("tank", "  scan: none requested\n").is_none(),
            "a pool that never scrubbed is not scrubbing"
        );
    }

    #[test]
    fn a_young_zpool_scrub_is_indeterminate_until_it_estimates() {
        let raw = "\
  pool: tank
 state: ONLINE
  scan: scrub in progress since Sun Oct 24 09:12:01 2026
        120G scanned at 20G/s, 0B issued at 0B/s, 1.40T total
        no estimated completion time
config:
";
        let state = parse_zpool_scan("tank", raw).expect("still live");
        assert_eq!(state.progress, None, "no estimate yet is not 0 %");
    }

    #[test]
    fn a_zpool_resilver_is_reported_like_a_scrub() {
        let raw = "\
  pool: tank
 state: DEGRADED
  scan: resilver in progress since Sun Oct 24 09:12:01 2026
        412G scanned at 273M/s, 88G issued at 190M/s, 1.40T total
        0B resilvered, 6.11% done, 02:04:22 to go
config:
";
        let state = parse_zpool_scan("tank", raw).expect("a live resilver");
        assert_eq!(state.progress, Some(6.11));
    }

    #[test]
    fn the_scan_block_stops_at_the_next_top_level_field() {
        let raw = "\
  pool: tank
 state: ONLINE
  scan: scrub in progress since Sun Oct 24 09:12:01 2026
        1.24T scanned, 412G issued, 1.40T total
        0B repaired, 29.37% done, 01:04:22 to go
config:
        a line that is not part of the scan block 99% done
errors: No known data errors
";
        let state = parse_zpool_scan("tank", raw).expect("live");
        assert!(!state.message.contains("errors"), "{state:?}");
        assert_eq!(state.progress, Some(29.37));
    }

    // -----------------------------------------------------------------
    // btrfs scrub
    // -----------------------------------------------------------------

    const BTRFS_RUNNING: &str = "\
UUID:             ad0f0e1a-4cd0-4b39-8a4e-37b4a1b3ca11
Scrub started:    Sun Oct 24 09:12:01 2026
Status:           running
Duration:         0:07:34
Total to scrub:   412.00GiB
Bytes scrubbed:   91.00GiB (22.09%)
Rate:             250.00MiB/s
Error summary:    no errors found
";

    const BTRFS_FINISHED: &str = "\
Status:           finished
Duration:         0:31:02
Total to scrub:   412.00GiB
Bytes scrubbed:   412.00GiB (100.00%)
";

    #[test]
    fn a_running_btrfs_scrub_is_reported_with_its_percentage() {
        let state = parse_btrfs_scrub_status("data", BTRFS_RUNNING).expect("live");
        assert_eq!(state.progress, Some(22.09));
        assert!(state.message.contains("412.00GiB"), "{state:?}");
    }

    #[test]
    fn a_finished_or_aborted_btrfs_scrub_produces_no_row() {
        assert!(parse_btrfs_scrub_status("data", BTRFS_FINISHED).is_none());
        let aborted = "Status:           aborted\n";
        assert!(parse_btrfs_scrub_status("data", aborted).is_none());
        assert!(parse_btrfs_scrub_status("data", "").is_none());
    }

    #[test]
    fn an_old_btrfs_tool_without_a_percentage_is_indeterminate() {
        let raw = "\
Status:           running
Duration:         0:07:34
Total to scrub:   412.00GiB
Rate:             250.00MiB/s
";
        let state = parse_btrfs_scrub_status("data", raw).expect("live");
        assert_eq!(state.progress, None);
    }

    #[test]
    fn the_scrub_job_id_is_per_pool() {
        let s = parse_zpool_scan("tank", ZPOOL_SCRUBBING).unwrap();
        let op = operation_from_scrub(&s, KIND_SCRUB);
        assert_eq!(op.id, "sys:pool:tank:scrub");
        assert_eq!(op.kind, KIND_SCRUB);
        assert_eq!(op.progress, Some(29.37));
    }

    // -----------------------------------------------------------------
    // Syncthing
    // -----------------------------------------------------------------

    #[test]
    fn a_syncthing_folder_still_transferring_is_reported_on_bytes() {
        // 100 GB global, 25 GB still needed → 75 % done.
        let pct = folder_sync_progress(100 * 1024 * 1024 * 1024, 25 * 1024 * 1024 * 1024);
        assert_eq!(pct, Some(75.0));
    }

    /// The assertion the bytes-vs-files choice needed: a folder catching up on
    /// one huge file out of ten thousand is one file of the need — a file-count
    /// ratio would read 99 % done while 40 GB are still in flight.
    #[test]
    fn one_huge_file_out_of_many_is_not_reported_as_nearly_done() {
        // 10 000 files, 1 TB total; 40 GB still needed in a single file.
        let total = 1024u64 * 1024 * 1024 * 1024;
        let need = 40u64 * 1024 * 1024 * 1024;
        let pct = folder_sync_progress(total, need).expect("still work to do");
        assert!(pct < 97.0, "the bar must show 40 GB left, not 1 file: {pct}");
        assert!(pct > 90.0);
    }

    #[test]
    fn a_synced_or_empty_folder_produces_no_row() {
        assert_eq!(folder_sync_progress(1_000_000, 0), None);
        assert_eq!(folder_sync_progress(0, 0), None);
        assert_eq!(folder_sync_progress(0, 100), None);
    }

    #[test]
    fn a_paused_syncthing_folder_is_not_work_in_progress() {
        let folders = vec![SyncthingFolderNeed {
            id: "photos".into(),
            label: "Photos".into(),
            global_bytes: 1_000_000,
            need_bytes: 400_000,
            paused: true,
        }];
        assert!(
            operations_from_syncthing(&folders).is_empty(),
            "a paused folder is deliberately not transferring"
        );
    }

    #[test]
    fn a_syncthing_row_carries_the_label_and_the_backlog() {
        let folders = vec![SyncthingFolderNeed {
            id: "photos".into(),
            label: "Photos".into(),
            global_bytes: 2 * 1024 * 1024 * 1024,
            need_bytes: 512 * 1024 * 1024,
            paused: false,
        }];
        let ops = operations_from_syncthing(&folders);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].id, "sys:syncthing:photos");
        assert_eq!(ops[0].kind, KIND_SYNCTHING);
        assert!(ops[0].message.contains("Photos"), "{:?}", ops[0]);
        assert!(ops[0].message.contains("512.0 MB"), "{:?}", ops[0]);
    }

    #[test]
    fn an_unlabelled_syncthing_folder_falls_back_to_its_id() {
        let folders = vec![SyncthingFolderNeed {
            id: "abc123".into(),
            label: String::new(),
            global_bytes: 1000,
            need_bytes: 500,
            paused: false,
        }];
        let ops = operations_from_syncthing(&folders);
        assert!(ops[0].message.contains("abc123"), "{:?}", ops[0]);
    }

    // -----------------------------------------------------------------
    // Reconciliation
    // -----------------------------------------------------------------

    fn op(id: &str, progress: f64) -> LiveOperation {
        LiveOperation {
            id: id.into(),
            kind: KIND_SCRUB,
            progress: Some(progress),
            message: format!("op {id}"),
        }
    }

    #[test]
    fn an_operation_that_ended_is_completed_and_a_new_one_created() {
        let known: HashSet<String> = ["sys:pool:tank:scrub".to_string()].into_iter().collect();
        let plan = plan_reconcile(&known, vec![op("sys:raid:md0:resync", 10.0)]);
        assert_eq!(plan.live.len(), 1);
        assert_eq!(plan.live[0].id, "sys:raid:md0:resync");
        assert_eq!(plan.finished, vec!["sys:pool:tank:scrub".to_string()]);
    }

    /// The poller must never complete a job it did not create: a real backup
    /// whose id happens to be in `known` (or any foreign job) is left alone.
    #[test]
    fn the_poller_never_touches_a_job_it_did_not_create() {
        let known: HashSet<String> = ["somebackup-uuid".to_string(), "sys:raid:md0:resync".to_string()]
            .into_iter()
            .collect();
        let plan = plan_reconcile(&known, vec![]);
        assert_eq!(
            plan.finished,
            vec!["sys:raid:md0:resync".to_string()],
            "only the synthetic id is swept"
        );
    }

    #[test]
    fn a_still_running_operation_is_neither_created_nor_completed() {
        let known: HashSet<String> = ["sys:pool:tank:scrub".to_string()].into_iter().collect();
        let plan = plan_reconcile(&known, vec![op("sys:pool:tank:scrub", 42.0)]);
        assert_eq!(plan.live.len(), 1);
        assert!(plan.finished.is_empty());
    }

    #[test]
    fn an_empty_tick_completes_every_known_synthetic_id() {
        let known: HashSet<String> = [
            "sys:pool:tank:scrub".to_string(),
            "sys:raid:md0:resync".to_string(),
            "sys:syncthing:photos".to_string(),
        ]
        .into_iter()
        .collect();
        let plan = plan_reconcile(&known, vec![]);
        assert!(plan.live.is_empty());
        assert_eq!(plan.finished.len(), 3);
    }

    #[test]
    fn a_synthetic_job_is_created_then_advanced_on_the_same_id() {
        // Direct manager exercise: the id a poller derives is the id it
        // advances, which is what keeps one row per operation.
        let id = scrub_job_id("tank");
        PROGRESS_MANAGER.remove_job(&id);
        apply_plan(&ReconcilePlan {
            live: vec![LiveOperation {
                id: id.clone(),
                kind: KIND_SCRUB,
                progress: Some(30.0),
                message: "scrub".into(),
            }],
            finished: vec![],
        });
        let first = PROGRESS_MANAGER
            .get_progress(&id)
            .expect("the row exists after the first tick");
        assert_eq!(first.progress, 30.0, "the real percentage is used");
        assert!(matches!(first.job_type, JobType::Custom(ref s) if s == KIND_SCRUB));

        apply_plan(&ReconcilePlan {
            live: vec![LiveOperation {
                id: id.clone(),
                kind: KIND_SCRUB,
                progress: Some(70.0),
                message: "scrub".into(),
            }],
            finished: vec![],
        });
        let second = PROGRESS_MANAGER.get_progress(&id).expect("still tracked");
        assert_eq!(second.progress, 70.0, "the same row advanced");
        assert_eq!(
            PROGRESS_MANAGER
                .list_active_jobs()
                .into_iter()
                .filter(|(j, _, _)| j == &id)
                .count(),
            1,
            "one row per operation, never one per poll"
        );

        apply_plan(&ReconcilePlan {
            live: vec![],
            finished: vec![id.clone()],
        });
        assert!(
            PROGRESS_MANAGER.get_progress(&id).is_none(),
            "a finished operation leaves the feed"
        );
    }

    #[test]
    fn an_indeterminate_operation_is_stored_as_such() {
        let id = raid_job_id("md9", "resync");
        PROGRESS_MANAGER.remove_job(&id);
        let op = LiveOperation {
            id: id.clone(),
            kind: KIND_RAID,
            progress: None,
            message: "resync".into(),
        };
        apply_plan(&ReconcilePlan {
            live: vec![op.clone()],
            finished: vec![],
        });
        let snap = PROGRESS_MANAGER
            .list_activity()
            .into_iter()
            .find(|s| s.id == id)
            .expect("the row exists");
        assert_eq!(
            snap.progress, None,
            "an operation with no percentage is indeterminate, not 0 %"
        );

        // …and it **stays** indeterminate on the following ticks. Advancing a
        // tracked row used to go through `emit_progress`, which flags the job
        // as having reported a percentage: a source with no number turned into
        // a fake 0 % that then never moved. The bar must stay an indeterminate
        // slide for as long as the source says nothing.
        for _ in 0..3 {
            apply_plan(&ReconcilePlan {
                live: vec![op.clone()],
                finished: vec![],
            });
        }
        let snap = PROGRESS_MANAGER
            .list_activity()
            .into_iter()
            .find(|s| s.id == id)
            .expect("still the same row");
        assert_eq!(
            snap.progress, None,
            "a second and third tick must not fabricate a 0 %"
        );
        PROGRESS_MANAGER.remove_job(&id);
    }

    /// A row that reported a real percentage keeps it when a later tick has no
    /// number: the bar must not fall back to indeterminate mid-operation.
    #[test]
    fn a_known_percentage_survives_a_tick_without_one() {
        let id = scrub_job_id("tank");
        PROGRESS_MANAGER.remove_job(&id);
        apply_plan(&ReconcilePlan {
            live: vec![LiveOperation {
                id: id.clone(),
                kind: KIND_SCRUB,
                progress: Some(29.4),
                message: "scrub".into(),
            }],
            finished: vec![],
        });
        apply_plan(&ReconcilePlan {
            live: vec![LiveOperation {
                id: id.clone(),
                kind: KIND_SCRUB,
                progress: None,
                message: "scrub".into(),
            }],
            finished: vec![],
        });
        let snap = PROGRESS_MANAGER
            .list_activity()
            .into_iter()
            .find(|s| s.id == id)
            .expect("the row exists");
        assert_eq!(snap.progress, Some(29.4), "the bar keeps its last number");
        PROGRESS_MANAGER.remove_job(&id);
    }

    #[test]
    fn the_synthetic_prefix_is_what_marks_a_job_as_ours() {
        assert!(raid_job_id("md0", "resync").starts_with(SYNTHETIC_PREFIX));
        assert!(scrub_job_id("tank").starts_with(SYNTHETIC_PREFIX));
        assert!(syncthing_job_id("photos").starts_with(SYNTHETIC_PREFIX));
    }

    #[test]
    fn job_ids_are_stable_and_distinct() {
        assert_eq!(raid_job_id("md0", "resync"), raid_job_id("md0", "resync"));
        assert_ne!(raid_job_id("md0", "resync"), raid_job_id("md0", "check"));
        assert_ne!(raid_job_id("md0", "resync"), raid_job_id("md1", "resync"));
        assert_ne!(scrub_job_id("tank"), scrub_job_id("data"));
    }

    #[test]
    fn a_synthetic_operation_maps_to_a_custom_job_type() {
        let o = op("sys:pool:tank:scrub", 1.0);
        assert_eq!(o.job_type(), JobType::Custom(KIND_SCRUB.to_string()));
    }

    #[test]
    fn format_bytes_walks_the_ladder() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn collapse_whitespace_joins_a_multiline_block() {
        assert_eq!(
            collapse_whitespace("a  b\n c\t d "),
            "a b c d".to_string()
        );
    }

    /// ROADMAP X.9 — every subprocess of the tree runs under a deadline. This
    /// module adds two (`sudo zpool status`, `sudo btrfs scrub status`), so the
    /// source guard lands with them.
    #[test]
    fn every_subprocess_is_bounded() {
        let src = include_str!("activity_poller.rs");
        for needle in [
            concat!(".", "output()"),
            concat!(".", "spawn()"),
            concat!(".", "status()"),
        ] {
            assert!(
                !src.contains(needle),
                "`{needle}` waits without a deadline — use `run_timed_async` (ROADMAP X.9)"
            );
        }
        assert!(
            src.contains("run_timed_async("),
            "the bounded helper must be the one in use"
        );
        assert!(ZPOOL_PROBE_TIMEOUT <= POLL_INTERVAL * 2);
        assert!(BTRFS_PROBE_TIMEOUT <= POLL_INTERVAL * 2);
    }
}
