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
}

/// Response containing the total number of users.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCountResponse {
    pub count: i64,
}
