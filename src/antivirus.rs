//! Antivirus scanning via ClamAV's `clamscan` CLI (on-demand + scheduled
//! only — no `clamd` real-time daemon, to stay within the low-power
//! hardware budget documented in `installer/homevault.service`).
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
pub fn clamscan_available() -> bool {
    std::process::Command::new("clamscan")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
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
    std::fs::create_dir_all(&settings.quarantine_dir)
        .map_err(|e| format!("Failed to create quarantine dir: {}", e))?;

    let scan = repo
        .create_scan(&scan_path, trigger)
        .await
        .map_err(|e| e.to_string())?;

    let result = run_clamscan(&scan_path, &settings.quarantine_dir, job_id.as_deref()).await;

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
            repo.finish_scan(&scan.id, "completed", summary.files_scanned, summary.threats_found, None)
                .await
                .map_err(|e| e.to_string())?;
            let _ = repo.set_last_scan_at(&Utc::now().to_rfc3339()).await;

            if let Some(jid) = &job_id {
                ProgressEmitter::complete(jid, &format!("Scan complete: {} threats found", summary.threats_found));
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

/// Spawns `clamscan` as an async child (not `spawn_blocking`) so we can
/// stream stdout line-by-line for live progress and keep a killable handle
/// for real cancellation via `cancel_scan()`.
async fn run_clamscan(
    scan_path: &str,
    quarantine_dir: &str,
    job_id: Option<&str>,
) -> Result<(AvScanSummary, Vec<ThreatMatch>), String> {
    let mut child = Command::new("clamscan")
        .args([
            "-r",
            "-i",
            "--no-summary",
            &format!("--move={}", quarantine_dir),
            scan_path,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start clamscan: {}", e))?;

    if let Some(pid) = child.id() {
        *AV_SCAN_PID.lock() = Some(pid);
    }

    let stdout = child.stdout.take().ok_or("Failed to capture clamscan stdout")?;
    let stderr = child.stderr.take().ok_or("Failed to capture clamscan stderr")?;
    // Drained concurrently with stdout below so a chatty stderr (e.g. missing
    // virus database, permission errors) can't fill its OS pipe buffer and
    // deadlock the child while we're only reading stdout.
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = tokio::io::AsyncReadExt::read_to_string(&mut BufReader::new(stderr), &mut buf).await;
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
                    &format!("{} files scanned, {} threats found", files_scanned, threats.len()),
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
                Err(format!("clamscan reported an error scanning '{}'", scan_path))
            } else {
                Err(format!("clamscan reported an error scanning '{}': {}", scan_path, detail))
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
    let pid = *AV_SCAN_PID.lock();
    match pid {
        Some(pid) => {
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .output()
                .await;
            true
        }
        None => false,
    }
}

/// Delete a quarantined file's on-disk copy and mark the threat resolved.
pub async fn resolve_threat(pool: Arc<sqlx::SqlitePool>, threat_id: &str, delete: bool) -> Result<(), String> {
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

    repo.resolve_threat(threat_id).await.map_err(|e| e.to_string())
}

async fn notify_scan(pool: Arc<sqlx::SqlitePool>, threats_found: u64, scan_path: &str) {
    let repo = NotificationRepository::new(pool);
    let (title, severity, message) = if threats_found > 0 {
        (
            "Antivirus scan found threats",
            "critical",
            format!("{} threat(s) found and quarantined while scanning '{}'.", threats_found, scan_path),
        )
    } else {
        (
            "Antivirus scan completed",
            "info",
            format!("No threats found scanning '{}'.", scan_path),
        )
    };
    let _ = repo.create(title, &message, severity, "antivirus", "antivirus_scan").await;
}

/// Registers a new SSE progress job for a scan (a `Custom` job type — no
/// dedicated antivirus variant exists on `JobType` yet).
pub fn create_scan_job() -> String {
    PROGRESS_MANAGER.create_job(JobType::Custom("antivirus_scan".to_string()), "Starting antivirus scan")
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
        assert_eq!(qpath, Some(quarantine.join("eicar.com").to_string_lossy().to_string()));
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
}
