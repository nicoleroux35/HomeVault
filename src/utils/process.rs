/* =============================================================================
 * File          : process.rs
 * Author        : Nicolas Le Roux
 * Created       : 23/10/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One implementation of "run a host command, but never wait for it
 *   forever": a blocking helper (for `spawn_blocking` code) and an async
 *   helper (for `tokio::process`) that both kill the child at a deadline and
 *   report the timeout as a normal failure instead of hanging the caller.
 *   Every dashboard probe — `nvidia-smi`, `rocm-smi`, `ip`, `lsblk`, `df`,
 *   `smartctl`, `sensors`, `dpkg-query`, `mdadm`, `zpool` — goes through it.
 *
 * Dependencies:
 *   - tokio (>= 1, `process` + `time` + `rt` features)
 *
 * Usage:
 *   `use crate::utils::process::{run_timed, run_timed_async, HOST_PROBE_TIMEOUT};`
 *   The blocking variant must only be called from a `spawn_blocking` task.
 * =============================================================================
 */

//! Bounded subprocess helpers: a deadline on every host probe, with the child
//! killed (not merely abandoned) when the deadline expires.

use std::io::Read as _;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Default deadline for a one-shot host probe (SMART status/temperature,
/// `lsblk`, `df`, `ip`, `nvidia-smi`, `sensors`, `dpkg-query`, …).
///
/// Long enough for a healthy spinning disk to answer a SMART query on a busy
/// NAS, short enough that a wedged drive or a hung NFS mount surfaces as "no
/// data" instead of piling requests up behind it.
pub const HOST_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for `smartctl` queries, which are the slowest probe on the box
/// (a drive can spin up on the first query after idle). Kept separate so the
/// cheap probes stay snappy while SMART still gets a fair chance to answer.
pub const SMART_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Run a blocking command with a hard deadline.
///
/// Output is drained on dedicated threads (a child that fills a pipe buffer
/// would otherwise deadlock before it exits), the deadline is enforced by
/// polling `try_wait`, and the child is killed and reaped on expiry. The
/// error string names the program and the deadline, which is what the caller
/// logs — a probe that times out is a *gap*, never a fabricated value.
pub fn run_timed(cmd: &mut Command, deadline: Duration) -> Result<Output, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run {}: {}", program, e))?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = out_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = err_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_reader.join();
                    let _ = err_reader.join();
                    return Err(format!(
                        "{} did not finish within {} seconds",
                        program,
                        deadline.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err(format!("Failed to wait for {}: {}", program, e));
            }
        }
    };

    Ok(Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    })
}

/// Run an async (tokio) command with a hard deadline.
///
/// Unlike dropping a bare `.output()` future, the child is killed when the
/// deadline expires (`kill_on_drop`), so a `smartctl` stuck on a failing disk
/// does not survive as an orphan. `None` means "no answer in time or the
/// command could not be spawned" — the caller must treat that as missing data.
pub async fn run_timed_async(
    cmd: &mut tokio::process::Command,
    deadline: Duration,
) -> Option<Output> {
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match tokio::time::timeout(deadline, cmd.output()).await {
        Ok(Ok(output)) => Some(output),
        // Spawn failure and timeout are both "no data"; the timeout case has
        // already killed the child via `kill_on_drop`.
        Ok(Err(_)) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_timed_returns_the_output_of_a_fast_command() {
        let out = run_timed(&mut Command::new("echo").arg("hello"), Duration::from_secs(5))
            .expect("echo must finish");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[test]
    fn run_timed_keeps_a_failing_exit_status_instead_of_erroring() {
        // A non-zero exit is data (the caller reads stdout/stderr); only a
        // timeout or a spawn failure is an error.
        let out = run_timed(&mut Command::new("false"), Duration::from_secs(5))
            .expect("false still runs to completion");
        assert!(!out.status.success());
    }

    #[test]
    fn run_timed_kills_a_command_that_overruns_its_deadline() {
        let started = Instant::now();
        let err = run_timed(
            &mut Command::new("sleep").arg("30"),
            Duration::from_millis(150),
        )
        .expect_err("a hung command must not succeed");
        assert!(err.contains("did not finish within"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must be enforced, not waited out"
        );
    }

    #[test]
    fn run_timed_reports_a_missing_program() {
        let err = run_timed(
            &mut Command::new("homevault-no-such-binary"),
            Duration::from_secs(1),
        )
        .expect_err("a missing program is an error");
        assert!(err.contains("Failed to run"), "{err}");
    }

    #[test]
    fn probe_deadlines_are_bounded_and_ordered() {
        // SMART gets the longest slot because a spinning disk may need to spin
        // up; both must stay well under a client poll interval so requests
        // cannot pile up behind one another.
        assert!(HOST_PROBE_TIMEOUT >= Duration::from_secs(1));
        assert!(SMART_PROBE_TIMEOUT >= HOST_PROBE_TIMEOUT);
        assert!(SMART_PROBE_TIMEOUT <= Duration::from_secs(30));
    }
}
