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
 *   `smartctl`, `sensors`, `dpkg-query`, `mdadm`, `zpool` — goes through it,
 *   as does every long-running rclone operation (probe, transfer, unmount).
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

/// Deadline for archive tools (`tar`, `zip`, `7z`, `gunzip`).
///
/// A ceiling, not an estimate: the first extraction of a large library
/// legitimately takes minutes on a spinning disk, so this cannot be a probe
/// budget — but a wedged tool must not hold its `spawn_blocking` thread (and
/// the job behind it) forever.
pub const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Run a blocking command with a hard deadline.
///
/// Output is drained on dedicated threads (a child that fills a pipe buffer
/// would otherwise deadlock before it exits), the deadline is enforced by
/// polling `try_wait`, and the child is killed and reaped on expiry. The
/// error string names the program and the deadline, which is what the caller
/// logs — a probe that times out is a *gap*, never a fabricated value.
pub fn run_timed(cmd: &mut Command, deadline: Duration) -> Result<Output, String> {
    run_timed_inner(cmd, deadline, None)
}

/// [`run_timed`] with the child's stdout pointed at a file instead of a pipe.
///
/// Extracting a `.gz` archive means `gunzip -c archive > out_file`: the data
/// must not be drained through this process. `Output.stdout` is therefore
/// empty by construction and the caller reads the file.
pub fn run_timed_to_file(
    cmd: &mut Command,
    deadline: Duration,
    sink: std::fs::File,
) -> Result<Output, String> {
    run_timed_inner(cmd, deadline, Some(Stdio::from(sink)))
}

fn run_timed_inner(
    cmd: &mut Command,
    deadline: Duration,
    stdout: Option<Stdio>,
) -> Result<Output, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null());
    match stdout {
        Some(sink) => {
            cmd.stdout(sink);
        }
        None => {
            cmd.stdout(Stdio::piped());
        }
    }
    cmd.stderr(Stdio::piped());

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
    run_timed_async_result(cmd, deadline).await.ok()
}

/// Why a [`run_timed_async_result`] call did not produce output.
///
/// The two cases must stay distinguishable: "the tool is not installed" and
/// "the tool took too long" need different words in front of a user, even
/// though both are a failure the caller has to handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessFailure {
    /// The command could not be started at all (not installed, not
    /// executable, …).
    Spawn { program: String, error: String },
    /// The deadline expired. The child has already been killed and reaped.
    Timeout { program: String, deadline: Duration },
}

impl std::fmt::Display for ProcessFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { program, error } => write!(f, "Failed to run {}: {}", program, error),
            Self::Timeout { program, deadline } => write!(
                f,
                "{} did not finish within {} seconds",
                program,
                deadline.as_secs()
            ),
        }
    }
}

impl std::error::Error for ProcessFailure {}

/// [`run_timed_async`], but keeping *why* it failed.
///
/// Same contract — `kill_on_drop` plus `tokio::time::timeout`, output drained
/// by the runtime — with a named [`ProcessFailure`] instead of a bare `None`.
/// Use this when the error reaches a user (rclone transfers, mounts) and the
/// `None` form when a missing answer is simply "no data" (host probes).
pub async fn run_timed_async_result(
    cmd: &mut tokio::process::Command,
    deadline: Duration,
) -> Result<Output, ProcessFailure> {
    let program = cmd.as_std().get_program().to_string_lossy().into_owned();
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match tokio::time::timeout(deadline, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(ProcessFailure::Spawn {
            program,
            error: e.to_string(),
        }),
        Err(_) => Err(ProcessFailure::Timeout { program, deadline }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_timed_returns_the_output_of_a_fast_command() {
        let out = run_timed(Command::new("echo").arg("hello"), Duration::from_secs(5))
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
        let err = run_timed(Command::new("sleep").arg("30"), Duration::from_millis(150))
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
    fn run_timed_keeps_a_caller_supplied_stdout() {
        // `gunzip -c archive > out_file` (and the Explorer's extraction) hands
        // the child a file as stdout. Filling in `piped()` unconditionally
        // would silently send the data to a pipe nobody reads and leave an
        // empty file behind.
        let dir = tempfile::tempdir().expect("temp dir");
        let sink = dir.path().join("out.txt");
        let file = std::fs::File::create(&sink).expect("create sink");
        let mut cmd = Command::new("echo");
        cmd.arg("redirected");

        let out =
            run_timed_to_file(&mut cmd, Duration::from_secs(5), file).expect("echo must finish");
        assert!(out.status.success());
        assert!(
            out.stdout.is_empty(),
            "nothing is left to drain on our side"
        );
        assert_eq!(
            std::fs::read_to_string(&sink).expect("read sink").trim(),
            "redirected"
        );
    }

    #[test]
    fn archive_deadline_is_a_ceiling_not_a_probe_budget() {
        // Long enough for a real extraction, still finite so a wedged tool
        // cannot pin a blocking thread forever.
        assert!(ARCHIVE_TIMEOUT >= Duration::from_secs(60));
        assert!(ARCHIVE_TIMEOUT <= Duration::from_secs(2 * 60 * 60));
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

    // -- run_timed_async_result (the named failure) ----------------------

    #[tokio::test]
    async fn run_timed_async_result_returns_the_output_of_a_fast_command() {
        let mut cmd = tokio::process::Command::new("echo");
        cmd.arg("hello");
        let out = run_timed_async_result(&mut cmd, Duration::from_secs(5))
            .await
            .expect("echo must finish");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[tokio::test]
    async fn run_timed_async_result_keeps_a_failing_exit_status_instead_of_erroring() {
        // Same contract as `run_timed`: a non-zero exit is data for the caller.
        let mut cmd = tokio::process::Command::new("false");
        let out = run_timed_async_result(&mut cmd, Duration::from_secs(5))
            .await
            .expect("false still runs to completion");
        assert!(!out.status.success());
    }

    #[tokio::test]
    async fn run_timed_async_result_names_a_timeout_it_enforced() {
        let started = std::time::Instant::now();
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("30");
        let err = run_timed_async_result(&mut cmd, Duration::from_millis(150))
            .await
            .expect_err("a hung command must not succeed");
        match &err {
            ProcessFailure::Timeout { program, deadline } => {
                assert_eq!(program, "sleep");
                assert_eq!(deadline, &Duration::from_millis(150));
            }
            other => panic!("expected a named timeout, got {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must be enforced, not waited out"
        );
        assert!(err.to_string().contains("did not finish within 0 seconds"));
    }

    #[tokio::test]
    async fn run_timed_async_result_distinguishes_a_missing_program_from_a_timeout() {
        // The distinction is the point: "not installed" and "too slow" are
        // different sentences in front of a user.
        let mut cmd = tokio::process::Command::new("homevault-no-such-binary");
        let err = run_timed_async_result(&mut cmd, Duration::from_secs(1))
            .await
            .expect_err("a missing program is an error");
        match &err {
            ProcessFailure::Spawn { program, .. } => {
                assert_eq!(program, "homevault-no-such-binary")
            }
            other => panic!("expected a spawn failure, got {other:?}"),
        }
        assert!(err.to_string().starts_with("Failed to run"));
    }

    #[tokio::test]
    async fn run_timed_async_still_reduces_to_none_for_probes() {
        // `run_timed_async` is the "no data" wrapper; it must keep both
        // failure kinds as `None` rather than growing a third behaviour.
        let mut missing = tokio::process::Command::new("homevault-no-such-binary");
        assert!(run_timed_async(&mut missing, Duration::from_secs(1))
            .await
            .is_none());
        let mut hung = tokio::process::Command::new("sleep");
        hung.arg("30");
        assert!(run_timed_async(&mut hung, Duration::from_millis(150))
            .await
            .is_none());
    }
}
