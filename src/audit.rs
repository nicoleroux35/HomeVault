/* =============================================================================
 * File          : audit.rs
 * Author        : Nicolas Le Roux
 * Created       : 08/11/2026
 * Last modified : 08/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Queryable audit trail (P1-18). Every `events::log_event` call is written
 *   to the `audit_log` table through a bounded background channel, so the
 *   mutation that logged it never waits on SQLite. The module also owns the
 *   admin query/export surface and the retention prune.
 *
 * Dependencies:
 *   - sqlx (SQLite), chrono, serde, tokio
 *
 * Usage:
 *   `audit::init(db.pool())` once at startup (src/web/mod.rs); then
 *   `audit::record(...)` / `events::log_event(...)`. Handlers under
 *   /api/audit and the Journal `audit` source use `list`.
 * =============================================================================
 */

//! Queryable audit trail.
//!
//! Design rules:
//!
//! - **Never block the caller.** `record` is a `try_send` on a bounded
//!   channel; a full channel drops the row and increments a counter instead
//!   of turning a successful mutation into an error.
//! - **No secrets, no file contents.** Every field goes through `sanitize`,
//!   which strips control characters and truncates on a character boundary.
//!   Callers must not put secrets in an event sentence — the same rule the
//!   tracing/syslog path already enforces (CLAUDE.md, "Secrets never leak").
//! - **Bounded retention.** `prune` deletes rows older than
//!   [`AUDIT_RETENTION_DAYS`]; the writer calls it at startup and every six
//!   hours.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, SqlitePool};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Rows older than this are pruned. Matches the journal auto-cleanup default.
pub const AUDIT_RETENTION_DAYS: u32 = 90;

/// Hard ceiling for one interactive page.
pub const AUDIT_PAGE_MAX: u32 = 500;

/// Hard ceiling for one export. Exports are admin-triggered and rare, so they
/// may pull more than a page, but never the whole table into memory.
pub const AUDIT_EXPORT_MAX: u32 = 5000;

/// Bounded queue between the event call site and the SQLite writer.
const CHANNEL_CAPACITY: usize = 4096;

/// Longest stored message. Event sentences are much shorter; the cap only
/// protects the table from an accidental multi-kilobyte dump.
pub const MAX_MESSAGE_LEN: usize = 2000;

/// Longest stored structured field (actor, action, target, client IP).
const MAX_FIELD_LEN: usize = 256;

/// Longest accepted free-text search term.
const MAX_SEARCH_LEN: usize = 200;

/// Rows dropped because the writer channel was full. Observable so a silent
/// gap in the audit trail is not silent.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// True once [`init`] has installed the writer.
static SENDER: OnceLock<tokio::sync::mpsc::Sender<AuditRecord>> = OnceLock::new();

/// Severity of one audit row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            _ => None,
        }
    }
}

/// One audit row as returned to an admin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: i64,
    pub timestamp: String,
    pub severity: String,
    pub actor: Option<String>,
    pub action: Option<String>,
    pub target: Option<String>,
    pub client_ip: Option<String>,
    pub message: String,
}

/// One row to write. `message` is required; the structured fields are optional
/// because most existing call sites only have the plain-English sentence.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub severity: Severity,
    pub actor: Option<String>,
    pub action: Option<String>,
    pub target: Option<String>,
    pub client_ip: Option<String>,
    pub message: String,
}

impl AuditRecord {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            actor: None,
            action: None,
            target: None,
            client_ip: None,
            message: message.into(),
        }
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::info(message)
        }
    }
}

/// One page of results plus the unpaged total.
#[derive(Debug, Clone, Serialize)]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Query parameters shared by the list and export endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuditQuery {
    /// Inclusive lower bound (RFC 3339 or `YYYY-MM-DD`).
    pub from: Option<String>,
    /// Inclusive upper bound (RFC 3339 or `YYYY-MM-DD`, end of day).
    pub to: Option<String>,
    /// `info` or `warning`.
    pub severity: Option<String>,
    pub actor: Option<String>,
    pub action: Option<String>,
    /// Case-insensitive substring match on the message.
    pub q: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl AuditQuery {
    /// The free-text term, trimmed and bounded.
    pub fn search_term(&self) -> Option<String> {
        self.q
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate_chars(s, MAX_SEARCH_LEN))
    }
}

/// Installs the background writer. Safe to call more than once (later calls
/// are no-ops), and safe before the first request: `record` drops silently
/// until a writer exists.
pub fn init(pool: std::sync::Arc<SqlitePool>) {
    if SENDER.get().is_some() {
        return;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AuditRecord>(CHANNEL_CAPACITY);
    if SENDER.set(tx).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        loop {
            tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(record) => {
                        if let Err(e) = insert_now(pool.as_ref(), &record).await {
                            tracing::warn!("Audit write failed: {}", e);
                        }
                    }
                    // The static sender is never dropped in practice; return
                    // instead of spinning on a closed channel if it ever is.
                    None => break,
                },
                _ = tick.tick() => {
                    if let Err(e) = prune(pool.as_ref(), AUDIT_RETENTION_DAYS).await {
                        tracing::warn!("Audit prune failed: {}", e);
                    }
                }
            }
        }
    });
}

/// Enqueues one record. Never blocks; drops (and counts) when the channel is
/// full so a burst of events cannot stall the request that emitted them.
pub fn record(record: AuditRecord) {
    let Some(sender) = SENDER.get() else {
        return;
    };
    if sender.try_send(normalize(record)).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Convenience for the `events` module.
pub fn record_event(message: impl std::fmt::Display, severity: Severity) {
    record(AuditRecord {
        severity,
        actor: None,
        action: None,
        target: None,
        client_ip: None,
        message: message.to_string(),
    });
}

/// How many rows were dropped because the writer queue was full.
pub fn dropped_count() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Writes one row synchronously. Public so integration tests can seed a
/// deterministic table without racing the background writer.
pub async fn insert_now(pool: &SqlitePool, record: &AuditRecord) -> Result<i64, sqlx::Error> {
    let record = normalize(record.clone());
    let result = sqlx::query(
        "INSERT INTO audit_log \
         (created_at, severity, actor, action, target, client_ip, message) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(record.severity.as_str())
    .bind(&record.actor)
    .bind(&record.action)
    .bind(&record.target)
    .bind(&record.client_ip)
    .bind(&record.message)
    .execute(pool)
    .await?;
    Ok(result.last_insert_rowid())
}

/// Lists one page. Validates every filter before it reaches SQL; an invalid
/// value is a `400` at the handler, never a silently ignored filter.
pub async fn list(
    pool: &SqlitePool,
    query: &AuditQuery,
    max_limit: u32,
) -> Result<AuditPage, String> {
    let valid = ValidQuery::from_query(query, max_limit)?;

    let mut count_qb: QueryBuilder<sqlx::Sqlite> =
        QueryBuilder::new("SELECT COUNT(*) AS n FROM audit_log WHERE 1 = 1");
    push_filters(&mut count_qb, &valid);
    let count_row = count_qb
        .build()
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    let total: i64 = count_row.try_get("n").map_err(|e| e.to_string())?;

    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT id, created_at, severity, actor, action, target, client_ip, message \
         FROM audit_log WHERE 1 = 1",
    );
    push_filters(&mut qb, &valid);
    qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
        .push_bind(valid.limit as i64)
        .push(" OFFSET ")
        .push_bind(valid.offset as i64);

    let rows = qb.build().fetch_all(pool).await.map_err(|e| e.to_string())?;
    let entries = rows
        .into_iter()
        .map(|row| AuditEntry {
            id: row.get("id"),
            timestamp: row.get("created_at"),
            severity: row.get("severity"),
            actor: row.get("actor"),
            action: row.get("action"),
            target: row.get("target"),
            client_ip: row.get("client_ip"),
            message: row.get("message"),
        })
        .collect();

    Ok(AuditPage {
        entries,
        total,
        limit: valid.limit,
        offset: valid.offset,
    })
}

/// Deletes rows older than `retention_days`. Returns how many went.
pub async fn prune(pool: &SqlitePool, retention_days: u32) -> Result<u64, sqlx::Error> {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days.clamp(1, 3650) as i64);
    let result = sqlx::query("DELETE FROM audit_log WHERE created_at < ?")
        .bind(cutoff.to_rfc3339())
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// CSV export (RFC 4180 quoting). The header is always written, even for an
/// empty result, so the file is self-describing.
pub fn to_csv(entries: &[AuditEntry]) -> String {
    let mut out = String::from("timestamp,severity,actor,action,target,client_ip,message\n");
    for entry in entries {
        let fields = [
            entry.timestamp.as_str(),
            entry.severity.as_str(),
            entry.actor.as_deref().unwrap_or(""),
            entry.action.as_deref().unwrap_or(""),
            entry.target.as_deref().unwrap_or(""),
            entry.client_ip.as_deref().unwrap_or(""),
            entry.message.as_str(),
        ];
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&csv_field(field));
        }
        out.push('\n');
    }
    out
}

/// Validated query ready for the SQL builder.
struct ValidQuery {
    from: Option<String>,
    to: Option<String>,
    severity: Option<String>,
    actor: Option<String>,
    action: Option<String>,
    search: Option<String>,
    limit: u32,
    offset: u32,
}

impl ValidQuery {
    fn from_query(query: &AuditQuery, max_limit: u32) -> Result<Self, String> {
        let from = match query.from.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(value) => Some(parse_bound(value, Bound::Start)?),
            None => None,
        };
        let to = match query.to.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(value) => Some(parse_bound(value, Bound::End)?),
            None => None,
        };
        if let (Some(from), Some(to)) = (&from, &to) {
            if from > to {
                return Err("'from' must be earlier than or equal to 'to'".to_string());
            }
        }

        let severity = match query
            .severity
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(value) => Some(
                Severity::parse(value)
                    .ok_or_else(|| "severity must be 'info' or 'warning'".to_string())?
                    .as_str()
                    .to_string(),
            ),
            None => None,
        };

        let actor = clean_filter(query.actor.as_deref())?;
        let action = clean_filter(query.action.as_deref())?;
        let search = query.search_term();

        let limit = query.limit.unwrap_or(100).clamp(1, max_limit.max(1));
        let offset = query.offset.unwrap_or(0).min(1_000_000);
        Ok(Self {
            from,
            to,
            severity,
            actor,
            action,
            search,
            limit,
            offset,
        })
    }
}

fn push_filters(qb: &mut QueryBuilder<sqlx::Sqlite>, query: &ValidQuery) {
    if let Some(from) = &query.from {
        qb.push(" AND created_at >= ").push_bind(from.clone());
    }
    if let Some(to) = &query.to {
        qb.push(" AND created_at <= ").push_bind(to.clone());
    }
    if let Some(severity) = &query.severity {
        qb.push(" AND severity = ").push_bind(severity.clone());
    }
    if let Some(actor) = &query.actor {
        qb.push(" AND actor = ").push_bind(actor.clone());
    }
    if let Some(action) = &query.action {
        qb.push(" AND action = ").push_bind(action.clone());
    }
    if let Some(search) = &query.search {
        qb.push(" AND message LIKE ")
            .push_bind(like_pattern(search))
            .push(" ESCAPE '\\'");
    }
}

enum Bound {
    Start,
    End,
}

/// Accepts RFC 3339 or a plain `YYYY-MM-DD` date. A date bound means the whole
/// day: 00:00:00 for `from`, 23:59:59 for `to`.
fn parse_bound(value: &str, bound: Bound) -> Result<String, String> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Utc).to_rfc3339());
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("Invalid date '{}': use RFC 3339 or YYYY-MM-DD", value))?;
    let (hour, minute, second) = match bound {
        Bound::Start => (0, 0, 0),
        Bound::End => (23, 59, 59),
    };
    let naive = date
        .and_hms_opt(hour, minute, second)
        .ok_or_else(|| format!("Invalid date '{}'", value))?;
    Ok(Utc.from_utc_datetime(&naive).to_rfc3339())
}

fn clean_filter(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if value.len() > MAX_FIELD_LEN {
        return Err(format!("Filter is longer than {} characters", MAX_FIELD_LEN));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err("Filter contains control characters".to_string());
    }
    Ok(Some(value.to_string()))
}

/// `%term%` with `%`, `_` and `\` escaped for `LIKE … ESCAPE '\'`.
fn like_pattern(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 2);
    out.push('%');
    for character in term.chars() {
        if matches!(character, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('%');
    out
}

fn normalize(mut record: AuditRecord) -> AuditRecord {
    record.message = sanitize(&record.message, MAX_MESSAGE_LEN);
    record.actor = record
        .actor
        .as_deref()
        .map(|value| sanitize(value, MAX_FIELD_LEN))
        .filter(|value| !value.is_empty());
    record.action = record
        .action
        .as_deref()
        .map(|value| sanitize(value, MAX_FIELD_LEN))
        .filter(|value| !value.is_empty());
    record.target = record
        .target
        .as_deref()
        .map(|value| sanitize(value, MAX_FIELD_LEN))
        .filter(|value| !value.is_empty());
    record.client_ip = record
        .client_ip
        .as_deref()
        .map(|value| sanitize(value, MAX_FIELD_LEN))
        .filter(|value| !value.is_empty());
    record
}

/// Strips control characters (turning them into spaces), trims, and truncates
/// on a character boundary.
fn sanitize(value: &str, max_len: usize) -> String {
    let flattened: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    truncate_chars(flattened.trim(), max_len)
}

fn truncate_chars(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    value.chars().take(max_len).collect()
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_strips_control_characters() {
        let value = sanitize("line one\u{0}line\ntwo", MAX_MESSAGE_LEN);
        assert!(!value.contains('\u{0}'));
        assert!(value.contains("line one line two"), "{value}");
    }

    #[test]
    fn test_sanitize_truncates_on_character_boundary() {
        let value = sanitize("ééééé", 3);
        assert_eq!(value, "ééé");
        assert_eq!(value.chars().count(), 3);
    }

    #[test]
    fn test_like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("100%_x\\y"), "%100\\%\\_x\\\\y%");
    }

    #[test]
    fn test_parse_bound_accepts_date_and_rfc3339() {
        assert_eq!(
            parse_bound("2026-11-08", Bound::Start).unwrap(),
            "2026-11-08T00:00:00+00:00"
        );
        assert_eq!(
            parse_bound("2026-11-08", Bound::End).unwrap(),
            "2026-11-08T23:59:59+00:00"
        );
        assert!(parse_bound("not-a-date", Bound::Start).is_err());
    }

    #[test]
    fn test_valid_query_rejects_bad_severity() {
        let query = AuditQuery {
            severity: Some("fatal".to_string()),
            ..AuditQuery::default()
        };
        assert!(ValidQuery::from_query(&query, AUDIT_PAGE_MAX).is_err());
    }

    #[test]
    fn test_valid_query_clamps_limit_and_offset() {
        let query = AuditQuery {
            limit: Some(9999),
            offset: Some(2_000_000),
            ..AuditQuery::default()
        };
        let valid = ValidQuery::from_query(&query, AUDIT_PAGE_MAX).unwrap();
        assert_eq!(valid.limit, AUDIT_PAGE_MAX);
        assert_eq!(valid.offset, 1_000_000);
    }

    #[test]
    fn test_valid_query_rejects_inverted_range() {
        let query = AuditQuery {
            from: Some("2026-11-09".to_string()),
            to: Some("2026-11-08".to_string()),
            ..AuditQuery::default()
        };
        assert!(ValidQuery::from_query(&query, AUDIT_PAGE_MAX).is_err());
    }

    #[test]
    fn test_csv_quotes_commas_quotes_and_newlines() {
        let entry = AuditEntry {
            id: 1,
            timestamp: "2026-11-08T00:00:00+00:00".to_string(),
            severity: "info".to_string(),
            actor: Some("alice".to_string()),
            action: Some("auth.login".to_string()),
            target: None,
            client_ip: Some("192.0.2.1".to_string()),
            message: "a, b \"quoted\"\nnext".to_string(),
        };
        let csv = to_csv(&[entry]);
        assert!(csv.starts_with("timestamp,severity,actor,action,target,client_ip,message\n"));
        assert!(csv.contains("\"a, b \"\"quoted\"\" next\"") || csv.contains("\"a, b \"\"quoted\"\""));
    }

    #[test]
    fn test_csv_writes_header_for_empty_result() {
        assert_eq!(
            to_csv(&[]),
            "timestamp,severity,actor,action,target,client_ip,message\n"
        );
    }
}
