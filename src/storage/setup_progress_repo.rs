/* =============================================================================
 * File          : setup_progress_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Durable first-run wizard progress (reboot-safe onboarding): the
 *   current step and completed flags.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Durable first-run wizard progress (reboot-safe onboarding).
//!
//! The Setup Wizard persists its current step plus the operator's
//! storage/RAID/restore choices into the singleton `setup_progress` row, so a
//! reboot or a closed browser resumes where it left off. Only non-secret
//! choices are stored — the encrypted-restore passphrase is never persisted.

use crate::impl_repository;
use crate::storage::database::DbError;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;

pub const MAX_SETUP_STEP: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupProgress {
    pub step: i64,
    pub data_json: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct SetupProgressRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(SetupProgressRepository);

impl SetupProgressRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn get(&self) -> Result<SetupProgress, DbError> {
        let row: Option<(i64, String, String)> =
            sqlx::query_as("SELECT step, data_json, updated_at FROM setup_progress WHERE id = 1")
                .fetch_optional(self.pool.as_ref())
                .await?;
        match row {
            Some((step, data_json, updated_at)) => Ok(SetupProgress {
                step,
                data_json,
                updated_at,
            }),
            None => Ok(SetupProgress {
                step: 0,
                data_json: "{}".to_string(),
                updated_at: String::new(),
            }),
        }
    }

    /// Persisted progress must be small, well-formed JSON. Rejects payloads
    /// over 32 KiB and anything that is not a JSON object (arrays/strings
    /// would break the wizard's merge logic client-side).
    pub fn validate_step_data(step: i64, data_json: &str) -> Result<(), DbError> {
        if !(0..=MAX_SETUP_STEP).contains(&step) {
            return Err(DbError::InvalidInput(format!(
                "step must be between 0 and {MAX_SETUP_STEP}"
            )));
        }
        if data_json.len() > 32 * 1024 {
            return Err(DbError::InvalidInput(
                "progress data too large".to_string(),
            ));
        }
        match serde_json::from_str::<serde_json::Value>(data_json) {
            Ok(serde_json::Value::Object(_)) => Ok(()),
            _ => Err(DbError::InvalidInput(
                "data_json must be a JSON object".to_string(),
            )),
        }
    }

    pub async fn save(&self, step: i64, data_json: &str) -> Result<SetupProgress, DbError> {
        Self::validate_step_data(step, data_json)?;
        sqlx::query(
            "UPDATE setup_progress SET step = ?, data_json = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = 1",
        )
        .bind(step)
        .bind(data_json)
        .execute(self.pool.as_ref())
        .await?;
        self.get().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_step_data_accepts_object() {
        assert!(SetupProgressRepository::validate_step_data(0, "{}").is_ok());
        assert!(SetupProgressRepository::validate_step_data(
            7,
            r#"{"language":"fr","storage":"sda"}"#
        )
        .is_ok());
    }

    #[test]
    fn test_validate_step_data_rejects_bad_step() {
        assert!(SetupProgressRepository::validate_step_data(-1, "{}").is_err());
        assert!(SetupProgressRepository::validate_step_data(8, "{}").is_err());
    }

    #[test]
    fn test_validate_step_data_rejects_non_object() {
        assert!(SetupProgressRepository::validate_step_data(0, "[]").is_err());
        assert!(SetupProgressRepository::validate_step_data(0, "null").is_err());
        assert!(SetupProgressRepository::validate_step_data(0, "{oops").is_err());
    }

    #[test]
    fn test_validate_step_data_rejects_oversize() {
        let big = format!("{{\"k\":\"{}\"}}", "x".repeat(33 * 1024));
        assert!(SetupProgressRepository::validate_step_data(0, &big).is_err());
    }
}
