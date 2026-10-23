/* =============================================================================
 * File          : journal.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   System Journal viewer: aggregates host logs from journalctl, a few
 *   well-known rotating /var/log text files, and HomeVault's own rotating
 *   application log. Endpoints list the available sources, read paged
 *   entries, clear a source (admin) and manage the automatic log-cleanup
 *   settings (admin).
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/journal in src/web/mod.rs; entry extraction shells out to
 *   journalctl / file reads per source (see the module doc for the source
 *   catalogue).
 * =============================================================================
 */

//! System Journal viewer — aggregates and serves host system logs via
//! `journalctl` (systemd journal) plus a few well-known rotating text logs
//! from `/var/log`, and HomeVault's own rotating application log files
//! under `crate::config::paths::logs_dir()` (source id `app`, written by
//! the `tracing-appender` file layer set up in `src/web/mod.rs::run()`).
//!
//! Two privilege levels, and they follow the data rather than the screen
//! (ROADMAP 20.1):
//!
//! - **Session** — the `app` source. Those files belong to this process, so
//!   reading them grants the caller nothing it did not already have.
//! - **Admin** — every host source: `system` and the `/var/log/*` text
//!   logs. `/var/log/auth.log` alone lists every account name, source
//!   address and sudo attempt on the box, so a non-admin session must not
//!   reach it. `GET /journal/sources` only advertises what the caller may
//!   actually read, so the screen never offers a source the backend will
//!   refuse.
//!
//! Clearing the `app` source's files is destructive and therefore
//! admin-gated too.

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use crate::handlers::AppStateArc;
use crate::storage::journal_cleanup_repo::JournalCleanupRepository;
use crate::utils::ResponseHelper;

/// Deadline for one `journalctl` / `tail` read. A wedged journald (or a
/// `/var/log` file on a stalled NFS mount) must not hold a handler forever.
pub const JOURNAL_READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Upper bound on concurrently running host reads. Each one forks `sudo`;
/// a burst of auto-refreshing desktops must not turn into a fork bomb.
const MAX_CONCURRENT_READS: usize = 4;

/// Hard cap on how many raw lines one filtered read may examine. `limit` is
/// clamped to 2000 by the handler, so `limit * SCAN_FACTOR` alone could
/// reach 40 000 — this is the ceiling that keeps a search bounded.
const MAX_SCAN_LINES: usize = 10_000;

/// How many raw lines to scan per requested entry when a search filter is
/// active. Searching happens *before* the limit is applied (ROADMAP 20.2),
/// so the window has to be wider than the page — otherwise a rare term is
/// invisible as soon as it is older than the last `limit` lines.
const SCAN_FACTOR: usize = 20;

/// Largest slice of one application log file read for a single query. The
/// reader takes the **tail**, so the newest entries are always kept; the cap
/// only bounds the memory a single request can allocate (ROADMAP 20.2).
const MAX_APP_LOG_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Longest systemd unit filter accepted (systemd's own unit-name limit).
const MAX_UNIT_NAME_LEN: usize = 256;

/// The read slots every request must hold while it runs a host command.
static READ_PERMITS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_READS)));

/// One journal entry as returned to the frontend.
#[derive(Serialize, Clone, Debug)]
pub struct JournalEntry {
    pub timestamp: Option<String>,
    pub unit: Option<String>,
    pub priority: Option<u8>,
    pub priority_text: Option<String>,
    pub pid: Option<String>,
    pub message: String,
}

/// A single log source the frontend can pick from.
#[derive(Serialize, Clone, Debug)]
pub struct JournalSource {
    pub id: String,
    pub label: String,
}

/// Query parameters for listing journal entries.
#[derive(Deserialize, Default, Debug)]
pub struct JournalQuery {
    /// Source id: `system` (default) or one of the text logs listed in
    /// `JournalSource` (e.g. `syslog`, `auth`, `kern`).
    #[serde(default)]
    pub source: Option<String>,
    /// Filter by systemd unit (e.g. `homevault.service`). Only valid for
    /// the `system` source.
    #[serde(default)]
    pub unit: Option<String>,
    /// Only show entries at or above this priority (0=emerg .. 7=debug).
    /// Defaults to 6 (info and above).
    #[serde(default)]
    pub priority: Option<u8>,
    /// Maximum number of lines to return (most recent first). Default 200.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Free-text case-insensitive substring filter on the message.
    #[serde(default)]
    pub search: Option<String>,
}

fn priority_text(p: u8) -> &'static str {
    match p {
        0 => "emerg",
        1 => "alert",
        2 => "crit",
        3 => "err",
        4 => "warning",
        5 => "notice",
        6 => "info",
        7 => "debug",
        _ => "unknown",
    }
}

/// One page of entries plus what the read actually covered.
struct JournalRead {
    entries: Vec<JournalEntry>,
    /// How many raw lines (or journal records) were examined.
    scanned: usize,
    /// True when the scan window filled up while a search filter was active,
    /// i.e. older matching entries may exist beyond what was examined. The
    /// screen says so instead of showing an unexplained short list.
    search_truncated: bool,
}

/// Which privilege level a source needs (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceKind {
    /// HomeVault's own log files — session-level.
    App,
    /// journald and the `/var/log` text files — admin only.
    Host,
}

/// Resolve a source id to its privilege level, or `None` if it is not a
/// source this module can read at all.
fn source_kind(source: &str) -> Option<SourceKind> {
    match source {
        "app" => Some(SourceKind::App),
        "system" | "syslog" | "auth" | "kern" | "daemon" => Some(SourceKind::Host),
        _ => None,
    }
}

/// How many raw lines a read with this page size may examine.
///
/// `min`/`max` rather than `clamp`: `clamp` panics when the floor exceeds the
/// ceiling, and a caller asking for more than `MAX_SCAN_LINES` entries must
/// still get a window at least as large as its page.
fn scan_limit(limit: usize) -> usize {
    limit
        .saturating_mul(SCAN_FACTOR)
        .min(MAX_SCAN_LINES)
        .max(limit)
}

/// Validate a systemd unit filter before it becomes `-u<value>` in
/// `journalctl`'s argv. It is not an injection (the value is one argv
/// entry), but a typo currently surfaces as raw `journalctl` stderr; a 400
/// naming the accepted characters is the honest answer.
fn validate_unit_filter(unit: &str) -> Result<&str, String> {
    let unit = unit.trim();
    if unit.is_empty() {
        return Err("Unit filter is empty".to_string());
    }
    if unit.len() > MAX_UNIT_NAME_LEN {
        return Err(format!(
            "Unit filter is longer than {} characters",
            MAX_UNIT_NAME_LEN
        ));
    }
    if !unit
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':' | '\\'))
    {
        return Err(
            "Unit filter may only contain letters, digits, '.', '-', '_', '@' and ':'".to_string(),
        );
    }
    Ok(unit)
}

/// Turn the per-file failures of a clear into one result: `Ok` only when
/// every file was handled, so a partial clear is reported as a failure
/// instead of being silently recorded as done (ROADMAP 20.3's shape).
fn clear_outcome(failures: Vec<String>) -> Result<(), String> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}


/// List the log sources this caller may read.
///
/// A non-admin only ever sees `app`: the host sources are not just refused
/// on read, they are not advertised at all — the screen must not offer a
/// source the backend will answer 403 for (ROADMAP 20.1).
pub async fn handle_journal_sources(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    let username = match crate::handlers::admin_guard::require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let mut sources = vec![JournalSource {
        id: "app".to_string(),
        label: "HomeVault Application Log".to_string(),
    }];

    if !state.auth.is_admin(&username).await {
        return Json(json!({
            "success": true,
            "data": sources
        }))
        .into_response();
    }

    sources.push(JournalSource {
        id: "system".to_string(),
        label: "System (journald)".to_string(),
    });

    // Only advertise a /var/log text source if the file actually exists on
    // this host — `installer/install.sh` doesn't install/require rsyslog,
    // and modern Debian/Ubuntu increasingly relies on journald alone, so
    // these can legitimately be absent even on a real deployment. A plain
    // `exists()` check only needs traversal permission on /var/log, not
    // read access to the file itself (that part still goes through `sudo
    // tail` in `read_entries`), so this correctly reflects real presence
    // regardless of the unprivileged homevault process's own read rights.
    for (id, path, label) in [
        ("syslog", "/var/log/syslog", "/var/log/syslog"),
        ("auth", "/var/log/auth.log", "/var/log/auth.log"),
        ("kern", "/var/log/kern.log", "/var/log/kern.log"),
        ("daemon", "/var/log/daemon.log", "/var/log/daemon.log"),
    ] {
        if std::path::Path::new(path).exists() {
            sources.push(JournalSource {
                id: id.to_string(),
                label: label.to_string(),
            });
        }
    }

    Json(json!({
        "success": true,
        "data": sources
    }))
    .into_response()
}

/// List journal entries for a given source with optional filters.
///
/// The `app` source is session-level; every host source is admin-only
/// (ROADMAP 20.1). An unknown source is a 400 rather than a silent empty
/// page, and an unusable unit filter is a 400 rather than raw `journalctl`
/// stderr surfacing in the UI.
pub async fn handle_journal_entries(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<JournalQuery>,
) -> Response {
    let username = match crate::handlers::admin_guard::require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    let source = query.source.clone().unwrap_or_else(|| "app".to_string());
    let kind = match source_kind(&source) {
        Some(kind) => kind,
        None => return ResponseHelper::json_error(format!("Unknown log source: {}", source)),
    };
    if kind == SourceKind::Host && !state.auth.is_admin(&username).await {
        return ResponseHelper::json_forbidden(
            "Host log sources are restricted to administrators".to_string(),
        );
    }

    // Validate before the value reaches `journalctl`'s argv.
    if let Some(unit) = query.unit.as_deref() {
        if !unit.trim().is_empty() {
            if let Err(msg) = validate_unit_filter(unit) {
                return ResponseHelper::json_error(msg);
            }
        }
    }

    let limit = query.limit.unwrap_or(200).clamp(1, 2000);
    let search = query.search.clone().filter(|s| !s.trim().is_empty());

    // Run the blocking subprocess work off the async worker, and hold one of
    // the read slots for its whole duration.
    let permit = match READ_PERMITS.clone().acquire_owned().await {
        Ok(p) => p,
        Err(_) => {
            return ResponseHelper::json_internal_error(
                "Journal reader is unavailable".to_string(),
            )
        }
    };
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read_entries(&source, &query, limit, search.as_deref())
    })
    .await;

    match result {
        Ok(Ok(read)) => Json(json!({
            "success": true,
            "data": read.entries,
            "meta": {
                "scanned": read.scanned,
                "limit": limit,
                "search_truncated": read.search_truncated,
            }
        }))
        .into_response(),
        Ok(Err(e)) => Json(json!({
            "success": false,
            "error": e
        }))
        .into_response(),
        Err(_) => Json(json!({
            "success": false,
            "error": "Internal server error reading journal"
        }))
        .into_response(),
    }
}

/// Delete HomeVault's own rotating application log files (source id
/// `app`). Never touches journald or `/var/log` — those host-level
/// sources are read-only and out of scope here. Admin-gated and destructive,
/// so the frontend requires an explicit confirmation before calling this.
pub async fn handle_journal_clear(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let result = tokio::task::spawn_blocking(clear_app_logs).await;

    match result {
        Ok(Ok(())) => Json(json!({
            "success": true,
            "message": "Application logs cleared"
        }))
        .into_response(),
        Ok(Err(e)) => Json(json!({
            "success": false,
            "error": e
        }))
        .into_response(),
        Err(_) => Json(json!({
            "success": false,
            "error": "Internal server error clearing logs"
        }))
        .into_response(),
    }
}

/// Request body for updating the auto-cleanup schedule.
#[derive(Deserialize, Debug)]
pub struct JournalCleanupSettingsRequest {
    pub auto_cleanup_enabled: bool,
    pub interval_days: u32,
}

/// Read the current auto-cleanup setting (on/off + interval in days). See
/// `journal_cleanup_scheduler.rs` for the background task that acts on it.
pub async fn handle_journal_cleanup_settings_get(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    let repo = JournalCleanupRepository::new(state.db.pool());
    match repo.get_settings().await {
        Ok(settings) => Json(json!({ "success": true, "data": settings })).into_response(),
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Failed to load cleanup settings: {}", e)
        }))
        .into_response(),
    }
}

/// Update the auto-cleanup setting. Admin-gated, since it controls a
/// recurring destructive action; the frontend surfaces it as an explicit,
/// user-chosen preference rather than something silently defaulted on.
pub async fn handle_journal_cleanup_settings_update(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    Json(body): Json<JournalCleanupSettingsRequest>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_admin(&state, &headers).await {
        return resp;
    }

    let repo = JournalCleanupRepository::new(state.db.pool());
    match repo
        .save_settings(body.auto_cleanup_enabled, body.interval_days)
        .await
    {
        Ok(()) => {
            Json(json!({ "success": true, "message": "Cleanup settings saved" })).into_response()
        }
        Err(e) => Json(json!({
            "success": false,
            "error": format!("Failed to save cleanup settings: {}", e)
        }))
        .into_response(),
    }
}

/// Returns `true` for filenames belonging to the `tracing-appender` daily
/// rotation of our own log (e.g. `homevault.log`, `homevault.log.2026-07-17`).
fn is_app_log_filename(name: &str) -> bool {
    name.starts_with("homevault.log")
}

/// Shared with `journal_cleanup_scheduler.rs`'s background auto-cleanup.
pub(crate) fn clear_app_logs() -> Result<(), String> {
    let dir = crate::config::paths::logs_dir();
    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        // Nothing has been logged to disk yet — already "clear".
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Failed to read logs directory: {}", e)),
    };

    // `tracing-appender`'s daily rotation keeps today's file
    // (`homevault.log.<UTC-date>`) open for the whole process lifetime and
    // only ever re-opens it at the next UTC midnight. `remove_file` on that
    // one unlinks the directory entry while the process keeps writing to
    // the now-orphaned inode — so the UI shows it gone but every event for
    // the rest of the day is silently lost. Truncate that file in place
    // instead (same inode, appender keeps writing, log immediately empty);
    // only actually delete the older, already-rotated files.
    let active_name = format!("homevault.log.{}", chrono::Utc::now().format("%Y-%m-%d"));

    // Every failure is collected instead of returning on the first one: the
    // caller (the admin button and the cleanup scheduler) must learn that the
    // clear was only partial, and the files after the failing one must still
    // be handled.
    let mut failures: Vec<String> = Vec::new();

    for entry in read_dir {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("Failed to read a logs directory entry: {}", e));
                continue;
            }
        };
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if is_app_log_filename(n) => n.to_string(),
            _ => continue,
        };

        if name == active_name || name == "homevault.log" {
            if let Err(e) = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
            {
                failures.push(format!("Failed to truncate {}: {}", path.display(), e));
            }
        } else if let Err(e) = std::fs::remove_file(&path) {
            failures.push(format!("Failed to remove {}: {}", path.display(), e));
        }
    }

    clear_outcome(failures)
}

/// Read at most the last `max_bytes` of a log file.
///
/// The whole-file `read_to_string` this replaces had no cap at all, so one
/// request could materialise a multi-hundred-megabyte file (ROADMAP 20.2).
/// When the file is longer than the cap the seek can land mid-line, so the
/// first (partial) line is dropped rather than parsed as a message.
fn read_file_tail(path: &std::path::Path, max_bytes: u64) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
    let len = file
        .metadata()
        .map_err(|e| format!("Failed to stat {}: {}", path.display(), e))?
        .len();

    let truncated = len > max_bytes;
    if truncated {
        file.seek(SeekFrom::Start(len - max_bytes))
            .map_err(|e| format!("Failed to seek {}: {}", path.display(), e))?;
    }

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let text = String::from_utf8_lossy(&buf).into_owned();

    if !truncated {
        return Ok(text);
    }
    // Drop the partial first line the seek created.
    Ok(match text.split_once('\n') {
        Some((_, rest)) => rest.to_string(),
        None => String::new(),
    })
}

/// Read the most recent entries from HomeVault's own application log files in
/// `logs_dir()`, newest file first, without loading the entire on-disk
/// history into memory.
///
/// The search filter runs **before** the page is cut, and the scan window is
/// `scan_limit(limit)` lines — so a rare term is still found when it is older
/// than the last `limit` lines (ROADMAP 20.2). When the window fills up the
/// caller is told (`search_truncated`) instead of being shown a short list
/// with no explanation.
fn read_app_log_entries(limit: usize, search: Option<&str>) -> Result<JournalRead, String> {
    let scan = scan_limit(limit);
    let dir = crate::config::paths::logs_dir();
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(is_app_log_filename)
                    .unwrap_or(false)
            })
            .collect(),
        // Nothing logged to disk yet (fresh install / just started).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        // A real read failure is reported, not folded into "no entries":
        // permission denied and "nothing logged yet" must stay distinguishable.
        Err(e) => return Err(format!("Failed to read logs directory: {}", e)),
    };
    // tracing-appender's daily rotation suffixes filenames with an
    // ISO-8601 date (`homevault.log.2026-07-17`), so a plain lexicographic
    // sort is also a chronological one.
    files.sort();

    let mut collected: Vec<String> = Vec::new();
    let mut truncated = false;
    for (idx, path) in files.iter().rev().enumerate() {
        let content = read_file_tail(path, MAX_APP_LOG_FILE_BYTES)?;
        let mut file_lines: Vec<String> = content.lines().map(String::from).collect();
        file_lines.extend(collected);
        collected = file_lines;
        if collected.len() >= scan {
            if collected.len() > scan {
                // Older lines are dropped: only the newest `scan` may be
                // examined, and the caller is told so.
                truncated = true;
                let start = collected.len() - scan;
                collected = collected.split_off(start);
            }
            // Any file behind this one is older and therefore unread.
            truncated = truncated || files.len() > idx + 1;
            break;
        }
    }

    let scanned = collected.len();
    let mut entries: Vec<JournalEntry> = Vec::new();
    for line in collected {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(s) = search {
            if !line.to_lowercase().contains(&s.to_lowercase()) {
                continue;
            }
        }
        entries.push(parse_app_log_line(&line));
    }

    if entries.len() > limit {
        let start = entries.len() - limit;
        entries = entries.split_off(start);
    }

    Ok(JournalRead {
        entries,
        scanned,
        search_truncated: search.is_some() && truncated,
    })
}

/// Parse one line of our own `tracing_subscriber::fmt` output
/// (`<rfc3339-timestamp>  <LEVEL> <rest>`). Falls back to an unparsed
/// message (no timestamp/priority) for anything that doesn't match —
/// e.g. a multi-line panic backtrace continuation.
fn parse_app_log_line(line: &str) -> JournalEntry {
    let fallback = || JournalEntry {
        timestamp: None,
        unit: None,
        priority: None,
        priority_text: None,
        pid: None,
        message: line.to_string(),
    };

    let trimmed = line.trim_start();
    let Some((ts_token, after_ts)) = trimmed.split_once(char::is_whitespace) else {
        return fallback();
    };
    let after_ts = after_ts.trim_start();
    let Some((level_token, rest)) = after_ts.split_once(char::is_whitespace) else {
        return fallback();
    };

    if chrono::DateTime::parse_from_rfc3339(ts_token).is_err() {
        return fallback();
    }

    let priority = match level_token.to_uppercase().as_str() {
        "ERROR" => 3,
        "WARN" => 4,
        "INFO" => 6,
        "DEBUG" | "TRACE" => 7,
        _ => return fallback(),
    };

    let (unit, message) = strip_target_prefix(rest.trim_start());

    JournalEntry {
        timestamp: Some(ts_token.to_string()),
        unit,
        priority: Some(priority),
        priority_text: Some(priority_text(priority).to_string()),
        pid: None,
        message,
    }
}

/// `tracing_subscriber::fmt`'s default formatter always prefixes the
/// message with the emitting `target` (e.g. `homevault::setup: ` or, for
/// events logged via `crate::events::log_event`, `homevault::event: `).
/// That's noise for a "simple to understand" log line, so it's pulled out
/// into the entry's `unit` field (rendered as a small badge, same as a
/// systemd unit name) instead of staying inline in the message text.
/// Falls back to leaving the line untouched if it doesn't look like a real
/// Rust module path (lowercase `snake_case` segments joined by `::`), so
/// this never mangles a message that just happens to contain a colon.
fn strip_target_prefix(msg: &str) -> (Option<String>, String) {
    if let Some(idx) = msg.find(": ") {
        let prefix = &msg[..idx];
        let looks_like_target = !prefix.is_empty()
            && prefix.split("::").all(|seg| {
                !seg.is_empty()
                    && seg
                        .chars()
                        .next()
                        .map(|c| c == '_' || c.is_ascii_lowercase())
                        .unwrap_or(false)
                    && seg.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            });
        if looks_like_target {
            return (Some(prefix.to_string()), msg[idx + 2..].to_string());
        }
    }
    (None, msg.to_string())
}

/// Run one command under a deadline.
///
/// Both pipes are drained on their own threads: a child that writes more than
/// a pipe buffer (a 10 000-line `journalctl` dump easily does) would block on
/// write and never reach `try_wait`, so the deadline could not fire at all.
/// On timeout the child is killed, so a wedged `journalctl` does not survive
/// the request (ROADMAP 20's "no timeout" finding).
fn run_timed(mut cmd: Command, deadline: Duration) -> Result<std::process::Output, String> {
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

    Ok(std::process::Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    })
}

fn read_entries(
    source: &str,
    query: &JournalQuery,
    limit: usize,
    search: Option<&str>,
) -> Result<JournalRead, String> {
    if source == "app" {
        return read_app_log_entries(limit, search);
    }

    // Scan a window wider than the page so the filter runs before the cut
    // (ROADMAP 20.2). The window is bounded, and `search_truncated` tells the
    // caller when it filled up.
    let scan = scan_limit(limit);

    let output = match source {
        "system" => {
            let mut cmd = Command::new("sudo");
            cmd.arg("journalctl");
            cmd.arg("--no-pager");
            cmd.arg("--output=json");
            cmd.arg("--quiet");
            if let Some(unit) = &query.unit {
                if !unit.trim().is_empty() {
                    cmd.arg(format!("-u{}", unit.trim()));
                }
            }
            if let Some(p) = query.priority {
                cmd.arg(format!("-p{}", p.clamp(0, 7)));
            }
            cmd.arg(format!("-n{}", scan));
            cmd.arg("--output-fields=__REALTIME_TIMESTAMP,_SYSTEMD_UNIT,PRIORITY,_PID,MESSAGE");
            run_timed(cmd, JOURNAL_READ_TIMEOUT)?
        }
        _ => {
            // Text log from /var/log — only allow the whitelisted names.
            let path = text_log_path(source)?;
            let mut cmd = Command::new("sudo");
            cmd.arg("tail").arg("-n").arg(scan.to_string()).arg(path);
            run_timed(cmd, JOURNAL_READ_TIMEOUT)?
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Command failed: {}", stderr.trim()));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();
    let mut scanned = 0usize;

    if source == "system" {
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            scanned += 1;
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(v) => {
                    let message = v
                        .get("MESSAGE")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(s) = search {
                        if !message.to_lowercase().contains(&s.to_lowercase()) {
                            continue;
                        }
                    }
                    let ts = v
                        .get("__REALTIME_TIMESTAMP")
                        .and_then(|t| t.as_str())
                        .and_then(|t| t.parse::<u64>().ok())
                        .map(seconds_to_iso);
                    let priority = v
                        .get("PRIORITY")
                        .and_then(|p| p.as_str())
                        .and_then(|p| p.parse::<u8>().ok());
                    let priority_text = priority.map(priority_text).map(String::from);
                    let pid = v.get("_PID").and_then(|p| p.as_str()).map(String::from);
                    let unit = v
                        .get("_SYSTEMD_UNIT")
                        .and_then(|u| u.as_str())
                        .map(String::from);
                    entries.push(JournalEntry {
                        timestamp: ts,
                        unit,
                        priority,
                        priority_text,
                        pid,
                        message,
                    });
                }
                Err(_) => continue,
            }
        }
    } else {
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            scanned += 1;
            if let Some(s) = search {
                if !line.to_lowercase().contains(&s.to_lowercase()) {
                    continue;
                }
            }
            entries.push(JournalEntry {
                timestamp: None,
                unit: None,
                priority: None,
                priority_text: None,
                pid: None,
                message: line.to_string(),
            });
        }
    }

    if entries.len() > limit {
        let start = entries.len() - limit;
        entries = entries.split_off(start);
    }

    Ok(JournalRead {
        entries,
        scanned,
        search_truncated: search.is_some() && scanned >= scan,
    })
}

/// Map a text-log source id to its absolute path. Only these are allowed.
fn text_log_path(source: &str) -> Result<String, String> {
    let path = match source {
        "syslog" => "/var/log/syslog",
        "auth" => "/var/log/auth.log",
        "kern" => "/var/log/kern.log",
        "daemon" => "/var/log/daemon.log",
        _ => return Err(format!("Unknown log source: {}", source)),
    };
    Ok(path.to_string())
}

/// Convert a microseconds-since-epoch value (journalctl's
/// `__REALTIME_TIMESTAMP`) into an ISO-8601 UTC string.
fn seconds_to_iso(micros: u64) -> String {
    let secs = micros / 1_000_000;
    let nanos = (micros % 1_000_000) * 1_000;
    use std::time::{Duration, UNIX_EPOCH};
    match UNIX_EPOCH.checked_add(Duration::new(secs, nanos as u32)) {
        Some(t) => {
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            dt.to_rfc3339()
        }
        None => micros.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- priority_text: numeric syslog priority → label ---

    #[test]
    fn test_priority_text_covers_every_level_and_falls_back() {
        assert_eq!(priority_text(0), "emerg");
        assert_eq!(priority_text(3), "err");
        assert_eq!(priority_text(4), "warning");
        assert_eq!(priority_text(6), "info");
        assert_eq!(priority_text(7), "debug");
        assert_eq!(priority_text(8), "unknown");
        assert_eq!(priority_text(255), "unknown");
    }

    // --- is_app_log_filename: which files belong to our own rotation ---

    #[test]
    fn test_is_app_log_filename() {
        assert!(is_app_log_filename("homevault.log"));
        assert!(is_app_log_filename("homevault.log.2026-07-17"));
        assert!(!is_app_log_filename("syslog"));
        assert!(!is_app_log_filename("other.log"));
        assert!(!is_app_log_filename("nginx-homevault.log"));
    }

    // --- text_log_path: source id → whitelisted /var/log path ---

    #[test]
    fn test_text_log_path_only_allows_the_whitelist() {
        assert_eq!(text_log_path("syslog").unwrap(), "/var/log/syslog");
        assert_eq!(text_log_path("auth").unwrap(), "/var/log/auth.log");
        assert_eq!(text_log_path("kern").unwrap(), "/var/log/kern.log");
        assert_eq!(text_log_path("daemon").unwrap(), "/var/log/daemon.log");
        assert!(text_log_path("../etc/passwd").is_err());
        assert!(text_log_path("system").is_err());
        assert!(text_log_path("app").is_err());
    }

    // --- strip_target_prefix: pull the `tracing` target out of the message ---

    #[test]
    fn test_strip_target_prefix_extracts_a_real_module_path() {
        assert_eq!(
            strip_target_prefix("homevault::setup: creating admin account"),
            (
                Some("homevault::setup".to_string()),
                "creating admin account".to_string()
            )
        );
        assert_eq!(
            strip_target_prefix("homevault::event: Admin 'x' created a share"),
            (
                Some("homevault::event".to_string()),
                "Admin 'x' created a share".to_string()
            )
        );
    }

    #[test]
    fn test_strip_target_prefix_leaves_a_plain_message_untouched() {
        // A colon that is not a module path — must not be mistaken for a target.
        assert_eq!(
            strip_target_prefix("Connection from 10.0.0.1: refused"),
            (None, "Connection from 10.0.0.1: refused".to_string())
        );
        assert_eq!(
            strip_target_prefix("no colon here"),
            (None, "no colon here".to_string())
        );
        // Capitalised first segment → not a snake_case module path.
        assert_eq!(
            strip_target_prefix("Warning: disk almost full"),
            (None, "Warning: disk almost full".to_string())
        );
    }

    // --- parse_app_log_line: one `tracing_subscriber::fmt` line → JournalEntry ---

    #[test]
    fn test_parse_app_log_line_with_timestamp_level_and_target() {
        let e = parse_app_log_line(
            "2026-07-17T10:30:00.123456Z  INFO homevault::auth: user 'nicolas' logged in",
        );
        assert_eq!(e.timestamp.as_deref(), Some("2026-07-17T10:30:00.123456Z"));
        assert_eq!(e.priority, Some(6));
        assert_eq!(e.priority_text.as_deref(), Some("info"));
        assert_eq!(e.unit.as_deref(), Some("homevault::auth"));
        assert_eq!(e.message, "user 'nicolas' logged in");
    }

    #[test]
    fn test_parse_app_log_line_maps_every_level() {
        for (lvl, prio) in [
            ("ERROR", 3),
            ("WARN", 4),
            ("INFO", 6),
            ("DEBUG", 7),
            ("TRACE", 7),
        ] {
            let e = parse_app_log_line(&format!("2026-07-17T10:30:00Z  {lvl} homevault::x: msg"));
            assert_eq!(e.priority, Some(prio), "{lvl}");
        }
    }

    #[test]
    fn test_parse_app_log_line_falls_back_for_unparseable_lines() {
        // No RFC3339 timestamp (a panic-backtrace continuation line).
        let e = parse_app_log_line("   at src/main.rs:42:1");
        assert_eq!(e.timestamp, None);
        assert_eq!(e.priority, None);
        assert_eq!(e.message, "   at src/main.rs:42:1");

        // Looks structured but the "level" is not a known one.
        let e = parse_app_log_line("2026-07-17T10:30:00Z  NOISE something happened");
        assert_eq!(e.priority, None);
        assert_eq!(e.message, "2026-07-17T10:30:00Z  NOISE something happened");

        // Empty / single token.
        assert_eq!(parse_app_log_line("hello").message, "hello");
    }

    // --- seconds_to_iso: journalctl __REALTIME_TIMESTAMP (µs) → ISO-8601 ---

    #[test]
    fn test_seconds_to_iso_converts_a_known_instant() {
        // 2021-01-01T00:00:00Z = 1_609_459_200 s = 1_609_459_200_000_000 µs
        let s = seconds_to_iso(1_609_459_200_000_000);
        assert!(s.starts_with("2021-01-01T00:00:00"), "got {s}");
        assert_eq!(seconds_to_iso(0), "1970-01-01T00:00:00+00:00");
    }

    // --- source_kind: which privilege level a source needs (ROADMAP 20.1) ---

    #[test]
    fn test_source_kind_puts_only_our_own_log_at_session_level() {
        assert_eq!(source_kind("app"), Some(SourceKind::App));
        for host in ["system", "syslog", "auth", "kern", "daemon"] {
            assert_eq!(source_kind(host), Some(SourceKind::Host), "{host}");
        }
        // Anything else is not a source at all — the handler answers 400
        // instead of letting `text_log_path` produce the error later.
        for bogus in ["notreal", "", "../etc/passwd", "APP"] {
            assert_eq!(source_kind(bogus), None, "{bogus}");
        }
    }

    // --- scan_limit: the search window is wider than the page, but bounded ---

    #[test]
    fn test_scan_limit_is_wider_than_the_page_and_capped() {
        // A small page scans SCAN_FACTOR times its size, so a rare term older
        // than the page is still found (ROADMAP 20.2).
        assert_eq!(scan_limit(10), 200);
        assert_eq!(scan_limit(200), 4_000);
        // The handler clamps `limit` to 2000; the window still stops at the
        // hard ceiling rather than growing to 40 000.
        assert_eq!(scan_limit(2_000), MAX_SCAN_LINES);
        // Never smaller than the page itself, even for a huge limit.
        assert!(scan_limit(20_000) >= 20_000);
    }

    // --- validate_unit_filter: reject a bad unit before it reaches argv ---

    #[test]
    fn test_validate_unit_filter_accepts_real_unit_names() {
        for ok in [
            "homevault.service",
            "systemd-journald.service",
            "docker@edge-1.service",
            "user@1000.slice",
            "nginx",
        ] {
            assert_eq!(validate_unit_filter(ok).unwrap(), ok, "{ok}");
        }
        // Surrounding whitespace is trimmed, not rejected.
        assert_eq!(validate_unit_filter("  nginx.service ").unwrap(), "nginx.service");
    }

    #[test]
    fn test_validate_unit_filter_rejects_shell_metacharacters_and_junk() {
        for bad in [
            "nginx; rm -rf /",
            "nginx service",
            "nginx && id",
            "$(id)",
            "nginx|cat",
            "",
            "   ",
        ] {
            assert!(validate_unit_filter(bad).is_err(), "{bad:?} should be refused");
        }
        // The 256-character systemd ceiling is enforced.
        let long = "a".repeat(MAX_UNIT_NAME_LEN + 1);
        assert!(validate_unit_filter(&long).is_err());
        let at_limit = "a".repeat(MAX_UNIT_NAME_LEN);
        assert!(validate_unit_filter(&at_limit).is_ok());
    }

    // --- clear_outcome: a partial clear is a failure, never a success ---

    #[test]
    fn test_clear_outcome_reports_every_failure() {
        assert!(clear_outcome(Vec::new()).is_ok());
        let err = clear_outcome(vec![
            "Failed to remove /a: Permission denied".to_string(),
            "Failed to truncate /b: Read-only file system".to_string(),
        ])
        .unwrap_err();
        // Both failures survive: the caller must not see a partial clear as a
        // success (ROADMAP 20.3's shape at the file level).
        assert!(err.contains("/a"), "{err}");
        assert!(err.contains("/b"), "{err}");
    }

    // --- run_timed: a wedged child is killed, a normal one returns its output ---

    #[test]
    fn test_run_timed_returns_a_fast_command_output() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'hello' && exit 0"]);
        let out = run_timed(cmd, Duration::from_secs(5)).expect("command should run");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
    }

    #[test]
    fn test_run_timed_keeps_the_failing_exit_code_and_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf 'nope' >&2 && exit 3"]);
        let out = run_timed(cmd, Duration::from_secs(5)).expect("command should run");
        assert!(!out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stderr), "nope");
    }

    #[test]
    fn test_run_timed_kills_a_hung_command_at_the_deadline() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let err = run_timed(cmd, Duration::from_millis(150)).unwrap_err();
        assert!(err.contains("did not finish within"), "{err}");
        // The deadline is honoured instead of waiting out the child.
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    // --- read_file_tail: bounded read, newest lines kept ---

    #[test]
    fn test_read_file_tail_returns_the_whole_file_when_it_fits() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("homevault.log");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(read_file_tail(&path, 1024).unwrap(), "one\ntwo\nthree\n");
    }

    #[test]
    fn test_read_file_tail_drops_the_line_cut_by_the_seek() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("homevault.log");
        // 40 bytes; a 20-byte cap seeks into the middle of a line.
        std::fs::write(&path, "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc\n").unwrap();
        let tail = read_file_tail(&path, 20).unwrap();
        assert!(!tail.contains("aaaaaaaaaa"), "the cut line must be dropped: {tail:?}");
        assert!(tail.contains("cccccccccc"), "{tail:?}");
        assert!(tail.len() <= 20, "{tail:?}");
    }

    #[test]
    fn test_read_file_tail_reports_an_unreadable_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("gone.log");
        let err = read_file_tail(&missing, 1024).unwrap_err();
        // A missing/permission-denied file is an error, not an empty log —
        // the caller must not render it as "no entries" (ROADMAP 20.2).
        assert!(err.contains("Failed to open"), "{err}");
        assert!(err.contains("gone.log"), "{err}");
    }
}
