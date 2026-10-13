//! Storage layer for the external AI-agent (MCP) server: issued bearer
//! tokens and the singleton settings row. Only the SHA-256 hash of a token's
//! secret half is stored — a database read alone never yields a usable
//! token. See `sqlx/migrations/038_mcp_server.sql` and `crate::mcp`.

use sqlx::SqlitePool;
use std::sync::Arc;

/// One issued MCP token (the secret itself is never stored).
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpToken {
    pub id: String,
    pub name: String,
    /// First characters of the full token, shown so the admin can tell
    /// tokens apart after issuing (the full value is shown exactly once).
    pub prefix: String,
    pub created_by: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

pub struct McpRepository {
    pool: Arc<SqlitePool>,
}

impl McpRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<McpToken>, sqlx::Error> {
        let rows = sqlx::query_as::<_, McpTokenRow>(
            "SELECT id, name, prefix, created_by, created_at, last_used_at
             FROM mcp_tokens ORDER BY created_at ASC",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(McpTokenRow::into_model).collect())
    }

    pub async fn insert(
        &self,
        id: &str,
        name: &str,
        secret_hash: &str,
        prefix: &str,
        created_by: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO mcp_tokens (id, name, secret_hash, prefix, created_by)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(secret_hash)
        .bind(prefix)
        .bind(created_by)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn secret_hash(&self, id: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT secret_hash FROM mcp_tokens WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool.as_ref())
                .await?;
        Ok(row.map(|(h,)| h))
    }

    pub async fn touch_last_used(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE mcp_tokens SET last_used_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM mcp_tokens WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn count(&self) -> Result<i64, sqlx::Error> {
        let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM mcp_tokens")
            .fetch_one(self.pool.as_ref())
            .await?;
        Ok(n)
    }

    pub async fn get_settings(&self) -> Result<(bool, String), sqlx::Error> {
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT enabled, permissions FROM mcp_settings WHERE id = 1")
                .fetch_optional(self.pool.as_ref())
                .await?;
        Ok(row
            .map(|(e, p)| (e != 0, p))
            .unwrap_or((false, "{}".to_string())))
    }

    pub async fn set_enabled(&self, enabled: bool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE mcp_settings SET enabled = ?, updated_at = datetime('now') WHERE id = 1",
        )
        .bind(enabled as i64)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    pub async fn set_permissions(&self, permissions_json: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE mcp_settings SET permissions = ?, updated_at = datetime('now') WHERE id = 1",
        )
        .bind(permissions_json)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct McpTokenRow {
    id: String,
    name: String,
    prefix: String,
    created_by: String,
    created_at: String,
    last_used_at: Option<String>,
}

impl McpTokenRow {
    fn into_model(self) -> McpToken {
        McpToken {
            id: self.id,
            name: self.name,
            prefix: self.prefix,
            created_by: self.created_by,
            created_at: self.created_at,
            last_used_at: self.last_used_at,
        }
    }
}
