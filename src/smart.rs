/* =============================================================================
 * File          : smart.rs
 * Author        : Nicolas Le Roux
 * Created       : 22/10/2026
 * Version       : 1.0.0
 *
 * Description:
 *   One shared builder for `smartctl` invocations, so every call site in the
 *   tree runs the same sudo-wrapped argv shape. `smartctl` needs root to open
 *   real disk devices (typically `660 root:disk`), and the sudoers grant
 *   (`installer/install.sh`) delegates exactly `smartctl` to the service —
 *   a bare (unprivileged) call silently sees nothing and reports "no SMART
 *   data" on perfectly healthy hardware.
 *
 *   callers: HealthService::check_disk_smart (blocking), Dashboard disk-card
 *   reads (async), Storage / disk.rs SMART details, tests and temperature
 *   probes (async), the Scheduled Tasks SMART-test action (blocking).
 *
 * Dependencies:
 *   - std::process, tokio::process
 *
 * Usage:
 *   crate::smart::std_command / tokio_command; callers append `.output()`.
 * =============================================================================
 */

//! Shared sudo-wrapped `smartctl` invocation (Health, Dashboard, Storage).

/// Build `sudo smartctl <args> <device>` as a blocking [`std::process::Command`].
///
/// For use inside `spawn_blocking`. The `sudo` prefix must not be stripped —
/// it is what makes the call work on `660 root:disk` devices, and the install
/// grants exactly `smartctl` (with volatile args) to the service user.
pub fn std_command(device: &str, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("sudo");
    command.arg("smartctl").args(args).arg(device);
    command
}

/// Build `sudo smartctl <args> <device>` as an async [`tokio::process::Command`].
///
/// For use from async handlers that run the command eagerly (`.output().await`).
/// Note: an async handler must still not run this on the runtime for a slow
/// query — offload to `spawn_blocking` on the patterns that stream or poll
/// (`run_smartctl`-style one-shot probes are the sanctioned exception).
pub fn tokio_command(device: &str, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("sudo");
    command.arg("smartctl").args(args).arg(device);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- std_command: the argv shape sudoers grants are written against ----

    /// Both command kinds expose the same `get_program`/`get_args` shape; a
    /// local trait keeps the two assertions identical.
    trait CommandLike {
        fn argv(self) -> Vec<String>;
    }

    impl CommandLike for std::process::Command {
        fn argv(self) -> Vec<String> {
            std::iter::once(self.get_program().to_string_lossy().into_owned())
                .chain(self.get_args().map(|a| a.to_string_lossy().into_owned()))
                .collect()
        }
    }

    impl CommandLike for tokio::process::Command {
        fn argv(self) -> Vec<String> {
            // tokio's Command mirrors the spawn side only; `as_std` exposes
            // the argv reads.
            let std_command = self.as_std();
            std::iter::once(std_command.get_program().to_string_lossy().into_owned())
                .chain(
                    std_command
                        .get_args()
                        .map(|a| a.to_string_lossy().into_owned()),
                )
                .collect()
        }
    }

    #[test]
    fn test_std_command_places_smartctl_under_sudo_and_device_last() {
        let actual = std_command("/dev/sda", &["-d", "sat", "-H"]).argv();
        assert_eq!(
            actual,
            vec!["sudo", "smartctl", "-d", "sat", "-H", "/dev/sda"]
        );
    }

    #[test]
    fn test_tokio_command_has_the_same_argv_shape() {
        let actual = tokio_command("/dev/nvme0", &["-H"]).argv();
        assert_eq!(actual, vec!["sudo", "smartctl", "-H", "/dev/nvme0"]);
    }
}
