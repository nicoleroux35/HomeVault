/* =============================================================================
 * File          : autostart.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 24/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Toggles whether the homevault systemd unit starts on boot (the
 *   installer enables it once; this lets an admin change it from Settings
 *   without shell access).
 *
 * Dependencies:
 *   - tokio (>= 1, `rt` feature) — `spawn_blocking`
 *   - crate::utils::process::run_timed — a deadline on every host call
 *
 * Usage:
 *   Called by the autostart settings endpoints (handlers/settings.rs).
 * =============================================================================
 */

//! Controls whether the `homevault.service` systemd unit is enabled to
//! start automatically on boot. `installer/install.sh` already runs
//! `systemctl enable homevault` once at install time; this lets an admin
//! toggle that afterward from the Settings page instead of needing shell
//! access to the host.
//!
//! Both calls are **bounded** (`run_timed`) and run on the blocking pool
//! (ROADMAP 25.1): `sudo systemctl enable` can wait for seconds on the sudo
//! PAM stack or a systemd job, and parking a Tokio worker on it also parks the
//! whole settings save. A refusal is returned with the host's own stderr
//! (ROADMAP 25.2) so the card can say *why*, not just "failed".

use crate::utils::process::run_timed;
use std::process::{Command, Output};
use std::time::Duration;

const SERVICE_NAME: &str = "homevault";

/// Deadline for the read-only `systemctl is-enabled` probe. The probe does not
/// need root; it is bounded so a container or a non-systemd host answers with
/// an error instead of hanging a request.
pub const AUTOSTART_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for the privileged `sudo systemctl enable|disable` call. Longer
/// than the probe because sudo may have to unlock its timestamp and systemd
/// may have to order the job behind another unit.
pub const AUTOSTART_UPDATE_TIMEOUT: Duration = Duration::from_secs(30);

/// The read-only probe's argv, without the program name. Pure, so the exact
/// shape is asserted without touching a host.
pub fn is_enabled_args() -> Vec<&'static str> {
    vec!["is-enabled", SERVICE_NAME]
}

/// The privileged toggle's argv, without the leading `sudo`.
///
/// The unit name is the module constant, never interpolated from the request:
/// the sudoers grant is scoped to exactly `systemctl enable|disable homevault`
/// (see `SetupService::configure_sudoers`), so a caller-supplied name could not
/// work anyway — and a free-form argument is how the grant would widen.
pub fn args_for(enabled: bool) -> Vec<&'static str> {
    vec![
        "systemctl",
        if enabled { "enable" } else { "disable" },
        SERVICE_NAME,
    ]
}

/// Map a finished `systemctl is-enabled` run to `enabled` / `not enabled` /
/// "the host could not answer".
///
/// The three states must stay distinct (ROADMAP 25.2): today a non-systemd
/// host or a missing unit collapses into `false`, so the card says "off"
/// while the truth is "unknown". A non-zero exit with a clean stderr is
/// systemd answering `disabled`/`masked`/`static`…; a non-empty stderr means
/// the query itself failed.
fn enabled_from_output(output: &Output) -> Result<bool, String> {
    if output.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        return Err(stderr.trim().to_string());
    }
    Ok(false)
}

/// Map a finished `sudo systemctl enable|disable` run to `Ok`/`Err`.
///
/// Pure, so the contract that matters — the host's stderr tail is the error
/// the admin is shown — is unit-tested without a host.
fn set_enabled_outcome(enabled: bool, output: &Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return Err(stderr);
    }
    // A silent non-zero exit still needs a reason the card can print.
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stdout.is_empty() {
        format!("exit status {}", output.status)
    } else {
        stdout
    };
    let action = if enabled { "enable" } else { "disable" };
    Err(format!(
        "sudo systemctl {action} {SERVICE_NAME} failed: {detail}"
    ))
}

/// True if the unit is enabled to start on boot.
///
/// `Err` means the host could not answer — never "disabled" (ROADMAP 25.2).
/// The blocking `Command` runs on the blocking pool with a deadline.
pub async fn is_enabled() -> Result<bool, String> {
    let args = is_enabled_args();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("systemctl");
        cmd.args(&args);
        let output = run_timed(&mut cmd, AUTOSTART_PROBE_TIMEOUT)?;
        enabled_from_output(&output)
    })
    .await
    .map_err(|e| format!("the autostart probe could not run: {e}"))?
}

/// Enables or disables start-on-boot for the HomeVault service itself.
/// Needs root — `homevault` has a scoped sudoers grant for exactly
/// `systemctl enable homevault` / `systemctl disable homevault` (see
/// `SetupService::configure_sudoers`), deliberately not the bare
/// `systemctl` binary, which can control *any* unit on the host.
///
/// The blocking `Command` runs on the blocking pool with a deadline, and a
/// refusal returns the host's stderr (ROADMAP 25.1/25.2).
pub async fn set_enabled(enabled: bool) -> Result<(), String> {
    let args = args_for(enabled);
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("sudo");
        cmd.args(&args);
        let output = run_timed(&mut cmd, AUTOSTART_UPDATE_TIMEOUT)?;
        set_enabled_outcome(enabled, &output)
    })
    .await
    .map_err(|e| format!("the autostart update could not run: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    /// A finished command result without running one: `code` is the wait
    /// status on Unix (`0` = success, `256` = exit code 1).
    fn finished(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn args_for_builds_the_scoped_sudo_argv() {
        // ROADMAP 25 — the exact argv the sudoers grant is scoped to. A change
        // here (bare `systemctl`, another verb, a second unit) must fail.
        assert_eq!(
            args_for(true),
            vec!["systemctl", "enable", "homevault"]
        );
        assert_eq!(
            args_for(false),
            vec!["systemctl", "disable", "homevault"]
        );
        assert_eq!(is_enabled_args(), vec!["is-enabled", "homevault"]);
    }

    #[test]
    fn the_unit_name_is_a_constant_never_user_input() {
        // The grant (`systemctl enable homevault`) is only safe because the
        // name is fixed. Both argv helpers must carry the module constant and
        // nothing else, so no request field can reach the privileged call.
        assert_eq!(SERVICE_NAME, "homevault");
        assert!(args_for(true).contains(&SERVICE_NAME));
        assert!(args_for(false).contains(&SERVICE_NAME));
        assert_eq!(is_enabled_args().len(), 2);
    }

    #[test]
    fn enabled_from_output_reports_enabled_on_success() {
        assert_eq!(enabled_from_output(&finished(0, "enabled\n", "")), Ok(true));
    }

    #[test]
    fn enabled_from_output_reads_a_disabled_unit_as_false() {
        // `systemctl is-enabled` prints the state on stdout and exits 1 with a
        // clean stderr for a unit that is not enabled.
        assert_eq!(
            enabled_from_output(&finished(256, "disabled\n", "")),
            Ok(false)
        );
    }

    #[test]
    fn enabled_from_output_surfaces_a_host_that_cannot_answer() {
        // A container / non-systemd host is *unknown*, not "off" (25.2).
        let err = enabled_from_output(&finished(
            256,
            "",
            "System has not been booted with systemd as init system (PID 1). Can't operate.\n",
        ))
        .expect_err("a host that cannot answer must be an error");
        assert!(err.contains("not been booted with systemd"), "{err}");

        let missing = enabled_from_output(&finished(
            256,
            "",
            "Failed to get unit file state for homevault.service: No such file or directory\n",
        ))
        .expect_err("a missing unit must be an error, not 'disabled'");
        assert!(missing.contains("Failed to get unit file state"), "{missing}");
    }

    #[test]
    fn set_enabled_outcome_returns_the_stderr_tail() {
        // ROADMAP 25.2 — this is what the card prints. "polkit refused" and
        // "a password is required" must reach the admin verbatim.
        let err = set_enabled_outcome(
            true,
            &finished(256, "", "polkit refused: no authorization\n"),
        )
        .expect_err("a refused command must be an error");
        assert_eq!(err, "polkit refused: no authorization");
    }

    #[test]
    fn set_enabled_outcome_explains_a_silent_non_zero_exit() {
        // No stderr at all: the response must still name the action and the
        // exit code instead of collapsing to an empty message.
        let err = set_enabled_outcome(false, &finished(256, "", ""))
            .expect_err("a non-zero exit must be an error");
        assert!(err.contains("disable"), "{err}");
        assert!(err.contains("exit status"), "{err}");
    }

    #[test]
    fn set_enabled_outcome_is_ok_on_success() {
        assert_eq!(
            set_enabled_outcome(true, &finished(0, "", "")),
            Ok(())
        );
    }

    #[test]
    fn deadlines_are_bounded_and_ordered() {
        // Both are deadlines, not estimates: the probe must stay short enough
        // that a doomed host answers a GET promptly, and the update must not
        // hold a settings save open for minutes.
        assert!(AUTOSTART_PROBE_TIMEOUT <= AUTOSTART_UPDATE_TIMEOUT);
        assert!(AUTOSTART_PROBE_TIMEOUT >= Duration::from_secs(1));
        assert!(AUTOSTART_UPDATE_TIMEOUT <= Duration::from_secs(60));
    }
}