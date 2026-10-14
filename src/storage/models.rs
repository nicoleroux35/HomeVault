/* =============================================================================
 * File          : models.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Shared storage-layer data models: users, sessions, login requests and
 *   responses, and the role constants (user/admin) every auth path uses.
 *
 * Dependencies:
 *   - chrono (>= 0.4), serde (>= 1.0), sqlx (>= 0.9)
 *
 * Usage:
 *   Re-exported through auth/models.rs; used by the auth service and user
 *   repositories.
 * =============================================================================
 */

//! Shared storage-layer data models (users, sessions, login types).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Role ID for a regular (non-admin) user.
pub const USER_ROLE_ID: i64 = 1;
/// Role ID for an administrator.
pub const ADMIN_ROLE_ID: i64 = 2;

/// A registered user in the HomeVault system.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    /// Unique login name.
    pub username: String,
    /// Argon2 hashed password.
    pub password_hash: String,
    /// Display name shown in the UI.
    pub fullname: String,
    /// Email address for notifications and password recovery.
    pub email: String,
    /// References `roles.id` (1 = user, 2 = admin).
    pub role_id: i64,
    /// Whether the account is locked due to failed attempts or admin action.
    pub locked: bool,
    /// Account creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Seed for DiceBear avatar generation (deterministic SVG).
    #[serde(default)]
    pub avatar_seed: String,
}

impl User {
    /// Returns `true` if this user has the admin role.
    pub fn is_admin(&self) -> bool {
        self.role_id == ADMIN_ROLE_ID
    }

    /// Human-readable role name: `"admin"` or `"user"`.
    pub fn role_name(&self) -> &'static str {
        match self.role_id {
            2 => "admin",
            _ => "user",
        }
    }
}

/// An active user session with CSRF protection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Random bearer token identifying the session.
    pub token: String,
    /// Owning user's username.
    pub username: String,
    /// IP address from which the session was created.
    pub ip_address: String,
    /// Token used to protect state-changing requests from CSRF.
    pub csrf_token: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session expires (auto-logout).
    pub expires_at: DateTime<Utc>,
    /// Cached admin flag (avoids a DB lookup on every request).
    pub is_admin: bool,
}

/// Database-level row that mirrors the `sessions` table.
/// Converted to [`Session`] for external use.
impl From<SessionRow> for Session {
    fn from(row: SessionRow) -> Self {
        Self {
            token: row.token,
            username: row.username,
            ip_address: row.ip_address,
            csrf_token: row.csrf_token,
            created_at: row.created_at,
            expires_at: row.expires_at,
            is_admin: row.is_admin,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct SessionRow {
    pub token: String,
    pub username: String,
    pub ip_address: String,
    pub csrf_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub is_admin: bool,
}

/// A dashboard widget tile with position and size on the grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Widget {
    pub id: String,
    pub wtype: String,
    pub title: String,
    pub size: String,
    pub row_pos: u32,
    pub col_pos: u32,
    pub row_span: u32,
    pub col_span: u32,
    pub username: Option<String>,
}

impl From<WidgetRow> for Widget {
    fn from(row: WidgetRow) -> Self {
        Self {
            id: row.id,
            wtype: row.wtype,
            title: row.title,
            size: row.size,
            row_pos: row.row_pos,
            col_pos: row.col_pos,
            row_span: row.row_span,
            col_span: row.col_span,
            username: row.username,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct WidgetRow {
    pub id: String,
    pub wtype: String,
    pub title: String,
    pub size: String,
    pub row_pos: u32,
    pub col_pos: u32,
    pub row_span: u32,
    pub col_span: u32,
    pub username: Option<String>,
}

/// Payload for creating a new user via the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub fullname: String,
    pub email: String,
    pub role_id: i64,
}

/// Credentials for user login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// When true, the session is issued a long-lived expiry ("remember me")
    /// instead of the default short-lived one.
    #[serde(default)]
    pub remember_me: bool,
}

/// Response returned after successful authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub csrf_token: String,
    pub username: String,
    pub is_admin: bool,
}

/// Payload to verify a TOTP code during two-factor login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoFactorVerifyRequest {
    pub username: String,
    pub password: String,
    pub totp_code: String,
    /// When true, the session is issued a long-lived expiry ("remember me")
    /// instead of the default short-lived one.
    #[serde(default)]
    pub remember_me: bool,
}

/// Payload for enabling or disabling two-factor authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoFactorSetupRequest {
    pub action: String,
    pub totp_code: Option<String>,
    pub password: Option<String>,
}

/// Contains the TOTP secret and QR code URL for 2FA enrollment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoFactorSetupResponse {
    pub secret: String,
    pub qrcode_url: String,
}

/// First-run setup payload to create the initial admin account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub fullname: Option<String>,
    #[serde(default)]
    pub avatar_seed: Option<String>,
    /// Optional TOTP enrollment done at the Setup Wizard's password step: the
    /// secret is generated client-side and the 6-digit code is validated by
    /// the server both before the account is created and again (against this
    /// secret) inside `handle_setup`. Both fields must be present together;
    /// a lone secret is ignored.
    #[serde(default)]
    pub totp_secret: Option<String>,
    #[serde(default)]
    pub totp_code: Option<String>,
}

/// Indicates whether the system has been set up and if hardware has changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatusResponse {
    pub is_first_run: bool,
    pub is_setup_valid: bool,
    pub hardware_changed: bool,
    /// True when an admin exists but the recorded hardware fingerprint no
    /// longer matches this host — the frontend shows the dedicated recovery
    /// screen (admin confirmation + re-baseline), never the Setup Wizard.
    #[serde(default)]
    pub needs_recovery: bool,
    pub server_machine_id: String,
    /// Whether `sudo` is available on the host (required for terminal access).
    #[serde(default)]
    pub sudo_available: bool,
}
