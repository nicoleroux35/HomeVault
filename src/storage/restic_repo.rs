/* =============================================================================
 * File          : restic_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Backup repository definitions: target URL, stored password, source
 *   paths, optional pre-backup snapshot settings.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Backup repository definitions (restic).

use crate::restic::ResticRepo;
use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

pub struct ResticRepository {
    pool: Arc<SqlitePool>,
}

impl ResticRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list_all(&self) -> Result<Vec<ResticRepo>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ResticRepoRow>(
            "SELECT id, name, repository, password, retention_policy_id, source_paths, exclude_patterns, tags, snapshot_source, snapshot_pool, snapshot_dataset, snapshot_create_before_backup, created_at, updated_at FROM restic_repos ORDER BY name",
        )
        .fetch_all(self.pool.as_ref())
        .await?;

        Ok(rows.into_iter().map(|r| r.into_model()).collect())
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<ResticRepo>, sqlx::Error> {
        let row = sqlx::query_as::<_, ResticRepoRow>(
            "SELECT id, name, repository, password, retention_policy_id, source_paths, exclude_patterns, tags, snapshot_source, snapshot_pool, snapshot_dataset, snapshot_create_before_backup, created_at, updated_at FROM restic_repos WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(row.map(|r| r.into_model()))
    }

    pub async fn get_by_name(&self, name: &str) -> Result<Option<ResticRepo>, sqlx::Error> {
        let row = sqlx::query_as::<_, ResticRepoRow>(
            "SELECT id, name, repository, password, retention_policy_id, source_paths, exclude_patterns, tags, snapshot_source, snapshot_pool, snapshot_dataset, snapshot_create_before_backup, created_at, updated_at FROM restic_repos WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(self.pool.as_ref())
        .await?;

        Ok(row.map(|r| r.into_model()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        name: &str,
        repository: &str,
        password: &str,
        retention_policy_id: Option<&str>,
        source_paths: &[String],
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
        snapshot_source: &str,
        snapshot_pool: Option<&str>,
        snapshot_dataset: Option<&str>,
        snapshot_create_before_backup: bool,
    ) -> Result<ResticRepo, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let source_paths_json =
            serde_json::to_string(source_paths).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            "INSERT INTO restic_repos (id, name, repository, password, retention_policy_id, source_paths, exclude_patterns, tags, snapshot_source, snapshot_pool, snapshot_dataset, snapshot_create_before_backup, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(repository)
        .bind(password)
        .bind(retention_policy_id)
        .bind(&source_paths_json)
        .bind(exclude_patterns)
        .bind(tags)
        .bind(snapshot_source)
        .bind(snapshot_pool)
        .bind(snapshot_dataset)
        .bind(snapshot_create_before_backup)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(ResticRepo {
            id,
            name: name.to_string(),
            repository: repository.to_string(),
            password: password.to_string(),
            retention_policy_id: retention_policy_id.map(|s| s.to_string()),
            source_paths: source_paths.to_vec(),
            exclude_patterns: exclude_patterns.map(|s| s.to_string()),
            tags: tags.map(|s| s.to_string()),
            snapshot_source: snapshot_source.to_string(),
            snapshot_pool: snapshot_pool.map(|s| s.to_string()),
            snapshot_dataset: snapshot_dataset.map(|s| s.to_string()),
            snapshot_create_before_backup,
            created_at: chrono::DateTime::parse_from_rfc3339(&now)
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| Utc::now()),
            updated_at: Utc::now(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        repository: Option<&str>,
        password: Option<&str>,
        retention_policy_id: Option<&str>,
        source_paths: Option<&[String]>,
        exclude_patterns: Option<&str>,
        tags: Option<&str>,
        snapshot_source: Option<&str>,
        snapshot_pool: Option<&str>,
        snapshot_dataset: Option<&str>,
        snapshot_create_before_backup: Option<bool>,
    ) -> Result<Option<ResticRepo>, sqlx::Error> {
        let existing = self.get_by_id(id).await?;
        let repo = match existing {
            Some(r) => r,
            None => return Ok(None),
        };

        let final_name = name.unwrap_or(&repo.name);
        let final_repository = repository.unwrap_or(&repo.repository);
        let final_password = password.unwrap_or(&repo.password);
        let final_retention_policy_id = retention_policy_id.or(repo.retention_policy_id.as_deref());
        let final_source_paths = source_paths
            .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "[]".to_string()))
            .unwrap_or_else(|| {
                serde_json::to_string(&repo.source_paths).unwrap_or_else(|_| "[]".to_string())
            });
        let final_exclude = exclude_patterns.or(repo.exclude_patterns.as_deref());
        let final_tags = tags.or(repo.tags.as_deref());
        let final_snapshot_source = snapshot_source.unwrap_or(&repo.snapshot_source);
        let final_snapshot_pool = snapshot_pool.or(repo.snapshot_pool.as_deref());
        let final_snapshot_dataset = snapshot_dataset.or(repo.snapshot_dataset.as_deref());
        let final_snap_before =
            snapshot_create_before_backup.unwrap_or(repo.snapshot_create_before_backup);
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE restic_repos SET name = ?, repository = ?, password = ?, retention_policy_id = ?, source_paths = ?, exclude_patterns = ?, tags = ?, snapshot_source = ?, snapshot_pool = ?, snapshot_dataset = ?, snapshot_create_before_backup = ?, updated_at = ? WHERE id = ?",
        )
        .bind(final_name)
        .bind(final_repository)
        .bind(final_password)
        .bind(final_retention_policy_id)
        .bind(&final_source_paths)
        .bind(final_exclude)
        .bind(final_tags)
        .bind(final_snapshot_source)
        .bind(final_snapshot_pool)
        .bind(final_snapshot_dataset)
        .bind(final_snap_before)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;

        self.get_by_id(id).await
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM restic_repos WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_backup_jobs(
        &self,
        repo_id: &str,
        limit: u32,
    ) -> Result<Vec<ResticBackupJobRow>, sqlx::Error> {
        sqlx::query_as::<_, ResticBackupJobRow>(
            "SELECT id, repo_id, snapshot_id, source, backup_type, status, files_new, files_changed, files_unmodified, data_added_bytes, total_files_processed, total_bytes, duration_secs, error_message, started_at, finished_at, created_at FROM restic_backup_jobs WHERE repo_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(repo_id)
        .bind(limit)
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn create_backup_job(
        &self,
        repo_id: &str,
        snapshot_id: Option<&str>,
        source: &str,
        backup_type: &str,
    ) -> Result<String, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO restic_backup_jobs (id, repo_id, snapshot_id, source, backup_type, status, started_at, created_at) VALUES (?, ?, ?, ?, ?, 'running', ?, ?)",
        )
        .bind(&id)
        .bind(repo_id)
        .bind(snapshot_id)
        .bind(source)
        .bind(backup_type)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_backup_job(
        &self,
        id: &str,
        status: &str,
        files_new: u64,
        files_changed: u64,
        files_unmodified: u64,
        data_added_bytes: u64,
        total_files_processed: u64,
        total_bytes: u64,
        duration_secs: u64,
        error_message: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "UPDATE restic_backup_jobs SET status = ?, files_new = ?, files_changed = ?, files_unmodified = ?, data_added_bytes = ?, total_files_processed = ?, total_bytes = ?, duration_secs = ?, error_message = ?, finished_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(files_new as i64)
        .bind(files_changed as i64)
        .bind(files_unmodified as i64)
        .bind(data_added_bytes as i64)
        .bind(total_files_processed as i64)
        .bind(total_bytes as i64)
        .bind(duration_secs as i64)
        .bind(error_message)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    /// RFC 3339 `finished_at` of the newest `completed` job for a repo, if any.
    /// Powers the backup-reminder list (`GET /api/restic/reminders`).
    pub async fn last_successful_backup_at(
        &self,
        repo_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT MAX(finished_at) FROM restic_backup_jobs WHERE repo_id = ? AND status = 'completed'",
        )
        .bind(repo_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.and_then(|r| r.0))
    }
}

#[derive(sqlx::FromRow)]
struct ResticRepoRow {
    id: String,
    name: String,
    repository: String,
    password: String,
    retention_policy_id: Option<String>,
    source_paths: String,
    exclude_patterns: Option<String>,
    tags: Option<String>,
    snapshot_source: String,
    snapshot_pool: Option<String>,
    snapshot_dataset: Option<String>,
    snapshot_create_before_backup: bool,
    created_at: String,
    updated_at: String,
}

impl ResticRepoRow {
    fn into_model(self) -> ResticRepo {
        let source_paths: Vec<String> =
            serde_json::from_str(&self.source_paths).unwrap_or_default();

        let parse_dt = |s: &str| -> chrono::DateTime<Utc> {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now())
        };

        ResticRepo {
            id: self.id,
            name: self.name,
            repository: self.repository,
            password: self.password,
            retention_policy_id: self.retention_policy_id,
            source_paths,
            exclude_patterns: self.exclude_patterns,
            tags: self.tags,
            snapshot_source: self.snapshot_source,
            snapshot_pool: self.snapshot_pool,
            snapshot_dataset: self.snapshot_dataset,
            snapshot_create_before_backup: self.snapshot_create_before_backup,
            created_at: parse_dt(&self.created_at),
            updated_at: parse_dt(&self.updated_at),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ResticBackupJobRow {
    pub id: String,
    pub repo_id: String,
    pub snapshot_id: Option<String>,
    pub source: String,
    pub backup_type: String,
    pub status: String,
    pub files_new: Option<i64>,
    pub files_changed: Option<i64>,
    pub files_unmodified: Option<i64>,
    pub data_added_bytes: Option<i64>,
    pub total_files_processed: Option<i64>,
    pub total_bytes: Option<i64>,
    pub duration_secs: Option<i64>,
    pub error_message: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
}
