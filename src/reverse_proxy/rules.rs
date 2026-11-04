/* =============================================================================
 * File          : rules.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Persistence of the user-defined reverse-proxy rules. One row is one
 *   `host [+ path] → upstream` mapping; the listener reads a snapshot, so a
 *   request never queries SQLite.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::storage::database::DbError
 *
 * Usage:
 *   handlers/reverse_proxy.rs (CRUD) and reverse_proxy::ReverseProxyManager
 *   (startup snapshot, reload after each mutation).
 * ============================================================================= */

//! SQLite persistence for [`crate::reverse_proxy::ProxyRule`].

use sqlx::SqlitePool;

use crate::storage::database::DbError;

use super::ProxyRule;

/// The columns every read selects, in one place so the row index cannot drift.
/// Kept as a comment because sqlx 0.9 requires a literal SQL string: each
/// query below spells the same list out.
type RuleRow = (i64, String, String, String, i64, i64, i64, String, String);

fn row_to_rule(row: RuleRow) -> ProxyRule {
    ProxyRule {
        id: row.0,
        host: row.1,
        path: row.2,
        upstream: row.3,
        tls: row.4 != 0,
        auth_required: row.5 != 0,
        enabled: row.6 != 0,
        created_at: row.7,
        updated_at: row.8,
    }
}

/// Every rule, newest id last, for the API and the listener snapshot.
pub async fn list(pool: &SqlitePool) -> Result<Vec<ProxyRule>, DbError> {
    let rows = sqlx::query_as::<_, RuleRow>(
        "SELECT id, host, path, upstream, tls, auth_required, enabled, created_at, updated_at \
         FROM reverse_proxy_rules ORDER BY host COLLATE NOCASE, path, id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_rule).collect())
}

/// Enabled rules only — what the listener needs.
pub async fn list_enabled(pool: &SqlitePool) -> Result<Vec<ProxyRule>, DbError> {
    let rows = sqlx::query_as::<_, RuleRow>(
        "SELECT id, host, path, upstream, tls, auth_required, enabled, created_at, updated_at \
         FROM reverse_proxy_rules WHERE enabled = 1 \
         ORDER BY host COLLATE NOCASE, path, id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_rule).collect())
}

/// One rule by id.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ProxyRule>, DbError> {
    let row = sqlx::query_as::<_, RuleRow>(
        "SELECT id, host, path, upstream, tls, auth_required, enabled, created_at, updated_at \
         FROM reverse_proxy_rules WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_rule))
}

/// Insert one validated rule and return the stored row.
pub async fn create(pool: &SqlitePool, rule: &ProxyRule) -> Result<ProxyRule, DbError> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO reverse_proxy_rules (host, path, upstream, tls, auth_required, enabled) \
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&rule.host)
    .bind(&rule.path)
    .bind(&rule.upstream)
    .bind(rule.tls as i64)
    .bind(rule.auth_required as i64)
    .bind(rule.enabled as i64)
    .fetch_one(pool)
    .await?;
    get(pool, id)
        .await?
        .ok_or_else(|| DbError::Migration("the created proxy rule vanished".to_string()))
}

/// Replace the mutable fields of one rule. `None` when the id does not exist.
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    rule: &ProxyRule,
) -> Result<Option<ProxyRule>, DbError> {
    let affected = sqlx::query(
        "UPDATE reverse_proxy_rules \
         SET host = ?, path = ?, upstream = ?, tls = ?, auth_required = ?, enabled = ?, \
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
         WHERE id = ?",
    )
    .bind(&rule.host)
    .bind(&rule.path)
    .bind(&rule.upstream)
    .bind(rule.tls as i64)
    .bind(rule.auth_required as i64)
    .bind(rule.enabled as i64)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if affected == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

/// Remove one rule. `true` when a row was deleted.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool, DbError> {
    let affected = sqlx::query("DELETE FROM reverse_proxy_rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// `true` when another rule already owns the same host + path. Used by the
/// handler to answer a readable `400` instead of surfacing the SQLite
/// constraint, and by `POST` to refuse an exact duplicate.
pub async fn host_path_taken(
    pool: &SqlitePool,
    host: &str,
    path: &str,
    except_id: Option<i64>,
) -> Result<bool, DbError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM reverse_proxy_rules \
         WHERE host = ? AND path = ? AND id <> ?)",
    )
    .bind(host)
    .bind(path)
    .bind(except_id.unwrap_or(-1))
    .fetch_one(pool)
    .await?;
    Ok(exists)
}
