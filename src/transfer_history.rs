/* =============================================================================
 * File          : transfer_history.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Finished-transfer audit trail for the File Explorer (HV-ADV-02): one row
 *   per completed, failed or cancelled upload/download/copy/move, with the
 *   byte count, duration, how the copy was made (reflink method) and whether
 *   the bytes were verified. Written best-effort (a history write can never
 *   fail a transfer), read by its owner or an administrator, and pruned by
 *   retention.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   `record_transfer(pool, NewTransfer { .. })` from the Explorer handlers;
 *   `TransferHistoryRepository` for the `GET /api/explorer/transfers/history`
 *   query. See docs/guides/explorer.md → "Transfer history".
 * =============================================================================
 */

//! Finished-transfer audit trail (`transfer_history`, migration 073).
//!
//! The activity feed answers "what is running now"; this answers "what
//! happened". The split is deliberate: the feed drops a completed job on the
//! next poll so the taskbar stays a live view, while a transfer that finished
//! is exactly what an administrator needs days later (who moved 400 GB to the
//! archive share at 03:00, and was it a clone or a stream that re-hashed every
//! byte?).

use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Row, SqlitePool};
use std::sync::Arc;

/// How long a history row is kept. 90 days is what the roadmap asks for and
/// what an audit conversation usually reaches back to; retention is enforced
/// on write (see [`should_prune`]), so an idle appliance does not grow.
pub const RETENTION_DAYS: i64 = 90;

/// Rows returned when the caller does not ask for a size.
pub const DEFAULT_LIMIT: u32 = 50;
/// Hard ceiling, whatever the caller asks for — the endpoint must not be
/// usable to pull the whole table into one response.
pub const MAX_LIMIT: u32 = 200;

/// Operations the Explorer records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferOperation {
    Upload,
    Download,
    Copy,
    Move,
    /// An on-demand checksum run (`POST /api/explorer/checksum`). Recorded so
    /// "was it verified?" has an answer too.
    Checksum,
}

impl TransferOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Checksum => "checksum",
        }
    }

    /// Strict parse of a filter value — an unknown operation is a `400`, never
    /// silently an unfiltered list.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "upload" => Some(Self::Upload),
            "download" => Some(Self::Download),
            "copy" => Some(Self::Copy),
            "move" => Some(Self::Move),
            "checksum" => Some(Self::Checksum),
            _ => None,
        }
    }
}

/// How the transfer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Complete,
    Failed,
    Cancelled,
}

impl TransferStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "complete" | "completed" => Some(Self::Complete),
            "failed" => Some(Self::Failed),
            "cancelled" | "canceled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// A transfer to record. Built by the handlers, written once.
#[derive(Debug, Clone)]
pub struct NewTransfer {
    /// Progress-manager job id when the transfer ran as a tracked job.
    pub job_id: Option<String>,
    pub username: String,
    pub operation: TransferOperation,
    pub source_path: Option<String>,
    pub dest_path: Option<String>,
    pub bytes_total: u64,
    pub duration_ms: Option<u64>,
    /// Reflink cascade slug, or the digest algorithm for a checksum run.
    pub method: Option<String>,
    /// `true` when the bytes were proven identical (digest match / read-back).
    pub verified: bool,
    pub status: TransferStatus,
    pub error_message: Option<String>,
}

impl NewTransfer {
    /// A successful transfer of `bytes` by `username`.
    pub fn complete(username: impl Into<String>, operation: TransferOperation) -> Self {
        Self {
            job_id: None,
            username: username.into(),
            operation,
            source_path: None,
            dest_path: None,
            bytes_total: 0,
            duration_ms: None,
            method: None,
            verified: false,
            status: TransferStatus::Complete,
            error_message: None,
        }
    }

    pub fn job_id(mut self, job_id: impl Into<String>) -> Self {
        self.job_id = Some(job_id.into());
        self
    }

    pub fn source(mut self, path: impl Into<String>) -> Self {
        self.source_path = Some(path.into());
        self
    }

    pub fn dest(mut self, path: impl Into<String>) -> Self {
        self.dest_path = Some(path.into());
        self
    }

    pub fn bytes(mut self, bytes: u64) -> Self {
        self.bytes_total = bytes;
        self
    }

    pub fn duration_ms(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    pub fn verified(mut self, verified: bool) -> Self {
        self.verified = verified;
        self
    }

    pub fn failed(mut self, error: impl Into<String>) -> Self {
        self.status = TransferStatus::Failed;
        self.error_message = Some(error.into());
        self
    }

    pub fn cancelled(mut self) -> Self {
        self.status = TransferStatus::Cancelled;
        self
    }
}

/// One history row as the API returns it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TransferEntry {
    pub id: i64,
    pub job_id: Option<String>,
    pub username: String,
    pub operation: String,
    pub source_path: Option<String>,
    pub dest_path: Option<String>,
    pub bytes_total: i64,
    pub duration_ms: Option<i64>,
    pub method: Option<String>,
    pub verified: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

/// Filters accepted by `GET /api/explorer/transfers/history`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HistoryFilters {
    pub operation: Option<String>,
    pub status: Option<String>,
    /// Unix seconds — only rows completed at or after this instant.
    pub since: Option<i64>,
}

impl HistoryFilters {
    /// Validate and normalise. Returns the operation/status slugs to filter on
    /// (`None` = no filter) or a human-readable reason when a value is
    /// unknown: answering with an unfiltered list after a typo would tell the
    /// caller "no failed transfers" while they were looking at every row.
    pub fn validate(&self) -> Result<(Option<TransferOperation>, Option<TransferStatus>), String> {
        let operation = match self.operation.as_deref().map(str::trim) {
            Some(v) if !v.is_empty() => Some(
                TransferOperation::parse(v)
                    .ok_or_else(|| format!("Unknown transfer operation '{v}'"))?,
            ),
            _ => None,
        };
        let status = match self.status.as_deref().map(str::trim) {
            Some(v) if !v.is_empty() => Some(
                TransferStatus::parse(v).ok_or_else(|| format!("Unknown transfer status '{v}'"))?,
            ),
            _ => None,
        };
        Ok((operation, status))
    }
}

/// Clamp a requested page size to [`DEFAULT_LIMIT`]/[`MAX_LIMIT`].
pub fn clamp_limit(requested: Option<u32>) -> i64 {
    requested.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as i64
}

/// `true` when retention is due, given the last prune instant and now.
///
/// Once an hour at most: pruning on every write would put a `DELETE` on the
/// hot path of every upload, and 90-day retention does not need minute
/// granularity.
pub fn should_prune(last_prune: i64, now: i64) -> bool {
    now - last_prune >= 3600 || last_prune <= 0
}

/// Unix-seconds cutoff below which rows are expired.
pub fn retention_cutoff(now: i64) -> i64 {
    now - RETENTION_DAYS * 86_400
}

fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Last time this process pruned, in unix seconds (0 = never).
static LAST_PRUNE: AtomicI64 = AtomicI64::new(0);

/// Reads and writes `transfer_history`.
#[derive(Clone)]
pub struct TransferHistoryRepository {
    pool: Arc<SqlitePool>,
}

impl TransferHistoryRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Insert one row and return its id.
    pub async fn record(&self, transfer: &NewTransfer) -> Result<i64, sqlx::Error> {
        let now = now_epoch();
        let result = sqlx::query(
            "INSERT INTO transfer_history
                (job_id, username, operation, source_path, dest_path, bytes_total,
                 duration_ms, method, verified, status, error_message, created_at, completed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&transfer.job_id)
        .bind(&transfer.username)
        .bind(transfer.operation.as_str())
        .bind(&transfer.source_path)
        .bind(&transfer.dest_path)
        .bind(transfer.bytes_total as i64)
        .bind(transfer.duration_ms.map(|d| d as i64))
        .bind(&transfer.method)
        .bind(transfer.verified)
        .bind(transfer.status.as_str())
        .bind(&transfer.error_message)
        .bind(now)
        .bind(now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Rows for `username` (or every user when `all_users`), newest first.
    ///
    /// `all_users` is the administrator's view; the handler decides it and the
    /// query binds the username otherwise, so a non-admin can never widen the
    /// scope by crafting a filter.
    pub async fn list(
        &self,
        username: &str,
        all_users: bool,
        operation: Option<TransferOperation>,
        status: Option<TransferStatus>,
        since: Option<i64>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TransferEntry>, sqlx::Error> {
        let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
            "SELECT id, job_id, username, operation, source_path, dest_path, bytes_total, \
             duration_ms, method, verified, status, error_message, created_at, completed_at \
             FROM transfer_history WHERE 1 = 1",
        );
        if !all_users {
            qb.push(" AND username = ").push_bind(username.to_string());
        }
        if let Some(op) = operation {
            qb.push(" AND operation = ").push_bind(op.as_str());
        }
        if let Some(st) = status {
            qb.push(" AND status = ").push_bind(st.as_str());
        }
        if let Some(since) = since {
            qb.push(" AND completed_at >= ").push_bind(since);
        }
        qb.push(" ORDER BY completed_at DESC, id DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);

        let rows = qb.build().fetch_all(self.pool.as_ref()).await?;
        Ok(rows
            .into_iter()
            .map(|row| TransferEntry {
                id: row.get("id"),
                job_id: row.get("job_id"),
                username: row.get("username"),
                operation: row.get("operation"),
                source_path: row.get("source_path"),
                dest_path: row.get("dest_path"),
                bytes_total: row.get("bytes_total"),
                duration_ms: row.get("duration_ms"),
                method: row.get("method"),
                verified: row.get::<i64, _>("verified") != 0,
                status: row.get("status"),
                error_message: row.get("error_message"),
                created_at: row.get("created_at"),
                completed_at: row.get("completed_at"),
            })
            .collect())
    }

    /// Delete rows older than [`RETENTION_DAYS`]; returns how many went.
    pub async fn prune(&self, now: i64) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM transfer_history WHERE completed_at < ?")
            .bind(retention_cutoff(now))
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }
}

/// Record a finished transfer, **best-effort**.
///
/// History is bookkeeping, not the transfer: a database hiccup must never turn
/// a successful upload into an error the user has to retry (and cannot, because
/// the bytes are already on disk). Failures are logged and swallowed.
pub async fn record_transfer(pool: Arc<SqlitePool>, transfer: NewTransfer) {
    let repo = TransferHistoryRepository::new(pool.clone());
    if let Err(e) = repo.record(&transfer).await {
        tracing::warn!(
            "Could not record {} transfer for '{}' in the history: {e}",
            transfer.operation.as_str(),
            transfer.username
        );
        return;
    }
    let now = now_epoch();
    let last = LAST_PRUNE.load(Ordering::Relaxed);
    if should_prune(last, now) && LAST_PRUNE.swap(now, Ordering::Relaxed) != now {
        if let Err(e) = repo.prune(now).await {
            tracing::warn!("Could not prune the transfer history: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    //! Pure validation/parsing tests — no DB. Run: `cargo test transfer_history::tests`
    use super::*;

    #[test]
    fn test_operation_round_trips_through_its_slug() {
        for op in [
            TransferOperation::Upload,
            TransferOperation::Download,
            TransferOperation::Copy,
            TransferOperation::Move,
            TransferOperation::Checksum,
        ] {
            assert_eq!(TransferOperation::parse(op.as_str()), Some(op));
        }
    }

    #[test]
    fn test_operation_parse_is_case_insensitive_and_strict() {
        assert_eq!(
            TransferOperation::parse(" Copy "),
            Some(TransferOperation::Copy)
        );
        assert_eq!(TransferOperation::parse("delete"), None);
        assert_eq!(TransferOperation::parse(""), None);
    }

    #[test]
    fn test_status_parse_accepts_the_aliases_and_refuses_junk() {
        assert_eq!(
            TransferStatus::parse("completed"),
            Some(TransferStatus::Complete)
        );
        assert_eq!(
            TransferStatus::parse("COMPLETE"),
            Some(TransferStatus::Complete)
        );
        assert_eq!(
            TransferStatus::parse("canceled"),
            Some(TransferStatus::Cancelled)
        );
        assert_eq!(TransferStatus::parse("running"), None);
    }

    #[test]
    fn test_filters_validate_rejects_an_unknown_operation() {
        let filters = HistoryFilters {
            operation: Some("teleport".into()),
            status: None,
            since: None,
        };
        let err = filters.validate().expect_err("must refuse");
        assert!(err.contains("teleport"), "{err}");
    }

    #[test]
    fn test_filters_validate_rejects_an_unknown_status() {
        let filters = HistoryFilters {
            operation: None,
            status: Some("half-done".into()),
            since: None,
        };
        assert!(filters.validate().is_err());
    }

    #[test]
    fn test_filters_validate_treats_blank_as_no_filter() {
        let filters = HistoryFilters {
            operation: Some("   ".into()),
            status: Some(String::new()),
            since: Some(1_700_000_000),
        };
        assert_eq!(filters.validate().unwrap(), (None, None));
    }

    #[test]
    fn test_clamp_limit_defaults_and_caps() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT as i64);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10)), 10);
        assert_eq!(clamp_limit(Some(10_000)), MAX_LIMIT as i64);
    }

    #[test]
    fn test_should_prune_is_hourly() {
        assert!(should_prune(0, 1_000), "a process that never pruned does");
        assert!(!should_prune(1_000, 1_000 + 3_599));
        assert!(should_prune(1_000, 1_000 + 3_600));
        assert!(
            !should_prune(2_000, 1_000),
            "a clock step backwards must not spam the prune"
        );
    }

    #[test]
    fn test_retention_cutoff_is_ninety_days() {
        let now = 1_800_000_000;
        assert_eq!(retention_cutoff(now), now - 90 * 86_400);
    }

    #[test]
    fn test_new_transfer_builders_compose_the_row() {
        let t = NewTransfer::complete("alice", TransferOperation::Copy)
            .job_id("job-1")
            .source("/a/file.bin")
            .dest("/b/file.bin")
            .bytes(4096)
            .duration_ms(12)
            .method("clone")
            .verified(true);
        assert_eq!(t.username, "alice");
        assert_eq!(t.operation, TransferOperation::Copy);
        assert_eq!(t.status, TransferStatus::Complete);
        assert_eq!(t.job_id.as_deref(), Some("job-1"));
        assert_eq!(t.bytes_total, 4096);
        assert!(t.verified);
        assert_eq!(t.method.as_deref(), Some("clone"));

        let failed = NewTransfer::complete("bob", TransferOperation::Upload)
            .failed("disk full")
            .cancelled();
        assert_eq!(failed.status, TransferStatus::Cancelled);
        assert_eq!(failed.error_message.as_deref(), Some("disk full"));
    }

    #[test]
    fn test_new_transfer_failed_sets_the_reason_and_status() {
        let t = NewTransfer::complete("bob", TransferOperation::Download).failed("I/O error");
        assert_eq!(t.status, TransferStatus::Failed);
        assert_eq!(t.error_message.as_deref(), Some("I/O error"));
    }
}
