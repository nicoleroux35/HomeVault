/* =============================================================================
 * File          : btrfs_maintenance.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Btrfs housekeeping (tasklist P1-22): the mount options HomeVault writes,
 *   a parser for `btrfs filesystem usage`, the conditional-balance rule and
 *   TRIM. Pure planning + parsing here; the caller runs the argv.
 *
 * Dependencies:
 *   - serde
 *
 * Usage:
 *   See docs/guides/storage.md.
 * =============================================================================
 */

//! Btrfs mount options, usage parsing and maintenance plans.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::handlers::AppState;
use crate::utils::process::run_timed_async_result;

/// Mount options HomeVault writes for a live Btrfs subvolume. `zstd:3` is the
/// upstream default compression level and cheap enough for a NAS; `noatime`
/// removes a write per file read. `space_cache=v2` is the modern free-space
/// tree (a pool created by an older kernel simply keeps its v1 cache).
pub const BTRFS_LIVE_OPTIONS: &str = "defaults,nofail,compress=zstd:3,space_cache=v2,noatime";

/// `/etc/fstab`-only options: systemd waits 30 s for the device and for the
/// mount instead of blocking boot forever. `mount(8)` does not understand
/// them, so the boot-recovery path strips them before its manual replay.
pub const FSTAB_SYSTEMD_OPTIONS: &str = "x-systemd.device-timeout=30s,x-systemd.mount-timeout=30s";

/// The `/etc/fstab` option string for a subvolume.
pub fn mount_options(subvol: Option<&str>) -> String {
    match subvol {
        Some(subvol) => format!("{BTRFS_LIVE_OPTIONS},subvol={subvol}"),
        None => BTRFS_LIVE_OPTIONS.to_string(),
    }
}

/// What HomeVault actually writes in `/etc/fstab`: the live mount options
/// plus the systemd-only timeouts.
pub fn fstab_options(subvol: Option<&str>) -> String {
    format!("{},{}", mount_options(subvol), FSTAB_SYSTEMD_OPTIONS)
}

/// A parsed `btrfs filesystem usage` report. Percentages are 0–100.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BtrfsUsage {
    pub unallocated_bytes: u64,
    pub metadata_dup_used_pct: f64,
    pub data_single_used_pct: f64,
}

/// Parse the size token btrfs prints (`12.34GiB`, `512.00MiB`, `1024`).
pub fn parse_size(value: &str) -> Option<u64> {
    let value = value.trim();
    let (number, multiplier) = if let Some(v) = value.strip_suffix("TiB") {
        (v, 1024u64.pow(4))
    } else if let Some(v) = value.strip_suffix("GiB") {
        (v, 1024u64.pow(3))
    } else if let Some(v) = value.strip_suffix("MiB") {
        (v, 1024u64.pow(2))
    } else if let Some(v) = value.strip_suffix("KiB") {
        (v, 1024)
    } else if let Some(v) = value.strip_suffix('B') {
        (v, 1)
    } else {
        (value, 1)
    };
    let number: f64 = number.trim().parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    Some((number * multiplier as f64) as u64)
}

/// Pull the percentage out of `Used:1.00GiB (20.00%)`.
fn used_percent(line: &str) -> Option<f64> {
    let start = line.rfind('(')?;
    let end = line[start..].find('%')? + start;
    line[start + 1..end].trim().parse().ok()
}

/// Parse a `btrfs filesystem usage` report.
pub fn parse_usage(output: &str) -> Option<BtrfsUsage> {
    let mut unallocated_bytes = None;
    let mut metadata_dup_used_pct = 0.0;
    let mut data_single_used_pct = 0.0;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Device unallocated:") {
            unallocated_bytes = parse_size(rest);
        } else if trimmed.starts_with("Metadata,DUP:") {
            metadata_dup_used_pct = used_percent(trimmed).unwrap_or(0.0);
        } else if trimmed.starts_with("Data,single:") {
            data_single_used_pct = used_percent(trimmed).unwrap_or(0.0);
        }
    }
    Some(BtrfsUsage {
        unallocated_bytes: unallocated_bytes?,
        metadata_dup_used_pct,
        data_single_used_pct,
    })
}

/// A balance is worth running when metadata or data chunks are nearly full
/// *and* the device has little unallocated space left. Otherwise a balance
/// would rewrite data for nothing.
pub fn balance_needed(usage: &BtrfsUsage) -> bool {
    const USED_PCT: f64 = 4.0;
    const MIN_UNALLOCATED: u64 = 5 * 1024 * 1024 * 1024;
    usage.unallocated_bytes < MIN_UNALLOCATED
        && (usage.metadata_dup_used_pct > USED_PCT || usage.data_single_used_pct > USED_PCT)
}

/// The argv that balances data chunks above 50 % utilisation. `-dusage=50` is
/// the conservative upstream default; metadata is left alone.
pub fn balance_command(mount_point: &str) -> Vec<String> {
    vec![
        "sudo".to_string(),
        "btrfs".to_string(),
        "balance".to_string(),
        "start".to_string(),
        "-dusage=50".to_string(),
        mount_point.to_string(),
    ]
}

/// The same command with `-v` replaced by `--dry-run`, for the UI preview.
pub fn balance_dry_run_command(mount_point: &str) -> Vec<String> {
    let mut command = balance_command(mount_point);
    command.insert(4, "--dry-run".to_string());
    command
}

/// `fstrim` releases blocks a Btrfs filesystem no longer uses to the SSD.
pub fn trim_command(mount_point: &str) -> Vec<String> {
    vec![
        "sudo".to_string(),
        "fstrim".to_string(),
        "--verbose".to_string(),
        mount_point.to_string(),
    ]
}

/// `btrfs device stats` — surfaced in Health by the caller.
pub fn device_stats_command(mount_point: &str) -> Vec<String> {
    vec![
        "sudo".to_string(),
        "btrfs".to_string(),
        "device".to_string(),
        "stats".to_string(),
        mount_point.to_string(),
    ]
}

/// Compression levels the UI accepts (`zstd:1` fast … `zstd:15` slow).
pub fn validate_compression_level(value: &str) -> Result<String, String> {
    let value = value.trim().to_ascii_lowercase();
    let level = value
        .strip_prefix("zstd:")
        .or_else(|| value.strip_prefix("zstd"))
        .unwrap_or("");
    let level: u8 = level
        .parse()
        .map_err(|_| "compression must be 'zstd:<1-15>' or 'none'".to_string())?;
    if !(1..=15).contains(&level) {
        return Err("the zstd level must be between 1 and 15".to_string());
    }
    Ok(format!("zstd:{level}"))
}

/// Defragmentation inside a snapshot would rewrite the snapshot's extents and
/// destroy the sharing (`reflink`) that keeps snapshots cheap — so the path is
/// refused **before** a command runs.
pub fn defrag_command(path: &str) -> Result<Vec<String>, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("a path is required".to_string());
    }
    if path.contains("/.snapshots/") || path.ends_with("/.snapshots") {
        return Err(
            "refusing to defragment inside .snapshots: it would break snapshot sharing".to_string(),
        );
    }
    Ok(vec![
        "sudo".to_string(),
        "btrfs".to_string(),
        "filesystem".to_string(),
        "defragment".to_string(),
        "-r".to_string(),
        "-czstd".to_string(),
        path.to_string(),
    ])
}

/// Validate a path that a compression policy may target: absolute, no `..`,
/// and never inside `.snapshots`.
pub fn validate_policy_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("a path is required".to_string());
    }
    if !path.starts_with('/') {
        return Err("the path must be absolute".to_string());
    }
    if path.split('/').any(|part| part == "..") {
        return Err("the path must not contain '..'".to_string());
    }
    if path.contains("/.snapshots/") || path.ends_with("/.snapshots") {
        return Err(
            "refusing to apply compression inside .snapshots: it would break snapshot sharing"
                .to_string(),
        );
    }
    Ok(path.to_string())
}

/// Defragment `path` with an explicit compression level (`zstd:1`…`zstd:15`
/// or `none` for a plain defrag).
pub fn defrag_level_command(path: &str, level: &str) -> Result<Vec<String>, String> {
    let path = validate_policy_path(path)?;
    let level = if level.trim().eq_ignore_ascii_case("none") {
        "none".to_string()
    } else {
        validate_compression_level(level)?
    };
    let mut command = vec![
        "sudo".to_string(),
        "btrfs".to_string(),
        "filesystem".to_string(),
        "defragment".to_string(),
        "-r".to_string(),
    ];
    if level != "none" {
        command.push(format!("-c{level}"));
    }
    command.push(path);
    Ok(command)
}

/// Make new files inherit compression (`chattr +c`) — or stop inheriting it
/// (`chattr -c`) when the policy is `none`.
pub fn compression_inherit_command(path: &str, level: &str) -> Result<Vec<String>, String> {
    let path = validate_policy_path(path)?;
    let flag = if level.trim().eq_ignore_ascii_case("none") {
        "-c"
    } else {
        "+c"
    };
    Ok(vec![
        "sudo".to_string(),
        "chattr".to_string(),
        flag.to_string(),
        path,
    ])
}

/// Whether a stored policy is due for another defrag pass (monthly).
pub fn policy_due(last: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last) = last else {
        return true;
    };
    match DateTime::parse_from_rfc3339(last.trim()) {
        Ok(at) => now.signed_duration_since(at.with_timezone(&Utc)).num_days() >= 30,
        Err(_) => true,
    }
}

/// Apply one stored policy: defragment existing extents when requested, then
/// set (or clear) compression inheritance for future files.
pub async fn apply_compression_policy(
    policy: &crate::storage::btrfs_policy_repo::BtrfsCompressionPolicy,
) -> Result<String, String> {
    validate_policy_path(&policy.path)?;
    if policy.defrag {
        let args = defrag_level_command(&policy.path, &policy.level)?;
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = run_sudo(&args, Duration::from_secs(3600)).await?;
        if !output.status.success() {
            return Err(format!(
                "defrag of {} failed: {}",
                policy.path,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let args = compression_inherit_command(&policy.path, &policy.level)?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_sudo(&args, Duration::from_secs(30)).await?;
    if !output.status.success() {
        return Err(format!(
            "could not set compression inheritance on {}: {}",
            policy.path,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(format!(
        "applied {} to {}{}",
        policy.level,
        policy.path,
        if policy.defrag {
            " (existing extents defragmented)"
        } else {
            ""
        }
    ))
}

/// Whether the built-in maintenance loop is allowed to run. Default on;
/// `0`/`false`/`no`/`off` disables both balance and TRIM.
pub fn maintenance_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0") | Some("false") | Some("no") | Some("off")
    )
}

/// TRIM is weekly. A missing marker (first run) counts as due; a malformed
/// timestamp also counts as due rather than silently postponing forever.
pub fn trim_due(last: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last) = last else {
        return true;
    };
    match DateTime::parse_from_rfc3339(last.trim()) {
        Ok(at) => {
            now.signed_duration_since(at.with_timezone(&Utc))
                .num_seconds()
                >= 7 * 24 * 60 * 60
        }
        Err(_) => true,
    }
}

/// Mount points of writable Btrfs filesystems in `/proc/self/mounts`.
/// Read-only mounts and `.snapshots` subvolumes are skipped: trimming a
/// read-only filesystem fails, and snapshots are not independent storage.
pub fn parse_btrfs_mounts(mounts: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 || fields[2] != "btrfs" {
            continue;
        }
        let mount = fields[1];
        if mount.ends_with("/.snapshots") || fields[3].split(',').any(|o| o == "ro") {
            continue;
        }
        out.push(mount.to_string());
    }
    out
}

fn trim_marker() -> std::path::PathBuf {
    crate::config::paths::data_dir()
        .join("state")
        .join("btrfs-last-trim")
}

fn last_trim() -> Option<String> {
    std::fs::read_to_string(trim_marker()).ok()
}

fn record_trim(now: &DateTime<Utc>) {
    let path = trim_marker();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = crate::storage::atomic::AtomicWriter::write(&path, &now.to_rfc3339());
}

async fn run_sudo(args: &[&str], deadline: Duration) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new("sudo");
    cmd.args(args);
    run_timed_async_result(&mut cmd, deadline)
        .await
        .map_err(|e| format!("sudo {}: {e}", args.join(" ")))
}

/// One maintenance pass: conditional balance on every mounted Btrfs pool and
/// a weekly `fstrim`. Returns the human-readable actions performed.
pub async fn run_once(state: &Arc<AppState>) -> Result<Vec<String>, String> {
    let now = Utc::now();
    let mounts = std::fs::read_to_string("/proc/self/mounts")
        .map(|text| parse_btrfs_mounts(&text))
        .unwrap_or_default();
    let mut actions = Vec::new();
    let trim = trim_due(last_trim().as_deref(), now);

    for mount in mounts {
        let mut usage_cmd = tokio::process::Command::new("btrfs");
        usage_cmd.args(["filesystem", "usage", "-b", &mount]);
        if let Ok(output) = run_timed_async_result(&mut usage_cmd, Duration::from_secs(30)).await {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(usage) = parse_usage(&text) {
                if balance_needed(&usage) {
                    let args: Vec<String> = balance_command(&mount);
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    match run_sudo(&args, Duration::from_secs(3600)).await {
                        Ok(out) if out.status.success() => {
                            actions.push(format!("balanced {mount} (space was short)"))
                        }
                        Ok(out) => actions.push(format!(
                            "balance on {mount} failed: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        )),
                        Err(e) => actions.push(format!("balance on {mount} failed: {e}")),
                    }
                }
            }
        }

        if trim {
            let args: Vec<String> = trim_command(&mount);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match run_sudo(&args, Duration::from_secs(600)).await {
                Ok(out) if out.status.success() => actions.push(format!("trimmed {mount}")),
                Ok(out) => actions.push(format!(
                    "trim on {mount} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                Err(e) => actions.push(format!("trim on {mount} failed: {e}")),
            }
        }
    }

    if trim && actions.iter().any(|action| action.starts_with("trimmed ")) {
        record_trim(&now);
    }

    // Per-share compression policies (P2-11): apply at most monthly, because
    // a recursive defrag rewrites every extent and is I/O-heavy.
    let repo = crate::storage::btrfs_policy_repo::BtrfsPolicyRepository::new(state.db.pool());
    if let Ok(policies) = repo.list().await {
        for policy in policies {
            if !policy_due(policy.last_applied_at.as_deref(), now) {
                continue;
            }
            match apply_compression_policy(&policy).await {
                Ok(message) => {
                    actions.push(message);
                    let _ = repo.mark_applied(&policy.path).await;
                }
                Err(e) => {
                    actions.push(format!("compression policy on {} failed: {e}", policy.path))
                }
            }
        }
    }
    Ok(actions)
}

/// Weekly maintenance loop. Balance is conditional; TRIM is weekly; both have
/// an execution deadline and never run on a read-only mount.
pub async fn run_scheduler_loop(state: Arc<AppState>) {
    tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    loop {
        if maintenance_enabled(std::env::var("HOMEVAULT_BTRFS_MAINTENANCE").ok().as_deref()) {
            match run_once(&state).await {
                Ok(actions) if !actions.is_empty() => {
                    crate::events::log_event(format!("Btrfs maintenance: {}", actions.join("; ")));
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("Btrfs maintenance failed: {e}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_options_keep_the_subvolume_and_the_safe_defaults() {
        assert_eq!(
            mount_options(Some("@")),
            format!("{BTRFS_LIVE_OPTIONS},subvol=@")
        );
        let live = mount_options(None);
        assert!(live.contains("compress=zstd:3"));
        assert!(live.contains("space_cache=v2"));
        assert!(live.contains("noatime"));
        assert!(!live.contains("subvol="));
    }

    #[test]
    fn fstab_options_add_the_systemd_timeouts_without_changing_the_mount() {
        let live = fstab_options(None);
        assert!(live.starts_with(BTRFS_LIVE_OPTIONS));
        assert!(live.ends_with(FSTAB_SYSTEMD_OPTIONS));
        assert!(live.contains("x-systemd.device-timeout=30s"));
        assert!(live.contains("x-systemd.mount-timeout=30s"));
        assert_eq!(
            fstab_options(Some("@snapshots")),
            format!("{BTRFS_LIVE_OPTIONS},subvol=@snapshots,{FSTAB_SYSTEMD_OPTIONS}")
        );
    }

    #[test]
    fn maintenance_is_opt_out_and_trim_is_weekly() {
        assert!(maintenance_enabled(None));
        assert!(maintenance_enabled(Some("1")));
        assert!(!maintenance_enabled(Some(" off ")));
        let now = Utc::now();
        assert!(trim_due(None, now));
        assert!(trim_due(Some("not-a-date"), now));
        assert!(!trim_due(
            Some(&(now - chrono::Duration::days(6)).to_rfc3339()),
            now
        ));
        assert!(trim_due(
            Some(&(now - chrono::Duration::days(8)).to_rfc3339()),
            now
        ));
    }

    #[test]
    fn btrfs_mounts_skip_read_only_and_snapshots() {
        let mounts = "\
/dev/sda1 /mnt/pools/photos btrfs rw,noatime,subvol=@ 0 0
/dev/sda1 /mnt/pools/photos/.snapshots btrfs rw,subvol=@snapshots 0 0
/dev/sdb1 /mnt/pools/cold btrfs ro,noatime 0 0
/dev/sdc1 / btrfs rw 0 0
/dev/sdd1 /mnt/ext4 ext4 rw 0 0
";
        assert_eq!(parse_btrfs_mounts(mounts), vec!["/mnt/pools/photos", "/"]);
    }

    #[test]
    fn sizes_and_percentages_are_parsed() {
        assert_eq!(parse_size("1.50GiB"), Some(1610612736));
        assert_eq!(parse_size("512.00MiB"), Some(536870912));
        assert_eq!(parse_size("10B"), Some(10));
        assert_eq!(parse_size("garbage"), None);
        let report = "\
Overall:
    Device size:		  100.00GiB
    Device allocated:		   20.00GiB
    Device unallocated:		   80.00GiB
    Free (estimated):		   78.00GiB	(min: 40.00GiB)
Data,single: Size:10.00GiB, Used:5.00GiB (50.00%)
Metadata,DUP: Size:5.00GiB, Used:1.00GiB (20.00%)
System,DUP: Size:32.00MiB, Used:16.00KiB (0.05%)
";
        let usage = parse_usage(report).expect("usage");
        assert_eq!(usage.unallocated_bytes, 80 * 1024 * 1024 * 1024);
        assert_eq!(usage.data_single_used_pct, 50.0);
        assert_eq!(usage.metadata_dup_used_pct, 20.0);
    }

    #[test]
    fn balance_only_runs_when_space_is_actually_short() {
        let tight = BtrfsUsage {
            unallocated_bytes: 1024 * 1024 * 1024,
            metadata_dup_used_pct: 90.0,
            data_single_used_pct: 10.0,
        };
        assert!(balance_needed(&tight));
        let roomy = BtrfsUsage {
            unallocated_bytes: 50 * 1024 * 1024 * 1024,
            metadata_dup_used_pct: 90.0,
            data_single_used_pct: 10.0,
        };
        assert!(!balance_needed(&roomy));
        let mostly_free = BtrfsUsage {
            unallocated_bytes: 0,
            metadata_dup_used_pct: 3.0,
            data_single_used_pct: 2.0,
        };
        assert!(!balance_needed(&mostly_free));
    }

    #[test]
    fn commands_are_unambiguous() {
        assert_eq!(
            balance_command("/mnt/pools/photos"),
            vec![
                "sudo",
                "btrfs",
                "balance",
                "start",
                "-dusage=50",
                "/mnt/pools/photos"
            ]
        );
        assert_eq!(
            balance_dry_run_command("/mnt/pools/photos"),
            vec![
                "sudo",
                "btrfs",
                "balance",
                "start",
                "--dry-run",
                "-dusage=50",
                "/mnt/pools/photos"
            ]
        );
        assert_eq!(
            trim_command("/mnt/pools/photos"),
            vec!["sudo", "fstrim", "--verbose", "/mnt/pools/photos"]
        );
        assert_eq!(device_stats_command("/mnt/pools/photos")[3], "stats");
    }

    #[test]
    fn compression_policy_commands_are_explicit() {
        assert_eq!(
            validate_policy_path("/mnt/pools/photos").unwrap(),
            "/mnt/pools/photos"
        );
        assert!(validate_policy_path("relative").is_err());
        assert!(validate_policy_path("/mnt/../etc").is_err());
        assert!(validate_policy_path("/mnt/pools/photos/.snapshots").is_err());

        assert_eq!(
            defrag_level_command("/mnt/pools/photos", "zstd:3").unwrap(),
            vec![
                "sudo",
                "btrfs",
                "filesystem",
                "defragment",
                "-r",
                "-czstd:3",
                "/mnt/pools/photos"
            ]
        );
        assert_eq!(
            defrag_level_command("/mnt/pools/photos", "none").unwrap(),
            vec![
                "sudo",
                "btrfs",
                "filesystem",
                "defragment",
                "-r",
                "/mnt/pools/photos"
            ]
        );
        assert_eq!(
            compression_inherit_command("/mnt/pools/photos", "zstd:3").unwrap(),
            vec!["sudo", "chattr", "+c", "/mnt/pools/photos"]
        );
        assert_eq!(
            compression_inherit_command("/mnt/pools/photos", "none").unwrap(),
            vec!["sudo", "chattr", "-c", "/mnt/pools/photos"]
        );
    }

    #[test]
    fn a_compression_policy_is_due_monthly() {
        let now = Utc::now();
        assert!(policy_due(None, now));
        assert!(policy_due(Some("bad-date"), now));
        assert!(!policy_due(
            Some(&(now - chrono::Duration::days(10)).to_rfc3339()),
            now
        ));
        assert!(policy_due(
            Some(&(now - chrono::Duration::days(31)).to_rfc3339()),
            now
        ));
    }

    #[test]
    fn compression_levels_are_closed_and_defrag_refuses_snapshots() {
        assert_eq!(validate_compression_level("zstd:3").unwrap(), "zstd:3");
        assert_eq!(validate_compression_level("15").unwrap(), "zstd:15");
        assert!(validate_compression_level("zstd:0").is_err());
        assert!(validate_compression_level("zstd:16").is_err());
        assert!(validate_compression_level("gzip").is_err());

        let command = defrag_command("/mnt/pools/photos/data").unwrap();
        assert_eq!(command[3], "defragment");
        assert!(defrag_command("/mnt/pools/photos/.snapshots/snap").is_err());
        assert!(defrag_command("/mnt/pools/photos/.snapshots").is_err());
        assert!(defrag_command("").is_err());
    }
}
