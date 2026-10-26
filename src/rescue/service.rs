/* =============================================================================
 * File          : service.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Rescue tool execution service: runs ddrescue (disk cloning with
 *   progress), TestDisk (analyze/rebuild), PhotoRec (file carving) and
 *   mdadm (diagnose/repair) as background processes, after the
 *   SecurityValidator cleared the request, streaming progress where
 *   applicable.
 *
 * Dependencies:
 *   - tokio (>= 1.52), internal validator + models
 *
 * Usage:
 *   handlers/rescue.rs (see docs/guides/rescue.md).
 * =============================================================================
 */

//! Rescue tool execution (ddrescue, TestDisk, PhotoRec, mdadm),
//! validator-gated, with progress streaming.

use super::jobs;
use super::models::*;
use super::validator::{ClonePlan, SecurityValidator, check_clone_safety};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tracing::warn;

/// Ceiling for the `--version` probes behind `GET /rescue/tools`. A missing
/// tool fails immediately; a wedged one must not hang the request.
const TOOL_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling for `lsblk` (device list, sizes, usage).
const LSBLK_TIMEOUT: Duration = Duration::from_secs(15);
/// Ceiling for `testdisk /list` (read-only partition enumeration).
const TESTDISK_ANALYZE_TIMEOUT: Duration = Duration::from_secs(120);
/// Ceiling for the scripted `testdisk /cmd … analyze,search` deep scan.
const TESTDISK_REBUILD_TIMEOUT: Duration = Duration::from_secs(300);
/// Ceiling for any `mdadm` call (both are prompt; a hung one is a kernel issue).
const MDADM_TIMEOUT: Duration = Duration::from_secs(60);

// Note on the two long-running children (`ddrescue`, `photorec`): they are
// deliberately NOT deadlined — a clone or a recovery runs for as long as it
// takes, and it is cancelled through its process group. They go through
// `spawn_child`, which adds `kill_on_drop` so a dropped guard cannot leave a
// write running. That is the one sanctioned exception to "every subprocess is
// bounded" in this module, and it is a choice, not an oversight (ROADMAP X.9).

pub struct RescueService;

/// Starts a long-running rescue child (`sudo ddrescue`, `sudo photorec`).
///
/// The child's *lifetime* is deliberately **not** bounded here: a disk clone or
/// a file recovery legitimately runs for hours, and it is cancelled through its
/// process group (`process_group(0)` + the `sudo kill` path), not through a
/// deadline. What this wrapper adds is `kill_on_drop`, so a cancellation that
/// drops the guard cannot leave the tool running, and one named error type for
/// the "could not start" case (ROADMAP X.9 — these two `spawn()` calls were the
/// last ones outside the module's bounded discipline; the surrounding
/// `RESCUE_SPAWN_TIMEOUT` documentation explains why a deadline is the wrong
/// instrument here).
fn spawn_child(cmd: &mut Command) -> Result<tokio::process::Child, RescueError> {
    cmd.kill_on_drop(true);
    cmd.spawn().map_err(RescueError::Io)
}

impl RescueService {
    /// Run `cmd` to completion under a hard timeout, killing it on expiry.
    ///
    /// Every subprocess in this module goes through here: a stuck `testdisk`,
    /// `mdadm` or `lsblk` used to leave the HTTP request pending forever (no
    /// timeout anywhere in the pre-2026-10-21 code except the TestDisk scan).
    async fn run_output(
        mut cmd: Command,
        timeout: Duration,
        what: &str,
    ) -> Result<std::process::Output, RescueError> {
        cmd.kill_on_drop(true);
        match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(e)) => Err(RescueError::Io(e)),
            Err(_) => Err(RescueError::Parse(format!(
                "{what} did not finish within {}s and was stopped",
                timeout.as_secs()
            ))),
        }
    }

    pub async fn check_tools() -> Vec<ToolStatus> {
        let mut statuses = Vec::new();

        for tool in ["ddrescue", "testdisk", "photorec", "mdadm"] {
            let mut cmd = Command::new(tool);
            cmd.arg("--version");
            statuses.push(
                match Self::run_output(cmd, TOOL_PROBE_TIMEOUT, tool).await {
                    Ok(o) if o.status.success() => {
                        // mdadm writes its version banner to stderr, not
                        // stdout, unlike the other three tools.
                        let text = if tool == "mdadm" {
                            &o.stderr
                        } else {
                            &o.stdout
                        };
                        ToolStatus {
                            name: tool.to_string(),
                            available: true,
                            version: String::from_utf8_lossy(text)
                                .lines()
                                .next()
                                .map(|l| l.trim().to_string()),
                        }
                    }
                    _ => ToolStatus {
                        name: tool.to_string(),
                        available: false,
                        version: None,
                    },
                },
            );
        }

        statuses
    }

    pub async fn list_block_devices() -> Result<Vec<BlockDevice>, RescueError> {
        let mut devices = Vec::new();

        // `-P` (key="value" pairs) instead of a positional column list: the
        // MODEL field routinely contains spaces ("TOSHIBA MQ01ABF050"), which
        // `split_whitespace()` on a plain `-o` list shifts every following
        // column by — the old code read the VENDOR as the model, the MODEL as
        // the transport, and the TRAN string as the "read-only" flag (and
        // then reported RO, not RM, as `removable`). `-b` gives SIZE in bytes
        // so there's no locale-formatted "465,8G" to parse.
        let mut lsblk = Command::new("lsblk");
        lsblk.args(["-d", "-n", "-b", "-P", "-o", "NAME,SIZE,TYPE,MODEL,TRAN,RM"]);
        let output = Self::run_output(lsblk, LSBLK_TIMEOUT, "lsblk").await?;

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let field = |key: &str| -> String {
                line.split(&format!("{}=\"", key))
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or("")
                    .to_string()
            };
            if field("TYPE") != "disk" {
                continue;
            }
            let name = field("NAME");
            if name.is_empty() || crate::disk::DiskService::is_pseudo_disk_name(&name) {
                continue;
            }
            let transport = match field("TRAN") {
                t if t.is_empty() => "unknown".to_string(),
                t => t,
            };
            devices.push(BlockDevice {
                device: format!("/dev/{}", name),
                name,
                size: field("SIZE").parse::<u64>().unwrap_or(0),
                model: field("MODEL").trim().to_string(),
                transport,
                removable: field("RM") == "1",
            });
        }

        Ok(devices)
    }

    /// Block-device byte size, from `lsblk -b`. `None` when it can't be read
    /// (the clone-safety check degrades to a warning, never to a panic).
    async fn device_size(device: &Path) -> Option<u64> {
        let mut cmd = Command::new("lsblk");
        cmd.args(["-b", "-n", "-d", "-o", "SIZE"]).arg(device);
        let out = Self::run_output(cmd, LSBLK_TIMEOUT, "lsblk").await.ok()?;
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()?
            .trim()
            .parse::<u64>()
            .ok()
    }

    /// Why `device` must not be overwritten right now, if it must not.
    ///
    /// Two independent sources, because either alone misses real cases:
    /// - mounted filesystems / swap on the device **or any of its children**
    ///   (`lsblk`, so `/dev/sdb` with a mounted `sdb1` is caught);
    /// - sysfs `holders`: a device that backs an md array, an LVM volume or a
    ///   dm-crypt mapping has live holders but no mountpoint of its own —
    ///   cloning onto it corrupts the array.
    async fn device_in_use(device: &Path) -> Option<String> {
        let mut cmd = Command::new("lsblk");
        cmd.args(["-n", "-r", "-o", "NAME,TYPE,MOUNTPOINTS"])
            .arg(device);
        if let Ok(out) = Self::run_output(cmd, LSBLK_TIMEOUT, "lsblk").await {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() < 2 {
                    continue;
                }
                let (name, kind) = (fields[0], fields[1]);
                // Rows: `sdb disk /mnt/data` or `sdb1 part [SWAP]`.
                if let Some(mount) = fields.get(2) {
                    return Some(format!("{name} is mounted on {mount}"));
                }
                if matches!(
                    kind,
                    "md" | "raid0" | "raid1" | "raid4" | "raid5" | "raid6" | "raid10"
                ) {
                    return Some(format!("{name} is an active md array"));
                }
                if matches!(kind, "crypt" | "lvm") {
                    return Some(format!("{name} is an active {kind} mapping"));
                }
            }
        }

        // sysfs holders: the device backs something (md/LVM/dm), even though
        // it has no mountpoint of its own.
        if let Some(name) = device.file_name().and_then(|n| n.to_str()) {
            let holders = Path::new("/sys/class/block").join(name).join("holders");
            if let Ok(entries) = std::fs::read_dir(&holders) {
                let names: Vec<String> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                if !names.is_empty() {
                    return Some(format!(
                        "{} is in use by {}",
                        device.display(),
                        names.join(", ")
                    ));
                }
            }
        }

        None
    }

    /// Default ddrescue mapfile for a job.
    ///
    /// Under HomeVault's own root-owned `rescue/` directory rather than
    /// `/tmp`: the mapfile is written **as root** by ddrescue, and `/tmp` is
    /// world-writable, so a pre-planted symlink there would have redirected a
    /// root write. `validate_log_path` then checks whatever the admin passed
    /// explicitly.
    fn default_mapfile(job_id: &str) -> Result<PathBuf, RescueError> {
        let dir = crate::config::paths::rescue_dir();
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join(format!("ddrescue_{job_id}.log")))
    }

    /// The default mapfile path as a string, without creating anything — used
    /// by the handler to report the path it will use before the job runs.
    pub fn default_mapfile_display(job_id: &str) -> String {
        crate::config::paths::rescue_dir()
            .join(format!("ddrescue_{job_id}.log"))
            .to_string_lossy()
            .into_owned()
    }

    pub async fn ddrescue_clone(
        source: &str,
        destination: &str,
        options: &DdrescueOptions,
        job_id: &str,
    ) -> Result<(), RescueError> {
        use crate::sse::ProgressEmitter;

        // Both source and destination are raw block devices (the frontend only
        // ever offers a dropdown of real devices for either field) — reject
        // anything that isn't a real, non-protected block device before ever
        // touching it with a destructive disk-to-disk clone.
        let source_path = SecurityValidator::validate_block_device(source)?;
        let dest_path = SecurityValidator::validate_block_device(destination)?;

        options.validate()?;
        let block_size = options.validated_block_size()?;

        // Resolve the mapfile through the shared validator, **without**
        // creating its final component: ddrescue writes this file as root, so
        // a symlink planted at that path would be followed by a root write.
        // The path the handler reported is resolved (not canonicalized) for
        // exactly the same reason.
        let logfile = match &options.logfile {
            Some(lf) => SecurityValidator::validate_log_path(lf)?,
            None => Self::default_mapfile(job_id)?,
        };
        if let Some(parent) = logfile.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Belt and braces: if anything exists at the mapfile path already, it
        // must be a plain regular file.
        if let Ok(meta) = std::fs::symlink_metadata(&logfile) {
            if meta.file_type().is_symlink() {
                return Err(RescueError::Security(format!(
                    "Mapfile '{}' is a symbolic link — refusing to write through it",
                    logfile.display()
                )));
            }
            if !meta.is_file() {
                return Err(RescueError::Security(format!(
                    "Mapfile '{}' exists and is not a regular file",
                    logfile.display()
                )));
            }
        }

        // Safety net before a destructive write: same device, an in-use
        // destination (mounted/swap/md/LVM), or one smaller than the source.
        let plan = ClonePlan {
            source: source_path.clone(),
            destination: dest_path.clone(),
            source_size: Self::device_size(&source_path).await,
            destination_size: Self::device_size(&dest_path).await,
            destination_in_use: Self::device_in_use(&dest_path).await,
        };
        let warnings = check_clone_safety(&plan).map_err(RescueError::InvalidInput)?;
        for warning in &warnings {
            warn!("ddrescue: {}", warning);
            ProgressEmitter::progress(job_id, 0.0, warning);
        }

        ProgressEmitter::progress(
            job_id,
            0.0,
            &format!("Starting ddrescue: {} -> {}", source, destination),
        );

        // destination is always a real block device (validated above, never
        // a regular file) — ddrescue refuses to write to one without
        // --force ("Output file exists and is not a regular file"), so
        // every device-to-device clone would otherwise fail immediately.
        let mut args: Vec<String> = vec![
            "-v".to_string(),
            "--force".to_string(),
            "-b".to_string(),
            block_size.to_string(),
        ];

        if options.direct_io.unwrap_or(false) {
            args.push("--direct".to_string());
        }
        if options.reverse.unwrap_or(false) {
            args.push("--reverse".to_string());
        }
        if options.verbose.unwrap_or(false) {
            args.push("-v".to_string());
        }
        if let Some(retries) = options.retry_passes {
            if retries > 0 {
                args.push("-r".to_string());
                args.push(retries.to_string());
            }
        }

        args.push(source_path.to_string_lossy().into_owned());
        args.push(dest_path.to_string_lossy().into_owned());
        args.push(logfile.to_string_lossy().into_owned());

        // Register before spawning so a cancellation that arrives during
        // `spawn()` is not lost (the pgid is attached right after).
        let handle = jobs::start(job_id, "ddrescue clone");

        // Raw block devices are `root:disk` mode 0660 — like every other
        // disk-touching command in the codebase (mdadm below, disk.rs,
        // pools.rs) this must go through `sudo` so it works when HomeVault
        // is not itself root. `sudo` is a no-op wrap when it already is.
        //
        // `process_group(0)` puts `sudo` + `ddrescue` in a fresh group so a
        // cancellation can signal both at once: killing only the `sudo`
        // wrapper leaves the real tool writing to the disk.
        let mut cmd = Command::new("sudo");
        cmd.arg("ddrescue")
            .args(&args)
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = match spawn_child(&mut cmd) {
            Ok(child) => child,
            Err(e) => {
                handle.finish();
                return Err(e);
            }
        };
        if let Some(pid) = child.id() {
            handle.attach(pid);
        }

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        use tokio::io::AsyncBufReadExt;

        let reader = tokio::io::BufReader::new(stdout);
        let err_reader = tokio::io::BufReader::new(stderr);

        let reader_lines = reader.lines();
        let err_lines = err_reader.lines();

        tokio::pin!(reader_lines);
        tokio::pin!(err_lines);

        loop {
            tokio::select! {
                line = reader_lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            if let Some(pct) = Self::parse_ddrescue_progress(&l) {
                                ProgressEmitter::progress(job_id, pct, &l);
                            }
                        }
                        _ => break,
                    }
                }
                line = err_lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            if let Some(pct) = Self::parse_ddrescue_progress(&l) {
                                ProgressEmitter::progress(job_id, pct, &l);
                            }
                        }
                        _ => break,
                    }
                }
            }
        }

        let status = child.wait().await?;
        handle.finish();

        // A cancelled clone writes a valid mapfile on SIGTERM, so it can be
        // resumed; report it as cancelled, not as a failure.
        if handle.cancelled() {
            ProgressEmitter::cancelled(
                job_id,
                "Clone cancelled — the mapfile is kept, so the clone can be resumed",
            );
            return Err(RescueError::Cancelled);
        }

        if status.success() {
            ProgressEmitter::complete(job_id, "ddrescue clone completed successfully");
        } else {
            ProgressEmitter::fail(job_id, "ddrescue clone failed");
            return Err(RescueError::Parse("ddrescue clone failed".to_string()));
        }

        Ok(())
    }

    fn parse_ddrescue_progress(line: &str) -> Option<f64> {
        if let Some(pct_str) = line.split('%').next() {
            if let Some(num) = pct_str.split_whitespace().last() {
                if let Ok(pct) = num.parse::<f64>() {
                    return Some(pct);
                }
            }
        }
        None
    }

    // `ddrescue --status` was tried here previously — it does not exist as
    // a ddrescue option (confirmed against 1.27: "unrecognized option
    // '--status'"), so this always failed and reported a fixed
    // running=true/progress=0% no matter the job's real state. ddrescue has
    // no live "query a running instance" command at all; the mapfile it
    // maintains on disk (updated periodically while running, final on exit)
    // is the only real source of truth for an on-demand status check —
    // parsed directly below.
    pub async fn get_ddrescue_log(logfile: &str) -> Result<DdrescueStatus, RescueError> {
        // The path comes straight from a URL segment: confine it to the rescue
        // directories, refuse symlinks, non-files and oversized files before
        // reading anything with the service's own privileges.
        //
        // A missing file is *not* an error: a clone that has not been started
        // yet has no mapfile, and the UI polls this endpoint to decide whether
        // to show "resume". The confinement checks still apply — the path is
        // rejected before the existence of the file is even considered, so a
        // probe of an arbitrary system path can never report success/failure
        // differences that leak whether it exists.
        let path = match SecurityValidator::validate_readable_log_path(logfile) {
            Ok(path) => path,
            Err(RescueError::JobNotFound(_)) => {
                return Ok(DdrescueStatus {
                    job_id: String::new(),
                    progress: 0.0,
                    message: "No mapfile yet — the rescue has not started writing one".to_string(),
                    bytes_copied: None,
                    bytes_total: None,
                    speed: None,
                    running: false,
                });
            }
            Err(e) => return Err(e),
        };

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DdrescueStatus {
                    job_id: String::new(),
                    progress: 0.0,
                    message: "No mapfile yet — the rescue has not started writing one".to_string(),
                    bytes_copied: None,
                    bytes_total: None,
                    speed: None,
                    running: false,
                });
            }
            Err(e) => return Err(RescueError::Io(e)),
        };

        Ok(Self::parse_ddrescue_mapfile(&content))
    }

    /// Parses a GNU ddrescue mapfile: a `# current_pos current_status
    /// current_pass` comment followed by one data line (current position,
    /// status char, pass number), then a `#      pos        size  status`
    /// comment followed by one data line per rescued/bad/non-tried block
    /// range. All three fields on a block-range line are whitespace-
    /// separated hex (`0x...`) except the trailing status character, which
    /// is one of `?` (non-tried) `*` (non-trimmed) `/` (non-scraped)
    /// `-` (bad sector) `+` (rescued). Progress is the fraction of the
    /// domain (sum of every block's size) whose status is `+`; "running"
    /// reflects whether the current status (the very first data line, not
    /// a block row) has reached `+` (finished) yet.
    fn parse_ddrescue_mapfile(content: &str) -> DdrescueStatus {
        let mut current_status_char: Option<char> = None;
        let mut seen_current_line = false;
        let mut rescued_bytes: u64 = 0;
        let mut total_bytes: u64 = 0;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = trimmed.split_whitespace().collect();
            if !seen_current_line {
                if let Some(status_field) = fields.get(1) {
                    current_status_char = status_field.chars().next();
                }
                seen_current_line = true;
                continue;
            }
            if fields.len() >= 3 {
                let size = u64::from_str_radix(fields[1].trim_start_matches("0x"), 16).unwrap_or(0);
                total_bytes += size;
                if fields[2] == "+" {
                    rescued_bytes += size;
                }
            }
        }

        let progress = if total_bytes > 0 {
            (rescued_bytes as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };
        let finished = current_status_char == Some('+');

        DdrescueStatus {
            job_id: String::new(),
            progress,
            message: if finished {
                "Finished".to_string()
            } else {
                format!("{:.1}% rescued", progress)
            },
            bytes_copied: Some(rescued_bytes),
            bytes_total: Some(total_bytes),
            speed: None,
            running: !finished,
        }
    }

    pub async fn testdisk_analyze(device: &str) -> Result<TestDiskResult, RescueError> {
        let device_path = SecurityValidator::validate_block_device(device)?;

        // `sudo`-wrapped: TestDisk opens the raw device (root:disk 0660).
        // Without this it prints "Unable to open ... Permission denied" and
        // the handler still returns success with an empty partition list.
        //
        // Nothing here can be honoured from the old options: the scripted mode
        // this integration uses always performs the deep scan (`analyze,
        // search`) and TestDisk writes its own log to its working directory,
        // so `deep_search`/`log_path` were accepted and silently ignored.
        let mut cmd = Command::new("sudo");
        cmd.args(["testdisk", "/log", "/list", &device_path.to_string_lossy()]);
        let output = Self::run_output(cmd, TESTDISK_ANALYZE_TIMEOUT, "testdisk /list").await?;

        let log = String::from_utf8_lossy(&output.stdout).to_string();

        let partitions = Self::parse_testdisk_output(&log);

        Ok(TestDiskResult {
            device: device.to_string(),
            partitions,
            log,
        })
    }

    /// Parse the partition list `testdisk /log /list <dev>` prints. Real
    /// TestDisk 7.1 format (verified live against an ext4 loop device):
    ///
    /// ```text
    ///      Partition               Start        End    Size in sectors
    ///  1 P Linux                       2048     262143     260096
    ///      ext4 blocksize=4096 Large_file Sparse_SB
    /// ```
    ///
    /// A partition row is `<n> <P|L|E|D|*> <type…> <start> <end> <sectors>`
    /// (the type name can contain spaces, e.g. "HPFS - NTFS"); the indented
    /// line right after it carries the filesystem detail. The previous
    /// implementation read fixed `parts[0..4]` offsets and produced garbage
    /// ("partition": "P", "filesystem": "262143", plus a bogus second entry
    /// from the detail line) — and only ever ran at all once TestDisk was
    /// `sudo`-wrapped, so it had never actually been exercised.
    fn parse_testdisk_output(output: &str) -> Vec<PartitionEntry> {
        let mut partitions: Vec<PartitionEntry> = Vec::new();
        let mut want_detail_for: Option<usize> = None;

        for line in output.lines() {
            let toks: Vec<&str> = line.split_whitespace().collect();
            let is_row = toks.len() >= 6
                && toks[0].parse::<u32>().is_ok()
                && matches!(toks[1], "P" | "L" | "E" | "D" | "*")
                && toks[toks.len() - 3..]
                    .iter()
                    .all(|t| t.parse::<u64>().is_ok());

            if is_row {
                let n = toks.len();
                let status = match toks[1] {
                    "P" => "Primary",
                    "*" => "Primary (bootable)",
                    "L" => "Logical",
                    "E" => "Extended",
                    "D" => "Deleted",
                    other => other,
                };
                partitions.push(PartitionEntry {
                    partition: toks[0].to_string(),
                    size: format!(
                        "{} - {} ({} sectors)",
                        toks[n - 3],
                        toks[n - 2],
                        toks[n - 1]
                    ),
                    filesystem: toks[2..n - 3].join(" "),
                    status: status.to_string(),
                });
                want_detail_for = Some(partitions.len() - 1);
            } else if let Some(idx) = want_detail_for.take() {
                // The line immediately after a row, when indented and
                // non-empty, is that partition's filesystem detail line.
                let detail = line.trim();
                if line.starts_with(char::is_whitespace) && !detail.is_empty() {
                    partitions[idx].filesystem =
                        format!("{} / {}", partitions[idx].filesystem, detail);
                }
            }
        }

        partitions
    }

    pub async fn testdisk_rebuild(device: &str) -> Result<String, RescueError> {
        // Rebuilding a partition table is a real write to the raw disk — must
        // never run against an unvalidated or protected device.
        let device_path = SecurityValidator::validate_block_device(device)?;

        // TestDisk's scripted mode is `testdisk /log /cmd <device>
        // <comma,separated,commands>` — the command list is an *argument*,
        // never an stdin script. The old code piped a newline "menu script"
        // to stdin of a bare `testdisk /cmd` (no device), which every
        // TestDisk build ignores — so this had never once worked and its
        // failures surfaced as an empty "TestDisk rebuild failed: ".
        //
        // We run `analyze,search` only. That does the hard part — deep-scan
        // for lost partitions — and exits cleanly. We deliberately do NOT
        // append `write`: TestDisk 7.1's `/cmd` blocks forever on the Y/N
        // "write?" prompt (stdin can't answer it, verified live), and even
        // 7.2 is unreliable here. The scan result (the partition boundaries
        // it recovered) is returned to the admin to apply; a hard timeout +
        // closed stdin guarantee the Tokio worker is never wedged.
        let commands = "analyze,search";

        // Spawned, then awaited under `TESTDISK_REBUILD_TIMEOUT` two statements
        // below (the guard in `utils/process.rs` pins this by name).
        let child = Command::new("sudo")
            .args([
                "testdisk",
                "/log",
                "/cmd",
                &device_path.to_string_lossy(),
                commands,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let output =
            match tokio::time::timeout(TESTDISK_REBUILD_TIMEOUT, child.wait_with_output()).await {
                Ok(res) => res?,
                Err(_) => {
                    return Err(RescueError::Parse(format!(
                        "TestDisk scan did not finish within {} minutes and was stopped.",
                        TESTDISK_REBUILD_TIMEOUT.as_secs() / 60
                    )));
                }
            };

        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && !stderr.trim().is_empty() {
            return Err(RescueError::Parse(format!(
                "TestDisk scan failed: {}",
                stderr.trim()
            )));
        }

        // The ncurses `/cmd` run leaves escape sequences in stdout — strip
        // them so the caller gets a readable log of what TestDisk found.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut clean = String::with_capacity(stdout.len());
        let mut chars = stdout.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else if c == '\n' || !c.is_control() {
                clean.push(c);
            }
        }
        Ok(format!(
            "{}\n\nTestDisk recovered the partition layout above. Review it, then \
             apply it (writing the partition table back is not done automatically \
             — the installed TestDisk cannot do it non-interactively).",
            clean.trim()
        ))
    }

    pub async fn photorec_recover(
        device: &str,
        output_dir: &str,
        options: &PhotoRecOptions,
        job_id: &str,
        owner: Option<&str>,
    ) -> Result<(), RescueError> {
        use crate::sse::ProgressEmitter;

        let device_path = SecurityValidator::validate_block_device(device)?;
        options.validate()?;
        let output_path = SecurityValidator::validate_output_path(output_dir)?;
        tokio::fs::create_dir_all(&output_path).await?;

        ProgressEmitter::progress(
            job_id,
            0.0,
            &format!("Starting PhotoRec on {} -> {}", device, output_dir),
        );

        // PhotoRec has no documented non-interactive mode in `--help`/`man` —
        // without /cmd it launches its full ncurses UI and just sits there
        // forever on a headless server, so nothing was ever actually
        // recovered. The /cmd grammar below (confirmed against a real
        // PhotoRec 7.2 binary: plain "search" recovers correctly on both an
        // unpartitioned raw device and a real partitioned one; the
        // fileopt/everything/enable-disable filtering was verified to
        // actually filter, not just get ignored) comes from
        // https://www.cgsecurity.org/testdisk_doc/scripted_run.html.
        //
        // `keep_broken` and `max_size` used to be accepted here and had no
        // effect at all: every `keep_corrupted_file` variant is a hard syntax
        // error on 7.2 (it aborts the recovery) and there is no size filter in
        // the /cmd grammar. They are gone from the API rather than silently
        // ignored.
        let mut cmd_parts: Vec<String> = Vec::new();
        if let Some(pt) = &options.partition_type {
            if let Some(keyword) = partition_type_keyword(pt) {
                cmd_parts.push(keyword.to_string());
            }
        }
        if let Some(types) = &options.file_types {
            if !types.is_empty() {
                cmd_parts.push("fileopt".to_string());
                cmd_parts.push("everything".to_string());
                cmd_parts.push("disable".to_string());
                for ft in types {
                    cmd_parts.push(ft.clone());
                    cmd_parts.push("enable".to_string());
                }
            }
        }
        cmd_parts.push("search".to_string());

        let args: Vec<String> = vec![
            "/log".to_string(),
            "/d".to_string(),
            output_path.to_string_lossy().into_owned(),
            "/cmd".to_string(),
            device_path.to_string_lossy().into_owned(),
            cmd_parts.join(","),
        ];

        let handle = jobs::start(job_id, "PhotoRec recovery");

        // `sudo`-wrapped: PhotoRec reads the raw device, so the recovered files
        // land owned by root — the chown below hands them to the admin who
        // asked for them. Same `process_group(0)` reasoning as ddrescue above.
        let mut cmd = Command::new("sudo");
        cmd.arg("photorec")
            .args(&args)
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = match spawn_child(&mut cmd) {
            Ok(child) => child,
            Err(e) => {
                handle.finish();
                return Err(e);
            }
        };
        if let Some(pid) = child.id() {
            handle.attach(pid);
        }

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        use tokio::io::AsyncBufReadExt;

        let reader = tokio::io::BufReader::new(stdout);
        let err_reader = tokio::io::BufReader::new(stderr);

        let reader_lines = reader.lines();
        let err_lines = err_reader.lines();

        tokio::pin!(reader_lines);
        tokio::pin!(err_lines);

        loop {
            tokio::select! {
                line = reader_lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            if l.contains("Recovered") || l.contains("%") {
                                if let Some(pct) = Self::parse_photorec_progress(&l) {
                                    ProgressEmitter::progress(job_id, pct, &l);
                                }
                            }
                        }
                        _ => break,
                    }
                }
                line = err_lines.next_line() => {
                    match line {
                        Ok(Some(l)) => {
                            ProgressEmitter::progress(job_id, 50.0, &l);
                        }
                        _ => break,
                    }
                }
            }
        }

        let status = child.wait().await?;
        handle.finish();

        if handle.cancelled() {
            ProgressEmitter::cancelled(
                job_id,
                "Recovery cancelled — the files already carved are kept",
            );
            return Err(RescueError::Cancelled);
        }

        if status.success() {
            // PhotoRec never writes into the /d directory itself — it always
            // creates a numbered sibling (recup_dir.1, .2, ...) per run, so
            // reporting the requested output_dir as-is would point the user
            // at an empty folder. Find the actual one it just created.
            let scan_root = output_path.clone();
            let scanned = tokio::task::spawn_blocking(move || {
                Self::find_latest_photorec_result_dir(&scan_root)
            })
            .await
            .unwrap_or(None);
            let result_dir = scanned.unwrap_or_else(|| output_dir.to_string());

            // The carve ran as root, so everything it wrote is root-owned and
            // unreadable to the admin who asked for it. Hand the recovered
            // tree to them (best effort: the account may not exist locally).
            if let Some(user) = owner {
                if let Err(e) = Self::chown_recursive(user, Path::new(&result_dir)).await {
                    warn!(
                        "PhotoRec: recovered files in {} could not be handed to '{}': {}",
                        result_dir, user, e
                    );
                }
            }

            ProgressEmitter::complete(
                job_id,
                &format!("PhotoRec recovery completed. Files saved to {}", result_dir),
            );
        } else {
            ProgressEmitter::fail(job_id, "PhotoRec recovery failed");
            return Err(RescueError::Parse("PhotoRec recovery failed".to_string()));
        }

        Ok(())
    }

    /// `chown -R user:user <dir>` — best effort, and only for a username that
    /// looks like a real account name (it reaches an argv slot).
    async fn chown_recursive(user: &str, dir: &Path) -> Result<(), RescueError> {
        let valid = !user.is_empty()
            && user.len() <= 32
            && !user.starts_with('-')
            && user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
        if !valid {
            return Err(RescueError::InvalidInput(format!(
                "refusing to chown to invalid account name '{user}'"
            )));
        }

        let mut cmd = Command::new("sudo");
        cmd.args(["chown", "-R"])
            .arg(format!("{user}:{user}"))
            .arg(dir);
        let out = Self::run_output(cmd, LSBLK_TIMEOUT, "chown").await?;
        if !out.status.success() {
            return Err(RescueError::Parse(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    /// PhotoRec always writes into `<recup_dir>.N` (N = 1, 2, ...), a sibling
    /// of the directory passed via `/d`, never that directory itself. Finds
    /// the highest-numbered match, i.e. the run that just finished.
    fn find_latest_photorec_result_dir(output_path: &std::path::Path) -> Option<String> {
        let parent = output_path.parent()?;
        let base_name = output_path.file_name()?.to_str()?;
        let prefix = format!("{}.", base_name);

        let mut best: Option<(u32, std::path::PathBuf)> = None;
        for entry in std::fs::read_dir(parent).ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_str()?;
            if let Some(suffix) = name.strip_prefix(&prefix) {
                if let Ok(n) = suffix.parse::<u32>() {
                    if best.as_ref().is_none_or(|(best_n, _)| n > *best_n) {
                        best = Some((n, entry.path()));
                    }
                }
            }
        }
        best.map(|(_, path)| path.to_string_lossy().into_owned())
    }

    fn parse_photorec_progress(line: &str) -> Option<f64> {
        if line.contains("Recovered") {
            if let Some(num_str) = line.split_whitespace().find(|w| w.ends_with('%')) {
                let num = num_str.trim_end_matches('%');
                if let Ok(pct) = num.parse::<f64>() {
                    return Some(pct);
                }
            }
        }
        None
    }

    pub async fn mdadm_diagnose(name: &str) -> Result<MdadmDiagnostic, RescueError> {
        let raw_device = if name.starts_with("/dev/") {
            name.to_string()
        } else {
            format!("/dev/md/{}", name)
        };
        // `name` can embed `..` (e.g. "../sda") to escape the /dev/md/ prefix
        // above — canonicalizing through validate_block_device resolves that
        // before mdadm ever sees it, and rejects protected/non-block targets.
        let device_path = SecurityValidator::validate_block_device(&raw_device)?;
        let device = device_path.to_string_lossy().into_owned();

        // mdadm requires root even for a read-only --detail (confirmed live:
        // "must be super-user to perform this action" otherwise) — delegated
        // to root via the sudoers NOPASSWD rule, same as ufw/wg/openvpn.
        let mut cmd = Command::new("sudo");
        cmd.args(["mdadm", "--detail", &device]);
        let output = Self::run_output(cmd, MDADM_TIMEOUT, "mdadm --detail").await?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            return Err(RescueError::Parse(format!(
                "mdadm detail failed for {}",
                device
            )));
        }

        let mut diag = MdadmDiagnostic {
            array: name.to_string(),
            level: String::new(),
            state: String::new(),
            devices: Vec::new(),
            size: String::new(),
            health: String::new(),
            rebuild_progress: None,
        };

        // Member devices live at the *end* of each row in the
        // "Number Major Minor RaidDevice State" table near the bottom of the
        // output, not at the start of a line. The old `starts_with("/dev/")`
        // check matched the `/dev/md127:` header line instead (a 2-disk mirror
        // then reported a single "device" of `/dev/md127:`) and never saw the
        // real members. Same bug/fix as `pools.rs::get_mdadm_devices`.
        let mut in_device_table = false;
        for line in stdout.lines() {
            let trimmed = line.trim();
            if let Some(val) = trimmed.strip_prefix("Raid Level :") {
                diag.level = val.trim().to_string();
            } else if let Some(val) = trimmed.strip_prefix("State :") {
                diag.state = val.trim().to_string();
                // mdadm reports a degraded mirror as "clean, degraded" — the
                // `clean`/`active` branch must not win over it, so check the
                // failure words first.
                diag.health = if val.contains("failed") {
                    "failed".to_string()
                } else if val.contains("degraded") {
                    "degraded".to_string()
                } else if val.contains("clean") || val.contains("active") {
                    "healthy".to_string()
                } else {
                    "unknown".to_string()
                };
            } else if let Some(val) = trimmed.strip_prefix("Array Size :") {
                diag.size = val.trim().to_string();
            } else if let Some(val) = trimmed.strip_prefix("Rebuild Status :") {
                diag.rebuild_progress = Some(val.trim().to_string());
            } else if trimmed.starts_with("Number") && trimmed.contains("RaidDevice") {
                in_device_table = true;
            } else if in_device_table {
                if let Some(dev) = trimmed
                    .split_whitespace()
                    .last()
                    .filter(|t| t.starts_with("/dev/"))
                {
                    diag.devices.push(dev.to_string());
                }
            }
        }

        Ok(diag)
    }

    pub async fn mdadm_repair(name: &str, options: &MdadmOptions) -> Result<String, RescueError> {
        let raw_device = if name.starts_with("/dev/") {
            name.to_string()
        } else {
            format!("/dev/md/{}", name)
        };
        // Same traversal protection as mdadm_diagnose above — this path
        // performs a real, potentially --force write repair on the resolved
        // device, so it must never resolve to a protected boot device.
        let device_path = SecurityValidator::validate_block_device(&raw_device)?;
        let device = device_path.to_string_lossy().into_owned();

        // `--action=` is a Misc-mode option and cannot be combined with
        // `--manage` — confirmed live: mdadm rejects it ("--action would set
        // mdadm mode to \"misc\", but it is already set to \"manage\""), so
        // every repair/check request failed outright before this fix,
        // regardless of the sudo/root fix above. There's also no standalone
        // `--repair` flag (it isn't in `mdadm --help` at all) — the real
        // mechanism is `--action=repair`. "rewrite" (offered in the
        // frontend's Action dropdown) isn't a real mdadm sync_action either
        // (only idle/frozen/check/repair are, per `man mdadm`); mapped to
        // "repair", the real write-correcting action, since that's what it
        // was clearly intended to mean.
        //
        // The value is whitelisted: it used to be forwarded verbatim
        // (`Some(other) => other`), i.e. any string reached `--action=<value>`.
        let action = options.validated_action()?;
        let mut args: Vec<String> = vec![format!("--action={}", action), device.clone()];

        if options.force.unwrap_or(false) {
            args.push("--force".to_string());
        }
        if options.verbose.unwrap_or(false) {
            args.push("--verbose".to_string());
        }

        // Same root requirement as mdadm_diagnose above.
        let mut cmd = Command::new("sudo");
        cmd.arg("mdadm").args(&args);
        let output = Self::run_output(cmd, MDADM_TIMEOUT, "mdadm --action").await?;

        if output.status.success() {
            Ok(format!("Repair started for {}", device))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(RescueError::Parse(format!(
                "Failed to repair {}: {}",
                device, stderr
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ddrescue_mapfile_finished() {
        // A real mapfile generated by `ddrescue -f src dst map.log` against
        // two small regular files (GNU ddrescue 1.27) after a clean, fully
        // successful run.
        let content = "\
# Mapfile. Created by GNU ddrescue version 1.27
# Command line: ddrescue -f src_test.bin dst_test.bin map_test.log
# Start time:   2026-09-05 16:56:09
# Current time: 2026-09-05 16:56:09
# Finished
# current_pos  current_status  current_pass
0x00010000     +               1
#      pos        size  status
0x00000000  0x00019000  +
";
        let status = RescueService::parse_ddrescue_mapfile(content);
        assert!(!status.running);
        assert_eq!(status.message, "Finished");
        assert_eq!(status.progress, 100.0);
        assert_eq!(status.bytes_copied, Some(0x19000));
        assert_eq!(status.bytes_total, Some(0x19000));
    }

    #[test]
    fn test_parse_ddrescue_mapfile_partial_in_progress() {
        // Synthetic mapfile matching the documented format for a rescue
        // that copied its first block (0x8000 bytes) and has 0x11000 bytes
        // of the domain still non-tried.
        let content = "\
# Mapfile. Created by GNU ddrescue version 1.27
# Command line: ddrescue -f src dst map.log
# Start time:   2026-09-05 17:00:00
# Current time: 2026-09-05 17:00:05
# Copying non-tried blocks... Pass 1 (forwards)
# current_pos  current_status  current_pass
0x00008000     ?               1
#      pos        size  status
0x00000000  0x00008000  +
0x00008000  0x00011000  ?
";
        let status = RescueService::parse_ddrescue_mapfile(content);
        assert!(status.running);
        assert_eq!(status.message, "32.0% rescued");
        assert!((status.progress - 32.0).abs() < 0.01);
        assert_eq!(status.bytes_copied, Some(0x8000));
        assert_eq!(status.bytes_total, Some(0x8000 + 0x11000));
    }

    #[test]
    fn test_parse_ddrescue_mapfile_with_bad_sectors() {
        let content = "\
# current_pos  current_status  current_pass
0x00000000     -               2
#      pos        size  status
0x00000000  0x00001000  +
0x00001000  0x00000100  -
0x00001100  0x00000f00  /
";
        let status = RescueService::parse_ddrescue_mapfile(content);
        assert!(status.running);
        assert_eq!(status.bytes_copied, Some(0x1000));
        assert_eq!(status.bytes_total, Some(0x1000 + 0x100 + 0xf00));
    }

    #[test]
    fn test_parse_ddrescue_mapfile_empty_content() {
        let status = RescueService::parse_ddrescue_mapfile("");
        assert_eq!(status.progress, 0.0);
        assert_eq!(status.bytes_copied, Some(0));
        assert_eq!(status.bytes_total, Some(0));
        // No current-status line was ever seen, so it can't be considered
        // finished — still reported as running rather than silently "done".
        assert!(status.running);
    }

    #[test]
    fn test_parse_ddrescue_mapfile_only_comments() {
        let content = "# Mapfile. Created by GNU ddrescue version 1.27\n# Start time: now\n";
        let status = RescueService::parse_ddrescue_mapfile(content);
        assert_eq!(status.progress, 0.0);
        assert!(status.running);
    }

    // -- parse_testdisk_output (real `testdisk /log /list` format) ----------

    #[test]
    fn test_parse_testdisk_output_single_ext4_partition() {
        // Captured verbatim from `sudo testdisk /log /list /dev/loop0`
        // (TestDisk 7.1) against a 128 MiB loop device with one ext4 primary.
        let out = "\
Disk /dev/loop0 - 134 MB / 128 MiB - 262144 sectors
     Partition\t\t\tStart        End    Size in sectors
 1 P Linux                       2048     262143     260096
     ext4 blocksize=4096 Large_file Sparse_SB
";
        let parts = RescueService::parse_testdisk_output(out);
        assert_eq!(
            parts.len(),
            1,
            "exactly one partition, not a bogus second from the detail line"
        );
        assert_eq!(parts[0].partition, "1");
        assert_eq!(parts[0].status, "Primary");
        assert_eq!(parts[0].size, "2048 - 262143 (260096 sectors)");
        assert!(parts[0].filesystem.starts_with("Linux"));
        assert!(
            parts[0].filesystem.contains("ext4"),
            "fs detail line folded in"
        );
    }

    #[test]
    fn test_parse_testdisk_output_multiword_type_and_multiple_rows() {
        // NTFS type name contains spaces/dashes ("HPFS - NTFS") — the old
        // fixed-offset parser mangled it.
        let out = "\
     Partition               Start        End    Size in sectors
 1 P HPFS - NTFS                 2048    2099199    2097152
     NTFS, 1024 MB / 976 MiB
 2 L Linux                    2099200    4196351    2097152
";
        let parts = RescueService::parse_testdisk_output(out);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].filesystem.starts_with("HPFS - NTFS"));
        assert_eq!(parts[1].partition, "2");
        assert_eq!(parts[1].status, "Logical");
    }

    #[test]
    fn test_parse_testdisk_output_no_partitions() {
        let out = "Disk /dev/loop0 - 134 MB / 128 MiB - 262144 sectors\nNo partition found or selected for recovery\n";
        assert!(RescueService::parse_testdisk_output(out).is_empty());
    }
}
