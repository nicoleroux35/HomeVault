use std::sync::Arc;
use thiserror::Error;

use crate::acl::{AclEntry, AclError, AclPermissions, AclService, AclTag};
use crate::groups::GroupService;
use crate::storage::share_perm_repo::{SharePermission, SharePermissionRepository};

/// Errors that can occur during permission management.
#[derive(Error, Debug)]
pub enum PermissionError {
    #[error("ACL error: {0}")]
    Acl(#[from] AclError),
    #[error("Share error: {0}")]
    Share(String),
    #[error("Invalid permission: {0}")]
    InvalidPermission(String),
    #[error("Not found: {0}")]
    NotFound(String),
}

/// Service for managing share-level permissions backed by POSIX ACLs.
pub struct PermissionService;

impl PermissionService {
    /// Parse a human-readable permission string into an `AclPermissions` struct.
    pub fn parse_permission(perm: &str) -> Result<AclPermissions, PermissionError> {
        match perm.to_lowercase().as_str() {
            "r" | "read" => Ok(AclPermissions {
                read: true,
                write: false,
                execute: false,
            }),
            "w" | "write" => Ok(AclPermissions {
                read: false,
                write: true,
                execute: false,
            }),
            "x" | "execute" => Ok(AclPermissions {
                read: false,
                write: false,
                execute: true,
            }),
            "rw" | "wr" | "readwrite" => Ok(AclPermissions {
                read: true,
                write: true,
                execute: false,
            }),
            "rx" | "xr" => Ok(AclPermissions {
                read: true,
                write: false,
                execute: true,
            }),
            "wx" | "xw" | "writeexecute" => Ok(AclPermissions {
                read: false,
                write: true,
                execute: true,
            }),
            "rwx" | "rxw" | "all" | "full" => Ok(AclPermissions {
                read: true,
                write: true,
                execute: true,
            }),
            _ => Err(PermissionError::InvalidPermission(perm.to_string())),
        }
    }

    /// Convert `AclPermissions` back to a short string like `"rw"` or `"rwx"`.
    pub fn permission_to_string(perms: &AclPermissions) -> String {
        match (perms.read, perms.write, perms.execute) {
            (true, true, true) => "rwx".to_string(),
            (true, true, false) => "rw".to_string(),
            (true, false, true) => "rx".to_string(),
            (true, false, false) => "r".to_string(),
            (false, true, true) => "wx".to_string(),
            (false, true, false) => "w".to_string(),
            (false, false, true) => "x".to_string(),
            (false, false, false) => "".to_string(),
        }
    }

    /// Return `true` if the string is a valid permission expression.
    pub fn validate_permission_string(perm: &str) -> bool {
        matches!(
            perm.to_lowercase().as_str(),
            "r" | "read"
                | "w"
                | "write"
                | "x"
                | "execute"
                | "rw"
                | "wr"
                | "readwrite"
                | "rx"
                | "xr"
                | "wx"
                | "xw"
                | "rwx"
                | "rxw"
                | "all"
                | "full"
        )
    }

    /// Apply all stored share permissions from the database as POSIX ACLs on the share path.
    pub async fn apply_share_permissions(
        share_name: &str,
        share_type: &str,
        share_path: &str,
        pool: Arc<sqlx::SqlitePool>,
    ) -> Result<(), PermissionError> {
        let repo = SharePermissionRepository::new(Arc::clone(&pool));
        let perms = repo
            .get_for_share(share_name, share_type)
            .await
            .map_err(|e| PermissionError::Share(e.to_string()))?;

        for perm in &perms {
            Self::apply_permission_to_path(share_path, perm)?;
        }

        tracing::info!(
            "Applied {} permissions for share '{}' ({})",
            perms.len(),
            share_name,
            share_type
        );
        Ok(())
    }

    /// Apply a single `SharePermission` as a POSIX ACL entry on the filesystem.
    pub fn apply_permission_to_path(
        share_path: &str,
        perm: &SharePermission,
    ) -> Result<(), PermissionError> {
        let (tag, qualifier) = if perm.grantee_type == "group" {
            (
                AclTag::Group,
                Some(GroupService::get_full_group_name(&perm.grantee_name)),
            )
        } else {
            (AclTag::User, Some(perm.grantee_name.clone()))
        };

        let mut permissions = Self::parse_permission(&perm.permission)?;

        // A share path is always a directory, and a directory's read/write
        // bits are meaningless without execute (traversal) — nothing inside
        // is reachable at all without it, regardless of read/write. Force it
        // on so an "r" or "rw" grant is actually usable instead of silently
        // granting access to a directory nothing can enter. This only
        // affects the ACL actually applied to the filesystem; the persisted
        // `perm.permission` string (what the UI displays as chosen) is
        // untouched.
        if std::path::Path::new(share_path).is_dir() {
            permissions.execute = true;
        }

        let acl_entry = AclEntry {
            tag,
            qualifier,
            permissions,
        };

        if perm.recursive {
            AclService::set_acl(share_path, std::slice::from_ref(&acl_entry))?;
            AclService::set_default_acl(share_path, &[acl_entry])?;
        } else {
            AclService::set_acl(share_path, &[acl_entry])?;
        }

        Ok(())
    }

    /// Remove all ACL entries on a share path.
    pub fn revoke_all_permissions(share_path: &str) -> Result<(), PermissionError> {
        AclService::remove_all_acl(share_path)?;
        tracing::info!("Revoked all ACLs for share path: {}", share_path);
        Ok(())
    }

    /// Remove ACL entries for a specific user or group from a share path.
    pub fn revoke_grantee_permissions(
        share_path: &str,
        grantee_type: &str,
        grantee_name: &str,
    ) -> Result<(), PermissionError> {
        let tag = if grantee_type == "group" {
            AclTag::Group
        } else {
            AclTag::User
        };

        let qualifier = if grantee_type == "group" {
            Some(GroupService::get_full_group_name(grantee_name))
        } else {
            Some(grantee_name.to_string())
        };

        AclService::remove_acl(share_path, &tag, qualifier.as_deref())?;
        Ok(())
    }

    /// List current ACL entries on a share path.
    pub fn get_share_permissions(share_path: &str) -> Result<Vec<AclEntry>, PermissionError> {
        let file_acl = AclService::get_acl(share_path)?;
        Ok(file_acl.entries)
    }

    /// Revoke all then re-apply all DB-stored permissions for a share (full sync).
    pub async fn sync_share_permissions(
        share_name: &str,
        share_type: &str,
        share_path: &str,
        pool: Arc<sqlx::SqlitePool>,
    ) -> Result<(), PermissionError> {
        Self::revoke_all_permissions(share_path)?;
        Self::apply_share_permissions(share_name, share_type, share_path, pool).await?;
        tracing::info!(
            "Synced permissions for share '{}' ({})",
            share_name,
            share_type
        );
        Ok(())
    }

    /// Grant a permission to a user or group on a share, persisting it and applying the ACL.
    ///
    /// Validates the permission string, grantee type, grantee existence, and share path
    /// before writing to the database and filesystem.
    #[allow(clippy::too_many_arguments)]
    pub async fn grant_permission(
        share_name: &str,
        share_type: &str,
        share_path: &str,
        grantee_type: &str,
        grantee_name: &str,
        permission: &str,
        recursive: bool,
        pool: Arc<sqlx::SqlitePool>,
    ) -> Result<SharePermission, PermissionError> {
        // Validate the permission string
        if !Self::validate_permission_string(permission) {
            return Err(PermissionError::InvalidPermission(permission.to_string()));
        }

        // Validate grantee type
        if grantee_type != "user" && grantee_type != "group" {
            return Err(PermissionError::InvalidPermission(format!(
                "Invalid grantee_type: {}. Must be 'user' or 'group'",
                grantee_type
            )));
        }

        // Verify the grantee actually exists to prevent orphaned permissions
        if grantee_type == "user" {
            use crate::storage::UserRepository;
            let user_repo = UserRepository::new(Arc::clone(&pool));
            match user_repo.find_by_username(grantee_name).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(PermissionError::NotFound(format!(
                        "User '{}' not found",
                        grantee_name
                    )));
                }
                Err(e) => {
                    return Err(PermissionError::Share(format!(
                        "Failed to verify user existence: {}",
                        e
                    )));
                }
            }
        } else if grantee_type == "group" && GroupService::get_group(grantee_name).is_err() {
            return Err(PermissionError::NotFound(format!(
                "Group '{}' not found",
                grantee_name
            )));
        }

        // Validate share path exists on disk
        if !std::path::Path::new(share_path).exists() {
            return Err(PermissionError::NotFound(format!(
                "Share path '{}' does not exist",
                share_path
            )));
        }

        // Apply the ACL to the real filesystem path *before* persisting to the
        // database, so a failure here (e.g. the path guard rejecting a share
        // outside the allowed directories) never leaves a "granted" row in
        // the database for a permission that was never actually applied.
        let pending = SharePermission {
            id: 0,
            share_name: share_name.to_string(),
            share_type: share_type.to_string(),
            grantee_type: grantee_type.to_string(),
            grantee_name: grantee_name.to_string(),
            permission: permission.to_string(),
            recursive,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        Self::apply_permission_to_path(share_path, &pending)?;

        let repo = SharePermissionRepository::new(Arc::clone(&pool));
        let perm = repo
            .upsert(
                share_name,
                share_type,
                grantee_type,
                grantee_name,
                permission,
                recursive,
            )
            .await
            .map_err(|e| PermissionError::Share(e.to_string()))?;

        tracing::info!(
            "Granted {} permission on '{}' ({}) to {}:{}",
            permission,
            share_name,
            share_type,
            grantee_type,
            grantee_name
        );

        Ok(perm)
    }

    /// Revoke a specific user/group permission from a share, removing it from both DB and filesystem.
    pub async fn revoke_permission(
        share_name: &str,
        share_type: &str,
        share_path: &str,
        grantee_type: &str,
        grantee_name: &str,
        pool: Arc<sqlx::SqlitePool>,
    ) -> Result<(), PermissionError> {
        // Best-effort: remove the ACL from the real filesystem path first.
        // A failure here (e.g. the path no longer exists, or a stale row from
        // before the grant path-guard fix) must not block deleting the
        // database row — otherwise a stale permission could never be cleaned
        // up from the UI. It's still attempted first so the common case
        // (path exists, ACL present) actually clears the real ACL.
        if let Err(e) = Self::revoke_grantee_permissions(share_path, grantee_type, grantee_name) {
            tracing::warn!(
                "Failed to remove filesystem ACL for {}:{} on '{}' ({}): {} — deleting database record anyway",
                grantee_type, grantee_name, share_name, share_type, e
            );
        }

        let repo = SharePermissionRepository::new(Arc::clone(&pool));
        let deleted = repo
            .delete(share_name, share_type, grantee_type, grantee_name)
            .await
            .map_err(|e| PermissionError::Share(e.to_string()))?;

        if deleted {
            tracing::info!(
                "Revoked permission on '{}' ({}) from {}:{}",
                share_name,
                share_type,
                grantee_type,
                grantee_name
            );
        }

        Ok(())
    }

    /// List all DB-stored permissions for a share.
    pub async fn list_share_permissions(
        share_name: &str,
        share_type: &str,
        pool: Arc<sqlx::SqlitePool>,
    ) -> Result<Vec<SharePermission>, PermissionError> {
        let repo = SharePermissionRepository::new(Arc::clone(&pool));
        repo.get_for_share(share_name, share_type)
            .await
            .map_err(|e| PermissionError::Share(e.to_string()))
    }

    /// Return all permissions for a user, including those inherited via group membership.
    pub async fn get_user_permissions(
        username: &str,
        pool: Arc<sqlx::SqlitePool>,
    ) -> Result<Vec<SharePermission>, PermissionError> {
        let repo = SharePermissionRepository::new(Arc::clone(&pool));

        let mut all_perms = repo
            .get_for_grantee("user", username)
            .await
            .map_err(|e| PermissionError::Share(e.to_string()))?;

        let groups = GroupService::get_user_groups(username)
            .map_err(|e| PermissionError::Share(e.to_string()))?;

        for group_name in groups {
            let group_perms = repo
                .get_for_grantee("group", &group_name)
                .await
                .map_err(|e| PermissionError::Share(e.to_string()))?;
            all_perms.extend(group_perms);
        }

        Ok(all_perms)
    }
}

#[cfg(test)]
mod tests {
    //! Pure-logic tests for the permission-string parser/formatter/validator.
    //! No DB, no filesystem, no ACL — those paths need the real host and are
    //! covered by `tests/e2e/shares/shares.spec.ts`.

    use super::*;

    #[test]
    fn test_parse_permission_read_variants() {
        for s in ["r", "R", "read", "READ"] {
            let p = PermissionService::parse_permission(s).unwrap();
            assert_eq!(
                (p.read, p.write, p.execute),
                (true, false, false),
                "input {s:?}"
            );
        }
    }

    #[test]
    fn test_parse_permission_write_variants() {
        for s in ["w", "write"] {
            let p = PermissionService::parse_permission(s).unwrap();
            assert_eq!(
                (p.read, p.write, p.execute),
                (false, true, false),
                "input {s:?}"
            );
        }
    }

    #[test]
    fn test_parse_permission_execute_variants() {
        for s in ["x", "execute"] {
            let p = PermissionService::parse_permission(s).unwrap();
            assert_eq!(
                (p.read, p.write, p.execute),
                (false, false, true),
                "input {s:?}"
            );
        }
    }

    #[test]
    fn test_parse_permission_readwrite_variants() {
        for s in ["rw", "wr", "readwrite", "RW"] {
            let p = PermissionService::parse_permission(s).unwrap();
            assert_eq!(
                (p.read, p.write, p.execute),
                (true, true, false),
                "input {s:?}"
            );
        }
    }

    #[test]
    fn test_parse_permission_full_variants() {
        for s in ["rwx", "rxw", "all", "full", "RWX"] {
            let p = PermissionService::parse_permission(s).unwrap();
            assert_eq!(
                (p.read, p.write, p.execute),
                (true, true, true),
                "input {s:?}"
            );
        }
    }

    #[test]
    fn test_parse_permission_rx_and_wx() {
        let rx = PermissionService::parse_permission("rx").unwrap();
        assert_eq!((rx.read, rx.write, rx.execute), (true, false, true));
        let wx = PermissionService::parse_permission("wx").unwrap();
        assert_eq!((wx.read, wx.write, wx.execute), (false, true, true));
    }

    #[test]
    fn test_parse_permission_rejects_garbage() {
        for s in ["", "rwxr", "z", "7", "rwrw", "read-write", " r "] {
            assert!(
                PermissionService::parse_permission(s).is_err(),
                "{s:?} must not parse as a permission"
            );
        }
    }

    #[test]
    fn test_permission_to_string_round_trips_every_bit_combo() {
        // Every (read, write, execute) triple must format to a string that
        // parses back to the same triple (except the all-false case, which
        // is "" and is intentionally not a valid input).
        for read in [false, true] {
            for write in [false, true] {
                for execute in [false, true] {
                    let perms = AclPermissions {
                        read,
                        write,
                        execute,
                    };
                    let s = PermissionService::permission_to_string(&perms);
                    if !read && !write && !execute {
                        assert_eq!(s, "");
                        continue;
                    }
                    let back = PermissionService::parse_permission(&s).unwrap();
                    assert_eq!(
                        (back.read, back.write, back.execute),
                        (read, write, execute),
                        "round-trip failed for {s:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_validate_permission_string_matches_the_grant_handler_contract() {
        // The grant handler advertises "r, rw, rwx" but the validator is
        // broader — pin down exactly what it accepts so a future edit that
        // narrows or widens it is caught.
        for ok in [
            "r",
            "read",
            "w",
            "write",
            "x",
            "execute",
            "rw",
            "wr",
            "readwrite",
            "rx",
            "xr",
            "wx",
            "xw",
            "rwx",
            "rxw",
            "all",
            "full",
            "RWX",
            "Read",
        ] {
            assert!(
                PermissionService::validate_permission_string(ok),
                "{ok:?} should be valid"
            );
        }
        for bad in ["", " ", "rwxx", "z", "0755", "r-x", "rwt"] {
            assert!(
                !PermissionService::validate_permission_string(bad),
                "{bad:?} should be invalid"
            );
        }
    }

    #[test]
    fn test_validate_and_parse_agree() {
        // Anything the validator accepts must also parse, and vice-versa —
        // the grant path relies on this (it validates, then later parses).
        for s in [
            "r", "w", "x", "rw", "rx", "wx", "rwx", "read", "write", "execute", "all", "full",
            "garbage", "", "rwxr", "z",
        ] {
            assert_eq!(
                PermissionService::validate_permission_string(s),
                PermissionService::parse_permission(s).is_ok(),
                "validator and parser disagree on {s:?}"
            );
        }
    }
}
