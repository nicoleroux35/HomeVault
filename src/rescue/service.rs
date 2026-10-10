use super::models::*;
use super::validator::SecurityValidator;
use tokio::process::Command;

pub struct RescueService;

impl RescueService {
    pub async fn check_tools() -> Vec<ToolStatus> {
        let tools = vec!["ddrescue", "testdisk", "photorec", "mdadm"];
        let mut statuses = Vec::new();

        for tool in tools {
            let (available, version) = Self::check_tool(tool).await;
            statuses.push(ToolStatus {
                name: tool.to_string(),
                available,
                version,
            });
        }

        statuses
    }

    async fn check_tool(tool: &str) -> (bool, Option<String>) {
        match tool {
            "ddrescue" => {
                let output = Command::new("ddrescue").arg("--version").output().await;
                match output {
                    Ok(o) if o.status.success() => {
                        let ver = String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .next()
                            .map(|l| l.to_string());
                        (true, ver)
                    }
                    _ => (false, None),
                }
            }
            "testdisk" => {
                let output = Command::new("testdisk").arg("--version").output().await;
                match output {
                    Ok(o) if o.status.success() => {
                        let ver = String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .next()
                            .map(|l| l.to_string());
                        (true, ver)
                    }
                    _ => (false, None),
                }
            }
            "photorec" => {
                let output = Command::new("photorec").arg("--version").output().await;
                match output {
                    Ok(o) if o.status.success() => {
                        let ver = String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .next()
                            .map(|l| l.to_string());
                        (true, ver)
                    }
                    _ => (false, None),
                }
            }
            "mdadm" => {
                let output = Command::new("mdadm").arg("--version").output().await;
                match output {
                    // mdadm writes its version banner to stderr, not stdout,
                    // unlike the other three tools checked above.
                    Ok(o) if o.status.success() => {
                        let ver = String::from_utf8_lossy(&o.stderr)
                            .lines()
                            .next()
                            .map(|l| l.trim().to_string());
                        (true, ver)
                    }
                    _ => (false, None),
                }
            }
            _ => (false, None),
        }
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
        let output = Command::new("lsblk")
            .args(["-d", "-n", "-b", "-P", "-o", "NAME,SIZE,TYPE,MODEL,TRAN,RM"])
            .output()
            .await?;

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
            if name.is_empty() {
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
        let logfile = match &options.logfile {
            Some(lf) => SecurityValidator::validate_log_path(lf)?
                .to_string_lossy()
                .into_owned(),
            None => "/tmp/ddrescue.log".to_string(),
        };

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
            options.block_size.unwrap_or(512).to_string(),
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
        args.push(logfile);

        // Raw block devices are `root:disk` mode 0660 — like every other
        // disk-touching command in the codebase (mdadm below, disk.rs,
        // pools.rs) this must go through `sudo` so it works when HomeVault
        // is not itself root. `sudo` is a no-op wrap when it already is.
        let mut child = Command::new("sudo")
            .arg("ddrescue")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

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
        let content = match tokio::fs::read_to_string(logfile).await {
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

    pub async fn testdisk_analyze(
        device: &str,
        _options: &TestDiskOptions,
    ) -> Result<TestDiskResult, RescueError> {
        let device_path = SecurityValidator::validate_block_device(device)?;

        // `sudo`-wrapped: TestDisk opens the raw device (root:disk 0660).
        // Without this it prints "Unable to open ... Permission denied" and
        // the handler still returns success with an empty partition list.
        let output = Command::new("sudo")
            .args(["testdisk", "/log", "/list", &device_path.to_string_lossy()])
            .output()
            .await?;

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

    pub async fn testdisk_rebuild(
        device: &str,
        _options: &TestDiskOptions,
    ) -> Result<String, RescueError> {
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

        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(300),
            child.wait_with_output(),
        )
        .await
        {
            Ok(res) => res?,
            Err(_) => {
                return Err(RescueError::Parse(
                    "TestDisk scan did not finish within 5 minutes and was stopped.".to_string(),
                ));
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
    ) -> Result<(), RescueError> {
        use crate::sse::ProgressEmitter;

        let device_path = SecurityValidator::validate_block_device(device)?;
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
        // `keep_broken` has no working equivalent found here (every
        // `keep_corrupted_file` variant tried produced a hard syntax error,
        // which would abort recovery entirely) so it's intentionally left
        // unwired, same as before this fix — not a regression.
        let mut cmd_parts: Vec<String> = Vec::new();
        if let Some(pt) = &options.partition_type {
            let keyword = match pt.as_str() {
                "Intel" => Some("partition_i386"),
                "EFI" => Some("partition_gpt"),
                "Mac" => Some("partition_mac"),
                "Sun" => Some("partition_sun"),
                "None" => Some("partition_none"),
                _ => None,
            };
            if let Some(k) = keyword {
                cmd_parts.push(k.to_string());
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

        // `sudo`-wrapped: PhotoRec reads the raw device. Recovered files land
        // in `output_path` owned by root — the reader below
        // (`find_latest_photorec_result_dir`) still lists them fine.
        let mut child = Command::new("sudo")
            .arg("photorec")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

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
        if status.success() {
            // PhotoRec never writes into the /d directory itself — it always
            // creates a numbered sibling (recup_dir.1, .2, ...) per run, so
            // reporting the requested output_dir as-is would point the user
            // at an empty folder. Find the actual one it just created.
            let result_dir = Self::find_latest_photorec_result_dir(&output_path)
                .unwrap_or_else(|| output_dir.to_string());
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
        let output = Command::new("sudo")
            .args(["mdadm", "--detail", &device])
            .output()
            .await?;
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
        let action = match options.action.as_deref() {
            Some("rewrite") => "repair",
            Some(other) => other,
            None => "repair",
        };
        let mut args: Vec<String> = vec![format!("--action={}", action), device.clone()];

        if options.force.unwrap_or(false) {
            args.push("--force".to_string());
        }
        if options.verbose.unwrap_or(false) {
            args.push("--verbose".to_string());
        }

        // Same root requirement as mdadm_diagnose above.
        let output = Command::new("sudo")
            .arg("mdadm")
            .args(&args)
            .output()
            .await?;

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
