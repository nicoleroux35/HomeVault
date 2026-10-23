/* =============================================================================
 * File          : scheduled_task_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage layer for the generic scheduled-task engine: CRUD for task rows
 *   plus the per-run history the Tasks app shows. Pure data access — the
 *   due/next-run logic lives in src/scheduled_tasks_scheduler.rs.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 *
 * Usage:
 *   Imported by handlers/scheduled_tasks.rs (HTTP) and
 *   scheduled_tasks_scheduler.rs (background loop).
 * =============================================================================
 */

//! Scheduled-task rows + run history (the storage half of the generic
//! scheduler; see `crate::scheduled_tasks_scheduler` for execution).

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// A scheduled task as configured by an admin. `payload` is the JSON object
/// consumed by the executor — its shape depends on `task_type`.
///
/// `timezone` is an IANA name (`Europe/Paris`) the cron is evaluated in;
/// `None` means "follow the appliance timezone" (ROADMAP 12.2).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub cron: String,
    pub payload: String,
    pub enabled: bool,
    pub timezone: Option<String>,
    pub last_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One execution attempt of a task (history drawer in the Tasks app).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScheduledTaskRun {
    pub id: String,
    pub task_id: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub output: Option<String>,
}

pub struct ScheduledTaskRepository {
    pool: Arc<SqlitePool>,
}

impl ScheduledTaskRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<ScheduledTask>, sqlx::Error> {
        sqlx::query_as::<_, ScheduledTask>(
            "SELECT id, name, task_type, cron, payload, enabled, timezone, last_run_at, \
             last_status, last_error, next_run_at, created_at, updated_at \
             FROM scheduled_tasks ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn list_enabled(&self) -> Result<Vec<ScheduledTask>, sqlx::Error> {
        sqlx::query_as::<_, ScheduledTask>(
            "SELECT id, name, task_type, cron, payload, enabled, timezone, last_run_at, \
             last_status, last_error, next_run_at, created_at, updated_at \
             FROM scheduled_tasks WHERE enabled = 1 ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn get(&self, id: &str) -> Result<Option<ScheduledTask>, sqlx::Error> {
        sqlx::query_as::<_, ScheduledTask>(
            "SELECT id, name, task_type, cron, payload, enabled, timezone, last_run_at, \
             last_status, last_error, next_run_at, created_at, updated_at \
             FROM scheduled_tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await
    }

    /// Inserts a task. Validation of `task_type`/`payload` is the *caller's*
    /// job (`TaskAction::from_parts`) — the DB CHECK only catches raw SQL.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        name: &str,
        task_type: &str,
        cron: &str,
        payload: &str,
        enabled: bool,
        timezone: Option<&str>,
        next_run_at: Option<&str>,
    ) -> Result<ScheduledTask, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO scheduled_tasks \
             (id, name, task_type, cron, payload, enabled, timezone, next_run_at, created_at, \
             updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(task_type)
        .bind(cron)
        .bind(payload)
        .bind(enabled)
        .bind(timezone)
        .bind(next_run_at)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(self.get(&id).await?.expect("row just inserted"))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: &str,
        name: &str,
        task_type: &str,
        cron: &str,
        payload: &str,
        enabled: bool,
        timezone: Option<&str>,
        next_run_at: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        // Editing type/payload resets the run summary — the old values no
        // longer describe the task that will run next.
        sqlx::query(
            "UPDATE scheduled_tasks SET name = ?, task_type = ?, cron = ?, payload = ?, \
             enabled = ?, timezone = ?, next_run_at = ?, last_status = NULL, last_error = NULL, \
             updated_at = ? WHERE id = ?",
        )
        .bind(name)
        .bind(task_type)
        .bind(cron)
        .bind(payload)
        .bind(enabled)
        .bind(timezone)
        .bind(next_run_at)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Flips `enabled` and nothing else — the enable/disable switch must not
    /// reset the last-run summary the way a full `update` does (a paused task
    /// keeps showing what its last run produced).
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE scheduled_tasks SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(&now)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Pauses a task that cannot run and records why (ROADMAP 12.3). Used for
    /// an unparseable stored cron: retrying it every tick would fill the run
    /// history with identical failures for ever.
    pub async fn pause_with_error(&self, id: &str, error: &str) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE scheduled_tasks SET enabled = 0, next_run_at = NULL, last_status = 'failed', \
             last_error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(error)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Closes every run row left `running` by a previous process (crash,
    /// `systemctl stop` during a run). The overlap guard is in-memory, so
    /// nothing can still own such a row; without this the screen would show
    /// "Running" for ever (ROADMAP 12.4).
    pub async fn close_interrupted_runs(&self) -> Result<u64, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let message = "Interrupted: the server stopped while this run was in progress";
        sqlx::query(
            "UPDATE scheduled_task_runs SET status = 'failed', finished_at = ?, output = ? \
             WHERE status = 'running'",
        )
        .bind(&now)
        .bind(message)
        .execute(self.pool.as_ref())
        .await?;
        let summary = sqlx::query(
            "UPDATE scheduled_tasks SET last_status = 'failed', last_error = ? \
             WHERE last_status = 'running'",
        )
        .bind(message)
        .execute(self.pool.as_ref())
        .await?;
        Ok(summary.rows_affected())
    }

    pub async fn delete(&self, id: &str) -> Result<(), sqlx::Error> {
        // Explicit run purge — the schema has ON DELETE CASCADE (and the pool
        // enables PRAGMA foreign_keys), but deleting explicitly keeps this
        // correct even against a database where either half is missing.
        sqlx::query("DELETE FROM scheduled_task_runs WHERE task_id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        sqlx::query("DELETE FROM scheduled_tasks WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Marks the task as started (status `running`, timestamp now) and opens
    /// a history row. Returns the history row id.
    pub async fn mark_started(&self, id: &str) -> Result<String, sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE scheduled_tasks SET last_run_at = ?, last_status = 'running', last_error = NULL WHERE id = ?")
            .bind(&now)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;

        let run_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO scheduled_task_runs (id, task_id, status, started_at) \
             VALUES (?, ?, 'running', ?)",
        )
        .bind(&run_id)
        .bind(id)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        Ok(run_id)
    }

    /// Closes a run: updates the task summary row and the history row.
    pub async fn mark_finished(
        &self,
        id: &str,
        run_id: &str,
        success: bool,
        error: Option<&str>,
        output: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        let status = if success { "success" } else { "failed" };
        sqlx::query(
            "UPDATE scheduled_tasks SET last_status = ?, last_error = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(status)
        .bind(error)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;

        // Truncate the stored output so a chatty command can't grow the DB
        // unbounded — the drawer only shows a tail anyway.
        let output = output.map(|o| o.chars().take(8000).collect::<String>());
        sqlx::query(
            "UPDATE scheduled_task_runs SET status = ?, finished_at = ?, output = ? \
             WHERE id = ?",
        )
        .bind(status)
        .bind(&now)
        .bind(output)
        .bind(run_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn set_next_run(
        &self,
        id: &str,
        next_run_at: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scheduled_tasks SET next_run_at = ? WHERE id = ?")
            .bind(next_run_at)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn list_runs(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<ScheduledTaskRun>, sqlx::Error> {
        sqlx::query_as::<_, ScheduledTaskRun>(
            "SELECT id, task_id, status, started_at, finished_at, output \
             FROM scheduled_task_runs WHERE task_id = ? \
             ORDER BY started_at DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn repo() -> ScheduledTaskRepository {
        let pool = Arc::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap(),
        );
        sqlx::query(
            "CREATE TABLE scheduled_tasks (id TEXT PRIMARY KEY, name TEXT NOT NULL, \
             task_type TEXT NOT NULL, cron TEXT NOT NULL, payload TEXT NOT NULL DEFAULT '{}', \
             enabled INTEGER NOT NULL DEFAULT 1, timezone TEXT, last_run_at TEXT, \
             last_status TEXT, last_error TEXT, next_run_at TEXT, created_at TEXT NOT NULL, \
             updated_at TEXT NOT NULL)",
        )
        .execute(pool.as_ref())
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE scheduled_task_runs (id TEXT PRIMARY KEY, task_id TEXT NOT NULL, \
             status TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT, output TEXT)",
        )
        .execute(pool.as_ref())
        .await
        .unwrap();
        ScheduledTaskRepository::new(pool)
    }

    #[tokio::test]
    async fn create_then_get_round_trips_payload() {
        let repo = repo().await;
        let task = repo
            .create(
                "Nightly snapshot",
                "snapshot",
                "0 2 * * *",
                r#"{"pool":"tank"}"#,
                true,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(task.name, "Nightly snapshot");
        assert_eq!(task.payload, r#"{"pool":"tank"}"#);
        assert!(task.enabled);
        assert_eq!(repo.list().await.unwrap().len(), 1);
        assert_eq!(repo.list_enabled().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn update_resets_run_summary() {
        let repo = repo().await;
        let task = repo
            .create("t", "command", "* * * * *", "{}", true, None, None)
            .await
            .unwrap();
        repo.mark_started(&task.id).await.unwrap();
        repo.mark_finished(&task.id, "missing-run-row", false, Some("boom"), None)
            .await
            .unwrap();
        let after = repo.get(&task.id).await.unwrap().unwrap();
        assert_eq!(after.last_status.as_deref(), Some("failed"));

        repo.update(
            &task.id,
            "t2",
            "command",
            "* * * * *",
            "{}",
            false,
            None,
            None,
        )
        .await
        .unwrap();
        let edited = repo.get(&task.id).await.unwrap().unwrap();
        assert_eq!(edited.name, "t2");
        assert!(!edited.enabled);
        assert!(edited.last_status.is_none(), "edit must reset last_status");
    }

    #[tokio::test]
    async fn run_history_is_limited_and_ordered() {
        let repo = repo().await;
        let task = repo
            .create("t", "command", "* * * * *", "{}", true, None, None)
            .await
            .unwrap();
        for _ in 0..3 {
            let run = repo.mark_started(&task.id).await.unwrap();
            repo.mark_finished(&task.id, &run, true, None, Some("ok out"))
                .await
                .unwrap();
        }
        let runs = repo.list_runs(&task.id, 2).await.unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].status, "success");
        assert_eq!(runs[0].output.as_deref(), Some("ok out"));
    }

    #[tokio::test]
    async fn delete_removes_history_rows() {
        let repo = repo().await;
        let task = repo
            .create("t", "command", "* * * * *", "{}", true, None, None)
            .await
            .unwrap();
        let run = repo.mark_started(&task.id).await.unwrap();
        repo.mark_finished(&task.id, &run, true, None, None)
            .await
            .unwrap();
        repo.delete(&task.id).await.unwrap();
        assert!(repo.list().await.unwrap().is_empty());
        assert!(repo.list_runs(&task.id, 10).await.unwrap().is_empty());
    }

    /// 12.2: the per-task timezone round-trips, and `NULL` stays `None` (which
    /// the scheduler reads as "follow the appliance timezone").
    #[tokio::test]
    async fn timezone_round_trips_and_defaults_to_none() {
        let repo = repo().await;
        let paris = repo
            .create(
                "paris",
                "command",
                "0 2 * * *",
                "{}",
                true,
                Some("Europe/Paris"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(paris.timezone.as_deref(), Some("Europe/Paris"));

        let server = repo
            .create("server", "command", "0 2 * * *", "{}", true, None, None)
            .await
            .unwrap();
        assert!(server.timezone.is_none());

        // A full edit replaces it (including back to "follow the server").
        repo.update(
            &paris.id,
            "paris",
            "command",
            "0 2 * * *",
            "{}",
            true,
            None,
            None,
        )
        .await
        .unwrap();
        let edited = repo.get(&paris.id).await.unwrap().unwrap();
        assert!(edited.timezone.is_none());
    }

    /// The enable/disable switch must not reset the run summary — the old
    /// handler re-submitted the whole task through `update`, which cleared it.
    #[tokio::test]
    async fn set_enabled_keeps_the_run_summary() {
        let repo = repo().await;
        let task = repo
            .create("t", "command", "* * * * *", "{}", true, None, None)
            .await
            .unwrap();
        let run = repo.mark_started(&task.id).await.unwrap();
        repo.mark_finished(&task.id, &run, true, None, Some("kept output"))
            .await
            .unwrap();

        repo.set_enabled(&task.id, false).await.unwrap();
        let paused = repo.get(&task.id).await.unwrap().unwrap();
        assert!(!paused.enabled);
        assert_eq!(paused.last_status.as_deref(), Some("success"));
        assert!(paused.last_run_at.is_some(), "the last run stays visible");

        repo.set_enabled(&task.id, true).await.unwrap();
        assert!(repo.get(&task.id).await.unwrap().unwrap().enabled);
    }

    /// 12.3: an unrunnable task is paused with the reason kept on the row and
    /// no next run, so the tick cannot pick it up again.
    #[tokio::test]
    async fn pause_with_error_disables_and_clears_the_next_run() {
        let repo = repo().await;
        let task = repo
            .create(
                "t",
                "command",
                "not a cron",
                "{}",
                true,
                None,
                Some("2026-01-01T00:00:00+00:00"),
            )
            .await
            .unwrap();

        repo.pause_with_error(&task.id, "Invalid cron expression 'not a cron'")
            .await
            .unwrap();
        let paused = repo.get(&task.id).await.unwrap().unwrap();
        assert!(!paused.enabled, "a task that cannot be scheduled is paused");
        assert!(paused.next_run_at.is_none());
        assert_eq!(paused.last_status.as_deref(), Some("failed"));
        assert_eq!(
            paused.last_error.as_deref(),
            Some("Invalid cron expression 'not a cron'")
        );
        assert!(
            repo.list_enabled().await.unwrap().is_empty(),
            "the tick must not see it any more"
        );
    }

    /// 12.4: a run left `running` by a previous process is closed at startup,
    /// on both the history row and the task summary.
    #[tokio::test]
    async fn close_interrupted_runs_closes_only_running_rows() {
        let repo = repo().await;
        let task = repo
            .create("t", "command", "* * * * *", "{}", true, None, None)
            .await
            .unwrap();
        // One run that finished normally, one still `running`.
        let done = repo.mark_started(&task.id).await.unwrap();
        repo.mark_finished(&task.id, &done, true, None, Some("ok"))
            .await
            .unwrap();
        let orphan = repo.mark_started(&task.id).await.unwrap();

        let closed = repo.close_interrupted_runs().await.unwrap();
        assert_eq!(closed, 1, "only the interrupted task is touched");

        let runs = repo.list_runs(&task.id, 10).await.unwrap();
        let orphan_row = runs.iter().find(|r| r.id == orphan).expect("orphan row");
        assert_eq!(orphan_row.status, "failed");
        assert!(orphan_row.finished_at.is_some());
        assert!(orphan_row
            .output
            .as_deref()
            .unwrap_or_default()
            .contains("Interrupted"));
        let done_row = runs.iter().find(|r| r.id == done).expect("done row");
        assert_eq!(done_row.status, "success", "a finished run is untouched");
    }
}
