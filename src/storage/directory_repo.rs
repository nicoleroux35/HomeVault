/* =============================================================================
 * File          : directory_repo.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Storage for directory principals: the `remote_identities` rows that say
 *   who an external provider vouched for, and the mirrored `users` rows that
 *   let an existing session outlive this module. A mirrored row can never be
 *   signed into with a password — it carries a sentinel hash — and a local
 *   account can never be overwritten by a directory principal of the same
 *   name.
 *
 * Dependencies:
 *   - sqlx (>= 0.9), crate::directory
 *
 * Usage:
 *   handlers/directory.rs and the login path; see docs/guides/directory-sso.md.
 * =============================================================================
 */

//! Directory principals (`remote_identities`, migration 072).

use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::impl_repository;
use crate::storage::database::DbError;
use crate::storage::models::{ADMIN_ROLE_ID, USER_ROLE_ID};

/// The sentinel `password_hash` a mirrored directory account carries.
///
/// It is not a valid Argon2 PHC string, so `verify_password` can never accept
/// it. Unlike [`crate::storage::models::PAM_PROVISIONED_HASH`] it does **not**
/// fall through to PAM: a directory principal's password lives in the
/// directory, and the login path routes it there before any local check.
pub const DIRECTORY_PROVISIONED_HASH: &str = "!directory-provisioned";

/// Where a `users` row came from. `local` is every account created on the
/// appliance; the others mirror a provider.
pub const AUTH_SOURCE_LOCAL: &str = "local";

/// One principal the directory vouched for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteIdentity {
    pub id: i64,
    /// `ldap` / `ad` today, `oidc` / `saml` when those stages land.
    pub provider: String,
    /// The provider's stable key (the DN for LDAP).
    pub subject: String,
    pub username: String,
    pub display_name: String,
    pub email: String,
    /// Group list exactly as the provider returned it.
    pub groups: Vec<String>,
    pub role_id: i64,
    pub disabled: bool,
    pub last_login_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl RemoteIdentity {
    pub fn is_admin(&self) -> bool {
        self.role_id == ADMIN_ROLE_ID
    }

    pub fn role_name(&self) -> &'static str {
        if self.is_admin() {
            "admin"
        } else {
            "user"
        }
    }
}

/// What a successful directory sign-in contributes.
#[derive(Debug, Clone)]
pub struct IdentityUpdate {
    pub provider: String,
    pub subject: String,
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub groups: Vec<String>,
    pub role_id: i64,
}

#[derive(Clone)]
pub struct DirectoryRepository {
    pool: Arc<SqlitePool>,
}

impl_repository!(DirectoryRepository);

/// A raw row, before the JSON group list is decoded.
#[derive(sqlx::FromRow)]
struct IdentityRow {
    id: i64,
    provider: String,
    subject: String,
    username: String,
    display_name: String,
    email: String,
    groups: String,
    role_id: i64,
    disabled: bool,
    last_login_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl IdentityRow {
    fn into_identity(self) -> RemoteIdentity {
        RemoteIdentity {
            id: self.id,
            provider: self.provider,
            subject: self.subject,
            username: self.username,
            display_name: self.display_name,
            email: self.email,
            groups: serde_json::from_str(&self.groups).unwrap_or_default(),
            role_id: self.role_id,
            disabled: self.disabled,
            last_login_at: self.last_login_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

impl DirectoryRepository {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }

    /// Every principal, newest login first — the screen's list.
    pub async fn list(&self) -> Result<Vec<RemoteIdentity>, DbError> {
        let rows = sqlx::query_as::<_, IdentityRow>(
            "SELECT id, provider, subject, username, display_name, email, groups, \
             role_id, disabled, last_login_at, created_at, updated_at \
             FROM remote_identities ORDER BY username COLLATE NOCASE",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows.into_iter().map(IdentityRow::into_identity).collect())
    }

    pub async fn count(&self) -> Result<i64, DbError> {
        let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM remote_identities")
            .fetch_one(self.pool.as_ref())
            .await?;
        Ok(count)
    }

    pub async fn find_by_subject(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<RemoteIdentity>, DbError> {
        let row = sqlx::query_as::<_, IdentityRow>(
            "SELECT id, provider, subject, username, display_name, email, groups, \
             role_id, disabled, last_login_at, created_at, updated_at \
             FROM remote_identities WHERE provider = ? AND subject = ?",
        )
        .bind(provider)
        .bind(subject)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(IdentityRow::into_identity))
    }

    pub async fn find_by_username(
        &self,
        username: &str,
    ) -> Result<Option<RemoteIdentity>, DbError> {
        let row = sqlx::query_as::<_, IdentityRow>(
            "SELECT id, provider, subject, username, display_name, email, groups, \
             role_id, disabled, last_login_at, created_at, updated_at \
             FROM remote_identities WHERE username = ? COLLATE NOCASE",
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(IdentityRow::into_identity))
    }

    pub async fn find(&self, id: i64) -> Result<Option<RemoteIdentity>, DbError> {
        let row = sqlx::query_as::<_, IdentityRow>(
            "SELECT id, provider, subject, username, display_name, email, groups, \
             role_id, disabled, last_login_at, created_at, updated_at \
             FROM remote_identities WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(IdentityRow::into_identity))
    }

    /// Whether a `users` row with this name exists at all.
    pub async fn user_exists(&self, username: &str) -> Result<bool, DbError> {
        let row = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM users WHERE username = ? COLLATE NOCASE",
        )
        .bind(username)
        .fetch_one(self.pool.as_ref())
        .await?;
        Ok(row.0 > 0)
    }

    /// The `auth_source` of a `users` row, or `None` when no such account
    /// exists. This is what tells the login path "check this password here" from
    /// "this principal's password lives in the directory".
    pub async fn auth_source(&self, username: &str) -> Result<Option<String>, DbError> {
        let row = sqlx::query_as::<_, (String,)>(
            "SELECT auth_source FROM users WHERE username = ? COLLATE NOCASE",
        )
        .bind(username)
        .fetch_optional(self.pool.as_ref())
        .await?;
        Ok(row.map(|(source,)| source))
    }

    /// Record a successful directory sign-in and mirror it into `users`.
    ///
    /// The mirror is what a session needs; the identity row is the source of
    /// truth. Refuses to touch a **local** account of the same name, so a
    /// directory cannot take over an appliance administrator.
    ///
    /// `disabled` is deliberately **not** refreshed: an administrator switched
    /// that principal off, and a sign-in must not silently switch it back on.
    pub async fn upsert_login(&self, update: &IdentityUpdate) -> Result<RemoteIdentity, DbError> {
        if let Some(source) = self.auth_source(&update.username).await? {
            if source == AUTH_SOURCE_LOCAL {
                return Err(DbError::InvalidInput(format!(
                    "'{}' is a local account — a directory principal cannot claim its name",
                    update.username
                )));
            }
        }

        let now = Utc::now().to_rfc3339();
        let groups = serde_json::to_string(&update.groups).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            "INSERT INTO remote_identities \
             (provider, subject, username, display_name, email, groups, role_id, disabled, \
              last_login_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?) \
             ON CONFLICT(provider, subject) DO UPDATE SET \
               username = excluded.username, \
               display_name = excluded.display_name, \
               email = excluded.email, \
               groups = excluded.groups, \
               role_id = excluded.role_id, \
               last_login_at = excluded.last_login_at, \
               updated_at = excluded.updated_at",
        )
        .bind(&update.provider)
        .bind(&update.subject)
        .bind(&update.username)
        .bind(&update.display_name)
        .bind(&update.email)
        .bind(&groups)
        .bind(update.role_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(self.pool.as_ref())
        .await?;

        // The username is unique, so a rename in the directory can collide with
        // another row. The database refuses it; surface that instead of
        // silently keeping two principals for one name.
        self.ensure_shadow_user(
            &update.username,
            &update.display_name,
            &update.email,
            update.role_id,
        )
        .await?;

        self.find_by_subject(&update.provider, &update.subject)
            .await?
            .ok_or(DbError::NotFound)
    }

    /// Create or refresh the `users` row a session needs.
    async fn ensure_shadow_user(
        &self,
        username: &str,
        display_name: &str,
        email: &str,
        role_id: i64,
    ) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO users \
             (username, password_hash, fullname, email, role_id, locked, created_at, \
              avatar_seed, must_change_password, auth_source) \
             VALUES (?, ?, ?, ?, ?, 0, ?, ?, 0, 'directory') \
             ON CONFLICT(username) DO UPDATE SET \
               fullname = excluded.fullname, \
               email = excluded.email, \
               role_id = excluded.role_id, \
               locked = 0, \
               auth_source = 'directory' \
             WHERE users.auth_source <> 'local'",
        )
        .bind(username)
        .bind(DIRECTORY_PROVISIONED_HASH)
        .bind(display_name)
        .bind(email)
        .bind(role_id)
        .bind(&now)
        .bind(username)
        .execute(self.pool.as_ref())
        .await?;
        Ok(())
    }

    /// Re-resolve every principal's role from the groups already stored. Called
    /// when the group-mapping table changes, so a demotion takes effect without
    /// waiting for the next sign-in.
    pub async fn identities_for_resync(&self) -> Result<Vec<(i64, String, Vec<String>)>, DbError> {
        let rows = sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, username, groups FROM remote_identities",
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, username, groups)| {
                (
                    id,
                    username,
                    serde_json::from_str(&groups).unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Apply a newly resolved role to one principal and its mirrored account.
    pub async fn set_role(&self, id: i64, username: &str, role_id: i64) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE remote_identities SET role_id = ?, updated_at = ? WHERE id = ?")
            .bind(role_id)
            .bind(&now)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        sqlx::query("UPDATE users SET role_id = ? WHERE username = ? AND auth_source <> 'local'")
            .bind(role_id)
            .bind(username)
            .execute(self.pool.as_ref())
            .await?;
        Ok(())
    }

    /// Enable or disable a principal. A disabled one cannot sign in and its
    /// sessions are gone.
    pub async fn set_disabled(
        &self,
        id: i64,
        username: &str,
        disabled: bool,
    ) -> Result<(), DbError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE remote_identities SET disabled = ?, updated_at = ? WHERE id = ?")
            .bind(disabled)
            .bind(&now)
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        if disabled {
            sqlx::query("DELETE FROM sessions WHERE username = ?")
                .bind(username)
                .execute(self.pool.as_ref())
                .await?;
        }
        Ok(())
    }

    /// Forget a principal: its row, its mirrored account and its sessions. The
    /// account in the directory is untouched — this is "stop trusting what the
    /// provider said", not "delete the person".
    pub async fn delete(&self, id: i64) -> Result<Option<RemoteIdentity>, DbError> {
        let Some(identity) = self.find(id).await? else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM sessions WHERE username = ?")
            .bind(&identity.username)
            .execute(self.pool.as_ref())
            .await?;
        sqlx::query("DELETE FROM remote_identities WHERE id = ?")
            .bind(id)
            .execute(self.pool.as_ref())
            .await?;
        // Only the mirror goes: a local account that happens to share the name
        // is refused above and can never reach this branch.
        sqlx::query("DELETE FROM users WHERE username = ? AND auth_source = 'directory'")
            .bind(&identity.username)
            .execute(self.pool.as_ref())
            .await?;
        Ok(Some(identity))
    }

    /// How many principals hold each role — the screen's summary line.
    pub async fn role_counts(&self) -> Result<(i64, i64), DbError> {
        let admins = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM remote_identities WHERE role_id = ? AND disabled = 0",
        )
        .bind(ADMIN_ROLE_ID)
        .fetch_one(self.pool.as_ref())
        .await?;
        let users = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM remote_identities WHERE role_id = ? AND disabled = 0",
        )
        .bind(USER_ROLE_ID)
        .fetch_one(self.pool.as_ref())
        .await?;
        Ok((admins.0, users.0))
    }
}
