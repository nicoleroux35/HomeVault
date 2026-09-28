use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetentionPolicy {
    pub id: String,
    pub name: String,
    pub max_count: Option<i64>,
    pub max_age_days: Option<i64>,
    pub max_size_gb: Option<i64>,
    pub apply_to_all: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(sqlx::FromRow)]
struct RetentionPolicyRow {
    id: String,
    name: String,
    max_count: Option<i64>,
    max_age_days: Option<i64>,
    max_size_gb: Option<i64>,
    apply_to_all: bool,
    created_at: String,
    updated_at: String,
}

impl From<RetentionPolicyRow> for RetentionPolicy {
    fn from(r: RetentionPolicyRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            max_count: r.max_count,
            max_age_days: r.max_age_days,
            max_size_gb: r.max_size_gb,
            apply_to_all: r.apply_to_all,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub struct RetentionPolicyRepository {
    pool: Arc<SqlitePool>,
}

impl RetentionPolicyRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list_all(&self) -> Result<Vec<RetentionPolicy>, sqlx::Error> {
        let rows = sqlx::query_as::<_, RetentionPolicyRow>(
            "SELECT id, name, max_count, max_age_days, max_size_gb, apply_to_all, created_at, updated_at FROM backup_retention_policies ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<RetentionPolicy>, sqlx::Error> {
        let row = sqlx::query_as::<_, RetentionPolicyRow>(
            "SELECT id, name, max_count, max_age_days, max_size_gb, apply_to_all, created_at, updated_at FROM backup_retention_policies WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(Into::into))
    }

    /// Returns the first policy marked `apply_to_all`, used as the default
    /// retention policy for repos that don't have one explicitly assigned.
    pub async fn get_default(&self) -> Result<Option<RetentionPolicy>, sqlx::Error> {
        let row = sqlx::query_as::<_, RetentionPolicyRow>(
            "SELECT id, name, max_count, max_age_days, max_size_gb, apply_to_all, created_at, updated_at FROM backup_retention_policies WHERE apply_to_all = 1 ORDER BY name LIMIT 1",
        )
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(Into::into))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        name: &str,
        max_count: Option<i64>,
        max_age_days: Option<i64>,
        max_size_gb: Option<i64>,
        apply_to_all: bool,
    ) -> Result<RetentionPolicy, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO backup_retention_policies (id, name, max_count, max_age_days, max_size_gb, apply_to_all, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(max_count)
        .bind(max_age_days)
        .bind(max_size_gb)
        .bind(apply_to_all)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(RetentionPolicy {
            id,
            name: name.to_string(),
            max_count,
            max_age_days,
            max_size_gb,
            apply_to_all,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        max_count: Option<i64>,
        max_age_days: Option<i64>,
        max_size_gb: Option<i64>,
        apply_to_all: Option<bool>,
    ) -> Result<Option<RetentionPolicy>, sqlx::Error> {
        let existing = self.get_by_id(id).await?;
        let Some(policy) = existing else {
            return Ok(None);
        };

        let final_name = name.unwrap_or(&policy.name);
        let final_max_count = max_count.or(policy.max_count);
        let final_max_age = max_age_days.or(policy.max_age_days);
        let final_max_size = max_size_gb.or(policy.max_size_gb);
        let final_apply_all = apply_to_all.unwrap_or(policy.apply_to_all);
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE backup_retention_policies SET name = ?, max_count = ?, max_age_days = ?, max_size_gb = ?, apply_to_all = ?, updated_at = ? WHERE id = ?",
        )
        .bind(final_name)
        .bind(final_max_count)
        .bind(final_max_age)
        .bind(final_max_size)
        .bind(final_apply_all)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM backup_retention_policies WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
