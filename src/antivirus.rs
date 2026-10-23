/* =============================================================================
 * File          : antivirus.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Antivirus scanning service (on-demand + scheduled): runs the ClamAV
 *   scanner CLI as the unprivileged service account over managed paths,
 *   with a single-flight scan guard, live SSE progress parsing and
 *   quarantine handling. Virus definitions are kept current by the OS
 *   timer, never by this service.
 *
 * Dependencies:
 *   - tokio (>= 1.52), internal SSE progress + storage repositories
 *
 * Usage:
 *   handlers/antivirus.rs; spawned scans run in the background (see
 *   docs/guides/antivirus.md).
 * =============================================================================
 */

//! Antivirus scanning via ClamAV's `clamscan` CLI (on-demand + scheduled
//! only — no `clamd` real-time daemon, to stay within the low-power
//! hardware budget documented in the systemd unit `installer/install.sh`
//! writes).
//!
//! `clamscan` runs as the unprivileged `homevault` user against paths the
//! service already manages; virus definitions are kept current by the
//! OS-level `clamav-freshclam` timer, never invoked by HomeVault itself.

use crate::sse::progress::{JobType, PROGRESS_MANAGER};
use crate::sse::ProgressEmitter;
use crate::storage::{AntivirusRepository, NotificationRepository};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Guards against two scans running concurrently (manual trigger racing the
/// scheduler, or a double click). `run_scan` is the single entry point used
/// by both, so claiming/releasing it there covers every trigger path —
/// mirrors `cache.rs`'s `MOVER_RUNNING`/`MoverRunGuard`.
static AV_SCAN_RUNNING: AtomicBool = AtomicBool::new(false);

/// PID of the currently running `clamscan` child, if any. A `tokio::process::
/// Child` can't be parked in a plain `Mutex` and later `.wait()`-ed across an
/// `.await` from a different task without holding the lock over the await
/// point (unsound/deadlock-prone), so `cancel_scan()` instead signals the
/// process by PID via the `kill` binary — `clamscan` runs as the same
/// unprivileged `homevault` user as this process, so no `sudo` is needed.
static AV_SCAN_PID: std::sync::LazyLock<parking_lot::Mutex<Option<u32>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

/// Non-authoritative peek used for an early, friendly rejection in the HTTP
/// handler before a job/DB row is even created. The actual claim happens
/// inside `run_scan`.
pub fn av_scan_is_running() -> bool {
    AV_SCAN_RUNNING.load(Ordering::SeqCst)
}

/// RAII guard that releases the scan slot (and clears the tracked PID) on
/// drop, including on panic unwind, so a claim can never be leaked.
struct AvRunGuard;

impl Drop for AvRunGuard {
    fn drop(&mut self) {
        AV_SCAN_PID.lock().take();
        AV_SCAN_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// Whether the `clamscan` binary is installed (mirrors `wireguard_available()`
/// in `src/vpn.rs`).
///
/// Blocking: forks `clamscan --version`, which loads the signature database and
/// can take a second. Handler and scheduler callers use
/// [`clamscan_available_async`]; this form is kept for the dependency probe,
/// whose `check_installed` contract is a plain `fn() -> bool`.
pub fn clamscan_available() -> bool {
    use crate::utils::process::{run_timed, HOST_PROBE_TIMEOUT};
    run_timed(
        std::process::Command::new("clamscan").arg("--version"),
        HOST_PROBE_TIMEOUT,
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// [`clamscan_available`] off the async runtime (ROADMAP X.1): a status poll
/// must not fork a signature-loading process on a runtime thread.
pub async fn clamscan_available_async() -> bool {
    tokio::task::spawn_blocking(clamscan_available)
        .await
        .unwrap_or(false)
}

/// Virus definitions freshness, read-only (HomeVault never runs `freshclam`
/// itself — that's the OS-level `clamav-freshclam` timer's job).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DefinitionsStatus {
    pub available: bool,
    pub last_updated: Option<DateTime<Utc>>,
}

pub fn definitions_status() -> DefinitionsStatus {
    let candidates = [
        "/var/lib/clamav/daily.cvd",
        "/var/lib/clamav/daily.cld",
        "/var/lib/clamav/main.cvd",
        "/var/lib/clamav/main.cld",
    ];
    let mut newest: Option<DateTime<Utc>> = None;
    for path in candidates {
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                let dt: DateTime<Utc> = modified.into();
                newest = Some(match newest {
                    Some(n) if n >= dt => n,
                    _ => dt,
                });
            }
        }
    }
    DefinitionsStatus {
        available: newest.is_some(),
        last_updated: newest,
    }
}

/// [`definitions_status`] off the async runtime: it stats a handful of files on
/// the (possibly slow/spun-down) data volume.
pub async fn definitions_status_async() -> DefinitionsStatus {
    tokio::task::spawn_blocking(definitions_status)
        .await
        .unwrap_or(DefinitionsStatus {
            available: false,
            last_updated: None,
        })
}

/// Validate the configured quarantine directory (ROADMAP 13.5).
///
/// The value is interpolated into `clamscan`'s `--move=<dir>` argument and
/// created with `create_dir_all`, so it is not just a display field. It used to
/// be stored verbatim, alone among the antivirus settings.
///
/// Rules: an absolute path, no `..` component, no control characters, not the
/// filesystem root, and inside one of `allowed_roots` (HomeVault's own base
/// directory by default, plus any configured storage root). A relative path is
/// refused rather than resolved, because `clamscan` resolves it against its own
/// working directory, which is not the one the admin is looking at.
pub fn validate_quarantine_dir(input: &str, allowed_roots: &[std::path::PathBuf]) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("Quarantine directory must not be empty".to_string());
    }
    if raw.len() > 1024 {
        return Err("Quarantine directory is too long".to_string());
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err("Quarantine directory must not contain control characters".to_string());
    }
    let path = std::path::Path::new(raw);
    if !path.is_absolute() {
        return Err(format!(
            "Quarantine directory '{}' must be an absolute path",
            raw
        ));
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Quarantine directory must not contain '..'".to_string());
    }
    let normalized: std::path::PathBuf =
        path.components().filter(|c| matches!(c, std::path::Component::Normal(_))).collect();
    if normalized.as_os_str().is_empty() {
        return Err("Quarantine directory must not be the filesystem root".to_string());
    }
    let inside = allowed_roots.iter().any(|root| {
        let root: std::path::PathBuf = root
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .collect();
        normalized.starts_with(&root)
    });
    if !inside {
        return Err(format!(
            "Quarantine directory '{}' must be inside HomeVault's data directory or a configured storage root",
            raw
        ));
    }
    // Trailing slashes are harmless to `clamscan`, but they make the stored
    // value differ from the one a later save sends back — trim them so the
    // field round-trips byte-for-byte.
    Ok(raw.trim_end_matches('/').to_string())
}

/// Summary of a completed (or failed) scan.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AvScanSummary {
    pub files_scanned: u64,
    pub threats_found: u64,
}

/// Run a `clamscan` recursive scan against `scan_path`, streaming matches
/// into `av_threats` and live progress into the SSE job `job_id` (if any).
///
/// Only one scan can be in flight at a time (manual trigger and the
/// scheduler both funnel through here); a concurrent call is rejected
/// immediately rather than racing two `clamscan` processes.
pub async fn run_scan(
    pool: Arc<sqlx::SqlitePool>,
    scan_path: String,
    trigger: &str,
    job_id: Option<String>,
) -> Result<(String, AvScanSummary), String> {
    if AV_SCAN_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let msg = "An antivirus scan is already running".to_string();
        if let Some(jid) = &job_id {
            ProgressEmitter::fail(jid, &msg);
            ProgressEmitter::remove(jid);
        }
        return Err(msg);
    }
    let _guard = AvRunGuard;

    let repo = AntivirusRepository::new(pool.clone());
    let settings = repo.get_settings().await.map_err(|e| e.to_string())?;
    // Async create: `std::fs` here ran a metadata+mkdir on the event loop
    // (ROADMAP X.1), and the quarantine volume is often a spun-down disk.
    tokio::fs::create_dir_all(&settings.quarantine_dir)
        .await
        .map_err(|e| format!("Failed to create quarantine dir: {}", e))?;

    let scan = repo
        .create_scan(&scan_path, trigger)
        .await
        .map_err(|e| e.to_string())?;

    let result = run_clamscan(
        &scan_path,
        &settings.quarantine_dir,
        settings.scan_archives,
        job_id.as_deref(),
    )
    .await;

    match result {
        Ok((summary, threats)) => {
            for t in &threats {
                crate::events::log_event_warn(format!(
                    "Antivirus detected threat '{}' in '{}' ({})",
                    t.signature_name, t.file_path, t.action
                ));
                let _ = repo
                    .add_threat(
                        &scan.id,
                        &t.file_path,
                        &t.signature_name,
                        &t.action,
                        t.quarantine_path.as_deref(),
                    )
                    .await;
            }
            repo.finish_scan(
                &scan.id,
                "completed",
                summary.files_scanned,
                summary.threats_found,
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
            let _ = repo.set_last_scan_at(&Utc::now().to_rfc3339()).await;

            if let Some(jid) = &job_id {
                ProgressEmitter::complete(
                    jid,
                    &format!("Scan complete: {} threats found", summary.threats_found),
                );
                ProgressEmitter::remove(jid);
            }
            notify_scan(pool, summary.threats_found, &scan_path).await;
            Ok((scan.id, summary))
        }
        Err(e) => {
            repo.finish_scan(&scan.id, "failed", 0, 0, Some(&e))
                .await
                .map_err(|e2| e2.to_string())?;
            if let Some(jid) = &job_id {
                ProgressEmitter::fail(jid, &e);
                ProgressEmitter::remove(jid);
            }
            Err(e)
        }
    }
}

struct ThreatMatch {
    file_path: String,
    signature_name: String,
    action: String,
    quarantine_path: Option<String>,
}

/// The exact `clamscan` argv for one scan. Pure, so "the archive toggle and
/// the quarantine path really reach the command line" is unit-testable.
///
/// ROADMAP 13.5: `scan_archives` was persisted and rendered but never used, so
/// the switch did nothing. It is now an explicit `--scan-archive=yes|no` —
/// explicit in both directions, so the value the UI shows is the behaviour
/// regardless of the argument `clamscan` happens to default to.
pub fn clamscan_args(scan_path: &str, quarantine_dir: &str, scan_archives: bool) -> Vec<String> {
    vec![
        "-r".to_string(),
        "-i".to_string(),
        "--no-summary".to_string(),
        format!("--move={}", quarantine_dir),
        format!(
            "--scan-archive={}",
            if scan_archives { "yes" } else { "no" }
        ),
        scan_path.to_string(),
    ]
}

/// Spawns `clamscan` as an async child (not `spawn_blocking`) so we can
/// stream stdout line-by-line for live progress and keep a killable handle
/// for real cancellation via `cancel_scan()`.
async fn run_clamscan(
    scan_path: &str,
    quarantine_dir: &str,
    scan_archives: bool,
    job_id: Option<&str>,
) -> Result<(AvScanSummary, Vec<ThreatMatch>), String> {
    let mut child = Command::new("clamscan")
        .args(clamscan_args(scan_path, quarantine_dir, scan_archives))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start clamscan: {}", e))?;

    if let Some(pid) = child.id() {
        *AV_SCAN_PID.lock() = Some(pid);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or("Failed to capture clamscan stdout")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Failed to capture clamscan stderr")?;
    // Drained concurrently with stdout below so a chatty stderr (e.g. missing
    // virus database, permission errors) can't fill its OS pipe buffer and
    // deadlock the child while we're only reading stdout.
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ =
            tokio::io::AsyncReadExt::read_to_string(&mut BufReader::new(stderr), &mut buf).await;
        buf
    });
    let mut reader = BufReader::new(stdout).lines();

    let mut files_scanned: u64 = 0;
    let mut threats = Vec::new();

    while let Ok(Some(line)) = reader.next_line().await {
        files_scanned += 1;
        if let Some(threat) = parse_clamscan_line(&line) {
            threats.push(threat);
            if let Some(jid) = job_id {
                ProgressEmitter::progress(
                    jid,
                    // clamscan gives no total up front; report an indeterminate
                    // 50% while running so the UI shows activity, not a stall.
                    50.0,
                    &format!(
                        "{} files scanned, {} threats found",
                        files_scanned,
                        threats.len()
                    ),
                );
            }
        } else if let Some(jid) = job_id {
            ProgressEmitter::progress(jid, 50.0, &format!("{} files scanned", files_scanned));
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("clamscan wait failed: {}", e))?;
    *AV_SCAN_PID.lock() = None;
    let stderr_text = stderr_task.await.unwrap_or_default();

    // clamscan prints each `... FOUND` line *before* it relocates the file
    // to the quarantine dir (`--move`). Checking the move mid-stream races
    // it — on slower storage the file isn't there yet, and the threat gets
    // wrongly recorded as `left_in_place` with no `quarantine_path` (so a
    // later "resolve + delete" can't clean the quarantined copy). Now that
    // the process has fully exited, every move is settled: resolve each
    // threat's real disposition against the final filesystem state.
    for threat in &mut threats {
        let (action, quarantine_path) = resolve_move_status(&threat.file_path, quarantine_dir);
        threat.action = action;
        threat.quarantine_path = quarantine_path;
    }

    // clamscan exit codes: 0 = clean, 1 = threat(s) found, 2 = error.
    // No code (`None`) means it was terminated by a signal — either a
    // `cancel_scan()` call or an external kill; report as cancelled either way.
    match status.code() {
        Some(0) | Some(1) => Ok((
            AvScanSummary {
                files_scanned,
                threats_found: threats.len() as u64,
            },
            threats,
        )),
        Some(2) => {
            // Surface clamscan's actual stderr (e.g. "No supported database
            // files found" when freshclam hasn't run yet, or a genuine
            // permission error on an unreadable path) rather than guessing —
            // a generic "permission issues" message previously masked the
            // real cause and misled admins into checking the wrong thing.
            let detail = stderr_text.trim();
            if detail.is_empty() {
                Err(format!(
                    "clamscan reported an error scanning '{}'",
                    scan_path
                ))
            } else {
                Err(format!(
                    "clamscan reported an error scanning '{}': {}",
                    scan_path, detail
                ))
            }
        }
        None => Err("Scan was cancelled".to_string()),
        Some(other) => Err(format!("clamscan exited with unexpected code {}", other)),
    }
}

/// `clamscan -i` (infected-only) output lines look like:
/// `/path/to/file: Win.Test.EICAR_HDB-1 FOUND`
///
/// Parses the path + signature only. The `action` / `quarantine_path` are
/// filled in later by `resolve_move_status`, once `clamscan` has exited and
/// its `--move` is guaranteed complete (see `run_clamscan`).
fn parse_clamscan_line(line: &str) -> Option<ThreatMatch> {
    let line = line.trim();
    let rest = line.strip_suffix("FOUND")?.trim();
    let (file_path, signature_name) = rest.rsplit_once(':')?;
    Some(ThreatMatch {
        file_path: file_path.trim().to_string(),
        signature_name: signature_name.trim().to_string(),
        // Provisional; overwritten by `resolve_move_status` after the scan.
        action: "left_in_place".to_string(),
        quarantine_path: None,
    })
}

/// Decide, from the settled filesystem state, whether `clamscan --move`
/// actually relocated an infected file: `("quarantined", Some(path))` if the
/// file now sits in `quarantine_dir`, `("left_in_place", None)` otherwise
/// (move failed, or the admin disabled quarantining). Called only after the
/// `clamscan` process has fully exited.
fn resolve_move_status(file_path: &str, quarantine_dir: &str) -> (String, Option<String>) {
    let filename = Path::new(file_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string());
    let quarantine_path = format!("{}/{}", quarantine_dir, filename);
    if Path::new(&quarantine_path).exists() {
        ("quarantined".to_string(), Some(quarantine_path))
    } else {
        ("left_in_place".to_string(), None)
    }
}

/// Signal the currently running scan to stop, if any. Returns whether a scan
/// was actually running.
pub async fn cancel_scan() -> bool {
    use crate::utils::process::run_timed_async;
    use std::time::Duration;

    let pid = *AV_SCAN_PID.lock();
    match pid {
        Some(pid) => {
            // ROADMAP X.2: bounded, like every other host command. `kill` on a
            // live PID returns in milliseconds; the deadline only exists so a
            // wedged process table cannot hang the cancel button.
            let mut cmd = Command::new("kill");
            cmd.args(["-TERM", &pid.to_string()]);
            let _ = run_timed_async(&mut cmd, Duration::from_secs(5)).await;
            true
        }
        None => false,
    }
}

/// Delete a quarantined file's on-disk copy and mark the threat resolved.
pub async fn resolve_threat(
    pool: Arc<sqlx::SqlitePool>,
    threat_id: &str,
    delete: bool,
) -> Result<(), String> {
    let repo = AntivirusRepository::new(pool);
    let threat = repo
        .get_threat(threat_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Threat not found".to_string())?;

    if delete {
        if let Some(qpath) = &threat.quarantine_path {
            let qpath = qpath.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(&qpath)).await;
        }
    }

    repo.resolve_threat(threat_id)
        .await
        .map_err(|e| e.to_string())
}

async fn notify_scan(pool: Arc<sqlx::SqlitePool>, threats_found: u64, scan_path: &str) {
    let repo = NotificationRepository::new(pool);
    let (title, severity, message) = if threats_found > 0 {
        (
            "Antivirus scan found threats",
            "critical",
            format!(
                "{} threat(s) found and quarantined while scanning '{}'.",
                threats_found, scan_path
            ),
        )
    } else {
        (
            "Antivirus scan completed",
            "info",
            format!("No threats found scanning '{}'.", scan_path),
        )
    };
    let _ = repo
        .create(title, &message, severity, "antivirus", "antivirus_scan")
        .await;
}

/// Registers a new SSE progress job for a scan (a `Custom` job type — no
/// dedicated antivirus variant exists on `JobType` yet).
pub fn create_scan_job() -> String {
    PROGRESS_MANAGER.create_job(
        JobType::Custom("antivirus_scan".to_string()),
        "Starting antivirus scan",
    )
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for parsing `clamscan -i` output. The scan itself
    //! (spawning `clamscan`, quarantine moves, cancellation) needs the real
    //! binary + a real EICAR file and is covered by
    //! `tests/security_integration.rs` / `tests/e2e/security/security.spec.ts`.

    use super::*;

    #[test]
    fn test_parse_clamscan_eicar_line() {
        let line = "/tmp/scan/eicar.com: Win.Test.EICAR_HDB-1 FOUND";
        let t = parse_clamscan_line(line).unwrap();
        assert_eq!(t.file_path, "/tmp/scan/eicar.com");
        assert_eq!(t.signature_name, "Win.Test.EICAR_HDB-1");
        // The move disposition is resolved later by `resolve_move_status`;
        // `parse_clamscan_line` itself only ever returns the provisional
        // "left_in_place" / None.
        assert_eq!(t.action, "left_in_place");
        assert_eq!(t.quarantine_path, None);
    }

    #[test]
    fn test_parse_clamscan_line_with_colons_in_path() {
        // `rsplit_once(':')` must split on the LAST colon, keeping a weird
        // path with its own colons intact.
        let line = "/mnt/data/weird:name:file.bin: Some.Signature FOUND";
        let t = parse_clamscan_line(line).unwrap();
        assert_eq!(t.file_path, "/mnt/data/weird:name:file.bin");
        assert_eq!(t.signature_name, "Some.Signature");
    }

    #[test]
    fn test_parse_clamscan_line_ignores_clean_and_summary_lines() {
        assert!(parse_clamscan_line("/tmp/scan/clean.txt: OK").is_none());
        assert!(parse_clamscan_line("----------- SCAN SUMMARY -----------").is_none());
        assert!(parse_clamscan_line("Infected files: 1").is_none());
        assert!(parse_clamscan_line("").is_none());
    }

    #[test]
    fn test_parse_clamscan_line_requires_a_colon_before_the_signature() {
        // A line ending in FOUND but with no `path: signature` shape.
        assert!(parse_clamscan_line("garbage FOUND").is_none());
    }

    #[test]
    fn test_resolve_move_status_reports_quarantined_when_the_file_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let scan_dir = tmp.path().join("scan");
        let quarantine = tmp.path().join("quarantine");
        std::fs::create_dir_all(&scan_dir).unwrap();
        std::fs::create_dir_all(&quarantine).unwrap();
        // Simulate `clamscan --move`: the file now lives in quarantine, not
        // at its original path.
        std::fs::write(quarantine.join("eicar.com"), b"x").unwrap();
        let original = scan_dir.join("eicar.com");

        let (action, qpath) =
            resolve_move_status(&original.to_string_lossy(), &quarantine.to_string_lossy());
        assert_eq!(action, "quarantined");
        assert_eq!(
            qpath,
            Some(quarantine.join("eicar.com").to_string_lossy().to_string())
        );
    }

    #[test]
    fn test_resolve_move_status_reports_left_in_place_when_the_move_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let quarantine = tmp.path().join("quarantine");
        std::fs::create_dir_all(&quarantine).unwrap();
        // Nothing was moved — the quarantine dir stays empty.
        let (action, qpath) =
            resolve_move_status("/some/share/infected.bin", &quarantine.to_string_lossy());
        assert_eq!(action, "left_in_place");
        assert_eq!(qpath, None);
    }

    #[test]
    fn test_definitions_status_shape_is_consistent() {
        // `available` must be true iff a timestamp was resolved — the two
        // fields can never disagree, regardless of what's on this host.
        let s = definitions_status();
        assert_eq!(s.available, s.last_updated.is_some());
    }

    #[test]
    fn test_av_scan_summary_serializes_expected_fields() {
        let json = serde_json::to_value(AvScanSummary {
            files_scanned: 12,
            threats_found: 1,
        })
        .unwrap();
        assert_eq!(json["files_scanned"], serde_json::json!(12));
        assert_eq!(json["threats_found"], serde_json::json!(1));
    }

    // -- ROADMAP 13.5: the quarantine directory reaches the clamscan argv ---

    fn roots() -> Vec<std::path::PathBuf> {
        vec![
            std::path::PathBuf::from("/var/lib/homevault"),
            std::path::PathBuf::from("/mnt/data"),
        ]
    }

    #[test]
    fn test_quarantine_dir_accepts_the_shipped_default_and_storage_roots() {
        assert_eq!(
            validate_quarantine_dir("/var/lib/homevault/quarantine", &roots()).unwrap(),
            "/var/lib/homevault/quarantine"
        );
        assert_eq!(
            validate_quarantine_dir("/mnt/data/quarantine", &roots()).unwrap(),
            "/mnt/data/quarantine"
        );
        // Trailing slashes are trimmed so the field round-trips.
        assert_eq!(
            validate_quarantine_dir("/mnt/data/quarantine/", &roots()).unwrap(),
            "/mnt/data/quarantine"
        );
    }

    #[test]
    fn test_quarantine_dir_refuses_anything_outside_the_managed_roots() {
        for bad in [
            "",
            "   ",
            "quarantine",              // relative
            "./quarantine",            // relative
            "/",                       // filesystem root
            "/etc",                    // outside every root
            "/var/lib/homevault/../etc", // traversal
            "/mnt/data/../etc",
            "/var/lib/homevault/quar\nantine",
        ] {
            assert!(
                validate_quarantine_dir(bad, &roots()).is_err(),
                "'{bad}' must not be accepted as a quarantine directory"
            );
        }
        // A sibling with a shared prefix is not "inside" the root.
        assert!(validate_quarantine_dir("/var/lib/homevault-evil", &roots()).is_err());
    }

    #[test]
    fn test_clamscan_argv_carries_the_archive_toggle_and_the_quarantine_dir() {
        let on = clamscan_args("/mnt/data/docs", "/var/lib/homevault/quarantine", true);
        assert!(on.contains(&"-r".to_string()));
        assert!(on.contains(&"--scan-archive=yes".to_string()));
        assert!(on.contains(&"--move=/var/lib/homevault/quarantine".to_string()));
        assert_eq!(on.last().map(String::as_str), Some("/mnt/data/docs"));

        let off = clamscan_args("/mnt/data/docs", "/var/lib/homevault/quarantine", false);
        assert!(off.contains(&"--scan-archive=no".to_string()));
        assert!(!off.iter().any(|a| a == "--scan-archive=yes"));
    }
}
