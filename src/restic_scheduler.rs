//! Background scheduler for automated restic backups.
//!
//! Polls the `restic_schedules` table for due, enabled schedules and runs
//! them via [`ResticService`], recording a backup job and applying the
//! repo's retention policy (if any) afterwards.

use crate::handlers::AppState;
use crate::restic::{ResticRepo, ResticService};
use crate::storage::restic_repo::ResticRepository;
use crate::storage::{ResticSchedule, ResticScheduleRepository, RetentionPolicyRepository};
use chrono::Utc;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 30;

/// Parses a standard 5-field cron expression (minute hour day-of-month month
/// day-of-week, the crontab format users are used to) and returns the next
/// occurrence strictly after `after`.
///
/// Two normalisations bridge the gap to the underlying `cron` crate:
///  * a leading `"0 "` (seconds = 0) is prepended — the crate wants a
///    6-field, seconds-first expression;
///  * the **day-of-week** field is translated from the crontab convention
///    (`0` or `7` = Sunday … `6` = Saturday) to the crate's own
///    (`1` = Sunday … `7` = Saturday, matching
///    `chrono::Weekday::number_from_sunday()`). Without this, `0` was
///    rejected outright and every other weekday number was off by one — a
///    schedule set for "every Monday" (`* * 1`) actually ran on Sunday.
///    Symbolic names (`SUN`, `MON`, …) already work and pass through.
pub fn compute_next_run(
    cron_expression: &str,
    after: chrono::DateTime<Utc>,
) -> Result<Option<chrono::DateTime<Utc>>, String> {
    let normalized = normalize_crontab_dow(cron_expression.trim());
    let full_expr = format!("0 {}", normalized);
    let schedule = cron::Schedule::from_str(&full_expr).map_err(|e| e.to_string())?;
    Ok(schedule.after(&after).next())
}

/// Rewrites the day-of-week field (5th field) of a 5-field crontab expression
/// from the standard crontab numbering to the `cron` crate's numbering.
/// Leaves the input untouched when it isn't 5 whitespace-separated fields
/// (so the parser produces a clear error) or when the DOW field is `*`/`?`
/// or uses symbolic names.
fn normalize_crontab_dow(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return expr.to_string();
    }

    // crontab 0/7 -> Sunday (crate 1); crontab 1..=6 (Mon..Sat) -> crate 2..=7.
    fn map_num(tok: &str) -> String {
        match tok.parse::<u32>() {
            Ok(0) | Ok(7) => "1".to_string(),
            Ok(v @ 1..=6) => (v + 1).to_string(),
            _ => tok.to_string(),
        }
    }
    fn map_component(part: &str) -> String {
        if let Some((a, b)) = part.split_once('-') {
            format!("{}-{}", map_num(a), map_num(b))
        } else if let Some((a, step)) = part.split_once('/') {
            let base = if a == "*" { a.to_string() } else { map_num(a) };
            format!("{}/{}", base, step)
        } else if part == "*" || part == "?" || !part.chars().all(|c| c.is_ascii_digit()) {
            part.to_string()
        } else {
            map_num(part)
        }
    }

    let dow = fields[4]
        .split(',')
        .map(map_component)
        .collect::<Vec<_>>()
        .join(",");
    format!("{} {} {} {} {}", fields[0], fields[1], fields[2], fields[3], dow)
}

/// Runs forever, checking for due schedules every `POLL_INTERVAL_SECS`.
/// Intended to be `tokio::spawn`ed once at server startup.
pub async fn run_scheduler_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        interval.tick().await;
        if let Err(e) = run_due_schedules(&state).await {
            tracing::warn!("Restic scheduler tick failed: {}", e);
        }
    }
}

async fn run_due_schedules(state: &Arc<AppState>) -> Result<(), String> {
    let schedule_repo = ResticScheduleRepository::new(state.db.pool());
    let now = Utc::now();
    let due = schedule_repo
        .list_due(&now.to_rfc3339())
        .await
        .map_err(|e| e.to_string())?;

    for schedule in due {
        if let Err(e) = run_one_schedule(state, &schedule).await {
            tracing::error!(
                "Scheduled restic backup '{}' (repo {}) failed: {}",
                schedule.name,
                schedule.repo_id,
                e
            );
        }
    }
    Ok(())
}

async fn run_one_schedule(
    state: &Arc<AppState>,
    schedule: &ResticSchedule,
) -> Result<(), String> {
    let repo_repo = ResticRepository::new(state.db.pool());
    let schedule_repo = ResticScheduleRepository::new(state.db.pool());

    let repo_config = repo_repo
        .get_by_id(&schedule.repo_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("repository {} not found", schedule.repo_id))?;

    crate::events::log_event(format!(
        "Scheduled backup '{}' started for repository '{}'",
        schedule.name, repo_config.name
    ));

    let backup_result = if repo_config.snapshot_create_before_backup {
        let pool = repo_config.snapshot_pool.as_deref().unwrap_or("zpool");
        let dataset = repo_config
            .snapshot_dataset
            .as_deref()
            .unwrap_or("zpool/data");
        let snap_name = format!("restic-sched-{}", Utc::now().format("%Y%m%d_%H%M%S"));
        ResticService::create_backup_with_snapshot(
            &repo_config,
            pool,
            dataset,
            &snap_name,
            repo_config.exclude_patterns.as_deref(),
            repo_config.tags.as_deref(),
        )
    } else if repo_config.source_paths.is_empty() {
        Err(crate::restic::ResticError::CommandFailed(
            "No source paths configured for this repository".to_string(),
        ))
    } else {
        ResticService::create_backup(
            &repo_config,
            &repo_config.source_paths,
            repo_config.exclude_patterns.as_deref(),
            repo_config.tags.as_deref(),
        )
    };

    // Always reschedule, even on failure — a single failed run (e.g. the
    // backend disk was briefly unreachable) must not silently wedge the
    // schedule forever with a stale `next_run_at`.
    match backup_result {
        Ok(result) => {
            if let Ok(db_job_id) = repo_repo
                .create_backup_job(
                    &schedule.repo_id,
                    Some(&result.snapshot_id),
                    &repo_config.source_paths.join(","),
                    "scheduled",
                )
                .await
            {
                let _ = repo_repo
                    .update_backup_job(
                        &db_job_id,
                        "completed",
                        result.files_new,
                        result.files_changed,
                        result.files_unmodified,
                        result.data_added_bytes,
                        result.total_files_processed,
                        result.total_bytes,
                        result.duration_secs,
                        None,
                    )
                    .await;
            }
            crate::events::log_event(format!(
                "Scheduled backup '{}' completed for repository '{}' ({} files, {} bytes added)",
                schedule.name, repo_config.name, result.total_files_processed, result.data_added_bytes
            ));
            apply_retention(state, &repo_config).await;
            reschedule(&schedule_repo, schedule).await
        }
        Err(e) => {
            crate::events::log_event_warn(format!(
                "Scheduled backup '{}' failed for repository '{}': {}",
                schedule.name, repo_config.name, e
            ));
            if let Ok(db_job_id) = repo_repo
                .create_backup_job(
                    &schedule.repo_id,
                    None,
                    &repo_config.source_paths.join(","),
                    "scheduled",
                )
                .await
            {
                let _ = repo_repo
                    .update_backup_job(&db_job_id, "failed", 0, 0, 0, 0, 0, 0, 0, Some(&e.to_string()))
                    .await;
            }
            reschedule(&schedule_repo, schedule).await?;
            Err(format!("backup failed: {}", e))
        }
    }
}

/// Applies the repo's retention policy (or the default `apply_to_all` policy
/// if the repo has none assigned) via `restic forget --prune`.
async fn apply_retention(state: &Arc<AppState>, repo_config: &ResticRepo) {
    let policy_repo = RetentionPolicyRepository::new(state.db.pool());

    let policy = match &repo_config.retention_policy_id {
        Some(id) => policy_repo.get_by_id(id).await.ok().flatten(),
        None => policy_repo.get_default().await.ok().flatten(),
    };

    let Some(policy) = policy else {
        return;
    };

    let keep_within = policy.max_age_days.map(|d| format!("{}d", d));
    if policy.max_count.is_none() && keep_within.is_none() {
        return;
    }

    if let Err(e) = ResticService::forget_snapshots(
        repo_config,
        policy.max_count.map(|n| n as u64),
        None,
        None,
        None,
        None,
        None,
        keep_within,
        true,
    ) {
        tracing::warn!(
            "Retention policy application failed for repo '{}': {}",
            repo_config.name,
            e
        );
    }
}

async fn reschedule(
    schedule_repo: &ResticScheduleRepository,
    schedule: &ResticSchedule,
) -> Result<(), String> {
    let now = Utc::now();
    let next = compute_next_run(&schedule.cron_expression, now)
        .ok()
        .flatten();
    schedule_repo
        .record_run(
            &schedule.id,
            &now.to_rfc3339(),
            next.map(|d| d.to_rfc3339()).as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    //! `compute_next_run` is the one pure, deterministic piece of the restic
    //! scheduler — a 5-field crontab expression + a reference instant → the
    //! next occurrence strictly after it. The rest of this module is DB + the
    //! `ResticService` side effects, covered by `tests/restic_integration.rs`
    //! (schedule lifecycle) and `tests/e2e/backup/backup.spec.ts`.
    //!
    //! Run: `cargo test restic_scheduler::tests`

    use super::{compute_next_run, normalize_crontab_dow};
    use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn test_daily_at_3am() {
        let next = compute_next_run("0 3 * * *", at("2026-09-06T10:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next, at("2026-09-07T03:00:00Z"));
    }

    #[test]
    fn test_daily_before_the_hour_same_day() {
        let next = compute_next_run("0 3 * * *", at("2026-09-06T01:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next, at("2026-09-06T03:00:00Z"));
    }

    #[test]
    fn test_every_15_minutes() {
        let next = compute_next_run("*/15 * * * *", at("2026-09-06T10:07:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next, at("2026-09-06T10:15:00Z"));
    }

    #[test]
    fn test_monthly_first_of_month() {
        let next = compute_next_run("0 0 1 * *", at("2026-09-15T12:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next, at("2026-10-01T00:00:00Z"));
    }

    #[test]
    fn test_weekly_monday_uses_crontab_numbering() {
        // Crontab: 1 = Monday. 2026-09-06 is Sunday → next Monday 2026-09-07.
        // (Without the DOW normalisation the `cron` crate reads 1 as Sunday
        // and this would land on 2026-09-13.)
        let next = compute_next_run("0 2 * * 1", at("2026-09-06T12:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next, at("2026-09-07T02:00:00Z"));
        assert_eq!(next.weekday(), Weekday::Mon);
    }

    #[test]
    fn test_weekly_sunday_zero_is_accepted() {
        // Crontab allows 0 for Sunday; the raw `cron` crate rejects it.
        for expr in ["0 2 * * 0", "0 2 * * 7"] {
            let next = compute_next_run(expr, at("2026-09-06T12:00:00Z"))
                .unwrap_or_else(|e| panic!("{expr}: {e}"))
                .unwrap();
            assert_eq!(next.weekday(), Weekday::Sun, "{expr}");
        }
    }

    #[test]
    fn test_weekdays_range_monday_to_friday() {
        // Crontab "1-5" = Mon–Fri. From Sunday, next is Monday.
        let next = compute_next_run("0 6 * * 1-5", at("2026-09-06T12:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(next.weekday(), Weekday::Mon);
        // and the following one is Tuesday, not skipping to next week
        let after = compute_next_run("0 6 * * 1-5", next).unwrap().unwrap();
        assert_eq!(after.weekday(), Weekday::Tue);
    }

    #[test]
    fn test_normalize_crontab_dow() {
        assert_eq!(normalize_crontab_dow("0 3 * * *"), "0 3 * * *");
        assert_eq!(normalize_crontab_dow("0 3 * * 0"), "0 3 * * 1"); // Sun
        assert_eq!(normalize_crontab_dow("0 3 * * 1"), "0 3 * * 2"); // Mon
        assert_eq!(normalize_crontab_dow("0 3 * * 6"), "0 3 * * 7"); // Sat
        assert_eq!(normalize_crontab_dow("0 3 * * 7"), "0 3 * * 1"); // Sun (alias)
        assert_eq!(normalize_crontab_dow("0 3 * * 1-5"), "0 3 * * 2-6");
        assert_eq!(normalize_crontab_dow("0 3 * * 1,3,5"), "0 3 * * 2,4,6");
        assert_eq!(normalize_crontab_dow("0 3 * * MON"), "0 3 * * MON"); // names untouched
        assert_eq!(normalize_crontab_dow("*/15 * * * *"), "*/15 * * * *");
        // not 5 fields → left as-is for the parser to reject
        assert_eq!(normalize_crontab_dow("bad"), "bad");
    }

    #[test]
    fn test_result_is_strictly_after_reference() {
        // Exactly on a boundary must return the *next* one, not itself.
        let base = at("2026-09-06T03:00:00Z");
        let next = compute_next_run("0 3 * * *", base).unwrap().unwrap();
        assert!(next > base);
        assert_eq!(next, at("2026-09-07T03:00:00Z"));
    }

    #[test]
    fn test_whitespace_is_trimmed() {
        assert!(compute_next_run("  0 3 * * *  ", Utc::now()).is_ok());
    }

    #[test]
    fn test_invalid_expressions_are_errors_not_panics() {
        for bad in [
            "",
            "not a cron",
            "0 3 * *",     // 4 fields
            "60 * * * *",  // minute out of range
            "0 25 * * *",  // hour out of range
            "0 3 32 * *",  // day-of-month out of range
        ] {
            assert!(
                compute_next_run(bad, Utc::now()).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn test_reschedule_time_moves_forward_across_a_year_boundary() {
        let dec = Utc.with_ymd_and_hms(2026, 12, 31, 23, 0, 0).unwrap();
        let next = compute_next_run("30 23 * * *", dec).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 12, 31, 23, 30, 0).unwrap());
        let after = compute_next_run("30 23 * * *", next).unwrap().unwrap();
        assert_eq!(after, Utc.with_ymd_and_hms(2027, 1, 1, 23, 30, 0).unwrap());
    }
}
