//! System Journal viewer — aggregates and serves host system logs via
//! `journalctl` (systemd journal) plus a few well-known rotating text logs
//! from `/var/log`, and HomeVault's own rotating application log files
//! under `crate::config::paths::logs_dir()` (source id `app`, written by
//! the `tracing-appender` file layer set up in `src/web/mod.rs::run()`).
//! Unlike the host-level sources, `app` needs no `sudo`/journald access —
//! the homevault process owns those files directly. This is a privileged,
//! host-sensitive read: it reveals real process names, service errors, IPs
//! in connection logs, etc. — the same sensitivity class as the
//! Dashboard/Health endpoints, so every read handler requires a valid
//! session. Clearing the `app` source's files is destructive and therefore
//! admin-gated.

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Command;

use crate::handlers::AppStateArc;
use crate::storage::journal_cleanup_repo::JournalCleanupRepository;

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

/// List the available log sources.
pub async fn handle_journal_sources(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    let mut sources = vec![
        JournalSource {
            id: "app".to_string(),
            label: "HomeVault Application Log".to_string(),
        },
        JournalSource {
            id: "system".to_string(),
            label: "System (journald)".to_string(),
        },
    ];

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
pub async fn handle_journal_entries(
    State(state): State<AppStateArc>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<JournalQuery>,
) -> Response {
    if let Err(resp) = crate::handlers::admin_guard::require_auth(&state, &headers).await {
        return resp;
    }

    let source = query.source.clone().unwrap_or_else(|| "app".to_string());
    let limit = query.limit.unwrap_or(200).clamp(1, 2000);
    let search = query.search.clone().filter(|s| !s.trim().is_empty());

    // Run the blocking subprocess work off the async worker.
    let result = tokio::task::spawn_blocking(move || {
        read_entries(&source, &query, limit, search.as_deref())
    })
    .await;

    match result {
        Ok(Ok(entries)) => Json(json!({
            "success": true,
            "data": entries
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

    for entry in read_dir.filter_map(|e| e.ok()) {
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
                return Err(format!("Failed to truncate {}: {}", path.display(), e));
            }
        } else if let Err(e) = std::fs::remove_file(&path) {
            return Err(format!("Failed to remove {}: {}", path.display(), e));
        }
    }

    Ok(())
}

/// Read the most recent `limit` lines (after `search`) from HomeVault's own
/// application log files in `logs_dir()`, newest file first, without
/// necessarily loading the entire on-disk history into memory.
fn read_app_log_entries(limit: usize, search: Option<&str>) -> Result<Vec<JournalEntry>, String> {
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
        Err(_) => Vec::new(),
    };
    // tracing-appender's daily rotation suffixes filenames with an
    // ISO-8601 date (`homevault.log.2026-07-17`), so a plain lexicographic
    // sort is also a chronological one.
    files.sort();

    let mut collected: Vec<String> = Vec::new();
    for path in files.iter().rev() {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let mut file_lines: Vec<String> = content.lines().map(String::from).collect();
        file_lines.extend(collected);
        collected = file_lines;
        // Comfortably more raw lines than `limit` even after a restrictive
        // search filter removes most of them — stop reading further back.
        if collected.len() >= limit.saturating_mul(4).max(limit) {
            break;
        }
    }

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

    Ok(entries)
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

fn read_entries(
    source: &str,
    query: &JournalQuery,
    limit: usize,
    search: Option<&str>,
) -> Result<Vec<JournalEntry>, String> {
    if source == "app" {
        return read_app_log_entries(limit, search);
    }

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
            cmd.arg(format!("-n{}", limit));
            cmd.arg("--output-fields=__REALTIME_TIMESTAMP,_SYSTEMD_UNIT,PRIORITY,_PID,MESSAGE");
            cmd.output()
                .map_err(|e| format!("Failed to run journalctl: {}", e))?
        }
        _ => {
            // Text log from /var/log — only allow the whitelisted names.
            let path = text_log_path(source)?;
            let mut cmd = Command::new("sudo");
            cmd.arg("tail").arg("-n").arg(limit.to_string()).arg(path);
            cmd.output()
                .map_err(|e| format!("Failed to read log: {}", e))?
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Command failed: {}", stderr.trim()));
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();

    if source == "system" {
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
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

    Ok(entries)
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
}
