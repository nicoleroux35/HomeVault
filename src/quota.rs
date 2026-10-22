/* =============================================================================
 * File          : quota.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 22/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Per-user home-directory disk quotas, enforced on the filesystem hosting
 *   the home folder. Two real backends are supported and a third state is
 *   refused explicitly instead of being silently "applied":
 *     - VFS quota tools (ext2/ext3/ext4, XFS) — `setquota` + `repquota`,
 *       which requires the filesystem to be mounted with `usrquota`;
 *     - ZFS native `userquota@<user>` properties on the dataset.
 *   Btrfs (qgroups are subtree-based, not uid-based) and non-POSIX
 *   filesystems (vfat/exfat/ntfs) are reported as unsupported, never as
 *   "saved".
 *
 *   Every privileged call (`setquota`, `repquota`, `quotaon`, `quotacheck`,
 *   `zfs`) runs as `sudo -n <cmd>` with a deadline, against the scoped
 *   `NOPASSWD` grants installed by `src/setup.rs`; nothing is ever run bare,
 *   and nothing is reported as enforced without a read-back.
 *
 * Dependencies:
 *   - serde (>= 1.0)        — the state is returned to the API
 *   - serde_json (>= 1.0)   — `findmnt -J` parsing
 *   - thiserror (>= 2)
 *   - tokio (>= 1.0)        — non-blocking subprocesses with a deadline
 *
 * Usage:
 *   `handlers/user_homes.rs` calls `QuotaService::apply` when a home folder is
 *   saved and `QuotaService::inspect` when its state is displayed;
 *   `web/mod.rs` calls `QuotaService::activate_all` at startup so a reboot
 *   cannot silently turn enforcement off.
 * ============================================================================= */

//! Per-user home-directory disk quotas: detection, enforcement and read-back.
//!
//! The feature used to "apply" a quota with a bare `setquota` and degrade to a
//! warning on failure, without ever telling the UI whether the limit was real.
//! This module answers the only question that matters — *is the limit actually
//! enforced right now?* — and never reports success it has not verified:
//! every `apply` is followed by a `repquota` / `zfs get` read-back.

use serde::Serialize;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;

/// Deadline for a read-only quota query.
const QUERY_TIMEOUT_SECS: u64 = 10;
/// Deadline for a quota mutation (`setquota`, `zfs set`).
const APPLY_TIMEOUT_SECS: u64 = 20;
/// Deadline for `quotaon`/`quotacheck` (a first check can scan a whole disk).
const ACTIVATE_TIMEOUT_SECS: u64 = 120;

/// Errors from a quota operation.
#[derive(Error, Debug)]
pub enum QuotaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Command failed: {0}")]
    CommandFailed(String),
    #[error("Quota command timed out: {0}")]
    Timeout(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

/// How the filesystem hosting a home folder can enforce a per-user quota.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaBackend {
    /// `quota-tools` backend: `setquota`/`repquota` on a mount that carries
    /// the `usrquota` option (`ext2/3/4`, XFS).
    Vfs {
        filesystem: String,
        source: String,
        mount_point: String,
        /// The mount options contain `usrquota` (or the generic `quota`).
        accounting_on: bool,
    },
    /// ZFS dataset exposing the native `userquota@<user>` property.
    Zfs {
        dataset: String,
        mount_point: String,
    },
    /// The filesystem could use VFS quotas but the mount does not ask for
    /// them, so no limit can be enforced today.
    NotMountedWithQuota {
        filesystem: String,
        mount_point: String,
    },
    /// The filesystem cannot enforce per-user quotas at all.
    Unsupported {
        filesystem: String,
        mount_point: String,
        reason: String,
    },
}

impl QuotaBackend {
    pub fn filesystem(&self) -> &str {
        match self {
            QuotaBackend::Vfs { filesystem, .. }
            | QuotaBackend::NotMountedWithQuota { filesystem, .. }
            | QuotaBackend::Unsupported { filesystem, .. } => filesystem,
            QuotaBackend::Zfs { .. } => "zfs",
        }
    }

    pub fn mount_point(&self) -> &str {
        match self {
            QuotaBackend::Vfs { mount_point, .. }
            | QuotaBackend::Zfs { mount_point, .. }
            | QuotaBackend::NotMountedWithQuota { mount_point, .. }
            | QuotaBackend::Unsupported { mount_point, .. } => mount_point,
        }
    }
}

/// Why a quota is not enforced, as a stable machine-readable code.
///
/// The English sentence in [`QuotaState::reason`] is a diagnostic for logs and
/// for a UI fallback; this code is what the frontend localises, so the Users
/// page can say "not enforced" in the admin's language instead of printing a
/// backend string. Unknown codes are impossible on the wire (serde) but the
/// client always falls back to `reason` if it does not know the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaReason {
    /// The mount exists but was not mounted with `usrquota`/`quota`.
    NotMountedWithQuota,
    /// The filesystem has no per-user quota interface (Btrfs, vfat, …).
    UnsupportedFilesystem,
    /// Accounting is off on the mount, so the read-back failed.
    AccountingInactive,
    /// Accounting is on but this account has no row in the quota report.
    NoEntry,
    /// The filesystem reports a different limit than the one configured.
    LimitMismatch,
    /// A quota command failed (`setquota`, `quotaon`, `quotacheck`, `zfs`).
    CommandFailed,
    /// The mount hosting the home folder could not be inspected at all.
    InspectFailed,
    /// The system account name is not usable in a quota command.
    InvalidUsername,
}

/// State of one user's home quota, as displayed by the API/UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuotaState {
    /// True only when the limit (or, when no limit is set, the accounting)
    /// is active on this filesystem **and** was verified by a read-back.
    pub enforced: bool,
    /// `ext4`, `xfs`, `zfs`, `btrfs`, …
    pub filesystem: String,
    /// Mount point (or ZFS dataset) the home folder lives on.
    pub mount_point: String,
    /// Configured hard limit, in bytes (`None` = unlimited).
    pub limit_bytes: Option<i64>,
    /// Usage reported by the filesystem when accounting is active.
    pub used_bytes: Option<u64>,
    /// Localisable cause — always set when `enforced` is false.
    pub reason_code: Option<QuotaReason>,
    /// English diagnostic detail (never shown as the primary message).
    pub reason: Option<String>,
}

impl QuotaState {
    fn not_enforced(
        backend: &QuotaBackend,
        limit_bytes: Option<i64>,
        used_bytes: Option<u64>,
        reason_code: QuotaReason,
        reason: String,
    ) -> Self {
        QuotaState {
            enforced: false,
            filesystem: backend.filesystem().to_string(),
            mount_point: backend.mount_point().to_string(),
            limit_bytes,
            used_bytes,
            reason_code: Some(reason_code),
            reason: Some(reason),
        }
    }

    /// A failure that happened before a backend could even be identified.
    fn unknown_mount(
        home_path: &str,
        limit_bytes: Option<i64>,
        reason_code: QuotaReason,
        reason: String,
    ) -> Self {
        QuotaState {
            enforced: false,
            filesystem: "unknown".to_string(),
            mount_point: home_path.to_string(),
            limit_bytes,
            used_bytes: None,
            reason_code: Some(reason_code),
            reason: Some(reason),
        }
    }
}

/// A row parsed from `repquota -u`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaUsage {
    pub used_kb: u64,
    pub soft_kb: u64,
    pub hard_kb: u64,
}

pub struct QuotaService;

impl QuotaService {
    /// Convert a byte count into the 1-KiB block count that `setquota` expects.
    /// Rounds down; negative inputs clamp to 0.
    pub fn bytes_to_quota_kb(bytes: i64) -> i64 {
        bytes.max(0) / 1024
    }

    /// Human-readable size from a 1-KiB block count, `"Unlimited"` for 0.
    pub fn format_size_kb(kb: i64) -> String {
        if kb == 0 {
            return "Unlimited".to_string();
        }
        if kb < 1024 {
            return format!("{} KB", kb);
        }
        if kb < 1024 * 1024 {
            return format!("{:.1} MB", kb as f64 / 1024.0);
        }
        format!("{:.2} GB", kb as f64 / (1024.0 * 1024.0))
    }

    /// True when the filesystem type can use the VFS quota tools at all.
    /// Btrfs is deliberately absent: its qgroups are subtree-based and there
    /// is no uid/gid quota interface, so `setquota` cannot work.
    pub fn supports_vfs_quota(filesystem: &str) -> bool {
        matches!(
            filesystem,
            "ext2" | "ext3" | "ext4" | "xfs" | "jfs" | "reiserfs"
        )
    }

    /// True when the mount options ask the kernel for per-user accounting.
    /// `quota` is accepted as the generic spelling some tools emit.
    pub fn mount_has_usrquota(options: &str) -> bool {
        options
            .split(',')
            .map(str::trim)
            .any(|o| o == "usrquota" || o == "quota")
    }

    /// Mount options that make the kernel account and enforce per-user quotas
    /// on `filesystem`, or `None` when it cannot do it at all.
    ///
    /// Used when a pool/RAID array is created (`disk.rs`) and by the installer:
    /// a limit can only be enforced on a mount that was mounted with these.
    /// XFS enables accounting from the mount option itself and has no quota
    /// files, so `usrquota` alone is correct there.
    pub fn quota_mount_options(filesystem: &str) -> Option<&'static str> {
        match filesystem {
            "ext2" | "ext3" | "ext4" | "jfs" | "reiserfs" => Some("usrquota,grpquota"),
            "xfs" => Some("usrquota"),
            _ => None,
        }
    }

    /// Which backend a mount maps to. Pure so the matrix is unit-tested
    /// without a host that has quota tooling.
    pub fn backend_for(
        filesystem: &str,
        source: &str,
        options: &str,
        mount_point: &str,
    ) -> QuotaBackend {
        match filesystem {
            "zfs" => QuotaBackend::Zfs {
                dataset: source.to_string(),
                mount_point: mount_point.to_string(),
            },
            fs if Self::supports_vfs_quota(fs) => {
                if Self::mount_has_usrquota(options) {
                    QuotaBackend::Vfs {
                        filesystem: fs.to_string(),
                        source: source.to_string(),
                        mount_point: mount_point.to_string(),
                        accounting_on: true,
                    }
                } else {
                    QuotaBackend::NotMountedWithQuota {
                        filesystem: fs.to_string(),
                        mount_point: mount_point.to_string(),
                    }
                }
            }
            "btrfs" => QuotaBackend::Unsupported {
                filesystem: "btrfs".to_string(),
                mount_point: mount_point.to_string(),
                reason: "Btrfs enforces quota per subvolume (qgroups), not per user, so a \
                         per-user home limit cannot be applied"
                    .to_string(),
            },
            fs => QuotaBackend::Unsupported {
                filesystem: fs.to_string(),
                mount_point: mount_point.to_string(),
                reason: format!("The '{fs}' filesystem has no per-user disk quota support"),
            },
        }
    }

    /// `setquota` argv: soft = hard = `kb` blocks, inode limits unlimited.
    /// `kb == 0` removes the limit.
    pub fn setquota_args(system_username: &str, kb: i64, mount_point: &str) -> Vec<String> {
        vec![
            "setquota".to_string(),
            "-u".to_string(),
            system_username.to_string(),
            kb.to_string(),
            kb.to_string(),
            "0".to_string(),
            "0".to_string(),
            mount_point.to_string(),
        ]
    }

    /// Parse the row for `system_username` out of `repquota -u` output.
    /// Columns: `name status used soft hard [grace] ...`; the three figures we
    /// need sit at fixed offsets after the status column, whether or not a
    /// grace period is present.
    pub fn parse_repquota(stdout: &str, system_username: &str) -> Option<QuotaUsage> {
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 || parts[0] != system_username {
                continue;
            }
            // parts[1] is the two-character status column ("--", "+-", ...).
            if parts[1].len() > 3 {
                continue;
            }
            let used_kb = parts[2].parse::<u64>().ok()?;
            let soft_kb = parts[3].parse::<u64>().ok()?;
            let hard_kb = parts[4].parse::<u64>().ok()?;
            return Some(QuotaUsage {
                used_kb,
                soft_kb,
                hard_kb,
            });
        }
        None
    }

    /// Parse `zfs get -H -p -o value userquota@<user> <dataset>`.
    /// Returns `None` for `none`/`-` (no limit) and `Some(bytes)` otherwise.
    pub fn parse_zfs_userquota(value: &str) -> Option<u64> {
        let v = value.trim();
        if v.is_empty() || v == "none" || v == "-" {
            return None;
        }
        v.parse::<u64>().ok()
    }

    /// Parse the usage of one user out of `zfs userspace -H -p -o name,used`.
    pub fn parse_zfs_userspace(stdout: &str, system_username: &str) -> Option<u64> {
        for line in stdout.lines() {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            let used = fields.next()?.trim();
            if name == system_username {
                return used.parse::<u64>().ok();
            }
        }
        None
    }

    /// The `userquota@…` value to hand to `zfs set`: exact bytes, or `none`.
    pub fn zfs_quota_value(hard_bytes: i64) -> String {
        if hard_bytes <= 0 {
            "none".to_string()
        } else {
            hard_bytes.to_string()
        }
    }

    /// Locate the backend serving `home_path` by asking the kernel's mount
    /// table (via `findmnt -J`, which handles spaces in paths safely).
    pub async fn backend_for_path(home_path: &str) -> Result<QuotaBackend, QuotaError> {
        let out = run_command(
            "findmnt",
            &[
                "-n",
                "-J",
                "-o",
                "FSTYPE,SOURCE,OPTIONS,TARGET",
                "--target",
                home_path,
            ],
            QUERY_TIMEOUT_SECS,
        )
        .await?;
        if !out.status.success() {
            return Err(QuotaError::CommandFailed(format!(
                "findmnt: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        Self::parse_findmnt(&stdout)
    }

    /// Parse the JSON document emitted by `findmnt -J`.
    pub fn parse_findmnt(stdout: &str) -> Result<QuotaBackend, QuotaError> {
        let doc: serde_json::Value = serde_json::from_str(stdout)
            .map_err(|e| QuotaError::CommandFailed(format!("findmnt output: {e}")))?;
        let mount = doc
            .get("filesystems")
            .and_then(|f| f.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| QuotaError::CommandFailed("findmnt returned no mount".to_string()))?;
        let get = |key: &str| {
            mount
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Ok(Self::backend_for(
            &get("fstype"),
            &get("source"),
            &get("options"),
            &get("target"),
        ))
    }

    /// Read the current state without changing anything.
    pub async fn inspect(
        system_username: &str,
        home_path: &str,
        limit_bytes: Option<i64>,
    ) -> QuotaState {
        match Self::backend_for_path(home_path).await {
            Ok(backend) => Self::inspect_backend(system_username, &backend, limit_bytes).await,
            Err(e) => QuotaState::unknown_mount(
                home_path,
                limit_bytes,
                QuotaReason::InspectFailed,
                format!("Could not inspect the home filesystem: {e}"),
            ),
        }
    }

    async fn inspect_backend(
        system_username: &str,
        backend: &QuotaBackend,
        limit_bytes: Option<i64>,
    ) -> QuotaState {
        match backend {
            QuotaBackend::Zfs { dataset, .. } => {
                let quota = Self::zfs_get_userquota(system_username, dataset).await;
                let used = Self::zfs_userspace_used(system_username, dataset).await;
                match quota {
                    Ok(current) => {
                        let effective_limit = current.map(|b| b as i64);
                        // A limit is enforced when ZFS reports the limit we
                        // stored; `None`/`None` (no limit) is honest too.
                        let enforced = match (effective_limit, limit_bytes) {
                            (None, None) => true,
                            (Some(have), Some(want)) => have == want,
                            _ => false,
                        };
                        QuotaState {
                            enforced,
                            filesystem: "zfs".to_string(),
                            mount_point: dataset.clone(),
                            limit_bytes,
                            used_bytes: used,
                            reason_code: if enforced {
                                None
                            } else {
                                Some(QuotaReason::LimitMismatch)
                            },
                            reason: if enforced {
                                None
                            } else {
                                Some(format!(
                                    "ZFS reports a limit of {} instead of the configured one",
                                    effective_limit
                                        .map(|b| Self::format_size_kb(b / 1024))
                                        .unwrap_or_else(|| "no limit".to_string())
                                ))
                            },
                        }
                    }
                    Err(e) => QuotaState::not_enforced(
                        backend,
                        limit_bytes,
                        used,
                        QuotaReason::CommandFailed,
                        format!("Could not read the ZFS userquota property: {e}"),
                    ),
                }
            }
            QuotaBackend::Vfs {
                mount_point,
                filesystem,
                ..
            } => match Self::read_repquota(system_username, mount_point).await {
                Ok(Some(usage)) => {
                    let enforced = match limit_bytes {
                        Some(want) => Self::bytes_to_quota_kb(want) == usage.hard_kb as i64,
                        None => true,
                    };
                    QuotaState {
                        enforced,
                        filesystem: filesystem.clone(),
                        mount_point: mount_point.clone(),
                        limit_bytes,
                        used_bytes: Some(usage.used_kb.saturating_mul(1024)),
                        reason_code: if enforced {
                            None
                        } else {
                            Some(QuotaReason::LimitMismatch)
                        },
                        reason: if enforced {
                            None
                        } else {
                            Some(format!(
                                "The filesystem reports a limit of {}, not the configured one",
                                Self::format_size_kb(usage.hard_kb as i64)
                            ))
                        },
                    }
                }
                Ok(None) => QuotaState::not_enforced(
                    backend,
                    limit_bytes,
                    None,
                    QuotaReason::NoEntry,
                    "Quota accounting is on but this account has no entry in the quota report"
                        .to_string(),
                ),
                Err(e) => QuotaState::not_enforced(
                    backend,
                    limit_bytes,
                    None,
                    QuotaReason::AccountingInactive,
                    format!("Quota accounting is not active on this mount: {e}"),
                ),
            },
            QuotaBackend::NotMountedWithQuota {
                filesystem,
                mount_point,
            } => QuotaState::not_enforced(
                backend,
                limit_bytes,
                None,
                QuotaReason::NotMountedWithQuota,
                format!(
                    "The {filesystem} filesystem mounted on {mount_point} was not mounted with \
                     'usrquota', so the kernel cannot enforce a per-user limit"
                ),
            ),
            QuotaBackend::Unsupported { reason, .. } => QuotaState::not_enforced(
                backend,
                limit_bytes,
                None,
                QuotaReason::UnsupportedFilesystem,
                reason.clone(),
            ),
        }
    }

    /// Apply (or clear) the hard limit and verify it by reading it back.
    /// Never returns `enforced: true` without that read-back.
    pub async fn apply(
        system_username: &str,
        home_path: &str,
        limit_bytes: Option<i64>,
    ) -> QuotaState {
        if !valid_system_username(system_username) {
            return QuotaState::unknown_mount(
                home_path,
                limit_bytes,
                QuotaReason::InvalidUsername,
                "Invalid system user name".to_string(),
            );
        }
        let backend = match Self::backend_for_path(home_path).await {
            Ok(b) => b,
            Err(e) => {
                return QuotaState::unknown_mount(
                    home_path,
                    limit_bytes,
                    QuotaReason::InspectFailed,
                    format!("Could not inspect the home filesystem: {e}"),
                )
            }
        };

        match &backend {
            QuotaBackend::Zfs { dataset, .. } => {
                let value = limit_bytes
                    .filter(|b| *b > 0)
                    .map(Self::zfs_quota_value)
                    .unwrap_or_else(|| "none".to_string());
                let prop = format!("userquota@{system_username}={value}");
                if let Err(e) = Self::zfs_set_userquota(dataset, &prop).await {
                    return QuotaState::not_enforced(
                        &backend,
                        limit_bytes,
                        Self::zfs_userspace_used(system_username, dataset).await,
                        QuotaReason::CommandFailed,
                        format!("ZFS refused the userquota update: {e}"),
                    );
                }
            }
            QuotaBackend::Vfs {
                mount_point,
                filesystem,
                ..
            } => {
                // Accounting may be on in fstab but not activated since the
                // last boot: activate it (idempotent) before writing.
                if let Err(e) = Self::activate_mount(filesystem, mount_point).await {
                    return QuotaState::not_enforced(
                        &backend,
                        limit_bytes,
                        None,
                        QuotaReason::AccountingInactive,
                        format!("Could not activate quota accounting: {e}"),
                    );
                }
                let kb = limit_bytes
                    .filter(|b| *b > 0)
                    .map(Self::bytes_to_quota_kb)
                    .unwrap_or(0);
                let args = Self::setquota_args(system_username, kb, mount_point);
                if let Err(e) = Self::run_setquota(&args).await {
                    return QuotaState::not_enforced(
                        &backend,
                        limit_bytes,
                        None,
                        QuotaReason::CommandFailed,
                        format!("setquota failed: {e}"),
                    );
                }
            }
            // Nothing to apply: report the same reason `inspect` would, so the
            // caller can tell the admin the limit is recorded but not enforced.
            QuotaBackend::NotMountedWithQuota { .. } | QuotaBackend::Unsupported { .. } => {
                return Self::inspect_backend(system_username, &backend, limit_bytes).await
            }
        }

        Self::inspect_backend(system_username, &backend, limit_bytes).await
    }

    /// Re-activate VFS accounting on every mount listed in `/etc/fstab`
    /// (`quotaon -aug`), once at startup. Without this a reboot silently
    /// turns enforcement off even though the limit is still stored.
    pub async fn activate_all() {
        match run_privileged("quotaon", &["-aug"], ACTIVATE_TIMEOUT_SECS).await {
            Ok(out) if out.status.success() => {
                tracing::info!("Filesystem quota accounting activated (quotaon -aug)");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                // Exit code 2 with "no quota" style output is the normal case
                // on a host without any quota-enabled mount.
                tracing::debug!("quotaon -aug did not enable anything: {}", stderr.trim());
            }
            Err(e) => tracing::debug!("quotaon -aug unavailable: {}", e),
        }
    }

    /// Create/refresh the quota files then start accounting on one mount.
    /// Called right after a pool/RAID array is mounted with
    /// `quota_mount_options`, so a fresh array enforces from the first byte
    /// written instead of only after the first home-folder save.
    pub async fn activate_mount(filesystem: &str, mount_point: &str) -> Result<(), QuotaError> {
        // XFS has no quota files: accounting is enabled by the mount option
        // alone and `quotaon` is a no-op/error there.
        if filesystem == "xfs" {
            return Ok(());
        }
        let on = run_privileged("quotaon", &["-u", mount_point], QUERY_TIMEOUT_SECS).await?;
        if on.status.success() {
            return Ok(());
        }
        // First use on this mount: build the quota files from current usage,
        // then retry. `-f` is required on a mounted filesystem.
        let check = run_privileged(
            "quotacheck",
            &["-cugm", "-f", mount_point],
            ACTIVATE_TIMEOUT_SECS,
        )
        .await?;
        if !check.status.success() {
            return Err(QuotaError::CommandFailed(format!(
                "quotacheck: {}",
                String::from_utf8_lossy(&check.stderr).trim()
            )));
        }
        let on = run_privileged("quotaon", &["-u", mount_point], QUERY_TIMEOUT_SECS).await?;
        if !on.status.success() {
            return Err(QuotaError::CommandFailed(format!(
                "quotaon: {}",
                String::from_utf8_lossy(&on.stderr).trim()
            )));
        }
        Ok(())
    }

    async fn read_repquota(
        system_username: &str,
        mount_point: &str,
    ) -> Result<Option<QuotaUsage>, QuotaError> {
        let out = run_privileged("repquota", &["-u", mount_point], QUERY_TIMEOUT_SECS).await?;
        if !out.status.success() {
            return Err(QuotaError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(Self::parse_repquota(
            &String::from_utf8_lossy(&out.stdout),
            system_username,
        ))
    }

    async fn run_setquota(args: &[String]) -> Result<(), QuotaError> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_privileged("setquota", &args, APPLY_TIMEOUT_SECS).await?;
        if !out.status.success() {
            return Err(QuotaError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    async fn zfs_get_userquota(
        system_username: &str,
        dataset: &str,
    ) -> Result<Option<u64>, QuotaError> {
        let prop = format!("userquota@{system_username}");
        let out = run_privileged(
            "zfs",
            &["get", "-H", "-p", "-o", "value", &prop, dataset],
            QUERY_TIMEOUT_SECS,
        )
        .await?;
        if !out.status.success() {
            return Err(QuotaError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(Self::parse_zfs_userquota(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }

    async fn zfs_userspace_used(system_username: &str, dataset: &str) -> Option<u64> {
        let out = run_privileged(
            "zfs",
            &["userspace", "-H", "-p", "-o", "name,used", dataset],
            QUERY_TIMEOUT_SECS,
        )
        .await
        .ok()?;
        if !out.status.success() {
            return None;
        }
        Self::parse_zfs_userspace(&String::from_utf8_lossy(&out.stdout), system_username)
    }

    async fn zfs_set_userquota(dataset: &str, prop: &str) -> Result<(), QuotaError> {
        let out = run_privileged("zfs", &["set", prop, dataset], APPLY_TIMEOUT_SECS).await?;
        if !out.status.success() {
            return Err(QuotaError::CommandFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }
}

/// System account names are validated once more before they reach an argv:
/// no `@`/`=`/`,` that could forge a ZFS property, and no whitespace.
fn valid_system_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// argv for a privileged quota command: `sudo -n <program> <args…>`.
///
/// `-n` is deliberate: without it `sudo` blocks on a password prompt and only
/// the deadline would end the wait. The scoped `NOPASSWD` grants are installed
/// by `src/setup.rs`, so `-n` either runs or fails immediately and loudly.
pub fn privileged_argv(program: &str, args: &[&str]) -> Vec<String> {
    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push("-n".to_string());
    argv.push(program.to_string());
    argv.extend(args.iter().map(|a| a.to_string()));
    argv
}

/// Run a privileged quota command through `sudo -n` with a deadline.
async fn run_privileged(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output, QuotaError> {
    let argv = privileged_argv(program, args);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    run_command("sudo", &argv, timeout_secs).await
}

/// Run one command with a deadline, capturing stdout/stderr.
async fn run_command(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output, QuotaError> {
    use tokio::process::Command;

    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(QuotaError::Io)?;

    match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(QuotaError::Io(e)),
        Err(_) => Err(QuotaError::Timeout(format!(
            "{program} did not answer within {timeout_secs}s"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_quota_kb_rounds_down() {
        assert_eq!(QuotaService::bytes_to_quota_kb(0), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(1), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(1023), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(1024), 1);
        assert_eq!(QuotaService::bytes_to_quota_kb(1536), 1);
        assert_eq!(
            QuotaService::bytes_to_quota_kb(10 * 1024 * 1024 * 1024),
            10 * 1024 * 1024
        );
    }

    #[test]
    fn test_bytes_to_quota_kb_clamps_negative() {
        assert_eq!(QuotaService::bytes_to_quota_kb(-1), 0);
        assert_eq!(QuotaService::bytes_to_quota_kb(-99_999), 0);
    }

    #[test]
    fn test_format_size_kb_zero_is_unlimited() {
        assert_eq!(QuotaService::format_size_kb(0), "Unlimited");
    }

    #[test]
    fn test_format_size_kb_below_one_mb() {
        assert_eq!(QuotaService::format_size_kb(1), "1 KB");
        assert_eq!(QuotaService::format_size_kb(512), "512 KB");
        assert_eq!(QuotaService::format_size_kb(1023), "1023 KB");
    }

    #[test]
    fn test_format_size_kb_in_mb_range() {
        assert_eq!(QuotaService::format_size_kb(1024), "1.0 MB");
        assert_eq!(QuotaService::format_size_kb(1536), "1.5 MB");
        assert_eq!(QuotaService::format_size_kb(1048575), "1024.0 MB");
    }

    #[test]
    fn test_format_size_kb_in_gb_range() {
        assert_eq!(QuotaService::format_size_kb(1048576), "1.00 GB");
        assert_eq!(QuotaService::format_size_kb(2097152), "2.00 GB");
        assert_eq!(QuotaService::format_size_kb(1572864), "1.50 GB");
    }

    #[test]
    fn test_quota_error_display() {
        let err = QuotaError::CommandFailed("setquota: not found".to_string());
        assert!(err.to_string().contains("setquota"));
    }

    #[test]
    fn test_vfs_support_matrix() {
        for fs in ["ext2", "ext3", "ext4", "xfs", "jfs", "reiserfs"] {
            assert!(
                QuotaService::supports_vfs_quota(fs),
                "{fs} must use VFS quotas"
            );
        }
        // Btrfs has no uid quota interface; those filesystems have none either.
        for fs in ["btrfs", "vfat", "exfat", "ntfs", "squashfs", "zfs", ""] {
            assert!(
                !QuotaService::supports_vfs_quota(fs),
                "{fs} must not be treated as a VFS-quota filesystem"
            );
        }
    }

    #[test]
    fn test_mount_has_usrquota() {
        assert!(QuotaService::mount_has_usrquota(
            "rw,relatime,usrquota,grpquota"
        ));
        assert!(QuotaService::mount_has_usrquota("rw,quota"));
        assert!(QuotaService::mount_has_usrquota("usrquota"));
        assert!(!QuotaService::mount_has_usrquota(
            "rw,relatime,errors=remount-ro"
        ));
        // A `noquota` token must not be read as `quota`.
        assert!(!QuotaService::mount_has_usrquota("rw,noquota"));
    }

    #[test]
    fn test_backend_matrix() {
        assert!(matches!(
            QuotaService::backend_for("ext4", "/dev/sda2", "rw,usrquota", "/"),
            QuotaBackend::Vfs {
                accounting_on: true,
                ..
            }
        ));
        assert!(matches!(
            QuotaService::backend_for("ext4", "/dev/sda2", "rw,relatime", "/"),
            QuotaBackend::NotMountedWithQuota { .. }
        ));
        assert!(matches!(
            QuotaService::backend_for("zfs", "tank/home", "rw", "/mnt/pools/tank"),
            QuotaBackend::Zfs { .. }
        ));
        match QuotaService::backend_for("btrfs", "/dev/sdb1", "rw", "/mnt/pools/data") {
            QuotaBackend::Unsupported { reason, .. } => {
                assert!(reason.contains("qgroups"), "reason must say why: {reason}")
            }
            other => panic!("btrfs must be unsupported, got {other:?}"),
        }
        match QuotaService::backend_for("exfat", "/dev/sdc1", "rw", "/mnt/usb") {
            QuotaBackend::Unsupported { reason, .. } => {
                assert!(
                    reason.contains("exfat"),
                    "reason must name the fs: {reason}"
                )
            }
            other => panic!("exfat must be unsupported, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_findmnt_json() {
        let json = r#"{"filesystems":[{"target":"/","source":"/dev/nvme0n1p2","fstype":"ext4","options":"rw,relatime,errors=remount-ro"}]}"#;
        match QuotaService::parse_findmnt(json).expect("valid findmnt JSON") {
            QuotaBackend::NotMountedWithQuota {
                filesystem,
                mount_point,
            } => {
                assert_eq!(filesystem, "ext4");
                assert_eq!(mount_point, "/");
            }
            other => panic!("expected NotMountedWithQuota, got {other:?}"),
        }
        // A mount whose path contains a space survives because JSON quotes it.
        let json = r#"{"filesystems":[{"target":"/mnt/my pool","source":"/dev/sdb1","fstype":"xfs","options":"rw,usrquota"}]}"#;
        match QuotaService::parse_findmnt(json).expect("valid findmnt JSON") {
            QuotaBackend::Vfs { mount_point, .. } => assert_eq!(mount_point, "/mnt/my pool"),
            other => panic!("expected Vfs, got {other:?}"),
        }
        assert!(QuotaService::parse_findmnt("not json").is_err());
        assert!(QuotaService::parse_findmnt(r#"{"filesystems":[]}"#).is_err());
    }

    #[test]
    fn test_setquota_args() {
        let args = QuotaService::setquota_args("alice", 1024, "/home");
        assert_eq!(
            args,
            ["setquota", "-u", "alice", "1024", "1024", "0", "0", "/home"]
        );
        let cleared = QuotaService::setquota_args("alice", 0, "/mnt/raid/data");
        assert_eq!(cleared[3], "0");
        assert_eq!(cleared[4], "0");
    }

    #[test]
    fn test_privileged_argv_uses_non_interactive_sudo() {
        // Every privileged quota command must go through `sudo -n` so a missing
        // NOPASSWD grant fails immediately instead of blocking on a prompt.
        assert_eq!(
            privileged_argv(
                "setquota",
                &["-u", "alice", "1024", "1024", "0", "0", "/home"]
            ),
            ["-n", "setquota", "-u", "alice", "1024", "1024", "0", "0", "/home"]
        );
        assert_eq!(
            privileged_argv("quotaon", &["-aug"]),
            ["-n", "quotaon", "-aug"]
        );
        assert_eq!(privileged_argv("repquota", &[]), ["-n", "repquota"]);
    }

    #[test]
    fn test_parse_repquota_row() {
        // Real `repquota -u /` shape: name, 2-char status, used, soft, hard,
        // then file limits.
        let out = "*** Report for user quotas on device /dev/sda2\n\
Block grace time: 7days; Inode grace time: 7days\n\
                        Block limits                File limits\n\
User            used    soft    hard  grace    used  soft  hard  grace\n\
----------------------------------------------------------------------\n\
root      -- 123456     0       0            123    0     0\n\
alice     --  20480  10485760 10485760          10    0     0\n";
        let usage = QuotaService::parse_repquota(out, "alice").expect("alice's row");
        assert_eq!(
            usage,
            QuotaUsage {
                used_kb: 20480,
                soft_kb: 10485760,
                hard_kb: 10485760
            }
        );
        assert_eq!(usage.used_kb * 1024, 20_971_520);
        assert!(QuotaService::parse_repquota(out, "bob").is_none());
        // A header-free/malformed line must not be mistaken for data.
        assert!(QuotaService::parse_repquota("alice -- notanumber 0 0", "alice").is_none());
    }

    #[test]
    fn test_parse_repquota_with_grace_column() {
        // When a grace period is active `repquota` inserts a `7days` token
        // before the file-limits columns; the block figures do not move.
        let out = "alice     +-  20480  10485760 10485760  7days  10    0     0\n";
        let usage = QuotaService::parse_repquota(out, "alice").expect("alice's row");
        assert_eq!(usage.hard_kb, 10485760);
        assert_eq!(usage.used_kb, 20480);
    }

    #[test]
    fn test_parse_zfs_values() {
        assert_eq!(QuotaService::parse_zfs_userquota("none\n"), None);
        assert_eq!(QuotaService::parse_zfs_userquota("-\n"), None);
        assert_eq!(
            QuotaService::parse_zfs_userquota("10485760\n"),
            Some(10485760)
        );
        assert_eq!(QuotaService::parse_zfs_userquota("garbage"), None);

        let userspace = "root\t1234\nalice\t20971520\nbob\t0\n";
        assert_eq!(
            QuotaService::parse_zfs_userspace(userspace, "alice"),
            Some(20_971_520)
        );
        assert_eq!(QuotaService::parse_zfs_userspace(userspace, "carol"), None);
        assert_eq!(QuotaService::parse_zfs_userspace("", "alice"), None);
    }

    #[test]
    fn test_zfs_quota_value() {
        assert_eq!(QuotaService::zfs_quota_value(0), "none");
        assert_eq!(QuotaService::zfs_quota_value(-5), "none");
        assert_eq!(QuotaService::zfs_quota_value(1024), "1024");
    }

    #[test]
    fn test_system_username_validation() {
        assert!(valid_system_username("alice"));
        assert!(valid_system_username("john.doe-2"));
        assert!(!valid_system_username(""));
        assert!(!valid_system_username("alice@tank"));
        assert!(!valid_system_username("alice=x"));
        assert!(!valid_system_username("alice bob"));
        assert!(!valid_system_username("alice,grp"));
        assert!(!valid_system_username(&"a".repeat(65)));
    }

    #[test]
    fn test_not_enforced_state_is_explicit() {
        let backend = QuotaService::backend_for("btrfs", "/dev/sdb1", "rw", "/mnt/pools/data");
        let state = QuotaState::not_enforced(
            &backend,
            Some(1024),
            None,
            QuotaReason::UnsupportedFilesystem,
            "nope".to_string(),
        );
        assert!(!state.enforced);
        assert!(state.reason.is_some());
        assert_eq!(state.reason_code, Some(QuotaReason::UnsupportedFilesystem));
        assert_eq!(state.limit_bytes, Some(1024));
        assert_eq!(state.filesystem, "btrfs");
    }

    #[test]
    fn test_quota_state_serialises_reason_code() {
        // The frontend switches on `reason_code`; it must be snake_case and
        // survive serde as a plain string, not a Rust enum debug name.
        let state = QuotaState::not_enforced(
            &QuotaService::backend_for("ext4", "/dev/sda2", "rw,relatime", "/"),
            Some(2048),
            None,
            QuotaReason::NotMountedWithQuota,
            "mount lacks usrquota".to_string(),
        );
        let json = serde_json::to_value(&state).expect("QuotaState serialises");
        assert_eq!(json["enforced"], false);
        assert_eq!(json["reason_code"], "not_mounted_with_quota");
        assert_eq!(json["filesystem"], "ext4");
        assert_eq!(json["mount_point"], "/");
        assert_eq!(json["limit_bytes"], 2048);
        assert!(json["used_bytes"].is_null());
        assert_eq!(json["reason"], "mount lacks usrquota");

        // Every variant must have a distinct, stable wire spelling.
        let codes: Vec<String> = [
            QuotaReason::NotMountedWithQuota,
            QuotaReason::UnsupportedFilesystem,
            QuotaReason::AccountingInactive,
            QuotaReason::NoEntry,
            QuotaReason::LimitMismatch,
            QuotaReason::CommandFailed,
            QuotaReason::InspectFailed,
            QuotaReason::InvalidUsername,
        ]
        .iter()
        .map(|c| {
            serde_json::to_value(c)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
        assert_eq!(
            codes,
            [
                "not_mounted_with_quota",
                "unsupported_filesystem",
                "accounting_inactive",
                "no_entry",
                "limit_mismatch",
                "command_failed",
                "inspect_failed",
                "invalid_username",
            ]
        );
    }
}
