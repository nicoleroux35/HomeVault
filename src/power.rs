/* =============================================================================
 * File          : power.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Host power management primitives: CPU scaling-governor read/set, host
 *   reboot/shutdown/suspend through systemctl, and disk spindown (hdparm
 *   -B/-S) application. Pure helpers (argument building, input validation)
 *   are unit-tested; the sudo-wrapped system calls are only exercised live —
 *   see "What NOT to test in Rust" in CLAUDE.md.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   handlers/power.rs (Settings power card), scheduled_tasks_scheduler.rs
 *   (scheduled power actions) and the startup re-apply in web/mod.rs.
 * =============================================================================
 */

//! Host power management: CPU governor, host power actions, disk spindown.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Scaling-governor sysfs directory (first CPU is representative — all CPUs
/// of one policy share the governor).
pub const GOVERNOR_DIR: &str = "/sys/devices/system/cpu/cpu0/cpufreq";

/// A host power action the admin (or a scheduled task) can request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Reboot,
    Shutdown,
    Suspend,
}

impl PowerAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            PowerAction::Reboot => "reboot",
            PowerAction::Shutdown => "shutdown",
            PowerAction::Suspend => "suspend",
        }
    }

    /// The systemctl verb the action maps to.
    pub fn systemctl_verb(&self) -> &'static str {
        match self {
            PowerAction::Reboot => "reboot",
            PowerAction::Shutdown => "poweroff",
            PowerAction::Suspend => "suspend",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reboot" => Some(PowerAction::Reboot),
            "shutdown" => Some(PowerAction::Shutdown),
            "suspend" => Some(PowerAction::Suspend),
            _ => None,
        }
    }
}

/// Reads the current CPU scaling governor, when cpufreq is available.
pub fn current_governor() -> Option<String> {
    std::fs::read_to_string(format!("{GOVERNOR_DIR}/scaling_governor"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Lists the governors the kernel makes available (one per line of
/// `scaling_available_governors`), empty when cpufreq is absent (VMs).
pub fn available_governors() -> Vec<String> {
    std::fs::read_to_string(format!("{GOVERNOR_DIR}/scaling_available_governors"))
        .map(|s| {
            s.split_whitespace()
                .map(|g| g.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Validates a governor name against the kernel-provided list — the value is
/// written through `sudo tee`, so anything with shell metacharacters must be
/// rejected before it leaves the process.
pub fn is_valid_governor(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && available_governors().iter().any(|g| g == name)
}

/// Sets the CPU scaling governor via `sudo tee` (sysfs is root-owned).
pub fn set_governor(governor: &str) -> Result<(), String> {
    if !is_valid_governor(governor) {
        return Err(format!("Unknown or invalid CPU governor '{}'", governor));
    }
    // Write to every policy so per-policy governors stay consistent —
    // cpu0/cpufreq is just the first one.
    let mut policies = Vec::new();
    let entries = std::fs::read_dir("/sys/devices/system/cpu").map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("cpu") && name[3..].chars().all(|c| c.is_ascii_digit()) {
            let gov = entry.path().join("cpufreq").join("scaling_governor");
            if gov.exists() {
                policies.push(gov);
            }
        }
    }
    if policies.is_empty() {
        return Err("cpufreq is not available on this host".to_string());
    }
    let mut failures = Vec::new();
    for gov in policies {
        let output = std::process::Command::new("sudo")
            .args(["tee", gov.to_str().unwrap_or_default()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(governor.as_bytes());
                }
                child.wait_with_output()
            });
        match output {
            Ok(out) if out.status.success() => {}
            Ok(out) => failures.push(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(e) => failures.push(e.to_string()),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("Failed to set governor: {}", failures.join("; ")))
    }
}

/// Executes a host power action through systemctl. This is intentionally
/// synchronous and terminal for the box — callers confirm in the UI first.
pub fn host_power_action(action: PowerAction) -> Result<(), String> {
    use crate::utils::process::run_timed;
    use std::time::Duration;

    // Bounded (ROADMAP X.2): a `systemctl reboot` that is waiting on a stuck
    // unit must still answer, not hold the request open.
    let output = run_timed(
        std::process::Command::new("sudo").args(["systemctl", action.systemctl_verb()]),
        Duration::from_secs(30),
    )
    .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "systemctl {} failed: {}",
            action.systemctl_verb(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Per-disk spindown configuration: APM level (1–255, 1 = most aggressive
/// spindown, 255 disables APM) and spindown timeout in minutes (0 = never).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskSpindownSetting {
    pub device: String,
    /// hdparm -B value: 1..=127 allow spindown, 128..=254 don't, 255 disables.
    pub apm_level: Option<u8>,
    /// hdparm -S value in minutes (0 = off; internally mapped to hdparm's
    /// coded units by `spindown_timeout_arg`).
    pub spindown_minutes: Option<u32>,
}

/// Maps a spindown timeout in minutes to hdparm's `-S` coded argument
/// (hdparm encodes 1..=240 as 5-second units — so 20 min is 240 — and
/// 241..=251 as half-hour blocks up to 5.5 h). 0 means "never spindown".
/// Longer than 5.5 h is rejected rather than silently wrapped.
pub fn spindown_timeout_arg(minutes: u32) -> Option<u8> {
    match minutes {
        0 => Some(0),
        1..=20 => Some((minutes * 12) as u8), // 5-second units, 20 min = 240
        21..=330 => Some(240 + minutes.div_ceil(30) as u8), // 241..=251
        _ => None,
    }
}

/// Builds the `hdparm` arguments for one disk. Returns `None` when the
/// setting has nothing to apply.
pub fn hdparm_args(setting: &DiskSpindownSetting) -> Option<Vec<String>> {
    let mut args = vec![];
    if let Some(apm) = setting.apm_level {
        args.push("-B".to_string());
        args.push(apm.to_string());
    }
    if let Some(minutes) = setting.spindown_minutes {
        if let Some(code) = spindown_timeout_arg(minutes) {
            args.push("-S".to_string());
            args.push(code.to_string());
        }
    }
    if args.is_empty() {
        return None;
    }
    args.push(format!("/dev/{}", setting.device));
    Some(args)
}

/// Validates a device basename for spindown/SMART use: it must be a bare
/// kernel block-device name (no paths, no options) because it is spliced
/// into `/dev/{device}` for sudo-wrapped commands.
pub fn is_valid_block_device_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && Path::new(name)
            .file_name()
            .map(|f| f.to_string_lossy() == name)
            .unwrap_or(false)
}

/// Applies one disk's spindown settings via hdparm. Best-effort: disks that
/// don't support APM (SSDs, USB bridges) fail — the caller logs, not fatal.
pub fn apply_spindown(setting: &DiskSpindownSetting) -> Result<(), String> {
    use crate::utils::process::run_timed;
    use std::time::Duration;

    if !is_valid_block_device_name(&setting.device) {
        return Err(format!("Invalid device name '{}'", setting.device));
    }
    let Some(args) = hdparm_args(setting) else {
        return Ok(()); // nothing configured for this disk
    };
    // Bounded (ROADMAP X.2): `hdparm -y` on a wedged controller blocks in the
    // kernel and used to keep the caller's blocking thread forever.
    let output = run_timed(
        std::process::Command::new("sudo").arg("hdparm").args(&args),
        Duration::from_secs(30),
    )
    .map_err(|e| format!("hdparm not available: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "hdparm failed for /dev/{}: {}",
            setting.device,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Re-applies every persisted disk spindown setting (startup + after edit).
pub fn apply_all_spindown(settings: &[DiskSpindownSetting]) {
    for setting in settings {
        if let Err(e) = apply_spindown(setting) {
            tracing::warn!("Spindown apply skipped for /dev/{}: {}", setting.device, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_action_round_trips() {
        for name in ["reboot", "shutdown", "suspend"] {
            let action = PowerAction::parse(name).expect("known action");
            assert_eq!(action.as_str(), name);
        }
        assert!(PowerAction::parse("format").is_none());
        assert_eq!(PowerAction::Shutdown.systemctl_verb(), "poweroff");
        assert_eq!(PowerAction::Suspend.systemctl_verb(), "suspend");
    }

    #[test]
    fn governor_names_with_metacharacters_are_rejected() {
        // `is_valid_governor` also requires the kernel to list the name, so
        // on a host without cpufreq everything is rejected — assert the
        // dangerous forms are rejected *even when* the list would allow them.
        for bad in [
            "performance; rm -rf /",
            "$(reboot)",
            "`id`",
            "performance\n",
            "",
            "performance settings",
            "a".repeat(33).as_str(),
        ] {
            assert!(!is_valid_governor(bad), "'{bad}' must be rejected");
        }
        assert!(!is_valid_governor("not_a_governor_the_kernel_lists"));
    }

    #[test]
    fn spindown_timeout_mapping_matches_hdparm_codes() {
        assert_eq!(spindown_timeout_arg(0), Some(0));
        assert_eq!(spindown_timeout_arg(1), Some(12), "1 min = 12 × 5 s");
        assert_eq!(spindown_timeout_arg(10), Some(120));
        assert_eq!(
            spindown_timeout_arg(20),
            Some(240),
            "20 min is the last 5-s unit"
        );
        assert_eq!(
            spindown_timeout_arg(21),
            Some(241),
            "21 min → first 30-min block"
        );
        assert_eq!(spindown_timeout_arg(30), Some(241));
        assert_eq!(spindown_timeout_arg(31), Some(242));
        assert_eq!(
            spindown_timeout_arg(330),
            Some(251),
            "5.5 h is the last block"
        );
        assert_eq!(
            spindown_timeout_arg(331),
            None,
            "longer is rejected, not wrapped"
        );
    }

    #[test]
    fn hdparm_args_built_from_setting() {
        let both = DiskSpindownSetting {
            device: "sda".into(),
            apm_level: Some(127),
            spindown_minutes: Some(10),
        };
        assert_eq!(
            hdparm_args(&both),
            Some(vec![
                "-B".into(),
                "127".into(),
                "-S".into(),
                "120".into(),
                "/dev/sda".into()
            ])
        );
        let off = DiskSpindownSetting {
            device: "sdb".into(),
            apm_level: None,
            spindown_minutes: None,
        };
        assert_eq!(hdparm_args(&off), None, "empty setting must be a no-op");
    }

    #[test]
    fn block_device_names_are_strictly_basenames() {
        assert!(is_valid_block_device_name("sda"));
        assert!(is_valid_block_device_name("nvme0n1"));
        assert!(is_valid_block_device_name("sda1"));
        for bad in ["../sda", "/dev/sda", "sda -c1", "", "sda;reboot", ".hidden"] {
            assert!(!is_valid_block_device_name(bad), "'{bad}' must be rejected");
        }
    }
}
