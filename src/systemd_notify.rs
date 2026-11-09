/* =============================================================================
 * File          : systemd_notify.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   `sd_notify`-style readiness and watchdog heartbeat (FIAB-02).
 *
 *   The systemd unit sets `WatchdogSec=30` and `NotifyAccess=main`. If the
 *   process stops answering — a blocked Tokio runtime, a wedged scheduler, a
 *   deadlock in a background loop — systemd restarts it instead of leaving a
 *   half-alive appliance that still holds the TCP port.
 *
 *   Everything here is best-effort: when HomeVault is started by hand (no
 *   `NOTIFY_SOCKET`), every call is a no-op. No new dependency and no root:
 *   the notification socket is a Unix datagram provided by systemd.
 *
 * Dependencies:
 *   - tokio (heartbeat task), std (UnixDatagram / SocketAddr)
 *
 * Usage:
 *   `ready()` after the listener is bound, then `spawn_heartbeat()`; see
 *   `src/web/mod.rs` and `installer/install.sh` (`render_service_unit`).
 * =============================================================================
 */

use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::Duration;

/// How often the heartbeat answers systemd. The unit's `WatchdogSec=30` gives
/// three missed heartbeats before a restart, which tolerates a long backup
/// without hiding a real freeze.
pub const WATCHDOG_INTERVAL: Duration = Duration::from_secs(10);

/// One `sd_notify` payload is newline-separated `KEY=value` lines and ends
/// with a newline. Pure, so the exact wire shape is unit-testable.
pub fn wire(payload: &str) -> String {
    format!("{payload}\n")
}

/// `/run/systemd/notify` is a filesystem path, but `NOTIFY_SOCKET` may also
/// be an abstract socket (`@name`). A malformed value is not an error: the
/// process simply runs without systemd notifications.
fn destination(socket: &str) -> Option<SocketAddr> {
    if let Some(name) = socket.strip_prefix('@') {
        if name.is_empty() {
            return None;
        }
        SocketAddr::from_abstract_name(name.as_bytes()).ok()
    } else {
        SocketAddr::from_pathname(socket).ok()
    }
}

/// Sends one notification. Returns `false` when there is no systemd socket or
/// when the datagram cannot be written; callers never treat that as fatal.
pub fn notify(payload: &str) -> bool {
    let Ok(socket) = std::env::var("NOTIFY_SOCKET") else {
        return false;
    };
    if socket.is_empty() {
        return false;
    }
    let Some(destination) = destination(&socket) else {
        return false;
    };
    let Ok(sender) = UnixDatagram::unbound() else {
        return false;
    };
    sender
        .send_to_addr(wire(payload).as_bytes(), &destination)
        .is_ok()
}

/// Tells systemd the service is up. `Type=simple` does not require `READY=1`,
/// but sending it makes `systemctl start` wait for the listener and gives a
/// truthful `STATUS=` line in `systemctl status`.
pub fn ready() -> bool {
    notify("READY=1\nSTATUS=HomeVault is ready")
}

/// One watchdog tick.
pub fn heartbeat() -> bool {
    notify("WATCHDOG=1")
}

/// Starts the heartbeat loop when systemd gave us a notification socket.
/// The returned handle is intentionally detached: if the runtime is blocked,
/// the loop stops, which is exactly the signal systemd must see.
pub fn spawn_heartbeat() -> Option<tokio::task::JoinHandle<()>> {
    if std::env::var_os("NOTIFY_SOCKET").is_none() {
        return None;
    }
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(WATCHDOG_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if !heartbeat() {
                // The socket disappeared (systemd reloaded or stopped us):
                // stop quietly instead of spinning on a dead fd.
                break;
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_newline_terminated_and_keeps_every_line() {
        assert_eq!(wire("WATCHDOG=1"), "WATCHDOG=1\n");
        assert_eq!(
            wire("READY=1\nSTATUS=HomeVault is ready"),
            "READY=1\nSTATUS=HomeVault is ready\n"
        );
    }

    #[test]
    fn destination_accepts_a_path_and_an_abstract_socket() {
        let path = std::env::temp_dir().join("hv-notify-test.sock");
        assert!(destination(&path.to_string_lossy()).is_some());
        assert!(destination("@homevault-test").is_some());
        assert!(destination("@").is_none());
        assert!(destination("").is_none());
    }

    #[test]
    fn notify_is_a_noop_without_a_systemd_socket() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::remove_var("NOTIFY_SOCKET");
        assert!(!notify("WATCHDOG=1"));
        assert!(spawn_heartbeat().is_none());
    }

    #[tokio::test]
    async fn a_real_datagram_is_delivered_to_the_notify_socket() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("notify.sock");
        let receiver = UnixDatagram::bind(&path).expect("bind notify socket");
        std::env::set_var("NOTIFY_SOCKET", &path);

        assert!(heartbeat());
        let mut buffer = [0u8; 128];
        let size = receiver.recv(&mut buffer).expect("receive heartbeat");
        assert_eq!(&buffer[..size], b"WATCHDOG=1\n");

        assert!(ready());
        let size = receiver.recv(&mut buffer).expect("receive ready");
        assert_eq!(&buffer[..size], b"READY=1\nSTATUS=HomeVault is ready\n");

        std::env::remove_var("NOTIFY_SOCKET");
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
