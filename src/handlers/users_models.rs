/* =============================================================================
 * File          : users_models.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Request and response payload types for the user-management endpoints:
 *   create/update/delete, password and role changes, group membership and
 *   home-folder settings. Quota sizes are expressed in bytes (0 or absent
 *   means no limit).
 *
 * Dependencies:
 *   - chrono (>= 0.4)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Imported by handlers/users.rs; mirrored by the frontend ApiClient
 *   types (frontend/src/lib/services/api.ts).
 * =============================================================================
 */

//! Request/response payload types for the user-management endpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Fields that a user (or admin) may update on a user profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserRequest {
    pub fullname: String,
    pub email: String,
}

/// Password change request. `current_password` is optional for admin-initiated changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePasswordRequest {
    pub current_password: Option<String>,
    pub new_password: String,
}

/// Role change request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRoleRequest {
    pub role_id: i64,
}

/// Lock/unlock request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLockedRequest {
    pub locked: bool,
}

/// Full payload for creating a new user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub fullname: String,
    pub email: String,
    pub role_id: i64,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub home_enabled: bool,
    pub home_disk: Option<String>,
    /// Explicit absolute directory for the home folder, chosen via the folder
    /// picker. When set it overrides `home_disk` (which is then derived from
    /// the partition that actually contains this path). Validated server-side
    /// against path traversal and system directories.
    #[serde(default)]
    pub home_path: Option<String>,
    /// Hard disk-quota limit for the home directory, in bytes.
    /// `None` or `0` means no limit.
    #[serde(default)]
    pub home_size_bytes: Option<i64>,
}

/// Admin payload for updating user profile and home-folder settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserFullRequest {
    pub fullname: String,
    pub email: String,
    pub home_enabled: bool,
    pub home_disk: Option<String>,
    /// Explicit absolute directory for the home folder (see `CreateUserRequest`).
    #[serde(default)]
    pub home_path: Option<String>,
    /// Hard disk-quota limit for the home directory, in bytes.
    /// `None` or `0` means no limit.
    #[serde(default)]
    pub home_size_bytes: Option<i64>,
}

/// Group membership sync request — replaces all current groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGroupMembershipRequest {
    pub groups: Vec<String>,
}

/// Toggle home folder on/off and optionally choose a disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetHomeFolderRequest {
    pub enabled: bool,
    pub disk: Option<String>,
    /// Explicit absolute directory chosen via the folder picker; overrides
    /// `disk`. Validated server-side (see `validate_custom_home_path`).
    #[serde(default)]
    pub path: Option<String>,
    /// Hard disk-quota limit for the home directory, in bytes.
    /// `None` or `0` means no limit.
    #[serde(default)]
    pub size_bytes: Option<i64>,
}

/// Username-only body for user deletion (legacy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteUserRequest {
    pub username: String,
}

/// A single user entry in the admin user listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListItem {
    pub username: String,
    pub fullname: String,
    pub email: String,
    pub role: String,
    pub role_id: i64,
    pub locked: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub groups: Vec<String>,
    pub home_enabled: bool,
    pub home_disk: Option<String>,
    pub home_path: Option<String>,
    /// Hard disk-quota limit for the home directory, in bytes (`None` = no limit).
    #[serde(default)]
    pub home_size_bytes: Option<i64>,
    #[serde(default)]
    pub avatar_seed: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub two_fa_enabled: bool,
    /// Admin-set expiry horizon (`None` = never; migration 050). The detail
    /// pane edits it through `PUT /users/{u}/expiry`, never via profile save.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// `true` when the account mirrors a host (Linux/PAM) account rather than
    /// a HomeVault-created one. Its credential lives in `/etc/shadow` and its
    /// Linux user is not HomeVault's to delete — the Users page shows a badge
    /// and disables deletion for it.
    #[serde(default)]
    pub is_host_account: bool,
}

/// Response containing the total number of users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCountResponse {
    pub count: i64,
}

/// One active session of a target user, in the admin vocabulary. The token is
/// MASKED (8 chars + ellipsis): an admin audits and force-logs-out, but never
/// receives a usable credential for someone else's session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSessionInfo {
    /// Masked session token reference (first 8 characters + `…`).
    pub id: String,
    /// IP address the session was created from.
    pub ip_address: String,
    /// Session creation timestamp (RFC 3339).
    pub created_at: String,
    /// Session expiry timestamp (RFC 3339).
    pub expires_at: String,
}

/// Response wrapper for the admin per-user sessions list.
#[derive(Debug, Serialize)]
pub struct AdminUserSessionsResponse {
    pub sessions: Vec<AdminSessionInfo>,
    pub total: usize,
}

/// Body for setting (RFC 3339 timestamp) or clearing (`null` = never) a
/// user's expiry horizon — migration 050, `users.expires_at`.
#[derive(Debug, Clone, Deserialize)]
pub struct SetExpiryRequest {
    /// `None` clears the horizon; `Some` must parse as RFC 3339 UTC.
    #[serde(default)]
    pub expires_at: Option<String>,
}
