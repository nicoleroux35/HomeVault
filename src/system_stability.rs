/* =============================================================================
 * File          : system_stability.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Host-stability policy (tasklist P1-28): the systemd hardening directives
 *   HomeVault's unit carries, the sysctl values it installs, and the
 *   write-cache/UPS warning. Pure data + parsing so the installer and the
 *   Health screen share one source of truth.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   `installer/install.sh` embeds the same values; see docs/guides/health.md.
 * =============================================================================
 */

//! systemd/sysctl hardening values and disk-write-cache policy.

/// Hardening lines written into the `[Service]` section of the HomeVault
/// unit. `TimeoutStopSec` bounds a graceful shutdown (the clean-shutdown
/// marker is written last); `StartLimit*` stops a crash-loop from hammering
/// the host; `OOMPolicy=stop` makes an OOM kill visible instead of leaving a
/// half-dead process.
pub const SERVICE_HARDENING: &[&str] = &[
    "LimitNOFILE=65536",
    "TimeoutStopSec=30",
    "StartLimitIntervalSec=60",
    "StartLimitBurst=5",
    "OOMPolicy=stop",
];

/// Kernel knobs that keep a panic/ooops from leaving the box wedged and bound
/// the dirty page cache so a power cut loses less.
pub const SYSCTL_LINES: &[(&str, &str)] = &[
    ("kernel.panic", "10"),
    ("kernel.panic_on_oops", "1"),
    ("vm.dirty_background_ratio", "5"),
    ("vm.dirty_ratio", "10"),
];

/// Render the `/etc/sysctl.d/99-homevault.conf` body.
pub fn sysctl_file() -> String {
    let mut out = String::from(
        "# Written by HomeVault (tasklist P1-28): bounded dirty cache and a\n\
         # reboot after a kernel panic instead of a silently wedged host.\n",
    );
    for (key, value) in SYSCTL_LINES {
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// Parse `hdparm -W <device>`: `Write cache: enabled` / `disabled`.
pub fn parse_write_cache(output: &str) -> Option<bool> {
    for line in output.lines() {
        let lower = line.trim().to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("write cache:") {
            let rest = rest.trim();
            if rest.starts_with("enabled") {
                return Some(true);
            }
            if rest.starts_with("disabled") {
                return Some(false);
            }
        }
    }
    None
}

/// A drive whose write cache is on but which is not protected by a UPS can
/// lose acknowledged writes on a power cut. Returns the warning text, or
/// `None` when the configuration is safe.
pub fn write_cache_warning(write_cache_enabled: bool, ups_present: bool) -> Option<String> {
    if write_cache_enabled && !ups_present {
        Some(
            "A disk write cache is enabled without a UPS: a power cut can lose writes the disk already acknowledged. Enable the UPS or disable the cache.".to_string(),
        )
    } else {
        None
    }
}

/// External monitoring services duplicate HomeVault's own jobs. Returns a
/// warning when both are active, so the admin can keep exactly one.
pub fn duplicate_monitor_warning(
    smartd_active: bool,
    mdmonitor_active: bool,
    homevault_disk_health: bool,
    homevault_raid_monitor: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if smartd_active && homevault_disk_health {
        warnings.push(
            "smartd and HomeVault's disk-health sampler are both active — keep one to avoid duplicate alerts"
                .to_string(),
        );
    }
    if mdmonitor_active && homevault_raid_monitor {
        warnings.push(
            "mdmonitor and HomeVault's RAID monitor are both active — keep one to avoid duplicate alerts"
                .to_string(),
        );
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_and_sysctl_policy_are_explicit() {
        assert!(SERVICE_HARDENING.contains(&"LimitNOFILE=65536"));
        assert!(SERVICE_HARDENING.contains(&"OOMPolicy=stop"));
        let file = sysctl_file();
        assert!(file.contains("kernel.panic = 10"));
        assert!(file.contains("vm.dirty_ratio = 10"));
        assert_eq!(SYSCTL_LINES.len(), 4);
    }

    #[test]
    fn hdparm_output_is_parsed_both_ways() {
        assert_eq!(
            parse_write_cache(" /dev/sda:\n  Write cache: enabled, read cache: enabled"),
            Some(true)
        );
        assert_eq!(
            parse_write_cache("  Write cache: disabled, read cache: enabled"),
            Some(false)
        );
        assert_eq!(parse_write_cache("nothing here"), None);
    }

    #[test]
    fn an_unprotected_write_cache_is_warned_about() {
        assert!(write_cache_warning(true, false).is_some());
        assert!(write_cache_warning(true, true).is_none());
        assert!(write_cache_warning(false, false).is_none());
    }

    #[test]
    fn duplicate_monitors_are_reported_once_each() {
        let warnings = duplicate_monitor_warning(true, true, true, true);
        assert_eq!(warnings.len(), 2);
        assert!(duplicate_monitor_warning(false, true, true, false).is_empty());
    }
}
