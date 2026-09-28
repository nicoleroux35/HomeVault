use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResticSchedule {
    pub id: String,
    pub repo_id: String,
    pub name: String,
    pub cron_expression: String,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(sqlx::FromRow)]
struct ResticScheduleRow {
    id: String,
    repo_id: String,
    name: String,
    cron_expression: String,
    enabled: bool,
    last_run_at: Option<String>,
    next_run_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<ResticScheduleRow> for ResticSchedule {
    fn from(r: ResticScheduleRow) -> Self {
        Self {
            id: r.id,
            repo_id: r.repo_id,
            name: r.name,
            cron_expression: r.cron_expression,
            enabled: r.enabled,
            last_run_at: r.last_run_at,
            next_run_at: r.next_run_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub struct ResticScheduleRepository {
    pool: Arc<SqlitePool>,
}

impl ResticScheduleRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list_all(&self) -> Result<Vec<ResticSchedule>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ResticScheduleRow>(
            "SELECT id, repo_id, name, cron_expression, enabled, last_run_at, next_run_at, created_at, updated_at FROM restic_schedules ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn list_by_repo(&self, repo_id: &str) -> Result<Vec<ResticSchedule>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ResticScheduleRow>(
            "SELECT id, repo_id, name, cron_expression, enabled, last_run_at, next_run_at, created_at, updated_at FROM restic_schedules WHERE repo_id = ? ORDER BY name",
        )
        .bind(repo_id)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<ResticSchedule>, sqlx::Error> {
        let row = sqlx::query_as::<_, ResticScheduleRow>(
            "SELECT id, repo_id, name, cron_expression, enabled, last_run_at, next_run_at, created_at, updated_at FROM restic_schedules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(Into::into))
    }

    /// Every enabled schedule whose `next_run_at` is due (<= now, or unset).
    pub async fn list_due(&self, now: &str) -> Result<Vec<ResticSchedule>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ResticScheduleRow>(
            "SELECT id, repo_id, name, cron_expression, enabled, last_run_at, next_run_at, created_at, updated_at FROM restic_schedules WHERE enabled = 1 AND (next_run_at IS NULL OR next_run_at <= ?)",
        )
        .bind(now)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn create(
        &self,
        repo_id: &str,
        name: &str,
        cron_expression: &str,
        enabled: bool,
        next_run_at: Option<&str>,
    ) -> Result<ResticSchedule, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO restic_schedules (id, repo_id, name, cron_expression, enabled, next_run_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(repo_id)
        .bind(name)
        .bind(cron_expression)
        .bind(enabled)
        .bind(next_run_at)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(ResticSchedule {
            id,
            repo_id: repo_id.to_string(),
            name: name.to_string(),
            cron_expression: cron_expression.to_string(),
            enabled,
            last_run_at: None,
            next_run_at: next_run_at.map(|s| s.to_string()),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        cron_expression: Option<&str>,
        enabled: Option<bool>,
        next_run_at: Option<Option<&str>>,
    ) -> Result<Option<ResticSchedule>, sqlx::Error> {
        let existing = self.get_by_id(id).await?;
        let Some(schedule) = existing else {
            return Ok(None);
        };

        let final_name = name.unwrap_or(&schedule.name);
        let final_cron = cron_expression.unwrap_or(&schedule.cron_expression);
        let final_enabled = enabled.unwrap_or(schedule.enabled);
        let final_next_run = match next_run_at {
            Some(v) => v.map(|s| s.to_string()),
            None => schedule.next_run_at.clone(),
        };
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE restic_schedules SET name = ?, cron_expression = ?, enabled = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(final_name)
        .bind(final_cron)
        .bind(final_enabled)
        .bind(&final_next_run)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;

        self.get_by_id(id).await
    }

    /// Records that a scheduled run just happened and stores the newly
    /// computed next occurrence.
    pub async fn record_run(
        &self,
        id: &str,
        last_run_at: &str,
        next_run_at: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE restic_schedules SET last_run_at = ?, next_run_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(last_run_at)
        .bind(next_run_at)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM restic_schedules WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
