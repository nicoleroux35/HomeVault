use super::models::{Session, User};
use crate::storage::database::ADMIN_GROUP_NAME;
use crate::storage::models::{ADMIN_ROLE_ID, USER_ROLE_ID};
use crate::storage::{
    Database, LoginAttemptRepository, PasswordHistoryRepository, PasswordRecoveryCodeRepository,
    SessionRepository, User2faRepository, UserRepository,
};
use chrono::{Duration, Utc};
use std::sync::Arc;
use thiserror::Error;

const MAX_USERS: usize = 100;
const SESSION_LIFETIME_SECONDS: i64 = 86400;
/// Session lifetime granted when the user checks "remember me" at login.
const REMEMBER_ME_SESSION_LIFETIME_SECONDS: i64 = 30 * 86400;
const MAX_LOGIN_ATTEMPTS: u32 = 5;
const LOCKOUT_DURATION_SECONDS: i64 = 300;
const PASSWORD_HISTORY_SIZE: usize = 5;
const MIN_PASSWORD_LENGTH: usize = 8;
/// Number of 4-digit groups in a generated recovery code. 8 groups = 32 digits
/// ≈ 106 bits of entropy (log2(10^32)), comfortably above the 96-bit target.
const RECOVERY_CODE_GROUPS: usize = 8;
/// Failed recovery-code attempts allowed within [`RECOVERY_CODE_WINDOW_SECONDS`]
/// before further attempts are refused. Brute-forcing a 106-bit code is
/// infeasible regardless; this is only defence-in-depth against online guessing.
const MAX_RECOVERY_CODE_ATTEMPTS: i64 = 5;
const RECOVERY_CODE_WINDOW_SECONDS: i64 = 900;
/// Placeholder `password_hash` for host/PAM accounts mirrored into `users`
/// (see [`AuthService::ensure_pam_user_row`]). Never a valid Argon2 string.
const PAM_PROVISIONED_HASH: &str = "!pam-provisioned";

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("User not found")]
    UserNotFound,
    #[error("User already exists")]
    UserAlreadyExists,
    #[error("Invalid password")]
    InvalidPassword,
    #[error("User locked")]
    UserLocked,
    #[error("Session not found")]
    SessionNotFound,
    #[error("Session expired")]
    SessionExpired,
    #[error("Database error: {0}")]
    Database(String),
    #[error("Limit exceeded")]
    LimitExceeded,
    #[error("Password hashing error: {0}")]
    HashingError(String),
    #[error("Too many login attempts")]
    TooManyAttempts,
    #[error("Account temporarily locked")]
    AccountLocked,
    #[error("Password does not meet requirements")]
    WeakPassword,
    #[error("Password was used recently")]
    PasswordReused,
    #[error("2FA code required")]
    TwoFactorRequired,
    #[error("Invalid 2FA code")]
    InvalidTwoFactorCode,
    #[error("Invalid role: must be 1 (user) or 2 (admin)")]
    InvalidRole,
    #[error("Invalid recovery code")]
    InvalidRecoveryCode,
    #[error("Too many recovery attempts")]
    TooManyRecoveryAttempts,
}

impl From<crate::storage::DbError> for AuthError {
    fn from(err: crate::storage::DbError) -> Self {
        AuthError::Database(err.to_string())
    }
}

impl From<sqlx::Error> for AuthError {
    fn from(err: sqlx::Error) -> Self {
        AuthError::Database(err.to_string())
    }
}

#[derive(Clone)]
pub struct AuthService {
    pool: Arc<sqlx::SqlitePool>,
}

impl AuthService {
    pub fn new(db: Database) -> Self {
        Self { pool: db.pool() }
    }

    pub async fn init(&self) -> Result<(), AuthError> {
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));
        session_repo.delete_expired().await?;

        let login_attempt_repo = LoginAttemptRepository::new(Arc::clone(&self.pool));
        login_attempt_repo.prune_old_entries(7).await?;

        Ok(())
    }

    pub async fn close(&self) -> Result<(), AuthError> {
        Ok(())
    }

    async fn check_rate_limit(&self, username: &str) -> Result<(), AuthError> {
        let login_attempt_repo = LoginAttemptRepository::new(Arc::clone(&self.pool));
        let count = login_attempt_repo
            .get_attempts_count(username, LOCKOUT_DURATION_SECONDS)
            .await?;

        if count >= MAX_LOGIN_ATTEMPTS as i64 {
            return Err(AuthError::TooManyAttempts);
        }

        Ok(())
    }

    async fn record_failed_attempt(
        &self,
        username: &str,
        ip_address: &str,
        user_agent: Option<&str>,
    ) -> Result<(), AuthError> {
        let login_attempt_repo = LoginAttemptRepository::new(Arc::clone(&self.pool));
        login_attempt_repo
            .record_attempt(username, ip_address, false, user_agent)
            .await?;

        let count = login_attempt_repo
            .get_attempts_count(username, LOCKOUT_DURATION_SECONDS)
            .await?;
        if count >= MAX_LOGIN_ATTEMPTS as i64 {
            let user_repo = UserRepository::new(Arc::clone(&self.pool));
            if let Ok(Some(mut user)) = user_repo.find_by_username(username).await {
                user.locked = true;
                let _ = user_repo.update(&user).await;
            }
            return Err(AuthError::AccountLocked);
        }

        Ok(())
    }

    async fn clear_failed_attempts(&self, username: &str) -> Result<(), AuthError> {
        let login_attempt_repo = LoginAttemptRepository::new(Arc::clone(&self.pool));
        login_attempt_repo.clear_attempts(username).await?;
        Ok(())
    }

    /// Record a successful login for the audit history. Failures are tracked
    /// by [`Self::authenticate`]; this is called from the login handlers once
    /// a session has actually been issued.
    pub async fn record_successful_attempt(
        &self,
        username: &str,
        ip_address: &str,
        user_agent: Option<&str>,
    ) -> Result<(), AuthError> {
        let login_attempt_repo = LoginAttemptRepository::new(Arc::clone(&self.pool));
        login_attempt_repo
            .record_attempt(username, ip_address, true, user_agent)
            .await?;
        Ok(())
    }

    fn validate_password_strength(&self, password: &str) -> Result<(), AuthError> {
        if password.len() < MIN_PASSWORD_LENGTH {
            return Err(AuthError::WeakPassword);
        }

        let has_uppercase = password.chars().any(|c| c.is_uppercase());
        let has_lowercase = password.chars().any(|c| c.is_lowercase());
        let has_digit = password.chars().any(|c| c.is_ascii_digit());

        if !has_uppercase || !has_lowercase || !has_digit {
            return Err(AuthError::WeakPassword);
        }

        Ok(())
    }

    async fn check_password_history(
        &self,
        password: &str,
        username: &str,
    ) -> Result<(), AuthError> {
        let password_history_repo = PasswordHistoryRepository::new(Arc::clone(&self.pool));
        let history = password_history_repo
            .get_history(username, PASSWORD_HISTORY_SIZE)
            .await?;

        for old_hash in &history {
            if self.verify_password(password, old_hash) {
                return Err(AuthError::PasswordReused);
            }
        }

        Ok(())
    }

    async fn add_password_to_history(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<(), AuthError> {
        let password_history_repo = PasswordHistoryRepository::new(Arc::clone(&self.pool));
        password_history_repo
            .add_entry(username, password_hash)
            .await?;
        password_history_repo
            .prune_old_entries(username, PASSWORD_HISTORY_SIZE)
            .await?;
        Ok(())
    }

    pub async fn is_2fa_enabled(&self, username: &str) -> Result<bool, AuthError> {
        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));
        let user_2fa = user_2fa_repo.get_by_username(username).await?;

        Ok(user_2fa.map(|u| u.enabled).unwrap_or(false))
    }

    pub async fn generate_totp_secret(
        &self,
        username: &str,
    ) -> Result<(String, String), AuthError> {
        let secret: Vec<u8> = (0..20).map(|_| rand::random::<u8>()).collect();
        let secret_b32 = Self::encode_base32(&secret);

        let issuer = "HomeVault";
        let otpauth_url = format!(
            "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
            issuer, username, secret_b32, issuer
        );

        Ok((secret_b32, otpauth_url))
    }

    pub async fn verify_totp_code(&self, username: &str, code: &str) -> Result<bool, AuthError> {
        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));
        let user_2fa = user_2fa_repo
            .get_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        let secret = &user_2fa.totp_secret_encrypted;
        let code_u64 = match code.parse::<u64>() {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        for offset in 0..=1 {
            let check_time = (current_time / 30) + offset;
            let expected = Self::generate_totp(secret, check_time);
            if expected == code_u64 {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub async fn enable_2fa(&self, username: &str, secret: &str) -> Result<Vec<String>, AuthError> {
        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));

        let backup_codes = User2faRepository::generate_backup_codes(8);
        let backup_codes_json = serde_json::to_string(&backup_codes).unwrap_or_default();

        user_2fa_repo
            .save(username, secret, &backup_codes_json)
            .await?;

        Ok(backup_codes)
    }

    /// Verify a TOTP code against a raw base32 secret that has not been stored
    /// yet, as used by the first-run Setup Wizard before the admin account
    /// exists (the secret is generated client-side at the password step; the
    /// final, authoritative check runs again inside `handle_setup`).
    ///
    /// Accepts a window of −1..=1 time step around `now` (unlike
    /// [`verify_totp_code`], which only looks forward): the wizard generates
    /// the secret and collects the code up to a full period before the account
    /// is created, so a code produced by the authenticator right at the edge of
    /// a period must not be rejected a moment later.
    pub fn verify_totp_code_with_secret(secret: &str, code: &str) -> bool {
        let code_u64 = match code.parse::<u64>() {
            Ok(c) => c,
            Err(_) => return false,
        };

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let base_step = current_time / 30;

        for step in base_step.saturating_sub(1)..=base_step.saturating_add(1) {
            if Self::generate_totp(secret, step) == code_u64 {
                return true;
            }
        }

        false
    }

    pub async fn disable_2fa(&self, username: &str) -> Result<(), AuthError> {
        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));

        let user_2fa = user_2fa_repo.get_by_username(username).await?;
        if user_2fa.is_none() {
            return Ok(());
        }

        user_2fa_repo.delete(username).await?;
        Ok(())
    }

    pub async fn get_backup_codes(&self, username: &str) -> Result<Vec<String>, AuthError> {
        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));
        let user_2fa = user_2fa_repo
            .get_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        let codes: Vec<String> = user_2fa
            .backup_codes
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default();

        Ok(codes)
    }

    pub async fn verify_backup_code(&self, username: &str, code: &str) -> Result<bool, AuthError> {
        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));
        let result = user_2fa_repo.mark_backup_code_used(username, code).await?;
        Ok(result)
    }

    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        ip_address: &str,
        user_agent: Option<&str>,
    ) -> Result<bool, AuthError> {
        self.check_rate_limit(username).await?;

        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        let user = user_repo.find_by_username(username).await?;

        match user {
            Some(user) => {
                if user.locked {
                    return Err(AuthError::UserLocked);
                }

                if self.verify_password(password, &user.password_hash) {
                    self.clear_failed_attempts(username).await?;
                    return Ok(true);
                }

                // A host/PAM account provisioned on a previous login keeps a
                // sentinel hash it can never match — its real credential lives
                // in the host's PAM stack, so fall through to that.
                if user.password_hash == PAM_PROVISIONED_HASH
                    && Self::authenticate_with_pam(username, password)
                {
                    self.ensure_pam_user_row(username).await?;
                    self.clear_failed_attempts(username).await?;
                    return Ok(true);
                }

                self.record_failed_attempt(username, ip_address, user_agent).await?;
                Err(AuthError::InvalidPassword)
            }
            None => {
                // Fallback: authenticate against the host's PAM stack so any
                // Linux account on the server can log in. Gate on a fast
                // `id`-based existence check first — otherwise every bad
                // *username* pays for a `python3` spawn plus `pam_unix`'s
                // multi-second authentication-failure delay.
                let is_host_account = {
                    let uname = username.to_string();
                    tokio::task::spawn_blocking(move || Self::is_system_user(&uname))
                        .await
                        .unwrap_or(false)
                };

                if is_host_account && Self::authenticate_with_pam(username, password) {
                    // Persist a minimal `users` row: `sessions.username` (and
                    // many other tables) have a FK to `users`, so without this
                    // `create_session` fails outright for a host account.
                    self.ensure_pam_user_row(username).await?;
                    self.clear_failed_attempts(username).await?;
                    Ok(true)
                } else {
                    self.record_failed_attempt(username, ip_address, user_agent).await?;
                    Err(AuthError::InvalidPassword)
                }
            }
        }
    }

    /// Sentinel `password_hash` for host/PAM accounts that HomeVault mirrors
    /// into its own `users` table. It is not a valid Argon2 PHC string, so
    /// `verify_password` can never accept it — these accounts authenticate
    /// only through [`Self::authenticate_with_pam`].
    ///
    /// Upserts the row on every PAM login so the mirrored `role_id` tracks the
    /// account's *current* host group membership (promoted/demoted in
    /// sudo/wheel/admin/homevault_admin).
    async fn ensure_pam_user_row(&self, username: &str) -> Result<(), AuthError> {
        let is_admin = Self::user_belongs_to_admin_group(username)
            .await
            .unwrap_or(false);
        let role_id = if is_admin { ADMIN_ROLE_ID } else { USER_ROLE_ID };

        sqlx::query(
            "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
             VALUES (?, ?, '', '', ?, 0, ?, ?)
             ON CONFLICT(username) DO UPDATE SET role_id = excluded.role_id",
        )
        .bind(username)
        .bind(PAM_PROVISIONED_HASH)
        .bind(role_id)
        .bind(Utc::now().to_rfc3339())
        .bind(crate::avatars::generate_random_seed())
        .execute(self.pool.as_ref())
        .await
        .map_err(|e| AuthError::Database(e.to_string()))?;

        Ok(())
    }

    fn authenticate_with_pam(username: &str, password: &str) -> bool {
        use std::process::Command;

        // Escape the credentials for safe inclusion in the Python source.
        let username_py = username.replace('\\', "\\\\").replace('\'', "\\'");
        let password_py = password.replace('\\', "\\\\").replace('\'', "\\'");

        let script = format!(
            r#"
import sys
try:
    import PAM as pam

    _pwd = '{password}'
    _user = '{username}'
    _supplied = False

    def _conv(auth, query_list, _userdata):
        global _supplied
        resp = []
        for q, t in query_list:
            if t in (pam.PAM_PROMPT_ECHO_OFF, pam.PAM_PROMPT_ECHO_ON):
                if _supplied:
                    return None
                _supplied = True
                resp.append((_pwd, 0))
            else:
                resp.append(('', 0))
        return resp

    auth = pam.pam()
    auth.start('login')
    auth.set_item(pam.PAM_USER, _user)
    auth.set_item(pam.PAM_CONV, _conv)
    try:
        # PyPAM's authenticate() returns None on success and raises
        # PAM.error on failure -- its return value must NOT be tested
        # (the old `sys.exit(0 if ok else 1)` rejected every valid login
        # because `ok` was always None).
        auth.authenticate()
        sys.exit(0)
    except Exception:
        sys.exit(1)
except Exception as e:
    sys.stderr.write(str(e))
    sys.exit(2)
"#,
            username = username_py,
            password = password_py,
        );

        let output = Command::new("python3").args(["-c", &script]).output();

        match output {
            Ok(out) => {
                if !out.status.success() {
                    tracing::debug!(
                        "PAM auth failed for {}: stderr={}",
                        username,
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                out.status.success()
            }
            Err(_) => false,
        }
    }

    pub async fn authenticate_with_2fa(
        &self,
        username: &str,
        password: &str,
        totp_code: &str,
        ip_address: &str,
        remember_me: bool,
        user_agent: Option<&str>,
    ) -> Result<Session, AuthError> {
        let valid = self
            .authenticate(username, password, ip_address, user_agent)
            .await?;

        if !valid {
            return Err(AuthError::InvalidPassword);
        }

        let two_fa_enabled = self.is_2fa_enabled(username).await?;
        if two_fa_enabled {
            let code_valid = self.verify_totp_code(username, totp_code).await?;

            if !code_valid {
                let backup_valid = self.verify_backup_code(username, totp_code).await?;
                if !backup_valid {
                    // TOTP/backup-code mismatch — audit it as a failed login
                    // without tripping the password-lockout counter.
                    let login_attempt_repo =
                        LoginAttemptRepository::new(Arc::clone(&self.pool));
                    login_attempt_repo
                        .record_attempt(username, ip_address, false, user_agent)
                        .await?;
                    return Err(AuthError::InvalidTwoFactorCode);
                }
            }
        }

        let session = self
            .create_session(username.to_string(), ip_address.to_string(), remember_me)
            .await?;
        self.record_successful_attempt(username, ip_address, user_agent)
            .await?;
        Ok(session)
    }

    pub async fn get_user(&self, username: &str) -> Result<User, AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        user_repo
            .find_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)
    }

    pub async fn create_user(
        &self,
        username: String,
        password: String,
        fullname: String,
        email: String,
        role_id: i64,
    ) -> Result<(), AuthError> {
        self.validate_password_strength(&password)?;

        if role_id != USER_ROLE_ID && role_id != ADMIN_ROLE_ID {
            return Err(AuthError::InvalidRole);
        }

        let user_repo = UserRepository::new(Arc::clone(&self.pool));

        let count = user_repo.count().await?;
        if count >= MAX_USERS as i64 {
            return Err(AuthError::LimitExceeded);
        }

        let existing = user_repo.find_by_username(&username).await?;
        if existing.is_some() {
            return Err(AuthError::UserAlreadyExists);
        }

        let password_hash = self
            .hash_password(&password)
            .map_err(AuthError::HashingError)?;

        let user = User {
            username: username.clone(),
            password_hash,
            fullname,
            email,
            role_id,
            locked: false,
            created_at: Utc::now(),
            avatar_seed: String::new(),
        };

        user_repo.create(&user).await?;
        self.add_password_to_history(&username, &user.password_hash)
            .await?;

        Ok(())
    }

    pub async fn delete_user(&self, username: &str) -> Result<(), AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));

        let existing = user_repo.find_by_username(username).await?;
        if existing.is_none() {
            return Err(AuthError::UserNotFound);
        }

        user_repo.delete(username).await?;

        let password_history_repo = PasswordHistoryRepository::new(Arc::clone(&self.pool));
        password_history_repo.prune_old_entries(username, 0).await?;

        let user_2fa_repo = User2faRepository::new(Arc::clone(&self.pool));
        let _ = user_2fa_repo.delete(username).await;

        Ok(())
    }

    pub async fn create_session(
        &self,
        username: String,
        ip_address: String,
        remember_me: bool,
    ) -> Result<Session, AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));

        let user = user_repo.find_by_username(&username).await?;

        let is_admin = if let Some(u) = user {
            u.role_id == ADMIN_ROLE_ID
        } else if Self::is_system_user(&username) {
            // The user is not in the HomeVault DB but authenticated via PAM.
            // Grant admin privileges if they belong to a privileged group on
            // the host (sudo, admin, or homevault_admin).
            Self::user_belongs_to_admin_group(&username)
                .await
                .unwrap_or(false)
        } else {
            false
        };

        let token = self.generate_token();
        let csrf_token = self.generate_token();
        let now = Utc::now();
        let lifetime_seconds = if remember_me {
            REMEMBER_ME_SESSION_LIFETIME_SECONDS
        } else {
            SESSION_LIFETIME_SECONDS
        };

        let session = Session {
            token: token.clone(),
            username: username.clone(),
            ip_address,
            csrf_token,
            created_at: now,
            expires_at: now + Duration::seconds(lifetime_seconds),
            is_admin,
        };

        session_repo.create(&session).await?;
        Ok(session)
    }

    /// Returns true if `username` exists as an account on the host system
    /// (i.e. has an entry in /etc/passwd or is resolvable via `id`).
    pub fn is_system_user(username: &str) -> bool {
        use std::process::Command;

        // Reject obviously invalid usernames early to avoid shelling out.
        if username.is_empty()
            || username.len() > 32
            || !username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return false;
        }

        Command::new("id")
            .args(["-un", username])
            .stdin(std::process::Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Returns true if the system user belongs to a group that grants
    /// administrative privileges in HomeVault (sudo, admin, wheel, or
    /// homevault_admin). Callers: `create_session` (once per login — cheap
    /// even blocking) and `is_admin` (every admin-gated request — the
    /// `spawn_blocking` below is what keeps that path off the Tokio runtime).
    async fn user_belongs_to_admin_group(username: &str) -> Result<bool, AuthError> {
        use std::process::Command;

        let username = username.to_string();
        let output = tokio::task::spawn_blocking(move || {
            Command::new("id")
                .args(["-Gn", &username])
                .stdin(std::process::Stdio::null())
                .output()
        })
        .await
        .map_err(|e| AuthError::Database(e.to_string()))?
        .map_err(|e| AuthError::Database(e.to_string()))?;

        if !output.status.success() {
            // `id` failed — treat as a regular user rather than granting
            // admin, which is the safer default.
            return Ok(false);
        }

        let groups_output = String::from_utf8_lossy(&output.stdout);
        Ok(groups_output.split_whitespace().any(|g| {
            g == "sudo"
                || g == "admin"
                || g == "wheel"
                || g == ADMIN_GROUP_NAME
        }))
    }

    pub async fn get_session(&self, token: &str) -> Result<Session, AuthError> {
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));

        let session = session_repo.find_by_token(token).await?;

        match session {
            Some(session) => {
                if session.expires_at < Utc::now() {
                    return Err(AuthError::SessionExpired);
                }
                Ok(session)
            }
            None => Err(AuthError::SessionNotFound),
        }
    }

    pub async fn invalidate_session(&self, token: &str) -> Result<(), AuthError> {
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));
        session_repo.delete(token).await?;
        Ok(())
    }

    pub async fn invalidate_user_sessions(&self, username: &str) -> Result<(), AuthError> {
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));
        session_repo.delete_by_username(username).await?;
        Ok(())
    }

    /// Called on every `require_admin`-gated request (RAID, Docker, Restic,
    /// Storage pools, encryption, user/group management, ...). The large
    /// majority of accounts are HomeVault DB accounts created via the Setup
    /// Wizard, not host PAM accounts, so check the database first — a plain
    /// indexed SQLite lookup — and only fall back to `is_system_user`
    /// (spawns `id`) for accounts that aren't in the database at all. This
    /// mirrors the check order `create_session` already uses. The subprocess
    /// fallback itself is deported to `spawn_blocking` so it can never stall
    /// a Tokio worker thread even for a PAM account.
    pub async fn is_admin(&self, username: &str) -> bool {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        if let Ok(Some(u)) = user_repo.find_by_username(username).await {
            return u.role_id == ADMIN_ROLE_ID;
        }

        // Not a HomeVault DB account — check whether it's a privileged host
        // (PAM) account instead.
        let uname = username.to_string();
        let is_system_user = tokio::task::spawn_blocking(move || Self::is_system_user(&uname))
            .await
            .unwrap_or(false);
        if !is_system_user {
            return false;
        }

        Self::user_belongs_to_admin_group(username)
            .await
            .unwrap_or(false)
    }

    pub async fn validate_session_token(&self, token: &str) -> bool {
        self.get_session(token).await.is_ok()
    }

    pub async fn validate_session_csrf_token(&self, session_token: &str, csrf_token: &str) -> bool {
        if let Ok(session) = self.get_session(session_token).await {
            session.csrf_token == csrf_token
        } else {
            false
        }
    }

    pub async fn refresh_session(&self, token: &str) -> Result<(), AuthError> {
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));

        let session = session_repo.find_by_token(token).await?;

        match session {
            Some(mut session) => {
                session.expires_at = Utc::now() + Duration::seconds(SESSION_LIFETIME_SECONDS);
                session_repo.delete(token).await?;
                session_repo.create(&session).await?;
                Ok(())
            }
            None => Err(AuthError::SessionNotFound),
        }
    }

    pub async fn cleanup_expired_sessions(&self) {
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));
        let _ = session_repo.delete_expired().await;
    }

    pub async fn update_user(
        &self,
        username: &str,
        fullname: String,
        email: String,
    ) -> Result<(), AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));

        let mut user = user_repo
            .find_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        user.fullname = fullname;
        user.email = email;
        user_repo.update(&user).await?;

        Ok(())
    }

    pub async fn update_password(
        &self,
        username: &str,
        current_password: Option<&str>,
        new_password: &str,
    ) -> Result<(), AuthError> {
        self.validate_password_strength(new_password)?;
        self.check_password_history(new_password, username).await?;

        let user_repo = UserRepository::new(Arc::clone(&self.pool));

        let user = user_repo
            .find_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        if let Some(pwd) = current_password {
            if !self.verify_password(pwd, &user.password_hash) {
                return Err(AuthError::InvalidPassword);
            }
        }

        let old_hash = user.password_hash.clone();
        let new_hash = self
            .hash_password(new_password)
            .map_err(AuthError::HashingError)?;

        let mut updated_user = user;
        updated_user.password_hash = new_hash.clone();
        user_repo.update(&updated_user).await?;

        self.add_password_to_history(username, &old_hash).await?;

        let session_repo = SessionRepository::new(Arc::clone(&self.pool));
        session_repo.delete_by_username(username).await?;

        Ok(())
    }

    pub async fn update_role(&self, username: &str, role_id: i64) -> Result<(), AuthError> {
        if role_id != USER_ROLE_ID && role_id != ADMIN_ROLE_ID {
            return Err(AuthError::InvalidRole);
        }
        let user_repo = UserRepository::new(Arc::clone(&self.pool));

        let mut user = user_repo
            .find_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        let old_role_id = user.role_id;
        user.role_id = role_id;
        user_repo.update(&user).await?;

        if role_id == ADMIN_ROLE_ID && old_role_id != ADMIN_ROLE_ID {
            Self::sync_user_to_admin_group(username).await?;
        } else if role_id != ADMIN_ROLE_ID && old_role_id == ADMIN_ROLE_ID {
            Self::remove_user_from_admin_group(username).await?;
        }

        Ok(())
    }

    async fn sync_user_to_admin_group(username: &str) -> Result<(), AuthError> {
        use std::process::Command;

        let output = Command::new("groups")
            .arg(username)
            .output()
            .map_err(|e| AuthError::Database(e.to_string()))?;

        if output.status.success() {
            let groups_output = String::from_utf8_lossy(&output.stdout);
            if !groups_output.contains(ADMIN_GROUP_NAME) {
                let _ = Command::new("usermod")
                    .args(["-a", "-G", ADMIN_GROUP_NAME, username])
                    .output();

                tracing::info!(
                    "Added user '{}' to admin group '{}'",
                    username,
                    ADMIN_GROUP_NAME
                );
            }
        }

        Ok(())
    }

    async fn remove_user_from_admin_group(username: &str) -> Result<(), AuthError> {
        use std::process::Command;

        let output = Command::new("gpasswd")
            .args(["-d", username, ADMIN_GROUP_NAME])
            .output()
            .map_err(|e| AuthError::Database(e.to_string()))?;

        if output.status.success() {
            tracing::info!(
                "Removed user '{}' from admin group '{}'",
                username,
                ADMIN_GROUP_NAME
            );
        }

        Ok(())
    }

    pub async fn set_user_locked(&self, username: &str, locked: bool) -> Result<(), AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));

        let mut user = user_repo
            .find_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        user.locked = locked;
        user_repo.update(&user).await?;

        if !locked {
            self.clear_failed_attempts(username).await?;
        }

        Ok(())
    }

    pub async fn user_exists(&self, username: &str) -> bool {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        user_repo
            .find_by_username(username)
            .await
            .ok()
            .flatten()
            .is_some()
    }

    pub async fn get_user_count(&self) -> Result<i64, AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        Ok(user_repo.count().await?)
    }

    pub async fn count_admins(&self) -> Result<i64, AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        Ok(user_repo.count_by_role(ADMIN_ROLE_ID).await?)
    }

    pub async fn get_all_users(&self) -> Result<Vec<User>, AuthError> {
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        Ok(user_repo.find_all().await?)
    }

    pub async fn get_user_by_index(&self, index: i64) -> Result<Option<User>, AuthError> {
        let users = self.get_all_users().await?;
        Ok(users.into_iter().nth(index.saturating_sub(1) as usize))
    }

    pub async fn get_username_from_token(&self, token: &str) -> Option<String> {
        self.get_session(token).await.ok().map(|s| s.username)
    }

    pub fn hash_password(&self, password: &str) -> Result<String, String> {
        use argon2::password_hash::PasswordHasher;
        use argon2::Argon2;

        let argon2 = Argon2::default();

        match argon2.hash_password(password.as_bytes()) {
            Ok(hash) => Ok(hash.to_string()),
            Err(e) => Err(format!("Hashing error: {}", e)),
        }
    }

    pub fn verify_password(&self, password: &str, hash: &str) -> bool {
        use argon2::password_hash::PasswordVerifier;
        use argon2::Argon2;

        let argon2 = Argon2::default();
        argon2.verify_password(password.as_bytes(), hash).is_ok()
    }

    fn encode_base32(data: &[u8]) -> String {
        const BASE32_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let mut result = String::new();
        let mut buffer = 0u64;
        let mut bits_left = 0;

        for byte in data {
            buffer = (buffer << 8) | (*byte as u64);
            bits_left += 8;

            while bits_left >= 5 {
                let index = ((buffer >> (bits_left - 5)) & 0x1F) as usize;
                result.push(BASE32_CHARS[index] as char);
                bits_left -= 5;
            }
        }

        if bits_left > 0 {
            let index = ((buffer << (5 - bits_left)) & 0x1F) as usize;
            result.push(BASE32_CHARS[index] as char);
        }

        result
    }

    fn decode_base32(data: &str) -> Result<Vec<u8>, AuthError> {
        // Maps 'A'-'Z' to 0-25 and '2'-'7' to 26-31 per RFC 4648; everything
        // else (including '0','1','8','9', which standard base32 excludes) is
        // invalid and skipped. Must stay the exact inverse of `encode_base32`'s
        // `BASE32_CHARS` alphabet, or generated TOTP secrets containing
        // 'W'-'Z' silently decode to the wrong bytes and 2FA codes never match.
        const DECODE_TABLE: [i8; 128] = [
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, 26, 27, 28, 29, 30,
            31, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0,
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11,
            12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
            23, 24, 25, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1,
        ];

        let data_upper = data.to_uppercase();
        let mut result = Vec::new();
        let mut buffer = 0u64;
        let mut bits_left = 0;

        for c in data_upper.chars() {
            let value = if c.is_ascii() {
                DECODE_TABLE[c as usize]
            } else {
                -1
            };
            if value < 0 {
                continue;
            }

            buffer = (buffer << 5) | (value as u64);
            bits_left += 5;

            while bits_left >= 8 {
                let byte = ((buffer >> (bits_left - 8)) & 0xFF) as u8;
                result.push(byte);
                bits_left -= 8;
            }
        }

        Ok(result)
    }

    fn generate_totp(secret: &str, counter: u64) -> u64 {
        let secret_bytes = match Self::decode_base32(secret) {
            Ok(bytes) => bytes,
            Err(_) => return 0,
        };

        use hmac::{Hmac, Mac, KeyInit};
        use sha1::Sha1;
        type HmacSha1 = Hmac<Sha1>;

        let mut mac = match HmacSha1::new_from_slice(&secret_bytes) {
            Ok(m) => m,
            Err(_) => return 0,
        };

        let counter_bytes = counter.to_be_bytes();
        mac.update(&counter_bytes);

        let result = mac.finalize();
        let hash: [u8; 20] = result.into_bytes().into();
        let offset = (hash[hash.len() - 1] & 0x0F) as usize;

        let truncated = ((hash[offset] & 0x7F) as u32) << 24
            | (hash[offset + 1] as u32) << 16
            | (hash[offset + 2] as u32) << 8
            | (hash[offset + 3] as u32);

        (truncated % 1_000_000) as u64
    }

    fn generate_token(&self) -> String {
        use rand::Rng;

        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::rng();
        let mut token = String::with_capacity(32);

        let mut buf = [0u8; 32];
        rng.fill_bytes(&mut buf);

        for byte in buf {
            token.push(CHARSET[(byte as usize) % CHARSET.len()] as char);
        }
        token
    }

    // =====================================================================
    // Numeric recovery code (email-free "forgot password")
    // =====================================================================

    /// Generate a fresh recovery code: [`RECOVERY_CODE_GROUPS`] space-separated
    /// groups of 4 cryptographically-random digits, e.g.
    /// `"4821 0193 7756 2240 9310 5589 0021 4467"`.
    pub fn generate_recovery_code() -> String {
        use rand::Rng;

        let mut rng = rand::rng();
        let mut buf = [0u8; RECOVERY_CODE_GROUPS * 4];
        rng.fill_bytes(&mut buf);

        let digits: String = buf.iter().map(|b| char::from(b'0' + (*b % 10))).collect();

        digits
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Strip every non-digit character so `"4821 0193"`, `"4821-0193"` and
    /// `"48210193"` all compare equal.
    pub fn normalize_recovery_code(input: &str) -> String {
        input.chars().filter(|c| c.is_ascii_digit()).collect()
    }

    /// Whether the user currently has a recovery code set.
    pub async fn has_recovery_code(&self, username: &str) -> Result<bool, AuthError> {
        let repo = PasswordRecoveryCodeRepository::new(Arc::clone(&self.pool));
        Ok(repo.exists(username).await?)
    }

    /// Generate a new recovery code for the user, persist only its Argon2 hash,
    /// and return the plaintext code (the caller must show it exactly once).
    /// Replaces any existing code.
    pub async fn set_recovery_code(&self, username: &str) -> Result<String, AuthError> {
        // Guard against setting a code for a user that doesn't exist (the FK
        // would reject it anyway, but this gives a clean error).
        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        if user_repo.find_by_username(username).await?.is_none() {
            return Err(AuthError::UserNotFound);
        }

        let code = Self::generate_recovery_code();
        let hash = self
            .hash_password(&Self::normalize_recovery_code(&code))
            .map_err(AuthError::HashingError)?;

        let repo = PasswordRecoveryCodeRepository::new(Arc::clone(&self.pool));
        repo.set(username, &hash).await?;

        Ok(code)
    }

    /// Verify a recovery code against the stored hash, rate-limited per user.
    /// Digit-only comparison (whitespace/dashes ignored).
    pub async fn verify_recovery_code(
        &self,
        username: &str,
        input: &str,
    ) -> Result<bool, AuthError> {
        let attempt_repo = LoginAttemptRepository::new(Arc::clone(&self.pool));
        // Namespaced key so recovery guesses never trip (or get tripped by) the
        // normal login lockout and never lock the account itself.
        let attempt_key = format!("recovery:{}", username);

        let recent = attempt_repo
            .get_attempts_count(&attempt_key, RECOVERY_CODE_WINDOW_SECONDS)
            .await?;
        if recent >= MAX_RECOVERY_CODE_ATTEMPTS {
            return Err(AuthError::TooManyRecoveryAttempts);
        }

        let repo = PasswordRecoveryCodeRepository::new(Arc::clone(&self.pool));
        let stored = repo.get_hash(username).await?;

        let normalized = Self::normalize_recovery_code(input);
        let ok = match &stored {
            Some(hash) => !normalized.is_empty() && self.verify_password(&normalized, hash),
            None => false,
        };

        if !ok {
            attempt_repo
                .record_attempt(&attempt_key, "0.0.0.0", false, None)
                .await?;
        } else {
            attempt_repo.clear_attempts(&attempt_key).await?;
        }

        Ok(ok)
    }

    /// Reset a password using a numeric recovery code.
    ///
    /// On success the code is left in place (it stays valid for a future reset),
    /// every session for the account is invalidated, and a locked account is
    /// unlocked.
    pub async fn reset_password_with_recovery_code(
        &self,
        username: &str,
        recovery_code: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        if !self.verify_recovery_code(username, recovery_code).await? {
            return Err(AuthError::InvalidRecoveryCode);
        }

        self.validate_password_strength(new_password)?;
        self.check_password_history(new_password, username).await?;

        let user_repo = UserRepository::new(Arc::clone(&self.pool));
        let mut user = user_repo
            .find_by_username(username)
            .await?
            .ok_or(AuthError::UserNotFound)?;

        let old_hash = user.password_hash.clone();
        let new_hash = self
            .hash_password(new_password)
            .map_err(AuthError::HashingError)?;

        user.password_hash = new_hash;
        user_repo.update(&user).await?;

        self.add_password_to_history(username, &old_hash).await?;

        // Invalidate all sessions (force re-login everywhere).
        let session_repo = SessionRepository::new(Arc::clone(&self.pool));
        session_repo.delete_by_username(username).await?;

        if user.locked {
            user.locked = false;
            user_repo.update(&user).await?;
            self.clear_failed_attempts(username).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod base32_totp_tests {
    use super::AuthService;

    // Regression test for a bug where `decode_base32`'s lookup table mapped
    // 'W'/'X'/'Y'/'Z' to -1 (invalid) instead of 22-25, silently dropping
    // those characters from any TOTP secret that contained them. Since
    // secrets are randomly generated, this made 2FA enrollment/login fail
    // for the large majority of accounts.
    #[test]
    fn decode_base32_round_trips_every_alphabet_character() {
        // Covers all 32 valid base32 symbols, including the previously
        // broken 'W', 'X', 'Y', 'Z'.
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let encoded = AuthService::encode_base32(alphabet);
        let decoded = AuthService::decode_base32(&encoded).expect("valid base32");
        let re_encoded = AuthService::encode_base32(&decoded);
        assert_eq!(re_encoded, encoded, "decode(encode(x)) must re-encode to the same string");
    }

    #[test]
    fn decode_base32_rejects_digits_outside_2_to_7() {
        // '0', '1', '8', '9' are not part of the RFC 4648 base32 alphabet
        // and must not silently decode to a value.
        for ch in ['0', '1', '8', '9'] {
            let s = ch.to_string();
            let decoded = AuthService::decode_base32(&s).expect("decode does not error");
            assert!(decoded.is_empty(), "'{}' must not decode to any bits", ch);
        }
    }

    #[test]
    fn recovery_code_has_eight_groups_of_four_digits() {
        let code = AuthService::generate_recovery_code();
        let groups: Vec<&str> = code.split(' ').collect();
        assert_eq!(groups.len(), 8, "expected 8 groups, got {:?}", groups);
        for g in groups {
            assert_eq!(g.len(), 4, "group '{}' is not 4 chars", g);
            assert!(g.chars().all(|c| c.is_ascii_digit()), "group '{}' has non-digits", g);
        }
    }

    #[test]
    fn recovery_codes_are_not_constant() {
        let a = AuthService::generate_recovery_code();
        let b = AuthService::generate_recovery_code();
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_recovery_code_strips_separators() {
        assert_eq!(
            AuthService::normalize_recovery_code("4821 0193-7756\t2240"),
            "4821019377562240"
        );
        assert_eq!(AuthService::normalize_recovery_code("abc"), "");
    }

    #[test]
    fn totp_matches_known_rfc6238_test_vector() {
        // RFC 6238 test vector for the SHA1 case: secret "12345678901234567890"
        // (ASCII) base32-encoded, at T=59s (counter=1) yields code 94287082.
        // generate_totp() truncates to 6 digits (`% 1_000_000`), so this
        // checks the last 6 digits: "287082".
        let secret = AuthService::encode_base32(b"12345678901234567890");
        let code = AuthService::generate_totp(&secret, 1);
        assert_eq!(code, 287_082);
    }

    #[test]
    fn verify_totp_code_with_secret_accepts_the_current_code() {
        // Deterministic 20-byte secret. The −1..=1 acceptance window makes the
        // assertion race-free: even if a 30 s boundary ticks between computing
        // the code (floor N) and verifying it (floor N or N+1), floor N is
        // always inside the window.
        let secret = AuthService::encode_base32(&[8u8; 20]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let code = AuthService::generate_totp(&secret, now / 30);
        assert!(AuthService::verify_totp_code_with_secret(&secret, &code.to_string()));
    }

    #[test]
    fn verify_totp_code_with_secret_rejects_a_wrong_code() {
        let secret = AuthService::encode_base32(&[8u8; 20]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let code = AuthService::generate_totp(&secret, now / 30);
        let wrong = (code + 1) % 1_000_000;
        assert!(!AuthService::verify_totp_code_with_secret(&secret, &wrong.to_string()));
    }

    #[test]
    fn verify_totp_code_with_secret_rejects_non_numeric_code() {
        assert!(!AuthService::verify_totp_code_with_secret(
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
            "abc"
        ));
        assert!(!AuthService::verify_totp_code_with_secret(
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ",
            ""
        ));
    }
}
