/* =============================================================================
 * File          : sync_job_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Scheduled cloud-sync jobs (rclone): source/remote pair, cadence
 *   and last-run state.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Scheduled cloud-sync jobs (rclone).

use crate::impl_repository;
use crate::storage::database::DbError;
use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

/// A saved Cloud Sync (rclone) job definition **and** its schedule + run state.
///
/// ROADMAP 16.4: the table used to carry the definition only, so a job ran
/// exactly when someone clicked "Run" and the screen's "last run" column was
/// the outcome of that click. The schedule columns (`schedule_kind`,
/// `interval_minutes`, `cron_expr`, `next_run_at`) and the state columns
/// (`last_run_at`, `last_status`, `last_message`) are written **by the
/// server** — the scheduler picks up `next_run_at`, the run path records the
/// outcome — so what the screen shows is a fact, not a recollection.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SyncJob {
    pub id: i64,
    pub name: String,
    pub source_type: String,
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: String,
    pub destination: String,
    pub dest_path: Option<String>,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// `none` | `interval` | `cron` (see [`ScheduleKind`]).
    pub schedule_kind: String,
    /// Cadence in minutes when `schedule_kind = 'interval'`.
    pub interval_minutes: Option<i64>,
    /// Cron expression when `schedule_kind = 'cron'` (5-field, appliance zone).
    pub cron_expr: Option<String>,
    /// ISO-8601 instant of the last finished attempt.
    pub last_run_at: Option<String>,
    /// `ok` | `failed` — never "running": see the module docs.
    pub last_status: Option<String>,
    /// The outcome's one-line reason (the failure's stderr tail, or a count).
    pub last_message: Option<String>,
    /// ISO-8601 instant the scheduler will consider the job due.
    pub next_run_at: Option<String>,
}

/// Which cadence a job uses.
///
/// `None` is deliberately the default and the migration's DEFAULT: an existing
/// appliance keeps behaving exactly as before (a job only runs when asked)
/// until an admin opts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleKind {
    None,
    Interval,
    Cron,
}

impl ScheduleKind {
    /// Parses the stored value. An unknown value reads as [`ScheduleKind::None`]
    /// rather than an error: a hand-edited or forward-compatible row must not
    /// stop the screen from listing the job.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "interval" => Self::Interval,
            "cron" => Self::Cron,
            _ => Self::None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Interval => "interval",
            Self::Cron => "cron",
        }
    }

    /// Whether the scheduler should ever pick this job up.
    pub fn is_scheduled(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Every column of a job row, in the order [`SyncJob`] declares them.
///
/// Kept as a literal and pasted into the three statements below rather than
/// built at runtime: sqlx 0.9 only accepts a `'static` SQL string (a dynamic
/// one has to be audited for injection), and nothing user-supplied reaches
/// these statements — only the values, which are `bind`-ed.
macro_rules! columns {
    () => {
        "id, name, source_type, source, source_path, dest_type, destination, dest_path, \
         created_at, schedule_kind, interval_minutes, cron_expr, last_run_at, last_status, \
         last_message, next_run_at"
    };
}

const LIST_SQL: &str = concat!("SELECT ", columns!(), " FROM sync_jobs ORDER BY id ASC");
const BY_ID_SQL: &str = concat!("SELECT ", columns!(), " FROM sync_jobs WHERE id = ?");
const DUE_SQL: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM sync_jobs \
      WHERE schedule_kind IN ('interval', 'cron') \
        AND next_run_at IS NOT NULL \
        AND next_run_at <= ? \
      ORDER BY next_run_at ASC"
);

/// Lowest accepted interval. A one-minute sync of a large tree would overlap
/// itself constantly, and the overlap guard would then skip most ticks.
pub const MIN_INTERVAL_MINUTES: i64 = 1;
/// Highest accepted interval (a year, in minutes). Beyond this a cron is the
/// right tool; the ceiling also keeps `interval * 60` far from any overflow.
pub const MAX_INTERVAL_MINUTES: i64 = 366 * 24 * 60;

/// Incoming payload for creating a job (mirrors [`SyncJob`] minus the
/// server-assigned id/timestamp).
#[derive(Debug, Clone)]
pub struct NewSyncJob {
    pub name: String,
    pub source_type: String,
    pub source: String,
    pub source_path: Option<String>,
    pub dest_type: String,
    pub destination: String,
    pub dest_path: Option<String>,
    /// Cadence (ROADMAP 16.4). Defaults to "no schedule" so the create payload
    /// stays backward compatible.
    pub schedule_kind: ScheduleKind,
    pub interval_minutes: Option<i64>,
    pub cron_expr: Option<String>,
}

impl NewSyncJob {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty()
            || self.source.trim().is_empty()
            || self.destination.trim().is_empty()
        {
            return Err("name, source and destination are required".to_string());
        }
        if !matches!(self.source_type.as_str(), "cloud" | "local") {
            return Err("source type must be \"cloud\" or \"local\"".to_string());
        }
        if !matches!(self.dest_type.as_str(), "cloud" | "local") {
            return Err("destination type must be \"cloud\" or \"local\"".to_string());
        }
        self.validate_schedule()
    }

    /// Checks the cadence makes sense for its kind, naming the field that is
    /// wrong.
    ///
    /// Validation runs **before** the row is written and before the scheduler
    /// ever sees it: a job whose cadence cannot be computed would otherwise be
    /// picked up, fail to schedule, and try again on every tick — the failure
    /// mode item 12.3 had to fix on the task screen (ROADMAP 16.4).
    pub fn validate_schedule(&self) -> Result<(), String> {
        match self.schedule_kind {
            ScheduleKind::None => Ok(()),
            ScheduleKind::Interval => match self.interval_minutes {
                None => Err("an interval schedule needs interval_minutes".to_string()),
                Some(m) if m < MIN_INTERVAL_MINUTES || m > MAX_INTERVAL_MINUTES => Err(format!(
                    "interval_minutes must be between {MIN_INTERVAL_MINUTES} and {MAX_INTERVAL_MINUTES}"
                )),
                Some(_) => Ok(()),
            },
            ScheduleKind::Cron => {
                let expr = self
                    .cron_expr
                    .as_deref()
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                    .ok_or_else(|| "a cron schedule needs cron_expr".to_string())?;
                if expr.len() > 120 {
                    return Err("cron_expr is longer than 120 characters".to_string());
                }
                // Parsed here so a typo is a 400 now, not a job that silently
                // never runs.
                crate::restic_scheduler::parse_schedule(expr)
                    .map(|_| ())
                    .map_err(|e| format!("cron_expr is not a valid 5-field cron: {e}"))
            }
        }
    }
}

/// Repository for sync job persistence.
#[derive(Clone)]
pub struct SyncJobRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SyncJobRepository);

impl SyncJobRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// List every saved job, oldest first.
    pub async fn list_all(&self) -> Result<Vec<SyncJob>, DbError> {
        let jobs = sqlx::query_as::<_, SyncJob>(LIST_SQL)
            .fetch_all(self.pool.as_ref())
            .await?;
        Ok(jobs)
    }

    /// Load a single job, if it exists.
    pub async fn get_by_id(&self, id: i64) -> Result<Option<SyncJob>, DbError> {
        let job = sqlx::query_as::<_, SyncJob>(BY_ID_SQL)
            .bind(id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        Ok(job)
    }

    /// Persist a new job and return its id.
    ///
    /// A scheduled job is written with its first `next_run_at` already set, so
    /// the scheduler picks it up on its next tick instead of waiting for a
    /// second write path to agree.
    pub async fn insert(&self, job: &NewSyncJob) -> Result<i64, DbError> {
        let now = Utc::now();
        let next_run = initial_next_run(job, now);
        let result = sqlx::query(
            "INSERT INTO sync_jobs
                (name, source_type, source, source_path, dest_type, destination, dest_path, created_at,
                 schedule_kind, interval_minutes, cron_expr, next_run_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(job.name.trim())
        .bind(&job.source_type)
        .bind(job.source.trim())
        .bind(&job.source_path)
        .bind(&job.dest_type)
        .bind(job.destination.trim())
        .bind(&job.dest_path)
        .bind(now.to_rfc3339())
        .bind(job.schedule_kind.as_str())
        .bind(job.interval_minutes)
        .bind(job.cron_expr.as_deref().map(str::trim))
        .bind(next_run.map(|t| t.to_rfc3339()))
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Rewrites a job's cadence and recomputes `next_run_at` from `now`.
    ///
    /// Switching to `none` clears `next_run_at` so the scheduler stops picking
    /// the job up; the last-run state is deliberately kept, so the screen still
    /// shows how the final run went.
    pub async fn set_schedule(
        &self,
        id: i64,
        kind: ScheduleKind,
        interval_minutes: Option<i64>,
        cron_expr: Option<&str>,
        now: chrono::DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let next = match kind {
            ScheduleKind::None => None,
            ScheduleKind::Interval => interval_minutes
                .map(|m| now + chrono::Duration::minutes(m.max(MIN_INTERVAL_MINUTES))),
            ScheduleKind::Cron => {
                cron_expr
                    .map(str::trim)
                    .filter(|e| !e.is_empty())
                    .and_then(|e| {
                        crate::restic_scheduler::compute_next_run(e, now)
                            .ok()
                            .flatten()
                    })
            }
        };
        let result = sqlx::query(
            "UPDATE sync_jobs
                SET schedule_kind = ?, interval_minutes = ?, cron_expr = ?, next_run_at = ?
              WHERE id = ?",
        )
        .bind(kind.as_str())
        .bind(interval_minutes)
        .bind(cron_expr.map(str::trim))
        .bind(next.map(|t| t.to_rfc3339()))
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Records the outcome of a run and computes the next due time.
    ///
    /// `status` is `ok` or `failed` — never "running": the check-and-set model
    /// is a *finished attempt*, which is what makes a crashed process leave a
    /// truthful row instead of a phantom "running" one.
    pub async fn record_run(
        &self,
        id: i64,
        status: &str,
        message: &str,
        finished_at: chrono::DateTime<Utc>,
        next_run: Option<chrono::DateTime<Utc>>,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "UPDATE sync_jobs
                SET last_run_at = ?, last_status = ?, last_message = ?, next_run_at = ?
              WHERE id = ?",
        )
        .bind(finished_at.to_rfc3339())
        .bind(status)
        // A message is shown on the screen; cap it so a chatty stderr tail
        // cannot bloat the row (the full text is in the event log).
        .bind(truncate_message(message))
        .bind(next_run.map(|t| t.to_rfc3339()))
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Every job the scheduler should run at `now`, oldest due first.
    ///
    /// Only scheduled kinds are returned, and a job with no `next_run_at` (a
    /// schedule that could not be computed) is skipped rather than retried on
    /// every tick — the same rule item 12.3 applies to a task with a broken
    /// cron.
    pub async fn due_now(&self, now: chrono::DateTime<Utc>) -> Result<Vec<SyncJob>, DbError> {
        let jobs = sqlx::query_as::<_, SyncJob>(DUE_SQL)
            .bind(now.to_rfc3339())
            .fetch_all(self.pool.as_ref())
            .await?;
        Ok(jobs)
    }

    /// Recomputes `next_run_at` for one job from `after`. Used after a run and
    /// when a cadence is edited.
    pub async fn set_next_run(
        &self,
        id: i64,
        next_run: Option<chrono::DateTime<Utc>>,
    ) -> Result<bool, DbError> {
        let result = sqlx::query("UPDATE sync_jobs SET next_run_at = ? WHERE id = ?")
            .bind(next_run.map(|t| t.to_rfc3339()))
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete a job by id; returns whether a row was removed.
    pub async fn delete(&self, id: i64) -> Result<bool, DbError> {
        let result = sqlx::query("DELETE FROM sync_jobs WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// The first due time for a brand-new job.
///
/// An interval job is due one interval **after it is created** (a "every 24 h"
/// job created at 09:00 first runs tomorrow at 09:00 — not immediately, which
/// would surprise the admin who just typed the definition); a cron job is due
/// at its first occurrence. `None` (no schedule, or a cron that cannot be
/// computed) means the scheduler never picks it up.
pub fn initial_next_run(
    job: &NewSyncJob,
    now: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    match job.schedule_kind {
        ScheduleKind::None => None,
        ScheduleKind::Interval => job.interval_minutes.map(|m| {
            now + chrono::Duration::minutes(m.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES))
        }),
        ScheduleKind::Cron => job
            .cron_expr
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .and_then(|e| {
                crate::restic_scheduler::compute_next_run(e, now)
                    .ok()
                    .flatten()
            }),
    }
}

/// The next due time after a run that finished at `finished_at`.
///
/// An interval job is scheduled from **the moment the run finished**, not from
/// the previous due time: a sync that took twenty minutes would otherwise fire
/// again immediately (and the overlap guard would then skip it), so "every
/// hour" would behave like "whenever the previous one happened to end". A cron
/// job is scheduled from its schedule, which is what a cron means.
pub fn next_run_after(
    job: &SyncJob,
    finished_at: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    match ScheduleKind::parse(&job.schedule_kind) {
        ScheduleKind::None => None,
        ScheduleKind::Interval => job.interval_minutes.map(|m| {
            finished_at
                + chrono::Duration::minutes(m.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES))
        }),
        ScheduleKind::Cron => job
            .cron_expr
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .and_then(|e| {
                crate::restic_scheduler::compute_next_run(e, finished_at)
                    .ok()
                    .flatten()
            }),
    }
}

/// Longest `last_message` kept in the row (the event log keeps the full text).
pub const MAX_RUN_MESSAGE_CHARS: usize = 400;

fn truncate_message(message: &str) -> String {
    let cleaned = message.trim();
    if cleaned.chars().count() <= MAX_RUN_MESSAGE_CHARS {
        return cleaned.to_string();
    }
    let kept: String = cleaned.chars().take(MAX_RUN_MESSAGE_CHARS).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    //! ROADMAP 16.4 — a persisted sync job can now carry a cadence, and the
    //! server writes the run state.
    //!
    //! Everything here is pure: the due-time arithmetic and the validation. The
    //! SQL side (insert → due_now → record_run) is covered by
    //! `tests/rclone_integration.rs` against a real in-memory database.

    use super::*;

    fn job(kind: ScheduleKind, interval: Option<i64>, cron: Option<&str>) -> NewSyncJob {
        NewSyncJob {
            name: "nightly".into(),
            source_type: "cloud".into(),
            source: "gdrive:photos".into(),
            source_path: None,
            dest_type: "local".into(),
            destination: "/srv/photos".into(),
            dest_path: None,
            schedule_kind: kind,
            interval_minutes: interval,
            cron_expr: cron.map(str::to_string),
        }
    }

    #[test]
    fn a_job_without_a_cadence_is_valid_and_never_due() {
        let j = job(ScheduleKind::None, None, None);
        assert!(j.validate_schedule().is_ok());
        let now = Utc::now();
        assert_eq!(initial_next_run(&j, now), None);
    }

    /// An interval job is first due **one interval after creation**, not
    /// immediately: an admin who types "every 24 h" at 09:00 expects the first
    /// run tomorrow at 09:00, not a sync that starts while they are typing.
    #[test]
    fn an_interval_job_is_first_due_one_interval_after_creation() {
        let j = job(ScheduleKind::Interval, Some(1440), None);
        let now = Utc::now();
        let next = initial_next_run(&j, now).expect("an interval is due later");
        assert_eq!(next, now + chrono::Duration::minutes(1440));
    }

    #[test]
    fn a_cron_job_is_first_due_at_its_next_occurrence() {
        let j = job(ScheduleKind::Cron, None, Some("0 2 * * *"));
        let now = Utc::now();
        let next = initial_next_run(&j, now).expect("a cron has a next occurrence");
        assert!(next > now);
        assert_eq!(
            next.with_timezone(&chrono_tz::UTC).format("%M").to_string(),
            "00"
        );
    }

    /// After a run, an interval job is scheduled from the moment the run
    /// **finished** — otherwise "every hour" would fire again immediately after
    /// a long sync and the overlap guard would skip it, making the cadence a lie.
    #[test]
    fn an_interval_job_reschedules_from_the_end_of_the_run() {
        let finished = Utc::now();
        let mut stored = SyncJob {
            id: 1,
            name: "nightly".into(),
            source_type: "cloud".into(),
            source: "a".into(),
            source_path: None,
            dest_type: "local".into(),
            destination: "b".into(),
            dest_path: None,
            created_at: finished.to_rfc3339(),
            schedule_kind: "interval".into(),
            interval_minutes: Some(60),
            cron_expr: None,
            last_run_at: None,
            last_status: None,
            last_message: None,
            next_run_at: Some(finished.to_rfc3339()),
        };
        assert_eq!(
            next_run_after(&stored, finished),
            Some(finished + chrono::Duration::minutes(60))
        );

        stored.schedule_kind = "none".into();
        assert_eq!(next_run_after(&stored, finished), None);
    }

    /// A cadence that cannot be computed is refused **before** the row is
    /// written: a job that the scheduler could never schedule would otherwise
    /// be picked up on every tick.
    #[test]
    fn an_unusable_cadence_is_refused_with_the_field_name() {
        // Interval without a value.
        let err = job(ScheduleKind::Interval, None, None)
            .validate_schedule()
            .unwrap_err();
        assert!(err.contains("interval_minutes"), "{err}");

        // Interval out of range, in both directions.
        for minutes in [0, -5, MAX_INTERVAL_MINUTES + 1] {
            let err = job(ScheduleKind::Interval, Some(minutes), None)
                .validate_schedule()
                .unwrap_err();
            assert!(err.contains("interval_minutes"), "{minutes}: {err}");
        }

        // Cron without a value, and a malformed one.
        let err = job(ScheduleKind::Cron, None, Some("  "))
            .validate_schedule()
            .unwrap_err();
        assert!(err.contains("cron_expr"), "{err}");
        let err = job(ScheduleKind::Cron, None, Some("not a cron"))
            .validate_schedule()
            .unwrap_err();
        assert!(err.contains("cron_expr"), "{err}");
    }

    #[test]
    fn a_usable_cadence_passes_validation() {
        assert!(
            job(ScheduleKind::Interval, Some(MIN_INTERVAL_MINUTES), None)
                .validate_schedule()
                .is_ok()
        );
        assert!(
            job(ScheduleKind::Interval, Some(MAX_INTERVAL_MINUTES), None)
                .validate_schedule()
                .is_ok()
        );
        assert!(job(ScheduleKind::Cron, None, Some("*/15 * * * *"))
            .validate_schedule()
            .is_ok());
    }

    /// The kind round-trips through the stored string, and an unknown value
    /// reads as "no schedule" rather than breaking the listing.
    #[test]
    fn the_schedule_kind_round_trips_and_unknown_values_are_inert() {
        for kind in [
            ScheduleKind::None,
            ScheduleKind::Interval,
            ScheduleKind::Cron,
        ] {
            assert_eq!(ScheduleKind::parse(kind.as_str()), kind);
        }
        assert_eq!(ScheduleKind::parse("weekly"), ScheduleKind::None);
        assert_eq!(ScheduleKind::parse("  CRON "), ScheduleKind::Cron);
        assert!(!ScheduleKind::None.is_scheduled());
        assert!(ScheduleKind::Cron.is_scheduled());
    }

    /// `last_message` is bounded: a chatty stderr tail must not bloat the row
    /// (the event log keeps the full text).
    #[test]
    fn the_recorded_message_is_truncated() {
        let long = "x".repeat(MAX_RUN_MESSAGE_CHARS + 50);
        let kept = truncate_message(&long);
        assert_eq!(kept.chars().count(), MAX_RUN_MESSAGE_CHARS + 1); // + the ellipsis
        assert!(kept.ends_with('…'));
        assert_eq!(truncate_message("  ok  "), "ok");
    }
}
