/* =============================================================================
 * File          : iscsi_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for the iSCSI export metadata: backstores, targets and their
 *   portals, LUNs and initiator ACLs. Every mutating call here writes rows
 *   only — nothing in this file talks to `targetcli` or to the kernel.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::iscsi
 *
 * Usage:
 *   handlers/iscsi.rs; see docs/guides/iscsi.md.
 * =============================================================================
 */

//! iSCSI export metadata (`iscsi_backstores` and friends, migration 071).

use chrono::Utc;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::iscsi::{IscsiAcl, IscsiBackstore, IscsiLun, IscsiPortal, IscsiTarget};
use crate::storage::database::DbError;

/// One initiator to store. `chap_secret_sealed` is already sealed with the
/// appliance key by the handler — this layer never sees a plaintext secret.
#[derive(Debug, Clone)]
pub struct IscsiAclInput {
    pub initiator_iqn: String,
    pub userid: Option<String>,
    pub chap_secret_sealed: Option<String>,
}

/// Everything the editor sends for one target, children included.
#[derive(Debug, Clone)]
pub struct IscsiTargetInput {
    pub iqn: String,
    pub enabled: bool,
    pub portals: Vec<(String, i64)>,
    pub luns: Vec<(i64, i64)>,
    pub acls: Vec<IscsiAclInput>,
}

#[derive(Clone)]
pub struct IscsiRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(IscsiRepository);

impl IscsiRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    // -- Backstores --------------------------------------------------------

    /// Every backstore, alphabetically, with its LUN count.
    pub async fn list_backstores(&self) -> Result<Vec<IscsiBackstore>, DbError> {
        Ok(sqlx::query_as::<_, IscsiBackstore>(
            "SELECT b.id, b.name, b.kind, b.path, b.size_bytes, b.readonly, \
             b.write_policy, b.emulate_fua, b.unmap, b.created_at, b.updated_at, \
             (SELECT COUNT(*) FROM iscsi_luns l WHERE l.backstore_id = b.id) AS lun_count \
             FROM iscsi_backstores b ORDER BY b.name COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    pub async fn find_backstore(&self, id: i64) -> Result<Option<IscsiBackstore>, DbError> {
        Ok(sqlx::query_as::<_, IscsiBackstore>(
            "SELECT b.id, b.name, b.kind, b.path, b.size_bytes, b.readonly, \
             b.write_policy, b.emulate_fua, b.unmap, b.created_at, b.updated_at, \
             (SELECT COUNT(*) FROM iscsi_luns l WHERE l.backstore_id = b.id) AS lun_count \
             FROM iscsi_backstores b WHERE b.id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?)
    }

    /// Whether another backstore already uses this name (case-insensitively).
    pub async fn backstore_name_taken(
        &self,
        name: &str,
        except: Option<i64>,
    ) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM iscsi_backstores WHERE name = ? COLLATE NOCASE AND id IS NOT ?",
        )
        .bind(name)
        .bind(except)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    /// Whether another backstore already points at this path. Two objects over
    /// one file would let two initiators corrupt each other.
    pub async fn backstore_path_taken(
        &self,
        path: &str,
        except: Option<i64>,
    ) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM iscsi_backstores WHERE path = ? AND id IS NOT ?",
        )
        .bind(path)
        .bind(except)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    /// Force a backstore read-only without touching any other field. The
    /// low-space guard uses this so a concurrent UI edit cannot be lost.
    pub async fn set_backstore_readonly(&self, id: i64, readonly: bool) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        let result =
            sqlx::query("UPDATE iscsi_backstores SET readonly = ?, updated_at = ? WHERE id = ?")
                .bind(readonly)
                .bind(&now)
                .bind(id)
                .execute(self.pool.as_ref())
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_backstore(
        &self,
        name: &str,
        kind: &str,
        path: &str,
        size_bytes: i64,
        readonly: bool,
        write_policy: &str,
        emulate_fua: bool,
        unmap: bool,
    ) -> Result<IscsiBackstore, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "INSERT INTO iscsi_backstores \
             (name, kind, path, size_bytes, readonly, write_policy, emulate_fua, unmap, \
              created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(name)
        .bind(kind)
        .bind(path)
        .bind(size_bytes)
        .bind(readonly)
        .bind(write_policy)
        .bind(emulate_fua)
        .bind(unmap)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;
        self.find_backstore(result.last_insert_rowid())
            .await?
            .ok_or(DbError::NotFound)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_backstore(
        &self,
        id: i64,
        name: &str,
        kind: &str,
        path: &str,
        size_bytes: i64,
        readonly: bool,
        write_policy: &str,
        emulate_fua: bool,
        unmap: bool,
    ) -> Result<IscsiBackstore, DbError> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE iscsi_backstores SET name = ?, kind = ?, path = ?, size_bytes = ?, \
             readonly = ?, write_policy = ?, emulate_fua = ?, unmap = ?, updated_at = ? \
             WHERE id = ?",
        )
        .bind(name)
        .bind(kind)
        .bind(path)
        .bind(size_bytes)
        .bind(readonly)
        .bind(write_policy)
        .bind(emulate_fua)
        .bind(unmap)
        .bind(&now)
        .bind(id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.find_backstore(id).await?.ok_or(DbError::NotFound)
    }

    /// Forget one backstore row. The caller has already checked
    /// [`IscsiRepository::backstore_lun_count`]: this never runs while a LUN
    /// still points at it.
    pub async fn delete_backstore(&self, id: i64) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM iscsi_backstores WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// How many LUNs on how many targets export this backstore.
    pub async fn backstore_lun_count(&self, id: i64) -> Result<i64, DbError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM iscsi_luns WHERE backstore_id = ?")
                .bind(id)
                .fetch_one(self.pool.as_ref())
                .await?;
        Ok(count)
    }

    // -- Targets -----------------------------------------------------------

    /// Every target with its portals, LUNs and ACLs.
    pub async fn list_targets(&self) -> Result<Vec<IscsiTarget>, DbError> {
        let rows = sqlx::query_as::<_, (i64, String, i64, String, String)>(
            "SELECT id, iqn, enabled, created_at, updated_at FROM iscsi_targets \
             ORDER BY iqn COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        let mut targets = Vec::with_capacity(rows.len());
        for (id, iqn, enabled, created_at, updated_at) in rows {
            targets.push(IscsiTarget {
                id,
                iqn,
                enabled: enabled != 0,
                created_at,
                updated_at,
                portals: Vec::new(),
                luns: Vec::new(),
                acls: Vec::new(),
            });
        }
        for target in &mut targets {
            target.portals = self.portals_for(target.id).await?;
            target.luns = self.luns_for(target.id).await?;
            target.acls = self.acls_for(target.id).await?;
        }
        Ok(targets)
    }

    pub async fn find_target(&self, id: i64) -> Result<Option<IscsiTarget>, DbError> {
        let row = sqlx::query_as::<_, (i64, String, i64, String, String)>(
            "SELECT id, iqn, enabled, created_at, updated_at FROM iscsi_targets WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        let Some((id, iqn, enabled, created_at, updated_at)) = row else {
            return Ok(None);
        };
        Ok(Some(IscsiTarget {
            id,
            iqn,
            enabled: enabled != 0,
            created_at,
            updated_at,
            portals: self.portals_for(id).await?,
            luns: self.luns_for(id).await?,
            acls: self.acls_for(id).await?,
        }))
    }

    pub async fn find_target_by_iqn(&self, iqn: &str) -> Result<Option<IscsiTarget>, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM iscsi_targets WHERE iqn = ? COLLATE NOCASE",
        )
        .bind(iqn)
        .fetch_optional(self.pool.as_ref())
        .await?;
        match row {
            Some((id,)) => self.find_target(id).await,
            None => Ok(None),
        }
    }

    pub async fn target_iqn_taken(&self, iqn: &str, except: Option<i64>) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT id FROM iscsi_targets WHERE iqn = ? COLLATE NOCASE AND id IS NOT ?",
        )
        .bind(iqn)
        .bind(except)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.is_some())
    }

    pub async fn create_target(&self, input: &IscsiTargetInput) -> Result<IscsiTarget, DbError> {
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO iscsi_targets (iqn, enabled, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&input.iqn)
        .bind(input.enabled)
        .bind(&now)
        .bind(&now)
        .execute(tx.as_mut())
        .await?;
        let id = result.last_insert_rowid();
        Self::insert_children(tx.as_mut(), id, input).await?;
        tx.commit().await?;
        self.find_target(id).await?.ok_or(DbError::NotFound)
    }

    /// Replace a target's metadata wholesale. The editor always sends the
    /// complete aggregate, so "what is not sent is gone" is the least
    /// surprising rule — and the one the UI can describe.
    pub async fn update_target(
        &self,
        id: i64,
        input: &IscsiTargetInput,
    ) -> Result<IscsiTarget, DbError> {
        let now = Utc::now().to_rfc3339();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE iscsi_targets SET iqn = ?, enabled = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&input.iqn)
        .bind(input.enabled)
        .bind(&now)
        .bind(id)
        .execute(tx.as_mut())
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        for table in ["iscsi_portals", "iscsi_luns", "iscsi_acls"] {
            let sql = match table {
                "iscsi_portals" => "DELETE FROM iscsi_portals WHERE target_id = ?",
                "iscsi_luns" => "DELETE FROM iscsi_luns WHERE target_id = ?",
                _ => "DELETE FROM iscsi_acls WHERE target_id = ?",
            };
            sqlx::query(sql).bind(id).execute(tx.as_mut()).await?;
        }
        Self::insert_children(tx.as_mut(), id, input).await?;
        tx.commit().await?;
        self.find_target(id).await?.ok_or(DbError::NotFound)
    }

    pub async fn delete_target(&self, id: i64) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM iscsi_targets WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    /// Whether any target still exports a given backstore. Used before an
    /// apply to warn about an orphan object, and before a delete to refuse it.
    pub async fn backstore_exported_targets(&self, id: i64) -> Result<Vec<String>, DbError> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT t.iqn FROM iscsi_luns l \
             JOIN iscsi_targets t ON t.id = l.target_id WHERE l.backstore_id = ? \
             ORDER BY t.iqn COLLATE NOCASE",
        )
        .bind(id)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(|(iqn,)| iqn).collect())
    }

    /// Sealed CHAP secrets with the target IQN and initiator they belong to,
    /// so the caller can derive each one's seal scope. A blob cannot be
    /// replayed onto another export: `open` fails its associated-data check.
    pub async fn chap_secrets_sealed(&self) -> Result<Vec<(String, String, String)>, DbError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            "SELECT t.iqn, a.initiator_iqn, a.chap_secret_sealed FROM iscsi_acls a \
             JOIN iscsi_targets t ON t.id = a.target_id \
             WHERE a.chap_secret_sealed IS NOT NULL",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows)
    }

    /// The sealed secret already stored for one initiator of one target,
    /// addressed by the initiator name the editor keeps stable across renames
    /// of the initiator string itself.
    pub async fn chap_secret_for(
        &self,
        target_id: i64,
        initiator_iqn: &str,
    ) -> Result<Option<String>, DbError> {
        let row = sqlx::query_as::<_, (String,)>(
            "SELECT chap_secret_sealed FROM iscsi_acls \
             WHERE target_id = ? AND initiator_iqn = ? COLLATE NOCASE \
               AND chap_secret_sealed IS NOT NULL",
        )
        .bind(target_id)
        .bind(initiator_iqn)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|(sealed,)| sealed))
    }

    // -- Children ----------------------------------------------------------

    async fn portals_for(&self, target_id: i64) -> Result<Vec<IscsiPortal>, DbError> {
        Ok(sqlx::query_as::<_, IscsiPortal>(
            "SELECT id, target_id, address, port FROM iscsi_portals \
             WHERE target_id = ? ORDER BY address, port",
        )
        .bind(target_id)
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    async fn luns_for(&self, target_id: i64) -> Result<Vec<IscsiLun>, DbError> {
        Ok(sqlx::query_as::<_, IscsiLun>(
            "SELECT l.id, l.target_id, l.lun, l.backstore_id, b.name AS backstore_name \
             FROM iscsi_luns l JOIN iscsi_backstores b ON b.id = l.backstore_id \
             WHERE l.target_id = ? ORDER BY l.lun",
        )
        .bind(target_id)
        .fetch_all(self.pool.as_ref())
        .await?)
    }

    async fn acls_for(&self, target_id: i64) -> Result<Vec<IscsiAcl>, DbError> {
        let rows = sqlx::query_as::<_, (i64, i64, String, Option<String>, Option<String>)>(
            "SELECT id, target_id, initiator_iqn, userid, chap_secret_sealed FROM iscsi_acls \
             WHERE target_id = ? ORDER BY initiator_iqn COLLATE NOCASE",
        )
        .bind(target_id)
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, target_id, initiator_iqn, userid, sealed)| IscsiAcl {
                id,
                target_id,
                initiator_iqn,
                userid,
                has_secret: sealed.is_some(),
            })
            .collect())
    }

    async fn insert_children(
        conn: &mut sqlx::SqliteConnection,
        target_id: i64,
        input: &IscsiTargetInput,
    ) -> Result<(), DbError> {
        for (address, port) in &input.portals {
            sqlx::query("INSERT INTO iscsi_portals (target_id, address, port) VALUES (?, ?, ?)")
                .bind(target_id)
                .bind(address)
                .bind(port)
                .execute(&mut *conn)
                .await?;
        }
        for (lun, backstore_id) in &input.luns {
            sqlx::query("INSERT INTO iscsi_luns (target_id, lun, backstore_id) VALUES (?, ?, ?)")
                .bind(target_id)
                .bind(lun)
                .bind(backstore_id)
                .execute(&mut *conn)
                .await?;
        }
        let now = Utc::now().to_rfc3339();
        for acl in &input.acls {
            sqlx::query(
                "INSERT INTO iscsi_acls \
                 (target_id, initiator_iqn, userid, chap_secret_sealed, created_at) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(target_id)
            .bind(&acl.initiator_iqn)
            .bind(&acl.userid)
            .bind(&acl.chap_secret_sealed)
            .bind(&now)
            .execute(&mut *conn)
            .await?;
        }
        Ok(())
    }
}
