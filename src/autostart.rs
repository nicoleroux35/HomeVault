//! Controls whether the `homevault.service` systemd unit is enabled to
//! start automatically on boot. `installer/install.sh` already runs
//! `systemctl enable homevault` once at install time; this lets an admin
//! toggle that afterward from the Settings page instead of needing shell
//! access to the host.

use std::process::Command;

const SERVICE_NAME: &str = "homevault";

/// True if the unit is enabled to start on boot. Read-only query — no
/// root needed, unlike `set_enabled` below.
pub fn is_enabled() -> bool {
    Command::new("systemctl")
        .args(["is-enabled", "--quiet", SERVICE_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Enables or disables start-on-boot for the HomeVault service itself.
/// Needs root — `homevault` has a scoped sudoers grant for exactly
/// `systemctl enable homevault` / `systemctl disable homevault` (see
/// `SetupService::configure_sudoers`), deliberately not the bare
/// `systemctl` binary, which can control *any* unit on the host.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let action = if enabled { "enable" } else { "disable" };
    let output = Command::new("sudo")
        .args(["systemctl", action, SERVICE_NAME])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
