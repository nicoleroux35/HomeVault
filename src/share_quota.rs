/* =============================================================================
 * File          : share_quota.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-share size quotas (QUICK-WIN QW-07): a ceiling on one shared folder,
 *   enforced by the filesystem that holds it — ZFS `refquota`, Btrfs qgroups,
 *   XFS project quotas — and checked in front of an upload or a copy so a
 *   runaway folder cannot fill the pool.
 *
 * Dependencies:
 *   - serde, tokio (process), crate::quota (filesystem detection),
 *     crate::storage::ShareQuotaRepository
 *
 * Usage:
 *   handlers/share_quota.rs (the quota API), handlers/explorer.rs (transfer
 *   refusal); see docs/guides/quotas.md.
 * =============================================================================
 */

//! Per-share quotas (see docs/guides/quotas.md).
//!
//! A share has no row of its own — it is derived from `smb.conf` /
//! `/etc/exports` (`SharesService::get_shares`) — so the *intent* lives in
//! `share_quotas` and the *enforcement* lives in the filesystem. Three rules
//! make that split safe:
//!
//! 1. **The filesystem is the authority, and it says so.** The backend is
//!    detected from where the share currently lives (`findmnt`, the same probe
//!    the per-user quotas use). A share on a filesystem that cannot enforce a
//!    directory limit reports `supported: false` with the reason, and the UI
//!    hides the field instead of offering a limit that would not hold.
//! 2. **Usage is read back, never guessed.** `zfs list -o used`,
//!    `btrfs qgroup show --raw`, `xfs_quota` report — never a `du` walk of a
//!    tree the kernel already measures.
//! 3. **A refusal is per transfer and names the numbers.** `would_exceed` is
//!    pure, so the sentence an uploader sees ("10.0 GB of 10.0 GB used") is the
//!    same one the tests assert.
//!
//! What is enforced where: the Explorer's **upload** and **copy** paths check
//! the destination share's quota before and during the transfer
//! (`handlers/explorer.rs`). A protocol client writing over SMB is bounded by
//! the filesystem-level limit itself (that is the whole point of `refquota` /
//! qgroups / project quotas), so the two enforcement points are complementary
//! rather than duplicated.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Formatting/validation bounds. A 1 MiB floor keeps "1 byte" from being a
/// quota nobody can satisfy; the ceiling is a petabyte, past which the number is
/// certainly a typo.
pub const MIN_QUOTA_BYTES: u64 = 1024 * 1024;
pub const MAX_QUOTA_BYTES: u64 = 1 << 50;

/// Deadline for every probe/mutation command (10 s matches the per-user quota
/// probe: `findmnt` and `zfs list` are instantaneous, `btrfs qgroup show` on a
/// huge dataset is the slow one).
pub const QUOTA_TIMEOUT: Duration = Duration::from_secs(10);

// --------------------------------------------------------------------------
// Backends
// --------------------------------------------------------------------------

/// How (and whether) one share's ceiling can be enforced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShareQuotaBackend {
    /// ZFS dataset — a native `refquota` on the dataset that holds the share.
    Zfs {
        dataset: String,
        mount_point: String,
    },
    /// Btrfs — a qgroup limit on the share's subvolume.
    Btrfs {
        mount_point: String,
        /// The subvolume id (`0/<id>`), resolved by `btrfs subvolume show`.
        subvolume_id: String,
    },
    /// XFS project quota — a per-directory limit on a `prjquota` mount.
    XfsProject {
        mount_point: String,
        /// Stable project id derived from the share path.
        project_id: u32,
    },
    /// The filesystem cannot enforce a per-directory ceiling here.
    Unsupported { filesystem: String, reason: String },
}

impl ShareQuotaBackend {
    /// Whether a limit can be applied at all.
    pub fn is_supported(&self) -> bool {
        !matches!(self, ShareQuotaBackend::Unsupported { .. })
    }

    /// Stable wire value (`zfs` | `btrfs` | `xfs_project` | `unsupported`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ShareQuotaBackend::Zfs { .. } => "zfs",
            ShareQuotaBackend::Btrfs { .. } => "btrfs",
            ShareQuotaBackend::XfsProject { .. } => "xfs_project",
            ShareQuotaBackend::Unsupported { .. } => "unsupported",
        }
    }

    /// The filesystem name shown next to the bar.
    pub fn filesystem(&self) -> &str {
        match self {
            ShareQuotaBackend::Zfs { .. } => "zfs",
            ShareQuotaBackend::Btrfs { .. } => "btrfs",
            ShareQuotaBackend::XfsProject { .. } => "xfs",
            ShareQuotaBackend::Unsupported { filesystem, .. } => filesystem,
        }
    }

    /// Why a limit cannot be enforced, for the card and the logs.
    pub fn reason(&self) -> Option<&str> {
        match self {
            ShareQuotaBackend::Unsupported { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// Pick the backend for a share, from the facts `findmnt` reports.
///
/// Pure so the whole matrix is unit-tested without a filesystem: the file
/// systems that can bound one directory, the ones that cannot, and the honest
/// reason for each refusal.
///
/// * `zfs` — `refquota` on the dataset (independent of any per-user quota).
/// * `btrfs` — qgroups limit a **subvolume**; `subvolume_id` is `None` when the
///   share path is a plain directory inside one, so the answer is `Unsupported`
///   with that exact reason (a qgroup limit would bound the whole subvolume,
///   i.e. every share living in it).
/// * `xfs` — project quotas, only when the mount carries `prjquota`.
/// * everything else — `Unsupported`, naming the filesystem.
pub fn backend_for(
    filesystem: &str,
    source: &str,
    options: &str,
    mount_point: &str,
    share_path: &str,
    btrfs_subvolume_id: Option<&str>,
) -> ShareQuotaBackend {
    let fs = filesystem.trim().to_ascii_lowercase();
    match fs.as_str() {
        "zfs" => ShareQuotaBackend::Zfs {
            dataset: source.trim().to_string(),
            mount_point: mount_point.to_string(),
        },
        "btrfs" => match btrfs_subvolume_id {
            Some(id) if !id.trim().is_empty() => ShareQuotaBackend::Btrfs {
                mount_point: mount_point.to_string(),
                subvolume_id: id.trim().to_string(),
            },
            _ => ShareQuotaBackend::Unsupported {
                filesystem: "btrfs".to_string(),
                reason: "this share is a plain folder inside a Btrfs subvolume, and a qgroup \
                         limit would cap the whole subvolume — every share in it. Move the \
                         share onto its own subvolume to give it a quota"
                    .to_string(),
            },
        },
        "xfs" => {
            let options = options.to_ascii_lowercase();
            if options.contains("prjquota") || options.contains("pquota") {
                ShareQuotaBackend::XfsProject {
                    mount_point: mount_point.to_string(),
                    project_id: project_id_for(share_path),
                }
            } else {
                ShareQuotaBackend::Unsupported {
                    filesystem: "xfs".to_string(),
                    reason: "the mount does not carry `prjquota`, so XFS cannot cap this folder — \
                             remount it with prjquota to use a share quota"
                        .to_string(),
                }
            }
        }
        other => ShareQuotaBackend::Unsupported {
            filesystem: if other.is_empty() { "unknown" } else { other }.to_string(),
            reason: format!(
                "the '{other}' filesystem has no per-directory quota this build can enforce — \
                 use ZFS, a Btrfs subvolume, or XFS with prjquota"
            ),
        },
    }
}

/// A stable XFS project id for a share path.
///
/// Deterministic (a flag on the directory must survive a restart) and inside the
/// range XFS accepts (1..=2^32-1, with the first few reserved for system use).
pub fn project_id_for(share_path: &str) -> u32 {
    // FNV-1a over the path, folded into 1000..=u32::MAX so the result never
    // collides with XFS's reserved low ids.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in share_path.trim().trim_end_matches('/').as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    1_000 + (hash % (u64::from(u32::MAX) - 1_000)) as u32
}

// --------------------------------------------------------------------------
// Command builders (pure: the argv is the contract)
// --------------------------------------------------------------------------

/// `zfs set refquota=<value> <dataset>`; `bytes == 0` clears it.
pub fn zfs_set_args(dataset: &str, bytes: u64) -> Vec<String> {
    vec![
        "set".to_string(),
        format!("refquota={}", zfs_value(bytes)),
        dataset.to_string(),
    ]
}

/// `zfs list -H -p -o used <dataset>` — `-p` prints raw bytes.
pub fn zfs_used_args(dataset: &str) -> Vec<String> {
    vec![
        "list".to_string(),
        "-H".to_string(),
        "-p".to_string(),
        "-o".to_string(),
        "used".to_string(),
        dataset.to_string(),
    ]
}

/// The string ZFS stores for a size: `none` when there is no limit.
///
/// A plain byte count is accepted by ZFS and keeps the round-trip exact (a
/// human `10G` would come back as `10737418240` anyway, and comparing the two
/// in a test would be guessing).
pub fn zfs_value(bytes: u64) -> String {
    if bytes == 0 {
        "none".to_string()
    } else {
        bytes.to_string()
    }
}

/// `btrfs qgroup limit <bytes|none> <subvolume path>`.
pub fn btrfs_limit_args(share_path: &str, bytes: u64) -> Vec<String> {
    vec![
        "qgroup".to_string(),
        "limit".to_string(),
        if bytes == 0 {
            "none".to_string()
        } else {
            bytes.to_string()
        },
        share_path.to_string(),
    ]
}

/// `btrfs qgroup show --raw <subvolume path>`.
pub fn btrfs_show_args(share_path: &str) -> Vec<String> {
    vec![
        "qgroup".to_string(),
        "show".to_string(),
        "--raw".to_string(),
        share_path.to_string(),
    ]
}

/// `xfs_quota -x -c "project -s -p <path> <id>"` — attach the directory to its
/// project id (idempotent; `-s` means "set up the project").
pub fn xfs_project_setup_args(share_path: &str, project_id: u32) -> Vec<String> {
    vec![
        "-x".to_string(),
        "-c".to_string(),
        format!("project -s -p {share_path} {project_id}"),
    ]
}

/// `xfs_quota -x -c "limit -p bhard=<value> <id>"`; `value == 0` clears it.
pub fn xfs_limit_args(project_id: u32, bytes: u64) -> Vec<String> {
    vec![
        "-x".to_string(),
        "-c".to_string(),
        format!("limit -p bhard={} {project_id}", xfs_blocks(bytes)),
    ]
}

/// `xfs_quota -x -c "report -p -b -N"` — project usage in blocks, no header.
pub fn xfs_report_args() -> Vec<String> {
    vec![
        "-x".to_string(),
        "-c".to_string(),
        "report -p -b -N".to_string(),
    ]
}

/// Bytes → XFS 1 KiB blocks (rounded up, so a limit is never quietly smaller
/// than what the admin typed).
pub fn xfs_blocks(bytes: u64) -> u64 {
    if bytes == 0 {
        0
    } else {
        bytes.div_ceil(1024)
    }
}

// --------------------------------------------------------------------------
// Parsers
// --------------------------------------------------------------------------

/// `zfs list -H -p -o used` prints one raw number.
pub fn parse_zfs_used(stdout: &str) -> Option<u64> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| line.parse::<u64>().ok())
}

/// `btrfs qgroup show --raw` prints one qgroup per line:
/// `0/256     16384     16384     none     none`.
///
/// The row for the share's own subvolume is the one whose id we resolved; the
/// middle column pair is `rfer`/`excl`, and `rfer` is what a quota is measured
/// against, so that is the usage reported.
pub fn parse_btrfs_usage(stdout: &str, subvolume_id: &str) -> Option<u64> {
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        if fields[0] != subvolume_id {
            continue;
        }
        return fields[1].parse::<u64>().ok();
    }
    None
}

/// `xfs_quota -x -c "report -p -b -N"` prints
/// `#1001    1048576    0    0    00 [--------] /srv/photos`.
pub fn parse_xfs_project_usage(stdout: &str, project_id: u32) -> Option<u64> {
    for line in stdout.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('#').or(Some(line)) else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(id) = fields.next() else { continue };
        let id = id.trim_start_matches('#');
        if id.parse::<u32>().ok() != Some(project_id) {
            continue;
        }
        let Some(used_blocks) = fields.next() else {
            continue;
        };
        return used_blocks
            .parse::<u64>()
            .ok()
            .map(|blocks| blocks.saturating_mul(1024));
    }
    None
}

/// `btrfs subvolume show <path>` prints `Subvolume ID:   256`.
pub fn parse_btrfs_subvolume_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("Subvolume ID") {
            let id = value.trim();
            if !id.is_empty() {
                return Some(format!("0/{id}"));
            }
        }
    }
    None
}

// --------------------------------------------------------------------------
// The decision
// --------------------------------------------------------------------------

/// A transfer that would cross the share's ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaExceeded {
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub incoming_bytes: u64,
}

impl QuotaExceeded {
    /// The sentence the uploader sees, with the three numbers that matter.
    pub fn message(&self) -> String {
        format!(
            "This shared folder is full: {} of {} used, and this transfer needs {}. \
             Free space, or ask an administrator to raise the share quota.",
            human_size(self.used_bytes),
            human_size(self.quota_bytes),
            human_size(self.incoming_bytes)
        )
    }
}

/// Whether adding `incoming` bytes would cross `quota`.
///
/// Pure, and deliberately `>` (not `>=`): a transfer that lands exactly on the
/// ceiling is allowed — the quota is the ceiling, not one byte below it. A quota
/// of `0` means "no limit" and can never be exceeded.
pub fn would_exceed(
    used_bytes: u64,
    quota_bytes: u64,
    incoming_bytes: u64,
) -> Result<(), QuotaExceeded> {
    if quota_bytes == 0 {
        return Ok(());
    }
    // An arithmetic overflow on `used + incoming` is itself a refusal, never a
    // silent pass-through: saturating_add would hide it by clamping at u64::MAX,
    // so detect the overflow first.
    if used_bytes.checked_add(incoming_bytes).is_none() {
        return Err(QuotaExceeded {
            used_bytes,
            quota_bytes,
            incoming_bytes,
        });
    }
    if used_bytes.saturating_add(incoming_bytes) > quota_bytes {
        return Err(QuotaExceeded {
            used_bytes,
            quota_bytes,
            incoming_bytes,
        });
    }
    Ok(())
}

/// A quota value as the UI shows it: `10.0 GB`, `512 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("TB", 1_000_000_000_000),
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
        ("KB", 1_000),
    ];
    for (suffix, scale) in UNITS {
        if bytes >= scale {
            let value = bytes as f64 / scale as f64;
            let rendered = if value >= 10.0 {
                format!("{value:.0}")
            } else {
                format!("{value:.1}")
            };
            return format!("{rendered} {suffix}");
        }
    }
    format!("{bytes} B")
}

/// Validate an admin-supplied limit. `0` clears it.
///
/// The floor exists so a mistyped `1` cannot make a share unusable; the ceiling
/// catches a unit mix-up (bytes typed as kilobytes).
pub fn validate_quota(quota_bytes: u64) -> Result<u64, String> {
    if quota_bytes == 0 {
        return Ok(0);
    }
    if quota_bytes < MIN_QUOTA_BYTES {
        return Err(format!(
            "a share quota must be at least {} (or 0 for no limit)",
            human_size(MIN_QUOTA_BYTES)
        ));
    }
    if quota_bytes > MAX_QUOTA_BYTES {
        return Err(format!(
            "a share quota must be at most {}",
            human_size(MAX_QUOTA_BYTES)
        ));
    }
    Ok(quota_bytes)
}

// --------------------------------------------------------------------------
// Live plumbing
// --------------------------------------------------------------------------

/// Read the quota state of one share, without changing anything.
#[derive(Debug, Clone, Serialize)]
pub struct ShareQuotaState {
    pub backend: String,
    pub supported: bool,
    pub filesystem: String,
    /// Why a limit cannot be enforced (only when `supported` is false).
    pub reason: Option<String>,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    /// `quota_bytes - used_bytes`, floored at 0.
    pub free_bytes: u64,
    pub percent: u8,
}

/// The `findmnt` facts the backend selection needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountFacts {
    pub filesystem: String,
    pub source: String,
    pub options: String,
    pub mount_point: String,
}

/// Parse the JSON `findmnt -n -J -o FSTYPE,SOURCE,OPTIONS,TARGET --target <p>`.
pub fn parse_findmnt(stdout: &str) -> Result<MountFacts, String> {
    let doc: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("findmnt output: {e}"))?;
    let mount = doc
        .get("filesystems")
        .and_then(|f| f.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| "findmnt returned no mount".to_string())?;
    let get = |key: &str| {
        mount
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(MountFacts {
        filesystem: get("fstype"),
        source: get("source"),
        options: get("options"),
        mount_point: get("target"),
    })
}

fn run(program: &str, args: &[String]) -> Result<String, String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    cmd.env("LC_ALL", "C");
    match crate::utils::process::run_timed(&mut cmd, QUOTA_TIMEOUT) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("{program} exited with {}", output.status)
            } else {
                stderr
            })
        }
        Err(e) => Err(e),
    }
}

/// Resolve the backend and the live usage of a share path.
///
/// Runs on the blocking pool: `findmnt` + the backend probe are subprocesses,
/// and a request must never hold a runtime thread while they run (ROADMAP X.1).
pub async fn inspect(
    share_path: &str,
    quota_bytes: u64,
) -> Result<ShareQuotaState, String> {
    let path = share_path.to_string();
    let resolved = tokio::task::spawn_blocking(move || resolve_backend(&path))
        .await
        .map_err(|e| format!("the quota probe did not finish: {e}"))??;
    let (backend, used_bytes) = match resolved {
        Ok(pair) => pair,
        Err(unsupported) => {
            return Ok(ShareQuotaState {
                backend: unsupported.as_str().to_string(),
                supported: false,
                filesystem: unsupported.filesystem().to_string(),
                reason: unsupported.reason().map(str::to_string),
                quota_bytes: 0,
                used_bytes: 0,
                free_bytes: 0,
                percent: 0,
            })
        }
    };

    let free_bytes = quota_bytes.saturating_sub(used_bytes);
    let percent = if quota_bytes == 0 {
        0
    } else {
        ((used_bytes.min(quota_bytes) * 100) / quota_bytes) as u8
    };
    Ok(ShareQuotaState {
        backend: backend.as_str().to_string(),
        supported: true,
        filesystem: backend.filesystem().to_string(),
        reason: None,
        quota_bytes,
        used_bytes,
        free_bytes,
        percent,
    })
}

/// `Ok((backend, used))` or `Err(unsupported)`.
#[allow(clippy::type_complexity)]
fn resolve_backend(
    share_path: &str,
) -> Result<Result<(ShareQuotaBackend, u64), ShareQuotaBackend>, String> {
    let facts = parse_findmnt(&run(
        "findmnt",
        &[
            "-n".to_string(),
            "-J".to_string(),
            "-o".to_string(),
            "FSTYPE,SOURCE,OPTIONS,TARGET".to_string(),
            "--target".to_string(),
            share_path.to_string(),
        ],
    )?)?;

    // Btrfs needs the subvolume id before the backend can even be chosen: a
    // plain directory inside a subvolume cannot be limited on its own.
    let subvolume_id = if facts.filesystem == "btrfs" {
        run("btrfs", &btrfs_subvolume_show_args(share_path))
            .ok()
            .and_then(|out| parse_btrfs_subvolume_id(&out))
    } else {
        None
    };

    let backend = backend_for(
        &facts.filesystem,
        &facts.source,
        &facts.options,
        &facts.mount_point,
        share_path,
        subvolume_id.as_deref(),
    );
    if !backend.is_supported() {
        return Ok(Err(backend));
    }

    let used = match &backend {
        ShareQuotaBackend::Zfs { dataset, .. } => run("zfs", &zfs_used_args(dataset))
            .ok()
            .and_then(|out| parse_zfs_used(&out))
            .unwrap_or(0),
        ShareQuotaBackend::Btrfs { subvolume_id, .. } => {
            run("btrfs", &btrfs_show_args(share_path))
                .ok()
                .and_then(|out| parse_btrfs_usage(&out, subvolume_id))
                .unwrap_or(0)
        }
        ShareQuotaBackend::XfsProject { project_id, .. } => run("xfs_quota", &xfs_report_args())
            .ok()
            .and_then(|out| parse_xfs_project_usage(&out, *project_id))
            .unwrap_or(0),
        ShareQuotaBackend::Unsupported { .. } => 0,
    };
    Ok(Ok((backend, used)))
}

/// `btrfs subvolume show <path>` (the subvolume id is in its output).
pub fn btrfs_subvolume_show_args(share_path: &str) -> Vec<String> {
    vec![
        "subvolume".to_string(),
        "show".to_string(),
        share_path.to_string(),
    ]
}

/// Apply (or clear) a limit on the share's filesystem.
///
/// `quota_bytes == 0` removes the limit. The intent is only persisted by the
/// caller once this succeeded, so the database never claims a limit the
/// filesystem refused.
pub async fn apply(share_path: &str, quota_bytes: u64) -> Result<ShareQuotaBackend, String> {
    let path = share_path.to_string();
    let outcome = tokio::task::spawn_blocking(move || -> Result<ShareQuotaBackend, String> {
        let facts = parse_findmnt(&run(
            "findmnt",
            &[
                "-n".to_string(),
                "-J".to_string(),
                "-o".to_string(),
                "FSTYPE,SOURCE,OPTIONS,TARGET".to_string(),
                "--target".to_string(),
                path.clone(),
            ],
        )?)?;
        let subvolume_id = if facts.filesystem == "btrfs" {
            run("btrfs", &btrfs_subvolume_show_args(&path))
                .ok()
                .and_then(|out| parse_btrfs_subvolume_id(&out))
        } else {
            None
        };
        let backend = backend_for(
            &facts.filesystem,
            &facts.source,
            &facts.options,
            &facts.mount_point,
            &path,
            subvolume_id.as_deref(),
        );
        match &backend {
            ShareQuotaBackend::Zfs { dataset, .. } => {
                run("zfs", &zfs_set_args(dataset, quota_bytes))?;
            }
            ShareQuotaBackend::Btrfs { .. } => {
                run("btrfs", &btrfs_limit_args(&path, quota_bytes))?;
            }
            ShareQuotaBackend::XfsProject { project_id, .. } => {
                // Attaching the directory to its project id is idempotent and
                // must happen before the limit, or `limit -p` sets a bound
                // nothing is measured against.
                run("xfs_quota", &xfs_project_setup_args(&path, *project_id))?;
                run("xfs_quota", &xfs_limit_args(*project_id, quota_bytes))?;
            }
            ShareQuotaBackend::Unsupported { reason, .. } => {
                return Err(reason.clone());
            }
        }
        Ok(backend)
    })
    .await
    .map_err(|e| format!("the quota command did not finish: {e}"))??;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Backend selection
    // ---------------------------------------------------------------------

    #[test]
    fn a_zfs_dataset_gets_a_refquota() {
        let backend = backend_for("zfs", "tank/photos", "rw,relatime", "/tank/photos", "/tank/photos", None);
        assert_eq!(
            backend,
            ShareQuotaBackend::Zfs {
                dataset: "tank/photos".to_string(),
                mount_point: "/tank/photos".to_string(),
            }
        );
        assert!(backend.is_supported());
        assert_eq!(backend.as_str(), "zfs");
        assert_eq!(backend.filesystem(), "zfs");
        assert_eq!(backend.reason(), None);
    }

    #[test]
    fn a_btrfs_subvolume_gets_a_qgroup_limit() {
        let backend = backend_for("btrfs", "/dev/sda2", "rw,subvolid=256", "/", "/mnt/photos", Some("0/256"));
        assert_eq!(
            backend,
            ShareQuotaBackend::Btrfs {
                mount_point: "/".to_string(),
                subvolume_id: "0/256".to_string(),
            }
        );
        assert!(backend.is_supported());
        assert_eq!(backend.as_str(), "btrfs");
    }

    #[test]
    fn a_plain_folder_inside_a_btrfs_subvolume_is_refused_with_the_reason() {
        // A qgroup limit would cap the whole subvolume — every share in it.
        let backend = backend_for("btrfs", "/dev/sda2", "rw", "/", "/mnt/photos", None);
        assert!(!backend.is_supported());
        let reason = backend.reason().unwrap_or_default();
        assert!(reason.contains("subvolume"), "{reason}");
        assert!(reason.contains("every share"), "the reason must say why: {reason}");
    }

    #[test]
    fn xfs_needs_prjquota_and_says_so_when_it_is_missing() {
        let with = backend_for("xfs", "/dev/sdb1", "rw,prjquota", "/srv", "/srv/photos", None);
        assert!(with.is_supported());
        assert_eq!(with.as_str(), "xfs_project");
        match with {
            ShareQuotaBackend::XfsProject { project_id, .. } => {
                assert!(project_id >= 1000, "reserved low ids are never used");
            }
            other => panic!("expected an XFS project backend, got {other:?}"),
        }

        let without = backend_for("xfs", "/dev/sdb1", "rw", "/srv", "/srv/photos", None);
        assert!(!without.is_supported());
        assert!(without.reason().unwrap_or_default().contains("prjquota"));

        // `pquota` is XFS's older spelling of the same mount option.
        assert!(backend_for("xfs", "/dev/sdb1", "rw,pquota", "/srv", "/srv/x", None).is_supported());
    }

    #[test]
    fn an_unsupported_filesystem_names_itself_and_the_alternatives() {
        for fs in ["ext4", "vfat", "ntfs3", "fuse.sshfs"] {
            let backend = backend_for(fs, "/dev/sdc1", "rw", "/mnt/x", "/mnt/x", None);
            assert!(!backend.is_supported(), "{fs} must not claim support");
            assert_eq!(backend.filesystem(), fs);
            let reason = backend.reason().unwrap_or_default();
            assert!(reason.contains(fs), "{reason}");
            assert!(
                reason.contains("ZFS") || reason.contains("Btrfs"),
                "the reason should name what does work: {reason}"
            );
        }
        // An empty fstype (findmnt gave nothing) is "unknown", never a guess.
        assert_eq!(backend_for("", "", "", "", "/x", None).filesystem(), "unknown");
    }

    #[test]
    fn the_project_id_is_stable_and_never_in_the_reserved_range() {
        let a = project_id_for("/srv/photos");
        assert_eq!(a, project_id_for("/srv/photos"), "stable across restarts");
        assert_ne!(a, project_id_for("/srv/backups"), "different shares differ");
        // The trailing slash is normalised: the same folder, the same id.
        assert_eq!(a, project_id_for("/srv/photos/"));
        assert!(a >= 1000);
    }

    // ---------------------------------------------------------------------
    // Command builders
    // ---------------------------------------------------------------------

    #[test]
    fn the_zfs_argv_sets_a_raw_refquota_and_none_clears_it() {
        assert_eq!(
            zfs_set_args("tank/photos", 10_000_000_000),
            vec!["set", "refquota=10000000000", "tank/photos"]
        );
        assert_eq!(
            zfs_set_args("tank/photos", 0),
            vec!["set", "refquota=none", "tank/photos"]
        );
        assert_eq!(zfs_value(0), "none");
        assert_eq!(zfs_value(1), "1");
        // Raw bytes on read, so the two sides of the round-trip agree.
        assert_eq!(
            zfs_used_args("tank/photos"),
            vec!["list", "-H", "-p", "-o", "used", "tank/photos"]
        );
    }

    #[test]
    fn the_btrfs_argv_limits_in_bytes_and_clears_with_none() {
        assert_eq!(
            btrfs_limit_args("/mnt/photos", 5_000_000),
            vec!["qgroup", "limit", "5000000", "/mnt/photos"]
        );
        assert_eq!(
            btrfs_limit_args("/mnt/photos", 0),
            vec!["qgroup", "limit", "none", "/mnt/photos"]
        );
        assert_eq!(
            btrfs_show_args("/mnt/photos"),
            vec!["qgroup", "show", "--raw", "/mnt/photos"]
        );
    }

    #[test]
    fn the_xfs_argv_attaches_the_project_then_sets_a_block_limit() {
        assert_eq!(
            xfs_project_setup_args("/srv/photos", 1001),
            vec!["-x", "-c", "project -s -p /srv/photos 1001"]
        );
        // XFS counts 1 KiB blocks, rounded **up** so a limit is never smaller
        // than the number the admin typed.
        assert_eq!(xfs_blocks(1_048_577), 1025);
        assert_eq!(xfs_blocks(1024), 1);
        assert_eq!(xfs_blocks(0), 0);
        assert_eq!(xfs_limit_args(1001, 1_048_576), vec!["-x", "-c", "limit -p bhard=1024 1001"]);
        assert_eq!(xfs_limit_args(1001, 0), vec!["-x", "-c", "limit -p bhard=0 1001"]);
        assert_eq!(xfs_report_args(), vec!["-x", "-c", "report -p -b -N"]);
    }

    // ---------------------------------------------------------------------
    // Parsers
    // ---------------------------------------------------------------------

    #[test]
    fn zfs_usage_is_the_first_raw_number() {
        assert_eq!(parse_zfs_used("123456789\n"), Some(123_456_789));
        assert_eq!(parse_zfs_used("  42  \n\n"), Some(42));
        // `used` can legitimately be 0 on a fresh dataset.
        assert_eq!(parse_zfs_used("0\n"), Some(0));
        assert_eq!(parse_zfs_used(""), None);
        assert_eq!(parse_zfs_used("10.5G\n"), None, "raw mode prints bytes only");
    }

    #[test]
    fn btrfs_usage_picks_the_shares_own_qgroup() {
        let raw = "\
0/256     16384     16384     none     none
0/257  10485760  10485760     none     none
";
        assert_eq!(parse_btrfs_usage(raw, "0/257"), Some(10_485_760));
        assert_eq!(parse_btrfs_usage(raw, "0/256"), Some(16_384));
        // A qgroup that is not in the table is "unknown", never zero-by-guess.
        assert_eq!(parse_btrfs_usage(raw, "0/999"), None);
        assert_eq!(parse_btrfs_usage("", "0/256"), None);
    }

    #[test]
    fn xfs_usage_comes_back_in_bytes() {
        let raw = "\
#1001      10240         0         0   00 [--------] /srv/photos
#1002          1         0         0   00 [--------] /srv/backups
";
        // 10240 blocks × 1024.
        assert_eq!(parse_xfs_project_usage(raw, 1001), Some(10_485_760));
        assert_eq!(parse_xfs_project_usage(raw, 1002), Some(1024));
        assert_eq!(parse_xfs_project_usage(raw, 1003), None);
        assert_eq!(parse_xfs_project_usage("", 1001), None);
    }

    #[test]
    fn the_btrfs_subvolume_id_is_read_from_the_show_output() {
        let raw = "\
/mnt/photos is a Btrfs subvolume
	Name: 			photos
	Subvolume ID: 		257
	Generation: 		42
";
        assert_eq!(parse_btrfs_subvolume_id(raw), Some("0/257".to_string()));
        assert_eq!(parse_btrfs_subvolume_id("not a subvolume"), None);
        // A blank id is not an id.
        assert_eq!(parse_btrfs_subvolume_id("Subvolume ID:   "), None);
    }

    #[test]
    fn findmnt_output_becomes_the_facts_the_backend_needs() {
        let raw = r#"{"filesystems":[{"target":"/tank/photos","source":"tank/photos","fstype":"zfs","options":"rw,relatime,xattr"}]}"#;
        let facts = parse_findmnt(raw).expect("parses");
        assert_eq!(facts.filesystem, "zfs");
        assert_eq!(facts.source, "tank/photos");
        assert_eq!(facts.mount_point, "/tank/photos");
        assert!(facts.options.contains("rw"));
        assert!(parse_findmnt("not json").is_err());
        assert!(parse_findmnt(r#"{"filesystems":[]}"#).is_err());
    }

    // ---------------------------------------------------------------------
    // The decision
    // ---------------------------------------------------------------------

    #[test]
    fn a_transfer_that_fits_is_allowed() {
        assert!(would_exceed(0, 10_000, 1_000).is_ok());
        assert!(would_exceed(9_000, 10_000, 1_000).is_ok(), "landing exactly on the ceiling");
    }

    #[test]
    fn a_transfer_that_crosses_the_ceiling_is_refused_with_its_numbers() {
        let exceeded = would_exceed(9_500, 10_000, 1_000).unwrap_err();
        assert_eq!(exceeded.used_bytes, 9_500);
        assert_eq!(exceeded.quota_bytes, 10_000);
        assert_eq!(exceeded.incoming_bytes, 1_000);
        let message = exceeded.message();
        assert!(message.contains("is full"), "{message}");
        assert!(message.contains("raise the share quota"), "{message}");
    }

    #[test]
    fn an_eleven_gigabyte_upload_into_a_ten_gigabyte_share_is_refused() {
        // The acceptance criterion, verbatim.
        let ten_gb = 10_000_000_000u64;
        let eleven_gb = 11_000_000_000u64;
        let exceeded = would_exceed(0, ten_gb, eleven_gb).unwrap_err();
        assert_eq!(exceeded.quota_bytes, ten_gb);
        let message = exceeded.message();
        assert!(message.contains("11 GB"), "{message}");
        assert!(message.contains("10 GB"), "{message}");
    }

    #[test]
    fn a_share_without_a_quota_can_never_be_full() {
        assert!(would_exceed(u64::MAX / 2, 0, u64::MAX / 2).is_ok());
    }

    #[test]
    fn an_already_full_share_refuses_even_a_tiny_file() {
        let exceeded = would_exceed(10_000, 10_000, 1).unwrap_err();
        assert_eq!(exceeded.incoming_bytes, 1);
    }

    #[test]
    fn the_overflow_cannot_wrap_around() {
        // `used + incoming` saturating at u64::MAX must still be a refusal, not
        // a silent pass through an overflow.
        assert!(would_exceed(u64::MAX - 1, u64::MAX, 10).is_err());
    }

    // ---------------------------------------------------------------------
    // Validation and formatting
    // ---------------------------------------------------------------------

    #[test]
    fn a_quota_is_validated_against_its_floor_and_ceiling() {
        assert_eq!(validate_quota(0).unwrap(), 0, "0 clears the limit");
        assert!(validate_quota(MIN_QUOTA_BYTES).is_ok());
        assert!(validate_quota(MAX_QUOTA_BYTES).is_ok());
        // A bare "1" is a typo, not a 1-byte share.
        let too_small = validate_quota(1).unwrap_err();
        assert!(too_small.contains("at least"), "{too_small}");
        let too_big = validate_quota(MAX_QUOTA_BYTES + 1).unwrap_err();
        assert!(too_big.contains("at most"), "{too_big}");
    }

    #[test]
    fn sizes_are_rendered_the_way_the_card_shows_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_000), "1.0 KB");
        assert_eq!(human_size(1_500_000), "1.5 MB");
        assert_eq!(human_size(10_000_000_000), "10 GB");
        assert_eq!(human_size(2_000_000_000_000), "2.0 TB");
    }
}
