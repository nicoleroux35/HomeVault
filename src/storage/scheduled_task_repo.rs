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
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub cron: String,
    pub payload: String,
    pub enabled: bool,
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
            "SELECT id, name, task_type, cron, payload, enabled, last_run_at, last_status, \
             last_error, next_run_at, created_at, updated_at \
             FROM scheduled_tasks ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn list_enabled(&self) -> Result<Vec<ScheduledTask>, sqlx::Error> {
        sqlx::query_as::<_, ScheduledTask>(
            "SELECT id, name, task_type, cron, payload, enabled, last_run_at, last_status, \
             last_error, next_run_at, created_at, updated_at \
             FROM scheduled_tasks WHERE enabled = 1 ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn get(&self, id: &str) -> Result<Option<ScheduledTask>, sqlx::Error> {
        sqlx::query_as::<_, ScheduledTask>(
            "SELECT id, name, task_type, cron, payload, enabled, last_run_at, last_status, \
             last_error, next_run_at, created_at, updated_at \
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
        next_run_at: Option<&str>,
    ) -> Result<ScheduledTask, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO scheduled_tasks \
             (id, name, task_type, cron, payload, enabled, next_run_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(task_type)
        .bind(cron)
        .bind(payload)
        .bind(enabled)
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
        next_run_at: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();
        // Editing type/payload resets the run summary — the old values no
        // longer describe the task that will run next.
        sqlx::query(
            "UPDATE scheduled_tasks SET name = ?, task_type = ?, cron = ?, payload = ?, \
             enabled = ?, next_run_at = ?, last_status = NULL, last_error = NULL, \
             updated_at = ? WHERE id = ?",
        )
        .bind(name)
        .bind(task_type)
        .bind(cron)
        .bind(payload)
        .bind(enabled)
        .bind(next_run_at)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
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
             enabled INTEGER NOT NULL DEFAULT 1, last_run_at TEXT, last_status TEXT, \
             last_error TEXT, next_run_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)",
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
            .create("t", "command", "* * * * *", "{}", true, None)
            .await
            .unwrap();
        repo.mark_started(&task.id).await.unwrap();
        repo.mark_finished(&task.id, "missing-run-row", false, Some("boom"), None)
            .await
            .unwrap();
        let after = repo.get(&task.id).await.unwrap().unwrap();
        assert_eq!(after.last_status.as_deref(), Some("failed"));

        repo.update(&task.id, "t2", "command", "* * * * *", "{}", false, None)
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
            .create("t", "command", "* * * * *", "{}", true, None)
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
            .create("t", "command", "* * * * *", "{}", true, None)
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
}
