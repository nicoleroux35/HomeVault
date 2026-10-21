/* =============================================================================
 * File          : groups.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Unix group management mapped onto the host's group tools: every
 *   HomeVault-managed group carries a reserved prefix to avoid colliding
 *   with system groups; list/create/delete and membership changes shell
 *   out to the standard group utilities.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2)
 *
 * Usage:
 *   handlers/groups.rs and the admin-group sync in auth/service.rs.
 * =============================================================================
 */

//! Unix group management (host-mapped, HomeVault-prefixed group names).

use serde::{Deserialize, Serialize};
use std::process::Command;
use thiserror::Error;

/// Prefix used for all HomeVault-managed system groups to avoid naming collisions.
pub const GROUP_PREFIX: &str = "homevault_";

/// Errors that can occur during group management operations.
#[derive(Error, Debug)]
pub enum GroupError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Group not found: {0}")]
    NotFound(String),
    #[error("User not found: {0}")]
    UserNotFound(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

/// A Unix group with its GID and member list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
}

/// Request payload for creating a new group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub members: Vec<String>,
    /// Optional description stored in the `hv_groups` mirror (migration 008).
    #[serde(default)]
    pub description: Option<String>,
}

/// Request payload for updating an existing group's members.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGroupRequest {
    pub members: Vec<String>,
}

/// Request payload for renaming a group and/or setting its description
/// (2026-10-20). Both fields are optional: a `PUT` may carry only one of them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGroupMetaRequest {
    /// New display name (no `homevault_` prefix; it is added automatically).
    #[serde(default)]
    pub name: Option<String>,
    /// New description; `Some("")` clears it, `None` leaves it untouched.
    #[serde(default)]
    pub description: Option<String>,
}

/// Request payload for adding a single user to a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMemberRequest {
    pub username: String,
}

/// A group summary suitable for display in lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupListItem {
    /// Full system group name (with `homevault_` prefix).
    pub name: String,
    /// Human-readable name (prefix stripped).
    pub display_name: String,
    pub gid: u32,
    pub member_count: usize,
    /// Free-text description from the `hv_groups` mirror (`None` = none set).
    /// The OS has no such field, so this is DB-only and may be absent for a
    /// group created straight on the host.
    #[serde(default)]
    pub description: Option<String>,
}

/// Service for managing HomeVault system groups via `getent`, `groupadd`, `groupdel`, `usermod`.
pub struct GroupService;

impl GroupService {
    /// Return the system group name by prepending `homevault_` if not already present.
    pub fn get_full_group_name(display_name: &str) -> String {
        if display_name.starts_with(GROUP_PREFIX) {
            display_name.to_string()
        } else {
            format!("{}{}", GROUP_PREFIX, display_name)
        }
    }

    /// Strip the `homevault_` prefix to get the human-readable display name.
    pub fn get_display_name(full_name: &str) -> String {
        if let Some(stripped) = full_name.strip_prefix(GROUP_PREFIX) {
            stripped.to_string()
        } else {
            full_name.to_string()
        }
    }

    /// Return `true` if the group name starts with the `homevault_` prefix.
    pub fn is_homevault_group(full_name: &str) -> bool {
        full_name.starts_with(GROUP_PREFIX)
    }

    /// True if `name` (display or full system name) refers to the reserved
    /// admin group — used to keep it out of generic group-management UIs,
    /// since it grants real passwordless sudo on the host.
    pub fn is_admin_group(name: &str) -> bool {
        Self::get_full_group_name(name) == crate::storage::database::ADMIN_GROUP_NAME
    }

    /// List all HomeVault-managed groups from the system's group database.
    pub fn list_groups() -> Result<Vec<GroupListItem>, GroupError> {
        let output = Command::new("getent").args(["group"]).output()?;

        if !output.status.success() {
            return Err(GroupError::CommandFailed(
                "Failed to get groups".to_string(),
            ));
        }

        let mut groups = Vec::new();

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 {
                let full_name = parts[0].to_string();
                let display_name = Self::get_display_name(&full_name);
                let gid: u32 = parts[2].parse().unwrap_or(0);
                let members: Vec<String> = parts[3]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();

                if !Self::is_homevault_group(&full_name) {
                    continue;
                }

                groups.push(GroupListItem {
                    name: full_name,
                    display_name,
                    gid,
                    member_count: members.len(),
                    // Filled in by the handler from the `hv_groups` mirror —
                    // the OS group database has no description column.
                    description: None,
                });
            }
        }

        groups.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        Ok(groups)
    }

    /// Get a single group by name (accepts display name or full system name).
    pub fn get_group(name: &str) -> Result<Group, GroupError> {
        let full_name = Self::get_full_group_name(name);
        let output = Command::new("getent")
            .args(["group", &full_name])
            .output()?;

        if !output.status.success() {
            return Err(GroupError::NotFound(name.to_string()));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().next().unwrap_or("");

        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 4 {
            let gid: u32 = parts[2].parse().unwrap_or(0);
            let members: Vec<String> = parts[3]
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            Ok(Group {
                name: full_name,
                gid,
                members,
            })
        } else {
            Err(GroupError::NotFound(name.to_string()))
        }
    }

    /// Validate a group name meets length, character, and safety constraints.
    pub fn validate_group_name(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("Group name cannot be empty".to_string());
        }

        if name.len() < 2 {
            return Err("Group name must be at least 2 characters".to_string());
        }

        if name.len() > 32 {
            return Err("Group name cannot exceed 32 characters".to_string());
        }

        // Strict whitelist: only alphanumeric, underscore, and hyphen allowed
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(
                "Group name can only contain letters, numbers, underscore and hyphen".to_string(),
            );
        }

        // Block dangerous names
        if name == ".." || name.starts_with('.') || name.starts_with('-') {
            return Err("Invalid group name".to_string());
        }

        Ok(())
    }

    /// Create a new system group. The `homevault_` prefix is added automatically.
    /// The caller should NOT include the prefix in `name`.
    pub fn create_group(name: &str, members: &[String]) -> Result<Group, GroupError> {
        let display_name = name.trim();

        if let Err(e) = Self::validate_group_name(display_name) {
            return Err(GroupError::CommandFailed(e));
        }

        if display_name.starts_with(GROUP_PREFIX) {
            return Err(GroupError::CommandFailed(
                "Do not include the 'homevault_' prefix — it is added automatically".to_string(),
            ));
        }

        let full_name = Self::get_full_group_name(display_name);

        let output = Command::new("getent")
            .args(["group", &full_name])
            .output()?;
        if output.status.success() {
            return Err(GroupError::CommandFailed(format!(
                "Group '{}' already exists",
                display_name
            )));
        }

        let output = Command::new("sudo")
            .arg("groupadd")
            .arg(&full_name)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GroupError::CommandFailed(stderr.to_string()));
        }

        tracing::info!("Created group '{}'", full_name);

        for member in members {
            // Best-effort: the group itself is already created and must not
            // be rolled back over one bad initial member (e.g. one whose
            // Linux account fails to provision). Logged rather than
            // silently discarded, so a member that didn't actually get
            // added is at least discoverable server-side instead of just
            // missing later.
            if let Err(e) = Self::ensure_linux_user(member) {
                tracing::warn!(
                    "Could not provision Linux account for initial member '{}' of new group '{}': {}",
                    member, full_name, e
                );
                continue;
            }
            if let Err(e) = Self::add_user_to_group(member, &full_name) {
                tracing::warn!(
                    "Could not add initial member '{}' to new group '{}': {}",
                    member,
                    full_name,
                    e
                );
            }
        }

        Self::get_group(&full_name)
    }

    /// Delete a system group. The admin group cannot be deleted.
    pub fn delete_group(name: &str) -> Result<(), GroupError> {
        let full_name = Self::get_full_group_name(name);

        if full_name == crate::storage::database::ADMIN_GROUP_NAME {
            return Err(GroupError::CommandFailed(
                "Cannot delete system group".to_string(),
            ));
        }

        let output = Command::new("sudo")
            .arg("groupdel")
            .arg(&full_name)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("does not exist") {
                return Err(GroupError::NotFound(name.to_string()));
            }
            return Err(GroupError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Rename a group at the OS level (`groupmod -n`), display names in,
    /// display names out. The admin group cannot be renamed (it is the sudo
    /// grant), and an existing target name is refused rather than merged.
    ///
    /// Callers are responsible for migrating the references a rename breaks —
    /// the `hv_groups` mirror, `resource_group_shares`, `transfer_quotas` and
    /// `share_permissions` all key on the display name. `handle_group_update`
    /// does exactly that; never call this without it.
    pub fn rename_group(old_name: &str, new_name: &str) -> Result<Group, GroupError> {
        let old_display = Self::get_display_name(old_name.trim());
        let new_display = new_name.trim().to_string();

        if let Err(e) = Self::validate_group_name(&new_display) {
            return Err(GroupError::CommandFailed(e));
        }
        if new_display.starts_with(GROUP_PREFIX) {
            return Err(GroupError::CommandFailed(
                "Do not include the 'homevault_' prefix — it is added automatically".to_string(),
            ));
        }
        if Self::is_admin_group(&old_display) || Self::is_admin_group(&new_display) {
            return Err(GroupError::CommandFailed(
                "The admin group cannot be renamed".to_string(),
            ));
        }
        if old_display == new_display {
            return Self::get_group(&new_display);
        }

        let old_full = Self::get_full_group_name(&old_display);
        let new_full = Self::get_full_group_name(&new_display);

        // Source must exist...
        if !Command::new("getent")
            .args(["group", &old_full])
            .output()?
            .status
            .success()
        {
            return Err(GroupError::NotFound(old_display));
        }
        // ...and the target must not.
        if Command::new("getent")
            .args(["group", &new_full])
            .output()?
            .status
            .success()
        {
            return Err(GroupError::CommandFailed(format!(
                "Group '{}' already exists",
                new_display
            )));
        }

        let output = Command::new("sudo")
            .arg("groupmod")
            .arg("-n")
            .arg(&new_full)
            .arg(&old_full)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GroupError::CommandFailed(stderr.to_string()));
        }

        tracing::info!("Renamed group '{}' to '{}'", old_full, new_full);
        Self::get_group(&new_full)
    }

    /// Ensure `username` has a real Linux account, provisioning a bare one
    /// on demand if it doesn't. Most HomeVault accounts are DB-only (no
    /// Linux user at all) until promoted to admin or given a home folder,
    /// but `add_user_to_group`/`remove_user_from_group` below shell out to
    /// `usermod`/`gpasswd`, which require one to exist — confirmed live
    /// that adding a DB-only user to a group otherwise fails outright with
    /// "user does not exist" and no way to fix it short of promoting them
    /// to admin (which also grants real sudo, wrong for an ordinary group).
    /// The provisioned account gets no home directory and no login shell
    /// (`useradd -M -s /usr/sbin/nologin`, matching the sudoers grant this
    /// requires) and no group membership beyond what the caller adds
    /// afterward — this only ever creates the account, never touches one
    /// that already exists.
    pub fn ensure_linux_user(username: &str) -> Result<(), GroupError> {
        if let Err(e) = crate::setup::SetupService::validate_username(username) {
            return Err(GroupError::CommandFailed(e));
        }

        let exists = Command::new("id")
            .arg(username)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if exists {
            return Ok(());
        }

        let output = Command::new("sudo")
            .arg("useradd")
            .args(["-M", "-s", "/usr/sbin/nologin", username])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GroupError::CommandFailed(stderr.to_string()));
        }

        tracing::info!(
            "Provisioned Linux account '{}' for group management",
            username
        );
        Ok(())
    }

    /// Add a user to a group via `usermod -a -G`.
    pub fn add_user_to_group(username: &str, groupname: &str) -> Result<(), GroupError> {
        let full_group_name = Self::get_full_group_name(groupname);
        let output = Command::new("sudo")
            .arg("usermod")
            .args(["-a", "-G", &full_group_name, username])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Real `usermod` (util-linux) wording is "does not exist", not
            // "doesn't exist" — the original check here never matched, so
            // every failure (including the common "this HomeVault account has
            // no real Linux user" case) silently fell through to the generic
            // `CommandFailed` variant instead of the specific one callers can
            // act on.
            let missing = stderr.contains("does not exist") || stderr.contains("doesn't exist");
            if missing && stderr.contains("group") {
                return Err(GroupError::NotFound(groupname.to_string()));
            }
            if missing {
                return Err(GroupError::UserNotFound(username.to_string()));
            }
            return Err(GroupError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Remove a user from a group via `gpasswd -d`. Returns `Ok` if the user is not a member.
    pub fn remove_user_from_group(username: &str, groupname: &str) -> Result<(), GroupError> {
        let full_group_name = Self::get_full_group_name(groupname);
        let output = Command::new("sudo")
            .arg("gpasswd")
            .args(["-d", username, &full_group_name])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("not a member") {
                return Ok(());
            }
            return Err(GroupError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Return the display names of all HomeVault-managed groups a user
    /// belongs to.
    ///
    /// `id -nG` reports *every* group the account is in, not just the ones
    /// this app manages — including the same-named private group `useradd`
    /// creates for every account by default (Debian/Ubuntu's
    /// `USERGROUPS_ENAB`), and any other incidental Linux group. Confirmed
    /// live: adding a user to exactly one `homevault_` group made this
    /// return two entries, inflating the Users/Groups pages' "N group(s)"
    /// count by one for every account. Filtered to `homevault_`-prefixed
    /// names only, matching `list_groups()`'s own scope.
    pub fn get_user_groups(username: &str) -> Result<Vec<String>, GroupError> {
        let output = Command::new("id").args(["-nG", username]).output()?;

        if !output.status.success() {
            return Err(GroupError::UserNotFound(username.to_string()));
        }

        let groups: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter(|name| Self::is_homevault_group(name))
            .map(Self::get_display_name)
            .collect();

        Ok(groups)
    }

    /// Check if a user is a member of a group.
    pub fn is_member(username: &str, groupname: &str) -> Result<bool, GroupError> {
        let full_group_name = Self::get_full_group_name(groupname);
        let output = Command::new("groups").arg(username).output()?;

        if !output.status.success() {
            return Err(GroupError::UserNotFound(username.to_string()));
        }

        let groups_output = String::from_utf8_lossy(&output.stdout);
        Ok(groups_output.contains(&full_group_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_homevault_group_prefixed() {
        assert!(GroupService::is_homevault_group("homevault_developers"));
        assert!(GroupService::is_homevault_group("homevault_admin"));
    }

    #[test]
    fn test_is_homevault_group_rejects_non_prefixed() {
        // The bug this guards against: `id -nG <user>` also reports the
        // account's own auto-created private group (same name as the
        // username) and any other incidental system group — neither should
        // ever be counted as a HomeVault-managed membership.
        assert!(!GroupService::is_homevault_group("someuser"));
        assert!(!GroupService::is_homevault_group("sudo"));
        assert!(!GroupService::is_homevault_group("docker"));
    }

    #[test]
    fn test_get_display_name_strips_prefix_only_when_present() {
        assert_eq!(
            GroupService::get_display_name("homevault_developers"),
            "developers"
        );
        assert_eq!(GroupService::get_display_name("developers"), "developers");
    }

    #[test]
    fn test_get_full_group_name_is_idempotent() {
        assert_eq!(
            GroupService::get_full_group_name("developers"),
            "homevault_developers"
        );
        assert_eq!(
            GroupService::get_full_group_name("homevault_developers"),
            "homevault_developers"
        );
    }

    #[test]
    fn test_user_groups_filter_excludes_private_and_system_groups() {
        // Mirrors get_user_groups()'s own filter chain against a fabricated
        // `id -nG` line, without shelling out to a real `id` — verifies the
        // fix in isolation: a user in exactly one HomeVault group plus their
        // own private group and an incidental system group must report
        // exactly one membership, not three.
        let raw = "alice sudo homevault_developers docker";
        let groups: Vec<String> = raw
            .split_whitespace()
            .filter(|name| GroupService::is_homevault_group(name))
            .map(GroupService::get_display_name)
            .collect();
        assert_eq!(groups, vec!["developers".to_string()]);
    }
}
