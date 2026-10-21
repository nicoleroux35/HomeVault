/* =============================================================================
 * File          : resource_share_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-member resource grants (app/folder/share, read/write): the
 *   fine-grained access layer on top of roles, enforced server-side.
 *
 * Dependencies:
 *   - sqlx (>= 0.9)
 * Usage:
 *   Imported by the services/handlers of its feature area.
 * =============================================================================
 */

//! Per-member resource grants (apps, folders, shares).

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::storage::database::DbError;

/// The kind of resource a `resource_shares` row grants access to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceKind {
    /// A Docker/compose app id (e.g. `jellyfin`).
    App,
    /// An absolute path under a home or a shared root.
    Folder,
    /// An SMB/NFS/FTP/WebDAV share name.
    Share,
}

impl ResourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceKind::App => "app",
            ResourceKind::Folder => "folder",
            ResourceKind::Share => "share",
        }
    }

    /// Parse the wire/db representation. Returns `None` for anything else, so
    /// a bad request value is a 400 rather than a silent default.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "app" => Some(ResourceKind::App),
            "folder" => Some(ResourceKind::Folder),
            "share" => Some(ResourceKind::Share),
            _ => None,
        }
    }
}

/// Access level granted by a `resource_shares` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourcePermission {
    Read,
    Write,
    Full,
}

impl ResourcePermission {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourcePermission::Read => "read",
            ResourcePermission::Write => "write",
            ResourcePermission::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ResourcePermission::Read),
            "write" => Some(ResourcePermission::Write),
            "full" => Some(ResourcePermission::Full),
            _ => None,
        }
    }

    /// Whether this level implies `other` (full ⊇ write ⊇ read).
    pub fn implies(self, other: ResourcePermission) -> bool {
        match self {
            ResourcePermission::Full => true,
            ResourcePermission::Write => {
                matches!(other, ResourcePermission::Read | ResourcePermission::Write)
            }
            ResourcePermission::Read => matches!(other, ResourcePermission::Read),
        }
    }
}

/// Database row from `resource_shares`.
#[derive(Debug, Clone, FromRow)]
pub struct ResourceShareRow {
    pub id: i64,
    pub owner: String,
    pub member: String,
    pub kind: String,
    pub resource: String,
    pub permission: String,
    pub created_at: String,
}

/// A resource access grant for one member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceShare {
    pub id: i64,
    pub owner: String,
    pub member: String,
    pub kind: String,
    pub resource: String,
    pub permission: String,
    pub created_at: String,
}

impl From<ResourceShareRow> for ResourceShare {
    fn from(row: ResourceShareRow) -> Self {
        Self {
            id: row.id,
            owner: row.owner,
            member: row.member,
            kind: row.kind,
            resource: row.resource,
            permission: row.permission,
            created_at: row.created_at,
        }
    }
}

/// A resource access grant held by a *group* (`resource_group_shares`,
/// migration 051). Every member of `group_name` inherits it, at the union of
/// their own direct grants and this one, strongest permission winning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceGroupShare {
    pub id: i64,
    pub owner: String,
    pub group_name: String,
    pub kind: String,
    pub resource: String,
    pub permission: String,
    pub created_at: String,
}

/// Database row from `resource_group_shares`.
#[derive(Debug, Clone, FromRow)]
pub struct ResourceGroupShareRow {
    pub id: i64,
    pub owner: String,
    pub group_name: String,
    pub kind: String,
    pub resource: String,
    pub permission: String,
    pub created_at: String,
}

impl From<ResourceGroupShareRow> for ResourceGroupShare {
    fn from(row: ResourceGroupShareRow) -> Self {
        Self {
            id: row.id,
            owner: row.owner,
            group_name: row.group_name,
            kind: row.kind,
            resource: row.resource,
            permission: row.permission,
            created_at: row.created_at,
        }
    }
}

/// Repository for per-member resource access grants.
#[derive(Clone)]
pub struct ResourceShareRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(ResourceShareRepository);

impl ResourceShareRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Grant (or update) `member`'s access to `kind:resource`. Idempotent: a
    /// second grant for the same (member, kind, resource) updates the level.
    pub async fn upsert(
        &self,
        owner: &str,
        member: &str,
        kind: ResourceKind,
        resource: &str,
        permission: ResourcePermission,
    ) -> Result<ResourceShare, DbError> {
        sqlx::query(
            r#"
            INSERT INTO resource_shares (owner, member, kind, resource, permission)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(member, kind, resource) DO UPDATE SET
                permission = excluded.permission,
                owner = excluded.owner
            "#,
        )
        .bind(owner)
        .bind(member)
        .bind(kind.as_str())
        .bind(resource)
        .bind(permission.as_str())
        .execute(self.pool.as_ref())
        .await?;

        self.find(member, kind, resource)
            .await?
            .ok_or(DbError::NotFound)
    }

    /// Look up one grant.
    pub async fn find(
        &self,
        member: &str,
        kind: ResourceKind,
        resource: &str,
    ) -> Result<Option<ResourceShare>, DbError> {
        let row = sqlx::query_as::<_, ResourceShareRow>(
            "SELECT id, owner, member, kind, resource, permission, created_at FROM resource_shares WHERE member = ? AND kind = ? AND resource = ?",
        )
        .bind(member)
        .bind(kind.as_str())
        .bind(resource)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(ResourceShare::from))
    }

    /// Every grant held by `member` (optionally narrowed to one kind).
    pub async fn list_for_member(
        &self,
        member: &str,
        kind: Option<ResourceKind>,
    ) -> Result<Vec<ResourceShare>, DbError> {
        let rows = match kind {
            Some(k) => {
                sqlx::query_as::<_, ResourceShareRow>(
                    "SELECT id, owner, member, kind, resource, permission, created_at FROM resource_shares WHERE member = ? AND kind = ? ORDER BY kind, resource",
                )
                .bind(member)
                .bind(k.as_str())
                .fetch_all(self.pool.as_ref())
                .await?
            }
            None => {
                sqlx::query_as::<_, ResourceShareRow>(
                    "SELECT id, owner, member, kind, resource, permission, created_at FROM resource_shares WHERE member = ? ORDER BY kind, resource",
                )
                .bind(member)
                .fetch_all(self.pool.as_ref())
                .await?
            }
        };
        Ok(rows.into_iter().map(ResourceShare::from).collect())
    }

    /// Every grant on the instance (admin/owner view), optionally for one member.
    pub async fn list_all(&self, member: Option<&str>) -> Result<Vec<ResourceShare>, DbError> {
        let rows = match member {
            Some(m) => {
                sqlx::query_as::<_, ResourceShareRow>(
                    "SELECT id, owner, member, kind, resource, permission, created_at FROM resource_shares WHERE member = ? ORDER BY member, kind, resource",
                )
                .bind(m)
                .fetch_all(self.pool.as_ref())
                .await?
            }
            None => {
                sqlx::query_as::<_, ResourceShareRow>(
                    "SELECT id, owner, member, kind, resource, permission, created_at FROM resource_shares ORDER BY member, kind, resource",
                )
                .fetch_all(self.pool.as_ref())
                .await?
            }
        };
        Ok(rows.into_iter().map(ResourceShare::from).collect())
    }

    /// Revoke one grant. Returns `true` when a row was removed.
    pub async fn revoke(
        &self,
        member: &str,
        kind: ResourceKind,
        resource: &str,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "DELETE FROM resource_shares WHERE member = ? AND kind = ? AND resource = ?",
        )
        .bind(member)
        .bind(kind.as_str())
        .bind(resource)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Revoke every grant held by a member (called when the account is deleted;
    /// the FK cascade also covers this, but an explicit call keeps the
    /// behaviour testable without relying on `PRAGMA foreign_keys` being on).
    pub async fn revoke_all_for_member(&self, member: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM resource_shares WHERE member = ?")
            .bind(member)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

    /// Grant (or update) `group_name`'s access to `kind:resource`. Idempotent,
    /// same contract as [`Self::upsert`] for a single member.
    pub async fn upsert_group(
        &self,
        owner: &str,
        group_name: &str,
        kind: ResourceKind,
        resource: &str,
        permission: ResourcePermission,
    ) -> Result<ResourceGroupShare, DbError> {
        sqlx::query(
            r#"
            INSERT INTO resource_group_shares (owner, group_name, kind, resource, permission)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(group_name, kind, resource) DO UPDATE SET
                permission = excluded.permission,
                owner = excluded.owner
            "#,
        )
        .bind(owner)
        .bind(group_name)
        .bind(kind.as_str())
        .bind(resource)
        .bind(permission.as_str())
        .execute(self.pool.as_ref())
        .await?;

        self.find_group(group_name, kind, resource)
            .await?
            .ok_or(DbError::NotFound)
    }

    /// Look up one group grant.
    pub async fn find_group(
        &self,
        group_name: &str,
        kind: ResourceKind,
        resource: &str,
    ) -> Result<Option<ResourceGroupShare>, DbError> {
        let row = sqlx::query_as::<_, ResourceGroupShareRow>(
            "SELECT id, owner, group_name, kind, resource, permission, created_at FROM resource_group_shares WHERE group_name = ? AND kind = ? AND resource = ?",
        )
        .bind(group_name)
        .bind(kind.as_str())
        .bind(resource)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(ResourceGroupShare::from))
    }

    /// Every group grant on the instance, optionally narrowed to one group.
    pub async fn list_groups_all(
        &self,
        group_name: Option<&str>,
    ) -> Result<Vec<ResourceGroupShare>, DbError> {
        let rows = match group_name {
            Some(g) => {
                sqlx::query_as::<_, ResourceGroupShareRow>(
                    "SELECT id, owner, group_name, kind, resource, permission, created_at FROM resource_group_shares WHERE group_name = ? ORDER BY group_name, kind, resource",
                )
                .bind(g)
                .fetch_all(self.pool.as_ref())
                .await?
            }
            None => {
                sqlx::query_as::<_, ResourceGroupShareRow>(
                    "SELECT id, owner, group_name, kind, resource, permission, created_at FROM resource_group_shares ORDER BY group_name, kind, resource",
                )
                .fetch_all(self.pool.as_ref())
                .await?
            }
        };
        Ok(rows.into_iter().map(ResourceGroupShare::from).collect())
    }

    /// Revoke one group grant. Returns `true` when a row was removed.
    pub async fn revoke_group(
        &self,
        group_name: &str,
        kind: ResourceKind,
        resource: &str,
    ) -> Result<bool, DbError> {
        let result = sqlx::query(
            "DELETE FROM resource_group_shares WHERE group_name = ? AND kind = ? AND resource = ?",
        )
        .bind(group_name)
        .bind(kind.as_str())
        .bind(resource)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Follow a group rename: every grant keyed by `old_name` moves to
    /// `new_name`. Returns the number of rows moved. The OS rename has already
    /// happened by the time a caller gets here, so this must never be skipped.
    pub async fn rename_group_name(&self, old_name: &str, new_name: &str) -> Result<u64, DbError> {
        let result =
            sqlx::query("UPDATE resource_group_shares SET group_name = ? WHERE group_name = ?")
                .bind(new_name)
                .bind(old_name)
                .execute(self.pool.as_ref())
                .await?;
        Ok(result.rows_affected())
    }

    /// Revoke every grant a group holds (group deleted/renamed away).
    pub async fn revoke_all_for_group(&self, group_name: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM resource_group_shares WHERE group_name = ?")
            .bind(group_name)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

    /// Every grant `groups` hold for `kind`, as `resource -> permission`.
    pub async fn visible_resources_for_groups(
        &self,
        groups: &[String],
        kind: ResourceKind,
    ) -> Result<Vec<(String, String)>, DbError> {
        let mut out: Vec<(String, String)> = Vec::new();
        for group in groups {
            let rows = sqlx::query_as::<_, (String, String)>(
                "SELECT resource, permission FROM resource_group_shares WHERE group_name = ? AND kind = ? ORDER BY resource",
            )
            .bind(group)
            .bind(kind.as_str())
            .fetch_all(self.pool.as_ref())
            .await?;
            out.extend(rows);
        }
        Ok(out)
    }

    /// Merge two `resource -> permission` lists into one, keeping the strongest
    /// level per resource (`full` ⊃ `write` ⊃ `read`). Pure, so the union rule
    /// is unit-tested without a database.
    pub fn merge_strongest(
        direct: Vec<(String, String)>,
        inherited: Vec<(String, String)>,
    ) -> Vec<(String, String)> {
        use std::collections::BTreeMap;
        let mut merged: BTreeMap<String, ResourcePermission> = BTreeMap::new();
        for (resource, level) in direct.into_iter().chain(inherited) {
            let Some(parsed) = ResourcePermission::parse(&level) else {
                continue;
            };
            merged
                .entry(resource)
                .and_modify(|current| {
                    if parsed.implies(*current) {
                        *current = parsed;
                    }
                })
                .or_insert(parsed);
        }
        merged
            .into_iter()
            .map(|(r, p)| (r, p.as_str().to_string()))
            .collect()
    }

    /// The set of resources of `kind` that `member` may see, as a
    /// `resource -> permission` map. This is the server-side filter every
    /// listing and WebSocket event must pass through.
    pub async fn visible_resources(
        &self,
        member: &str,
        kind: ResourceKind,
    ) -> Result<Vec<(String, String)>, DbError> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT resource, permission FROM resource_shares WHERE member = ? AND kind = ? ORDER BY resource",
        )
        .bind(member)
        .bind(kind.as_str())
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows)
    }

    /// Whether `member` may see `resource` at (at least) `needed`. Owners and
    /// admins are handled by the caller (`is_privileged`), not here, so this
    /// stays a pure data check.
    pub async fn can_access(
        &self,
        member: &str,
        kind: ResourceKind,
        resource: &str,
        needed: ResourcePermission,
    ) -> Result<bool, DbError> {
        let Some(grant) = self.find(member, kind, resource).await? else {
            return Ok(false);
        };
        Ok(ResourcePermission::parse(&grant.permission)
            .map(|p| p.implies(needed))
            .unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_kind_roundtrip() {
        for k in [ResourceKind::App, ResourceKind::Folder, ResourceKind::Share] {
            assert_eq!(ResourceKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(ResourceKind::parse("bogus"), None);
        assert_eq!(ResourceKind::parse(""), None);
    }

    #[test]
    fn test_resource_permission_roundtrip() {
        for p in [
            ResourcePermission::Read,
            ResourcePermission::Write,
            ResourcePermission::Full,
        ] {
            assert_eq!(ResourcePermission::parse(p.as_str()), Some(p));
        }
        assert_eq!(ResourcePermission::parse("admin"), None);
    }

    #[test]
    fn test_merge_strongest_takes_the_stronger_level() {
        // Direct read + inherited write → write.
        let merged = ResourceShareRepository::merge_strongest(
            vec![("jellyfin".to_string(), "read".to_string())],
            vec![("jellyfin".to_string(), "write".to_string())],
        );
        assert_eq!(merged, vec![("jellyfin".to_string(), "write".to_string())]);

        // Direct full + inherited read → full (a weaker group row must never
        // demote a member who holds a stronger direct grant).
        let merged = ResourceShareRepository::merge_strongest(
            vec![("media".to_string(), "full".to_string())],
            vec![("media".to_string(), "read".to_string())],
        );
        assert_eq!(merged, vec![("media".to_string(), "full".to_string())]);
    }

    #[test]
    fn test_merge_strongest_is_a_union_sorted_by_resource() {
        let merged = ResourceShareRepository::merge_strongest(
            vec![("b-app".to_string(), "read".to_string())],
            vec![
                ("a-folder".to_string(), "write".to_string()),
                ("b-app".to_string(), "full".to_string()),
            ],
        );
        assert_eq!(
            merged,
            vec![
                ("a-folder".to_string(), "write".to_string()),
                ("b-app".to_string(), "full".to_string()),
            ]
        );
    }

    #[test]
    fn test_merge_strongest_ignores_unknown_levels() {
        // A corrupt row must not grant anything (fail closed), and must not
        // shadow a valid row for the same resource either.
        let merged = ResourceShareRepository::merge_strongest(
            vec![("x".to_string(), "admin".to_string())],
            vec![("x".to_string(), "read".to_string())],
        );
        assert_eq!(merged, vec![("x".to_string(), "read".to_string())]);
        assert!(ResourceShareRepository::merge_strongest(
            vec![("y".to_string(), "bogus".to_string())],
            vec![]
        )
        .is_empty());
    }

    #[test]
    fn test_permission_implies_is_a_total_order() {
        use ResourcePermission::*;
        // full ⊇ write ⊇ read
        assert!(Full.implies(Full) && Full.implies(Write) && Full.implies(Read));
        assert!(!Write.implies(Full));
        assert!(Write.implies(Write) && Write.implies(Read));
        assert!(!Read.implies(Write) && !Read.implies(Full));
        assert!(Read.implies(Read));
    }
}
