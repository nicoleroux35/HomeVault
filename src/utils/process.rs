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
use std::process::{Command, ExitStatus, Output, Stdio};
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
    run_timed_inner(cmd, deadline, StdinInput::Closed, true)
}

/// [`run_timed`], feeding `input` to the child's stdin.
///
/// `sudo tee /etc/fstab` is why this exists: the content is small, and routing
/// it through a temp file would leave a copy of a system config on disk. It
/// cannot go through [`run_timed`] either — that one closes stdin, and `tee`
/// would then write an **empty** file, silently truncating the target.
pub fn run_timed_with_stdin(
    cmd: &mut Command,
    deadline: Duration,
    input: &[u8],
) -> Result<Output, String> {
    run_timed_inner(cmd, deadline, StdinInput::Bytes(input), true)
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
    cmd.stdout(Stdio::from(sink));
    // stdout is the caller's file, not a pipe to drain.
    run_timed_inner(cmd, deadline, StdinInput::Closed, false)
}

/// Run a command with **both** streams detached (`/dev/null`) under a hard
/// deadline, killing the child on expiry. Only the exit status comes back.
///
/// `openvpn --daemon` is why this exists: the parent forks and exits at once,
/// but the daemon keeps the inherited write ends of any pipe we hand it, so
/// [`run_timed`]'s drain threads never see EOF and its deadline can never
/// fire. There is nothing to read here anyway — the daemon's own output goes
/// to its `--log` file — so both streams are `/dev/null` and the caller reads
/// the log itself.
pub fn run_timed_detached(cmd: &mut Command, deadline: Duration) -> Result<ExitStatus, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run {}: {}", program, e))?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if started.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
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
                return Err(format!("Failed to wait for {}: {}", program, e));
            }
        }
    }
}

/// How the child's stdin is fed: closed, or a byte slice written by a thread.
enum StdinInput<'a> {
    Closed,
    Bytes(&'a [u8]),
}

fn run_timed_inner(
    cmd: &mut Command,
    deadline: Duration,
    stdin: StdinInput<'_>,
    pipe_stdout: bool,
) -> Result<Output, String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    match stdin {
        StdinInput::Bytes(_) => {
            cmd.stdin(Stdio::piped());
        }
        StdinInput::Closed => {
            cmd.stdin(Stdio::null());
        }
    }
    // `run_timed_to_file` has already pointed stdout at its sink; leave it be.
    if pipe_stdout {
        cmd.stdout(Stdio::piped());
    }
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run {}: {}", program, e))?;

    // The input is written by its own thread: a child that reads nothing (or
    // dies first) would otherwise block this one on a full pipe before the
    // deadline loop ever starts.
    if let StdinInput::Bytes(input) = stdin {
        let mut pipe = child.stdin.take();
        let owned = input.to_vec();
        std::thread::spawn(move || {
            use std::io::Write as _;
            if let Some(pipe) = pipe.as_mut() {
                let _ = pipe.write_all(&owned);
            }
            // Dropping the handle closes stdin, which is what `tee` waits for.
        });
    }

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

/// [`run_timed_async_result`], feeding `input` to the child's stdin.
///
/// The async twin of [`run_timed_with_stdin`], for `sudo tee /etc/fstab` inside
/// an `async fn` (the RAID/pool persistence path). Same deadline, same
/// `kill_on_drop`: a `sudo tee` that never returns must not outlive the job
/// that started it while holding `PERSIST_LOCK`.
pub async fn run_timed_async_with_stdin(
    cmd: &mut tokio::process::Command,
    deadline: Duration,
    input: &[u8],
) -> Result<Output, ProcessFailure> {
    use tokio::io::AsyncWriteExt as _;

    let program = cmd.as_std().get_program().to_string_lossy().into_owned();
    cmd.kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| ProcessFailure::Spawn {
        program: program.clone(),
        error: e.to_string(),
    })?;
    if let Some(mut pipe) = child.stdin.take() {
        // A child that exits before reading everything turns this into a
        // broken pipe; that is not the failure we are reporting, the exit
        // status is.
        let _ = pipe.write_all(input).await;
        let _ = pipe.shutdown().await;
    }

    match tokio::time::timeout(deadline, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(ProcessFailure::Spawn {
            program,
            error: e.to_string(),
        }),
        Err(_) => Err(ProcessFailure::Timeout { program, deadline }),
    }
}

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
    fn run_timed_to_file_does_not_pipe_stdout_back() {
        // The `.gz` extraction path: `gunzip -c archive` writes to the sink, so
        // there is nothing to drain on our side and the caller reads the file.
        let dir = tempfile::tempdir().expect("temp dir");
        let sink = dir.path().join("out.bin");
        let file = std::fs::File::create(&sink).expect("create sink");
        let mut cmd = Command::new("echo");
        cmd.arg("streamed");
        let out = run_timed_to_file(&mut cmd, Duration::from_secs(5), file).expect("echo finishes");
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
        assert_eq!(
            std::fs::read_to_string(&sink).expect("read sink").trim(),
            "streamed"
        );
    }

    #[test]
    fn run_timed_with_stdin_feeds_the_child_and_keeps_stdout_piped() {
        // `sudo tee /etc/fstab` is the caller: `tee` copies stdin to stdout, so
        // the content must both reach the child and come back on our pipe.
        let mut cmd = Command::new("cat");
        let out = run_timed_with_stdin(&mut cmd, Duration::from_secs(5), b"# marker\nentry\n")
            .expect("cat must finish");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "# marker\nentry\n");
    }

    #[test]
    fn run_timed_with_stdin_does_not_deadlock_when_the_child_ignores_stdin() {
        // The input is written by its own thread: a child that never reads it
        // must still be waited out (and killed) instead of blocking the caller
        // on a full pipe before the deadline loop even starts.
        let started = Instant::now();
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let err = run_timed_with_stdin(&mut cmd, Duration::from_millis(150), &[b'x'; 256 * 1024])
            .expect_err("a hung child must not succeed");
        assert!(err.contains("did not finish within"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn run_timed_async_with_stdin_feeds_the_child_and_bounds_it() {
        // The `sudo tee /etc/fstab` path of the RAID/pool persistence code: the
        // content reaches the child, and a child that never reads it is killed
        // at the deadline instead of holding the job that started it.
        let mut cmd = tokio::process::Command::new("cat");
        let out = run_timed_async_with_stdin(&mut cmd, Duration::from_secs(5), b"a\nb\n")
            .await
            .expect("cat must finish");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "a\nb\n");

        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("30");
        let err = run_timed_async_with_stdin(&mut cmd, Duration::from_millis(120), &[b'x'; 65536])
            .await
            .expect_err("a hung child must not succeed");
        assert!(matches!(err, ProcessFailure::Timeout { .. }), "{err:?}");
    }

    #[test]
    fn run_timed_with_stdin_reports_a_missing_program() {
        let mut cmd = Command::new("homevault-no-such-binary");
        let err = run_timed_with_stdin(&mut cmd, Duration::from_secs(1), b"x")
            .expect_err("a missing program is an error");
        assert!(err.contains("Failed to run"), "{err}");
    }

    #[test]
    fn run_timed_detached_returns_a_fast_command_status() {
        let status = run_timed_detached(&mut Command::new("true"), Duration::from_secs(5))
            .expect("true must finish");
        assert!(status.success());
        let status = run_timed_detached(&mut Command::new("false"), Duration::from_secs(5))
            .expect("false still runs to completion");
        assert_eq!(status.code(), Some(1));
    }

    #[test]
    fn run_timed_detached_kills_a_command_that_overruns_its_deadline() {
        // `openvpn --daemon`'s parent normally exits at once; a parent that
        // never does must not park the caller (`start_openvpn_connection`)
        // forever.
        let started = Instant::now();
        let err = run_timed_detached(
            &mut Command::new("sleep").arg("30"),
            Duration::from_millis(150),
        )
        .expect_err("a hung command must not succeed");
        assert!(err.contains("did not finish within"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn run_timed_detached_reports_a_missing_program() {
        let err = run_timed_detached(
            &mut Command::new("homevault-no-such-binary"),
            Duration::from_secs(1),
        )
        .expect_err("a missing program is an error");
        assert!(err.contains("Failed to run"), "{err}");
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

    // -----------------------------------------------------------------
    // ROADMAP X.9 — the tree-wide source guard
    // -----------------------------------------------------------------

    /// Every `Command::new` of `src/` must reach a bounded invocation.
    ///
    /// X.2 claimed this tree-wide and only enforced it in the modules that got
    /// their own guard (`mdns`, `shares`, `health`, `disk`, …), so 98 sites
    /// survived across 26 files. This guard is the missing global one: it walks
    /// `src/`, finds every `Command::new(…)`, and looks at the next few lines
    /// for an invocation terminator (`.output()`, `.status()`, `.spawn()`,
    /// `wait_with_output()`) that is **not** wrapped by `run_timed…`, by one of
    /// the named bounded spawn helpers, or by an explicit
    /// `tokio::time::timeout`.
    ///
    /// Two families are legitimately exempt, and they are listed rather than
    /// discovered — a new exemption must be added here on purpose:
    ///
    /// 1. **Long-running children** whose lifetime is the work itself
    ///    (`ddrescue`, `photorec`, `clamscan`, the TestDisk scan): they are
    ///    cancelled through their process group and carry `kill_on_drop`, not a
    ///    deadline.
    /// 2. **The helpers that build a command for a caller** (`smart::std_command`
    ///    / `tokio_command`, `encryption::run`, the test shims): the bound lives
    ///    at the call site, which this guard also scans.
    /// Every entry carries the reason it exists; the guard prints the file and
    /// the line, so a new exemption is a deliberate edit here.
    const LONG_RUNNING_CHILDREN: &[(&str, &str)] = &[
        // A full-drive ClamAV pass: cancelled through `cancel_scan()`, and the
        // spawn carries `kill_on_drop`.
        ("antivirus.rs", "clamscan"),
        // A disk clone / a file recovery: they run for hours by design and are
        // cancelled through their process group (`process_group(0)` + the
        // bounded `sudo kill`).
        ("rescue/service.rs", "ddrescue"),
        ("rescue/service.rs", "photorec"),
        // The TestDisk scan spawns then waits under `TESTDISK_REBUILD_TIMEOUT`
        // immediately afterwards (`child.wait_with_output()` inside
        // `tokio::time::timeout`). Matched on the `sudo` wrapper, since the
        // argv that names the tool is spread over the following lines.
        ("rescue/service.rs", "/cmd"),
        // Builders, not callers: `std_command`/`tokio_command` hand the command
        // to a caller whose own call site is bounded (`disk.rs::bounded`,
        // `dashboard.rs`, `health.rs`, the scheduler), and the guard scans those
        // too.
        ("smart.rs", "sudo"),
        // Same shape: `encryption.rs::run` is the module's bounded runner.
        ("encryption.rs", ""),
    ];

    fn src_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// A `Command::new` is bounded when one of these appears before the
    /// invocation terminator.
    fn is_bounded(window: &str) -> bool {
        [
            "run_timed",
            "run_timed_async",
            "run_timed_with_stdin",
            "run_timed_async_with_stdin",
            "run_timed_to_file",
            "run_timed_detached",
            "spawn_under_deadline",
            "spawn_child",
            "tokio::time::timeout",
            "acl_run",
            "grp_run",
            "sftp_run",
            "pam_run",
            "run_probe",
        ]
        .iter()
        .any(|needle| window.contains(needle))
    }

    #[test]
    fn every_subprocess_of_the_tree_is_bounded() {
        let mut files = Vec::new();
        rust_files(&src_dir(), &mut files);
        assert!(
            files.len() > 40,
            "the sweep found only {} files — it is not walking src/",
            files.len()
        );

        let mut offenders = Vec::new();
        for path in &files {
            let rel = path
                .strip_prefix(src_dir())
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let text = std::fs::read_to_string(path).expect("read source");
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if !line.contains("Command::new(") {
                    continue;
                }
                // The helper definitions themselves, and this guard's own text.
                if rel.contains("utils/process.rs") {
                    continue;
                }
                let window = lines[i..(i + 14).min(lines.len())].join("\n");
                // Plain substring checks — the terminators are literal enough
                // and a regex dependency would be overkill for a source guard.
                let invokes = [".output()", ".status()", ".spawn()", "wait_with_output()"]
                    .iter()
                    .any(|needle| window.contains(needle));
                if !invokes || is_bounded(&window) {
                    continue;
                }
                // The needle may sit on a later line of a multi-line `args([…])`
                // (the TestDisk argv, for instance), so match against the whole
                // window rather than the `Command::new` line alone.
                let exempt = LONG_RUNNING_CHILDREN.iter().any(|(file, needle)| {
                    rel.ends_with(file) && (needle.is_empty() || window.contains(needle))
                });
                if exempt {
                    continue;
                }
                offenders.push(format!("{rel}:{}", i + 1));
            }
        }

        assert!(
            offenders.is_empty(),
            "these `Command::new` invocations have no deadline (ROADMAP X.9) — \
             route them through `utils::process::run_timed*` or add a named \
             exemption with its reason:\n{}",
            offenders.join("\n")
        );
    }

    /// The guard must actually bite: a deliberately unbounded call is caught.
    /// Checked on a sample line rather than by editing `src/`, so the test
    /// cannot leave the tree in a broken state.
    #[test]
    fn the_source_guard_detects_an_unbounded_call() {
        let unbounded = "let out = Command::new(\"df\").arg(\"/\").output()?;";
        let window = unbounded.to_string();
        assert!(!is_bounded(&window));
        assert!(window.contains(".output()"));

        let bounded = "run_timed(Command::new(\"df\").arg(\"/\"), T).ok()?;";
        assert!(is_bounded(bounded));
    }

    /// The exemption list must not become a dumping ground: every entry names a
    /// real file and a real reason, and none of them is `utils/process.rs`
    /// itself.
    #[test]
    fn every_exemption_names_a_real_file() {
        for (file, _needle) in LONG_RUNNING_CHILDREN {
            let path = src_dir().join(file);
            assert!(
                path.is_file(),
                "exemption for {file} points at a file that does not exist"
            );
        }
        assert!(
            LONG_RUNNING_CHILDREN.len() <= 8,
            "the exemption list is growing without review"
        );
    }
}
