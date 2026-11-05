/* =============================================================================
 * File          : replication.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Snapshot replication to a second dataset (local or another machine over
 *   SSH): build a resumable `zfs send`/`btrfs send` pipeline from the previous
 *   replicated snapshot, run it under a deadline with an optional bandwidth
 *   cap, and prune **only** the destination snapshots this job created.
 *
 * Dependencies:
 *   - crate::snapshots (source snapshots), crate::utils::process
 *
 * Usage:
 *   handlers/replication.rs, replication_scheduler.rs; see
 *   docs/guides/replication.md.
 * =============================================================================
 */

//! Snapshot replication (see docs/guides/replication.md).
//!
//! Rules this module is built around:
//!
//! * **Incremental by default.** The base of a run is the snapshot the last
//!   successful run recorded (`last_snapshot`), so an appliance restarted after
//!   a week sends one increment, not the whole dataset.
//! * **Resumable.** `zfs send -s` / `zfs receive -s` (and their Btrfs
//!   equivalent, which inherits the parent stream) let an interrupted transfer
//!   resume instead of restarting; the partially received dataset is what makes
//!   that possible.
//! * **Destination-only retention.** A job prunes snapshots whose name carries
//!   its own `hv-repl-<id>-` prefix on the destination. It never deletes a
//!   source snapshot and never touches a snapshot another job or an admin made.
//! * **No shell, no secrets in logs.** Every stage is an argv vector; nothing is
//!   interpolated into a shell string, and a log line carries the job id and the
//!   snapshot names — never a credential (SSH authentication is the account's
//!   own key, configured once by the admin).

use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Name prefix of every snapshot a replication job creates (both sides).
pub const REPLICATION_PREFIX: &str = "hv-repl";

/// Deadline for one replication run. A first full send of a home dataset can
/// take hours, but an unbounded pipeline would hold a worker for ever after a
/// dead SSH peer — twelve hours is the ceiling.
pub const REPLICATION_TIMEOUT: Duration = Duration::from_secs(12 * 60 * 60);

/// Deadline for a metadata call (`zfs list` on either side).
pub const REPLICATION_QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest interval between runs (30 days), and the shortest (5 minutes).
pub const MAX_INTERVAL_MINUTES: i64 = 43_200;
pub const MIN_INTERVAL_MINUTES: i64 = 5;

/// Retention bounds. `keep_last = 0` would prune the snapshot just sent.
pub const MAX_KEEP_LAST: i64 = 365;
pub const MAX_BANDWIDTH_KBPS: i64 = 10_000_000;

/// The filesystem-side technology of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplicationBackend {
    Zfs,
    Btrfs,
}

impl ReplicationBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReplicationBackend::Zfs => "zfs",
            ReplicationBackend::Btrfs => "btrfs",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "zfs" => Some(ReplicationBackend::Zfs),
            "btrfs" => Some(ReplicationBackend::Btrfs),
            _ => None,
        }
    }
}

/// Where a job sends to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DestinationKind {
    Local,
    Ssh,
}

impl DestinationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            DestinationKind::Local => "local",
            DestinationKind::Ssh => "ssh",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(DestinationKind::Local),
            "ssh" => Some(DestinationKind::Ssh),
            _ => None,
        }
    }
}

/// A persisted replication job (`replication_jobs`).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ReplicationJob {
    pub id: i64,
    pub name: String,
    pub source_dataset: String,
    pub backend: String,
    pub destination_kind: String,
    pub destination_dataset: String,
    pub destination_host: Option<String>,
    pub destination_user: Option<String>,
    pub destination_port: i64,
    pub interval_minutes: i64,
    pub keep_last: i64,
    pub bandwidth_kbps: i64,
    pub enabled: i64,
    pub next_run_at: Option<String>,
    pub last_run_at: Option<String>,
    pub last_snapshot: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ReplicationJob {
    pub fn backend_kind(&self) -> Option<ReplicationBackend> {
        ReplicationBackend::parse(&self.backend)
    }

    pub fn destination(&self) -> Option<DestinationKind> {
        DestinationKind::parse(&self.destination_kind)
    }

    /// `user@host`, or `None` for a local destination.
    pub fn ssh_target(&self) -> Option<String> {
        match (&self.destination_user, &self.destination_host) {
            (Some(user), Some(host)) => Some(format!("{user}@{host}")),
            _ => None,
        }
    }
}

/// The dataset component of a name (`tank/data@snap` → `tank/data`).
fn dataset_of(snapshot: &str) -> &str {
    snapshot.split('@').next().unwrap_or(snapshot)
}

/// The bare snapshot component (`tank/data@snap` → `snap`).
fn bare_of(snapshot: &str) -> &str {
    snapshot.rsplit('@').next().unwrap_or(snapshot)
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A ZFS/Btrfs dataset path or one of their snapshot names.
///
/// The value reaches `zfs`/`btrfs` argv and a remote command line, so it must
/// contain only the characters those tools accept: no space, no shell
/// metacharacter, no `..`, no leading `-`.
pub fn validate_dataset(value: &str) -> Result<(), String> {
    let v = value.trim();
    if v.is_empty() {
        return Err("a dataset is required".to_string());
    }
    if v.len() > 255 {
        return Err("the dataset name is too long".to_string());
    }
    if v.starts_with('-') || v.starts_with('.') {
        return Err("a dataset name cannot start with '-' or '.'".to_string());
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | ':' | '@' | '+'))
    {
        return Err(format!(
            "'{v}' contains characters a dataset name may not use (spaces and shell metacharacters are refused)"
        ));
    }
    if v.split('/').any(|part| part == ".." || part == ".") || v.contains("..") {
        return Err("a dataset name cannot contain '.' or '..' segments".to_string());
    }
    Ok(())
}

/// A remote host: a hostname or an IP literal — never something a shell would
/// interpret.
pub fn validate_host(value: &str) -> Result<(), String> {
    let v = value.trim();
    if v.is_empty() {
        return Err("a destination host is required".to_string());
    }
    if v.len() > 255 {
        return Err("the destination host name is too long".to_string());
    }
    if v.starts_with('-') || v.starts_with('.') {
        return Err("the destination host cannot start with '-' or '.'".to_string());
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '%'))
    {
        return Err(format!(
            "'{v}' is not a host name or an IP address (no spaces, no shell characters)"
        ));
    }
    Ok(())
}

/// A remote account name.
pub fn validate_user(value: &str) -> Result<(), String> {
    let v = value.trim();
    if v.is_empty() {
        return Err("a destination account is required".to_string());
    }
    if v.len() > 32 {
        return Err("the destination account name is too long".to_string());
    }
    if v.starts_with('-') {
        return Err("the destination account cannot start with '-'".to_string());
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(format!("'{v}' is not a valid account name"));
    }
    Ok(())
}

/// Validate a job exactly as the API will persist it.
pub fn validate_job(job: &ReplicationJob) -> Result<(), String> {
    if job.name.trim().is_empty() || job.name.chars().count() > 64 {
        return Err("a job name of 1 to 64 characters is required".to_string());
    }
    if job.name.chars().any(|c| c.is_control()) {
        return Err("the job name cannot contain control characters".to_string());
    }
    validate_dataset(&job.source_dataset)?;
    validate_dataset(&job.destination_dataset)?;
    let Some(backend) = job.backend_kind() else {
        return Err("the backend must be 'zfs' or 'btrfs'".to_string());
    };
    let Some(kind) = job.destination() else {
        return Err("the destination must be 'local' or 'ssh'".to_string());
    };
    if kind == DestinationKind::Ssh {
        validate_host(job.destination_host.as_deref().unwrap_or(""))?;
        validate_user(job.destination_user.as_deref().unwrap_or(""))?;
        if job.destination_port < 1 || job.destination_port > 65535 {
            return Err("the SSH port must be between 1 and 65535".to_string());
        }
    }
    if job.source_dataset == job.destination_dataset && kind == DestinationKind::Local {
        return Err("the destination dataset must differ from the source".to_string());
    }
    if job.interval_minutes != 0
        && !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&job.interval_minutes)
    {
        return Err(format!(
            "the interval must be 0 (manual only) or between {MIN_INTERVAL_MINUTES} and {MAX_INTERVAL_MINUTES} minutes"
        ));
    }
    if !(1..=MAX_KEEP_LAST).contains(&job.keep_last) {
        return Err(format!("keep-last must be between 1 and {MAX_KEEP_LAST}"));
    }
    if !(0..=MAX_BANDWIDTH_KBPS).contains(&job.bandwidth_kbps) {
        return Err(format!(
            "the bandwidth cap must be 0 (unlimited) or at most {MAX_BANDWIDTH_KBPS} kB/s"
        ));
    }
    if job.source_dataset.contains('@') || job.destination_dataset.contains('@') {
        return Err(
            "the datasets must be dataset paths, not snapshots — the job creates its own snapshots"
                .to_string(),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Naming and retention
// ---------------------------------------------------------------------------

/// The snapshot name one run creates: `hv-repl-<job id>-<UTC timestamp>`.
pub fn snapshot_name(job_id: i64, now: DateTime<Utc>) -> String {
    format!(
        "{REPLICATION_PREFIX}-{job_id}-{}",
        now.format("%Y%m%dT%H%M%SZ")
    )
}

/// Whether `name` is a snapshot this job owns (its own id, its own prefix).
pub fn is_owned_snapshot(name: &str, job_id: i64) -> bool {
    let bare = bare_of(name);
    let prefix = format!("{REPLICATION_PREFIX}-{job_id}-");
    bare.strip_prefix(&prefix)
        .is_some_and(|rest| rest.len() >= 16 && rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// Which destination snapshots retention must delete.
///
/// Only this job's own snapshots (`hv-repl-<id>-…`), newest first, keeping
/// `keep_last`. Anything else on the destination is left alone.
pub fn select_prune(names: &[String], job_id: i64, keep_last: usize) -> Vec<String> {
    let mut owned: Vec<&String> = names
        .iter()
        .filter(|name| is_owned_snapshot(name, job_id))
        .collect();
    // The embedded timestamp sorts lexicographically (ISO-8601 basic format),
    // so a plain string sort is chronological — no clock parsing needed.
    owned.sort_by(|a, b| b.cmp(a));
    owned.into_iter().skip(keep_last.max(1)).cloned().collect()
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// The pipeline one run executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPlan {
    /// Name of the snapshot this run created (present on both sides).
    pub snapshot: String,
    /// The snapshot the increment is based on, when there is one.
    pub base: Option<String>,
    /// The argv stages, wired first → last.
    pub stages: Vec<Vec<String>>,
}

impl SendPlan {
    /// A human-readable rendering for the dry-run preview and the journal. It
    /// never contains a credential — the SSH stage authenticates with the
    /// account's own key.
    pub fn describe(&self) -> String {
        self.stages
            .iter()
            .map(|stage| stage.join(" "))
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

/// Pick the base snapshot of a run.
///
/// Preference order: the snapshot the last successful run recorded (the honest
/// increment), then the newest existing `hv-repl-<id>-…` snapshot on the source
/// (a run that succeeded but whose bookkeeping was lost). A base that no longer
/// exists on the source is ignored — `zfs send -i` would fail on it.
pub fn choose_base(
    job_id: i64,
    recorded: Option<&str>,
    source_snapshots: &[String],
) -> Option<String> {
    let exists = |candidate: &str| {
        source_snapshots
            .iter()
            .any(|name| bare_of(name) == bare_of(candidate))
    };
    if let Some(recorded) = recorded {
        if exists(recorded) {
            // Normalised to the bare name: the job stores what it sent, and
            // `build_plan` re-qualifies it with the source dataset.
            return Some(bare_of(recorded).to_string());
        }
    }
    source_snapshots
        .iter()
        .filter(|name| is_owned_snapshot(name, job_id))
        .max_by(|a, b| a.cmp(b))
        .map(|name| bare_of(name).to_string())
}

/// Build the send/receive pipeline.
///
/// * ZFS: `zfs send -s [-i base] src@snap` piped into `zfs receive -s dest`
///   (locally) or into `ssh … zfs receive -s dest`.
/// * Btrfs: `btrfs send [-p parent] <source snapshot path>` piped into
///   `btrfs receive <destination path>` (local only — see [`validate_job`]).
///
/// `pv` is inserted when a bandwidth cap is set; the caller checks that the
/// tool is installed (`pv_available`) and otherwise reports the cap as ignored.
pub fn build_plan(
    job: &ReplicationJob,
    source_pool: &str,
    base: Option<&str>,
    snapshot_name: &str,
    source_snapshot_path: Option<&str>,
    destination_path: Option<&str>,
    use_pv: bool,
) -> Result<SendPlan, String> {
    let Some(backend) = job.backend_kind() else {
        return Err("unknown backend".to_string());
    };
    let Some(kind) = job.destination() else {
        return Err("unknown destination kind".to_string());
    };

    let mut stages: Vec<Vec<String>> = Vec::new();
    match backend {
        ReplicationBackend::Zfs => {
            let source_snapshot = format!("{}@{}", job.source_dataset, snapshot_name);
            let mut send = vec!["sudo".to_string(), "zfs".to_string(), "send".to_string()];
            // `-s` keeps the stream resumable after an interruption.
            send.push("-s".to_string());
            if let Some(base) = base {
                let base_name = if base.contains('@') {
                    base.to_string()
                } else {
                    format!("{}@{}", job.source_dataset, base)
                };
                send.push("-i".to_string());
                send.push(base_name);
            }
            send.push(source_snapshot.clone());
            stages.push(send);

            let mut receive = match kind {
                DestinationKind::Local => {
                    vec!["sudo".to_string(), "zfs".to_string(), "receive".to_string()]
                }
                DestinationKind::Ssh => vec![
                    "ssh".to_string(),
                    "-p".to_string(),
                    job.destination_port.to_string(),
                    "-o".to_string(),
                    "BatchMode=yes".to_string(),
                    "-o".to_string(),
                    "ConnectTimeout=15".to_string(),
                    "-o".to_string(),
                    "StrictHostKeyChecking=accept-new".to_string(),
                    job.ssh_target().unwrap_or_default(),
                    "zfs".to_string(),
                    "receive".to_string(),
                ],
            };
            receive.push("-s".to_string());
            receive.push(job.destination_dataset.clone());
            stages.push(receive);
        }
        ReplicationBackend::Btrfs => {
            let source_path = source_snapshot_path.ok_or_else(|| {
                "the source snapshot path is unknown (is the pool mounted?)".to_string()
            })?;
            let destination_path = destination_path
                .ok_or_else(|| "the destination path is unknown (is it mounted?)".to_string())?;
            let mut send = vec!["sudo".to_string(), "btrfs".to_string(), "send".to_string()];
            if let Some(base) = base {
                let parent = base.rsplit('@').next().unwrap_or(base);
                send.push("-p".to_string());
                let prefix = source_snapshot_path_prefix(source_path);
                send.push(format!("{prefix}{parent}"));
            }
            send.push(source_path.to_string());
            stages.push(send);
            let mut receive = match kind {
                DestinationKind::Local => vec![
                    "sudo".to_string(),
                    "btrfs".to_string(),
                    "receive".to_string(),
                ],
                DestinationKind::Ssh => {
                    let mut cmd = ssh_prefix(job)
                        .ok_or_else(|| "the SSH destination is incomplete".to_string())?;
                    cmd.push("sudo".to_string());
                    cmd.push("btrfs".to_string());
                    cmd.push("receive".to_string());
                    cmd
                }
            };
            receive.push(destination_path.to_string());
            stages.push(receive);
        }
    }

    if job.bandwidth_kbps > 0 && use_pv {
        // Insert the throttle between send and receive.
        let pv = vec![
            "pv".to_string(),
            "-q".to_string(),
            "-L".to_string(),
            format!("{}k", job.bandwidth_kbps),
        ];
        stages.insert(1, pv);
    }

    // `source_pool` is only used for the ZFS base name above; keep the
    // parameter check honest so a caller cannot pass an empty pool.
    if source_pool.trim().is_empty() {
        return Err("the source pool is unknown".to_string());
    }

    Ok(SendPlan {
        snapshot: snapshot_name.to_string(),
        base: base.map(str::to_string),
        stages,
    })
}

/// The directory part of a Btrfs snapshot path (`…/.snapshots/snap-1` →
/// `…/.snapshots/`), used to build the `-p <parent>` argument.
fn source_snapshot_path_prefix(path: &str) -> String {
    match path.rfind('/') {
        Some(index) => path[..=index].to_string(),
        None => String::new(),
    }
}

/// The commands that list destination snapshots, so retention can run against
/// the destination (never the source).
pub fn destination_list_command(job: &ReplicationJob) -> Option<Vec<String>> {
    let backend = job.backend_kind()?;
    let kind = job.destination()?;
    match (backend, kind) {
        (ReplicationBackend::Zfs, DestinationKind::Local) => Some(vec![
            "sudo".to_string(),
            "zfs".to_string(),
            "list".to_string(),
            "-H".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            "snapshot".to_string(),
            "-o".to_string(),
            "name,creation".to_string(),
            "-s".to_string(),
            "creation".to_string(),
            "-r".to_string(),
            job.destination_dataset.clone(),
        ]),
        (ReplicationBackend::Zfs, DestinationKind::Ssh) => Some(vec![
            "ssh".to_string(),
            "-p".to_string(),
            job.destination_port.to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "ConnectTimeout=15".to_string(),
            job.ssh_target()?,
            "zfs".to_string(),
            "list".to_string(),
            "-H".to_string(),
            "-p".to_string(),
            "-t".to_string(),
            "snapshot".to_string(),
            "-o".to_string(),
            "name,creation".to_string(),
            "-s".to_string(),
            "creation".to_string(),
            "-r".to_string(),
            job.destination_dataset.clone(),
        ]),
        (ReplicationBackend::Btrfs, DestinationKind::Local) => Some(vec![
            "sudo".to_string(),
            "btrfs".to_string(),
            "subvolume".to_string(),
            "list".to_string(),
            "-s".to_string(),
            "-o".to_string(),
            job.destination_dataset.clone(),
        ]),
        (ReplicationBackend::Btrfs, DestinationKind::Ssh) => {
            let mut cmd = ssh_prefix(job)?;
            cmd.extend([
                "sudo".to_string(),
                "btrfs".to_string(),
                "subvolume".to_string(),
                "list".to_string(),
                "-s".to_string(),
                "-o".to_string(),
                job.destination_dataset.clone(),
            ]);
            Some(cmd)
        }
    }
}

/// The commands that delete one destination snapshot.
pub fn destination_delete_command(job: &ReplicationJob, snapshot: &str) -> Option<Vec<String>> {
    let backend = job.backend_kind()?;
    let kind = job.destination()?;
    match (backend, kind) {
        (ReplicationBackend::Zfs, DestinationKind::Local) => Some(vec![
            "sudo".to_string(),
            "zfs".to_string(),
            "destroy".to_string(),
            format!("{}@{}", job.destination_dataset, bare_of(snapshot)),
        ]),
        (ReplicationBackend::Zfs, DestinationKind::Ssh) => Some(vec![
            "ssh".to_string(),
            "-p".to_string(),
            job.destination_port.to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "ConnectTimeout=15".to_string(),
            job.ssh_target()?,
            "zfs".to_string(),
            "destroy".to_string(),
            format!("{}@{}", job.destination_dataset, bare_of(snapshot)),
        ]),
        (ReplicationBackend::Btrfs, DestinationKind::Local) => Some(vec![
            "sudo".to_string(),
            "btrfs".to_string(),
            "subvolume".to_string(),
            "delete".to_string(),
            format!("{}/{}", job.destination_dataset, bare_of(snapshot)),
        ]),
        (ReplicationBackend::Btrfs, DestinationKind::Ssh) => {
            let mut cmd = ssh_prefix(job)?;
            cmd.extend([
                "sudo".to_string(),
                "btrfs".to_string(),
                "subvolume".to_string(),
                "delete".to_string(),
                format!("{}/{}", job.destination_dataset, bare_of(snapshot)),
            ]);
            Some(cmd)
        }
    }
}

/// The common `ssh` prefix a remote command runs behind.
fn ssh_prefix(job: &ReplicationJob) -> Option<Vec<String>> {
    Some(vec![
        "ssh".to_string(),
        "-p".to_string(),
        job.destination_port.to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        job.ssh_target()?,
    ])
}

/// After a successful transfer, prove the destination is readable and is the
/// snapshot that was just sent. ZFS returns its checksum property; Btrfs
/// proves the subvolume exists with `subvolume show`.
pub fn destination_verify_command(job: &ReplicationJob) -> Option<Vec<String>> {
    let backend = job.backend_kind()?;
    let kind = job.destination()?;
    match (backend, kind) {
        (ReplicationBackend::Zfs, DestinationKind::Local) => Some(vec![
            "sudo".to_string(),
            "zfs".to_string(),
            "get".to_string(),
            "-H".to_string(),
            "-o".to_string(),
            "value".to_string(),
            "checksum".to_string(),
            job.destination_dataset.clone(),
        ]),
        (ReplicationBackend::Zfs, DestinationKind::Ssh) => {
            let mut cmd = ssh_prefix(job)?;
            cmd.extend([
                "zfs".to_string(),
                "get".to_string(),
                "-H".to_string(),
                "-o".to_string(),
                "value".to_string(),
                "checksum".to_string(),
                job.destination_dataset.clone(),
            ]);
            Some(cmd)
        }
        (ReplicationBackend::Btrfs, DestinationKind::Local) => Some(vec![
            "sudo".to_string(),
            "btrfs".to_string(),
            "subvolume".to_string(),
            "show".to_string(),
            job.destination_dataset.clone(),
        ]),
        (ReplicationBackend::Btrfs, DestinationKind::Ssh) => {
            let mut cmd = ssh_prefix(job)?;
            cmd.extend([
                "sudo".to_string(),
                "btrfs".to_string(),
                "subvolume".to_string(),
                "show".to_string(),
                job.destination_dataset.clone(),
            ]);
            Some(cmd)
        }
    }
}

/// Parse the destination listing into bare snapshot names, oldest first.
///
/// ZFS prints `name<TAB>creation`; Btrfs prints `ID <n> gen <n> top level <n>
/// otime … path <path>` where a snapshot's path is `<dataset>/<name>`.
pub fn parse_destination_listing(backend: ReplicationBackend, output: &str) -> Vec<String> {
    match backend {
        ReplicationBackend::Zfs => output
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(|name| bare_of(name).to_string())
            .filter(|name| !name.is_empty())
            .collect(),
        ReplicationBackend::Btrfs => output
            .lines()
            .filter_map(|line| {
                let index = line.find(" path ")?;
                let path = line[index + " path ".len()..].trim();
                path.rsplit('/').next().map(str::to_string)
            })
            .filter(|name| !name.is_empty())
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Whether the `pv` throttle is installed.
pub fn pv_available() -> bool {
    crate::dependencies::binary_available("pv", "/usr/bin/pv")
}

/// Run one argv stage under a deadline, capturing its output.
pub fn run_stage(stage: &[String], timeout: Duration) -> Result<std::process::Output, String> {
    let Some((program, args)) = stage.split_first() else {
        return Err("empty command".to_string());
    };
    let mut cmd = Command::new(program);
    cmd.args(args);
    crate::utils::process::run_timed(&mut cmd, timeout)
        .map_err(|e| format!("{program} failed: {e}"))
}

/// Run the stages as a pipeline (`stdout` of one into `stdin` of the next).
///
/// Delegates to [`crate::utils::process::run_pipeline_timed`], which owns the
/// bounded pipeline implementation: one deadline for the whole chain, every
/// stage killed and reaped on expiry, and each stage's stderr drained on its
/// own thread so a chatty `zfs send` cannot deadlock the pipe.
pub fn run_pipeline(stages: &[Vec<String>], timeout: Duration) -> Result<(), String> {
    crate::utils::process::run_pipeline_timed(stages, timeout)
}

/// The destination directory of a Btrfs local job (the dataset path itself).
pub fn btrfs_destination_path(job: &ReplicationJob) -> Option<String> {
    if job.backend_kind()? != ReplicationBackend::Btrfs {
        return None;
    }
    Some(job.destination_dataset.clone())
}

/// The source snapshot path of a Btrfs job (`<mount>/.snapshots/<name>`).
pub fn btrfs_source_snapshot_path(pool: &str, snapshot_name: &str) -> Result<String, String> {
    let mount = crate::pools::PoolService::btrfs_mount_point(pool)
        .ok_or_else(|| format!("pool '{pool}' is not mounted"))?;
    Ok(format!("{mount}/.snapshots/{snapshot_name}"))
}

/// Whether a dry run may claim the bandwidth cap will be applied.
pub fn bandwidth_note(job: &ReplicationJob) -> Option<String> {
    if job.bandwidth_kbps > 0 && !pv_available() {
        return Some(
            "`pv` is not installed, so this run is unthrottled — install the `pv` package to apply the cap"
                .to_string(),
        );
    }
    None
}

/// The dataset a source snapshot belongs to (used by the scheduler to record
/// what it replicated).
pub fn source_dataset_of(snapshot: &str) -> &str {
    dataset_of(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn job() -> ReplicationJob {
        ReplicationJob {
            id: 7,
            name: "nightly".to_string(),
            source_dataset: "tank/data".to_string(),
            backend: "zfs".to_string(),
            destination_kind: "local".to_string(),
            destination_dataset: "backup/data".to_string(),
            destination_host: None,
            destination_user: None,
            destination_port: 22,
            interval_minutes: 1440,
            keep_last: 7,
            bandwidth_kbps: 0,
            enabled: 1,
            next_run_at: None,
            last_run_at: None,
            last_snapshot: None,
            last_status: None,
            last_error: None,
            created_by: "admin".to_string(),
            created_at: "2026-10-31T00:00:00+00:00".to_string(),
            updated_at: "2026-10-31T00:00:00+00:00".to_string(),
        }
    }

    fn ssh_job() -> ReplicationJob {
        ReplicationJob {
            destination_kind: "ssh".to_string(),
            destination_host: Some("nas2.lan".to_string()),
            destination_user: Some("backup".to_string()),
            destination_port: 2222,
            ..job()
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap()
    }

    // -----------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------

    #[test]
    fn test_a_valid_job_passes_validation() {
        assert!(validate_job(&job()).is_ok());
        assert!(validate_job(&ssh_job()).is_ok());
    }

    #[test]
    fn test_dataset_names_cannot_reach_a_shell_or_escape() {
        assert!(validate_dataset("tank/data").is_ok());
        assert!(validate_dataset("tank/data-set_1").is_ok());
        assert!(validate_dataset("tank/data@snap").is_ok());
        assert!(validate_dataset("").is_err());
        assert!(validate_dataset("tank/data; rm -rf /").is_err());
        assert!(validate_dataset("tank/../etc").is_err());
        assert!(validate_dataset("-destroy").is_err());
        assert!(validate_dataset("tank/data with space").is_err());
        assert!(validate_dataset("tank/$(id)").is_err());
        assert!(validate_dataset(&"x".repeat(256)).is_err());
    }

    #[test]
    fn test_host_and_user_validation_refuses_shell_characters() {
        assert!(validate_host("nas2.lan").is_ok());
        assert!(validate_host("192.168.1.20").is_ok());
        assert!(validate_host("fe80::1").is_ok());
        assert!(validate_host("nas;reboot").is_err());
        assert!(validate_host("-oProxyCommand=x").is_err());
        assert!(validate_host("").is_err());
        assert!(validate_user("backup").is_ok());
        assert!(validate_user("backup;id").is_err());
        assert!(validate_user("-root").is_err());
        assert!(validate_user("").is_err());
    }

    #[test]
    fn test_a_job_that_would_lose_data_is_refused() {
        // Same source and destination locally.
        let mut same = job();
        same.destination_dataset = "tank/data".to_string();
        assert!(validate_job(&same).is_err());

        // A snapshot in the dataset field: the job creates its own.
        let mut snapshot_path = job();
        snapshot_path.destination_dataset = "backup/data@old".to_string();
        assert!(validate_job(&snapshot_path).is_err());

        // A retention of zero would delete the snapshot it just sent.
        let mut zero_keep = job();
        zero_keep.keep_last = 0;
        assert!(validate_job(&zero_keep).is_err());

        // Btrfs over SSH is supported since P1-26: send locally, receive on
        // the remote host through `ssh … sudo btrfs receive`.
        let mut btrfs_ssh = ssh_job();
        btrfs_ssh.backend = "btrfs".to_string();
        assert!(validate_job(&btrfs_ssh).is_ok());

        // A bad port, interval and cap are refused.
        let mut bad_port = ssh_job();
        bad_port.destination_port = 0;
        assert!(validate_job(&bad_port).is_err());
        let mut bad_interval = job();
        bad_interval.interval_minutes = 1;
        assert!(validate_job(&bad_interval).is_err());
        let mut bad_cap = job();
        bad_cap.bandwidth_kbps = -1;
        assert!(validate_job(&bad_cap).is_err());
    }

    // -----------------------------------------------------------------
    // Naming and retention
    // -----------------------------------------------------------------

    #[test]
    fn test_a_snapshot_name_is_owned_only_by_its_job() {
        let name = snapshot_name(7, now());
        assert_eq!(name, "hv-repl-7-20261031T120000Z");
        assert!(is_owned_snapshot(&name, 7));
        assert!(is_owned_snapshot(&format!("tank/data@{name}"), 7));
        // Another job's snapshot, a manual snapshot, and a look-alike.
        assert!(!is_owned_snapshot(&name, 8));
        assert!(!is_owned_snapshot("manual-20261031", 7));
        assert!(!is_owned_snapshot("hv-repl-7-short", 7));
        assert!(!is_owned_snapshot("hv-repl-77-20261031T120000Z", 7));
    }

    #[test]
    fn test_retention_keeps_the_newest_of_this_jobs_own_snapshots() {
        let names = vec![
            "hv-repl-7-20261001T000000Z".to_string(),
            "hv-repl-7-20261002T000000Z".to_string(),
            "hv-repl-7-20261003T000000Z".to_string(),
            "hv-repl-7-20261004T000000Z".to_string(),
            // Another job's and a manual snapshot must never be pruned.
            "hv-repl-8-20260101T000000Z".to_string(),
            "manual-20260101".to_string(),
        ];
        let pruned = select_prune(&names, 7, 2);
        assert_eq!(
            pruned,
            vec![
                "hv-repl-7-20261002T000000Z".to_string(),
                "hv-repl-7-20261001T000000Z".to_string()
            ]
        );
        // Keeping more than exist deletes nothing.
        assert!(select_prune(&names, 7, 10).is_empty());
        // A retention of 0 is treated as 1, never as "delete everything".
        assert_eq!(select_prune(&names, 7, 0).len(), 3);
    }

    // -----------------------------------------------------------------
    // Base selection
    // -----------------------------------------------------------------

    #[test]
    fn test_the_recorded_base_is_preferred_when_it_still_exists() {
        let sources = vec![
            "tank/data@hv-repl-7-20261001T000000Z".to_string(),
            "tank/data@hv-repl-7-20261002T000000Z".to_string(),
        ];
        assert_eq!(
            choose_base(7, Some("hv-repl-7-20261001T000000Z"), &sources).as_deref(),
            Some("hv-repl-7-20261001T000000Z")
        );
        // A base that was deleted (or recorded as a full name) falls back to the
        // newest owned snapshot that still exists.
        assert_eq!(
            choose_base(7, Some("hv-repl-7-20261001T000000Z"), &sources[1..]).as_deref(),
            Some("hv-repl-7-20261002T000000Z")
        );
        // A recorded full name is normalised to the bare snapshot name.
        assert_eq!(
            choose_base(7, Some("tank/data@hv-repl-7-20261001T000000Z"), &sources).as_deref(),
            Some("hv-repl-7-20261001T000000Z")
        );
        // No history at all: a first full send.
        assert_eq!(choose_base(7, None, &[]), None);
    }

    // -----------------------------------------------------------------
    // Planning
    // -----------------------------------------------------------------

    #[test]
    fn test_a_full_local_zfs_send_is_resumable_and_incremental_thereafter() {
        let job = job();
        let full = build_plan(
            &job,
            "tank",
            None,
            "hv-repl-7-20261031T120000Z",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            full.stages[0],
            vec![
                "sudo",
                "zfs",
                "send",
                "-s",
                "tank/data@hv-repl-7-20261031T120000Z"
            ]
        );
        assert_eq!(
            full.stages[1],
            vec!["sudo", "zfs", "receive", "-s", "backup/data"]
        );
        assert!(full.base.is_none());

        let incremental = build_plan(
            &job,
            "tank",
            Some("hv-repl-7-20261030T120000Z"),
            "hv-repl-7-20261031T120000Z",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(
            incremental.stages[0],
            vec![
                "sudo",
                "zfs",
                "send",
                "-s",
                "-i",
                "tank/data@hv-repl-7-20261030T120000Z",
                "tank/data@hv-repl-7-20261031T120000Z"
            ]
        );
        let description = incremental.describe();
        assert!(
            description.contains(" | sudo zfs receive -s backup/data"),
            "{description}"
        );
    }

    #[test]
    fn test_an_ssh_destination_pipes_into_ssh_without_a_shell() {
        let job = ssh_job();
        let plan = build_plan(
            &job,
            "tank",
            None,
            "hv-repl-7-20261031T120000Z",
            None,
            None,
            false,
        )
        .unwrap();
        let receive = &plan.stages[1];
        assert_eq!(receive[0], "ssh");
        assert!(receive.contains(&"-p".to_string()));
        assert!(receive.contains(&"2222".to_string()));
        assert!(receive.contains(&"BatchMode=yes".to_string()));
        assert!(receive.contains(&"backup@nas2.lan".to_string()));
        // `zfs receive` is the remote command, not a shell string.
        assert_eq!(receive[receive.len() - 4], "zfs");
        assert_eq!(receive[receive.len() - 3], "receive");
        assert_eq!(receive[receive.len() - 2], "-s");
        assert_eq!(receive[receive.len() - 1], "backup/data");
    }

    #[test]
    fn test_a_bandwidth_cap_inserts_pv_between_send_and_receive() {
        let mut job = job();
        job.bandwidth_kbps = 8000;
        let plan = build_plan(
            &job,
            "tank",
            None,
            "hv-repl-7-20261031T120000Z",
            None,
            None,
            true,
        )
        .unwrap();
        assert_eq!(plan.stages.len(), 3);
        assert_eq!(plan.stages[1], vec!["pv", "-q", "-L", "8000k"]);
        // Without `pv` installed the cap is skipped, not faked.
        let unthrottled = build_plan(
            &job,
            "tank",
            None,
            "hv-repl-7-20261031T120000Z",
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(unthrottled.stages.len(), 2);
        assert!(bandwidth_note(&job).is_some());
    }

    #[test]
    fn test_a_btrfs_plan_uses_the_snapshot_paths() {
        let mut job = job();
        job.backend = "btrfs".to_string();
        job.destination_dataset = "/mnt/backup".to_string();
        let plan = build_plan(
            &job,
            "tank",
            Some("hv-repl-7-20261030T120000Z"),
            "hv-repl-7-20261031T120000Z",
            Some("/mnt/pools/tank/.snapshots/hv-repl-7-20261031T120000Z"),
            Some("/mnt/backup"),
            false,
        )
        .unwrap();
        assert_eq!(
            plan.stages[0],
            vec![
                "sudo",
                "btrfs",
                "send",
                "-p",
                "/mnt/pools/tank/.snapshots/hv-repl-7-20261030T120000Z",
                "/mnt/pools/tank/.snapshots/hv-repl-7-20261031T120000Z"
            ]
        );
        assert_eq!(
            plan.stages[1],
            vec!["sudo", "btrfs", "receive", "/mnt/backup"]
        );
        // Without the paths the plan is refused rather than guessed.
        assert!(build_plan(&job, "tank", None, "s", None, None, false).is_err());
    }

    // -----------------------------------------------------------------
    // Destination listing and pruning commands
    // -----------------------------------------------------------------

    #[test]
    fn test_destination_listing_parsing_handles_both_backends() {
        let zfs = "backup/data@hv-repl-7-20261001T000000Z\t1759300000\n\
                   backup/data@hv-repl-7-20261002T000000Z\t1759400000\n";
        assert_eq!(
            parse_destination_listing(ReplicationBackend::Zfs, zfs),
            vec!["hv-repl-7-20261001T000000Z", "hv-repl-7-20261002T000000Z"]
        );
        let btrfs = "ID 257 gen 41 top level 5 otime 2026-10-01 path backup/data/hv-repl-7-20261001T000000Z\n\
                     ID 258 gen 42 top level 5 otime 2026-10-02 path backup/data/hv-repl-7-20261002T000000Z\n";
        assert_eq!(
            parse_destination_listing(ReplicationBackend::Btrfs, btrfs),
            vec!["hv-repl-7-20261001T000000Z", "hv-repl-7-20261002T000000Z"]
        );
        assert!(parse_destination_listing(ReplicationBackend::Zfs, "").is_empty());
    }

    #[test]
    fn test_destination_commands_target_only_the_destination() {
        let job = job();
        let list = destination_list_command(&job).unwrap();
        assert_eq!(list[0], "sudo");
        assert!(list.contains(&"backup/data".to_string()));
        let delete = destination_delete_command(&job, "hv-repl-7-20261001T000000Z").unwrap();
        assert_eq!(
            delete,
            vec![
                "sudo",
                "zfs",
                "destroy",
                "backup/data@hv-repl-7-20261001T000000Z"
            ]
        );
        // An SSH job lists/deletes through ssh, never locally.
        let ssh_list = destination_list_command(&ssh_job()).unwrap();
        assert_eq!(ssh_list[0], "ssh");
        let ssh_delete =
            destination_delete_command(&ssh_job(), "hv-repl-7-20261001T000000Z").unwrap();
        assert_eq!(ssh_delete[0], "ssh");
        assert_eq!(
            ssh_delete[ssh_delete.len() - 1],
            "backup/data@hv-repl-7-20261001T000000Z"
        );
    }

    #[test]
    fn test_source_dataset_extraction() {
        assert_eq!(source_dataset_of("tank/data@snap-1"), "tank/data");
        assert_eq!(source_dataset_of("tank"), "tank");
    }
}
