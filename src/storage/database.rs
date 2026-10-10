use include_dir::{include_dir, Dir};
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use std::sync::Arc;
use thiserror::Error;

use crate::storage::models::{User, ADMIN_ROLE_ID};

/// The `.sql` migration files, embedded into the binary at compile time.
///
/// Previously read from `Path::new(env!("CARGO_MANIFEST_DIR")).join("sqlx/migrations")`
/// at runtime — `CARGO_MANIFEST_DIR` bakes in the *build* machine's absolute
/// path, not the deployment machine's. `installer/install.sh` extracts the
/// binary into `/opt/homevault`, which has no reason to match the path the
/// binary was compiled at, so every real deployment's first boot would fail
/// with "Failed to read migrations dir" the moment the build and run
/// locations differ — masked in this sandbox (and likely in prior sessions)
/// only because both happened to be the same checkout. Migrations are
/// static, known at build time, so embedding them removes the filesystem
/// dependency entirely instead of trying to guess the right runtime path.
static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/sqlx/migrations");

pub const ADMIN_GROUP_NAME: &str = "homevault_admin";

/// Errors originating from database operations.
#[derive(Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Migration error: {0}")]
    Migration(String),
    #[error("Record not found")]
    NotFound,
}

/// Wraps a shared SQLite connection pool and provides lifecycle management.
#[derive(Clone)]
pub struct Database {
    pool: Arc<SqlitePool>,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self, DbError> {
        // A single-admin, few-concurrent-users home server doesn't need a large
        // pool; each idle SQLite connection still reserves some RAM (page cache
        // buffers), so this is sized for the app's actual concurrency needs
        // rather than a generic web-service default.
        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;

        Self::configure_sqlite(&pool).await?;

        Ok(Self {
            pool: Arc::new(pool),
        })
    }

    /// Tune SQLite pragmas for better performance and safety.
    async fn configure_sqlite(pool: &SqlitePool) -> Result<(), DbError> {
        // WAL mode allows concurrent reads while writing.
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(pool)
            .await?;

        // Normal sync: balances crash safety with write speed.
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(pool)
            .await?;

        // 32 MB page cache (~8000 pages of 4 KB).
        sqlx::query("PRAGMA cache_size = -32000")
            .execute(pool)
            .await?;

        // Keep temporary tables/indexes in memory (not on disk).
        sqlx::query("PRAGMA temp_store = MEMORY")
            .execute(pool)
            .await?;

        // 128 MB memory-map for faster reads.
        sqlx::query("PRAGMA mmap_size = 134217728")
            .execute(pool)
            .await?;

        // Enforce foreign key constraints at the engine level.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(pool)
            .await?;

        // Wait up to 5s for a lock instead of failing immediately with
        // "database is locked" when a background task (scheduler, notification
        // engine) and a web request briefly collide on a write.
        sqlx::query("PRAGMA busy_timeout = 5000")
            .execute(pool)
            .await?;

        tracing::info!("SQLite configured with optimized settings (WAL mode, 32MB cache)");
        Ok(())
    }

    /// Returns a cloned `Arc` to the shared connection pool.
    pub fn pool(&self) -> Arc<SqlitePool> {
        Arc::clone(&self.pool)
    }

    /// Run all pending `.sql` migrations (embedded in the binary at compile
    /// time from `sqlx/migrations/`) in sorted order.
    pub async fn run_migrations(&self) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        // Bootstrap: create applied-migrations table if it doesn't exist.
        sqlx::query("CREATE TABLE IF NOT EXISTS _hv_migrations (name TEXT PRIMARY KEY)")
            .execute(tx.as_mut())
            .await?;

        let mut entries: Vec<_> = MIGRATIONS_DIR
            .files()
            .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("sql"))
            .collect();

        entries.sort_by_key(|f| f.path().to_path_buf());

        for entry in entries {
            let filename = entry
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Skip if already applied.
            let already_applied: Option<(String,)> =
                sqlx::query_as("SELECT name FROM _hv_migrations WHERE name = ?")
                    .bind(&filename)
                    .fetch_optional(tx.as_mut())
                    .await?;

            if already_applied.is_some() {
                tracing::debug!("Skipping already-applied migration: {}", filename);
                continue;
            }

            let migration_sql = entry.contents_utf8().ok_or_else(|| {
                DbError::Migration(format!("Migration {} is not valid UTF-8", filename))
            })?;

            let statements: Vec<String> = Self::parse_sql_statements(migration_sql);

            for statement in statements {
                if statement.trim().is_empty() {
                    continue;
                }
                let query = sqlx::query(&*Box::leak(statement.into_boxed_str()));
                query.execute(tx.as_mut()).await?;
            }

            sqlx::query("INSERT INTO _hv_migrations (name) VALUES (?)")
                .bind(&filename)
                .execute(tx.as_mut())
                .await?;

            tracing::info!("Ran migration: {}", filename);
        }

        tx.commit()
            .await
            .map_err(|e| DbError::Migration(e.to_string()))?;

        Ok(())
    }

    /// Replace the free-text `role` column with a normalized `roles` table + FK.
    /// Idempotent: each DDL step checks whether it has already been applied.
    pub async fn migrate_roles_005(&self) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;

        // Step 1: Create roles reference table (idempotent).
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS roles (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                CHECK (id IN (1, 2))
            )",
        )
        .execute(tx.as_mut())
        .await?;
        sqlx::query("INSERT OR IGNORE INTO roles (id, name) VALUES (1, 'user'), (2, 'admin')")
            .execute(tx.as_mut())
            .await?;

        // Step 2: Add role_id column only if it doesn't already exist.
        let role_id_exists: bool =
            sqlx::query("SELECT count(*) FROM pragma_table_info('users') WHERE name = 'role_id'")
                .fetch_one(tx.as_mut())
                .await
                .map(|row| row.get::<i64, _>(0) > 0)
                .unwrap_or(false);

        if !role_id_exists {
            sqlx::query("ALTER TABLE users ADD COLUMN role_id INTEGER NOT NULL DEFAULT 1")
                .execute(tx.as_mut())
                .await?;
            sqlx::query(
                "UPDATE users SET role_id = (SELECT id FROM roles WHERE name = users.role)",
            )
            .execute(tx.as_mut())
            .await?;
        }

        // Step 3: Recreate users table if old `role` column still exists.
        let role_column_exists: bool =
            sqlx::query("SELECT count(*) FROM pragma_table_info('users') WHERE name = 'role'")
                .fetch_one(tx.as_mut())
                .await
                .map(|row| row.get::<i64, _>(0) > 0)
                .unwrap_or(false);

        if role_column_exists {
            sqlx::query("ALTER TABLE users RENAME TO _users_old_005")
                .execute(tx.as_mut())
                .await?;
            sqlx::query(
                "CREATE TABLE users (
                    username TEXT PRIMARY KEY,
                    password_hash TEXT NOT NULL,
                    fullname TEXT NOT NULL,
                    email TEXT NOT NULL,
                    role_id INTEGER NOT NULL DEFAULT 1,
                    locked INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    FOREIGN KEY (role_id) REFERENCES roles(id),
                    CHECK (role_id IN (1, 2))
                )",
            )
            .execute(tx.as_mut())
            .await?;
            sqlx::query(
                "INSERT INTO users
                 SELECT username, password_hash, fullname, email, role_id, locked, created_at
                 FROM _users_old_005",
            )
            .execute(tx.as_mut())
            .await?;
            sqlx::query("DROP TABLE _users_old_005")
                .execute(tx.as_mut())
                .await?;
        }

        tx.commit()
            .await
            .map_err(|e| DbError::Migration(e.to_string()))?;
        tracing::info!("Applied migration: 005_roles");
        Ok(())
    }

    /// Split raw SQL into individual statements, respecting string literals and
    /// ignoring comment-only splits. This avoids mis-splitting tokens like `ELSE 0;`.
    ///
    /// Also tracks `--` line comments explicitly: a semicolon appearing inside a
    /// comment (e.g. explanatory prose like `-- for quick reads; live status is
    /// truth.`) must not be treated as a statement terminator, or the text after
    /// it gets split off as an orphaned fragment lacking its `--` prefix and is
    /// sent to SQLite as if it were real SQL, producing a syntax error at the
    /// first bare word after the semicolon.
    fn parse_sql_statements(sql: &str) -> Vec<String> {
        let mut stmts = Vec::new();
        let mut buf = String::new();
        let mut in_string = false;
        let mut in_comment = false;
        let mut prev_was_semicolon = false;
        let mut prev_char: Option<char> = None;

        for ch in sql.chars() {
            if in_comment {
                buf.push(ch);
                if ch == '\n' {
                    in_comment = false;
                }
                prev_char = Some(ch);
                continue;
            }
            if ch == '\'' && !in_string {
                in_string = true;
                buf.push(ch);
            } else if ch == '\'' && in_string {
                in_string = false;
                buf.push(ch);
            } else if ch == '-' && prev_char == Some('-') && !in_string {
                // The first '-' was already pushed as a normal char below; this
                // is its pair, so the rest of the line is a comment.
                in_comment = true;
                buf.push(ch);
            } else if ch == ';' && !in_string {
                let stmt = buf.trim();
                // Only keep statements that have at least one non-comment line.
                if !stmt.lines().all(|line| {
                    let trimmed = line.trim();
                    trimmed.is_empty() || trimmed.starts_with("--")
                }) {
                    stmts.push(stmt.to_string());
                }
                buf.clear();
                prev_was_semicolon = true;
            } else {
                if prev_was_semicolon && ch.is_whitespace() {
                    // skip leading whitespace after a semicolon
                    prev_char = Some(ch);
                    continue;
                }
                prev_was_semicolon = false;
                buf.push(ch);
            }
            prev_char = Some(ch);
        }

        // Don't forget the last statement (no trailing semicolon).
        let stmt = buf.trim();
        if !stmt.is_empty()
            && !stmt.lines().all(|line| {
                let trimmed = line.trim();
                trimmed.is_empty() || trimmed.starts_with("--")
            })
        {
            stmts.push(stmt.to_string());
        }

        stmts
    }

    /// Ensure the `homevault_admin` system group exists (best-effort).
    /// Ensure the `homevault_admin` system group exists on the host (best-effort).
    pub async fn ensure_admin_group(&self) -> Result<(), DbError> {
        let getent_check = tokio::process::Command::new("which")
            .arg("getent")
            .output()
            .await;

        if getent_check.is_err() || !getent_check.unwrap().status.success() {
            tracing::warn!("System group management not available (getent not found). Skipping admin group creation.");
            return Ok(());
        }

        let output = tokio::process::Command::new("getent")
            .args(["group", ADMIN_GROUP_NAME])
            .output()
            .await
            .map_err(DbError::Io)?;

        if !output.status.success() {
            // Attempt group creation, but do not fail if we lack privileges.
            match tokio::process::Command::new("sudo")
                .arg("groupadd")
                .arg(ADMIN_GROUP_NAME)
                .output()
                .await
            {
                Ok(output) => {
                    if output.status.success() {
                        tracing::info!("Created admin group: {}", ADMIN_GROUP_NAME);
                    } else {
                        tracing::warn!("Could not create system group (insufficient permissions). Continuing without system group integration.");
                    }
                }
                Err(e) => {
                    tracing::warn!("Could not create system group: {}. Continuing without system group integration.", e);
                }
            }
        }

        Ok(())
    }

    /// Add all database admin users to the `homevault_admin` system group.
    pub async fn sync_admin_group_members(&self) -> Result<(), DbError> {
        let users = sqlx::query_as::<_, User>(
            "SELECT username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed FROM users WHERE role_id = ?"
        )
        .bind(ADMIN_ROLE_ID)
        .fetch_all(self.pool.as_ref())
        .await?;

        for user in users {
            let output = tokio::process::Command::new("getent")
                .args(["group", ADMIN_GROUP_NAME])
                .output()
                .await
                .map_err(DbError::Io)?;

            if output.status.success() {
                let group_line = String::from_utf8_lossy(&output.stdout);
                let members: Vec<String> = group_line
                    .split(':')
                    .nth(3)
                    .map(|m| {
                        m.split(',')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .unwrap_or_default();

                if !members.contains(&user.username) {
                    let output = tokio::process::Command::new("sudo")
                        .arg("usermod")
                        .args(["-a", "-G", ADMIN_GROUP_NAME, &user.username])
                        .output()
                        .await
                        .map_err(DbError::Io)?;

                    if output.status.success() {
                        tracing::info!(
                            "Added user '{}' to admin group '{}'",
                            user.username,
                            ADMIN_GROUP_NAME
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Sync a single admin user to the `homevault_admin` system group and sudo/wheel.
    /// Creates the Linux user if it does not exist (best-effort).
    pub async fn sync_admin_user_to_group(&self, username: &str) -> Result<(), DbError> {
        // First, ensure the Linux user exists
        let user_exists = tokio::process::Command::new("id")
            .arg(username)
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false);

        if !user_exists {
            // Create Linux user with home directory
            tracing::info!("Creating Linux user '{}' for admin access", username);
            let home_path = crate::config::paths::user_home_path(username);
            let home = home_path.to_string_lossy();
            let output = tokio::process::Command::new("sudo")
                .arg("useradd")
                .args([
                    "-m", // Create home directory
                    "-d",
                    home.as_ref(), // Home under /home/homevault/{user}
                    "-s",
                    "/bin/bash", // Set shell to bash
                    "-G",
                    ADMIN_GROUP_NAME, // Add to admin group
                    username,
                ])
                .output()
                .await
                .map_err(DbError::Io)?;

            if output.status.success() {
                tracing::info!("Created Linux user '{}' with home directory", username);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    "Failed to create Linux user '{}': {}. Terminal access may be limited.",
                    username,
                    stderr
                );
                // Don't fail - user can still use HomeVault without Linux account
                return Ok(());
            }
        } else {
            tracing::info!("Linux user '{}' already exists", username);
        }

        // Add user to homevault_admin group
        let output = tokio::process::Command::new("getent")
            .args(["group", ADMIN_GROUP_NAME])
            .output()
            .await
            .map_err(DbError::Io)?;

        if output.status.success() {
            // Check if user is already in the group
            let group_line = String::from_utf8_lossy(&output.stdout);
            let members: Vec<String> = group_line
                .split(':')
                .nth(3)
                .map(|m| {
                    m.split(',')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();

            if !members.contains(&username.to_string()) {
                let output = tokio::process::Command::new("sudo")
                    .arg("usermod")
                    .args(["-a", "-G", ADMIN_GROUP_NAME, username])
                    .output()
                    .await
                    .map_err(DbError::Io)?;

                if output.status.success() {
                    tracing::info!(
                        "Added user '{}' to admin group '{}'",
                        username,
                        ADMIN_GROUP_NAME
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    tracing::warn!("Failed to add user '{}' to group: {}", username, stderr);
                }
            }
        } else {
            tracing::warn!("Admin group '{}' does not exist yet", ADMIN_GROUP_NAME);
        }

        // Also add to system sudo group for full admin privileges
        // Check if 'sudo' group exists (Debian/Ubuntu)
        let sudo_check = tokio::process::Command::new("getent")
            .args(["group", "sudo"])
            .output()
            .await
            .map_err(DbError::Io)?;

        if sudo_check.status.success() {
            let output = tokio::process::Command::new("sudo")
                .arg("usermod")
                .args(["-a", "-G", "sudo", username])
                .output()
                .await
                .map_err(DbError::Io)?;

            if output.status.success() {
                tracing::info!("Added user '{}' to system 'sudo' group", username);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::debug!("Could not add user to sudo group: {}", stderr);
            }
        }

        // Check if 'wheel' group exists (RedHat/CentOS/Fedora)
        let wheel_check = tokio::process::Command::new("getent")
            .args(["group", "wheel"])
            .output()
            .await
            .map_err(DbError::Io)?;

        if wheel_check.status.success() {
            let output = tokio::process::Command::new("sudo")
                .arg("usermod")
                .args(["-a", "-G", "wheel", username])
                .output()
                .await
                .map_err(DbError::Io)?;

            if output.status.success() {
                tracing::info!("Added user '{}' to system 'wheel' group", username);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::debug!("Could not add user to wheel group: {}", stderr);
            }
        }

        Ok(())
    }

    /// Verify the admin user's home directory exists with correct ownership.
    /// The directory is normally created by `useradd -m` during `sync_admin_user_to_group`.
    pub async fn create_admin_home_directory(&self, username: &str) -> Result<(), DbError> {
        let home_dir = crate::config::paths::user_home_path(username)
            .to_string_lossy()
            .into_owned();

        let home_exists = tokio::fs::metadata(&home_dir).await.is_ok();
        if !home_exists {
            tracing::warn!(
                "Home directory '{}' does not exist. Was the Linux user created?",
                home_dir
            );
            return Ok(());
        }

        // Verify ownership
        let uid_output = tokio::process::Command::new("id")
            .args(["-u", username])
            .output()
            .await
            .map_err(DbError::Io)?;

        if !uid_output.status.success() {
            tracing::warn!("Could not get UID for user '{}'", username);
            return Ok(());
        }

        let gid_output = tokio::process::Command::new("id")
            .args(["-g", username])
            .output()
            .await
            .map_err(DbError::Io)?;

        if !gid_output.status.success() {
            tracing::warn!("Could not get GID for user '{}'", username);
            return Ok(());
        }

        let uid_str = String::from_utf8_lossy(&uid_output.stdout)
            .trim()
            .to_string();
        let gid_str = String::from_utf8_lossy(&gid_output.stdout)
            .trim()
            .to_string();

        // Ensure ownership is correct
        let output = tokio::process::Command::new("chown")
            .args(["-R", &format!("{}:{}", uid_str, gid_str), &home_dir])
            .output()
            .await
            .map_err(DbError::Io)?;

        if output.status.success() {
            tracing::info!("Verified home directory ownership: {}", home_dir);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("Failed to set ownership on '{}': {}", home_dir, stderr);
        }

        Ok(())
    }
}
