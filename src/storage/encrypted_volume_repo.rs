use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use uuid::Uuid;

/// A LUKS2-encrypted standalone volume's metadata. The passphrase is
/// deliberately not part of this model — it is never persisted.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EncryptedVolume {
    pub id: String,
    pub device_path: String,
    pub mapper_name: String,
    pub luks_uuid: String,
    pub filesystem: String,
    pub label: String,
    pub mount_point: String,
    pub created_at: String,
}

pub struct EncryptedVolumeRepository {
    pool: Arc<SqlitePool>,
}

impl EncryptedVolumeRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list_all(&self) -> Result<Vec<EncryptedVolume>, sqlx::Error> {
        sqlx::query_as::<_, EncryptedVolume>(
            "SELECT id, device_path, mapper_name, luks_uuid, filesystem, label, mount_point, created_at FROM encrypted_volumes ORDER BY label",
        )
        .fetch_all(self.pool.as_ref())
        .await
    }

    pub async fn get_by_id(&self, id: &str) -> Result<Option<EncryptedVolume>, sqlx::Error> {
        sqlx::query_as::<_, EncryptedVolume>(
            "SELECT id, device_path, mapper_name, luks_uuid, filesystem, label, mount_point, created_at FROM encrypted_volumes WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        device_path: &str,
        mapper_name: &str,
        luks_uuid: &str,
        filesystem: &str,
        label: &str,
        mount_point: &str,
    ) -> Result<EncryptedVolume, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO encrypted_volumes (id, device_path, mapper_name, luks_uuid, filesystem, label, mount_point, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(device_path)
        .bind(mapper_name)
        .bind(luks_uuid)
        .bind(filesystem)
        .bind(label)
        .bind(mount_point)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        Ok(EncryptedVolume {
            id,
            device_path: device_path.to_string(),
            mapper_name: mapper_name.to_string(),
            luks_uuid: luks_uuid.to_string(),
            filesystem: filesystem.to_string(),
            label: label.to_string(),
            mount_point: mount_point.to_string(),
            created_at: now,
        })
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM encrypted_volumes WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
