/* =============================================================================
 * File          : recovery.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Boot-time storage recovery: assemble mdadm arrays, rescan Btrfs members,
 *   replay every HomeVault-managed fstab entry that did not mount, and fall
 *   back to a read-only mount when the filesystem refuses a normal one.
 *   Also owns the clean-shutdown marker used to decide whether the next boot
 *   should schedule a consistency check.
 *
 * Dependencies:
 *   - tokio, serde, chrono, crate::pools, crate::scrub
 *
 * Usage:
 *   Called once from `web::run` before the share services start; see
 *   docs/guides/storage.md → "Boot recovery".
 * =============================================================================
 */

//! Boot-time storage recovery and the clean-shutdown marker.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

use crate::pools::{PoolService, PoolType};
use crate::scrub::{self, ScrubKind};
use crate::storage::atomic::AtomicWriter;
use crate::utils::process::run_timed_async_result;

/// One recovery command (mdadm assemble, btrfs rescue, e2fsck, mount, …).
const RECOVERY_CMD_TIMEOUT: Duration = Duration::from_secs(30);

/// A scrub started after an unclean shutdown waits a little: the box has just
/// booted, services are coming up, and a full-disk scrub would compete with
/// both. It still runs unattended.
const UNCLEAN_SCRUB_DELAY: Duration = Duration::from_secs(120);

/// A HomeVault-managed `/etc/fstab` entry (the marker line is kept so the
/// report can name which feature owns it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FstabEntry {
    pub marker: String,
    pub source: String,
    pub mount_point: String,
    pub fstype: String,
    pub options: String,
}

/// The subset of an fstab option string that `mount(8)` understands.
///
/// `x-systemd.*` options are consumed by the systemd fstab generator, not by
/// the mount syscall. Replaying the raw string after a failed boot would make
/// the manual retry fail for the wrong reason, so they are removed; an empty
/// result falls back to `defaults`.
pub fn manual_mount_options(options: &str) -> String {
    let kept: Vec<&str> = options
        .split(',')
        .map(str::trim)
        .filter(|option| !option.is_empty() && !option.starts_with("x-systemd."))
        .collect();
    if kept.is_empty() {
        "defaults".to_string()
    } else {
        kept.join(",")
    }
}

/// What happened to one entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryReport {
    pub mount_point: String,
    pub source: String,
    pub fstype: String,
    /// `already-mounted`, `mounted`, `mounted-ro`, `failed`.
    pub status: String,
    pub error: Option<String>,
}

impl EntryReport {
    pub fn is_failure(&self) -> bool {
        self.status == "failed"
    }
}

/// The persisted outcome of one boot-recovery pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub started_at: String,
    pub finished_at: String,
    pub unclean_boot: bool,
    pub entries: Vec<EntryReport>,
    pub errors: Vec<String>,
}

impl RecoveryReport {
    pub fn failed_entries(&self) -> impl Iterator<Item = &EntryReport> {
        self.entries.iter().filter(|entry| entry.is_failure())
    }
}

/// Whether the previous shutdown was clean, and whether this is a first boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootState {
    pub first_boot: bool,
    pub unclean: bool,
}

/// Directory holding the boot markers (`data/state`).
pub fn state_dir() -> PathBuf {
    crate::config::paths::data_dir().join("state")
}

fn clean_marker(dir: &Path) -> PathBuf {
    dir.join("clean-shutdown")
}

fn initialized_marker(dir: &Path) -> PathBuf {
    dir.join("initialized")
}

/// Read the markers at startup and reset them for the next shutdown.
///
/// A missing `clean-shutdown` is only an *unclean* boot when the appliance has
/// already completed a first boot; otherwise every fresh install would look
/// like a crash and trigger a scrub of empty pools.
pub fn begin_boot() -> BootState {
    begin_boot_in(&state_dir())
}

pub fn begin_boot_in(dir: &Path) -> BootState {
    let _ = std::fs::create_dir_all(dir);
    let clean = clean_marker(dir);
    let initialized = initialized_marker(dir);
    let first_boot = !initialized.exists();
    let unclean = !first_boot && !clean.exists();

    if clean.exists() {
        let _ = std::fs::remove_file(&clean);
    }
    let _ = AtomicWriter::write(&initialized, "1");

    BootState {
        first_boot,
        unclean,
    }
}

/// Write the clean-shutdown marker. Called from the graceful shutdown path
/// once every child and FUSE mount has been stopped.
pub fn mark_clean_shutdown() -> std::io::Result<()> {
    mark_clean_shutdown_in(&state_dir())
}

pub fn mark_clean_shutdown_in(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    AtomicWriter::write(clean_marker(dir), "1").map_err(std::io::Error::other)
}

/// Parse every HomeVault-managed fstab entry. The marker is the comment line
/// immediately above the entry (`# homevault-raid:…`, `# homevault-pool:…`,
/// `# homevault-pool-snapshots:…`, `# homevault-encrypted:…`).
pub fn parse_homevault_fstab(text: &str) -> Vec<FstabEntry> {
    let mut entries = Vec::new();
    let mut pending_marker: Option<String> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            let is_homevault = line.starts_with("# homevault-raid:")
                || line.starts_with("# homevault-pool:")
                || line.starts_with("# homevault-pool-snapshots:")
                || line.starts_with("# homevault-encrypted:");
            pending_marker = is_homevault.then(|| line.to_string());
            continue;
        }

        let Some(marker) = pending_marker.take() else {
            continue;
        };
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        entries.push(FstabEntry {
            marker,
            source: fields[0].to_string(),
            mount_point: fields[1].to_string(),
            fstype: fields[2].to_string(),
            options: fields[3].to_string(),
        });
    }
    entries
}

/// Whether `mount_point` already appears in a `/proc/mounts` body.
pub fn is_mount_point_mounted(mount_point: &str, mounts: &str) -> bool {
    let wanted = unescape_mount_field(mount_point);
    mounts.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _source = fields.next();
        fields
            .next()
            .map(|field| unescape_mount_field(field) == wanted)
            .unwrap_or(false)
    })
}

/// `/proc/mounts` escapes spaces and a few other bytes as `\040`, `\011`, …
fn unescape_mount_field(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let octal = &value[i + 1..i + 4];
            if let Ok(byte) = u8::from_str_radix(octal, 8) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `dumpe2fs -h` says "Filesystem state: not clean" after an unclean
/// unmount. Anything else (missing output, "clean", "unknown") is not a
/// reason to run e2fsck automatically.
pub fn ext4_is_unclean(dumpe2fs_output: &str) -> bool {
    dumpe2fs_output.lines().any(|line| {
        line.to_ascii_lowercase().contains("filesystem state:")
            && line.to_ascii_lowercase().contains("not clean")
    })
}

/// The recovery strategy for one entry, in the order the mount attempts are
/// made. Pure, so the ladder is testable without touching a device.
pub fn mount_fallbacks(fstype: &str) -> Vec<&'static str> {
    match fstype {
        "btrfs" => vec!["rescue-then-ro"],
        "ext2" | "ext3" | "ext4" => vec!["fsck-then-normal", "ro-noload", "ro"],
        _ => vec!["ro"],
    }
}

/// Run the full recovery pass.
pub async fn run(unclean_boot: bool) -> RecoveryReport {
    let started_at = Utc::now().to_rfc3339();
    let mut errors = Vec::new();

    // Assemble arrays and rescan Btrfs members before trying fstab entries.
    // Both are best-effort: a missing tool (a host with no mdadm/Btrfs) must
    // not turn every boot into a warning.
    if command_available("mdadm") {
        if let Err(e) = run_sudo(&["mdadm", "--assemble", "--scan"]).await {
            tracing::debug!("mdadm --assemble --scan: {e}");
        }
    }
    if command_available("btrfs") {
        if let Err(e) = run_sudo(&["btrfs", "device", "scan"]).await {
            tracing::debug!("btrfs device scan: {e}");
        }
    }

    let fstab = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let entries = parse_homevault_fstab(&fstab);
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut reports = Vec::new();

    for entry in entries {
        if is_mount_point_mounted(&entry.mount_point, &mounts) {
            reports.push(EntryReport {
                mount_point: entry.mount_point,
                source: entry.source,
                fstype: entry.fstype,
                status: "already-mounted".to_string(),
                error: None,
            });
            continue;
        }

        match recover_one(&entry).await {
            Ok(status) => {
                if status == "mounted-ro" {
                    errors.push(format!(
                        "{} was mounted read-only after recovery",
                        entry.mount_point
                    ));
                }
                reports.push(EntryReport {
                    mount_point: entry.mount_point,
                    source: entry.source,
                    fstype: entry.fstype,
                    status: status.to_string(),
                    error: None,
                });
            }
            Err(e) => {
                errors.push(format!("{}: {e}", entry.mount_point));
                reports.push(EntryReport {
                    mount_point: entry.mount_point,
                    source: entry.source,
                    fstype: entry.fstype,
                    status: "failed".to_string(),
                    error: Some(e),
                });
            }
        }
    }

    let report = RecoveryReport {
        started_at,
        finished_at: Utc::now().to_rfc3339(),
        unclean_boot,
        entries: reports,
        errors,
    };

    persist_report(&report);
    if !report.errors.is_empty() {
        crate::events::log_event_warn(format!(
            "Storage recovery finished with {} issue(s): {}",
            report.errors.len(),
            report.errors.join("; ")
        ));
    } else if !report.entries.is_empty() {
        crate::events::log_event(format!(
            "Storage recovery checked {} entrie(s), no issue",
            report.entries.len()
        ));
    }
    report
}

/// Recover one fstab entry: normal mount, then the filesystem-specific
/// fallback ladder. Public so the opt-in `dm-flakey` acceptance test can drive
/// the exact production path without rewriting `/etc/fstab`.
pub async fn recover_one(entry: &FstabEntry) -> Result<&'static str, String> {
    if !entry.mount_point.starts_with('/') {
        return Err("mount point is not absolute".to_string());
    }

    // First attempt: let mount replay the fstab options exactly, minus the
    // systemd-only options `mount(8)` rejects.
    let options = manual_mount_options(&entry.options);
    let normal = run_sudo(&["mount", "-o", &options, &entry.source, &entry.mount_point]).await;
    if normal.is_ok() {
        return Ok("mounted");
    }
    let normal_error = normal.err().unwrap_or_default();

    for step in mount_fallbacks(&entry.fstype) {
        match step {
            "rescue-then-ro" => {
                let _ = run_sudo(&["btrfs", "rescue", "super-recover", "-y", &entry.source]).await;
                if run_sudo(&["mount", "-o", "ro", &entry.source, &entry.mount_point])
                    .await
                    .is_ok()
                {
                    return Ok("mounted-ro");
                }
            }
            "fsck-then-normal" => {
                if let Ok(output) = run_sudo(&["dumpe2fs", "-h", &entry.source]).await {
                    let text = String::from_utf8_lossy(&output.stdout);
                    if ext4_is_unclean(&text) {
                        let _ = run_sudo(&["e2fsck", "-p", &entry.source]).await;
                    }
                }
                if run_sudo(&["mount", "-o", &options, &entry.source, &entry.mount_point])
                    .await
                    .is_ok()
                {
                    return Ok("mounted");
                }
            }
            "ro-noload" => {
                if run_sudo(&[
                    "mount",
                    "-o",
                    "ro,noload",
                    &entry.source,
                    &entry.mount_point,
                ])
                .await
                .is_ok()
                {
                    return Ok("mounted-ro");
                }
            }
            "ro" => {
                if run_sudo(&["mount", "-o", "ro", &entry.source, &entry.mount_point])
                    .await
                    .is_ok()
                {
                    return Ok("mounted-ro");
                }
            }
            _ => {}
        }
    }

    Err(normal_error)
}

async fn run_sudo(args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("sudo");
    cmd.args(args);
    let output = run_timed_async_result(&mut cmd, RECOVERY_CMD_TIMEOUT)
        .await
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output)
}

/// Whether a program is on `PATH`; used to skip the mdadm/Btrfs pre-pass on a
/// host that does not have those tools instead of logging a failure on every
/// boot.
fn command_available(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn persist_report(report: &RecoveryReport) {
    let path = state_dir().join("storage-recovery.json");
    let Ok(json) = serde_json::to_string_pretty(report) else {
        return;
    };
    let _ = std::fs::create_dir_all(state_dir());
    if let Err(e) = AtomicWriter::write(&path, &json) {
        tracing::warn!("Could not persist the storage recovery report: {e}");
    }
}

/// After an unclean shutdown, scrub every mounted, healthy pool once. It runs
/// in the background after a short delay, one pool at a time, through the
/// same [`crate::scrub`] lock as a scheduled scrub.
pub async fn scrub_after_unclean_shutdown() {
    tokio::time::sleep(UNCLEAN_SCRUB_DELAY).await;

    // A scrub that was running when the box went down is resumed *exactly*,
    // on the same pool, before the broader "scan every pool" pass: two
    // different policies would otherwise race and start a second scrub.
    match crate::scrub::resume_interrupted().await {
        Ok(Some(message)) => {
            crate::events::log_event(format!("Post-unclean scrub: {message}"));
            return;
        }
        Ok(None) => {}
        Err(e) => {
            crate::events::log_event_warn(format!("Resuming the interrupted scrub failed: {e}"))
        }
    }

    let pools = match tokio::task::spawn_blocking(PoolService::scan_pools).await {
        Ok(Ok(pools)) => pools,
        Ok(Err(e)) => {
            tracing::warn!("Post-unclean scrub: pool scan failed: {e}");
            return;
        }
        Err(e) => {
            tracing::warn!("Post-unclean scrub: pool scan panicked: {e}");
            return;
        }
    };

    for pool in pools {
        if pool.mount_point.is_none() || pool.health.to_ascii_lowercase().contains("degrad") {
            continue;
        }
        let kind = match pool.pool_type {
            PoolType::Zfs => ScrubKind::Zfs,
            PoolType::Btrfs => ScrubKind::Btrfs,
            PoolType::Mdadm => ScrubKind::Mdadm,
            PoolType::Unknown => continue,
        };
        match scrub::run(&pool.name, kind).await {
            Ok(message) => crate::events::log_event(format!("Post-unclean scrub: {message}")),
            Err(e) => crate::events::log_event_warn(format!(
                "Post-unclean scrub of '{}' failed: {e}",
                pool.name
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fstab_parser_keeps_only_homevault_marked_entries() {
        let text = "\
/dev/sda1 / ext4 defaults 0 1
# homevault-raid:data
UUID=1111 /mnt/raid/data ext4 defaults,nofail 0 2
# a plain comment
/dev/sdb1 /mnt/other ext4 defaults 0 2
# homevault-pool:photos
UUID=2222 /mnt/pools/photos btrfs defaults,nofail,subvol=@ 0 2
# homevault-pool-snapshots:photos
UUID=2222 /mnt/pools/photos/.snapshots btrfs defaults,nofail,subvol=@snapshots 0 2
# homevault-encrypted:vault
/dev/mapper/vault /mnt/encrypted/vault ext4 defaults,nofail 0 2
";
        let entries = parse_homevault_fstab(text);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].marker, "# homevault-raid:data");
        assert_eq!(entries[0].mount_point, "/mnt/raid/data");
        assert_eq!(entries[1].marker, "# homevault-pool:photos");
        assert_eq!(entries[2].mount_point, "/mnt/pools/photos/.snapshots");
        assert_eq!(entries[3].marker, "# homevault-encrypted:vault");
    }

    #[test]
    fn fstab_parser_ignores_a_marker_with_no_entry() {
        let text = "# homevault-pool:empty\n# next comment\n";
        assert!(parse_homevault_fstab(text).is_empty());
    }

    #[test]
    fn mounted_check_matches_the_kernel_mount_point_and_unescapes_spaces() {
        let mounts = "\
/dev/md0 /mnt/raid/data ext4 rw,relatime 0 0
/dev/sdb1 /mnt/my\\040disk btrfs rw 0 0
";
        assert!(is_mount_point_mounted("/mnt/raid/data", mounts));
        assert!(is_mount_point_mounted("/mnt/my disk", mounts));
        assert!(!is_mount_point_mounted("/mnt/raid/other", mounts));
    }

    #[test]
    fn ext4_clean_state_is_the_only_safe_state() {
        assert!(ext4_is_unclean("Filesystem state: not clean\n"));
        assert!(!ext4_is_unclean("Filesystem state: clean\n"));
        assert!(!ext4_is_unclean(""));
    }

    #[test]
    fn fallback_ladder_matches_the_filesystem() {
        assert_eq!(mount_fallbacks("btrfs"), vec!["rescue-then-ro"]);
        assert_eq!(
            mount_fallbacks("ext4"),
            vec!["fsck-then-normal", "ro-noload", "ro"]
        );
        assert_eq!(mount_fallbacks("xfs"), vec!["ro"]);
    }

    #[test]
    fn boot_markers_distinguish_first_boot_clean_and_unclean() {
        let dir = tempfile::tempdir().unwrap();

        // First boot: no marker exists yet, but it is not an unclean shutdown.
        let first = begin_boot_in(dir.path());
        assert!(first.first_boot);
        assert!(!first.unclean);

        // The first shutdown was clean.
        mark_clean_shutdown_in(dir.path()).unwrap();
        let second = begin_boot_in(dir.path());
        assert!(!second.first_boot);
        assert!(!second.unclean);

        // The second shutdown was not clean: the marker is absent.
        let third = begin_boot_in(dir.path());
        assert!(!third.first_boot);
        assert!(third.unclean);
    }

    #[test]
    fn unclean_marker_is_cleared_for_the_next_boot() {
        let dir = tempfile::tempdir().unwrap();
        begin_boot_in(dir.path());
        mark_clean_shutdown_in(dir.path()).unwrap();
        begin_boot_in(dir.path());
        assert!(!clean_marker(dir.path()).exists());
        assert!(initialized_marker(dir.path()).exists());
    }

    #[test]
    fn manual_mount_options_strip_systemd_only_flags() {
        assert_eq!(
            manual_mount_options(
                "defaults,nofail,compress=zstd:3,x-systemd.device-timeout=30s,x-systemd.mount-timeout=30s"
            ),
            "defaults,nofail,compress=zstd:3"
        );
        assert_eq!(
            manual_mount_options("x-systemd.mount-timeout=30s"),
            "defaults"
        );
        assert_eq!(manual_mount_options("ro,noload"), "ro,noload");
    }
}
